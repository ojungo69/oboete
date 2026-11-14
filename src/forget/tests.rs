    use super::*;
    use crate::raw::{self, Item};
    use std::collections::HashMap;

    /// D5 (#439): a forget registered by another process while a preview reads waits for the
    /// preview: its sample is the one read before the commit.
    #[test]
    fn a_forget_registered_inside_a_preview_waits_for_it() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let (h, u) = (home.clone(), uid.clone());
        let (sent, started) = std::sync::mpsc::channel();
        SAMPLING.set(Some(Box::new(move || {
            sent.send(registering(&h, &u)).unwrap();
        })));
        let p = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert!(p.sample.as_deref().is_some_and(|s| s.contains("Use tabs.")));
        started.recv().unwrap().join().unwrap();
        assert!(preview(&home, Target::parse_uid(&uid).unwrap()).is_err());
        // A record's preview holds it too (Codex on #446).
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let seq = native(&mut raw, "a", r#"{"prompt":"a synthetic canary"}"#);
        let (h, target) = (home.path().to_owned(), record(&raw, seq));
        let (sent, started) = std::sync::mpsc::channel();
        let registered = target.clone();
        SAMPLING.set(Some(Box::new(move || {
            sent.send(registering_target(&h, registered)).unwrap();
        })));
        let p = preview(home.path(), target).unwrap();
        assert!(p.sample.as_deref().is_some_and(|s| s.contains("canary")));
        started.recv().unwrap().join().unwrap();
        assert!(!shown(&raw, seq));
    }

    #[test]
    fn bounded_log_lines_keep_crlf_the_cap_and_a_final_request() {
        let request = Request {
            v: 1,
            home: "synthetic".into(),
            job: "a".repeat(32),
            started: 0,
            target: Target::Record {
                device: "synthetic".into(),
                seq: 1,
            },
            records: vec![Record {
                device: "synthetic".into(),
                seq: 1,
                origin: origin("synthetic", "bounded-record"),
                session: session("claude", "bounded-session"),
                ts: None,
            }],
        };
        let line = request.line().unwrap();
        let mut input = vec![b'\n'];
        input.extend(std::iter::repeat_n(b' ', MAX_LINE - line.len()));
        input.extend(line.as_bytes());
        input.extend(b"\r\n");
        input.extend(std::iter::repeat_n(b'x', MAX_LINE + 1));
        input.extend(b"\n\xff\n");
        input.extend(line.as_bytes());
        let mut report = Report::default();
        let read = read_lines(
            std::io::Cursor::new(input),
            Path::new("synthetic.log"),
            Some("synthetic"),
            &mut report,
        )
        .unwrap();
        assert_eq!(read, vec![request.clone(), request]);
        assert_eq!(report.problems.len(), 2);
        assert!(report.problems[0].contains("line 3") && report.problems[0].contains("cap"));
        assert!(report.problems[1].contains("line 4"));
    }

    /// An imported record of origin `id`, as an importer appends it.
    fn native(raw: &mut raw::Raw, id: &str, body: &str) -> i64 {
        let mut event = raw::test_event(body);
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        raw.append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            &[identity],
            "",
            None,
        )
        .unwrap()
        .first()
        .copied()
        .unwrap_or(0)
    }

    /// A hook's record of `body` at `ts` in `session` (slice 3a).
    fn live(raw: &mut raw::Raw, session: &str, ts: i64, body: &str) -> i64 {
        let mut event = raw::test_event(body);
        event.session = session.into();
        event.ts = ts;
        raw.append(&event).unwrap()
    }

    /// A record of `source` (`transcript` or `oboete-v1`) and origin `id` in `session`, as its
    /// importer appends it; `ambiguous` as a repeated transcript event is.
    fn imported(
        raw: &mut raw::Raw,
        source: &str,
        session: &str,
        id: &str,
        ambiguous: bool,
        body: &str,
    ) -> i64 {
        let mut event = raw::test_event(body);
        event.source = source.into();
        event.session = session.into();
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: super::session(&event.agent, session),
            ambiguous: ambiguous.then(|| origin("synthetic", &format!("{id}:0"))),
            unverified: false,
        };
        raw.append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            &[identity],
            "",
            None,
        )
        .unwrap()[0]
    }

    fn seqs(records: &[Record]) -> Vec<i64> {
        records.iter().map(|r| r.seq).collect()
    }

    fn record(raw: &raw::Raw, seq: i64) -> Target {
        Target::Record {
            device: raw.device().into(),
            seq,
        }
    }

    fn shown(raw: &raw::Raw, seq: i64) -> bool {
        raw.after(raw.device(), seq - 1, 1)
            .unwrap()
            .first()
            .is_some_and(|r| r.seq == seq && matches!(r.item, Item::Event(_)))
    }

    /// Rule 1: one raw transaction hides the record, denies its origin and keeps the job; rule 5:
    /// an import of the same origin is not recorded again, another origin is.
    #[test]
    fn a_forget_hides_the_record_and_its_origin_is_not_imported_again() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let seq = native(&mut raw, "a", r#"{"prompt":"a synthetic canary"}"#);
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        assert_eq!(p.count(), 1);
        let (status, report) = start(home.path(), &p).unwrap();
        assert_eq!(status.records, 1);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(!shown(&raw, seq));
        assert_eq!(
            native(&mut raw, "a", r#"{"prompt":"a synthetic canary"}"#),
            0
        );
        let other = native(&mut raw, "b", r#"{"prompt":"another record"}"#);
        assert!(shown(&raw, other));
        assert_eq!(raw.forget_requests().unwrap().len(), 1);
        let log = std::fs::read_to_string(home.path().join(LOG)).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert!(!log.contains("canary"), "{log}");
    }

    /// Rule 5, read through docs/provenance.md P1: a claim that quotes a forgotten record, or a
    /// correction anchored on one, is never recorded; quoting or anchored on another record, each
    /// is.
    #[test]
    fn what_names_a_forgotten_record_is_not_recorded() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let gone = native(&mut raw, "a", r#"{"prompt":"a synthetic canary"}"#);
        let kept = native(&mut raw, "b", r#"{"prompt":"another record"}"#);
        let device = raw.device().to_owned();
        let p = raw.forget_preview(record(&raw, gone)).unwrap();
        start(home.path(), &p).unwrap();
        let correction = |seq: i64| {
            let body = serde_json::json!({"uid": "a".repeat(64),
                "anchor": {"device": device, "seq": seq}, "status": "done"});
            (raw::OpKind::Correction, body)
        };
        let claim = |seq: i64| claim_op(&device, &[(seq, "a synthetic canary")], "Claimed.");
        for op in [claim(gone), correction(gone)] {
            let refused = raw.append_ops(&[op]).unwrap_err();
            assert!(refused.to_string().contains("invalidated"), "{refused}");
        }
        for op in [claim(kept), correction(kept)] {
            assert_eq!(raw.append_ops(&[op]).unwrap().len(), 1);
        }
    }

    /// D5: a uid's preview is read from knowledge.db: a claim's ops and the claims it supersedes,
    /// a document's ops and title, no raw record. What names no claim or document here, and a
    /// record's, a card's or a summary's id, is refused before anything is registered.
    #[test]
    fn a_uid_preview_counts_its_ops_and_refuses_what_names_no_uid() {
        let mut s = crate::search::b::fixture::Store::new();
        let older = s.decided("github.com/o/r", 1_000, "Use spaces.", &[]);
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[&older]);
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        s.run();
        let home = s.home.path();
        let p = preview(home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert_eq!((p.count(), p.sample.as_deref()), (0, Some("Use tabs.")));
        let again = vec!["Use spaces.".to_owned()];
        let shown = UidPreview {
            kind: "claim",
            ops: 1,
            vectors: 0,
            again,
            // The worker's pass backed its op up in one segment.
            segments: Some(crate::backup::OpsPurge {
                live: 1,
                ..Default::default()
            }),
            copies: 0,
        };
        assert_eq!(p.uid, Some(shown));
        p.validate().unwrap();
        let p = preview(home, Target::parse_uid(&doc).unwrap()).unwrap();
        let u = p.uid.as_ref().unwrap();
        assert_eq!(
            (u.kind, u.ops, p.sample.as_deref()),
            ("document", 1, Some("Deploy notes"))
        );
        let device = s.raw.device();
        let e = Target::parse_uid(&format!("{device}:1")).unwrap_err();
        assert!(e.to_string().contains("--record"), "{e}");
        for id in [
            format!("{device}.4.0"),
            "4.0".into(),
            format!("S{device}.4"),
        ] {
            let e = Target::parse_uid(&id).unwrap_err();
            assert!(e.to_string().contains("card"), "{id}: {e}");
        }
        let e = preview(home, Target::parse_uid(&"a".repeat(64)).unwrap()).unwrap_err();
        assert!(e.to_string().contains("neither"), "{e}");
        assert!(Target::parse_uid("a b").is_err());
        assert!(
            raw::open(home)
                .unwrap()
                .forget_requests()
                .unwrap()
                .is_empty()
        );
        // A claim it supersedes that is forgotten already comes back as nothing (Codex's
        // adversarial review of slice 2a).
        start(
            home,
            &preview(home, Target::parse_uid(&older).unwrap()).unwrap(),
        )
        .unwrap();
        let p = preview(home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert_eq!(p.uid.map(|u| u.again), Some(Vec::new()));
    }

    /// D5 (Codex on #435): a uid whose ops no consumer has read yet, right after an import or
    /// while the worker lags, is found in the op log and forgotten; once the consumers read them,
    /// search still passes it over.
    #[test]
    fn a_uid_knowledge_has_not_read_yet_is_forgotten_from_the_op_log() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs for the parser.", &[]);
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Deploy with the parser.");
        let home = s.home.path().to_owned();
        let p = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        let shown = p.uid.as_ref().map(|u| (u.kind, u.ops));
        assert_eq!(
            (shown, p.sample.as_deref()),
            (Some(("claim", 1)), Some("Use tabs for the parser."))
        );
        start(&home, &p).unwrap();
        let p = preview(&home, Target::parse_uid(&doc).unwrap()).unwrap();
        let shown = p.uid.as_ref().map(|u| (u.kind, u.ops));
        assert_eq!(
            (shown, p.sample.as_deref()),
            (Some(("document", 1)), Some("Deploy notes"))
        );
        start(&home, &p).unwrap();
        let e = preview(&home, Target::parse_uid(&"b".repeat(64)).unwrap()).unwrap_err();
        assert!(e.to_string().contains("neither"), "{e}");
        s.run();
        let q = crate::search::b::Query {
            text: "parser".into(),
            all: true,
            limit: 20,
            ..Default::default()
        };
        let found: Vec<String> = s.query(&q).hits.into_iter().map(|h| h.key).collect();
        assert!(!found.contains(&uid) && !found.contains(&doc));
    }

    /// D5 (Codex and CodeRabbit on #435): the op log gives the preview its count while knowledge.db
    /// lags it, and a uid of 64 hex digits that no claim has is looked for among the imports.
    #[test]
    fn the_preview_counts_what_the_op_log_holds() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs for the parser.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let ops = |uid: &str| {
            preview(&home, Target::parse_uid(uid).unwrap())
                .unwrap()
                .uid
                .map(|u| (u.kind, u.ops))
        };
        assert_eq!(ops(&uid), Some(("claim", 1)));
        // An owner's change no consumer has read yet.
        let change = serde_json::json!({"uid": uid, "muted": true});
        s.raw
            .append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        assert_eq!(ops(&uid), Some(("claim", 2)));
        let hex = "c".repeat(64);
        s.raw
            .append_imports(vec![raw::ImportDoc {
                uid: hex.clone(),
                source: "claude-mem:test".into(),
                source_id: "h".into(),
                kind: "decision".into(),
                repo: crate::import::repo("r"),
                session: "cm".into(),
                ts: 3_000,
                title: "Hex notes".into(),
                body: "Hex.".into(),
            }])
            .unwrap();
        assert_eq!(ops(&hex), Some(("document", 1)));
    }

    /// Every op raw.db holds: (device, op_seq, type, body), in key order.
    fn op_rows(home: &Path) -> Vec<(String, i64, String, String)> {
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        let mut st = conn
            .prepare("SELECT device, op_seq, type, body FROM ops ORDER BY device, op_seq")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// Each forget job's (target uid, job id, step).
    fn uid_jobs(home: &Path) -> Vec<(String, String, i64)> {
        raw::open(home)
            .unwrap()
            .forget_jobs()
            .unwrap()
            .into_iter()
            .filter_map(|(r, step)| match r.target {
                Target::Uid { uid } => Some((uid, r.job, step)),
                _ => None,
            })
            .collect()
    }

    /// D5 (2b-1): a purge rewrites what holds a forgotten uid's text, its claim's Claim op and the
    /// Correction that names it, or a document's Import op, to `{"forgotten": job}` with its
    /// type, op_seq and batch kept; every other op stays byte for byte; steps 2 and 3 are recorded,
    /// and the next purge finds nothing to do.
    #[test]
    fn a_purge_rewrites_the_forgotten_uids_ops_and_no_other() {
        let mut s = crate::search::b::fixture::Store::new();
        let older = s.decided("github.com/o/r", 1_000, "Use spaces.", &[]);
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[&older]);
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        let change = serde_json::json!({"uid": uid, "muted": true});
        s.raw
            .append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let before = op_rows(&home);
        for u in [&uid, &doc] {
            start(
                &home,
                &preview(&home, Target::parse_uid(u).unwrap()).unwrap(),
            )
            .unwrap();
        }
        let jobs = uid_jobs(&home);
        let job = |u: &str| jobs.iter().find(|j| j.0 == u).unwrap().1.clone();
        // A rebuild wants raw.db open nowhere else.
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops, purged.rebuilt), (2, 3, true));
        assert!(purged.unfinished.is_empty());
        let after = op_rows(&home);
        let mut rewritten = 0;
        for (device, seq, kind, body) in &before {
            let now = after
                .iter()
                .find(|a| (&a.0, a.1) == (device, *seq))
                .unwrap();
            assert_eq!(&now.2, kind);
            let owner = match kind.as_str() {
                "claim" if body.contains("Use tabs.") => Some(&uid),
                "correction" => Some(&uid),
                "import" => Some(&doc),
                _ => None,
            };
            match owner {
                Some(u) => {
                    let expected = serde_json::json!({ "forgotten": job(u) });
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&now.3).unwrap(),
                        expected
                    );
                    rewritten += 1;
                }
                None => assert_eq!(&now.3, body, "op {seq} changed"),
            }
        }
        assert_eq!(rewritten, 3);
        assert!(uid_jobs(&home).iter().all(|j| j.2 == 5));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// D5 (2b-1): ops a purge rewrote derive nothing, read by a pass that had not reached them and
    /// by a rebuild: the worker goes on, a rewritten correction is passed for good, and after the
    /// rebuild the uids keep no claim, derivation, correction or imported row and the claim the
    /// forgotten one superseded is current again.
    #[test]
    fn rewritten_ops_derive_nothing_and_what_they_superseded_is_current_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let older = s.decided("github.com/o/r", 1_000, "Use spaces.", &[]);
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[&older]);
        s.run();
        let home = s.home.path().to_owned();
        let k = |home: &Path| crate::knowledge::open(home).unwrap();
        // What supersedes `older`: with the forgotten claim gone, nothing, and it is current again.
        let superseded = |home: &Path| -> i64 {
            k(home)
                .query_row(
                    "SELECT COUNT(*) FROM edges WHERE to_uid = ?1 AND type = 'supersedes'",
                    [&older],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(superseded(&home), 1);
        // Not read by any consumer yet when they are purged.
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        let change = serde_json::json!({"uid": uid, "muted": true});
        s.raw
            .append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        for u in [&uid, &doc] {
            start(
                &home,
                &preview(&home, Target::parse_uid(u).unwrap()).unwrap(),
            )
            .unwrap();
        }
        // Rewritten as a purge's step 2 rewrites them, before any consumer has read them.
        let rewritten: usize = uid_jobs(&home)
            .iter()
            .map(|(u, job, _)| s.raw.purge_uid(u, job).unwrap())
            .sum();
        assert_eq!(rewritten, 3);
        let skips = |home: &Path| -> Vec<String> {
            k(home)
                .prepare("SELECT reason FROM claim_skips ORDER BY op_seq")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let held = |home: &Path| -> i64 {
            k(home)
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM claims WHERE uid = ?1)
                          + (SELECT COUNT(*) FROM derivations WHERE uid = ?1)
                          + (SELECT COUNT(*) FROM corrections WHERE uid = ?1)
                          + (SELECT COUNT(*) FROM imported WHERE uid = ?2)",
                    [&uid, &doc],
                    |r| r.get(0),
                )
                .unwrap()
        };
        s.run();
        assert_eq!(skips(&home), ["forgotten"]);
        let imported: i64 = k(&home)
            .query_row("SELECT COUNT(*) FROM imported", [], |r| r.get(0))
            .unwrap();
        assert_eq!(imported, 0);
        // A rebuild wants raw.db open nowhere else.
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::worker::rebuild(&home).unwrap();
        assert_eq!(held(&home), 0);
        assert_eq!(superseded(&home), 0);
        assert_eq!(skips(&home), ["forgotten", "forgotten"]);
    }

    /// A text only a forgotten record holds (slice 3c).
    const RECORD_CANARY: &str = "rcanary-5d21";

    /// Every store file of `home` (raw.db, knowledge.db and their WALs) that holds `bytes`.
    fn store_hits(home: &Path, bytes: &[u8]) -> Vec<&'static str> {
        ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"]
            .into_iter()
            .filter(|f| {
                std::fs::read(home.join(f))
                    .is_ok_and(|b| b.windows(bytes.len()).any(|w| w == bytes))
            })
            .collect()
    }

    /// A records row's type, labels and body, as the purge leaves them.
    fn row(
        home: &Path,
        device: &str,
        seq: i64,
    ) -> (String, i64, String, String, Option<String>, bool) {
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .query_row(
                "SELECT type, ts, source, kind, session, body IS NULL FROM records
                 WHERE device = ?1 AND seq = ?2",
                rusqlite::params![device, seq],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap()
    }

    /// Slice 3c, test 1: a forgotten record's text, a short body and one that overflows its page,
    /// leaves no byte in raw.db, knowledge.db or their WALs once purged; its row keeps device,
    /// seq, ts, source and kind, and loses its labels, body and ledger rows; status says what is
    /// left; a second purge has nothing to do, and the requests applied again add no tombstone. A
    /// restore that gives a row its text back is purged again.
    #[test]
    fn a_purged_records_text_leaves_no_byte_in_the_stores() {
        let mut s = crate::search::b::fixture::Store::new();
        let short = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Remember {RECORD_CANARY}."),
        );
        let long = s.said(
            "s",
            "github.com/o/r",
            2_000,
            &format!("{} {RECORD_CANARY}", "long ".repeat(2_000)),
        );
        let kept = s.said("s", "github.com/o/r", 3_000, "Kept text.");
        s.run();
        let home = s.home.path().to_owned();
        let device = s.raw.device().to_owned();
        let crate::search::b::fixture::Store { home: _home, raw } = s;
        drop(raw);
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, '/prompt', 'r', 0, 4, 0, 'v')",
                rusqlite::params![device, short],
            )
            .unwrap();
        for seq in [short, long] {
            let target = Target::Record {
                device: device.clone(),
                seq,
            };
            start(&home, &preview(&home, target).unwrap()).unwrap();
        }
        let raw = crate::raw::open(&home).unwrap();
        assert!(
            raw.forgotten_record_keys()
                .unwrap()
                .contains(&format!("{device}:{short}"))
        );
        let tombstones = |raw: &crate::raw::Raw| raw.after(raw.device(), 0, 100).unwrap().len();
        let before = tombstones(&raw);
        drop(raw);
        assert!(!store_hits(&home, RECORD_CANARY.as_bytes()).is_empty());
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.records, purged.rebuilt), (2, 2, true));
        assert_eq!(
            store_hits(&home, RECORD_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        for seq in [short, long] {
            let (kind, ts, source, event, session, empty) = row(&home, &device, seq);
            assert_eq!(
                (kind.as_str(), source.as_str(), event.as_str()),
                ("removed", "hook", "prompt")
            );
            assert!(ts > 0 && session.is_none() && empty, "{seq}");
        }
        assert_eq!(row(&home, &device, kept).0, "event");
        let ledger: i64 = rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM ledger", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ledger, 0);
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
        let again = purge(&home).unwrap().unwrap();
        assert_eq!((again.jobs, again.records, again.rebuilt), (0, 0, false));
        // A restore from a backup made before the purge gives a row its text back, its job past
        // step 3: the state decides, and it is purged again.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE records SET type = 'event', body = ?3 WHERE device = ?1 AND seq = ?2",
                rusqlite::params![device, short, format!(r#"{{"prompt":"{RECORD_CANARY}"}}"#)],
            )
            .unwrap();
        assert!(status(&home).unwrap().iter().any(|j| j.local == PENDING));
        let back = purge(&home).unwrap().unwrap();
        assert_eq!((back.jobs, back.records, back.rebuilt), (1, 1, true));
        assert_eq!(
            store_hits(&home, RECORD_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
        // The requests applied again find the same records and add no tombstone.
        let mut raw = crate::raw::open(&home).unwrap();
        let requests = raw.forget_requests().unwrap();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute_batch("DELETE FROM forget_jobs")
            .unwrap();
        raw.forget_apply(&requests).unwrap();
        assert_eq!(tombstones(&raw), before);
    }

    /// Codex on #469: raw.db and both request logs lost, the backup restored. A segment from
    /// before the forget brings the records back with their text, a later one their denials, not
    /// their jobs: the purge finds them by the state, a live record by its identity linked again,
    /// and leaves no byte of their text in the stores.
    #[test]
    fn a_restore_from_segments_alone_has_its_forgotten_text_purged() {
        let (_home, home, device, _, kept) = restored_without_jobs();
        assert!(!store_hits(&home, RECORD_CANARY.as_bytes()).is_empty());
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.records, purged.rebuilt), (2, true));
        assert_eq!(
            store_hits(&home, RECORD_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        // Slice 3c-2: and the segment the restore read, with no job to owe it.
        assert_eq!(purged.segments, 1);
        assert_eq!(backup_hits(&home, RECORD_CANARY), Vec::<String>::new());
        for seq in kept {
            assert_eq!(row(&home, &device, seq).0, "event");
        }
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// `backed_up_records`' two forgotten, their tombstones exported after them, then raw.db and
    /// both request logs lost and the backup restored: their denials are back, not their jobs.
    fn restored_without_jobs() -> (tempfile::TempDir, PathBuf, String, [i64; 2], [i64; 2]) {
        let (s, device, forgotten, kept) = backed_up_records();
        let home = s.home.path().to_owned();
        let guard = forget_records(s, &device, forgotten);
        crate::backup::export(&home).unwrap();
        for f in ["forget.log", "raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        std::fs::remove_file(crate::backup::dir(&home).unwrap().join(LOG)).unwrap();
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        let raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forget_jobs().unwrap().is_empty());
        assert_eq!(records_due(&raw).unwrap().len(), 2);
        (guard, home, device, forgotten, kept)
    }

    /// CodeRabbit on #694: a restore from segments alone brings back the denials with no job row,
    /// and the forgotten records' text: the readers go by the tombstones (docs/my-work-model.md S5)
    /// until a purge takes the text again.
    #[test]
    fn a_denial_whose_text_came_back_with_no_job_keeps_the_backstop() {
        let (_kept, home, _, _, _) = restored_without_jobs();
        assert!(raw::open(&home).unwrap().forget_unrebuilt().unwrap());
        purge(&home).unwrap().unwrap();
        assert!(!raw::open(&home).unwrap().forget_unrebuilt().unwrap());
    }

    /// Codex on #470: a purge of denials with no job row stopped inside a record segment's
    /// replacement leaves the backups owed in raw.db: a worker starts a continuation for them, the
    /// next purge, with no job and no row holding text, rewrites them, and the one after has
    /// nothing to do.
    #[test]
    fn a_purge_stopped_before_its_backups_owes_them_with_no_job() {
        let (_home, home, _, _, _) = restored_without_jobs();
        crate::backup::FAIL_AFTER.set(Some("purge_segment_written"));
        assert!(purge(&home).is_err());
        assert_eq!(
            store_hits(&home, RECORD_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        assert!(!backup_hits(&home, RECORD_CANARY).is_empty());
        let raw = crate::raw::open(&home).unwrap();
        assert!(records_due(&raw).unwrap().is_empty());
        assert!(raw.record_backups_owed().unwrap());
        let before = CONTINUED.get();
        continue_due(&home, &raw);
        assert_eq!(CONTINUED.get(), before + 1);
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert!(!purged.rebuilt);
        assert_eq!(backup_hits(&home, RECORD_CANARY), Vec::<String>::new());
        assert!(
            !crate::raw::open(&home)
                .unwrap()
                .record_backups_owed()
                .unwrap()
        );
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// A text only the claims made from a forgotten record hold (slice 3d-1).
    const CLAIM_CANARY: &str = "ccanary-3f19";

    /// A decided decision op quoting each of `quotes` (record seq and its text, whole), in order,
    /// with `body`: its uid is its kind and its first quote's sentence.
    fn claim_op(
        device: &str,
        quotes: &[(i64, &str)],
        body: &str,
    ) -> (raw::OpKind, serde_json::Value) {
        let evidence: Vec<crate::claims::Evidence> = quotes
            .iter()
            .map(|(seq, text)| crate::claims::Evidence {
                device: device.into(),
                seq: *seq,
                offset: 0,
                length: text.len() as i64,
                sentence: 0,
                quote: (*text).into(),
                claim_at: None,
            })
            .collect();
        let op = crate::claims::ClaimOp {
            id: "c".into(),
            kind: "decision".into(),
            status: "decided".into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: body.into(),
            evidence,
            supersedes: Vec::new(),
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
            support: Vec::new(),
        };
        (raw::OpKind::Claim, serde_json::to_value(op).unwrap())
    }

    /// Slice 3d-1: step 2 forgets the claims made from a forgotten record before it rewrites the
    /// record's row: a purge stopped between them has tombstoned their uids already.
    #[test]
    fn a_record_purge_forgets_its_claims_before_its_rows() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {CLAIM_CANARY} mode.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let op = claim_op(&device, &[(r1, &gone)], &gone);
        let uid = crate::claims::op_uid(&op.1).unwrap();
        s.raw.append_ops(&[op]).unwrap();
        let home = s.home.path().to_owned();
        // Counted from raw's op log, before a worker read the op (CodeRabbit on #471).
        let shown = preview(&home, Target::Record { device, seq: r1 }).unwrap();
        assert_eq!(shown.claims, 1);
        s.run();
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::backup::FAIL_AFTER.set(Some("record_claims_purged"));
        assert!(purge(&home).is_err());
        let raw = crate::raw::open(&home).unwrap();
        assert!(raw.forgotten(&uid).unwrap() && !raw.uid_has_ops(&uid).unwrap());
        assert!(!raw.record_jobs_holding().unwrap().is_empty());
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert_eq!(
            store_hits(&home, CLAIM_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        // A job 3c-2 left at step 4, with nothing held, goes on to step 5 and is done.
        let raw = crate::raw::open(&home).unwrap();
        let job = raw.forget_jobs().unwrap()[0].0.job.clone();
        raw.restart_forget_step(&job, 4).unwrap();
        assert_eq!(records_due(&raw).unwrap().len(), 1);
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert!(
            records_due(&crate::raw::open(&home).unwrap())
                .unwrap()
                .is_empty()
        );
    }

    /// A claim of a forgotten record, its record's text holding `CLAIM_CANARY`, consumed and
    /// backed up, a session shown it, and the record's forget registered: its home, its uid and
    /// the shown value.
    fn claim_shown_and_forgotten() -> (tempfile::TempDir, PathBuf, String, String) {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {CLAIM_CANARY} mode.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let op = claim_op(&device, &[(r1, &gone)], &gone);
        let uid = crate::claims::op_uid(&op.1).unwrap();
        s.raw.append_ops(&[op]).unwrap();
        s.run();
        let home = s.home.path().to_owned();
        crate::backup::export(&home).unwrap();
        let shown = format!(r#"{{"{uid}":{{"fp":"f","body":true}}}}"#);
        let value = shown.clone();
        crate::hookstate::update(&home, "claude", "s", "shown", |_| Some(value)).unwrap();
        start(
            &home,
            &preview(&home, Target::Record { device, seq: r1 }).unwrap(),
        )
        .unwrap();
        let crate::search::b::fixture::Store { home: kept, raw } = s;
        drop(raw);
        (kept, home, uid, shown)
    }

    /// CodeRabbit on #471: a purge whose step 4 stops (a backup segment not rewritten) has taken
    /// the claims made from the forgotten record out of every session's shown value already,
    /// after its rebuild; the next purge finishes.
    #[test]
    fn a_purge_stopped_in_its_backups_has_taken_the_claims_out_of_shown() {
        let (_kept, home, uid, _) = claim_shown_and_forgotten();
        crate::backup::FAIL_AFTER.set(Some("purge_segment_written"));
        assert!(purge(&home).is_err());
        let left = crate::hookstate::value(&home, "claude", "s", "shown").unwrap();
        assert!(!left.contains(&uid), "{left}");
        purge(&home).unwrap().unwrap();
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
    }

    /// Codex on #471: after a restore from segments alone (its denials, no job row), a purge
    /// stopped before step 5 leaves the backups owed in raw.db, so the next purge, with no job,
    /// still takes the claim made from the forgotten record out of a shown value a hook wrote
    /// after the rebuild; the one after has nothing to do.
    #[test]
    fn a_purge_of_denials_with_no_job_owes_step_5_until_it_is_done() {
        let (_kept, home, uid, shown) = claim_shown_and_forgotten();
        crate::backup::export(&home).unwrap();
        for f in ["forget.log", "raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        std::fs::remove_file(crate::backup::dir(&home).unwrap().join(LOG)).unwrap();
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        let raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forget_jobs().unwrap().is_empty());
        drop(raw);
        let at = home.clone();
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                crate::hookstate::update(&at, "claude", "s", "shown", |_| Some(shown)).unwrap();
            }));
        });
        crate::backup::FAIL_AFTER.set(Some("backups_purged"));
        assert!(purge(&home).is_err());
        assert!(
            crate::raw::open(&home)
                .unwrap()
                .record_backups_owed()
                .unwrap()
        );
        let value = || crate::hookstate::value(&home, "claude", "s", "shown").unwrap();
        assert!(value().contains(&uid));
        purge(&home).unwrap().unwrap();
        assert!(!value().contains(&uid), "{}", value());
        assert!(
            !crate::raw::open(&home)
                .unwrap()
                .record_backups_owed()
                .unwrap()
        );
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// Codex on #471: after a restore from segments alone, a claim made from the forgotten record
    /// whose uid a uid job's forget op tombstones already (its job row gone with the restore, the
    /// claim's text back) is bound to the record's job by a forget op of its own, so step 4
    /// rewrites its ops in the backup's segments, and the purges after it know the uid.
    #[test]
    fn a_claim_a_uid_job_forgot_is_bound_to_the_record_job_too() {
        let (_kept, home, uid, _) = claim_shown_and_forgotten();
        forget(&home, &uid);
        crate::backup::export(&home).unwrap();
        for f in ["forget.log", "raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        std::fs::remove_file(crate::backup::dir(&home).unwrap().join(LOG)).unwrap();
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        let raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forget_jobs().unwrap().is_empty());
        assert!(raw.forgotten_set().unwrap().contains(&uid));
        assert!(raw.record_derived_uids().unwrap().is_empty());
        let job: String = rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .query_row("SELECT job FROM denied_records", [], |r| r.get(0))
            .unwrap();
        drop(raw);
        assert!(!backup_hits(&home, CLAIM_CANARY).is_empty());
        purge(&home).unwrap().unwrap();
        assert_eq!(backup_hits(&home, CLAIM_CANARY), Vec::<String>::new());
        assert_eq!(
            crate::raw::open(&home)
                .unwrap()
                .record_derived_uids()
                .unwrap(),
            std::collections::HashMap::from([(uid, job)])
        );
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// Slice 3d-1: the claims quoting a forgotten record, alone or beside a kept one, are forgotten
    /// by their uids with it, every op of theirs (another derivation, one quoting the kept record
    /// only, a correction) without its text, in raw.db, knowledge.db, the ops segments, a copy of
    /// raw.db and the sessions' shown values; a claim of the kept record alone stays. The preview
    /// counts them, the status says so, a second purge has nothing to do, a claim of a forgotten
    /// uid is refused, and an op of theirs whose text a restore brought back is purged again.
    #[test]
    fn a_forgotten_records_claims_are_forgotten_by_uid_with_it() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {CLAIM_CANARY} mode.");
        let kept = "Deploys run on Fridays.";
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, kept);
        let alone = claim_op(&device, &[(r1, &gone)], &gone);
        let again = claim_op(&device, &[(r1, &gone)], &format!("Again: {gone}"));
        let partial = claim_op(
            &device,
            &[(r2, kept), (r1, &gone)],
            &format!("Fridays, in {CLAIM_CANARY} mode."),
        );
        // The partial claim's uid derived again from the kept record alone: its active derivation,
        // which the anchors consumer keeps.
        let later = claim_op(&device, &[(r2, kept)], &format!("Fridays, {CLAIM_CANARY}."));
        // Another kind: its uid is not the partial claim's, whose first quote is the same.
        let mut other = claim_op(&device, &[(r2, kept)], kept);
        other.1["kind"] = "preference".into();
        let uid = |op: &(raw::OpKind, serde_json::Value)| crate::claims::op_uid(&op.1).unwrap();
        let (a, b, c) = (uid(&alone), uid(&partial), uid(&other));
        assert_eq!(uid(&later), b);
        s.raw
            .append_ops(&[alone.clone(), again, partial, later, other])
            .unwrap();
        s.run();
        s.correct(&a, None, Some(&format!("Corrected: {CLAIM_CANARY}.")));
        s.run();
        crate::backup::export(s.home.path()).unwrap();
        let home = s.home.path().to_owned();
        let correction: String = rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .query_row("SELECT body FROM ops WHERE type = 'correction'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let entry = format!(r#"{{"fp":"f","body":"{gone}"}}"#);
        let shown_value = format!(r#"{{"{a}":{entry},"{c}":{{"fp":"g","body":"Kept."}}}}"#);
        crate::hookstate::update(&home, "claude", "s", "shown", |_| Some(shown_value)).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        let shown = preview(&home, target).unwrap();
        assert_eq!(shown.claims, 2);
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert!(!store_hits(&home, CLAIM_CANARY.as_bytes()).is_empty());
        let purged = purge(&home).unwrap().unwrap();
        // Four claim ops and the correction; the session shown the first claim.
        assert_eq!((purged.records, purged.ops, purged.shown), (1, 5, 1));
        let left = crate::hookstate::value(&home, "claude", "s", "shown").unwrap();
        assert!(!left.contains(&a) && left.contains(&c));
        assert_eq!(
            store_hits(&home, CLAIM_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        assert_eq!(backup_hits(&home, CLAIM_CANARY), Vec::<String>::new());
        assert!(!holds_text(&copy, CLAIM_CANARY));
        let job = crate::raw::open(&home).unwrap().forget_jobs().unwrap()[0]
            .0
            .job
            .clone();
        let raw = crate::raw::open(&home).unwrap();
        assert_eq!(
            raw.record_derived_uids().unwrap(),
            std::collections::HashMap::from([(a.clone(), job.clone()), (b.clone(), job.clone())])
        );
        assert!(raw.forgotten(&a).unwrap() && raw.forgotten(&b).unwrap());
        assert!(!raw.forgotten(&c).unwrap() && raw.uid_has_ops(&c).unwrap());
        assert!(!raw.uid_has_ops(&a).unwrap() && !raw.uid_has_ops(&b).unwrap());
        drop(raw);
        assert!(
            knowledge_holding(&home, &HashSet::from([a.clone(), b.clone()]))
                .unwrap()
                .is_empty()
        );
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        let mut raw = crate::raw::open(&home).unwrap();
        assert!(raw.append_ops(std::slice::from_ref(&alone)).is_err());
        // Nor is the partial claim's uid derived again from the record kept.
        assert!(
            raw.append_ops(&[claim_op(&device, &[(r2, kept)], kept)])
                .is_err()
        );
        // A restore from older ops segments gives the correction its text back: found by its uid's
        // forget op, as no claim op quotes the record any longer, and purged again.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE ops SET body = ?1 WHERE type = 'correction'",
                [correction],
            )
            .unwrap();
        drop(raw);
        assert!(status(&home).unwrap().iter().any(|j| j.local == PENDING));
        let back = purge(&home).unwrap().unwrap();
        assert_eq!(back.ops, 1);
        assert_eq!(
            store_hits(&home, CLAIM_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
    }

    /// #167, slice 3d-3: the forget of the record a claim's status stood on (the user's acceptance
    /// of the proposal it quotes) leaves the claim a proposal: it holds none of the record's text,
    /// so it is not forgotten with it, and its op stays as it was.
    #[test]
    fn a_claim_whose_acceptance_is_forgotten_is_a_proposal_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let proposal = "Let's keep the parsed files in a cache.";
        let r1 = s.said("s", "github.com/o/r", 1_000, proposal);
        let yes = s.said("s", "github.com/o/r", 2_000, "Yes, go ahead.");
        let mut op = claim_op(&device, &[(r1, proposal)], "Cache the parsed files.");
        op.1["speaker"] = "assistant proposal".into();
        op.1["support"] = serde_json::json!([{"device": device, "seq": yes}]);
        s.raw.append_ops(&[op]).unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let status = || {
            let k = crate::knowledge::open(&home).unwrap();
            let got: Vec<String> = crate::claims::current(&k, "github.com/o/r")
                .unwrap()
                .into_iter()
                .map(|c| c.status)
                .collect();
            got
        };
        assert_eq!(status(), ["decided"]);
        let before = op_bodies(&home);
        let p = preview(&home, Target::Record { device, seq: yes }).unwrap();
        assert_eq!(p.claims, 0);
        start(&home, &p).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert_eq!(status(), ["proposed"]);
        let after = op_bodies(&home);
        assert!(
            before
                .iter()
                .all(|(seq, body)| after.get(seq) == Some(body))
        );
        assert!(
            crate::raw::open(&home)
                .unwrap()
                .record_derived_uids()
                .unwrap()
                .is_empty()
        );
    }

    /// X17 (docs/subagents.md): a forget of a session takes what its own window made, and nothing
    /// of another session's window that holds no more of it than records a curator is shown no
    /// line of (its start, a subagent's stop): that window's card stays shown, its op and claims as
    /// they were.
    #[test]
    fn a_forgotten_sessions_lineless_records_take_nothing_from_another_sessions_window() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let repo = "github.com/o/r";
        let (deploys, tests, hooks) = (
            "Deploys run on Fridays.",
            "Tests run on CI.",
            "Hooks keep secrets out.",
        );
        let a1 = s.said("a", repo, 1_000, deploys);
        s.event("start", "b", (repo, "main"), 1_500, json!({}));
        let stop = s.event(
            raw::SUBAGENT_STOP,
            "b",
            (repo, "main"),
            2_500,
            json!({"agent_id": "a1"}),
        );
        let a2 = s.said("a", repo, 3_000, tests);
        let b1 = s.said("b", repo, 4_000, hooks);
        let mut append = |ops: &[(raw::OpKind, serde_json::Value)]| s.raw.append_ops(ops).unwrap();
        let wa = append(&[
            window_op(a1, a2, "A's window.", json!({})),
            claim_op(&device, &[(a1, deploys)], deploys),
            claim_op(&device, &[(a2, tests)], tests),
        ]);
        let wb = append(&[
            window_op(b1, b1, "B's window.", json!({})),
            claim_op(&device, &[(b1, hooks)], hooks),
        ]);
        s.run();
        let home = s.home.path().to_owned();
        let before = op_bodies(&home);
        let target = session_target(&home, &format!("{device}:{stop}")).unwrap();
        let shown = preview(&home, target).unwrap();
        assert_eq!((shown.claims, shown.cards, shown.summaries), (1, 1, 0));
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let job = crate::raw::open(&home).unwrap().forget_jobs().unwrap()[0]
            .0
            .job
            .clone();
        let after = op_bodies(&home);
        for seq in &wa {
            assert_eq!(after[seq], before[seq], "{seq}");
        }
        assert_ne!(after[&wb[0]], before[&wb[0]]);
        assert_eq!(after[&wb[1]], json!({ "forgotten": job }));
        // A's card is still read; B's is gone with its window.
        let raw = crate::raw::open(&home).unwrap();
        let k = crate::knowledge::open(&home).unwrap();
        let taken: Vec<(i64, bool)> = k
            .prepare(&format!(
                "SELECT {} FROM cards ORDER BY op_seq",
                crate::cards::COLUMNS
            ))
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(1)?, crate::cards::taken_row(r, &raw).unwrap()))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(taken, [(wa[0], false)]);
    }

    /// A text only what curators were shown of a forgotten record holds (slice 3d-2).
    const WINDOW_CANARY: &str = "wcanary-7c2e";

    /// A curated window op over `from` to `to`, its fields as curation writes them and `extra`,
    /// whose summary and card say `text`.
    fn window_op(
        from: i64,
        to: i64,
        text: &str,
        extra: serde_json::Value,
    ) -> (raw::OpKind, serde_json::Value) {
        let mut op = serde_json::json!({"outcome": "curated", "candidates": [], "provider": "p",
            "summary": text, "dropped": [["c9", "its quote is not in the window"]], "lowered": [],
            "from_seq": from, "from_offset": null, "to_seq": to, "to_offset": null, "elided": [],
            "observations": [{"type": "decision", "title": text, "narrative": text}]});
        if let (Some(o), serde_json::Value::Object(x)) = (op.as_object_mut(), extra) {
            o.extend(x);
        }
        (raw::OpKind::Window, op)
    }

    /// A turn op over `from` to `through` that read the cards of `read`, whose summary says `text`.
    fn turn_op(
        from: i64,
        through: i64,
        read: &[(i64, i64)],
        text: &str,
    ) -> (raw::OpKind, serde_json::Value) {
        let op = serde_json::json!({"agent": "claude", "session": "s", "repo": "github.com/o/r",
            "ts": 1, "from": from, "through": through, "read": read, "goals": [], "removed": [],
            "fields": {"request": text, "learned": text}, "skipped": false, "excluded": false});
        (raw::OpKind::Turn, op)
    }

    /// Each op's body, by op seq.
    fn op_bodies(home: &Path) -> HashMap<i64, serde_json::Value> {
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .prepare("SELECT op_seq, body FROM ops")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .map(|r| {
                let (seq, body) = r.unwrap();
                (seq, serde_json::from_str(&body).unwrap())
            })
            .collect()
    }

    /// docs/my-work-model.md S5: a forget takes a My Work op that was shown a forgotten record,
    /// and one built on an op it took, in op order: its job, subject and what it read stay, its
    /// fields go, marked with the forget's job, in raw.db, knowledge.db and the backup's ops. One
    /// that read neither keeps its text.
    #[test]
    fn a_forget_takes_the_my_work_ops_that_read_what_it_took() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Parsers stay in {WINDOW_CANARY} mode."),
        );
        let r2 = s.said("s", "github.com/o/r", 2_000, "Deploys run on Fridays.");
        let op = |about: &str, spans: &[(i64, i64)], ops: &[i64], text: &str| {
            let body = json!({"job": "review", "about": about, "entry": "anthropic", "ts": 5_000,
                "read": {"spans": spans, "ops": ops}, "removed": [],
                "fields": {"summary": text}, "skipped": false});
            (crate::raw::OpKind::MyWork, body)
        };
        let read = op("R1", &[(r1, r1)], &[], &format!("Read: {WINDOW_CANARY}."));
        let first = s.raw.append_ops(&[read]).unwrap()[0];
        let card = window_op(r1, r1, &format!("Its card: {WINDOW_CANARY}."), json!({}));
        let window = s.raw.append_ops(&[card]).unwrap()[0];
        let built = op(
            "R2",
            &[(r2, r2)],
            &[first],
            &format!("On it: {WINDOW_CANARY}."),
        );
        let carded = op(
            "R4",
            &[(r2, r2)],
            &[window],
            &format!("Its card: {WINDOW_CANARY}."),
        );
        let kept = op("R3", &[(r2, r2)], &[], "Kept: it read the kept record.");
        let rest = s.raw.append_ops(&[built, carded, kept]).unwrap();
        let home = s.home.path().to_owned();
        s.run();
        crate::backup::export(&home).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        let before = op_bodies(&home);
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        let shown = preview(&home, target).unwrap();
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        // The window op, and three My Work ops: the one shown the record, the one built on it,
        // the one that read the window.
        assert_eq!((purged.records, purged.ops), (1, 4));
        let job = crate::raw::open(&home).unwrap().forget_jobs().unwrap()[0]
            .0
            .job
            .clone();
        let after = op_bodies(&home);
        for seq in [first, rest[0], rest[1]] {
            let gone = crate::raw::forgotten_my_work_body(&before[&seq], &job);
            assert_eq!(after[&seq], gone, "{seq}");
        }
        assert_eq!(after[&rest[2]], before[&rest[2]]);
        assert_eq!(backup_hits(&home, WINDOW_CANARY), Vec::<String>::new());
        assert!(!holds_text(&copy, WINDOW_CANARY));
        assert_eq!(
            store_hits(&home, WINDOW_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        let k = crate::knowledge::open(&home).unwrap();
        let review = crate::my_work_ops::newest(&k, "review", "R1")
            .unwrap()
            .unwrap();
        assert_eq!(
            (review.op.skipped, review.op.forgotten.as_deref()),
            (true, Some(job.as_str()))
        );
        assert!(review.op.fields.is_empty());
        let r3 = crate::my_work_ops::newest(&k, "review", "R3")
            .unwrap()
            .unwrap();
        assert_eq!(
            r3.op.fields["summary"],
            json!("Kept: it read the kept record.")
        );
        // Closed before the purges below rebuild knowledge.db: Windows moves no open file.
        drop(k);
        // A restore from older ops gives a My Work op its text back: purged again.
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE ops SET body = ?1 WHERE op_seq = ?2",
                rusqlite::params![before[&first].to_string(), first],
            )
            .unwrap();
        assert!(status(&home).unwrap().iter().any(|j| j.local == PENDING));
        assert_eq!(purge(&home).unwrap().unwrap().ops, 1);
        assert_eq!(
            store_hits(&home, WINDOW_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
    }

    /// Codex on #664: My Work ops only the backup's ops segments and a copy of raw.db hold lose
    /// their text there when they read a record a forget takes, or an op it takes, in op order;
    /// so does one built on an op a purge rewrote there before.
    #[test]
    fn a_forget_takes_the_my_work_ops_only_the_backups_hold_and_those_built_on_them() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Parsers stay in {WINDOW_CANARY} mode."),
        );
        let r2 = s.said("s", "github.com/o/r", 2_000, "Deploys run on Fridays.");
        let op = |spans: serde_json::Value, ops: serde_json::Value, text: &str| {
            let body = json!({"job": "review", "about": "R1", "entry": "anthropic", "ts": 5_000,
                "read": {"spans": spans, "ops": ops}, "removed": [],
                "fields": {"summary": text}, "skipped": false});
            (crate::raw::OpKind::MyWork, body)
        };
        let read = op(
            json!([[r1, r1]]),
            json!([]),
            &format!("Read: {WINDOW_CANARY}."),
        );
        let shown = s.raw.append_ops(&[read]).unwrap()[0];
        let built = op(
            json!([[r2, r2]]),
            json!([shown]),
            &format!("On it: {WINDOW_CANARY}."),
        );
        s.raw.append_ops(&[built]).unwrap();
        // One a purge rewrote before, for another forget, and one built on it.
        let gone = json!({"job": "review", "about": "R2", "entry": "anthropic", "ts": 5_000,
            "read": {"spans": [], "ops": []}, "removed": [], "fields": {}, "skipped": true,
            "forgotten": "earlier"});
        let earlier = s
            .raw
            .append_ops(&[(crate::raw::OpKind::MyWork, gone)])
            .unwrap()[0];
        let on = op(
            json!([[r2, r2]]),
            json!([earlier]),
            &format!("Then: {WINDOW_CANARY}."),
        );
        s.raw.append_ops(&[on]).unwrap();
        let home = s.home.path().to_owned();
        crate::backup::export(&home).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        // raw.db no longer holds them, as after a restore of an older store.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE type = 'my_work'", [])
            .unwrap();
        assert!(!backup_hits(&home, WINDOW_CANARY).is_empty());
        let in_copy =
            |copy: &Path| (copy_ops(copy).iter()).any(|(_, _, body)| body.contains(WINDOW_CANARY));
        assert!(in_copy(&copy));
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        let shown = preview(&home, target).unwrap();
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert_eq!(backup_hits(&home, WINDOW_CANARY), Vec::<String>::new());
        assert!(!in_copy(&copy));
    }

    /// docs/my-work-model.md S5 (Codex on #664): after a restore reused op seqs, a copy and a
    /// segment can hold different bodies under one op seq. Each is judged, so the copy's version
    /// that read a forgotten record is taken, and the copy's op built on it with it.
    #[test]
    fn every_body_an_op_seq_has_in_the_backups_and_copies_is_judged() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Parsers stay in {WINDOW_CANARY} mode."),
        );
        let r2 = s.said("s", "github.com/o/r", 2_000, "Deploys run on Fridays.");
        let body = |spans: serde_json::Value, ops: serde_json::Value, text: &str| {
            json!({"job": "review", "about": "R1", "entry": "anthropic", "ts": 5_000,
                "read": {"spans": spans, "ops": ops}, "removed": [],
                "fields": {"summary": text}, "skipped": false})
        };
        let mut append = |op: serde_json::Value| {
            s.raw
                .append_ops(&[(crate::raw::OpKind::MyWork, op)])
                .unwrap()[0]
        };
        // The copy's: one op read the forgotten record, the next was built on it.
        let a = append(body(
            json!([[r1, r1]]),
            json!([]),
            &format!("Read: {WINDOW_CANARY}."),
        ));
        let b = append(body(
            json!([[r2, r2]]),
            json!([a]),
            &format!("On it: {WINDOW_CANARY}."),
        ));
        let home = s.home.path().to_owned();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        // The segment's, from the store a restore made: another op under the first one's seq,
        // which read nothing forgotten, and none under the second's.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        let other = body(json!([[r2, r2]]), json!([]), "Kept: another op.");
        conn.execute(
            "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3",
            rusqlite::params![other.to_string(), device, a],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM ops WHERE device = ?1 AND op_seq = ?2",
            rusqlite::params![device, b],
        )
        .unwrap();
        crate::backup::export(&home).unwrap();
        conn.execute("DELETE FROM ops WHERE type = 'my_work'", [])
            .unwrap();
        drop(conn);
        let in_copy = |text: &str| (copy_ops(&copy).iter()).any(|(_, _, body)| body.contains(text));
        assert!(in_copy(&format!("On it: {WINDOW_CANARY}.")));
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        let shown = preview(&home, target).unwrap();
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert!(!in_copy(WINDOW_CANARY), "{:?}", copy_ops(&copy));
    }

    /// Slice 3d-2: the window ops that showed a curator a forgotten record (in their span or their
    /// goals) or a claim made from one (among their candidates, or the claims carried in), and the
    /// turn ops that rest on such a record or read a card of such a window, keep their structure
    /// and lose their text, in raw.db, knowledge.db, the ops segments and a copy of raw.db; those
    /// windows are queued for recuration, again after a rebuild. A window that listed the record
    /// removed whole, one written well after the forget, a turn that read the span's later window
    /// well after it, and the ops of kept records stay. The preview counts them, the status is
    /// done, and a window op whose text a restore brought back is purged again.
    #[test]
    fn what_curators_were_shown_of_a_forgotten_record_loses_its_text() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {WINDOW_CANARY} mode.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, "Deploys run on Fridays.");
        let r3 = s.said("s", "github.com/o/r", 3_000, "Tests run on CI.");
        let claim = claim_op(&device, &[(r1, &gone)], &gone);
        let a = crate::claims::op_uid(&claim.1).unwrap();
        s.raw.append_ops(&[claim]).unwrap();
        let said = |what: &str| format!("{what}: {WINDOW_CANARY}.");
        let removed_whole = json!({"removed": [[r1, null, null]]});
        let ops = [
            window_op(r1, r1, &said("its span"), json!({})),
            window_op(r2, r2, &said("its goals"), json!({"goals": [r1]})),
            window_op(r3, r3, &said("its candidates"), json!({"candidates": [a]})),
            window_op(r3, r3, &said("carried in"), json!({"carried": [a]})),
            window_op(r2, r2, "Kept: the span's later window.", json!({})),
            window_op(r1, r1, "Kept: it listed the record removed.", removed_whole),
            turn_op(r1, r1, &[], &said("its record")),
            turn_op(r2, r2, &[(r1, r2)], &said("a card of the record")),
            turn_op(r3, r3, &[(r3, r3)], &said("a card of the claim")),
            turn_op(
                r2,
                r2,
                &[(r2, r2)],
                "Kept: it read the later window, an hour on.",
            ),
            turn_op(r3, r3, &[], "Kept: none of it."),
            turn_op(
                r3,
                r3,
                &[(r3, r3)],
                "Kept: it read the span an hour before them.",
            ),
        ];
        let seqs = s.raw.append_ops(&ops).unwrap();
        let (gone_ops, kept_ops) = (
            [0, 1, 2, 3, 6, 7, 8].map(|i| seqs[i]),
            [4, 5, 9, 10, 11].map(|i| seqs[i]),
        );
        let home = s.home.path().to_owned();
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        conn.execute(
            "UPDATE ops SET ts = ts + 3600000 WHERE op_seq = ?1",
            [seqs[9]],
        )
        .unwrap();
        conn.execute(
            "UPDATE ops SET ts = ts - 3600000 WHERE op_seq = ?1",
            [seqs[11]],
        )
        .unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        let before = op_bodies(&home);
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        let shown = preview(&home, target).unwrap();
        assert_eq!((shown.claims, shown.cards, shown.summaries), (1, 4, 3));
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        // Written well after the forget, with more removals than an op lists: not shown it. One
        // written within a minute of the request's time may have been cut before its transaction.
        let tombstone: i64 = conn
            .query_row(
                "SELECT MIN(ts) FROM records WHERE type = 'tombstone' AND source = 'forget'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let more = [
            window_op(r1, r1, "Kept: cut after the forget.", json!({})),
            window_op(r1, r1, &said("cut as the forget began"), json!({})),
        ];
        let added = crate::raw::open(&home).unwrap().append_ops(&more).unwrap();
        let (late, early) = (added[0], added[1]);
        for (seq, ts) in [(late, tombstone + 3_600_000), (early, tombstone + 30_000)] {
            conn.execute(
                "UPDATE ops SET ts = ?1 WHERE op_seq = ?2",
                rusqlite::params![ts, seq],
            )
            .unwrap();
        }
        let mut before = before;
        for seq in [late, early] {
            before.insert(seq, op_bodies(&home)[&seq].clone());
        }
        let gone_windows = [gone_ops[0], gone_ops[1], gone_ops[2], gone_ops[3], early];
        assert!(!store_hits(&home, WINDOW_CANARY.as_bytes()).is_empty());
        let purged = purge(&home).unwrap().unwrap();
        // The claim, five windows and three turns.
        assert_eq!((purged.records, purged.ops), (1, 9));
        let job = crate::raw::open(&home).unwrap().forget_jobs().unwrap()[0]
            .0
            .job
            .clone();
        let after = op_bodies(&home);
        let (w, t) = (&after[&gone_ops[0]], &after[&gone_ops[4]]);
        let mut keys: Vec<&str> = w.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "elided",
                "forgotten",
                "from_offset",
                "from_seq",
                "outcome",
                "to_offset",
                "to_seq"
            ]
        );
        assert_eq!((&w["forgotten"], &w["from_seq"]), (&json!(job), &json!(r1)));
        assert_eq!(
            [
                &t["agent"],
                &t["session"],
                &t["repo"],
                &t["fields"],
                &t["skipped"]
            ],
            [
                &json!(""),
                &json!(""),
                &json!(null),
                &json!({}),
                &json!(true)
            ]
        );
        assert_eq!((&t["forgotten"], &t["through"]), (&json!(job), &json!(r1)));
        for seq in gone_windows.iter() {
            assert_eq!(
                after[seq],
                crate::raw::forgotten_window_body(&before[seq], &job),
                "{seq}"
            );
        }
        for seq in &gone_ops[4..] {
            assert_eq!(
                after[seq],
                crate::raw::forgotten_turn_body(&before[seq], &job),
                "{seq}"
            );
        }
        for seq in kept_ops.iter().chain([&late]) {
            assert_eq!(after[seq], before[seq], "{seq}");
        }
        assert_eq!(
            store_hits(&home, WINDOW_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        assert_eq!(backup_hits(&home, WINDOW_CANARY), Vec::<String>::new());
        assert!(!holds_text(&copy, WINDOW_CANARY));
        let queued = |home: &Path| -> Vec<i64> {
            rusqlite::Connection::open(home.join("knowledge.db"))
                .unwrap()
                .prepare("SELECT op_seq FROM recurate ORDER BY op_seq")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(queued(&home), gone_windows);
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        crate::worker::rebuild(&home).unwrap();
        assert_eq!(queued(&home), gone_windows);
        // A restore from older ops segments gives a window op its text back: purged again.
        conn.execute(
            "UPDATE ops SET body = ?1 WHERE op_seq = ?2",
            rusqlite::params![before[&gone_ops[0]].to_string(), gone_ops[0]],
        )
        .unwrap();
        assert!(status(&home).unwrap().iter().any(|j| j.local == PENDING));
        assert_eq!(purge(&home).unwrap().unwrap().ops, 1);
        assert_eq!(
            store_hits(&home, WINDOW_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
    }

    /// A turn op that carried a forgotten record in as a goal loses its text, as one over it does
    /// (docs/provenance.md P1: a turn's goals); one that did not keeps its text.
    #[test]
    fn a_turn_that_carried_a_forgotten_record_as_a_goal_loses_its_text() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Parsers stay in {WINDOW_CANARY} mode."),
        );
        let r2 = s.said("s", "github.com/o/r", 2_000, "Deploys run on Fridays.");
        let mut carried = turn_op(r2, r2, &[], &format!("Its turn: {WINDOW_CANARY}."));
        carried.1["goals"] = serde_json::json!([r1]);
        let seqs = s
            .raw
            .append_ops(&[carried, turn_op(r2, r2, &[], "Kept: deploys.")])
            .unwrap();
        let home = s.home.path().to_owned();
        let target = Target::Record { device, seq: r1 };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap();
        let bodies = op_bodies(&home);
        assert!(
            bodies[&seqs[0]]["forgotten"].is_string(),
            "{}",
            bodies[&seqs[0]]
        );
        assert_eq!(bodies[&seqs[1]]["fields"]["learned"], "Kept: deploys.");
    }

    /// Decision 107 (docs/curator-conversation.md): a window op lists `read`, the spans of the
    /// stretches its curator was sent again before its own. A forget of a record in one of them
    /// takes the op's text and the claims it wrote, as a forget of a record of its own span does
    /// (rules a and c), in raw.db and knowledge.db, and the op keeps `read`; a window that read
    /// another stretch, and one that read none, keep their text.
    #[test]
    fn a_window_that_read_a_forgotten_records_stretch_loses_its_text() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {WINDOW_CANARY} mode.");
        let deploys = "Deploys run on Fridays.";
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, deploys);
        let r3 = s.said("s", "github.com/o/r", 3_000, "Tests run on CI.");
        let later = window_op(
            r2,
            r2,
            &format!("After its earlier stretch: {WINDOW_CANARY}."),
            json!({"read": [[r1, r1]]}),
        );
        // A claim it wrote: it quotes the window's own record and says what the stretch showed.
        let said = format!("Deploys follow {WINDOW_CANARY}.");
        let claim = claim_op(&device, &[(r2, deploys)], &said);
        let batch = s.raw.append_ops(&[later, claim]).unwrap();
        let kept = s
            .raw
            .append_ops(&[
                window_op(
                    r3,
                    r3,
                    "Kept: it read another stretch.",
                    json!({"read": [[r2, r2]]}),
                ),
                window_op(r3, r3, "Kept: it read none.", json!({})),
            ])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let before = op_bodies(&home);
        let target = Target::Record { device, seq: r1 };
        let shown = preview(&home, target).unwrap();
        assert_eq!((shown.cards, shown.written), (1, 1));
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert!(!store_hits(&home, WINDOW_CANARY.as_bytes()).is_empty());
        purge(&home).unwrap();
        let job = crate::raw::open(&home).unwrap().forget_jobs().unwrap()[0]
            .0
            .job
            .clone();
        let after = op_bodies(&home);
        let window = &after[&batch[0]];
        assert_eq!(
            window,
            &crate::raw::forgotten_window_body(&before[&batch[0]], &job)
        );
        assert_eq!(
            (&window["read"], &window["forgotten"]),
            (&json!([[r1, r1]]), &json!(job))
        );
        assert_eq!(
            after[&batch[1]]["forgotten"],
            json!(job),
            "{}",
            after[&batch[1]]
        );
        for seq in &kept {
            assert_eq!(after[seq], before[seq], "{seq}");
        }
        assert_eq!(
            store_hits(&home, WINDOW_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
    }

    /// Decision 107 (Codex on #974): a turn op lists the stretches its cards' windows read with
    /// those windows' spans, and names them in `stretches`. A window taken for a claim it was
    /// shown (its text may repeat the claim) takes the summary of a turn that read its card (rule
    /// b), and of one whose op does not say the span is a stretch only; not the summary of a turn
    /// that read the card of a later window which was sent the taken window's stretch again: the
    /// claim was in the earlier call's tail, never in the stretch. The later window keeps its text
    /// too.
    #[test]
    fn a_turn_keeps_its_text_when_its_cards_window_read_the_stretch_of_a_taken_window() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {WINDOW_CANARY} mode.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, "Deploys run on Fridays.");
        let r3 = s.said("s", "github.com/o/r", 3_000, "Tests run on CI.");
        let claim = claim_op(&device, &[(r1, &gone)], &gone);
        let a = crate::claims::op_uid(&claim.1).unwrap();
        s.raw.append_ops(&[claim]).unwrap();
        let said = |what: &str| format!("{what}: {WINDOW_CANARY}.");
        let mut after_a_stretch = turn_op(
            r3,
            r3,
            &[(r3, r3), (r2, r2)],
            "Kept: its card's window read a stretch.",
        );
        after_a_stretch.1["stretches"] = json!([[r2, r2]]);
        let ops = [
            window_op(r2, r2, &said("its candidates"), json!({"candidates": [a]})),
            window_op(
                r3,
                r3,
                "Kept: it read the stretch of the window shown the claim.",
                json!({"read": [[r2, r2]]}),
            ),
            after_a_stretch,
            turn_op(r3, r3, &[(r2, r2)], &said("a card of that window")),
            turn_op(r3, r3, &[(r3, r3), (r2, r2)], &said("both cards")),
        ];
        let seqs = s.raw.append_ops(&ops).unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let before = op_bodies(&home);
        let target = Target::Record { device, seq: r1 };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap();
        let after = op_bodies(&home);
        for gone in [0, 3, 4] {
            let body = &after[&seqs[gone]];
            assert!(body["forgotten"].is_string(), "op {gone}: {body}");
        }
        for kept in [1, 2] {
            assert_eq!(after[&seqs[kept]], before[&seqs[kept]], "op {kept}");
        }
        assert_eq!(
            store_hits(&home, WINDOW_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
    }

    /// Decision 107: `Raw::window_read` gives the spans a window op lists in `read`, as the one
    /// reading of its provenance takes them (an item that is no pair of seqs is none), and none
    /// of a turn op, whose `read` is its own, nor of an op raw does not hold.
    #[test]
    fn only_a_window_ops_read_is_a_windows() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said("s", "github.com/o/r", 1_000, "Deploys run on Fridays.");
        let read = json!({"read": [[1, 2], "x", [3], [4, 5]]});
        let ops = [
            window_op(r1, r1, "A window.", read),
            turn_op(r1, r1, &[(1, 2)], "A turn."),
        ];
        let seqs = s.raw.append_ops(&ops).unwrap();
        assert_eq!(
            s.raw.window_read(&device, seqs[0]).unwrap(),
            [(1, 2), (4, 5)]
        );
        assert_eq!(s.raw.window_read(&device, seqs[1]).unwrap(), []);
        assert_eq!(s.raw.window_read(&device, seqs[1] + 1).unwrap(), []);
        assert_eq!(s.raw.window_read("another", seqs[0]).unwrap(), []);
    }

    /// A window op that holds no text, a skip, keeps its body when its span's record is forgotten
    /// (docs/provenance.md P1: only a curated window holds text made from what it read).
    #[test]
    fn a_skipped_window_over_a_forgotten_record_keeps_its_body() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Parsers stay in {WINDOW_CANARY} mode."),
        );
        let skipped = serde_json::json!({"outcome": "skipped", "reason": "excluded",
            "from_seq": r1, "from_offset": null, "to_seq": r1, "to_offset": null});
        let seq = s
            .raw
            .append_ops(&[(raw::OpKind::Window, skipped.clone())])
            .unwrap()[0];
        let home = s.home.path().to_owned();
        let target = Target::Record { device, seq: r1 };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap();
        assert_eq!(op_bodies(&home)[&seq], skipped);
    }

    /// docs/provenance.md F2, for slice 2: after a restore that lost raw.db's tail gave a new op the
    /// op seq a segment's op still holds, a forget's purge judges the segment's op by its own body:
    /// a window that showed no forgotten record keeps its text, and is not given the new op's
    /// rewritten body (a turn's).
    #[test]
    #[ignore = "docs/provenance.md slice 2 (F2)"]
    fn a_segments_op_at_a_reused_seq_is_judged_by_its_own_body() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let home = s.home.path().to_owned();
        let gone = format!("Parsers stay in {WINDOW_CANARY} mode.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        crate::backup::export(&home).unwrap();
        let r2 = s.said("s", "github.com/o/r", 2_000, "Logs rotate daily.");
        let window = "Window marker zq7.";
        let seq = s
            .raw
            .append_ops(&[window_op(r2, r2, window, json!({}))])
            .unwrap()[0];
        crate::backup::export(&home).unwrap();
        // The restore loses r2's records segment: the window over it is cut from raw.db.
        let lost = backup_hits(&home, "Logs rotate daily.");
        assert_eq!(lost.len(), 1, "{lost:?}");
        let dir = crate::backup::dir(&home).unwrap();
        std::fs::write(dir.join(&lost[0]), b"damaged").unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        let mut raw = crate::backup::open_raw(&home).unwrap();
        assert_eq!(raw.max_seq().unwrap(), r1);
        assert!(!backup_hits(&home, window).is_empty(), "a segment holds it");
        // The next op, a turn that showed r1, is given the window's op seq.
        let turn = turn_op(r1, r1, &[], &format!("Its turn: {WINDOW_CANARY}."));
        assert_eq!(raw.append_ops(&[turn]).unwrap()[0], seq);
        drop(raw);
        let target = Target::Record { device, seq: r1 };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        purge(&home).unwrap();
        assert!(
            !backup_hits(&home, window).is_empty(),
            "the window showed no forgotten record: its text stays"
        );
    }

    /// Codex and CodeRabbit on #472: the window, turn and claim ops raw.db does not hold (raw.db put
    /// back without them: their ops segment is set aside, and a copy of raw.db holds them) are
    /// judged by their own bodies as raw.db's are, and lose their text in both; another record's
    /// window stays.
    #[test]
    fn what_raw_does_not_hold_is_judged_by_its_own_body() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {WINDOW_CANARY} mode.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, "Deploys run on Fridays.");
        let r3 = s.said("s", "github.com/o/r", 3_000, "Tests run nightly.");
        let said = |what: &str| format!("{what}: {WINDOW_CANARY}.");
        let kept = "Kept: another record's window.";
        let claim = claim_op(&device, &[(r1, &gone)], &said("its claim"));
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        // Another span's window that showed the claim, a turn that read its card, and the owner's
        // correction of the claim: only the claim's uid ties them to the record (Codex on #472).
        let seqs = s
            .raw
            .append_ops(&[
                window_op(r1, r1, &said("its span"), json!({})),
                turn_op(r1, r1, &[], &said("its turn")),
                claim,
                window_op(r2, r2, kept, json!({})),
                window_op(r3, r3, &said("its candidate"), json!({"candidates": [uid]})),
                turn_op(r3, r3, &[(r3, r3)], &said("its card's turn")),
                (
                    raw::OpKind::Correction,
                    json!({"uid": uid, "body": said("its correction")}),
                ),
            ])
            .unwrap();
        let home = s.home.path().to_owned();
        crate::backup::export(&home).unwrap();
        // And a claim and a window that showed it which only the copy holds.
        let r4 = s.said("s", "github.com/o/r", 4_000, "Logs rotate daily.");
        let late = claim_op(
            &device,
            &[(r2, "Deploys run on Fridays."), (r1, &gone)],
            &said("its late claim"),
        );
        let late_uid = raw::op_uid(late.0, &late.1).unwrap();
        s.raw
            .append_ops(&[
                late,
                window_op(
                    r4,
                    r4,
                    &said("its late candidate"),
                    json!({"candidates": [late_uid]}),
                ),
            ])
            .unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        // The claim, the window that showed it and what names its uid, the segments' alone.
        let only_segments = [seqs[2], seqs[4], seqs[5], seqs[6]];
        rusqlite::Connection::open(&copy)
            .unwrap()
            .execute(
                "DELETE FROM ops WHERE op_seq IN (SELECT value FROM json_each(?1))",
                [serde_json::to_string(&only_segments).unwrap()],
            )
            .unwrap();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE op_seq >= ?1", [seqs[0]])
            .unwrap();
        let target = Target::Record { device, seq: r1 };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let held = |text: &str| {
            copy_ops(&copy)
                .iter()
                .filter(|(.., b)| b.contains(text))
                .count()
        };
        assert_eq!((held(WINDOW_CANARY), held(kept)), (4, 1));
        purge(&home).unwrap().unwrap();
        assert_eq!(backup_hits(&home, WINDOW_CANARY), Vec::<String>::new());
        assert_eq!((held(WINDOW_CANARY), held(kept)), (0, 1));
        let wal = PathBuf::from(format!("{}-wal", copy.display()));
        assert!(!holds_text(&copy, WINDOW_CANARY) && !holds_text(&wal, WINDOW_CANARY));
        assert!(!backup_hits(&home, kept).is_empty());
    }

    /// Slice 3d-4: a canary of 32 bytes, the least a claim must repeat of what its window showed to
    /// be followed.
    const BATCH_CANARY: &str = "bcanary-4f1d9e7a2c3b6d5e8f0a1b2c";

    /// The windows queued for recuration, by op seq.
    fn queued(home: &Path) -> Vec<i64> {
        rusqlite::Connection::open(home.join("knowledge.db"))
            .unwrap()
            .prepare("SELECT op_seq FROM recurate ORDER BY op_seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// Slice 3d-4 (rule c): a window that showed a forgotten record loses the claims it wrote with
    /// its cards: the one that quotes the record by its uid (3d-1), the others with their uids
    /// kept. One that repeated 32 bytes of what its window showed (the record, a claim quoting it
    /// the window named, or a claim copied before) is `copied`, and a later window that named it
    /// before then, as a candidate or carried in, is judged by it (b'), with its own claims;
    /// another device's window, before them in op order, too. In raw.db, knowledge.db, the ops
    /// segments and a copy of raw.db; the preview counts them. A window that named the claim that
    /// copied nothing stays. A window restored from older ops is purged again by the chain a
    /// window's `copied` keeps, and a claim given back its text beside its rewritten window is
    /// taken again; a window written a minute or more after the purge that names a copied claim, a
    /// recuration's claim of its uid, and a window that named it after that claim, stay.
    #[test]
    fn the_claims_a_window_wrote_lose_their_text_with_its_cards() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {BATCH_CANARY} mode.");
        let (deploys, tests) = ("Deploys run on Fridays.", "Tests run on CI.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, deploys);
        let r3 = s.said("s", "github.com/o/r", 3_000, tests);
        // Claims of one record's sentence told apart by the sentence their uid names.
        let claim = |seq: i64, quote: &str, sentence: i64, body: &str| {
            let mut op = claim_op(&device, &[(seq, quote)], body);
            op.1["evidence"][0]["sentence"] = sentence.into();
            op
        };
        let uid = |op: &(raw::OpKind, serde_json::Value)| raw::op_uid(op.0, &op.1).unwrap();
        let c1 = claim(r1, &gone, 0, "Parsers stay strict.");
        let c2 = claim(r2, deploys, 0, &format!("Deploys follow {BATCH_CANARY}."));
        let c3 = claim(
            r2,
            deploys,
            1,
            "Deploys run on Fridays, never on a Monday morning.",
        );
        let c4 = claim(
            r3,
            tests,
            0,
            &format!("Tests run on CI as {BATCH_CANARY} says."),
        );
        let c5 = claim(r3, tests, 1, "Tests run on CI before each release.");
        let c6 = claim(r3, tests, 2, &format!("Tests skip {BATCH_CANARY} on CI."));
        let (u1, u2, u3, u4, u6) = (uid(&c1), uid(&c2), uid(&c3), uid(&c4), uid(&c6));
        let mut append = |ops: &[(raw::OpKind, serde_json::Value)]| s.raw.append_ops(ops).unwrap();
        let w1 = append(&[window_op(r1, r2, "Its span.", json!({})), c1, c2, c3]);
        let w2 = append(&[
            window_op(r3, r3, "It showed the copy.", json!({"candidates": [u2]})),
            c4,
        ]);
        let w4 = append(&[
            window_op(
                r3,
                r3,
                "Kept: it showed a claim that copied nothing.",
                json!({"candidates": [u3]}),
            ),
            c5,
        ]);
        let w5 = append(&[
            window_op(
                r3,
                r3,
                "It showed the quoting claim.",
                json!({"candidates": [u1]}),
            ),
            c6,
        ]);
        let w6 = append(&[window_op(
            r3,
            r3,
            "It showed the second copy.",
            json!({"candidates": [u6]}),
        )]);
        // Another device's window that carried the copy in, synced: first in op order.
        let w3 = 1_000_000;
        let home = s.home.path().to_owned();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                 VALUES('0', ?1, 'window', ?2, ?3, ?1)",
                rusqlite::params![
                    w3,
                    crate::db::now_ms(),
                    window_op(r3, r3, "It carried the copy.", json!({"carried": [u2]}))
                        .1
                        .to_string()
                ],
            )
            .unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        let wal = PathBuf::from(format!("{}-wal", copy.display()));
        let before = op_bodies(&home);
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        let shown = preview(&home, target).unwrap();
        assert_eq!((shown.claims, shown.cards, shown.written), (1, 5, 4));
        start(&home, &shown).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert!(!store_hits(&home, BATCH_CANARY.as_bytes()).is_empty());
        let purged = purge(&home).unwrap().unwrap();
        // The quoting claim, five windows and four of their claims.
        assert_eq!((purged.records, purged.ops), (1, 10));
        let job = crate::raw::open(&home).unwrap().forget_jobs().unwrap()[0]
            .0
            .job
            .clone();
        let after = op_bodies(&home);
        let forgotten = json!({ "forgotten": job });
        for seq in [w1[1], w1[2], w1[3], w2[1], w5[1]] {
            assert_eq!(after[&seq], forgotten, "{seq}");
        }
        let at = after[&w1[0]]["copied_at"].as_i64().unwrap();
        let rewritten = |seq: i64, copied: &[&str]| {
            let mut body = before[&seq].clone();
            if !copied.is_empty() {
                body["copied"] = json!(copied);
                body["copied_at"] = at.into();
            }
            crate::raw::forgotten_window_body(&body, &job)
        };
        for (seq, copied) in [
            (w1[0], vec![u2.as_str()]),
            (w2[0], vec![u4.as_str()]),
            (w5[0], vec![u6.as_str()]),
            (w6[0], vec![]),
            (w3, vec![]),
        ] {
            assert_eq!(after[&seq], rewritten(seq, &copied), "{seq}");
        }
        for seq in [w4[0], w4[1]] {
            assert_eq!(after[&seq], before[&seq], "{seq}");
        }
        let denied = crate::raw::open(&home)
            .unwrap()
            .record_derived_uids()
            .unwrap();
        assert!(denied.contains_key(&u1) && !denied.contains_key(&u2) && !denied.contains_key(&u3));
        assert_eq!(
            store_hits(&home, BATCH_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        assert_eq!(backup_hits(&home, BATCH_CANARY), Vec::<String>::new());
        assert!(!holds_text(&copy, BATCH_CANARY) && !holds_text(&wal, BATCH_CANARY));
        assert!(!store_hits(&home, b"before each release").is_empty());
        assert_eq!(queued(&home), [w1[0], w2[0], w5[0], w6[0], w3]);
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        // A restore from older ops segments gives one claim of the first window its text back, then
        // a later window: each is owed and purged again, the claim by its window's job, the window
        // by the `copied` of the one before it.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for seq in [w1[3], w6[0]] {
            conn.execute(
                "UPDATE ops SET body = ?1 WHERE op_seq = ?2",
                rusqlite::params![before[&seq].to_string(), seq],
            )
            .unwrap();
            assert!(
                status(&home).unwrap().iter().any(|j| j.local == PENDING),
                "{seq}"
            );
            assert_eq!(purge(&home).unwrap().unwrap().ops, 1, "{seq}");
        }
        assert_eq!(op_bodies(&home), after);
        // Written after the purge, they stay: a window that names a copied claim an hour later; a
        // recuration ten seconds after it that writes another copied claim again from its record
        // without the copy; a window within the minute after the purge that names that claim
        // after then (it showed the claim written again).
        let later = [
            window_op(
                r3,
                r3,
                "Kept: cut an hour after.",
                json!({"candidates": [u4]}),
            ),
            window_op(r2, r2, "Kept: the recuration.", json!({"recurate": true})),
            claim(r2, deploys, 0, "Deploys run on Fridays only."),
            window_op(
                r3,
                r3,
                "Kept: cut after the recuration.",
                json!({"candidates": [u2]}),
            ),
        ];
        let mut store = crate::raw::open(&home).unwrap();
        let mut seqs = store.append_ops(&later[..1]).unwrap();
        seqs.extend(store.append_ops(&later[1..3]).unwrap());
        seqs.extend(store.append_ops(&later[3..]).unwrap());
        assert_eq!(uid(&later[2]), u2);
        for (seq, after) in seqs.iter().zip([3_600_000, 10_000, 10_000, 20_000]) {
            conn.execute(
                "UPDATE ops SET ts = ?1 WHERE op_seq = ?2",
                rusqlite::params![at + after, seq],
            )
            .unwrap();
        }
        drop(store);
        let kept = op_bodies(&home);
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        for seq in seqs {
            assert_eq!(op_bodies(&home)[&seq], kept[&seq], "{seq}");
        }
    }

    /// docs/retention.md: a forget of a record whose text expired names it by its labels (it stays
    /// an event to forget's selection), hides it at once, and its purge makes its row and its
    /// backup line Removed, labels and all.
    #[test]
    fn a_forget_of_an_expired_record_removes_its_labels() {
        let mut s = crate::search::b::fixture::Store::new();
        let gone = s.said("s", "github.com/o/r", 1_000, "Expired words.");
        s.said("s", "github.com/o/r", 2_000, "Kept words.");
        s.raw.expire_for_test(&[gone]);
        let home = s.home.path().to_owned();
        crate::backup::export(&home).unwrap();
        let device = s.raw.device().to_owned();
        let kind_of = |home: &Path| -> String {
            segment_text(&segments_of(home, false)[0])
                .lines()
                .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
                .find(|v| v["seq"] == gone)
                .unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(kind_of(&home), "expired");
        let target = Target::Record {
            device: device.clone(),
            seq: gone,
        };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        assert!(matches!(
            s.raw.after(&device, gone - 1, 1).unwrap()[0].item,
            raw::Item::Removed
        ));
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.records, 1);
        let row: (String, Option<String>, Option<String>, Option<i64>) =
            rusqlite::Connection::open(home.join("raw.db"))
                .unwrap()
                .query_row(
                    "SELECT type, session, repo, expired FROM records WHERE seq = ?1",
                    [gone],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
        assert_eq!(row, ("removed".into(), None, None, None));
        assert_eq!(kind_of(&home), "removed");
    }

    /// docs/retention.md: a copy of raw.db set aside by a binary before retention has no
    /// `expired` column; a purge still rewrites its forgotten record, and keeps the copy.
    #[test]
    fn a_copy_from_before_retention_is_rewritten_not_removed() {
        let mut s = crate::search::b::fixture::Store::new();
        let gone = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Old {RECORD_CANARY}."),
        );
        s.said("s", "github.com/o/r", 2_000, "Kept words.");
        let home = s.home.path().to_owned();
        let device = s.raw.device().to_owned();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        rusqlite::Connection::open(&copy)
            .unwrap()
            .execute_batch("ALTER TABLE records DROP COLUMN expired")
            .unwrap();
        let target = Target::Record {
            device: device.clone(),
            seq: gone,
        };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.copies, purged.copies_removed), (1, 0));
        let kind: String = rusqlite::Connection::open(&copy)
            .unwrap()
            .query_row("SELECT type FROM records WHERE seq = ?1", [gone], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "removed");
    }

    /// docs/retention.md: a forgotten record whose text expired gives nothing to compare a claim
    /// with, so every claim its window kept counts as a copy of it, and a later window shown one
    /// loses its text too.
    #[test]
    fn a_forgotten_expired_record_makes_its_windows_claims_copies() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {BATCH_CANARY} mode.");
        let (deploys, tests) = ("Deploys run on Fridays.", "Tests run on CI.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, deploys);
        let r3 = s.said("s", "github.com/o/r", 3_000, tests);
        let c2 = claim_op(&device, &[(r2, deploys)], "Deploys run weekly.");
        let u2 = raw::op_uid(c2.0, &c2.1).unwrap();
        let c3 = claim_op(&device, &[(r3, tests)], "Tests run on CI.");
        s.raw
            .append_ops(&[window_op(r1, r2, "Its span.", json!({})), c2])
            .unwrap();
        let w2 = s
            .raw
            .append_ops(&[
                window_op(r3, r3, "It showed the claim.", json!({"candidates": [u2]})),
                c3,
            ])
            .unwrap();
        s.run();
        s.raw.expire_for_test(&[r1]);
        let home = s.home.path().to_owned();
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let after = op_bodies(&home);
        assert!(after[&w2[0]]["forgotten"].is_string(), "{}", after[&w2[0]]);
        assert!(after[&w2[1]]["forgotten"].is_string(), "{}", after[&w2[1]]);
    }

    /// docs/retention.md (CodeRabbit on #551): the rows decide whether a window a forget took the
    /// text of is queued again, at the forget's own rebuild. While a record of it is still expired
    /// it is not; once forgets took every expired one, it is, as any forgotten window is.
    #[test]
    fn a_forgotten_window_is_queued_once_it_holds_no_expired_record() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said("s", "github.com/o/r", 1_000, "First words.");
        let r2 = s.said("s", "github.com/o/r", 2_000, "Second words.");
        let w = s
            .raw
            .append_ops(&[window_op(r1, r2, "Its span.", json!({}))])
            .unwrap();
        s.run();
        s.raw.expire_for_test(&[r1, r2]);
        let home = s.home.path().to_owned();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        for (seq, want) in [(r1, Vec::new()), (r2, w)] {
            let target = Target::Record {
                device: device.clone(),
                seq,
            };
            start(&home, &preview(&home, target).unwrap()).unwrap();
            assert_eq!(purge(&home).unwrap().unwrap().records, 1);
            assert_eq!(queued(&home), want, "{seq}");
        }
    }

    /// Slice 3d-4 (Codex on #481): what a window was shown of a claim it named is every wording of
    /// it: the body the owner's correction gave a claim quoting the forgotten record, and each
    /// wording windows gave a copied claim's uid. A claim that repeated 32 bytes of either is
    /// copied, and the window that named it is taken.
    #[test]
    fn every_wording_of_a_named_claim_is_what_its_window_showed() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (second, corrected) = (
            "scanary-9d3b5e7f1a2c4e6b8d0f1a3c",
            "ccanary-2e4f6a8c0b1d3f5a7c9e1b3d",
        );
        let gone = format!("Parsers stay in {BATCH_CANARY} mode and {second} too.");
        let (deploys, tests) = ("Deploys run on Fridays.", "Tests run on CI.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, deploys);
        let r3 = s.said("s", "github.com/o/r", 3_000, tests);
        let claim = |seq: i64, quote: &str, sentence: i64, body: &str| {
            let mut op = claim_op(&device, &[(seq, quote)], body);
            op.1["evidence"][0]["sentence"] = sentence.into();
            op
        };
        let uid = |op: &(raw::OpKind, serde_json::Value)| raw::op_uid(op.0, &op.1).unwrap();
        // The claim quoting R1, whose body the owner corrected to one only the correction holds.
        let c1 = claim(r1, &gone, 0, "Parsers stay strict.");
        let u1 = uid(&c1);
        let fix = json!({"uid": u1, "anchor": {"device": device, "seq": r1},
            "body": format!("Parsers keep {corrected} on.")});
        // Two wordings of one claim of R2, each repeating another canary of R1.
        let a1 = claim(r2, deploys, 0, &format!("Deploys follow {BATCH_CANARY}."));
        let a2 = claim(r2, deploys, 0, &format!("Deploys keep {second} on."));
        let ua = uid(&a1);
        assert_eq!(uid(&a2), ua);
        // Claims of R3 that repeat the corrected body, and the second wording.
        let b1 = claim(r3, tests, 0, &format!("Tests check {corrected} first."));
        let b2 = claim(r3, tests, 1, &format!("Tests check {second} last."));
        let (ub1, ub2) = (uid(&b1), uid(&b2));
        let mut append = |ops: &[(raw::OpKind, serde_json::Value)]| s.raw.append_ops(ops).unwrap();
        append(&[window_op(r1, r1, "Its span.", json!({})), c1]);
        append(&[(raw::OpKind::Correction, fix)]);
        append(&[window_op(r1, r2, "The first wording.", json!({})), a1]);
        append(&[window_op(r1, r2, "The second wording.", json!({})), a2]);
        let w1 = append(&[
            window_op(
                r3,
                r3,
                "It named the corrected claim.",
                json!({"candidates": [u1]}),
            ),
            b1,
        ]);
        let w2 = append(&[
            window_op(
                r3,
                r3,
                "It named the claim of two wordings.",
                json!({"candidates": [ua]}),
            ),
            b2,
        ]);
        let n1 = append(&[window_op(
            r3,
            r3,
            "It named the first copy.",
            json!({"candidates": [ub1]}),
        )]);
        let n2 = append(&[window_op(
            r3,
            r3,
            "It named the second copy.",
            json!({"candidates": [ub2]}),
        )]);
        let home = s.home.path().to_owned();
        s.run();
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let after = op_bodies(&home);
        assert_eq!(after[&w1[0]]["copied"], json!([ub1]));
        assert_eq!(after[&w2[0]]["copied"], json!([ub2]));
        for seq in [n1[0], n2[0]] {
            assert!(after[&seq].get("forgotten").is_some(), "{seq}");
        }
        assert_eq!(store_hits(&home, corrected.as_bytes()), Vec::<&str>::new());
        assert_eq!(store_hits(&home, second.as_bytes()), Vec::<&str>::new());
    }

    /// Slice 3d-4 (Codex on #481): what a window was shown of a claim it named is the wordings of
    /// it written by then. A correction the owner wrote after it, or a later window's wording of a
    /// copied claim, that one of its claims repeats makes that claim no copy, and a window that
    /// named that claim stays.
    #[test]
    fn a_wording_written_after_a_window_is_not_what_it_showed() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (second, later) = (
            "scanary-9d3b5e7f1a2c4e6b8d0f1a3c",
            "lcanary-7a9c1e3b5d7f9a1c3e5b7d9f",
        );
        let gone = format!("Parsers stay in {BATCH_CANARY} mode and {second} too.");
        let (deploys, tests) = ("Deploys run on Fridays.", "Tests run on CI.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, deploys);
        let r3 = s.said("s", "github.com/o/r", 3_000, tests);
        let claim = |seq: i64, quote: &str, sentence: i64, body: &str| {
            let mut op = claim_op(&device, &[(seq, quote)], body);
            op.1["evidence"][0]["sentence"] = sentence.into();
            op
        };
        let uid = |op: &(raw::OpKind, serde_json::Value)| raw::op_uid(op.0, &op.1).unwrap();
        let c1 = claim(r1, &gone, 0, "Parsers stay strict.");
        let u1 = uid(&c1);
        // A claim of R2 whose first wording copies R1, and its second wording, which does too and
        // repeats what the claims of the windows below repeat.
        let a1 = claim(r2, deploys, 0, &format!("Deploys follow {BATCH_CANARY}."));
        let a2 = claim(
            r2,
            deploys,
            0,
            &format!("Deploys keep {second} on, as {later} says."),
        );
        let ua = uid(&a1);
        let b1 = claim(r3, tests, 0, &format!("Tests check {later} first."));
        let b2 = claim(r3, tests, 1, &format!("Tests check {later} last."));
        let (ub1, ub2) = (uid(&b1), uid(&b2));
        let fix = json!({"uid": u1, "anchor": {"device": device, "seq": r1},
            "body": format!("Parsers keep {later} on.")});
        let mut append = |ops: &[(raw::OpKind, serde_json::Value)]| s.raw.append_ops(ops).unwrap();
        append(&[window_op(r1, r1, "Its span.", json!({})), c1]);
        append(&[window_op(r1, r2, "The first wording.", json!({})), a1]);
        let w1 = append(&[
            window_op(
                r3,
                r3,
                "It named the claim quoting R1.",
                json!({"candidates": [u1]}),
            ),
            b1,
        ]);
        let w2 = append(&[
            window_op(
                r3,
                r3,
                "It named the copied claim.",
                json!({"candidates": [ua]}),
            ),
            b2,
        ]);
        // A second after them: the owner's correction, and the second wording.
        let mut after = append(&[(raw::OpKind::Correction, fix)]);
        after.extend(append(&[
            window_op(r1, r2, "The second wording.", json!({})),
            a2,
        ]));
        let n1 = append(&[window_op(
            r3,
            r3,
            "Kept: it named the first claim.",
            json!({"candidates": [ub1]}),
        )]);
        let n2 = append(&[window_op(
            r3,
            r3,
            "Kept: it named the second claim.",
            json!({"candidates": [ub2]}),
        )]);
        let home = s.home.path().to_owned();
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        let ts = |seq: i64| -> i64 {
            conn.query_row("SELECT ts FROM ops WHERE op_seq = ?1", [seq], |r| r.get(0))
                .unwrap()
        };
        let then = ts(w2[0]) + 1_000;
        for seq in after {
            conn.execute(
                "UPDATE ops SET ts = ?1 WHERE op_seq = ?2",
                rusqlite::params![then, seq],
            )
            .unwrap();
        }
        s.run();
        let before = op_bodies(&home);
        let target = Target::Record {
            device: device.clone(),
            seq: r1,
        };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let after = op_bodies(&home);
        for seq in [w1[0], w2[0]] {
            assert!(after[&seq].get("forgotten").is_some(), "{seq}");
            assert!(after[&seq].get("copied").is_none(), "{seq}");
        }
        for seq in [n1[0], n2[0]] {
            assert_eq!(after[&seq], before[&seq], "{seq}");
        }
        assert_eq!(store_hits(&home, second.as_bytes()), Vec::<&str>::new());
    }

    /// Slice 3d-4 (Codex's security review on #481): a claim a purge took as a copy, written again
    /// by a recuration from its record, then copied again by a window over another record a later
    /// forget takes, is a copy again: a window that named it after the recuration and before that
    /// purge is taken with it.
    #[test]
    fn a_claim_copied_again_after_its_recuration_is_followed_again() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let again = "acanary-3c5e7a9b1d3f5a7c9e1b3d5f";
        let gone = format!("Parsers stay in {BATCH_CANARY} mode.");
        let builds = format!("Builds keep {again} on.");
        let (deploys, tests) = ("Deploys run on Fridays.", "Tests run on CI.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, deploys);
        let r3 = s.said("s", "github.com/o/r", 3_000, &builds);
        let r4 = s.said("s", "github.com/o/r", 4_000, tests);
        let claim = |body: &str| claim_op(&device, &[(r2, deploys)], body);
        let c = claim(&format!("Deploys follow {BATCH_CANARY}."));
        let u = raw::op_uid(c.0, &c.1).unwrap();
        let w1 = s
            .raw
            .append_ops(&[window_op(r1, r2, "Its span.", json!({})), c])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let forget_record = |seq: i64| {
            let target = Target::Record {
                device: device.clone(),
                seq,
            };
            start(&home, &preview(&home, target).unwrap()).unwrap();
            purge(&home).unwrap().unwrap();
        };
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        forget_record(r1);
        let at = op_bodies(&home)[&w1[0]]["copied_at"].as_i64().unwrap();
        // Ten seconds after the purge a recuration writes the claim again from its record; then a
        // window over R3 writes it again, copying R3, and a window names it.
        let mut store = crate::raw::open(&home).unwrap();
        let mut seqs = store
            .append_ops(&[
                window_op(r2, r2, "The recuration.", json!({"recurate": true})),
                claim("Deploys run on Fridays only."),
            ])
            .unwrap();
        let w2 = store
            .append_ops(&[
                window_op(r2, r3, "It showed R3.", json!({})),
                claim(&format!("Deploys keep {again} on.")),
            ])
            .unwrap();
        let w3 = store
            .append_ops(&[window_op(
                r4,
                r4,
                "It named the second copy.",
                json!({"candidates": [u]}),
            )])
            .unwrap();
        drop(store);
        seqs.extend(&w2);
        seqs.extend(&w3);
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for (seq, after) in seqs.iter().zip([10_000, 10_000, 15_000, 15_000, 20_000]) {
            conn.execute(
                "UPDATE ops SET ts = ?1 WHERE op_seq = ?2",
                rusqlite::params![at + after, seq],
            )
            .unwrap();
        }
        crate::worker::run_once(&home).unwrap();
        forget_record(r3);
        let after = op_bodies(&home);
        assert!(after[&w2[0]].get("forgotten").is_some());
        assert!(after[&w3[0]].get("forgotten").is_some());
        assert_eq!(store_hits(&home, again.as_bytes()), Vec::<&str>::new());
    }

    /// Slice 3d-4: a window op and its claims that only the ops segments and a copy of raw.db
    /// hold (raw.db put back without them) lose their text there by rule (c), with no copy looked
    /// for.
    #[test]
    fn the_claims_of_a_window_raw_does_not_hold_lose_their_text() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let gone = format!("Parsers stay in {BATCH_CANARY} mode.");
        let deploys = "Deploys run on Fridays.";
        let r1 = s.said("s", "github.com/o/r", 1_000, &gone);
        let r2 = s.said("s", "github.com/o/r", 2_000, deploys);
        let seqs = s
            .raw
            .append_ops(&[
                window_op(r1, r2, "Its span.", json!({})),
                claim_op(
                    &device,
                    &[(r2, deploys)],
                    &format!("Deploys follow {BATCH_CANARY}."),
                ),
            ])
            .unwrap();
        let home = s.home.path().to_owned();
        crate::backup::export(&home).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE op_seq >= ?1", [seqs[0]])
            .unwrap();
        let target = Target::Record { device, seq: r1 };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let wal = PathBuf::from(format!("{}-wal", copy.display()));
        let held = || holds_text(&copy, BATCH_CANARY) || holds_text(&wal, BATCH_CANARY);
        assert!(!backup_hits(&home, BATCH_CANARY).is_empty() && held());
        purge(&home).unwrap().unwrap();
        assert_eq!(backup_hits(&home, BATCH_CANARY), Vec::<String>::new());
        assert!(!held());
        let job = crate::raw::open(&home).unwrap().forget_jobs().unwrap()[0]
            .0
            .job
            .clone();
        let claim = copy_ops(&copy)
            .into_iter()
            .find(|o| o.1 == seqs[1])
            .unwrap();
        assert_eq!(claim.2, json!({ "forgotten": job }).to_string());
    }

    /// Slice 3c, test 2 (rule 10): a session whose live records are all purged keeps its transcript
    /// cut at the first of them, from their denials, though their rows lost their labels.
    #[test]
    fn a_purged_sessions_transcript_cut_stays() {
        let mut s = crate::search::b::fixture::Store::new();
        let first = s.said("s", "github.com/o/r", 1_000, "First.");
        s.said("s", "github.com/o/r", 2_000, "Second.");
        let home = s.home.path().to_owned();
        let device = s.raw.device().to_owned();
        let crate::search::b::fixture::Store { home: _home, raw } = s;
        drop(raw);
        let target = session_target(&home, &format!("{device}:{first}")).unwrap();
        start(&home, &preview(&home, target).unwrap()).unwrap();
        purge(&home).unwrap().unwrap();
        let raw = crate::raw::open(&home).unwrap();
        let agent = raw::test_event("{}").agent;
        assert!(
            !raw.earliest_by_session()
                .unwrap()
                .contains_key(&(agent.clone(), "s".into()))
        );
        assert_eq!(raw.transcript_cut(&agent, "s", None).unwrap(), Some(1_000));
        assert_eq!(
            raw.transcript_cut(&agent, "s", Some(5_000)).unwrap(),
            Some(1_000)
        );
        assert_eq!(
            raw.transcript_cut(&agent, "other", Some(5_000)).unwrap(),
            Some(5_000)
        );
    }

    /// Slice 3c, test 3: a body stored compressed leaves no byte of its compressed form either.
    #[test]
    fn a_purged_compressed_body_leaves_no_byte() {
        let mut s = crate::search::b::fixture::Store::new();
        let seq = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("{RECORD_CANARY} {}", "compressible text ".repeat(400)),
        );
        let device = s.raw.device().to_owned();
        assert_eq!(s.raw.compress_through(&device, 0, seq).unwrap(), 1);
        let home = s.home.path().to_owned();
        let blob: Vec<u8> = rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .query_row(
                "SELECT body FROM records WHERE device = ?1 AND seq = ?2 AND enc = 'zstd'",
                rusqlite::params![device, seq],
                |r| r.get(0),
            )
            .unwrap();
        let crate::search::b::fixture::Store { home: _home, raw } = s;
        drop(raw);
        let target = Target::Record {
            device: device.clone(),
            seq,
        };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        assert!(!store_hits(&home, &blob).is_empty());
        purge(&home).unwrap().unwrap();
        assert_eq!(store_hits(&home, &blob), Vec::<&str>::new());
    }

    /// A text that only a claim op holds, none of its records (2b, decision 10).
    const CANARY: &str = "canary-7f3a";

    /// A decided decision whose claim op's body holds `CANARY` and whose record does not: its uid.
    fn canary_claim(s: &mut crate::search::b::fixture::Store) -> String {
        let seq = s.said("s", "github.com/o/r", 1_000, "We deploy on Fridays.");
        let evidence = crate::claims::Evidence {
            device: s.raw.device().to_owned(),
            seq,
            offset: 0,
            length: "We deploy on Fridays.".len() as i64,
            sentence: 0,
            quote: "We deploy on Fridays.".into(),
            claim_at: None,
        };
        let uid = crate::claims::uid("decision", &evidence);
        let op = crate::claims::ClaimOp {
            id: "c1".into(),
            kind: "decision".into(),
            status: "decided".into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: format!("Deploys happen on Friday, {CANARY}."),
            evidence: vec![evidence],
            supersedes: Vec::new(),
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
            support: Vec::new(),
        };
        s.raw
            .append_ops(&[(raw::OpKind::Claim, serde_json::to_value(op).unwrap())])
            .unwrap();
        uid
    }

    /// D5 (2b, item 10, test 2): a claim op's text that no record holds leaves no byte in raw.db,
    /// knowledge.db, their WALs or any file under `state/` once purged: the ops are rewritten with
    /// secure_delete and the WAL truncated, knowledge.db is rebuilt without the claim, and every
    /// session's shown value (OpenCode's receipts too, and a part a crashed change left) loses it.
    #[test]
    fn a_purge_leaves_no_byte_of_the_claims_own_text_in_the_stores_or_the_hook_state() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = canary_claim(&mut s);
        s.run();
        let home = s.home.path().to_owned();
        // What sessions were shown: Claude Code's form, OpenCode's with a receipt not yet taken,
        // and the part a change that crashed left in a session not shown it yet.
        let entry = format!(r#"{{"fp":"f","body":"Deploys happen on Friday, {CANARY}."}}"#);
        let kept = r#"{"fp":"g","body":"Kept."}"#;
        let flat = format!(r#"{{"{uid}":{entry},"kept-uid":{kept}}}"#);
        let receipts = format!(
            r#"{{"entries":{{"{uid}":{entry}}},"pending":[{{"id":"{}","at":1,"changes":[{{"uid":"{uid}","before":null,"after":{entry}}}],"replace":{{"{uid}":{entry}}}}}]}}"#,
            "0".repeat(32)
        );
        let before_crash = format!(r#"{{"kept-uid":{kept}}}"#);
        crate::hookstate::update(&home, "claude", "s", "shown", |_| Some(flat)).unwrap();
        crate::hookstate::update(&home, "claude", "crashed", "shown", |_| Some(before_crash))
            .unwrap();
        crate::hookstate::update(&home, "opencode", "oc", "shown", |_| Some(receipts)).unwrap();
        let crashed = std::fs::read_dir(home.join("state/hooks/claude"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|d| {
                !std::fs::read_to_string(d.join("shown"))
                    .unwrap()
                    .contains(&uid)
            })
            .unwrap();
        std::fs::write(crashed.join(".shown.part"), &entry).unwrap();
        // Every store file and every file under `state/` that holds the canary.
        fn holding(dir: &Path, found: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    holding(&path, found);
                } else if std::fs::read(&path)
                    .unwrap()
                    .windows(CANARY.len())
                    .any(|w| w == CANARY.as_bytes())
                {
                    found.push(path);
                }
            }
        }
        let hits = |home: &Path| -> Vec<PathBuf> {
            let mut found = Vec::new();
            holding(&home.join("state"), &mut found);
            for f in ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"] {
                let path = home.join(f);
                if std::fs::read(&path)
                    .is_ok_and(|b| b.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()))
                {
                    found.push(path);
                }
            }
            found
        };
        let before = hits(&home);
        assert!(
            before
                .iter()
                .any(|p| p.ends_with("knowledge.db") || p.ends_with("knowledge.db-wal"))
        );
        assert_eq!(
            before
                .iter()
                .filter(|p| p.starts_with(home.join("state")))
                .count(),
            3
        );
        start(
            &home,
            &preview(&home, Target::parse_uid(&uid).unwrap()).unwrap(),
        )
        .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.ops, purged.rebuilt, purged.shown), (1, true, 2));
        assert_eq!(hits(&home), Vec::<PathBuf>::new());
        let shown = crate::hookstate::value(&home, "claude", "s", "shown").unwrap();
        assert!(shown.contains("kept-uid"), "{shown}");
    }

    /// D5 (2b-1): a purged Import op no longer names its source id, so the import's own check
    /// would take the document for a new one: the forgotten uid is what keeps it out.
    #[test]
    fn a_purged_document_is_not_imported_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        let home = s.home.path().to_owned();
        start(
            &home,
            &preview(&home, Target::parse_uid(&doc).unwrap()).unwrap(),
        )
        .unwrap();
        // The purge rebuilds knowledge.db, which takes raw.db from every other handle.
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let mut raw = crate::raw::open(&home).unwrap();
        assert!(raw.import_repos("claude-mem:test").unwrap().is_empty());
        let again = raw::ImportDoc {
            uid: doc.clone(),
            source: "claude-mem:test".into(),
            source_id: "o1".into(),
            kind: "decision".into(),
            repo: crate::import::repo("r"),
            session: "cm".into(),
            ts: 3_000,
            title: "Deploy notes".into(),
            body: "Notes.".into(),
        };
        assert_eq!(raw.append_imports(vec![again]).unwrap(), 0);
        assert!(!op_rows(&home).iter().any(|o| o.3.contains("Deploy notes")));
    }

    /// docs/file-note.md F12 (test 18): a document's files are held for it until its purge, which
    /// takes them from raw.db's ops and, by the rebuild, from knowledge.db.
    #[test]
    fn a_purged_documents_files_go_with_it() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.imported_with_files(
            "o1",
            ("r", "cm"),
            "bugfix",
            3_000,
            "Fixed the reader",
            &["src/reader.rs"],
            &[],
        );
        s.run();
        let home = s.home.path().to_owned();
        // Its files alone hold it: the document's row taken away by hand.
        let k = rusqlite::Connection::open(home.join("knowledge.db")).unwrap();
        k.execute("DELETE FROM imported WHERE uid = ?1", [&uid])
            .unwrap();
        drop(k);
        let uids = HashSet::from([uid.clone()]);
        assert!(knowledge_holding(&home, &uids).unwrap().contains(&uid));
        start(
            &home,
            &preview(&home, Target::parse_uid(&uid).unwrap()).unwrap(),
        )
        .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert!(knowledge_holding(&home, &uids).unwrap().is_empty());
        assert!(!op_rows(&home).iter().any(|o| o.3.contains("src/reader.rs")));
    }

    /// D5 (2b-1): while a reader holds raw.db-wal, the purge rewrites the ops but cannot truncate
    /// the WAL, which may still hold their old pages: step 2 is not recorded, and the next purge,
    /// with the reader gone, records it and goes on to step 3.
    #[test]
    fn a_wal_a_reader_holds_keeps_the_step() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        start(
            &home,
            &preview(&home, Target::parse_uid(&uid).unwrap()).unwrap(),
        )
        .unwrap();
        let reader = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM ops", [], |r| r.get(0))
            .unwrap();
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.ops, purged.unfinished.len()), (1, 1));
        assert_eq!(uid_jobs(&home)[0].2, 1);
        reader.execute_batch("COMMIT").unwrap();
        drop(reader);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops, purged.rebuilt), (1, 0, true));
        assert!(purged.unfinished.is_empty());
        assert_eq!(uid_jobs(&home)[0].2, 5);
    }

    /// D5 (2b-1): one purge at a time: while another holds `state/purge.lock`, a purge does
    /// nothing and says so.
    #[test]
    fn one_purge_at_a_time() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        let home = s.home.path().to_owned();
        start(
            &home,
            &preview(&home, Target::parse_uid(&uid).unwrap()).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(home.join("state")).unwrap();
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.join("state/purge.lock"))
            .unwrap();
        held.lock().unwrap();
        assert_eq!(purge(&home).unwrap(), None);
        assert_eq!(uid_jobs(&home)[0].2, 1);
        drop(held);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().ops, 1);
    }

    /// Registers the forget of `uid` in `home`.
    fn forget(home: &Path, uid: &str) {
        start(
            home,
            &preview(home, Target::parse_uid(uid).unwrap()).unwrap(),
        )
        .unwrap();
    }

    /// The knowledge.db copies set aside in `home`: a rebuild that stopped leaves one.
    fn set_aside_copy(home: &Path, to: &Path) {
        let k = crate::knowledge::open(home).unwrap();
        k.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        drop(k);
        std::fs::copy(home.join("knowledge.db"), to).unwrap();
    }

    /// D5 (2b, item 4, test 5): one rebuild takes every job past step 2: two forgets registered
    /// before a purge leave knowledge.db together, and the next purge has nothing to do.
    #[test]
    fn one_rebuild_takes_every_job_registered_before_it() {
        let mut s = crate::search::b::fixture::Store::new();
        let tabs = s.decided("github.com/o/r", 1_000, "Use tabs.", &[]);
        let doc = s.imported("o1", "r", 2_000, "Deploy notes", "Notes.");
        s.run();
        let home = s.home.path().to_owned();
        forget(&home, &tabs);
        forget(&home, &doc);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.rebuilt), (2, true));
        assert!(purged.stopped.is_empty());
        let uids = HashSet::from([tabs, doc]);
        assert!(knowledge_holding(&home, &uids).unwrap().is_empty());
        assert!(uid_jobs(&home).iter().all(|j| j.2 == 5));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        let local: Vec<&str> = status(&home).unwrap().iter().map(|s| s.local).collect();
        assert_eq!(local, [DONE; 2]);
    }

    /// D5 (2b, item 5, test 6): no forgotten uid's vector comes back, neither from the knowledge.db
    /// the purge's rebuild sets aside nor from one an earlier rebuild left set aside when it
    /// stopped; a kept claim's comes back, and the copy is removed.
    #[test]
    fn no_forgotten_vector_comes_back_from_a_copy_set_aside() {
        let mut s = crate::search::b::fixture::Store::new();
        // A quote shorter than its record: no record's vector is the claim's.
        let said = "Parser caches stay in Redis, the team said.";
        let seq = s.said("s", "github.com/o/r", 1_000, said);
        let quote = "Parser caches stay in Redis";
        let gone = s.claim(seq, quote, ("decision", "decided", "user"), &[]);
        let kept = s.decided("github.com/o/r", 2_000, "Deploys run on Fridays.", &[]);
        s.run();
        crate::embed_phase::fixture::vectors(&s);
        let home = s.home.path().to_owned();
        let sha = |uid: &str| -> String {
            crate::knowledge::open(&home)
                .unwrap()
                .query_row(
                    "SELECT src_sha FROM vector_keys WHERE kind = 'c' AND key = ?1",
                    [uid],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let (gone_sha, kept_sha) = (sha(&gone), sha(&kept));
        let others: i64 = crate::knowledge::open(&home)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM vector_keys WHERE src_sha = ?1 AND key <> ?2",
                [&gone_sha, &gone],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(others, 0);
        set_aside_copy(
            &home,
            &home.join(format!("knowledge.db.rebuilding-{}", crate::db::now_ms())),
        );
        forget(&home, &gone);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert!(purged.rebuilt && purged.aside > 0);
        assert!(aside_files(&home).unwrap().is_empty());
        let k = crate::knowledge::open(&home).unwrap();
        let has = |sha: &str| -> bool {
            k.query_row(
                "SELECT EXISTS(SELECT 1 FROM vectors WHERE src_sha = ?1)",
                [sha],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(!has(&gone_sha));
        assert!(has(&kept_sha));
    }

    /// D5 (2b, item 7, test 6): a purge stopped after its rebuild, before it removed the copy set
    /// aside, rebuilds again when it goes on (nothing records that the first rebuild finished,
    /// Codex on #444): the copy is removed and step 3 recorded.
    #[test]
    fn a_purge_stopped_after_its_rebuild_rebuilds_again_and_removes_the_copy() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let copy = home.join("copy");
        set_aside_copy(&home, &copy);
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert!(purge(&home).unwrap().unwrap().rebuilt);
        // Stopped there: the old file left set aside, step 3 not recorded.
        let aside = home.join(format!("knowledge.db.rebuilding-{}", crate::db::now_ms()));
        std::fs::rename(&copy, aside).unwrap();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("UPDATE forget_jobs SET step = 2", [])
            .unwrap();
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops, purged.rebuilt), (1, 0, true));
        assert!(purged.aside > 0);
        assert!(aside_files(&home).unwrap().is_empty());
        assert_eq!(uid_jobs(&home)[0].2, 5);
    }

    /// D5 (2b, item 7, test 6): a restore that brings a purged op's text back is purged again,
    /// though its job recorded step 3: the state decides, and a worker starts the continuation.
    #[test]
    fn a_text_a_restore_brings_back_is_purged_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        let home = s.home.path().to_owned();
        let before = op_rows(&home);
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().ops, 1);
        assert_eq!(uid_jobs(&home)[0].2, 5);
        // The bodies an older raw.db held, back in place.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for (device, seq, _, body) in &before {
            conn.execute(
                "UPDATE ops SET body = ?3 WHERE device = ?1 AND op_seq = ?2",
                rusqlite::params![device, seq, body],
            )
            .unwrap();
        }
        drop(conn);
        CONTINUED.set(0);
        let raw = crate::raw::open(&home).unwrap();
        continue_due(&home, &raw);
        drop(raw);
        assert_eq!(CONTINUED.get(), 1);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops), (1, 1));
        assert_eq!(uid_jobs(&home)[0].2, 5);
        assert!(!op_rows(&home).iter().any(|o| o.3.contains("Use tabs.")));
    }

    /// A restore that brings a forget's text back owes its rebuild again at once (step 2), so the
    /// readers go by its tombstones until the purge marks what it took again (docs/my-work-model.md
    /// S5); the purge then finishes it again.
    #[test]
    fn a_restore_bringing_text_back_owes_the_rebuild_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        let home = s.home.path().to_owned();
        let before = op_rows(&home);
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert!(!raw::open(&home).unwrap().forget_unrebuilt().unwrap());
        // The bodies an older raw.db held, back in place.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for (device, seq, _, body) in &before {
            conn.execute(
                "UPDATE ops SET body = ?3 WHERE device = ?1 AND op_seq = ?2",
                rusqlite::params![device, seq, body],
            )
            .unwrap();
        }
        drop(conn);
        let raw = raw::open(&home).unwrap();
        owe_rebuild_again(&raw).unwrap();
        assert!(raw.forget_unrebuilt().unwrap());
        drop(raw);
        assert_eq!(uid_jobs(&home)[0].2, 2);
        purge(&home).unwrap().unwrap();
        assert!(!raw::open(&home).unwrap().forget_unrebuilt().unwrap());
    }

    /// Parity row 3 (Codex and CodeRabbit on #655): a purge started while another runs waits for
    /// it, and goes on when it ends; a home with no `state/` yet gets one, and its purge starts now.
    #[test]
    fn a_purge_started_while_one_runs_waits_for_it() {
        let home = tempfile::tempdir().unwrap();
        let before = CONTINUED.get();
        assert_eq!(continue_now(home.path(), true), Some(false));
        assert!(home.path().join("state").join("purge.lock").exists());
        let running = purge_lock(home.path()).unwrap().unwrap();
        assert_eq!(continue_now(home.path(), true), Some(true));
        // The worker's starts none beside it: it looks again in ten minutes (D5 item 8).
        assert_eq!(continue_now(home.path(), false), None);
        assert_eq!(CONTINUED.get(), before + 2);
        let (tx, rx) = std::sync::mpsc::channel();
        let waiting = home.path().to_owned();
        let waiter = std::thread::spawn(move || {
            wait_for_purge(&waiting).unwrap();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "it waits while the purge runs"
        );
        drop(running);
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("and goes on when it ends");
        waiter.join().unwrap();
        // One whose `state/` cannot be made starts none, and says so.
        let blocked = tempfile::tempdir().unwrap();
        std::fs::write(blocked.path().join("state"), "").unwrap();
        assert_eq!(continue_now(blocked.path(), true), None);
    }

    /// D5 (2b, item 5): a listing that fails is an error, never a home without copies set aside
    /// (Codex on #444). Root lists every directory: checked only where the listing fails.
    #[cfg(unix)]
    #[test]
    fn a_listing_that_fails_is_no_home_without_copies() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let shut = home.path().join("shut");
        std::fs::create_dir(&shut).unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refused = std::fs::read_dir(&shut).is_err();
        let listed = aside_files(&shut);
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o700)).unwrap();
        if refused {
            assert!(listed.is_err());
        }
    }

    /// A claim with its vector, forgotten, then a raw.db that lacks the forget the logs hold (put
    /// back from an older copy: no forget op and no job): the home, its uid and its vector's sha.
    fn forgotten_in_the_logs_only() -> (tempfile::TempDir, std::path::PathBuf, String, String) {
        let mut s = crate::search::b::fixture::Store::new();
        let said = "Parser caches stay in Redis, the team said.";
        let seq = s.said("s", "github.com/o/r", 1_000, said);
        let quote = "Parser caches stay in Redis";
        let gone = s.claim(seq, quote, ("decision", "decided", "user"), &[]);
        s.run();
        crate::embed_phase::fixture::vectors(&s);
        let home = s.home.path().to_owned();
        let sha: String = crate::knowledge::open(&home)
            .unwrap()
            .query_row(
                "SELECT src_sha FROM vector_keys WHERE kind = 'c' AND key = ?1",
                [&gone],
                |r| r.get(0),
            )
            .unwrap();
        forget(&home, &gone);
        let crate::search::b::fixture::Store { home: kept, raw } = s;
        drop(raw);
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        conn.execute_batch("DELETE FROM ops WHERE type = 'forget'; DELETE FROM forget_jobs;")
            .unwrap();
        (kept, home, gone, sha)
    }

    fn holds_vector(home: &Path, sha: &str) -> bool {
        crate::knowledge::open(home)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM vectors WHERE src_sha = ?1)",
                [sha],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// D5 (2b, item 5): a forget only the logs hold carries no vector of its uid into a rebuild,
    /// nor from a copy set aside: the carries read the logs too, since the pass after them is what
    /// reconciles the logs into raw.db (Codex on #444). Only this home's requests count: one of
    /// another home that shares the backup directory leaves the vector to both (Codex on #444).
    #[test]
    fn a_forget_only_the_logs_hold_carries_no_vector() {
        for other in [false, true] {
            for aside in [false, true] {
                let (_kept, home, gone, sha) = forgotten_in_the_logs_only();
                if other {
                    let mut report = Report::default();
                    for log in logs(&home, &mut report) {
                        let lines: Vec<String> = read_log(&log, None, &mut report)
                            .unwrap()
                            .into_iter()
                            .map(|mut r| {
                                r.home = "another".into();
                                r.line().unwrap()
                            })
                            .collect();
                        assert_eq!(lines.len(), 1);
                        std::fs::write(&log, lines.join("\n") + "\n").unwrap();
                    }
                }
                if aside {
                    set_aside_copy(
                        &home,
                        &home.join(format!("knowledge.db.rebuilding-{}", crate::db::now_ms())),
                    );
                    let k = crate::knowledge::open(&home).unwrap();
                    k.execute("DELETE FROM vectors WHERE src_sha = ?1", [&sha])
                        .unwrap();
                    let raw = crate::raw::open(&home).unwrap();
                    crate::embed_phase::carry_set_aside(&home, &k, &raw).unwrap();
                } else {
                    crate::worker::rebuild(&home).unwrap();
                    let raw = crate::raw::open(&home).unwrap();
                    assert_eq!(raw.forgotten(&gone).unwrap(), !other);
                }
                assert_eq!(holds_vector(&home, &sha), other, "{other} {aside}");
            }
        }
    }

    /// D5 (2b, item 7): a purge of a job not past step 3 rebuilds knowledge.db though it holds no
    /// row of the uid: rows a rewind took out leave the vector cached, and only the rebuild's carry
    /// leaves it behind (Greptile on #444), also when an earlier purge recorded step 2 and stopped.
    #[test]
    fn a_purge_before_step_3_rebuilds_without_its_rows() {
        for stopped in [false, true] {
            rebuilds_without_its_rows(stopped);
        }
    }

    fn rebuilds_without_its_rows(stopped: bool) {
        let mut s = crate::search::b::fixture::Store::new();
        let said = "Parser caches stay in Redis, the team said.";
        let seq = s.said("s", "github.com/o/r", 1_000, said);
        let quote = "Parser caches stay in Redis";
        let gone = s.claim(seq, quote, ("decision", "decided", "user"), &[]);
        s.run();
        crate::embed_phase::fixture::vectors(&s);
        let home = s.home.path().to_owned();
        let k = crate::knowledge::open(&home).unwrap();
        let sha: String = k
            .query_row(
                "SELECT src_sha FROM vector_keys WHERE kind = 'c' AND key = ?1",
                [&gone],
                |r| r.get(0),
            )
            .unwrap();
        // As a rewind leaves it: the rows and the key gone, the vector in the cache.
        for sql in [
            "DELETE FROM claims WHERE uid = ?1",
            "DELETE FROM derivations WHERE uid = ?1",
            "DELETE FROM vector_keys WHERE key = ?1",
        ] {
            k.execute(sql, [&gone]).unwrap();
        }
        drop(k);
        assert!(holds_vector(&home, &sha));
        forget(&home, &gone);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        if stopped {
            // Stopped after step 2 was recorded, before the rebuild (Codex on #444).
            let mut raw = raw::open(&home).unwrap();
            let job = uid_jobs(&home)[0].1.clone();
            raw.purge_uid(&gone, &job).unwrap();
            assert!(raw.truncate_wal(TRUNCATE_WAIT).unwrap());
            raw.finish_forget_step(&job, 2).unwrap();
        }
        assert!(purge(&home).unwrap().unwrap().rebuilt);
        assert!(!holds_vector(&home, &sha));
    }

    /// A knowledge.db whose metadata cannot be read is not taken for none (Codex on #444).
    #[cfg(unix)]
    #[test]
    fn a_knowledge_db_that_cannot_be_looked_up_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let locked = home.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::os::unix::fs::symlink(
            locked.join("knowledge.db"),
            home.path().join("knowledge.db"),
        )
        .unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refused = std::fs::metadata(locked.join("knowledge.db"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
        let looked = knowledge_holding(home.path(), &HashSet::from(["u".to_owned()]));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Root looks into every directory: the check applies where the look is refused.
        if refused {
            assert!(looked.is_err());
        }
    }

    /// A log copy that cannot be read stops a carry rather than passing over the uids it may hold:
    /// the rebuild puts knowledge.db back and says so (CodeRabbit on #444).
    #[cfg(unix)]
    #[test]
    fn a_log_that_cannot_be_read_stops_the_carry() {
        use std::os::unix::fs::PermissionsExt;
        let (_kept, home, _, _) = forgotten_in_the_logs_only();
        std::fs::remove_file(crate::backup::dir(&home).unwrap().join(LOG)).unwrap();
        let log = home.join(LOG);
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refused = std::fs::read(&log).is_err();
        let rebuilt = crate::worker::rebuild(&home);
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();
        if refused {
            let said = format!("{:#}", rebuilt.unwrap_err());
            assert!(said.contains("could not be carried"), "{said}");
            assert!(said.contains("could not be read"), "{said}");
        }
        // Nor can a carry go on without the backup directory's copy when its place is unknown.
        std::fs::write(home.join("config.toml"), "[backup]\ndir = 42\n").unwrap();
        assert!(with_logged(&home, Forgotten::default(), None).is_err());
    }

    /// D5 (2b, item 7): bodies a restore puts back while the purge runs, after its step 2, keep the
    /// job from step 3, and the next continuation rewrites them again (Codex on #444).
    #[test]
    fn bodies_put_back_during_the_purge_leave_the_job_to_the_next_continuation() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let before = op_rows(&home);
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let at = home.clone();
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let conn = rusqlite::Connection::open(at.join("raw.db")).unwrap();
                for (device, seq, _, body) in &before {
                    conn.execute(
                        "UPDATE ops SET body = ?3 WHERE device = ?1 AND op_seq = ?2",
                        rusqlite::params![device, seq, body],
                    )
                    .unwrap();
                }
            }))
        });
        let purged = purge(&home).unwrap().unwrap();
        let job = uid_jobs(&home)[0].1.clone();
        assert_eq!(purged.unfinished, std::slice::from_ref(&job));
        assert!(say_purged(&purged).is_err());
        assert_eq!(uid_jobs(&home)[0].2, 2);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.ops, 1);
        assert!(purged.unfinished.is_empty());
        assert_eq!(uid_jobs(&home)[0].2, 5);
    }

    /// D5 (2b, item 8, test 7): a worker that finds a uid job its purge left starts `oboete forget
    /// --continue`, at most once in ten minutes and not while a purge runs; none once it is done.
    #[test]
    fn the_worker_starts_a_continuation_once_in_ten_minutes() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        let home = s.home.path().to_owned();
        forget(&home, &uid);
        CONTINUED.set(0);
        s.run();
        assert_eq!(CONTINUED.get(), 1);
        s.run();
        assert_eq!(CONTINUED.get(), 1, "looked again within ten minutes");
        let ago = |home: &Path| {
            let at = crate::db::now_ms() - 11 * 60_000;
            std::fs::write(home.join("state/forget-continue"), at.to_string()).unwrap();
        };
        ago(&home);
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.join("state/purge.lock"))
            .unwrap();
        held.lock().unwrap();
        s.run();
        assert_eq!(CONTINUED.get(), 1, "started beside a purge that runs");
        drop(held);
        ago(&home);
        s.run();
        assert_eq!(CONTINUED.get(), 2);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        ago(&home);
        crate::worker::run_once(&home).unwrap();
        assert_eq!(CONTINUED.get(), 2, "started with nothing to continue");
    }

    /// D5 (2b, item 7): a uid knowledge.db still holds after the purge's rebuild came from an op the
    /// purge did not find: the job stops, the purge fails saying so, `status` and doctor name it,
    /// and neither a worker nor the next purge goes on with it.
    #[test]
    fn a_uid_the_rebuild_leaves_in_knowledge_db_stops_its_job() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        left_after_the_rebuild(&home, &uid);
        let purged = purge(&home).unwrap().unwrap();
        let job = uid_jobs(&home)[0].1.clone();
        assert_eq!(purged.stopped, std::slice::from_ref(&job));
        assert!(say_purged(&purged).unwrap_err().to_string().contains(&job));
        assert_eq!(status(&home).unwrap()[0].local, STOPPED);
        let (lines, stopped) = doctor_lines(&home).unwrap();
        assert!(stopped && lines.len() == 1 && lines[0].contains(&job));
        CONTINUED.set(0);
        continue_due(&home, &raw::open(&home).unwrap());
        assert_eq!(CONTINUED.get(), 0);
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// A purge's rebuild on this thread leaves `uid` in knowledge.db, as an op the purge did not
    /// find would: its job stops (D5 item 7).
    fn left_after_the_rebuild(home: &Path, uid: &str) {
        let (at, left) = (home.to_owned(), uid.to_owned());
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                crate::knowledge::open(&at)
                    .unwrap()
                    .execute(
                        "INSERT INTO claims(uid, op_device, op_seq) VALUES (?1, 'd', 1)",
                        [&left],
                    )
                    .unwrap();
            }))
        });
    }

    /// The live ops segments of the backup location, of every device, by name: in op order for
    /// each.
    fn ops_segments(home: &Path) -> Vec<PathBuf> {
        segments_of(home, true)
    }

    /// The live segments of the backup location of one kind (ops or records), of every device, by
    /// name: in seq order for each.
    fn segments_of(home: &Path, ops: bool) -> Vec<PathBuf> {
        let dir = crate::backup::dir(home).unwrap();
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(crate::backup::segment_name)
                    .is_some_and(|(is_ops, ..)| is_ops == ops)
            })
            .collect();
        found.sort();
        found
    }

    fn segment_text(path: &Path) -> String {
        let bytes = std::fs::read(path).unwrap();
        String::from_utf8(zstd::stream::decode_all(&bytes[..]).unwrap()).unwrap()
    }

    /// The hash a seal names for the file at `path`.
    fn file_sha(path: &Path) -> String {
        Sha256::digest(std::fs::read(path).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The names of the files in the backup location that hold `CANARY`, each segment's text read
    /// decompressed.
    fn backup_canaries(home: &Path) -> Vec<String> {
        backup_hits(home, CANARY)
    }

    /// The names of the files in the backup location that hold `text`, each segment's text read
    /// decompressed.
    fn backup_hits(home: &Path, text: &str) -> Vec<String> {
        let holds = |b: &[u8]| b.windows(text.len()).any(|w| w == text.as_bytes());
        let mut found = Vec::new();
        for e in std::fs::read_dir(crate::backup::dir(home).unwrap())
            .unwrap()
            .flatten()
        {
            let bytes = std::fs::read(e.path()).unwrap();
            if holds(&bytes) || holds(&zstd::stream::decode_all(&bytes[..]).unwrap_or_default()) {
                found.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        found
    }

    /// `canary_claim`'s claim and a kept one, backed up in two ops segments or more, the first
    /// holding the canary's op and the later ones only ops of the kept claim: the store, the
    /// canary's uid and the kept claim's.
    fn backed_up_canary() -> (crate::search::b::fixture::Store, String, String) {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = canary_claim(&mut s);
        let kept = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        crate::backup::export(s.home.path()).unwrap();
        let change = serde_json::json!({"uid": kept, "muted": true});
        s.raw
            .append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        crate::backup::export(s.home.path()).unwrap();
        let ops = ops_segments(s.home.path());
        assert!(ops.len() >= 2, "{ops:?}");
        assert!(segment_text(&ops[0]).contains(CANARY));
        assert!(!ops[1..].iter().any(|p| segment_text(p).contains(CANARY)));
        (s, uid, kept)
    }

    /// D5 (2c, decisions 2-4, tests 1 and 3): a purge rewrites every ops segment of the backup
    /// location that holds an op of the forgotten uid, of any device, live or set aside, changing
    /// only that op's body; a set-aside one that does not decompress is removed, and so are a
    /// damaged one with a line it cannot judge and the files a write left before its rename (an
    /// export's, a purge's). No file there holds the text then; a file of another name is not
    /// touched. A restore from the rewritten segments keeps the uid forgotten and every op the
    /// segments hold, and the export after it writes only new ops (test 5).
    #[test]
    fn a_purge_rewrites_the_backup_segments_without_the_uids_text() {
        let (s, uid, _) = backed_up_canary();
        let home = s.home.path().to_owned();
        let dir = crate::backup::dir(&home).unwrap();
        let device = s.raw.device().to_owned();
        let (first, second) = match &ops_segments(&home)[..] {
            [first, second, ..] => (first.clone(), second.clone()),
            other => panic!("{other:?}"),
        };
        let name = first.file_name().unwrap().to_str().unwrap().to_owned();
        let before = segment_text(&first);
        // Another device's, as a home copied from another machine leaves them (decision 2):
        // sealed, and with no seal.
        let tail = name.strip_prefix(&format!("{device}-")).unwrap().to_owned();
        let (sealed, unsealed) = (
            dir.join(format!("0ther0-{tail}")),
            dir.join(format!("0ther1-{tail}")),
        );
        for other in [&sealed, &unsealed] {
            std::fs::copy(&first, other).unwrap();
        }
        std::fs::copy(
            dir.join(format!("{name}.sha256")),
            format!("{}.sha256", sealed.display()),
        )
        .unwrap();
        // Set aside, under the name an export sets a segment past raw's end aside with
        // (MUST-M14): whole, and one that does not decompress.
        let aside = |dir: &Path| {
            dir.join(crate::backup::fresh_name(dir, &format!("{name}.quarantined-")).unwrap())
        };
        let whole = aside(&dir);
        std::fs::copy(&first, &whole).unwrap();
        let broken = aside(&dir);
        std::fs::write(&broken, format!("not zstd: {CANARY}")).unwrap();
        // An export's segment, left before its rename; what a purge's writes left before theirs
        // (Greptile on #453), of a segment this purge does not rewrite: its replacement's file and
        // a set-aside segment's rewrite in place.
        let other = second.file_name().unwrap().to_str().unwrap();
        let left = [
            dir.join(format!(".{name}.tmp")),
            dir.join(format!("..{other}.purge.tmp")),
            dir.join(format!(".{other}.quarantined-1.tmp")),
        ];
        for part in &left {
            std::fs::copy(&first, part).unwrap();
        }
        // A damaged segment that decompresses, the canary's line cut short (Codex on #453), and a
        // file of the owner's under a name like a set-aside segment's.
        let cut = dir.join(format!("0ther2-{tail}"));
        let damaged: Vec<&str> = before
            .lines()
            .map(|l| {
                if l.contains(CANARY) {
                    &l[..l.len() - 2]
                } else {
                    l
                }
            })
            .collect();
        std::fs::write(
            &cut,
            zstd::bulk::compress(damaged.join("\n").as_bytes(), 3).unwrap(),
        )
        .unwrap();
        let notes = [
            dir.join("notes.ops.zst.quarantined-old"),
            dir.join("notes!-000000000001-000000000002.ops.zst"),
            dir.join("d-000000000000-000000000000.ops.zst"),
            dir.join("d-000000000002-000000000001.ops.zst"),
        ];
        for file in &notes {
            std::fs::write(file, "the owner's notes").unwrap();
        }
        let shown = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        let counted = crate::backup::OpsPurge {
            live: 3,
            aside: 1,
            removed: 2,
        };
        assert_eq!(shown.uid.unwrap().segments, Some(counted));
        assert!(
            left.iter().all(|p| p.exists()) && broken.exists() && cut.exists(),
            "the preview changed a file"
        );
        forget(&home, &uid);
        let job = uid_jobs(&home)[0].1.clone();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.segments, purged.segments_removed), (4, 2));
        assert!(!broken.exists() && !cut.exists());
        for part in &left {
            assert!(!part.exists(), "{part:?}");
        }
        assert_eq!(backup_canaries(&home), Vec::<String>::new());
        for file in &notes {
            assert_eq!(std::fs::read_to_string(file).unwrap(), "the owner's notes");
        }
        let after = segment_text(&first);
        assert_eq!(after.lines().count(), before.lines().count());
        let mut changed = 0;
        for (was, now) in before.lines().zip(after.lines()) {
            if !was.contains(CANARY) {
                assert_eq!(now, was);
                continue;
            }
            let (was, now): (serde_json::Value, serde_json::Value) = (
                serde_json::from_str(was).unwrap(),
                serde_json::from_str(now).unwrap(),
            );
            for key in ["device", "op_seq", "type", "ts", "batch"] {
                assert_eq!(now[key], was[key], "{key}");
            }
            assert_eq!(
                now["body"],
                serde_json::json!({ "forgotten": job }).to_string()
            );
            changed += 1;
        }
        assert_eq!(changed, 1);
        for copy in [&sealed, &unsealed, &whole] {
            assert_eq!(segment_text(copy), after);
        }
        for segment in [&first, &sealed] {
            let seal = std::fs::read_to_string(format!("{}.sha256", segment.display())).unwrap();
            assert_eq!(seal.lines().count(), 1, "{seal}");
            assert!(seal.starts_with(&file_sha(segment)), "{seal}");
        }
        assert!(!PathBuf::from(format!("{}.sha256", unsealed.display())).exists());
        assert_eq!(uid_jobs(&home)[0].2, 5);
        assert_eq!(status(&home).unwrap()[0].local, DONE);
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());

        // Test 5: raw.db lost, restored from the rewritten segments.
        let mine: Vec<PathBuf> = ops_segments(&home)
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(&device)
            })
            .collect();
        let backed_up: Vec<(i64, String)> = mine
            .iter()
            .flat_map(|p| {
                segment_text(p)
                    .lines()
                    .map(|l| {
                        let v: serde_json::Value = serde_json::from_str(l).unwrap();
                        (
                            v["op_seq"].as_i64().unwrap(),
                            v["body"].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        let mut raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forgotten(&uid).unwrap());
        let restored = op_rows(&home);
        for (seq, body) in &backed_up {
            assert!(
                restored
                    .iter()
                    .any(|o| (&o.0, o.1, &o.3) == (&device, *seq, body)),
                "op {seq} not restored"
            );
        }
        assert!(!restored.iter().any(|o| o.3.contains(CANARY)));
        let end = mine
            .iter()
            .filter_map(|p| p.file_name().and_then(crate::backup::segment_name))
            .map(|(.., last)| last)
            .max()
            .unwrap();
        let change = serde_json::json!({"uid": "kept-uid", "muted": true});
        raw.append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        drop(raw);
        crate::backup::export(&home).unwrap();
        let new: Vec<PathBuf> = ops_segments(&home)
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(&device)
            })
            .filter(|p| !mine.contains(p))
            .collect();
        let firsts: Vec<i64> = new
            .iter()
            .filter_map(|p| p.file_name().and_then(crate::backup::segment_name))
            .map(|(_, _, first, _)| first)
            .collect();
        assert_eq!(firsts.iter().min(), Some(&(end + 1)), "{new:?}");
        for p in &mine {
            assert!(p.exists(), "{p:?} was set aside");
        }
    }

    /// D5 (Codex on #453): a done job whose ops hold its text again (as a restore can bring the
    /// bodies back) is purged from step 2 again, and its step goes back to 2, so a rebuild that
    /// stops leaves it due; the next purge rebuilds and finishes it.
    #[test]
    fn a_job_whose_text_came_back_stays_due_until_its_rebuild_finishes() {
        let (s, uid, _) = backed_up_canary();
        let home = s.home.path().to_owned();
        let bodies: Vec<(String, i64, String)> = op_rows(&home)
            .into_iter()
            .filter(|o| o.3.contains(CANARY))
            .map(|o| (o.0, o.1, o.3))
            .collect();
        assert!(!bodies.is_empty());
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert_eq!(uid_jobs(&home)[0].2, 5);
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for (device, seq, body) in &bodies {
            conn.execute(
                "UPDATE ops SET body = ?1 WHERE device = ?2 AND op_seq = ?3",
                rusqlite::params![body, device, seq],
            )
            .unwrap();
        }
        drop(conn);
        crate::backup::FAIL_AFTER.set(Some("vectors_carried"));
        assert!(purge(&home).is_err());
        crate::backup::FAIL_AFTER.set(None);
        assert_eq!(uid_jobs(&home)[0].2, 2);
        assert_eq!(due(&raw::open(&home).unwrap()).unwrap().len(), 1);
        let purged = purge(&home).unwrap().unwrap();
        assert!(purged.rebuilt);
        assert_eq!(uid_jobs(&home)[0].2, 5);
        assert!(!op_rows(&home).iter().any(|o| o.3.contains(CANARY)));
    }

    /// D5 (2c, CodeRabbit on #453): two jobs of one uid (the logs brought a request back beside
    /// the one raw.db holds, under another job id) both reach step 4, and then step 5.
    #[test]
    fn every_job_of_a_uid_reaches_step_4() {
        let (s, uid, _) = backed_up_canary();
        let home = s.home.path().to_owned();
        forget(&home, &uid);
        let crate::search::b::fixture::Store {
            home: _kept,
            mut raw,
        } = s;
        let mut again = raw.forget_requests().unwrap().remove(0);
        again.job = "b2000000000000000000000000000002".into();
        assert_eq!(raw.forget_apply(&[again]).unwrap(), 1);
        drop(raw);
        assert_eq!(uid_jobs(&home).len(), 2);
        purge(&home).unwrap().unwrap();
        let jobs = uid_jobs(&home);
        assert!(jobs.iter().all(|j| j.0 == uid && j.2 == 5), "{jobs:?}");
        assert_eq!(backup_canaries(&home), Vec::<String>::new());
    }

    /// D5 (2c, Greptile on #453): a purge stopped inside a write, after the data and before its
    /// rename (a replacement's file, a set-aside segment's rewrite in place), leaves only hidden
    /// files the next purge takes for leftovers (`backup::leftover`); that purge leaves none, no
    /// file holds the text, and the job is done.
    #[test]
    fn a_purge_stopped_inside_a_write_leaves_only_what_the_next_one_removes() {
        for stage in ["purge_part_unrenamed", "purge_rewrite_unrenamed"] {
            let (s, uid, _) = backed_up_canary();
            let home = s.home.path().to_owned();
            let dir = crate::backup::dir(&home).unwrap();
            let first = ops_segments(&home)[0].clone();
            let name = first.file_name().unwrap().to_str().unwrap().to_owned();
            let aside = crate::backup::fresh_name(&dir, &format!("{name}.quarantined-")).unwrap();
            std::fs::copy(&first, dir.join(aside)).unwrap();
            forget(&home, &uid);
            let crate::search::b::fixture::Store { home: _kept, raw } = s;
            drop(raw);
            let hidden = || -> Vec<String> {
                std::fs::read_dir(&dir)
                    .unwrap()
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with('.'))
                    .collect()
            };
            crate::backup::FAIL_AFTER.set(Some(stage));
            assert!(purge(&home).is_err(), "{stage}");
            let left = hidden();
            assert!(!left.is_empty(), "{stage}");
            assert!(
                left.iter().all(|n| crate::backup::leftover(n)),
                "{stage}: {left:?}"
            );
            purge(&home).unwrap().unwrap();
            assert_eq!(hidden(), Vec::<String>::new(), "{stage}");
            assert_eq!(backup_canaries(&home), Vec::<String>::new(), "{stage}");
            assert_eq!(uid_jobs(&home)[0].2, 5, "{stage}");
        }
    }

    /// D5 (2c, decision 3, test 2): a purge stopped after each step of a segment's replacement
    /// leaves segments a restore takes whole, the one replaced and those after it; the next purge
    /// finishes the replacement, a one-line seal and no text, without rebuilding knowledge.db
    /// again.
    #[test]
    fn a_purge_stopped_inside_a_segments_replacement_leaves_every_op_restorable() {
        for stage in [
            "purge_segment_written",
            "purge_seal_doubled",
            "purge_segment_renamed",
            "purge_seal_single",
        ] {
            let (s, uid, _) = backed_up_canary();
            let home = s.home.path().to_owned();
            forget(&home, &uid);
            let crate::search::b::fixture::Store { home: _kept, raw } = s;
            drop(raw);
            // What a restore takes before step 4: the purge's rebuild backed up what was new.
            let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
            let (at, keep) = (home.clone(), seen.clone());
            BEFORE_STEP_3.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    *keep.borrow_mut() = crate::backup::preview_restore(&at).unwrap().backup;
                }))
            });
            crate::backup::FAIL_AFTER.set(Some(stage));
            assert!(purge(&home).is_err(), "{stage}");
            let full = seen.borrow_mut().take().unwrap();
            let now = crate::backup::preview_restore(&home)
                .unwrap()
                .backup
                .unwrap();
            assert_eq!(now.skipped_op_segments, 0, "{stage}");
            assert_eq!(
                (now.op_segments, now.op_lines, now.op_prefix_through),
                (full.op_segments, full.op_lines, full.op_prefix_through),
                "{stage}"
            );
            assert_eq!(
                status(&home).unwrap()[0].local,
                "purged here; backups pending"
            );
            let purged = purge(&home).unwrap().unwrap();
            assert!(!purged.rebuilt, "{stage}");
            let first = &ops_segments(&home)[0];
            let seal = std::fs::read_to_string(format!("{}.sha256", first.display())).unwrap();
            assert_eq!(seal.lines().count(), 1, "{stage}: {seal}");
            assert!(seal.starts_with(&file_sha(first)), "{stage}: {seal}");
            assert_eq!(backup_canaries(&home), Vec::<String>::new(), "{stage}");
            let hidden: Vec<String> = std::fs::read_dir(crate::backup::dir(&home).unwrap())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with('.'))
                .collect();
            assert_eq!(hidden, Vec::<String>::new(), "{stage}");
            assert_eq!(uid_jobs(&home)[0].2, 5, "{stage}");
        }
    }

    /// D5 (2c, decision 8, test 6): a stopped job whose backup was not rewritten yet is not
    /// rebuilt again, alone or beside a new forget whose rebuild still leaves its uid: the next
    /// purge rewrites its backup, and `status` goes on saying it stopped.
    #[test]
    fn a_stopped_job_is_not_rebuilt_again_but_its_backup_is_rewritten() {
        for another in [false, true] {
            let (s, uid, kept) = backed_up_canary();
            let home = s.home.path().to_owned();
            forget(&home, &uid);
            let crate::search::b::fixture::Store { home: _kept, raw } = s;
            drop(raw);
            left_after_the_rebuild(&home, &uid);
            crate::backup::FAIL_AFTER.set(Some("purge_segment_written"));
            assert!(purge(&home).is_err());
            assert_eq!(uid_jobs(&home)[0].2, 3);
            assert_eq!(status(&home).unwrap()[0].local, STOPPED);
            if another {
                forget(&home, &kept);
                left_after_the_rebuild(&home, &uid);
            }
            let purged = purge(&home).unwrap().unwrap();
            assert_eq!(purged.rebuilt, another);
            assert!(purged.stopped.is_empty(), "{another}: {:?}", purged.stopped);
            assert_eq!(purged.segments, if another { 2 } else { 1 });
            assert!(uid_jobs(&home).iter().all(|j| j.2 == 5));
            let local: Vec<&str> = status(&home).unwrap().iter().map(|s| s.local).collect();
            let expected: &[&str] = if another {
                &[STOPPED, DONE]
            } else {
                &[STOPPED]
            };
            assert_eq!(local, expected);
            assert_eq!(backup_canaries(&home), Vec::<String>::new());
        }
    }

    /// `home`'s raw.db copied under `name`, its WAL beside it, as a quarantine moves the pair.
    fn copy_raw(home: &Path, name: &str) -> PathBuf {
        let to = home.join(name);
        std::fs::copy(home.join("raw.db"), &to).unwrap();
        if home.join("raw.db-wal").exists() {
            std::fs::copy(home.join("raw.db-wal"), format!("{}-wal", to.display())).unwrap();
        }
        to
    }

    /// The bodies of the ops a copy of raw.db holds, by (device, op_seq).
    fn copy_ops(path: &Path) -> Vec<(String, i64, String)> {
        let conn = rusqlite::Connection::open(path).unwrap();
        let mut st = conn
            .prepare("SELECT device, op_seq, body FROM ops ORDER BY device, op_seq")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// D5 (2c, decision 5, tests 1 and 4): a purge rewrites the forgotten uid's ops in every copy
    /// of raw.db set aside, and in a stopped restore's build beside raw.db, leaving no byte of the
    /// text in the copy or beside it and every other op as it was; a copy that does not open goes
    /// with its sidecars, and a restore's partial build too.
    #[test]
    fn a_purge_rewrites_the_copies_of_raw_db_set_aside() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let named = |prefix: &str| crate::backup::fresh_name(&home, prefix).unwrap();
        s.said("s", "github.com/o/r", 500, "An older record.");
        // From before the claim: nothing in it to rewrite.
        let older = copy_raw(&home, &named("raw.db.quarantined-"));
        let older_ops = copy_ops(&older);
        let uid = canary_claim(&mut s);
        s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let copies = [
            copy_raw(&home, &named("raw.db.quarantined-")),
            copy_raw(&home, &named("raw.db.restored.quarantined-")),
            copy_raw(&home, "raw.db.restored"),
        ];
        let before: Vec<_> = copies.iter().map(|c| copy_ops(c)).collect();
        let broken = home.join(named("raw.db.quarantined-"));
        let sidecars =
            |p: &Path| ["-wal", "-shm"].map(|x| PathBuf::from(format!("{}{x}", p.display())));
        for path in std::iter::once(broken.clone()).chain(sidecars(&broken)) {
            std::fs::write(path, format!("not a database: {CANARY}")).unwrap();
        }
        let partial = copy_raw(&home, "raw.db.restoring");
        std::fs::write(home.join("raw.db.restoring-journal"), CANARY).unwrap();
        // A file of the owner's under a name like a quarantine's: not a copy.
        let notes = home.join("raw.db.quarantined-notes");
        std::fs::write(&notes, "the owner's notes").unwrap();
        let shown = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert_eq!(shown.uid.unwrap().copies, 6);
        forget(&home, &uid);
        let job = uid_jobs(&home)[0].1.clone();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.copies, purged.copies_removed), (3, 2));
        let forgotten = serde_json::json!({ "forgotten": job }).to_string();
        for (copy, was) in copies.iter().zip(&before) {
            let now = copy_ops(copy);
            assert_eq!(now.len(), was.len());
            let mut changed = 0;
            for (now, was) in now.iter().zip(was) {
                assert_eq!((&now.0, now.1), (&was.0, was.1));
                if was.2.contains(CANARY) {
                    assert_eq!(now.2, forgotten);
                    changed += 1;
                } else {
                    assert_eq!(now.2, was.2);
                }
            }
            assert_eq!(changed, 1, "{copy:?}");
        }
        let holds = |p: &Path| {
            std::fs::read(p).is_ok_and(|b| b.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()))
        };
        let mut left = Vec::new();
        for e in std::fs::read_dir(&home).unwrap().flatten() {
            if e.path().is_file() && holds(&e.path()) {
                left.push(e.file_name());
            }
        }
        assert_eq!(left, Vec::<std::ffi::OsString>::new());
        for gone in std::iter::once(broken.clone())
            .chain(sidecars(&broken))
            .chain([partial, home.join("raw.db.restoring-journal")])
        {
            assert!(!gone.exists(), "{gone:?}");
        }
        assert!(home.join("raw.db.restored").exists());
        assert_eq!(
            std::fs::read_to_string(&notes).unwrap(),
            "the owner's notes"
        );
        assert_eq!(copy_ops(&older), older_ops);
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// Slice 3c-2: a live record and an import with its identity, to be forgotten, and the records
    /// kept beside them, one an import with no identity (as v1's touch records are), exported in
    /// one record segment: the store, its device, the seqs to forget and the seqs kept.
    fn backed_up_records() -> (crate::search::b::fixture::Store, String, [i64; 2], [i64; 2]) {
        let mut s = crate::search::b::fixture::Store::new();
        let live = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Remember {RECORD_CANARY}."),
        );
        let mut event = crate::raw::test_event(
            &serde_json::json!({ "prompt": format!("Imported {RECORD_CANARY}.") }).to_string(),
        );
        (event.source, event.session, event.ts) = ("transcript".into(), "t".into(), 2_000);
        let identity = crate::raw::ImportIdentity {
            origin: origin("synthetic", "imported"),
            session: session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let captured = |event| crate::capture::Captured {
            event,
            ledger: Vec::new(),
        };
        let imported = s
            .raw
            .append_imported_origins(&[captured(event)], &[identity], "", None)
            .unwrap()[0];
        let mut touch = crate::raw::test_event(r#"{"repo":"kept without identity"}"#);
        (touch.source, touch.kind, touch.session, touch.ts) =
            ("oboete-v1".into(), "touch".into(), "v".into(), 2_500);
        let bare = s.raw.append_imported(&[captured(touch)], "", None).unwrap()[0];
        let kept = s.said("s", "github.com/o/r", 3_000, "Kept text.");
        crate::backup::export(s.home.path()).unwrap();
        assert_eq!(segments_of(s.home.path(), false).len(), 1);
        let device = s.raw.device().to_owned();
        (s, device, [live, imported], [bare, kept])
    }

    /// `seqs` forgotten one by one, and the store let go: its home, kept until dropped.
    fn forget_records(
        s: crate::search::b::fixture::Store,
        device: &str,
        seqs: [i64; 2],
    ) -> tempfile::TempDir {
        let home = s.home.path().to_owned();
        for seq in seqs {
            let target = Target::Record {
                device: device.to_owned(),
                seq,
            };
            start(&home, &preview(&home, target).unwrap()).unwrap();
        }
        let crate::search::b::fixture::Store { home: kept, raw } = s;
        drop(raw);
        kept
    }

    /// Slice 3c-2, tests 1 and 2: a purge makes each forgotten record's line, in every record
    /// segment of the backup location, of any device, live or set aside, the Removed line an export
    /// writes for it now, and rewrites its row in every copy of raw.db set aside: no file holds the
    /// text then. An import with no identity, which no forget can name, stays byte for byte, in the
    /// segments and in the copy. A damaged segment no seal vouches for is removed; a sealed one with
    /// a line this version cannot judge is rewritten around it. The seals hold, a second purge has
    /// nothing to do, and a restore from the segments brings the forgotten records back as Removed
    /// rows and the others with their text.
    #[test]
    fn a_purge_rewrites_the_record_segments_and_copies_without_the_records_text() {
        let (s, device, forgotten, kept) = backed_up_records();
        let home = s.home.path().to_owned();
        let dir = crate::backup::dir(&home).unwrap();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, '/prompt', 'r', 0, 4, 0, 'v')",
                rusqlite::params![device, forgotten[0]],
            )
            .unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        let first = segments_of(&home, false)[0].clone();
        let name = first.file_name().unwrap().to_str().unwrap().to_owned();
        let before = segment_text(&first);
        assert!(before.contains(RECORD_CANARY));
        // Another device's, sealed (2c, decision 2); one set aside; a damaged one no seal vouches
        // for, the canary's line cut short; and a sealed one with a line of a kind this version does
        // not know.
        let tail = name.strip_prefix(&format!("{device}-")).unwrap().to_owned();
        let other = dir.join(format!("0ther0-{tail}"));
        std::fs::copy(&first, &other).unwrap();
        std::fs::copy(
            dir.join(format!("{name}.sha256")),
            format!("{}.sha256", other.display()),
        )
        .unwrap();
        let aside =
            dir.join(crate::backup::fresh_name(&dir, &format!("{name}.quarantined-")).unwrap());
        std::fs::copy(&first, &aside).unwrap();
        let cut = dir.join(format!("0ther2-{tail}"));
        let damaged: Vec<&str> = before
            .lines()
            .map(|l| {
                if l.contains(RECORD_CANARY) {
                    &l[..l.len() - 2]
                } else {
                    l
                }
            })
            .collect();
        std::fs::write(
            &cut,
            zstd::bulk::compress(damaged.join("\n").as_bytes(), 3).unwrap(),
        )
        .unwrap();
        // What an export's write left before its rename.
        let left = dir.join(format!(".{name}.tmp"));
        std::fs::copy(&first, &left).unwrap();
        let unknown = r#"{"type":"a later kind"}"#;
        let future = dir.join(format!("0ther3-{tail}"));
        let text = format!("{}\n{unknown}\n", before.trim_end());
        std::fs::write(&future, zstd::bulk::compress(text.as_bytes(), 3).unwrap()).unwrap();
        std::fs::write(
            format!("{}.sha256", future.display()),
            format!("{}  0ther3-{tail}\n", file_sha(&future)),
        )
        .unwrap();
        // A device whose record this store does not hold, whose denial it does (a request another
        // machine logged): its segment's line is judged by its identity alone.
        let far = dir.join(format!("0ther9-{tail}"));
        let far_import = origin("synthetic", "far");
        let lines = [
            serde_json::json!({"device": "0ther9", "seq": 7, "type": "event", "ts": 5,
                "kind": "prompt", "agent": "claude", "session": "x", "source": "hook",
                "body": format!(r#"{{"prompt":"Far {RECORD_CANARY}."}}"#), "ledger": []}),
            serde_json::json!({"device": "0ther9", "seq": 8, "type": "event", "ts": 6,
                "kind": "prompt", "agent": "claude", "session": "y", "source": "transcript",
                "body": format!(r#"{{"prompt":"Far import {RECORD_CANARY}."}}"#), "ledger": [],
                "import_identity": {"origin": far_import, "session": "y", "ambiguous": false}}),
        ];
        std::fs::write(
            &far,
            zstd::bulk::compress(format!("{}\n{}\n", lines[0], lines[1]).as_bytes(), 3).unwrap(),
        )
        .unwrap();
        std::fs::write(
            format!("{}.sha256", far.display()),
            format!("{}  0ther9-{tail}\n", file_sha(&far)),
        )
        .unwrap();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO denied_records(origin, device, seq, ts, session, job)
                 VALUES(?1, '0ther9', 7, 5, 'x', 'far'), (?2, '0ther9', 8, NULL, 'y', 'far')",
                [live_origin("0ther9", 7, 5), far_import.clone()],
            )
            .unwrap();
        let _home = forget_records(s, &device, forgotten);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.records, 2);
        assert_eq!((purged.segments, purged.segments_removed), (5, 1));
        // Its import keeps the identity its line holds: the store has none of it.
        let far_lines: Vec<serde_json::Value> = segment_text(&far)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            far_lines,
            [
                serde_json::json!({"device": "0ther9", "seq": 7, "type": "removed",
                    "source": "hook", "kind": "prompt", "ts": 5}),
                serde_json::json!({"device": "0ther9", "seq": 8, "type": "removed",
                    "source": "transcript", "kind": "prompt", "ts": 6,
                    "import_identity": {"origin": far_import, "session": "y", "ambiguous": false}}),
            ]
        );
        assert_eq!((purged.copies, purged.copies_removed), (1, 0));
        assert!(!cut.exists() && !left.exists());
        assert_eq!(backup_hits(&home, RECORD_CANARY), Vec::<String>::new());
        let holds = |p: &Path| {
            std::fs::read(p).is_ok_and(|b| {
                b.windows(RECORD_CANARY.len())
                    .any(|w| w == RECORD_CANARY.as_bytes())
            })
        };
        let left: Vec<std::ffi::OsString> = std::fs::read_dir(&home)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file() && holds(&e.path()))
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, Vec::<std::ffi::OsString>::new());
        // Each forgotten line is the one an export writes now, every other line as it was.
        let now: std::collections::HashMap<i64, serde_json::Value> = crate::raw::open(&home)
            .unwrap()
            .export_lines(0, usize::MAX)
            .unwrap()
            .into_iter()
            .map(|(seq, l)| (seq, serde_json::from_str(&l).unwrap()))
            .collect();
        let after = segment_text(&first);
        assert_eq!(after.lines().count(), before.lines().count());
        let mut changed = 0;
        for (was, is) in before.lines().zip(after.lines()) {
            let seq = serde_json::from_str::<serde_json::Value>(was).unwrap()["seq"]
                .as_i64()
                .unwrap();
            if forgotten.contains(&seq) {
                let is: serde_json::Value = serde_json::from_str(is).unwrap();
                assert_eq!(
                    (&is, &is["type"]),
                    (&now[&seq], &serde_json::json!("removed"))
                );
                changed += 1;
            } else {
                assert_eq!(is, was, "{seq}");
            }
        }
        assert_eq!(changed, 2);
        assert!(after.contains("kept without identity"));
        assert_eq!(
            (segment_text(&other), segment_text(&aside)),
            (after.clone(), after.clone())
        );
        assert_eq!(
            segment_text(&future),
            format!("{}\n{unknown}\n", after.trim_end())
        );
        assert_eq!(crate::backup::verify(&dir).unwrap().len(), 0);
        // The copy: the forgotten rows rewritten and their ledger rows gone, the others kept.
        let conn = rusqlite::Connection::open(&copy).unwrap();
        for seq in forgotten {
            let (kind, empty): (String, bool) = conn
                .query_row(
                    "SELECT type, body IS NULL AND session IS NULL FROM records
                     WHERE device = ?1 AND seq = ?2",
                    rusqlite::params![device, seq],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!((kind.as_str(), empty), ("removed", true), "{seq}");
        }
        let ledger: i64 = conn
            .query_row("SELECT COUNT(*) FROM ledger", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ledger, 0);
        for seq in kept {
            let kind: String = conn
                .query_row(
                    "SELECT type FROM records WHERE device = ?1 AND seq = ?2",
                    rusqlite::params![device, seq],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(kind, "event", "{seq}");
        }
        drop(conn);
        assert!(holds_text(&copy, "kept without identity"));
        assert!(status(&home).unwrap().iter().all(|j| j.local == DONE));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        // raw.db lost, restored from the rewritten segments of this device.
        for file in [&other, &future, &far] {
            std::fs::remove_file(file).unwrap();
            std::fs::remove_file(format!("{}.sha256", file.display())).unwrap();
        }
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        drop(crate::backup::open_raw(&home).unwrap());
        for seq in forgotten {
            assert_eq!(row(&home, &device, seq).0, "removed", "{seq}");
        }
        for seq in kept {
            assert_eq!(row(&home, &device, seq).0, "event", "{seq}");
        }
        assert_eq!(
            store_hits(&home, RECORD_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
        assert!(!store_hits(&home, b"kept without identity").is_empty());
    }

    /// Whether the file at `path` holds `text`.
    fn holds_text(path: &Path, text: &str) -> bool {
        std::fs::read(path).is_ok_and(|b| b.windows(text.len()).any(|w| w == text.as_bytes()))
    }

    /// Slice 3c-2: a purge stopped inside a record segment's replacement leaves its record jobs at
    /// step 3, whose status says the backups are pending; the next purge finishes them without a
    /// rebuild.
    #[test]
    fn a_purge_stopped_inside_a_record_segments_replacement_finishes_on_the_next() {
        for stage in ["purge_segment_written", "purge_segment_renamed"] {
            let (s, device, forgotten, _) = backed_up_records();
            let home = s.home.path().to_owned();
            let _home = forget_records(s, &device, forgotten);
            crate::backup::FAIL_AFTER.set(Some(stage));
            assert!(purge(&home).is_err(), "{stage}");
            assert!(
                status(&home).unwrap().iter().all(|j| j.local == RAW_PURGED),
                "{stage}"
            );
            let purged = purge(&home).unwrap().unwrap();
            assert!(!purged.rebuilt, "{stage}");
            assert!(
                status(&home).unwrap().iter().all(|j| j.local == DONE),
                "{stage}"
            );
            let first = &segments_of(&home, false)[0];
            let seal = std::fs::read_to_string(format!("{}.sha256", first.display())).unwrap();
            assert_eq!(seal.lines().count(), 1, "{stage}: {seal}");
            assert!(seal.starts_with(&file_sha(first)), "{stage}: {seal}");
            assert_eq!(
                backup_hits(&home, RECORD_CANARY),
                Vec::<String>::new(),
                "{stage}"
            );
        }
    }

    /// D5 (2c, Greptile on #458): a copy holding an op whose holding a uid's text cannot be told (a
    /// claim cut short, a claim whose body names no uid, an op of a kind no name gives) may hold
    /// the text: it is removed, as a segment with such a line is. A removal stopped at a sidecar
    /// leaves the copy, which the next purge removes with what is left (Greptile, Codex on #458).
    #[test]
    fn a_copy_with_an_op_that_cannot_be_judged_is_removed_even_after_a_stopped_removal() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = canary_claim(&mut s);
        let seq = s.said("s", "github.com/o/r", 2_000, "Use tabs.");
        s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.raw
            .append_ops(&[window_op(seq, seq, "Tabs.", serde_json::json!({}))])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let damaged = [
            "UPDATE ops SET body = substr(body, 1, 10) WHERE type = 'claim' AND body LIKE '%Use tabs.%'",
            "UPDATE ops SET body = '{}' WHERE type = 'claim' AND body LIKE '%Use tabs.%'",
            "UPDATE ops SET type = 'a kind no name gives' WHERE type = 'claim' AND body LIKE '%Use tabs.%'",
            // A window's card may be in it (Codex on #472).
            "UPDATE ops SET body = substr(body, 1, 10) WHERE type = 'window'",
        ]
        .map(|sql| {
            let copy = copy_raw(
                &home,
                &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
            );
            let c = rusqlite::Connection::open(&copy).unwrap();
            assert_eq!(c.execute(sql, []).unwrap(), 1, "{sql}");
            copy
        });
        // A sidecar that cannot be removed: the removal stops there.
        let journal = PathBuf::from(format!("{}-journal", damaged[0].display()));
        std::fs::create_dir(&journal).unwrap();
        std::fs::write(journal.join("held"), "x").unwrap();
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert!(purge(&home).is_err());
        assert!(damaged[0].exists(), "the copy went before its sidecar");
        std::fs::remove_file(journal.join("held")).unwrap();
        std::fs::remove_dir(&journal).unwrap();
        // The stopped purge may have removed the others already, in the directory's order.
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.copies, 0);
        assert!(purged.copies_removed >= 1);
        for copy in &damaged {
            assert!(!copy.exists(), "{copy:?}");
            assert!(!PathBuf::from(format!("{}-wal", copy.display())).exists());
        }
        assert!(!journal.exists());
    }

    /// D5 (2c, Codex on #458): a name a quarantine gives that is a link, or another name of raw.db
    /// itself, is no copy: the purge neither opens nor removes it, and the preview counts none.
    #[cfg(unix)]
    #[test]
    fn a_link_or_another_name_of_raw_db_is_no_copy() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = canary_claim(&mut s);
        s.run();
        let home = s.home.path().to_owned();
        let named = |prefix: &str| home.join(crate::backup::fresh_name(&home, prefix).unwrap());
        let hard = named("raw.db.quarantined-");
        std::fs::hard_link(home.join("raw.db"), &hard).unwrap();
        // A link to a file of the owner's elsewhere, here a copy of raw.db: not opened through it.
        let elsewhere = tempfile::tempdir().unwrap();
        let theirs = elsewhere.path().join("theirs.db");
        std::fs::copy(home.join("raw.db"), &theirs).unwrap();
        if home.join("raw.db-wal").exists() {
            std::fs::copy(
                home.join("raw.db-wal"),
                elsewhere.path().join("theirs.db-wal"),
            )
            .unwrap();
        }
        let ops = copy_ops(&theirs);
        let soft = named("raw.db.restored.quarantined-");
        std::os::unix::fs::symlink(&theirs, &soft).unwrap();
        let shown = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert_eq!(shown.uid.unwrap().copies, 0);
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.copies, purged.copies_removed), (0, 0));
        assert!(hard.exists());
        assert!(
            std::fs::symlink_metadata(&soft)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(copy_ops(&theirs), ops);
    }

    /// D5 (2c, Codex and CodeRabbit on #458): sidecars left without their copy (a quarantine moves
    /// `-wal` before the database; a restore's cleanup removes the database before its journal)
    /// may hold the text: they go. A copy from before the op log holds no uid's op: it stays.
    #[test]
    fn sidecars_left_without_their_copy_go_and_a_copy_without_ops_stays() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = canary_claim(&mut s);
        s.run();
        let home = s.home.path().to_owned();
        let named = |prefix: &str| home.join(crate::backup::fresh_name(&home, prefix).unwrap());
        let orphan = PathBuf::from(format!("{}-wal", named("raw.db.quarantined-").display()));
        std::fs::write(&orphan, CANARY).unwrap();
        let journal = home.join("raw.db.restoring-journal");
        std::fs::write(&journal, CANARY).unwrap();
        let old = named("raw.db.restored.quarantined-");
        rusqlite::Connection::open(&old)
            .unwrap()
            .execute_batch("CREATE TABLE records(body TEXT); INSERT INTO records VALUES('kept')")
            .unwrap();
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.copies, purged.copies_removed), (0, 2));
        assert!(!orphan.exists());
        assert!(!journal.exists());
        let body: String = rusqlite::Connection::open(&old)
            .unwrap()
            .query_row("SELECT body FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(body, "kept");
    }

    /// D5 (2c, decision 5): a copy of raw.db another connection reads cannot have its WAL truncated
    /// (the old page, the text in it, would stay in the file): it is removed instead.
    #[cfg(unix)]
    #[test]
    fn a_copy_another_connection_reads_is_removed() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = canary_claim(&mut s);
        s.run();
        let home = s.home.path().to_owned();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        let reader = rusqlite::Connection::open(&copy).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let held: i64 = reader
            .query_row("SELECT COUNT(*) FROM ops", [], |r| r.get(0))
            .unwrap();
        assert!(held > 0);
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        drop(reader);
        assert_eq!((purged.copies, purged.copies_removed), (0, 1));
        assert!(!copy.exists());
    }

    /// D5 (2c, decision 7, test 7): a session's shown value written with a forgotten uid after the
    /// purge's rebuild swept them (by a hook that read before the registration, which #439's fence
    /// makes wait) loses it at step 5, and the job is done.
    #[test]
    fn step_5_sweeps_the_shown_values_again_and_the_job_is_done() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let (at, left) = (home.clone(), uid.clone());
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let value = format!(r#"{{"{left}":{{"fp":"f","body":"Use tabs."}},"k":{{"fp":"g","body":"Kept."}}}}"#);
                crate::hookstate::update(&at, "claude", "late", "shown", |_| Some(value)).unwrap();
            }))
        });
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.shown, 1);
        let shown = crate::hookstate::value(&home, "claude", "late", "shown").unwrap();
        assert!(!shown.contains(&uid) && shown.contains("Kept."), "{shown}");
        assert_eq!(uid_jobs(&home)[0].2, 5);
        assert_eq!(status(&home).unwrap()[0].local, DONE);
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// D5 (2c, decision 7): a purge whose last sweep of the hook state fails stops after step 4,
    /// and `status` says so; the next purge goes on with step 5 without a rebuild.
    #[cfg(unix)]
    #[test]
    fn a_purge_stopped_before_step_5_goes_on_with_it() {
        use std::os::unix::fs::PermissionsExt;
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        // After the rebuild's sweep: an agent's directory the last sweep cannot list.
        let agent = home.join("state/hooks/claude");
        let at = agent.clone();
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::create_dir_all(&at).unwrap();
                std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o000)).unwrap();
            }))
        });
        let stopped = purge(&home);
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
        if stopped.is_ok() {
            return; // root lists every directory
        }
        assert_eq!(uid_jobs(&home)[0].2, 4);
        assert_eq!(status(&home).unwrap()[0].local, BACKUPS_PURGED);
        let purged = purge(&home).unwrap().unwrap();
        assert!(!purged.rebuilt);
        assert_eq!(uid_jobs(&home)[0].2, 5);
        assert_eq!(status(&home).unwrap()[0].local, DONE);
    }

    /// D5: from the commit of its request, version 2 in both logs, a uid is passed over by every
    /// reader though knowledge.db still holds it: search, get, the viewer's claim, cite and the
    /// timeline.
    #[test]
    fn a_forgotten_uid_is_hidden_from_every_reader_at_once() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs for the parser.", &[]);
        let doc = s.imported(
            "o1",
            "r",
            3_000,
            "Deploy notes",
            "Deploy with the parser script.",
        );
        s.run();
        let home = s.home.path().to_owned();
        let found = |s: &crate::search::b::fixture::Store| -> Vec<String> {
            let q = crate::search::b::Query {
                text: "parser".into(),
                all: true,
                limit: 20,
                ..Default::default()
            };
            s.query(&q).hits.into_iter().map(|h| h.key).collect()
        };
        let before = found(&s);
        assert!(before.contains(&uid) && before.contains(&doc), "{before:?}");
        for u in [&uid, &doc] {
            let p = preview(&home, Target::parse_uid(u).unwrap()).unwrap();
            let (status, report) = start(&home, &p).unwrap();
            assert_eq!(status.records, 0);
            assert!(report.problems.is_empty(), "{:?}", report.problems);
        }
        let k = crate::knowledge::open(&home).unwrap();
        let held: i64 = k
            .query_row(
                "SELECT (SELECT COUNT(*) FROM claims WHERE uid = ?1)
                      + (SELECT COUNT(*) FROM imported WHERE uid = ?2)",
                [&uid, &doc],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(held, 2, "knowledge.db keeps both until the purge");
        let after = found(&s);
        assert!(!after.contains(&uid) && !after.contains(&doc), "{after:?}");
        for u in [&uid, &doc] {
            assert_eq!(crate::search::b::get(&home, u).unwrap(), None);
        }
        assert!(crate::search::b::claim(&home, &uid).unwrap().is_none());
        let cited = crate::search::b::cite(&home, std::slice::from_ref(&uid)).unwrap();
        assert_eq!(cited[0]["error"], "not a claim");
        let line = crate::search::b::timeline(&home, None, None, None, 10).unwrap();
        assert!(
            line.iter().all(|i| i.key != uid && i.key != doc),
            "{line:?}"
        );
        for log in [home.join(LOG), crate::backup::dir(&home).unwrap().join(LOG)] {
            let text = std::fs::read_to_string(&log).unwrap();
            assert_eq!(text.lines().filter(|l| l.contains("\"v\":2")).count(), 2);
            assert!(!text.contains("parser"), "{text}");
        }
    }

    /// D5: a forgotten uid comes back with a lost raw.db: from the request logs, which hold a
    /// record's request (version 1) beside it, when the backup segments predate it; then from the
    /// ops segment that holds its forget op when the logs are gone too. A reconcile adds no second
    /// op.
    #[test]
    fn a_forgotten_uid_survives_a_lost_raw_db() {
        // Imported records and documents only.
        let mut s = crate::search::b::fixture::Store::new();
        let seq = native(&mut s.raw, "a", r#"{"prompt":"a synthetic canary"}"#);
        let uid = s.imported(
            "o1",
            "r",
            3_000,
            "Deploy notes",
            "Deploy with the parser script.",
        );
        s.run();
        let crate::search::b::fixture::Store { home: dir, raw } = s;
        let home = dir.path().to_owned();
        crate::backup::export(&home).unwrap();
        start(&home, &raw.forget_preview(record(&raw, seq)).unwrap()).unwrap();
        let p = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        start(&home, &p).unwrap();
        let logged = std::fs::read_to_string(home.join(LOG)).unwrap();
        assert!(
            logged.contains("\"v\":1") && logged.contains("\"v\":2"),
            "{logged}"
        );
        drop(raw);
        let lose = |home: &Path| {
            std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
            for f in ["raw.db-wal", "raw.db-shm"] {
                let _ = std::fs::remove_file(home.join(f));
            }
        };
        let forgets = |raw: &raw::Raw| {
            raw.ops_after(raw.device(), 0, 1_000)
                .unwrap()
                .iter()
                .filter(|o| o.kind == raw::OpKind::Forget)
                .count()
        };
        lose(&home);
        let mut raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forgotten(&uid).unwrap() && !shown(&raw, seq));
        reconcile(&home, &mut raw).unwrap();
        assert_eq!(forgets(&raw), 1);
        crate::backup::export(&home).unwrap();
        drop(raw);
        for log in [home.join(LOG), crate::backup::dir(&home).unwrap().join(LOG)] {
            std::fs::remove_file(log).unwrap();
        }
        lose(&home);
        let raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forgotten(&uid).unwrap());
        assert_eq!(forgets(&raw), 1);
    }

    /// Existing request-log copies keep their hashes private even if earlier permissions were
    /// broad; the public registration path restricts the opened files before appending.
    #[cfg(unix)]
    #[test]
    fn existing_request_logs_are_private_when_registration_appends() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o644, 0o666] {
            let home = tempfile::tempdir().unwrap();
            let h = home.path();
            let mut raw = raw::open(h).unwrap();
            let seq = native(
                &mut raw,
                "existing-log-permissions",
                r#"{"prompt":"a synthetic permissions canary"}"#,
            );
            let paths = [h.join(LOG), crate::backup::dir(h).unwrap().join(LOG)];
            for path in &paths {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "").unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            let p = preview(h, record(&raw, seq)).unwrap();
            let (registered, report) = start(h, &p).unwrap();
            assert!(report.problems.is_empty(), "{report:?}");
            assert_eq!(report.copies.len(), 2);
            for path in paths {
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                    0o600,
                    "the existing request log stayed readable by other users"
                );
                let log = std::fs::read_to_string(path).unwrap();
                assert_eq!(Request::parse(log.trim()).unwrap().job, registered.job);
                assert!(!log.contains("canary"));
            }
        }
    }

    /// A malformed backup setting or unreadable config cannot undo registration or hide the
    /// missing second log copy; fixing it lets the status command reconcile that copy.
    #[test]
    fn a_backup_directory_failure_is_reported_after_registration_and_recovers() {
        for unreadable in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let h = home.path();
            let mut raw = raw::open(h).unwrap();
            let seq = native(
                &mut raw,
                "backup-log-report",
                r#"{"prompt":"a synthetic backup log canary"}"#,
            );
            let config = h.join("config.toml");
            if unreadable {
                std::fs::create_dir(&config).unwrap();
            } else {
                std::fs::write(&config, "[backup]\ndir = 42\n").unwrap();
            }
            let p = preview(h, record(&raw, seq)).unwrap();
            let (registered, report) = start(h, &p).expect("the request must stay registered");
            assert_eq!(registered.records, 1);
            assert_eq!(registered.local, "hidden; physical purge pending");
            assert_eq!(status(h).unwrap()[0].job, registered.job);
            assert!(!shown(&raw, seq));
            let primary = h.join(LOG);
            assert_eq!(report.copies, vec![primary.clone()]);
            let log = std::fs::read_to_string(&primary).unwrap();
            assert_eq!(log.lines().count(), 1);
            assert_eq!(Request::parse(log.trim()).unwrap().job, registered.job);
            assert!(!log.contains("canary"));
            assert_eq!(
                report.problems.len(),
                1,
                "backup directory resolution failed without warning: {report:?}"
            );
            assert!(
                report.problems[0].contains("backup request log directory could not be resolved")
            );
            // Hooks use raw directly: a copy problem must not stop a new event.
            raw::open(h)
                .unwrap()
                .append(&raw::test_event("a synthetic event after the log warning"))
                .unwrap();
            if unreadable {
                std::fs::remove_dir(&config).unwrap();
            }
            std::fs::write(&config, "[backup]\ndir = 'repaired-backups'\n").unwrap();
            run(h, None, None, None, None, None, None, false, true, false).unwrap();
            let second = h.join("repaired-backups").join(LOG);
            assert_eq!(std::fs::read_to_string(second).unwrap(), log);
            assert_eq!(std::fs::read_to_string(primary).unwrap(), log);
            assert_eq!(status(h).unwrap()[0].job, registered.job);
            assert_eq!(
                status(h).unwrap()[0].local,
                "hidden; physical purge pending"
            );
            assert!(reconcile(h, &mut raw).unwrap().problems.is_empty());
        }
    }

    /// Registration committed even if a later reconciliation cannot read another job. The
    /// caller receives its true registered state and the log problem, never "not registered".
    #[test]
    fn a_registered_request_keeps_its_status_when_later_reconciliation_fails() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let seq = native(
            &mut raw,
            "registered",
            r#"{"prompt":"a synthetic registered request"}"#,
        );
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO forget_jobs(id, request, started, step) VALUES('broken', 'not JSON', 0, 1)",
                [],
            )
            .unwrap();
        let (status, report) = start(home.path(), &p).expect("the request was already registered");
        assert_eq!(status.records, 1);
        assert_eq!(status.local, "hidden; physical purge pending");
        assert!(!report.problems.is_empty());
        assert!(!shown(&raw, seq));
    }

    /// The importer may have prepared a transcript batch before forget registered its cut.
    /// The append's transaction checks the current request, even with another source origin.
    #[test]
    fn a_prepared_transcript_batch_overlapping_a_legacy_forget_is_refused_before_the_cut() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let mut event = raw::test_event(r#"{"prompt":"a synthetic forgotten prompt"}"#);
        event.source = "oboete-v1".into();
        event.ts = 1_001;
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", "native-cut"),
            session: session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event: event.clone(),
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        event.source = "transcript".into();
        event.ts -= 1;
        assert_eq!(
            raw.transcript_cut(&event.agent, &event.session, None)
                .unwrap(),
            None
        );
        // A request accepted before the cross-source check: replay must retain it, including
        // when this transcript batch was prepared before the request reached raw authority.
        let request = Request {
            v: 1,
            home: raw.home_id().to_owned(),
            job: "b1000000000000000000000000000001".into(),
            started: 1_000,
            target: record(&raw, seq),
            records: vec![Record {
                device: raw.device().into(),
                seq,
                origin: origin("synthetic", "native-cut"),
                session: session(&event.agent, &event.session),
                ts: Some(event.ts + 1),
            }],
        };
        assert_eq!(raw.forget_apply(&[request]).unwrap(), 1);
        let before = raw.after(raw.device(), 0, 100).unwrap().len();
        let append = |raw: &mut raw::Raw, event: raw::Event, id: &str| {
            let identity = raw::ImportIdentity {
                origin: origin("synthetic-transcript", id),
                session: session(&event.agent, &event.session),
                ambiguous: None,
                unverified: false,
            };
            raw.append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
        };
        let refused = append(&mut raw, event.clone(), "skewed");
        assert!(
            refused.is_err(),
            "a prepared transcript batch restored an earlier-stamped copy: {refused:?}"
        );
        assert!(format!("{:#}", refused.unwrap_err()).contains("cross-source event identity"));
        assert_eq!(raw.after(raw.device(), 0, 100).unwrap().len(), before);
        event.session = "unrelated-native-session".into();
        event.body = r#"{"prompt":"unrelated history"}"#.into();
        let kept = append(&mut raw, event, "unrelated").unwrap();
        assert_eq!(kept.len(), 1);
        assert!(shown(&raw, kept[0]));
    }

    /// Rule 5: a request hides every record of its origin in the store it is applied to, at
    /// whatever seq it has, and never another record that took the old seq.
    #[test]
    fn a_request_follows_its_origin_and_never_a_reused_seq() {
        let first = tempfile::tempdir().unwrap();
        let mut raw = raw::open(first.path()).unwrap();
        let seq = native(&mut raw, "a", r#"{"prompt":"forgotten"}"#);
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        let (_, _) = start(first.path(), &p).unwrap();
        let request = raw.forget_requests().unwrap().remove(0);
        // An imported record alone keeps version 1, which an older binary applies as this one does.
        assert_eq!(request.v, 1);
        // Another store of the same device where the seq holds another record, and the origin
        // came in later at another seq.
        let second = tempfile::tempdir().unwrap();
        std::fs::copy(first.path().join("raw.db"), second.path().join("raw.db")).unwrap();
        let mut other = raw::open(second.path()).unwrap();
        let conn = rusqlite::Connection::open(second.path().join("raw.db")).unwrap();
        conn.execute_batch(
            "DELETE FROM records; DELETE FROM import_origins; DELETE FROM denied_records;
             DELETE FROM forget_jobs;",
        )
        .unwrap();
        let reused = native(&mut other, "b", r#"{"prompt":"took the seq"}"#);
        assert_eq!(reused, seq);
        let moved = native(&mut other, "a", r#"{"prompt":"forgotten"}"#);
        assert_eq!(
            other.forget_apply(std::slice::from_ref(&request)).unwrap(),
            1
        );
        assert!(shown(&other, reused));
        assert!(!shown(&other, moved));
        // Applied once.
        assert_eq!(other.forget_apply(&[request]).unwrap(), 0);
    }

    /// Slice 3a: a live record is forgotten by where and when it was recorded, never its text; a
    /// seq that a store put back from an older copy gives again holds a later record, which stays.
    #[test]
    fn a_live_record_is_forgotten_by_where_and_when_it_was_recorded() {
        const CANARY: &str = "live-forget-canary-5121";
        let first = tempfile::tempdir().unwrap();
        let mut raw = raw::open(first.path()).unwrap();
        let body = format!(r#"{{"prompt":"{CANARY}"}}"#);
        let seq = live(&mut raw, "s", 1_000, &body);
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        assert_eq!((p.count(), p.copies(), p.passed), (1, 0, 0));
        start(first.path(), &p).unwrap();
        assert!(!shown(&raw, seq));
        let request = raw.forget_requests().unwrap().remove(0);
        // A binary before slice 3a skips it as of an unknown version (Codex on #460).
        assert_eq!(request.v, 3);
        assert_eq!(
            request.records[0].origin,
            live_origin(raw.device(), seq, 1_000)
        );
        let log = std::fs::read_to_string(first.path().join(LOG)).unwrap();
        assert!(!log.contains(CANARY), "{log}");
        // Its session's transcript is not imported again: the denial counts toward the cut.
        let e = raw.transcript_cut("claude", "s", None).unwrap_err();
        assert!(e.to_string().contains("not imported"), "{e:#}");
        assert!(raw.transcript_cut("claude", "other", None).is_ok());
        let other = |name: &str, ts: i64, text: &str| {
            let dir = tempfile::tempdir().unwrap();
            for f in ["raw.db", "raw.db-wal"] {
                if first.path().join(f).exists() {
                    std::fs::copy(first.path().join(f), dir.path().join(f)).unwrap();
                }
            }
            let conn = rusqlite::Connection::open(dir.path().join("raw.db")).unwrap();
            conn.execute_batch(
                "DELETE FROM records; DELETE FROM import_origins; DELETE FROM denied_records;
                 DELETE FROM forget_jobs;",
            )
            .unwrap();
            // The same device's older copy put back in place: its file keeps the device id.
            conn.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'store_file'",
                [crate::db::store_file(&dir.path().join("raw.db"))],
            )
            .unwrap();
            drop(conn);
            let mut store = raw::open(dir.path()).unwrap();
            assert_eq!(store.device(), raw.device(), "{name}");
            assert_eq!(live(&mut store, "s", ts, text), seq, "{name}");
            assert_eq!(
                store.forget_apply(std::slice::from_ref(&request)).unwrap(),
                1
            );
            (dir, shown(&store, seq))
        };
        // A later record at the same seq stays; the record itself, put back at its seq, goes.
        assert!(other("reused", 2_000, r#"{"prompt":"took the seq"}"#).1);
        assert!(!other("restored", 1_000, &body).1);
    }

    /// Slice 3a: a live record hidden by a rescan before its first backup is backed up without
    /// its body but with its time, so once raw.db is lost the request the logs hold still names
    /// it, and its identity is kept again for every later backup to carry.
    #[test]
    fn a_live_record_backed_up_without_its_body_is_named_again_after_a_restore() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut raw = raw::open(h).unwrap();
        let seq = live(&mut raw, "s", 7_000, r#"{"prompt":"rescanned away"}"#);
        let device = raw.device().to_owned();
        raw.append_tombstone(raw::Target::Record {
            device: device.clone(),
            seq,
        })
        .unwrap();
        crate::backup::export(h).unwrap();
        start(h, &raw.forget_preview(record(&raw, seq)).unwrap()).unwrap();
        drop(raw);
        std::fs::write(h.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(h.join(f));
        }
        let raw = crate::backup::open_raw(h).unwrap();
        assert_eq!(raw.forget_requests().unwrap().len(), 1);
        let kept: String = rusqlite::Connection::open(h.join("raw.db"))
            .unwrap()
            .query_row(
                "SELECT origin FROM import_origins WHERE device = ?1 AND seq = ?2",
                rusqlite::params![device, seq],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, live_origin(&device, seq, 7_000));
    }

    /// Slice 3b, test 1: a session target, named by any of its records, takes every source's
    /// records of the session, an ambiguous transcript record among them, and no other session's;
    /// its preview says its agent, its first record's time, its records by source and device, its
    /// first prompt, and how many live records of its agent name no session. Its one request is
    /// version 4, every record goes, and its session is denied from then.
    #[test]
    fn a_session_target_takes_every_sources_records_of_the_session() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let t1 = imported(
            &mut raw,
            "transcript",
            "s",
            "t1",
            false,
            r#"{"prompt":"one"}"#,
        );
        let t2 = imported(
            &mut raw,
            "transcript",
            "s",
            "t2",
            true,
            r#"{"prompt":"one"}"#,
        );
        let v = imported(&mut raw, "oboete-v1", "s", "v1", false, r#"{"prompt":"v"}"#);
        let touch = v1_touch(&mut raw, "s", "v-touch");
        // A touch older than every record of the session is not its first record's time.
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("UPDATE records SET ts = -1 WHERE seq = ?1", [touch])
            .unwrap();
        let other = imported(
            &mut raw,
            "transcript",
            "s2",
            "t3",
            false,
            r#"{"prompt":"x"}"#,
        );
        let l1 = live(&mut raw, "s", 5_000, r#"{"prompt":"first of its own"}"#);
        let l2 = live(&mut raw, "s", 6_000, r#"{"prompt":"two"}"#);
        let other_live = live(&mut raw, "s2", 7_000, r#"{"prompt":"y"}"#);
        let idless = live(&mut raw, "unknown-d", 8_000, r#"{"prompt":"z"}"#);
        let masked = live(&mut raw, "sk-masked", 8_500, r#"{"prompt":"m"}"#);
        mask_session(home.path(), &raw, masked);
        let device = raw.device().to_owned();
        let target = session_target(home.path(), &format!("{device}:{l2}")).unwrap();
        assert_eq!(
            target,
            Target::Session {
                agent: "claude".into(),
                session: super::session("claude", "s"),
            }
        );
        // Any record of it names the same session.
        assert_eq!(
            session_target(home.path(), &format!("{device}:{t1}")).unwrap(),
            target
        );
        let p = preview(home.path(), target.clone()).unwrap();
        assert_eq!(seqs(&p.records), [t1, t2, v, touch, l1, l2]);
        // A touch keeps no time the transcript cut counts (rule 10).
        assert_eq!(p.records[3].ts, None);
        assert!(p.copies.is_empty());
        assert_eq!(
            p.scope,
            Some(ScopePreview {
                agent: Some("claude".into()),
                first: Some(0),
                sources: vec![
                    ("hook".into(), 2),
                    ("oboete-v1".into(), 2),
                    ("transcript".into(), 2)
                ],
                devices: 1,
                idless: 2,
                ..Default::default()
            })
        );
        assert_eq!(p.sample.as_deref(), Some(r#"{"prompt":"one"}"#));
        // A preview whose record names another session is refused before it is read again.
        let mut other_session = p.clone();
        other_session.records[0].session = super::session("claude", "s2");
        let e = start(home.path(), &other_session).unwrap_err();
        assert!(e.to_string().contains("differs from its preview"), "{e:#}");
        start(home.path(), &p).unwrap();
        let request = raw.forget_requests().unwrap().remove(0);
        assert_eq!(
            (request.v, seqs(&request.records)),
            (4, vec![t1, t2, v, touch, l1, l2])
        );
        for (seq, kept) in [
            (t1, false),
            (t2, false),
            (v, false),
            (touch, false),
            (l1, false),
            (l2, false),
            (other, true),
            (other_live, true),
            (idless, true),
            (masked, true),
        ] {
            assert_eq!(shown(&raw, seq), kept, "{seq}");
        }
        // A second preview of it has none left to register; another session's has its own.
        assert!(preview(home.path(), target).unwrap().records.is_empty());
        let again = preview(home.path(), session_target_of(&raw, other)).unwrap();
        assert_eq!(seqs(&again.records), [other, other_live]);
    }

    /// Slice 3b, test 1: a session target takes the session's records of another device in the
    /// store (a copied raw.db records later records under a new device), live and imported.
    #[test]
    fn a_session_target_takes_its_records_of_another_device() {
        let first = tempfile::tempdir().unwrap();
        let mut raw = raw::open(first.path()).unwrap();
        let t = imported(
            &mut raw,
            "transcript",
            "s",
            "t",
            false,
            r#"{"prompt":"one"}"#,
        );
        let l1 = live(&mut raw, "s", 5_000, r#"{"prompt":"two"}"#);
        let before = raw.device().to_owned();
        drop(raw);
        let second = tempfile::tempdir().unwrap();
        for f in ["raw.db", "raw.db-wal"] {
            if first.path().join(f).exists() {
                std::fs::copy(first.path().join(f), second.path().join(f)).unwrap();
            }
        }
        let mut raw = raw::open(second.path()).unwrap();
        let now = raw.device().to_owned();
        assert_ne!(now, before);
        let l2 = live(&mut raw, "s", 6_000, r#"{"prompt":"three"}"#);
        let target = session_target(second.path(), &format!("{now}:{l2}")).unwrap();
        let p = preview(second.path(), target).unwrap();
        let picked: Vec<(String, i64)> = p
            .records
            .iter()
            .map(|r| (r.device.clone(), r.seq))
            .collect();
        let mut expected = vec![(before.clone(), t), (before.clone(), l1), (now.clone(), l2)];
        expected.sort();
        assert_eq!(picked, expected);
        assert_eq!(p.scope.as_ref().map(|s| s.devices), Some(2));
        start(second.path(), &p).unwrap();
        for (device, seq) in [(&before, t), (&before, l1), (&now, l2)] {
            let row = raw.after(device, seq - 1, 1).unwrap();
            assert!(matches!(row[0].item, Item::Removed), "{device}:{seq}");
        }
    }

    /// A ledger row as the gate writes when it masks the session label of `seq`'s record.
    fn mask_session(home: &Path, raw: &raw::Raw, seq: i64) {
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, 'session', 'r', 0, 9, 0, 'v')",
                rusqlite::params![raw.device(), seq],
            )
            .unwrap();
    }

    /// A v1 touch (its `session_repos` row) of `session`, with origin `id`, as v1's import keeps it.
    fn v1_touch(raw: &mut raw::Raw, session: &str, id: &str) -> i64 {
        let mut event = raw::test_event("{}");
        event.source = "oboete-v1".into();
        event.kind = "touch".into();
        event.session = session.into();
        event.ts = 500;
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: super::session(&event.agent, session),
            ambiguous: None,
            unverified: false,
        };
        let captured = crate::capture::Captured {
            event,
            ledger: Vec::new(),
        };
        raw.append_imported_origins(&[captured], &[identity], "", None)
            .unwrap()[0]
    }

    /// The session target `seq`'s record names.
    fn session_target_of(raw: &raw::Raw, seq: i64) -> Target {
        let (agent, session) = raw.session_of(raw.device(), seq).unwrap();
        Target::Session { agent, session }
    }

    /// Slice 3b, test 2: a record that names no session its agent sent (none sent, or a label the
    /// gate masked) names no session target, and neither does a record that is not there.
    #[test]
    fn a_record_of_no_session_names_no_session_target() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let idless = live(&mut raw, "unknown-d", 1_000, r#"{"prompt":"z"}"#);
        let masked = live(&mut raw, "sk-masked", 2_000, r#"{"prompt":"m"}"#);
        mask_session(home.path(), &raw, masked);
        let device = raw.device().to_owned();
        for seq in [idless, masked, 99] {
            let e = session_target(home.path(), &format!("{device}:{seq}")).unwrap_err();
            assert!(
                e.to_string().contains("names no session") || e.to_string().contains("no record"),
                "{seq}: {e:#}"
            );
        }
        assert!(session_target(home.path(), "not a record").is_err());
    }

    /// Slice 3b, test 3: a session of 1,234 records is registered as three requests (500, 500 and
    /// 234), one job each, in one transaction: a failure on the second page registers none.
    /// `--status` shows them as one, and a record of the session added since the preview makes it
    /// stale.
    #[test]
    fn a_session_of_many_records_is_registered_in_pages_together() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let first = live(&mut raw, "s", 1, r#"{"prompt":"p0"}"#);
        for i in 1..1_234 {
            live(&mut raw, "s", 1 + i, &format!(r#"{{"prompt":"p{i}"}}"#));
        }
        let device = raw.device().to_owned();
        let target = session_target(home.path(), &format!("{device}:{first}")).unwrap();
        let p = preview(home.path(), target.clone()).unwrap();
        assert_eq!(p.count(), 1_234);
        let conn = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER second BEFORE INSERT ON denied_scopes
               WHEN (SELECT COUNT(*) FROM denied_scopes) >= 1
             BEGIN SELECT RAISE(ABORT, 'test'); END;",
        )
        .unwrap();
        assert!(start(home.path(), &p).is_err());
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            (
                count("forget_jobs"),
                count("denied_records"),
                count("denied_scopes")
            ),
            (0, 0, 0)
        );
        conn.execute_batch("DROP TRIGGER second").unwrap();
        let (started, _) = start(home.path(), &p).unwrap();
        assert_eq!((started.records, started.pages), (1_234, 3));
        let requests = raw.forget_requests().unwrap();
        // In the order of their random job ids.
        let mut pages: Vec<_> = requests.iter().map(|r| (r.v, r.records.len())).collect();
        pages.sort_unstable();
        assert_eq!(pages, [(4, 234), (4, 500), (4, 500)]);
        assert_eq!(
            requests
                .iter()
                .map(|r| r.job.clone())
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        let shown_status = status(home.path()).unwrap();
        assert_eq!(shown_status.len(), 1);
        assert_eq!((shown_status[0].records, shown_status[0].pages), (1_234, 3));
        assert!(
            serde_json::to_string(&shown_status[0])
                .unwrap()
                .contains("\"pages\":3")
        );
        // A record of the session after the preview: stale.
        let mut more = raw::test_event(r#"{"prompt":"later"}"#);
        more.ts = crate::db::now_ms() + 60_000;
        raw.append(&more).unwrap();
        let p = preview(home.path(), target).unwrap();
        live(
            &mut raw,
            "s",
            crate::db::now_ms() + 61_000,
            r#"{"prompt":"later still"}"#,
        );
        let e = start(home.path(), &p).unwrap_err();
        assert!(e.to_string().contains("stale"), "{e:#}");
    }

    /// Slice 3b, test 4 (D3): a forgotten session's record from before its registration that
    /// arrives later is not recorded, by a hook's append (a call that took its time before the
    /// registration), a write-back of a kept call, or a transcript import; one from after it is.
    #[test]
    fn a_forgotten_sessions_late_records_from_before_it_are_not_recorded() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut raw = raw::open(h).unwrap();
        let l0 = live(&mut raw, "s", 500, r#"{"prompt":"zero"}"#);
        let l1 = live(&mut raw, "s", 1_000, r#"{"prompt":"one"}"#);
        let t1 = imported(
            &mut raw,
            "transcript",
            "t",
            "t1",
            false,
            r#"{"prompt":"x"}"#,
        );
        let device = raw.device().to_owned();
        // One of the session's records forgotten alone first: the session's tombstone covers its
        // denial too. Another session's record forgotten alone: no tombstone covers it.
        let u0 = live(&mut raw, "u", 600, r#"{"prompt":"u"}"#);
        for seq in [l0, u0] {
            start(h, &preview(h, record(&raw, seq)).unwrap()).unwrap();
        }
        for seq in [l1, t1] {
            let target = session_target(h, &format!("{device}:{seq}")).unwrap();
            start(h, &preview(h, target).unwrap()).unwrap();
        }
        // Each session's registration has its own time: before the first, after the last.
        let times: Vec<i64> = raw
            .forget_requests()
            .unwrap()
            .iter()
            .map(|r| r.started)
            .collect();
        let (started, last) = (times[0], times[times.len() - 1]);
        let records = |raw: &raw::Raw| raw.after(raw.device(), 0, 10_000).unwrap().len();
        let before = records(&raw);
        // A hook's append that took its time before the registration, or at its very time.
        let mut late = raw::test_event(r#"{"prompt":"late"}"#);
        late.ts = started;
        assert_eq!(raw.append_with_ledger(&late, &[], "").unwrap(), None);
        late.ts = started - 1;
        assert_eq!(raw.append_with_ledger(&late, &[], "").unwrap(), None);
        assert!(raw.append(&late).is_err());
        // A kept call written back.
        crate::unwritten::keep(
            h,
            &[crate::capture::Captured {
                event: late.clone(),
                ledger: Vec::new(),
            }],
            "r",
        )
        .unwrap();
        crate::unwritten::write_back(h, &mut raw, &Default::default())
            .1
            .unwrap();
        assert_eq!(crate::unwritten::counts(h).unwrap(), (0, 0));
        // A transcript import of the transcript-only session's turn from before it.
        let mut turn = raw::test_event(r#"{"prompt":"turn"}"#);
        turn.source = "transcript".into();
        turn.session = "t".into();
        turn.ts = started - 1;
        let identity = |id: &str| raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: super::session("claude", "t"),
            ambiguous: None,
            unverified: false,
        };
        let captured = |e: &raw::Event| crate::capture::Captured {
            event: e.clone(),
            ledger: Vec::new(),
        };
        assert_eq!(
            raw.append_imported_origins(&[captured(&turn)], &[identity("t2")], "", None)
                .unwrap(),
            Vec::<i64>::new()
        );
        assert_eq!(records(&raw), before);
        // From after it, each is recorded.
        late.ts = last + 1;
        assert!(raw.append_with_ledger(&late, &[], "").unwrap().is_some());
        turn.ts = last + 1;
        assert_eq!(
            raw.append_imported_origins(&[captured(&turn)], &[identity("t3")], "", None)
                .unwrap()
                .len(),
            1
        );
        // The live session's later records too, from a transcript and from v1: its tombstone
        // decides each, and its live record's denial refuses no batch (CodeRabbit and Greptile on
        // #465). A record forget's denial outside the tombstone still refuses the batch.
        assert_eq!(raw.transcript_cut("claude", "s", Some(7)).unwrap(), Some(7));
        let in_s = |id: &str| raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: super::session("claude", "s"),
            ambiguous: None,
            unverified: false,
        };
        for source in ["transcript", "oboete-v1"] {
            let mut e = turn.clone();
            e.source = source.into();
            e.session = "s".into();
            e.ts = started - 1;
            let early = format!("{source}-early");
            assert_eq!(
                raw.append_imported_origins(&[captured(&e)], &[in_s(&early)], "", None)
                    .unwrap(),
                Vec::<i64>::new(),
                "{source}"
            );
            e.ts = last + 1;
            let later = format!("{source}-later");
            assert_eq!(
                raw.append_imported_origins(&[captured(&e)], &[in_s(&later)], "", None)
                    .unwrap()
                    .len(),
                1,
                "{source}"
            );
        }
        // The transcript-only session's denials (no time kept) are its own pages': a v1 record of
        // it from after the registration is recorded.
        let mut v1 = turn.clone();
        v1.source = "oboete-v1".into();
        v1.ts = last + 1;
        assert_eq!(
            raw.append_imported_origins(&[captured(&v1)], &[identity("v1-later")], "", None)
                .unwrap()
                .len(),
            1
        );
        let l2 = live(&mut raw, "s", last + 2, r#"{"prompt":"after"}"#);
        start(h, &preview(h, record(&raw, l2)).unwrap()).unwrap();
        assert!(raw.transcript_cut("claude", "s", None).is_err());
        assert!(raw.transcript_cut("claude", "u", None).is_err());
        // The denial comes back with the request from its log.
        let conn = rusqlite::Connection::open(h.join("raw.db")).unwrap();
        conn.execute_batch("DELETE FROM denied_scopes").unwrap();
        let requests = raw.forget_requests().unwrap();
        conn.execute_batch("DELETE FROM forget_jobs").unwrap();
        raw.forget_apply(&requests).unwrap();
        late.ts = started - 2;
        assert_eq!(raw.append_with_ledger(&late, &[], "").unwrap(), None);
    }

    /// Slice 3b (Greptile on #465): a batch that holds a forgotten session's repeated transcript
    /// record from before the forget, whose ambiguous group names a forgotten occurrence, and a
    /// later turn of the session: the old record is passed over and the turn recorded.
    #[test]
    fn a_forgotten_sessions_repeated_record_does_not_refuse_its_later_turn() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut raw = raw::open(h).unwrap();
        let zero = imported(
            &mut raw,
            "transcript",
            "t",
            "x:0",
            false,
            r#"{"prompt":"x"}"#,
        );
        imported(&mut raw, "transcript", "t", "x", true, r#"{"prompt":"x"}"#);
        let device = raw.device().to_owned();
        let target = session_target(h, &format!("{device}:{zero}")).unwrap();
        start(h, &preview(h, target).unwrap()).unwrap();
        let started = raw.forget_requests().unwrap()[0].started;
        let event = |ts: i64, body: &str| {
            let mut e = raw::test_event(body);
            e.source = "transcript".into();
            e.session = "t".into();
            e.ts = ts;
            crate::capture::Captured {
                event: e,
                ledger: Vec::new(),
            }
        };
        let identity = |id: &str, ambiguous: Option<&str>| raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: super::session("claude", "t"),
            ambiguous: ambiguous.map(|g| origin("synthetic", g)),
            unverified: false,
        };
        let seqs = raw
            .append_imported_origins(
                &[
                    event(started - 1, r#"{"prompt":"x"}"#),
                    event(started + 1, r#"{"prompt":"new"}"#),
                ],
                &[identity("x", Some("x:0")), identity("new", None)],
                "",
                None,
            )
            .unwrap();
        assert_eq!(seqs.len(), 1);
    }

    /// Slice 3b (Codex on #465): an import with no identity its importer kept names no session (one
    /// from before import identities), and a live record whose label the ledger masks is never the
    /// session's, though its label reads as the session's id.
    #[test]
    fn only_a_kept_identity_or_an_unmasked_label_names_a_session() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut raw = raw::open(h).unwrap();
        let mut legacy = raw::test_event(r#"{"prompt":"old"}"#);
        legacy.source = "transcript".into();
        legacy.session = "s".into();
        let old = raw.append(&legacy).unwrap();
        let device = raw.device().to_owned();
        let e = session_target(h, &format!("{device}:{old}")).unwrap_err();
        assert!(e.to_string().contains("names no session"), "{e:#}");
        let named = imported(
            &mut raw,
            "transcript",
            "s",
            "s1",
            false,
            r#"{"prompt":"a"}"#,
        );
        let masked = live(&mut raw, "s", 2_000, r#"{"prompt":"masked"}"#);
        mask_session(h, &raw, masked);
        let p = preview(h, session_target(h, &format!("{device}:{named}")).unwrap()).unwrap();
        assert_eq!(seqs(&p.records), [named]);
    }

    /// Slice 3b (Greptile on #465): a job a later oboete wrote, of a target or a version this one
    /// does not know, is passed over by status, the purge and the reconcile, which says so.
    #[test]
    fn a_job_a_later_oboete_wrote_is_passed_over_and_said() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut raw = raw::open(h).unwrap();
        let seq = native(&mut raw, "n", r#"{"prompt":"n"}"#);
        start(h, &preview(h, record(&raw, seq)).unwrap()).unwrap();
        let request = serde_json::to_value(&raw.forget_requests().unwrap()[0]).unwrap();
        let mut future = request.clone();
        future["target"]["kind"] = serde_json::json!("future");
        let mut later = request;
        later["v"] = serde_json::json!(99);
        let conn = rusqlite::Connection::open(h.join("raw.db")).unwrap();
        for (id, r) in [("future", future), ("later", later)] {
            conn.execute(
                "INSERT INTO forget_jobs(id, request, started, step) VALUES(?1, ?2, 0, 1)",
                rusqlite::params![id, r.to_string()],
            )
            .unwrap();
        }
        assert_eq!(raw.forget_jobs().unwrap().len(), 1);
        assert_eq!(status(h).unwrap().len(), 1);
        let report = reconcile(h, &mut raw).unwrap();
        for id in ["future", "later"] {
            assert!(
                report
                    .problems
                    .iter()
                    .any(|p| p.contains(&format!("forget job {id} "))),
                "{:?}",
                report.problems
            );
        }
    }

    /// Slice 3b-2 (Codex on #468): a repository's or a time range's target read with another device
    /// than the store's (a raw.db restored or put back since) is refused, and so are masked
    /// repository labels that read as the repository's key.
    #[test]
    fn a_scope_target_of_another_device_or_a_masked_repository_label_is_not_taken() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut raw = raw::open(h).unwrap();
        let mut e = raw::test_event(r#"{"prompt":"masked"}"#);
        e.repo = Some("[REDACTED]".into());
        e.cwd = Some("/masked-dir".into());
        let masked = raw.append(&e).unwrap();
        rusqlite::Connection::open(h.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, 'repo', 'r', 0, 9, 0, 'v')",
                rusqlite::params![raw.device(), masked],
            )
            .unwrap();
        let mut plain = raw::test_event(r#"{"prompt":"plain"}"#);
        plain.repo = Some("[REDACTED]".into());
        plain.cwd = Some("/kept-dir".into());
        let kept = raw.append(&plain).unwrap();
        // A record of it forgotten alone before: neither taken nor shown where it was recorded.
        let mut earlier = plain.clone();
        earlier.cwd = Some("/forgotten-dir".into());
        let gone = raw.append(&earlier).unwrap();
        let alone = Target::Record {
            device: raw.device().to_owned(),
            seq: gone,
        };
        start(h, &preview(h, alone).unwrap()).unwrap();
        let target = repo_target(h, "[REDACTED]").unwrap();
        let p = preview(h, target).unwrap();
        assert_eq!(seqs(&p.records), [kept]);
        // Where the records it takes were recorded, and no other (Codex on #468).
        assert_eq!(p.scope.map(|s| s.paths), Some(vec!["/kept-dir".to_owned()]));
        let other = "0".repeat(32);
        for target in [
            Target::Repo {
                repo: repo("[REDACTED]"),
                device: other.clone(),
            },
            Target::Range {
                device: other.clone(),
                since: 0,
                until: i64::MAX,
            },
        ] {
            let e = preview(h, target).unwrap_err();
            assert!(e.to_string().contains("raw.db changed"), "{e:#}");
        }
    }

    /// A live record whose session label the ledger masks is no copy of a record whose session
    /// reads the same (Greptile on #468): its label is a rule's marker, not its session's id.
    #[test]
    fn a_masked_live_label_is_no_copy() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut raw = raw::open(h).unwrap();
        let v = imported(
            &mut raw,
            "oboete-v1",
            "[REDACTED]",
            "v-masked",
            false,
            r#"{"prompt":"a v1 record"}"#,
        );
        let masked = live(&mut raw, "[REDACTED]", 5_000, r#"{"prompt":"another"}"#);
        mask_session(h, &raw, masked);
        let target = Target::Range {
            device: raw.device().to_owned(),
            since: 0,
            until: 1_000,
        };
        let p = preview(h, target).unwrap();
        assert_eq!(seqs(&p.records), [v]);
        assert!(p.copies.is_empty(), "{:?}", seqs(&p.copies));
    }

    /// Slice 3b: a registration's pages are one status line while they are in one state, and apart
    /// once their states differ; another registration's job is its own line.
    #[test]
    fn a_registrations_pages_are_one_line_while_in_one_state() {
        let target = Target::Session {
            agent: "claude".into(),
            session: super::session("claude", "s"),
        };
        let page = |job: &str, started: i64, local: &'static str| Status {
            job: job.into(),
            target: target.clone(),
            records: 500,
            started,
            local,
            pages: 1,
        };
        let shown = grouped([
            page("a", 1, PENDING),
            page("b", 1, PENDING),
            page("c", 1, STOPPED),
            page("d", 2, STOPPED),
        ]);
        let lines: Vec<(&str, usize, usize)> = shown
            .iter()
            .map(|s| (s.job.as_str(), s.records, s.pages))
            .collect();
        assert_eq!(lines, [("a", 1_000, 2), ("c", 500, 1), ("d", 500, 1)]);
    }

    /// A live record of `session` in the repository `repo` (its cwd under it), recorded at `ts`.
    fn live_in(raw: &mut raw::Raw, session: &str, repo: &str, ts: i64, body: &str) -> i64 {
        let mut event = raw::test_event(body);
        event.session = session.into();
        event.repo = Some(repo.into());
        event.cwd = Some(format!("/work/{repo}"));
        event.ts = ts;
        raw.append(&event).unwrap()
    }

    /// A transcript record of `session` in the repository `repo`, with origin `id`, at `ts`.
    fn transcript_in(raw: &mut raw::Raw, session: &str, repo: &str, id: &str, ts: i64) -> i64 {
        let mut event = raw::test_event(r#"{"prompt":"from a transcript"}"#);
        event.source = "transcript".into();
        event.session = session.into();
        event.repo = Some(repo.into());
        event.ts = ts;
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: super::session(&event.agent, session),
            ambiguous: None,
            unverified: false,
        };
        let captured = crate::capture::Captured {
            event,
            ledger: Vec::new(),
        };
        raw.append_imported_origins(&[captured], &[identity], "", None)
            .unwrap()[0]
    }

    /// A raw.db copied into a new home, where later records get a new device.
    fn copied_home(from: &Path) -> tempfile::TempDir {
        let to = tempfile::tempdir().unwrap();
        for f in ["raw.db", "raw.db-wal"] {
            if from.join(f).exists() {
                std::fs::copy(from.join(f), to.path().join(f)).unwrap();
            }
        }
        to
    }

    /// Slice 3b-2, test 1: a repository target takes its records on every device, whichever
    /// source's, and their sessions' records from another source as copies (in another repository
    /// too); not another repository's records, nor one whose repository label is masked (counted).
    /// A directory names the key capture gives it. The request pages are version 5, and every
    /// record taken goes.
    #[test]
    fn a_repository_target_takes_every_devices_records_of_it_and_their_copies() {
        const R: &str = "github.com/o/r";
        let first = tempfile::tempdir().unwrap();
        let mut raw = raw::open(first.path()).unwrap();
        let l1 = live_in(&mut raw, "s1", R, 1_000, r#"{"prompt":"first in r"}"#);
        let t1 = transcript_in(&mut raw, "s1", R, "t1", 1_500);
        let copy = transcript_in(&mut raw, "s1", "github.com/o/other", "t2", 1_600);
        let other = live_in(
            &mut raw,
            "s2",
            "github.com/o/other",
            2_000,
            r#"{"prompt":"x"}"#,
        );
        let masked = live_in(&mut raw, "s3", "[redacted]", 3_000, r#"{"prompt":"m"}"#);
        rusqlite::Connection::open(first.path().join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, 'repo', 'r', 0, 10, 0, 'v')",
                rusqlite::params![raw.device(), masked],
            )
            .unwrap();
        let before = raw.device().to_owned();
        drop(raw);
        let second = copied_home(first.path());
        let home = second.path();
        let mut raw = raw::open(home).unwrap();
        let now = raw.device().to_owned();
        let l2 = live_in(&mut raw, "s4", R, 4_000, r#"{"prompt":"second device"}"#);
        let target = repo_target(home, R).unwrap();
        assert_eq!(
            target,
            Target::Repo {
                repo: super::repo(R),
                device: now.clone()
            }
        );
        let p = preview(home, target).unwrap();
        let named = |records: &[Record]| -> Vec<(String, i64)> {
            records.iter().map(|r| (r.device.clone(), r.seq)).collect()
        };
        let mut picked = vec![
            (before.clone(), l1),
            (before.clone(), t1),
            (now.clone(), l2),
        ];
        picked.sort();
        assert_eq!(named(&p.records), picked);
        assert_eq!(named(&p.copies), [(before.clone(), copy)]);
        let s = p.scope.clone().unwrap();
        assert_eq!(
            (
                s.keys.clone(),
                s.paths.clone(),
                s.masked,
                s.devices,
                s.idless
            ),
            (vec![R.to_owned()], vec![format!("/work/{R}")], 1, 2, 0)
        );
        start(home, &p).unwrap();
        let requests = raw.forget_requests().unwrap();
        assert_eq!(
            requests
                .iter()
                .map(|r| (r.v, r.records.len()))
                .collect::<Vec<_>>(),
            [(5, 4)]
        );
        for (device, seq, kept) in [
            (&before, l1, false),
            (&before, t1, false),
            (&before, copy, false),
            (&now, l2, false),
            (&before, other, true),
            (&before, masked, true),
        ] {
            let row = raw.after(device, seq - 1, 1).unwrap();
            let gone = matches!(row[0].item, Item::Removed) || !shown_at(&raw, device, seq);
            assert_eq!(!gone, kept, "{device}:{seq}");
        }
        // A second preview of it has none left to register.
        assert!(
            preview(home, repo_target(home, R).unwrap())
                .unwrap()
                .records
                .is_empty()
        );
        // A directory is read as capture reads it.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            repo_target(home, &dir.path().to_string_lossy()).unwrap(),
            Target::Repo {
                repo: super::repo(&crate::repo::key(dir.path())),
                device: now
            }
        );
    }

    /// Whether `device`:`seq` is shown to a reader.
    fn shown_at(raw: &raw::Raw, device: &str, seq: i64) -> bool {
        raw.after(device, seq - 1, 1)
            .unwrap()
            .into_iter()
            .any(|r| r.seq == seq && matches!(r.item, Item::Event(_)))
    }

    /// Slice 3b-2, tests 2 and 3: a time range target takes this device's records in it (`until`
    /// itself not), and their sessions' records from another source as copies, outside the range
    /// and on another device too; not another device's records in it. A record that names no
    /// session is taken and counted, not refused as a record target refuses it.
    #[test]
    fn a_time_range_target_takes_this_devices_records_in_it_and_their_copies() {
        let first = tempfile::tempdir().unwrap();
        let mut raw = raw::open(first.path()).unwrap();
        let t = transcript_in(&mut raw, "s1", "github.com/o/r", "t1", 100);
        let early = live(&mut raw, "s1", 1_000, r#"{"prompt":"before it"}"#);
        let theirs = live(&mut raw, "s2", 2_000, r#"{"prompt":"the first device's"}"#);
        let before = raw.device().to_owned();
        drop(raw);
        let second = copied_home(first.path());
        let home = second.path();
        let mut raw = raw::open(home).unwrap();
        let now = raw.device().to_owned();
        let l1 = live(&mut raw, "s1", 2_500, r#"{"prompt":"in it"}"#);
        let idless = live(&mut raw, "unknown-d", 2_600, r#"{"prompt":"no session"}"#);
        let masked = live(
            &mut raw,
            "sk-masked",
            2_700,
            r#"{"prompt":"masked session"}"#,
        );
        mask_session(home, &raw, masked);
        // Rows a restore kept no body of: one an import's identity names, one nothing names.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        let (named, unnamed) = (2_800_i64, 2_900_i64);
        let next = |conn: &rusqlite::Connection| -> i64 {
            conn.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM records WHERE device = ?1",
                [&now],
                |r| r.get(0),
            )
            .unwrap()
        };
        let restored = next(&conn);
        conn.execute(
            "INSERT INTO records(device, seq, type, ts, source, kind)
             VALUES(?1, ?2, 'removed', ?3, 'transcript', 'prompt')",
            rusqlite::params![now, restored, named],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO import_origins(origin, device, seq, native_session, ambiguous)
             VALUES(?1, ?2, ?3, ?4, 0)",
            rusqlite::params![
                origin("synthetic", "restored"),
                now,
                restored,
                super::session("claude", "s9")
            ],
        )
        .unwrap();
        // A restored v1 touch: in the range by its row's time, which is no session's (rule 10).
        let touch = next(&conn);
        conn.execute(
            "INSERT INTO records(device, seq, type, ts, source, kind)
             VALUES(?1, ?2, 'removed', 1600, 'oboete-v1', 'touch')",
            rusqlite::params![now, touch],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO import_origins(origin, device, seq, native_session, ambiguous)
             VALUES(?1, ?2, ?3, ?4, 0)",
            rusqlite::params![
                origin("synthetic", "restored-touch"),
                now,
                touch,
                super::session("claude", "s9")
            ],
        )
        .unwrap();
        let bare = next(&conn);
        conn.execute(
            "INSERT INTO records(device, seq, type, ts, source, kind)
             VALUES(?1, ?2, 'removed', ?3, 'hook', 'prompt')",
            rusqlite::params![now, bare, unnamed],
        )
        .unwrap();
        let edge = live(&mut raw, "s1", 3_000, r#"{"prompt":"at until"}"#);
        // As search reads them: an ISO time, UTC without an offset; `until` itself is not in it.
        let target =
            range_target(home, "1970-01-01T00:00:01.500Z", "1970-01-01T00:00:03Z").unwrap();
        assert_eq!(
            target,
            Target::Range {
                device: now.clone(),
                since: 1_500,
                until: 3_000
            }
        );
        let p = preview(home, target).unwrap();
        assert_eq!(seqs(&p.records), [l1, idless, masked, restored, touch]);
        assert_eq!(p.passed, 1);
        // The first time is a record's in the range: not a restored row's absent one, nor a touch's.
        assert_eq!(p.scope.as_ref().and_then(|s| s.first), Some(2_500));
        assert!(p.records.iter().all(|r| r.device == now));
        let copies: Vec<(String, i64)> =
            p.copies.iter().map(|r| (r.device.clone(), r.seq)).collect();
        assert_eq!(copies, [(before.clone(), t)]);
        assert_eq!(p.scope.as_ref().map(|s| s.idless), Some(2));
        start(home, &p).unwrap();
        for (device, seq, kept) in [
            (&now, l1, false),
            (&now, idless, false),
            (&before, t, false),
            (&now, edge, true),
            (&before, early, true),
            (&before, theirs, true),
        ] {
            assert_eq!(shown_at(&raw, device, seq), kept, "{device}:{seq}");
        }
        assert!(range_target(home, "1970-01-01T00:00:03Z", "1970-01-01T00:00:01Z").is_err());
        // A date as --until runs to the end of its day, as search reads it.
        assert!(matches!(
            range_target(home, "1970-01-01", "1970-01-01").unwrap(),
            Target::Range {
                since: 0,
                until: 86_400_000,
                ..
            }
        ));
    }

    /// Slice 3b-2, test 4 (D3): after a repository's and a time range's forget, a record of the
    /// scope from before the registration that arrives later (a hook's append, a kept call
    /// written back, an import) is not recorded on the device that registered it; one of another
    /// repository, outside the range, or from after the registration is, and so is another
    /// device's record of the repository (past what the preview confirmed of it). The denials come
    /// back with the requests from the log.
    #[test]
    fn late_records_of_a_repository_or_a_time_range_from_before_it_are_not_recorded() {
        const R: &str = "github.com/o/r";
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let mut raw = raw::open(h).unwrap();
        live_in(&mut raw, "s1", R, 1_000, r#"{"prompt":"in r"}"#);
        live(&mut raw, "s2", 10_000, r#"{"prompt":"in the range"}"#);
        start(h, &preview(h, repo_target(h, R).unwrap()).unwrap()).unwrap();
        let range = Target::Range {
            device: raw.device().to_owned(),
            since: 9_000,
            until: crate::db::now_ms() + 3_600_000,
        };
        start(h, &preview(h, range).unwrap()).unwrap();
        let times: Vec<i64> = raw
            .forget_requests()
            .unwrap()
            .iter()
            .map(|r| r.started)
            .collect();
        let (started, last) = (times[0], times[times.len() - 1]);
        let event = |repo: Option<&str>, ts: i64| {
            let mut e = raw::test_event(r#"{"prompt":"late"}"#);
            e.session = "late".into();
            e.repo = repo.map(str::to_owned);
            e.ts = ts;
            e
        };
        // The repository's, from before it: not recorded, by an append or a write-back.
        assert_eq!(
            raw.append_with_ledger(&event(Some(R), 5_000), &[], "")
                .unwrap(),
            None
        );
        let records = |raw: &raw::Raw| raw.after(raw.device(), 0, 10_000).unwrap().len();
        let before = records(&raw);
        crate::unwritten::keep(
            h,
            &[crate::capture::Captured {
                event: event(Some(R), 5_000),
                ledger: Vec::new(),
            }],
            "r",
        )
        .unwrap();
        crate::unwritten::write_back(h, &mut raw, &Default::default())
            .1
            .unwrap();
        // The kept call is taken, and nothing of it is in the store.
        assert_eq!(crate::unwritten::counts(h).unwrap(), (0, 0));
        assert_eq!(records(&raw), before);
        // In the range, from before it: not recorded, nor imported.
        assert_eq!(
            raw.append_with_ledger(&event(None, 9_500), &[], "")
                .unwrap(),
            None
        );
        let mut turn = event(None, 9_500);
        turn.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", "late-turn"),
            session: super::session("claude", "late"),
            ambiguous: None,
            unverified: false,
        };
        let captured = crate::capture::Captured {
            event: turn,
            ledger: Vec::new(),
        };
        assert!(
            raw.append_imported_origins(&[captured], &[identity], "", None)
                .unwrap()
                .is_empty()
        );
        // A repository label its ledger masks is no repository's, whatever it reads as.
        let masked = [(
            "repo".to_owned(),
            crate::redact::Finding {
                rule: "r".into(),
                offset: 0,
                length: 4,
            },
        )];
        assert!(
            raw.append_with_ledger(&event(Some(R), 5_000), &masked, "")
                .unwrap()
                .is_some()
        );
        // Another repository, outside the range, or after the registration: recorded.
        for e in [
            event(Some("github.com/o/other"), 5_000),
            event(None, 5_000),
            event(Some(R), last + 1),
            event(None, last + 1),
        ] {
            assert!(
                raw.append_with_ledger(&e, &[], "").unwrap().is_some(),
                "{e:?}"
            );
        }
        assert!(started <= last);
        // Another device's record of the repository: past what the preview confirmed of it.
        drop(raw);
        let copy = copied_home(h);
        let mut elsewhere = raw::open(copy.path()).unwrap();
        assert!(
            elsewhere
                .append_with_ledger(&event(Some(R), 5_000), &[], "")
                .unwrap()
                .is_some()
        );
        // The denials come back with the requests from the log.
        let mut raw = raw::open(h).unwrap();
        let conn = rusqlite::Connection::open(h.join("raw.db")).unwrap();
        conn.execute_batch("DELETE FROM denied_scopes").unwrap();
        let requests = raw.forget_requests().unwrap();
        conn.execute_batch("DELETE FROM forget_jobs").unwrap();
        raw.forget_apply(&requests).unwrap();
        assert_eq!(
            raw.append_with_ledger(&event(Some(R), 5_000), &[], "")
                .unwrap(),
            None
        );
        assert_eq!(
            raw.append_with_ledger(&event(None, 9_600), &[], "")
                .unwrap(),
            None
        );
    }

    /// Slice 3b-2 (rule 9): a repository's or a time range's page read from a log holds records of
    /// any device, and one that names no session; one to MAX_RECORDS of them; at version 5 only.
    #[test]
    fn a_repository_or_time_range_page_is_checked_as_one() {
        let record = |device: &str, seq: i64, session: &str| Record {
            device: device.into(),
            seq,
            origin: origin("synthetic", &seq.to_string()),
            session: session.into(),
            ts: Some(seq),
        };
        let page = |v: u32, target: Target, records: Vec<Record>| Request {
            v,
            home: "h".into(),
            job: "0".repeat(32),
            started: 1,
            target,
            records,
        };
        let repo = Target::Repo {
            repo: super::repo("github.com/o/r"),
            device: "d1".into(),
        };
        let range = Target::Range {
            device: "d1".into(),
            since: 1,
            until: 2,
        };
        let s = super::session("claude", "s");
        for target in [repo.clone(), range.clone()] {
            page(5, target, vec![record("d1", 1, &s), record("d2", 7, "")])
                .check()
                .unwrap();
        }
        let full: Vec<Record> = (0..=MAX_RECORDS as i64)
            .map(|i| record("d1", i, &s))
            .collect();
        let backwards = Target::Range {
            device: "d1".into(),
            since: 2,
            until: 2,
        };
        let unhashed = Target::Repo {
            repo: "github.com/o/r".into(),
            device: "d1".into(),
        };
        let no_device = Target::Repo {
            repo: super::repo("github.com/o/r"),
            device: "not a device".into(),
        };
        for (request, why) in [
            (page(5, repo.clone(), Vec::new()), "record count"),
            (page(5, range.clone(), full), "record count"),
            (
                page(5, repo.clone(), vec![record("d1", 1, "s")]),
                "identity",
            ),
            (
                page(5, backwards, vec![record("d1", 1, &s)]),
                "invalid time range",
            ),
            (page(5, unhashed, vec![record("d1", 1, &s)]), "identity"),
            (
                page(5, no_device, vec![record("d1", 1, &s)]),
                "invalid device identity",
            ),
            (
                page(4, repo, vec![record("d1", 1, &s)]),
                "unknown request version",
            ),
            (
                page(3, range, vec![record("d1", 1, &s)]),
                "unknown request version",
            ),
        ] {
            let e = request.check().unwrap_err();
            assert!(e.to_string().contains(why), "{why}: {e:#}");
        }
    }

    /// Slice 3b (rule 9): a session's page read from a log names only that session's records, of
    /// any device, one to MAX_RECORDS of them; a session target of another version is unknown.
    #[test]
    fn a_session_page_names_only_its_session() {
        let session = super::session("claude", "s");
        let record = |device: &str, seq: i64, session: &str| Record {
            device: device.into(),
            seq,
            origin: origin("synthetic", &seq.to_string()),
            session: session.into(),
            ts: Some(seq),
        };
        let page = |v: u32, records: Vec<Record>| Request {
            v,
            home: "h".into(),
            job: "0".repeat(32),
            started: 1,
            target: Target::Session {
                agent: "claude".into(),
                session: session.clone(),
            },
            records,
        };
        page(
            4,
            vec![record("d1", 1, &session), record("d2", 7, &session)],
        )
        .check()
        .unwrap();
        let other = super::session("claude", "s2");
        let full: Vec<Record> = (0..=MAX_RECORDS as i64)
            .map(|i| record("d1", i, &session))
            .collect();
        for (request, why) in [
            (
                page(4, vec![record("d1", 1, &session), record("d1", 2, &other)]),
                "outside its target",
            ),
            (page(4, Vec::new()), "record count"),
            (page(4, full), "record count"),
            (
                page(1, vec![record("d1", 1, &session)]),
                "unknown request version",
            ),
            (
                page(3, vec![record("d1", 1, &session)]),
                "unknown request version",
            ),
        ] {
            let e = request.check().unwrap_err();
            assert!(e.to_string().contains(why), "{why}: {e:#}");
        }
    }

    /// Slice 3a: a record's copies are the records of its session from another source, which
    /// share no record-level identity with it; two sources picked in a session take every record
    /// of it. Another session, and the same source's other records, stay.
    #[test]
    fn a_records_copies_are_its_sessions_records_from_another_source() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let t1 = imported(
            &mut raw,
            "transcript",
            "s",
            "t1",
            false,
            r#"{"prompt":"one"}"#,
        );
        let t2 = imported(
            &mut raw,
            "transcript",
            "s",
            "t2",
            false,
            r#"{"prompt":"two"}"#,
        );
        let elsewhere = imported(
            &mut raw,
            "transcript",
            "s2",
            "t3",
            false,
            r#"{"prompt":"x"}"#,
        );
        let l1 = live(&mut raw, "s", 5_000, r#"{"prompt":"one"}"#);
        let l2 = live(&mut raw, "s", 6_000, r#"{"prompt":"four"}"#);
        let device = raw.device().to_owned();
        assert_eq!(
            seqs(&raw.forget_preview(record(&raw, l1)).unwrap().copies),
            [t1, t2]
        );
        assert_eq!(
            seqs(&raw.forget_preview(record(&raw, t1)).unwrap().copies),
            [l1, l2]
        );
        let both = raw
            .forget_preview(Target::Span {
                device: device.clone(),
                from: t2,
                to: l1,
            })
            .unwrap();
        assert_eq!(seqs(&both.records), [t2, elsewhere, l1]);
        assert_eq!(seqs(&both.copies), [t1, l2]);
        let p = raw.forget_preview(record(&raw, l1)).unwrap();
        start(home.path(), &p).unwrap();
        let request = raw.forget_requests().unwrap().remove(0);
        assert_eq!((request.v, seqs(&request.records)), (3, vec![l1, t1, t2]));
        for (seq, kept) in [
            (l1, false),
            (t1, false),
            (t2, false),
            (l2, true),
            (elsewhere, true),
        ] {
            assert_eq!(shown(&raw, seq), kept, "{seq}");
        }
        // A v1 record takes its session's transcript and live records.
        let v = imported(
            &mut raw,
            "oboete-v1",
            "s3",
            "v",
            false,
            r#"{"prompt":"v1"}"#,
        );
        let t = imported(
            &mut raw,
            "transcript",
            "s3",
            "t4",
            false,
            r#"{"prompt":"v1"}"#,
        );
        let l = live(&mut raw, "s3", 9_000, r#"{"prompt":"later"}"#);
        assert_eq!(
            seqs(&raw.forget_preview(record(&raw, v)).unwrap().copies),
            [t, l]
        );
    }

    /// Slice 3a: a selection whose copies take it past the cap is refused at its preview, before
    /// it is shown and confirmed, as its registration would refuse it (Codex on #460).
    #[test]
    fn a_selection_whose_copies_pass_the_cap_is_refused_at_its_preview() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let l = live(&mut raw, "s", 5_000, r#"{"prompt":"one"}"#);
        let copies = |raw: &mut raw::Raw, ids: std::ops::Range<usize>| {
            let (mut events, mut identities) = (Vec::new(), Vec::new());
            for n in ids {
                let mut event = raw::test_event(r#"{"prompt":"t"}"#);
                event.source = "transcript".into();
                event.session = "s".into();
                identities.push(raw::ImportIdentity {
                    origin: origin("synthetic", &format!("t{n}")),
                    session: super::session(&event.agent, "s"),
                    ambiguous: None,
                    unverified: false,
                });
                events.push(crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                });
            }
            raw.append_imported_origins(&events, &identities, "", None)
                .unwrap();
        };
        copies(&mut raw, 0..MAX_RECORDS - 1);
        let p = raw.forget_preview(record(&raw, l)).unwrap();
        assert_eq!((p.records.len(), p.copies.len()), (1, MAX_RECORDS - 1));
        copies(&mut raw, MAX_RECORDS - 1..MAX_RECORDS);
        let err = raw.forget_preview(record(&raw, l)).unwrap_err();
        assert!(
            format!("{err:#}").contains("their copies counted") && err.is::<TooLarge>(),
            "{err:#}"
        );
    }

    /// Slice 3a: a copy is taken whichever device's it is in this store: a copied raw.db records
    /// its later records under another device, and its earlier imports keep the first.
    #[test]
    fn a_copy_of_another_device_in_the_store_is_taken() {
        let first = tempfile::tempdir().unwrap();
        let mut raw = raw::open(first.path()).unwrap();
        let t = imported(
            &mut raw,
            "transcript",
            "s",
            "t",
            false,
            r#"{"prompt":"one"}"#,
        );
        let before = raw.device().to_owned();
        drop(raw);
        let second = tempfile::tempdir().unwrap();
        for f in ["raw.db", "raw.db-wal"] {
            if first.path().join(f).exists() {
                std::fs::copy(first.path().join(f), second.path().join(f)).unwrap();
            }
        }
        let mut raw = raw::open(second.path()).unwrap();
        assert_ne!(raw.device(), before);
        let l = live(&mut raw, "s", 5_000, r#"{"prompt":"one"}"#);
        let p = raw.forget_preview(record(&raw, l)).unwrap();
        let copies: Vec<(String, i64)> =
            p.copies.iter().map(|r| (r.device.clone(), r.seq)).collect();
        assert_eq!(copies, [(before.clone(), t)]);
        start(second.path(), &p).unwrap();
        let hidden = raw.after(&before, t - 1, 1).unwrap();
        assert!(matches!(hidden[0].item, Item::Removed));
        assert!(!shown(&raw, l));
    }

    /// Slice 3a: a live record whose session is not its agent's own id (none sent, or masked)
    /// cannot have its copies found: it is refused, and so is an import of its agent while it is
    /// there. Nothing is registered.
    #[test]
    fn a_live_record_of_no_known_session_is_refused_and_so_is_an_import_beside_it() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let t = imported(
            &mut raw,
            "transcript",
            "s",
            "t",
            false,
            r#"{"prompt":"one"}"#,
        );
        let idless = live(&mut raw, "unknown-d", 1_000, r#"{"prompt":"two"}"#);
        let e = raw.forget_preview(record(&raw, idless)).unwrap_err();
        assert!(e.to_string().contains("names no session"), "{e:#}");
        let e = raw.forget_preview(record(&raw, t)).unwrap_err();
        assert!(e.to_string().contains("unverified native copies"), "{e:#}");
        // A live record of a known session takes that transcript record as its copy: the idless
        // record may copy the transcript record, not the live one.
        let known = live(&mut raw, "s", 2_000, r#"{"prompt":"three"}"#);
        assert_eq!(
            seqs(&raw.forget_preview(record(&raw, known)).unwrap().copies),
            [t]
        );
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let masked = live(&mut raw, "s-masked", 1_000, r#"{"prompt":"four"}"#);
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, 'session', 'r', 0, 1, 0, '')",
                rusqlite::params![raw.device(), masked],
            )
            .unwrap();
        let e = raw.forget_preview(record(&raw, masked)).unwrap_err();
        assert!(e.to_string().contains("names no session"), "{e:#}");
        assert!(raw.forget_requests().unwrap().is_empty());
    }

    /// Slice 3a: an ambiguous transcript identity may change on a later import, so alone it is
    /// refused; beside a live or v1 record of its session, whose denial refuses every later
    /// transcript batch of that session, it is forgotten with it.
    #[test]
    fn an_ambiguous_transcript_record_goes_only_with_a_live_or_v1_record_of_its_session() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let alone = imported(
            &mut raw,
            "transcript",
            "a",
            "x",
            true,
            r#"{"prompt":"yes"}"#,
        );
        let e = raw.forget_preview(record(&raw, alone)).unwrap_err();
        assert!(e.to_string().contains("ambiguous"), "{e:#}");
        let twin = imported(
            &mut raw,
            "transcript",
            "s",
            "y",
            true,
            r#"{"prompt":"yes"}"#,
        );
        let l = live(&mut raw, "s", 1_000, r#"{"prompt":"go"}"#);
        assert_eq!(
            seqs(&raw.forget_preview(record(&raw, l)).unwrap().copies),
            [twin]
        );
        assert_eq!(
            seqs(&raw.forget_preview(record(&raw, twin)).unwrap().copies),
            [l]
        );
    }

    /// Rule 12's count is in the preview: a forget between a preview and its start makes it
    /// stale, and nothing is registered.
    #[test]
    fn a_preview_read_before_another_forget_is_stale() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let a = native(&mut raw, "a", r#"{"prompt":"one"}"#);
        let b = native(&mut raw, "b", r#"{"prompt":"two"}"#);
        let late = raw.forget_preview(record(&raw, a)).unwrap();
        let p = raw.forget_preview(record(&raw, b)).unwrap();
        start(home.path(), &p).unwrap();
        let e = start(home.path(), &late).unwrap_err();
        assert!(e.to_string().contains("stale"), "{e:#}");
        assert_eq!(raw.forget_requests().unwrap().len(), 1);
        assert!(shown(&raw, a));
    }

    /// Rule 9: a torn line is skipped alone, and the next append starts on a line of its own.
    #[test]
    fn a_torn_line_is_skipped_alone_and_the_next_line_starts_on_its_own() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let a = native(&mut raw, "a", r#"{"prompt":"one"}"#);
        start(home.path(), &raw.forget_preview(record(&raw, a)).unwrap()).unwrap();
        let path = home.path().join(LOG);
        let whole = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 20]).unwrap();
        let b = native(&mut raw, "b", r#"{"prompt":"two"}"#);
        start(home.path(), &raw.forget_preview(record(&raw, b)).unwrap()).unwrap();
        let mut report = Report::default();
        let read = read_log(&path, Some(raw.device()), &mut report).unwrap();
        // The torn first line is skipped; the second request, and the first written again from
        // raw.db by the reconcile after it, read.
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        let jobs: std::collections::HashSet<String> = read.into_iter().map(|r| r.job).collect();
        assert_eq!(jobs.len(), 2);
    }

    /// A secret no rule found when it was recorded, and the rule a rescan finds it with (slice 3e-1).
    const MASKED: &str = "acme-482915";
    /// #483: what a window that named a masked record's claim said, which its purge takes.
    const SHOWN: &str = "It showed the deploy notes.";
    const MASK_RULE: &str =
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";

    /// A prompt that holds `MASKED`, long enough to be stored compressed, its seq, recorded and
    /// compressed in `s`, with a ledger row whose field names it (a JSON pointer names a body's
    /// keys).
    fn masked_prompt(s: &mut crate::search::b::fixture::Store) -> (i64, String) {
        let device = s.raw.device().to_owned();
        let said = format!(
            "Deploy with {MASKED} on Fridays, {}",
            "and keep the logs. ".repeat(40)
        );
        let seq = s.said("s", "github.com/o/r", 1_000, &said);
        assert_eq!(s.raw.compress_through(&device, 0, seq).unwrap(), 1);
        rusqlite::Connection::open(s.home.path().join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger(device, seq, field, rule, offset, length, ts, ruleset)
                 VALUES(?1, ?2, ?3, 'aws', 0, 3, 1, 'v')",
                rusqlite::params![device, seq, format!("/prompt#{MASKED}")],
            )
            .unwrap();
        (seq, said)
    }

    /// Record `seq`'s body in `home`'s raw.db as stored, its compression undone.
    fn stored_body(home: &Path, device: &str, seq: i64) -> String {
        let (body, enc): (Vec<u8>, String) = rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .query_row(
                "SELECT body, enc FROM records WHERE device = ?1 AND seq = ?2",
                rusqlite::params![device, seq],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let body = match enc.as_str() {
            "zstd" => zstd::stream::decode_all(&body[..]).unwrap(),
            _ => body,
        };
        String::from_utf8(body).unwrap()
    }

    /// Slice 3e-1 (spec 6.7, Rescan range): a secret that a rule added later finds leaves no byte in
    /// raw.db (its compressed row too), its WAL, knowledge.db, any backup segment or a copy of raw.db
    /// once the purge has run: in the record's row and its ledger row's field, the claim op that
    /// quotes it, the window op whose card repeats it and the turn op that read it. The record's
    /// other words are found, its window is listed for recuration, another window keeps its text,
    /// the masks are paid and a second purge does nothing.
    #[test]
    fn a_rescan_mask_reaches_every_stored_byte() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, said) = masked_prompt(&mut s);
        let r2 = s.said("s", "github.com/o/r", 2_000, "Logs rotate daily.");
        // An imported record with no identity (no forget names one): masked all the same.
        let imported = s
            .raw
            .append(&raw::Event {
                source: "oboete-v1".into(),
                kind: "prompt".into(),
                session: "v1".into(),
                ts: 3_000,
                ..raw::test_event(&json!({ "prompt": format!("Old note {MASKED}.") }).to_string())
            })
            .unwrap();
        let home = s.home.path().to_owned();
        let claim = claim_op(&device, &[(r1, &said)], &format!("Deploys use {MASKED}."));
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        // And a claim of another sentence quoting it too, and a window of another span that
        // showed it, which only the backup and the copy hold (`Judge::widen`).
        let lone = claim_op(
            &device,
            &[(r2, "Logs rotate daily."), (r1, &said)],
            &format!("Logs and {MASKED}."),
        );
        let lone_uid = raw::op_uid(lone.0, &lone.1).unwrap();
        let seqs = s
            .raw
            .append_ops(&[
                window_op(r1, r1, &format!("They deploy with {MASKED}."), json!({})),
                turn_op(r1, r1, &[], &format!("Asked to deploy with {MASKED}.")),
                claim,
                window_op(r2, r2, "Logs rotate.", json!({})),
                window_op(
                    r2,
                    r2,
                    &format!("Shown: {MASKED}."),
                    json!({"candidates": [uid]}),
                ),
                lone,
                window_op(
                    r2,
                    r2,
                    &format!("Shown alone: {MASKED}."),
                    json!({"carried": [lone_uid]}),
                ),
            ])
            .unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        // Out of raw.db (a store put back without them), leaving none of their bytes behind.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        conn.execute_batch("PRAGMA secure_delete = ON").unwrap();
        conn.execute("DELETE FROM ops WHERE op_seq >= ?1", [seqs[5]])
            .unwrap();
        drop(conn);
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        assert!(s.raw.masks_owed().unwrap().is_some());
        assert!(
            s.raw
                .forgotten_record_keys()
                .unwrap()
                .contains(&format!("{device}:{r1}"))
        );
        let (lines, stopped) = doctor_lines(&home).unwrap();
        assert!(!stopped && lines.iter().any(|l| l.starts_with("redaction masks")));
        std::fs::remove_file(home.join("state").join("forget-continue")).unwrap();
        CONTINUED.set(0);
        continue_due(&home, &s.raw);
        assert_eq!(CONTINUED.get(), 1);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let wal = PathBuf::from(format!("{}-wal", copy.display()));
        assert!(!backup_hits(&home, MASKED).is_empty());
        assert!(holds_text(&copy, MASKED) || holds_text(&wal, MASKED));
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.masked, purged.rebuilt), (2, true));
        assert!(!stored_body(&home, &device, imported).contains(MASKED));
        assert_eq!(store_hits(&home, MASKED.as_bytes()), Vec::<&str>::new());
        assert_eq!(backup_hits(&home, MASKED), Vec::<String>::new());
        assert!(!holds_text(&copy, MASKED) && !holds_text(&wal, MASKED));
        let stars = "*".repeat(MASKED.len());
        assert!(stored_body(&home, &device, r1).contains(&format!("Deploy with {stars} on")));
        assert!(
            !crate::search::raw(&home, "Fridays", None, 5)
                .unwrap()
                .is_empty()
        );
        let bodies = op_bodies(&home);
        for seq in [seqs[0], seqs[1], seqs[2], seqs[4]] {
            assert!(bodies[&seq].get("forgotten").is_some(), "{}", bodies[&seq]);
        }
        assert_eq!(bodies[&seqs[2]], json!({"forgotten": crate::raw::MASK_JOB}));
        assert_eq!(bodies[&seqs[3]]["summary"], "Logs rotate.");
        assert!(holds_text(&copy, "Logs rotate.") || holds_text(&wal, "Logs rotate."));
        let k = crate::knowledge::open(&home).unwrap();
        let queued = |seq: i64| -> i64 {
            k.query_row(
                "SELECT COUNT(*) FROM recurate WHERE from_seq <= ?1 AND to_seq >= ?1",
                [seq],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!((queued(r1), queued(r2)), (1, 1));
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_none());
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// Slice 3e-1: a claim op made from the masked text, after the mask, stays, and its uid, the
    /// same as the one made before it, is not denied: a recuration of the window, or #186's
    /// directive, makes the claim again.
    #[test]
    fn a_claim_made_after_the_mask_stays_and_its_uid_is_not_denied() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, said) = masked_prompt(&mut s);
        let home = s.home.path().to_owned();
        let before = claim_op(&device, &[(r1, &said)], &format!("Deploys use {MASKED}."));
        let uid = raw::op_uid(before.0, &before.1).unwrap();
        let seqs = s.raw.append_ops(&[before]).unwrap();
        s.run();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        // The mask two minutes old, the claim older still.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        conn.execute("UPDATE ops SET ts = ts - 200000", []).unwrap();
        conn.execute(
            "UPDATE records SET ts = ts - 120000 WHERE type = 'tombstone'",
            [],
        )
        .unwrap();
        let stars = "*".repeat(MASKED.len());
        let now = said.replace(MASKED, &stars);
        let after = claim_op(&device, &[(r1, &now)], "Deploys use the deploy key.");
        assert_eq!(raw::op_uid(after.0, &after.1).unwrap(), uid);
        let later = s.raw.append_ops(&[after]).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let bodies = op_bodies(&home);
        assert_eq!(bodies[&seqs[0]], serde_json::json!({"forgotten": "rescan"}));
        assert_eq!(bodies[&later[0]]["body"], "Deploys use the deploy key.");
        assert!(
            !raw::open(&home)
                .unwrap()
                .forgotten_set()
                .unwrap()
                .contains(&uid)
        );
        crate::worker::run_once(&home).unwrap();
        let k = crate::knowledge::open(&home).unwrap();
        let current = crate::claims::current(&k, "github.com/o/r").unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(
            (current[0].uid.as_str(), current[0].body.as_str()),
            (uid.as_str(), "Deploys use the deploy key.")
        );
    }

    /// Slice 3e-1: a restore from segments that hold a record from before its mask, and the mask
    /// after it, owes the mask again: the purge masks the row it brought back.
    #[test]
    fn a_restore_owes_the_masks_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, _) = masked_prompt(&mut s);
        let home = s.home.path().to_owned();
        s.run();
        crate::backup::export(&home).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        crate::worker::run_once(&home).unwrap();
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_some());
        assert!(stored_body(&home, &device, r1).contains(MASKED));
        purge(&home).unwrap().unwrap();
        assert!(!stored_body(&home, &device, r1).contains(MASKED));
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_none());
    }

    /// Codex on #480: a restore from segments brings a mask's time back with its range, and one a
    /// segment written before tombstone lines kept their time brings back without it counts as
    /// late as can be, beside another mask of the record whose time is known too: a claim made
    /// from the record before the mask, and a window that named it, lose their text either way.
    #[test]
    fn a_restore_keeps_the_time_of_a_mask() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let plain = "We deploy on Fridays.";
        let r0 = s.said("s", "github.com/o/r", 500, plain);
        let (r1, said) = masked_prompt(&mut s);
        let home = s.home.path().to_owned();
        let claim = claim_op(&device, &[(r1, &said)], "Deploys on Fridays.");
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        let seq = s.raw.append_ops(&[claim]).unwrap()[0];
        let shown = window_op(r0, r0, plain, serde_json::json!({"candidates": [uid]}));
        let window = s.raw.append_ops(&[shown]).unwrap()[0];
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let masked_at = |home: &Path| -> i64 {
            rusqlite::Connection::open(home.join("raw.db"))
                .unwrap()
                .query_row(
                    "SELECT ts FROM records WHERE type = 'tombstone' AND target_seq = ?1",
                    [r1],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let at = masked_at(&home);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(f));
        }
        crate::worker::run_once(&home).unwrap();
        assert_eq!(masked_at(&home), at);
        // As an older segment brings it back, beside a mask of the record whose time is known and
        // older than the claim.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("UPDATE records SET ts = 0 WHERE type = 'tombstone'", [])
            .unwrap();
        crate::raw::open(&home)
            .unwrap()
            .append_tombstone(raw::Target::Range {
                device,
                seq: r1,
                offset: 0,
                length: 2,
            })
            .unwrap();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE records SET ts = 1 WHERE type = 'tombstone' AND ts <> 0",
                [],
            )
            .unwrap();
        purge(&home).unwrap().unwrap();
        let after = op_bodies(&home);
        assert_eq!(
            after[&seq],
            serde_json::json!({"forgotten": crate::raw::MASK_JOB})
        );
        assert_eq!(after[&window]["forgotten"], crate::raw::MASK_JOB);
    }

    /// Codex on #480: a forget's tombstone an older segment brought back without its time counts as
    /// late as can be: a window that showed the record loses its text.
    #[test]
    fn a_forget_whose_time_a_restore_lost_takes_what_was_shown() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let r1 = s.said(
            "s",
            "github.com/o/r",
            1_000,
            &format!("Parsers stay in {WINDOW_CANARY} mode."),
        );
        let said = format!("Its span: {WINDOW_CANARY}.");
        let seq = s
            .raw
            .append_ops(&[window_op(r1, r1, &said, serde_json::json!({}))])
            .unwrap()[0];
        let home = s.home.path().to_owned();
        let target = Target::Record { device, seq: r1 };
        start(&home, &preview(&home, target).unwrap()).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("UPDATE records SET ts = 0 WHERE type = 'tombstone'", [])
            .unwrap();
        purge(&home).unwrap().unwrap();
        assert!(op_bodies(&home)[&seq]["forgotten"].is_string());
        assert_eq!(
            store_hits(&home, WINDOW_CANARY.as_bytes()),
            Vec::<&str>::new()
        );
    }

    /// Slice 3e-1: ranges a rescan masked before this version, which marked nothing, are owed as a
    /// later one is: a store upgraded with them has paid none.
    #[test]
    fn masks_from_before_this_version_are_owed() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, _) = masked_prompt(&mut s);
        let home = s.home.path().to_owned();
        s.run();
        let offset = stored_body(&home, &device, r1).find(MASKED).unwrap();
        // The row alone, as an older version's rescan wrote it.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO records(device, seq, type, ts, source, target_device, target_seq,
                                     target_offset, target_length)
                 VALUES(?1, (SELECT MAX(seq) + 1 FROM records WHERE device = ?1), 'tombstone',
                        ?2, 'rescan', ?1, ?3, ?4, ?5)",
                rusqlite::params![
                    device,
                    crate::db::now_ms(),
                    r1,
                    offset as i64,
                    MASKED.len() as i64
                ],
            )
            .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_some());
        assert_eq!(purge(&home).unwrap().unwrap().masked, 1);
        assert!(!stored_body(&home, &device, r1).contains(MASKED));
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_none());
    }

    /// Slice 3e-1: a claim quoting two masked records is judged by each one's mask, not by the
    /// first one's only: masked long before the claim was written, the first does not take its
    /// text; the second, masked after, does (Codex on #480).
    #[test]
    fn each_masked_record_a_claim_quotes_is_judged_by_its_own_mask() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, said) = masked_prompt(&mut s);
        let other = "Rotate acme-731904 on Mondays.";
        let r2 = s.said("s", "github.com/o/r", 2_000, other);
        let home = s.home.path().to_owned();
        let quotes = [(r1, said.as_str()), (r2, other)];
        let seq = s
            .raw
            .append_ops(&[claim_op(&device, &quotes, "Deploys and rotations.")])
            .unwrap()[0];
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        // The first record masked ten minutes before the claim was written.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        let ts: i64 = conn
            .query_row("SELECT ts FROM ops WHERE op_seq = ?1", [seq], |r| r.get(0))
            .unwrap();
        conn.execute(
            "UPDATE records SET ts = ?1 WHERE type = 'tombstone' AND target_seq = ?2",
            rusqlite::params![ts - 600_000, r1],
        )
        .unwrap();
        drop(conn);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert_eq!(
            op_bodies(&home)[&seq],
            serde_json::json!({"forgotten": crate::raw::MASK_JOB})
        );
    }

    /// Slice 3e-1: a claim uid two derivations give, each quoting another masked record, holds its
    /// latest mask: a window that named it before then, and after the older mask, saw the text the
    /// later one hides (Codex on #480); and so does a claim op that quotes both records.
    #[test]
    fn a_window_that_named_a_claim_before_its_latest_mask_loses_its_text() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let plain = "We deploy on Fridays.";
        let r0 = s.said("s", "github.com/o/r", 500, plain);
        let (r1, said) = masked_prompt(&mut s);
        let other = "Rotate acme-731904 on Mondays.";
        let r2 = s.said("s", "github.com/o/r", 2_000, other);
        let home = s.home.path().to_owned();
        let a = claim_op(&device, &[(r0, plain), (r1, &said)], "Deploys on Fridays.");
        let b = claim_op(
            &device,
            &[(r0, plain), (r2, other)],
            "And rotations on Mondays.",
        );
        let uid = raw::op_uid(a.0, &a.1).unwrap();
        assert_eq!(raw::op_uid(b.0, &b.1).unwrap(), uid);
        let shown = window_op(
            r0,
            r0,
            "They rotate acme-731904.",
            json!({"candidates": [uid]}),
        );
        // And one claim op that quotes both masked records, named by a window as well.
        let c = claim_op(&device, &[(r1, &said), (r2, other)], "Both were said.");
        let both = raw::op_uid(c.0, &c.1).unwrap();
        let named = window_op(r0, r0, "They said both.", json!({"candidates": [both]}));
        let seqs = s.raw.append_ops(&[a, b, shown, c, named]).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        // The first derivation at t0, its record masked a second later, the second derivation at
        // t0 + 2 s, the window at t0 + 5 min, the second record masked at t0 + 10 min; the claim
        // that quotes both at t0 and the window that named it at t0 + 5 min.
        let t0 = crate::db::now_ms() - 3_600_000;
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for (seq, ts) in [
            (seqs[0], t0),
            (seqs[1], t0 + 2_000),
            (seqs[2], t0 + 300_000),
            (seqs[3], t0),
            (seqs[4], t0 + 300_000),
        ] {
            conn.execute(
                "UPDATE ops SET ts = ?1 WHERE op_seq = ?2",
                rusqlite::params![ts, seq],
            )
            .unwrap();
        }
        for (seq, ts) in [(r1, t0 + 1_000), (r2, t0 + 600_000)] {
            conn.execute(
                "UPDATE records SET ts = ?1 WHERE type = 'tombstone' AND target_seq = ?2",
                rusqlite::params![ts, seq],
            )
            .unwrap();
        }
        drop(conn);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let bodies = op_bodies(&home);
        assert!(bodies[&seqs[2]].get("forgotten").is_some());
        assert!(bodies[&seqs[4]].get("forgotten").is_some());
    }

    /// #483: a purge that stops in step 4 after the segment holding a masked record's claim, before
    /// the segment holding a window that named it, leaves the next one the claim's uid: it rewrites
    /// that window's line, and pays the masks only then. No other store holds either (a restore left
    /// them out of raw.db): the uid is kept from the segment's line.
    #[test]
    fn a_purge_stopped_between_segments_judges_a_window_by_a_kept_uid() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, said) = masked_prompt(&mut s);
        let r2 = s.said("s", "github.com/o/r", 2_000, "We deploy on Fridays.");
        let home = s.home.path().to_owned();
        let claim = claim_op(&device, &[(r1, &said)], "Deploys on Fridays.");
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        let c = s.raw.append_ops(&[claim]).unwrap()[0];
        crate::backup::export(&home).unwrap();
        let named = window_op(r2, r2, SHOWN, json!({"candidates": [uid]}));
        let w = s.raw.append_ops(&[named]).unwrap()[0];
        crate::backup::export(&home).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        // raw.db without either, as a restore that left them out makes it: the purge's rebuild
        // sets their segments aside, and step 4 rewrites each in name order.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for seq in [c, w] {
            conn.execute("DELETE FROM ops WHERE op_seq = ?1", [seq])
                .unwrap();
        }
        drop(conn);
        crate::backup::FAIL_AFTER.set(Some("purge_rewrite_renamed"));
        assert!(purge(&home).is_err());
        assert!(!backup_hits(&home, SHOWN).is_empty());
        purge(&home).unwrap().unwrap();
        assert_eq!(backup_hits(&home, SHOWN), Vec::<String>::new());
        let raw = raw::open(&home).unwrap();
        assert!(raw.masks_owed().unwrap().is_none());
        assert!(raw.keep_masked_uids(&HashMap::new()).unwrap().is_empty());
    }

    /// #483 (CodeRabbit on #497): kept uids that do not parse stop the purge, naming them, rather
    /// than pay the masks without them.
    #[test]
    fn kept_uids_that_do_not_parse_stop_the_purge() {
        let mut s = crate::search::b::fixture::Store::new();
        masked_prompt(&mut s);
        let home = s.home.path().to_owned();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('rescan_masked_uids', '{')",
                [],
            )
            .unwrap();
        let e = format!("{:#}", purge(&home).unwrap_err());
        assert!(e.contains("rescan_masked_uids"), "{e}");
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_some());
    }

    /// #483: a purge that stops after rewriting a masked record's claim in raw.db, before step 4,
    /// keeps its uid: the next one judges a segment's window that named it, though no backup or
    /// copy holds the claim and raw.db no longer holds the window.
    #[test]
    fn a_purge_stopped_after_the_masked_claims_keeps_their_uids() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, said) = masked_prompt(&mut s);
        let r2 = s.said("s", "github.com/o/r", 2_000, "We deploy on Fridays.");
        let home = s.home.path().to_owned();
        let claim = claim_op(&device, &[(r1, &said)], "Deploys on Fridays.");
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        let named = window_op(r2, r2, SHOWN, json!({"candidates": [uid]}));
        let w = s.raw.append_ops(&[named]).unwrap()[0];
        crate::backup::export(&home).unwrap();
        // The window in the segment alone, the claim, written after it, in raw.db alone.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE op_seq = ?1", [w])
            .unwrap();
        s.raw.append_ops(&[claim]).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::backup::FAIL_AFTER.set(Some("masked_claims_purged"));
        assert!(purge(&home).is_err());
        purge(&home).unwrap().unwrap();
        assert_eq!(backup_hits(&home, SHOWN), Vec::<String>::new());
    }

    /// #483: and a window only a copy of raw.db holds, the purge stopped after the ops segments,
    /// before the copies: the next one judges the copy's window by the kept uid, though the copy,
    /// set aside before the claim was written, never held it, and raw.db no longer holds the
    /// window.
    #[test]
    fn a_purge_stopped_before_the_copies_judges_a_copys_window_by_a_kept_uid() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, said) = masked_prompt(&mut s);
        let r2 = s.said("s", "github.com/o/r", 2_000, "We deploy on Fridays.");
        let home = s.home.path().to_owned();
        let claim = claim_op(&device, &[(r1, &said)], "Deploys on Fridays.");
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        let named = window_op(r2, r2, SHOWN, json!({"candidates": [uid]}));
        let w = s.raw.append_ops(&[named]).unwrap()[0];
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        // The window in the copy alone.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE op_seq = ?1", [w])
            .unwrap();
        s.raw.append_ops(&[claim]).unwrap();
        crate::backup::export(&home).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::backup::FAIL_AFTER.set(Some("ops_segments_purged"));
        assert!(purge(&home).is_err());
        assert!(holds_text(&copy, SHOWN));
        purge(&home).unwrap().unwrap();
        assert!(!holds_text(&copy, SHOWN));
    }

    /// Slice 3e-1: a restore between the purge's rebuild and its last steps, which put a row back
    /// as the segments held it, leaves the masks owed: the count is not paid over a row that holds
    /// a masked byte again (Codex on #480).
    #[test]
    fn a_restore_during_the_purge_leaves_the_masks_owed() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, _) = masked_prompt(&mut s);
        let home = s.home.path().to_owned();
        s.run();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let at = home.clone();
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(at.join("raw.db"), b"not a database at all").unwrap();
                for f in ["raw.db-wal", "raw.db-shm"] {
                    let _ = std::fs::remove_file(at.join(f));
                }
                crate::worker::run_once(&at).unwrap();
            }));
        });
        let purged = purge(&home).unwrap().unwrap();
        assert!(purged.unfinished.iter().any(|j| j == crate::raw::MASK_JOB));
        assert!(stored_body(&home, &device, r1).contains(MASKED));
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_some());
        purge(&home).unwrap().unwrap();
        assert!(!stored_body(&home, &device, r1).contains(MASKED));
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_none());
    }

    /// Slice 3e-2 (#186): a preference whose directive a rescan masks part of is written again by
    /// the purge from the masked text, with its uid: a current global claim whose body is that
    /// text, the owner's mute still on it, and no byte of the secret left in the stores. A mask of
    /// another record leaves it as it is (an op that calls itself a preference but whose body is
    /// not its quote is taken); a rule that masks more of the directive takes it and writes it
    /// again from the text masked twice, on the next run when a purge stops after taking its text.
    #[test]
    fn a_masked_preference_is_written_again_from_its_masked_text() {
        let mut s = crate::search::b::fixture::Store::new();
        let (home, device) = (s.home.path().to_owned(), s.raw.device().to_owned());
        let other = s.said("s", "github.com/o/r", 1_000, "Ship zeta-4821 today.");
        let said = format!("Use tabs, key {MASKED}, always.");
        let uid = crate::claims::pref_add(&home, &said).unwrap();
        crate::claims::mute(&home, &uid, true).unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let rules = |extra: &str| {
            let acme = r#"{ id = "acme", regex = 'acme-[0-9]{6}' }"#;
            std::fs::write(
                home.join("config.toml"),
                format!("[redaction]\nextra_rules = [{acme}{extra}]\n"),
            )
            .unwrap();
            crate::worker::run_once(&home).unwrap();
        };
        let shown = |home: &Path| -> (String, bool) {
            crate::knowledge::open(home)
                .unwrap()
                .query_row(
                    "SELECT body, muted FROM active WHERE uid = ?1 AND scope = 'global'",
                    [&uid],
                    |r| Ok((r.get(0)?, r.get::<_, Option<bool>>(1)?.unwrap_or(false))),
                )
                .unwrap()
        };
        // The preference ops that hold a body.
        let preferences = |home: &Path| -> Vec<serde_json::Value> {
            op_bodies(home)
                .into_values()
                .filter(|b| b["recipe"] == crate::claims::PREF_RECIPE)
                .collect()
        };
        rules("");
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.preferences, 1);
        let masked = said.replace(MASKED, &"*".repeat(MASKED.len()));
        assert_eq!(shown(&home), (masked.clone(), true));
        assert_eq!(store_hits(&home, MASKED.as_bytes()), Vec::<&str>::new());
        assert_eq!(backup_hits(&home, MASKED), Vec::<String>::new());
        // Another record's mask: the op written again stays as it is, in raw.db and in the ops
        // segment it was backed up to. An op that calls itself a preference, its quote the masked
        // text but its body not its quote, is not one `pref add` wrote: it loses its text.
        crate::backup::export(&home).unwrap();
        let mut odd = preferences(&home).remove(0);
        odd["body"] = format!("Keep {MASKED} handy.").into();
        let odd = crate::raw::open(&home)
            .unwrap()
            .append_ops(&[(raw::OpKind::Claim, odd)])
            .unwrap()[0];
        let mut before = op_bodies(&home);
        rules(r#", { id = "zeta", regex = 'zeta-[0-9]{4}' }"#);
        assert!(stored_body(&home, &device, other).contains("zeta-4821"));
        assert_eq!(purge(&home).unwrap().unwrap().preferences, 0);
        assert!(!stored_body(&home, &device, other).contains("zeta-4821"));
        before.insert(odd, serde_json::json!({"forgotten": crate::raw::MASK_JOB}));
        assert_eq!(op_bodies(&home), before);
        assert!(!backup_hits(&home, crate::claims::PREF_RECIPE).is_empty());
        // A rule that masks more of the directive, and a purge that stops after taking the text.
        rules(r#", { id = "zeta", regex = 'zeta-[0-9]{4}' }, { id = "tabs", regex = 'tabs' }"#);
        crate::backup::FAIL_AFTER.set(Some("masked_claims_purged"));
        assert!(purge(&home).is_err());
        assert!(preferences(&home).iter().all(|b| b["body"] != masked));
        assert_eq!(purge(&home).unwrap().unwrap().preferences, 1);
        let twice = masked.replace("tabs", "****");
        assert_eq!(shown(&home), (twice.clone(), true));
        // The ops of the uid that held the text lost it: the one written last alone holds a body.
        let bodies: Vec<serde_json::Value> = preferences(&home)
            .into_iter()
            .map(|b| b["body"].clone())
            .collect();
        assert_eq!(bodies, [serde_json::json!(twice)]);
    }

    /// Slice 8b: a preference of one repository whose directive a rescan masks part of is written
    /// again of that repository, with its uid: scope `repo`, its repository the directive's, still
    /// the owner's own.
    #[test]
    fn a_masked_preference_of_a_repository_stays_that_repositorys() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let repo = "github.com/o/r";
        s.said("s", repo, 1_000, "Hello.");
        s.run();
        let said = format!("Use tabs, key {MASKED}, always.");
        let uid = crate::claims::pref_add_in(&home, &said, Some(repo)).unwrap();
        s.run();
        let acme = r#"{ id = "acme", regex = 'acme-[0-9]{6}' }"#;
        std::fs::write(
            home.join("config.toml"),
            format!("[redaction]\nextra_rules = [{acme}]\n"),
        )
        .unwrap();
        crate::worker::run_once(&home).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().preferences, 1);
        let k = crate::knowledge::open(&home).unwrap();
        let shown: (String, String, String) = k
            .query_row(
                "SELECT body, scope, repo FROM active WHERE uid = ?1",
                [&uid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let masked = said.replace(MASKED, &"*".repeat(MASKED.len()));
        assert_eq!(shown, (masked, "repo".into(), repo.into()));
        let owned = crate::claims::owned(&k, repo).unwrap();
        assert_eq!(owned.iter().map(|c| &c.uid).collect::<Vec<_>>(), [&uid]);
    }

    /// Slice 3e-2 (Codex on #485): an op that calls itself a preference, its body its one quote and
    /// that quote reading now, but that holds more than `pref add` writes (a `why`, a field no op of
    /// ours has) is not one it wrote: a mask of its record takes its text, in raw.db and in the
    /// backup.
    #[test]
    fn an_op_shaped_as_a_preference_that_holds_more_loses_its_text() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        crate::claims::pref_add(&home, &format!("Use tabs, key {MASKED}, always.")).unwrap();
        let written = op_bodies(&home)
            .into_values()
            .find(|b| b["recipe"] == crate::claims::PREF_RECIPE)
            .unwrap();
        let mut e: crate::claims::Evidence =
            serde_json::from_value(written["evidence"][0].clone()).unwrap();
        e.quote = "Use tabs".into();
        e.length = 8;
        let prefix = serde_json::to_value(crate::claims::preference_op(e, false)).unwrap();
        let (mut why, mut extra) = (prefix.clone(), prefix);
        why["why"] = format!("key {MASKED}").into();
        extra["note"] = MASKED.into();
        let seqs = s
            .raw
            .append_ops(&[(raw::OpKind::Claim, why), (raw::OpKind::Claim, extra)])
            .unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        let bodies = op_bodies(&home);
        for seq in seqs {
            assert_eq!(
                bodies[&seq],
                serde_json::json!({"forgotten": crate::raw::MASK_JOB})
            );
        }
        assert_eq!(store_hits(&home, MASKED.as_bytes()), Vec::<&str>::new());
        assert_eq!(backup_hits(&home, MASKED), Vec::<String>::new());
    }

    /// Slice 3e-2 (CodeRabbit on #485): an op shaped as `pref add` writes one but quoting only the
    /// start of its directive has the preference's uid (its first sentence's) and does not stand in
    /// for it: once a mask takes the directive's whole quote, the preference is written again whole
    /// from the masked text, and the shorter op loses its text.
    #[test]
    fn a_shorter_quote_of_a_directive_does_not_stand_in_for_its_preference() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let said = format!("Use tabs, key {MASKED}, always.");
        let uid = crate::claims::pref_add(&home, &said).unwrap();
        let written = op_bodies(&home)
            .into_values()
            .find(|b| b["recipe"] == crate::claims::PREF_RECIPE)
            .unwrap();
        let mut e: crate::claims::Evidence =
            serde_json::from_value(written["evidence"][0].clone()).unwrap();
        e.quote = "Use tabs".into();
        e.length = 8;
        assert_eq!(crate::claims::uid("preference", &e), uid);
        let short = serde_json::to_value(crate::claims::preference_op(e, false)).unwrap();
        let short = s.raw.append_ops(&[(raw::OpKind::Claim, short)]).unwrap()[0];
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().preferences, 1);
        let body: String = crate::knowledge::open(&home)
            .unwrap()
            .query_row("SELECT body FROM active WHERE uid = ?1", [&uid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(body, said.replace(MASKED, &"*".repeat(MASKED.len())));
        assert_eq!(
            op_bodies(&home)[&short],
            serde_json::json!({"forgotten": crate::raw::MASK_JOB})
        );
    }

    /// Slice 3e-2 (Codex on #485): more preferences to write again than one append takes go in
    /// appends within its caps.
    #[test]
    fn preferences_written_again_go_in_appends_within_the_caps() {
        let mut s = crate::search::b::fixture::Store::new();
        let (home, device) = (s.home.path().to_owned(), s.raw.device().to_owned());
        let settings = crate::capture::Settings::load(&home).unwrap();
        let n = crate::raw::MAX_BATCH_OPS + 1;
        let mut ops = Vec::new();
        for i in 0..n {
            let text = format!("Rule {i} keeps {MASKED}.");
            let c = crate::capture::directive(&text, 1_000 + i as i64, &settings, None);
            let quote = crate::curate::long_text(&c.event).unwrap();
            let evidence = crate::claims::Evidence {
                device: device.clone(),
                seq: s.raw.append(&c.event).unwrap(),
                offset: 0,
                length: quote.len() as i64,
                sentence: 0,
                quote,
                claim_at: None,
            };
            let op = crate::claims::preference_op(evidence, false);
            ops.push((raw::OpKind::Claim, serde_json::to_value(op).unwrap()));
        }
        let batches = crate::raw::append_batches(&ops);
        assert_eq!(
            batches.iter().map(|b| b.len()).collect::<Vec<_>>(),
            [n - 1, 1]
        );
        for batch in batches {
            s.raw.append_ops(batch).unwrap();
        }
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().preferences, n);
    }

    /// Slice 3e-2 (Codex on #485): a copy of raw.db that holds another op at the op seq of a
    /// preference written again (a restore gives a dropped op's seq to a new op) is judged by its
    /// own body: the older claim there, which quotes the directive as it was, loses its text.
    #[test]
    fn another_op_at_a_preferences_op_seq_is_judged_by_its_body() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        s.said("s", "github.com/o/r", 1_000, "Ship zeta-4821 today.");
        let said = format!("Use tabs, key {MASKED}, always.");
        crate::claims::pref_add(&home, &said).unwrap();
        s.run();
        let preference = |home: &Path| -> (i64, serde_json::Value) {
            op_bodies(home)
                .into_iter()
                .find(|(_, b)| b["recipe"] == crate::claims::PREF_RECIPE && b.get("body").is_some())
                .unwrap()
        };
        let (_, old) = preference(&home);
        let acme = r#"{ id = "acme", regex = 'acme-[0-9]{6}' }"#;
        let rules = |extra: &str| {
            std::fs::write(
                home.join("config.toml"),
                format!("[redaction]\nextra_rules = [{acme}{extra}]\n"),
            )
            .unwrap();
            crate::worker::run_once(&home).unwrap();
        };
        rules("");
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().preferences, 1);
        let (seq, _) = preference(&home);
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        rusqlite::Connection::open(&copy)
            .unwrap()
            .execute(
                "UPDATE ops SET body = ?1 WHERE op_seq = ?2",
                rusqlite::params![old.to_string(), seq],
            )
            .unwrap();
        let wal = PathBuf::from(format!("{}-wal", copy.display()));
        assert!(holds_text(&copy, MASKED) || holds_text(&wal, MASKED));
        // Another record's mask: a purge runs, and the preference at that op seq reads now.
        rules(r#", { id = "zeta", regex = 'zeta-[0-9]{4}' }"#);
        assert_eq!(purge(&home).unwrap().unwrap().preferences, 0);
        assert!(!holds_text(&copy, MASKED) && !holds_text(&wal, MASKED));
    }

    /// Slice 3e-2: a preference forgotten by its uid is not written again when a rescan masks part
    /// of its directive.
    #[test]
    fn a_forgotten_preference_is_not_written_again() {
        let s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let uid = crate::claims::pref_add(&home, &format!("Use tabs, key {MASKED}.")).unwrap();
        s.run();
        start(&home, &preview(&home, Target::Uid { uid }).unwrap()).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().preferences, 0);
        assert!(
            op_bodies(&home)
                .values()
                .all(|b| b["recipe"] != crate::claims::PREF_RECIPE)
        );
    }

    /// Slice 3e-4 (#478, spec 6.7's Rescan range): a rule added later masks what it finds in the
    /// text of every op, not only in what was made from a masked record: a claim that quotes
    /// another record and repeats the secret in its body, a window that named it and repeated it,
    /// the owner's correction of that claim, a work state and an imported document. No byte is left
    /// in raw.db, its WAL, knowledge.db, the ops segments or a copy of raw.db; each op reads as
    /// before (the claim is current with its body masked, the correction still sets its status),
    /// no uid an op names changes, and the window is not listed for recuration. A second purge
    /// scans nothing.
    #[test]
    fn a_rule_added_later_masks_the_text_of_every_op() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, _) = masked_prompt(&mut s);
        let logs = "Logs rotate daily.";
        let r2 = s.said("s", "github.com/o/r", 2_000, logs);
        let r3 = s.said("s", "github.com/o/r", 3_000, "Tests run on CI.");
        let home = s.home.path().to_owned();
        let copied = claim_op(&device, &[(r2, logs)], &format!("Logs keep {MASKED}."));
        let uid = raw::op_uid(copied.0, &copied.1).unwrap();
        let seqs = s
            .raw
            .append_ops(&[
                window_op(r1, r2, "Their span.", json!({})),
                copied,
                window_op(
                    r3,
                    r3,
                    &format!("It showed {MASKED}."),
                    json!({"candidates": [uid]}),
                ),
            ])
            .unwrap();
        let fields = json!({"task": format!("Rotate {MASKED}."), "status": "doing",
            "source": format!("ci {MASKED}")});
        let state = s
            .raw
            .work_state(
                "github.com/o/r",
                "deploys",
                fields.as_object().unwrap(),
                None,
            )
            .unwrap();
        s.raw
            .append_imports(vec![raw::ImportDoc {
                uid: "claude-mem:abc:o1".into(),
                source: "claude-mem:abc".into(),
                source_id: "o1".into(),
                kind: "decision".into(),
                repo: "github.com/o/r".into(),
                session: "s".into(),
                ts: 1,
                title: "An old note".into(),
                body: format!("Old note {MASKED}."),
            }])
            .unwrap();
        s.run();
        let fix = format!("Logs keep {MASKED} a week.");
        crate::claims::correct(&home, &uid, Some("done"), Some(&fix)).unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        let wal = PathBuf::from(format!("{}-wal", copy.display()));
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let before = op_bodies(&home);
        let purged = purge(&home).unwrap().unwrap();
        // The claim, the window that named it, the work state, the import and the correction.
        assert_eq!((purged.texts, purged.rebuilt), (5, true));
        assert_eq!(store_hits(&home, MASKED.as_bytes()), Vec::<&str>::new());
        assert_eq!(backup_hits(&home, MASKED), Vec::<String>::new());
        assert!(!holds_text(&copy, MASKED) && !holds_text(&wal, MASKED));
        let stars = "*".repeat(MASKED.len());
        let k = crate::knowledge::open(&home).unwrap();
        let shown: (String, String) = k
            .query_row(
                "SELECT status, body FROM active WHERE uid = ?1",
                [&uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(shown, ("done".into(), format!("Logs keep {stars} a week.")));
        let after = op_bodies(&home);
        assert_eq!(after[&seqs[1]]["body"], format!("Logs keep {stars}."));
        assert_eq!(after[&seqs[2]]["summary"], format!("It showed {stars}."));
        assert_eq!(after[&state]["fields"]["task"], format!("Rotate {stars}."));
        assert_eq!(after[&state]["fields"]["source"], format!("ci {stars}"));
        // The names of every op that keeps its text (3e-1 took the first window's).
        for (seq, body) in before
            .iter()
            .filter(|(seq, _)| after[seq].get("forgotten").is_none())
        {
            for name in [
                "uid",
                "candidates",
                "device",
                "anchor",
                "status",
                "list",
                "repo",
            ] {
                assert_eq!(after[seq].get(name), body.get(name), "{seq} {name}");
            }
        }
        let queued: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM recurate WHERE from_seq <= ?1 AND to_seq >= ?1",
                [r3],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(queued, 0);
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// Slice 3e-4: a worker that finds op text the rules now mask none of records the scan and
    /// starts no purge (a new store's first look too); one that finds some starts it. A purge scans
    /// the op text once for each version of the rules: an op written after it, its text not masked
    /// by the rules of its time, waits for the next change of the rules.
    #[test]
    fn op_text_is_scanned_once_for_each_version_of_the_rules() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let fields = |task: String| {
            serde_json::json!({ "task": task })
                .as_object()
                .unwrap()
                .clone()
        };
        let first = s
            .raw
            .work_state(
                "github.com/o/r",
                "deploys",
                &fields(format!("Rotate {MASKED}.")),
                None,
            )
            .unwrap();
        std::fs::create_dir_all(home.join("state")).unwrap();
        let look = |raw: &raw::Raw| {
            let _ = std::fs::remove_file(home.join("state").join("forget-continue"));
            CONTINUED.set(0);
            continue_due(&home, raw);
            CONTINUED.get()
        };
        assert_eq!(look(&s.raw), 0);
        let bundled = crate::redact::Rules::default();
        assert!(!s.raw.ops_text_owed(bundled.version()).unwrap());
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        assert_eq!(look(&s.raw), 1);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert_eq!(purge(&home).unwrap().unwrap().texts, 1);
        let second = crate::raw::open(&home)
            .unwrap()
            .work_state(
                "github.com/o/r",
                "deploys",
                &fields(format!("Again {MASKED}.")),
                None,
            )
            .unwrap();
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        let task = |seq: i64| op_bodies(&home)[&seq]["fields"]["task"].clone();
        assert_eq!(task(second), format!("Again {MASKED}."));
        let zeta = r#"{ id = "zeta", regex = 'zeta-[0-9]{4}' }"#;
        let acme = r#"{ id = "acme", regex = 'acme-[0-9]{6}' }"#;
        std::fs::write(
            home.join("config.toml"),
            format!("[redaction]\nextra_rules = [{acme}, {zeta}]\n"),
        )
        .unwrap();
        assert_eq!(purge(&home).unwrap().unwrap().texts, 1);
        let stars = "*".repeat(MASKED.len());
        assert_eq!(
            (task(first), task(second)),
            (
                format!("Rotate {stars}.").into(),
                format!("Again {stars}.").into()
            )
        );
    }

    /// Slice 3e-4 (Codex on #487): an op only a backup's ops segment or a copy of raw.db set aside
    /// holds (a restore that left it out of raw.db) is masked by a rule added later: a worker that
    /// finds no op in raw.db the rules mask starts the purge while either is there, rather than
    /// recording the scan, and the purge's step 4 masks it. The next look starts none.
    #[test]
    fn an_op_only_a_segment_or_a_copy_holds_is_masked_by_a_rule_added_later() {
        for aside in [false, true] {
            let mut s = crate::search::b::fixture::Store::new();
            let home = s.home.path().to_owned();
            let fields = serde_json::json!({ "task": format!("Rotate {MASKED}.") });
            let seq = s
                .raw
                .work_state(
                    "github.com/o/r",
                    "deploys",
                    fields.as_object().unwrap(),
                    None,
                )
                .unwrap();
            let crate::search::b::fixture::Store { home: _kept, raw } = s;
            drop(raw);
            let copy = aside.then(|| {
                copy_raw(
                    &home,
                    &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
                )
            });
            if !aside {
                crate::backup::export(&home).unwrap();
                assert!(!backup_hits(&home, MASKED).is_empty());
            }
            // raw.db without it, as a restore that left it out makes it.
            rusqlite::Connection::open(home.join("raw.db"))
                .unwrap()
                .execute("DELETE FROM ops WHERE op_seq = ?1", [seq])
                .unwrap();
            std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
            std::fs::create_dir_all(home.join("state")).unwrap();
            let look = || {
                let _ = std::fs::remove_file(home.join("state").join("forget-continue"));
                CONTINUED.set(0);
                continue_due(&home, &raw::open(&home).unwrap());
                CONTINUED.get()
            };
            assert_eq!(look(), 1, "aside: {aside}");
            let purged = purge(&home).unwrap().unwrap();
            assert_eq!(
                (purged.texts, purged.segments, purged.copies),
                (0, usize::from(!aside), usize::from(aside))
            );
            match copy {
                Some(copy) => assert!(!holds_text(&copy, MASKED)),
                None => assert_eq!(backup_hits(&home, MASKED), Vec::<String>::new()),
            }
            assert_eq!(look(), 0, "aside: {aside}");
        }
    }

    /// Owner decision 43 (Codex on #502): rules turned off after a purge's look at raw.db's op text
    /// and before its step 4 mask no op only the backups hold with the rules before (it may have
    /// been stored as typed while they were off); the scan stays owed to the rules now.
    #[test]
    fn rules_turned_off_before_step_4_mask_no_op_only_the_backups_hold() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let fields = serde_json::json!({ "task": format!("Rotate {MASKED}.") });
        let seq = s
            .raw
            .work_state(
                "github.com/o/r",
                "deploys",
                fields.as_object().unwrap(),
                None,
            )
            .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::backup::export(&home).unwrap();
        // raw.db without it, as a restore that left it out makes it.
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM ops WHERE op_seq = ?1", [seq])
            .unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        let config = home.join("config.toml");
        let off = MASK_RULE.replace("[redaction]\n", "[redaction]\nenabled = false\n");
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || std::fs::write(&config, off).unwrap()));
        });
        purge(&home).unwrap().unwrap();
        assert!(!backup_hits(&home, MASKED).is_empty());
        let now = crate::redact::Rules::load(&home).unwrap();
        assert!(
            raw::open(&home)
                .unwrap()
                .ops_text_owed(now.version())
                .unwrap()
        );
    }

    /// Owner decision 43 (Codex on #502): rules turned off while a purge waits for the worker
    /// lock mask no op text with the rules before; the scan stays owed to the rules now.
    #[test]
    fn rules_turned_off_while_a_purge_waits_mask_no_op_text() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        let fields = serde_json::json!({ "task": format!("Rotate {MASKED}.") });
        let seq = s
            .raw
            .work_state(
                "github.com/o/r",
                "deploys",
                fields.as_object().unwrap(),
                None,
            )
            .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let config = home.join("config.toml");
        let off = MASK_RULE.replace("[redaction]\n", "[redaction]\nenabled = false\n");
        AFTER_PURGE_LOCK.set(Some(Box::new(move || {
            std::fs::write(&config, off).unwrap();
        })));
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.texts, 0);
        let task = op_bodies(&home)[&seq]["fields"]["task"].clone();
        assert_eq!(task, format!("Rotate {MASKED}."));
    }

    /// Owner decision 43 (Codex's security review of the switch): an op appended under rules
    /// turned off between a worker's look at the op text and its record of the scan leaves the
    /// scan unrecorded, so the next look finds the op and starts the purge.
    #[test]
    fn an_op_appended_off_during_the_look_leaves_the_scan_unrecorded() {
        let s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        std::fs::create_dir_all(home.join("state")).unwrap();
        let on = crate::redact::Rules::load(&home).unwrap();
        let off_toml = MASK_RULE.replace("[redaction]\n", "[redaction]\nenabled = false\n");
        let off = crate::redact::Rules::new(
            &crate::config::parse_capture(Some(&off_toml))
                .unwrap()
                .redaction,
        )
        .unwrap();
        let look = || {
            let _ = std::fs::remove_file(home.join("state").join("forget-continue"));
            CONTINUED.set(0);
            continue_due(&home, &raw::open(&home).unwrap());
            CONTINUED.get()
        };
        let writer = home.clone();
        AFTER_TEXT_LOOK.set(Some(Box::new(move || {
            let mut raw = raw::open(&writer).unwrap();
            raw.gates_with(&off);
            let fields = serde_json::json!({ "task": format!("Rotate {MASKED}.") });
            raw.append_ops(&[(
                raw::OpKind::WorkState,
                serde_json::json!({"repo": "github.com/o/r", "list": "deploys",
                    "fields": fields, "clock": 1}),
            )])
            .unwrap();
        })));
        assert_eq!(look(), 0);
        assert!(
            raw::open(&home)
                .unwrap()
                .ops_text_owed(on.version())
                .unwrap()
        );
        assert_eq!(look(), 1);
    }

    /// Owner decision 43 (Codex's security review of the switch): an agent's work state written
    /// while redaction is off, and the same rules back on before any worker read them off, still
    /// owes the scan of the op text, though the version is the one the last scan recorded: the
    /// worker starts the purge that masks it.
    #[test]
    fn an_op_written_while_off_owes_the_scan_when_the_same_rules_come_back() {
        let s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        std::fs::create_dir_all(home.join("state")).unwrap();
        let look = || {
            let _ = std::fs::remove_file(home.join("state").join("forget-continue"));
            CONTINUED.set(0);
            continue_due(&home, &raw::open(&home).unwrap());
            CONTINUED.get()
        };
        assert_eq!(look(), 0);
        let on = crate::capture::Settings::load(&home).unwrap();
        let mut raw = raw::open(&home).unwrap();
        assert!(!raw.ops_text_owed(on.rules.version()).unwrap());
        let off_rules = MASK_RULE.replace("[redaction]\n", "[redaction]\nenabled = false\n");
        std::fs::write(home.join("config.toml"), off_rules).unwrap();
        let off = crate::capture::Settings::load(&home).unwrap();
        let fields = serde_json::json!({ "task": format!("Rotate {MASKED}.") });
        raw.work_state_under(
            "github.com/o/r",
            "deploys",
            fields.as_object().unwrap(),
            "/x",
            &off,
            ("github.com/o/r", Some("/x")),
        )
        .unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        assert!(raw.ops_text_owed(on.rules.version()).unwrap());
        assert_eq!(look(), 1);
    }

    /// Owner decision 43: rules turned off find nothing in a backup's ops segment either, so a
    /// worker records the scan under them and starts no purge for it; turned on again with a rule
    /// that masks an op, the purge starts.
    #[test]
    fn rules_turned_off_start_no_purge_for_the_op_text() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let fields = serde_json::json!({ "task": format!("Rotate {MASKED}.") });
        s.raw
            .work_state(
                "github.com/o/r",
                "deploys",
                fields.as_object().unwrap(),
                None,
            )
            .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::backup::export(&home).unwrap();
        std::fs::write(home.join("config.toml"), "[redaction]\nenabled = false\n").unwrap();
        std::fs::create_dir_all(home.join("state")).unwrap();
        let look = || {
            let _ = std::fs::remove_file(home.join("state").join("forget-continue"));
            CONTINUED.set(0);
            continue_due(&home, &raw::open(&home).unwrap());
            CONTINUED.get()
        };
        assert_eq!(look(), 0);
        let off = crate::redact::Rules::load(&home).unwrap();
        assert!(
            !raw::open(&home)
                .unwrap()
                .ops_text_owed(off.version())
                .unwrap()
        );
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        assert_eq!(look(), 1);
    }

    /// Slice 3e-4 (Codex on #487): an op whose uid cannot be told, a claim of the shape from before
    /// Task 7, has its text masked in a backup's ops segment as in raw.db.
    #[test]
    fn an_op_whose_uid_cannot_be_told_has_its_text_masked_in_the_backup() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        let old = serde_json::json!({ "text": format!("Logs keep {MASKED}.") });
        let seq = s.raw.append_ops(&[(raw::OpKind::Claim, old)]).unwrap()[0];
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::backup::export(&home).unwrap();
        assert!(!backup_hits(&home, MASKED).is_empty());
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.texts, purged.segments), (1, 1));
        assert_eq!(backup_hits(&home, MASKED), Vec::<String>::new());
        let stars = "*".repeat(MASKED.len());
        assert_eq!(
            op_bodies(&home)[&seq]["text"],
            format!("Logs keep {stars}.")
        );
    }

    /// Slice 3e-4: a purge that stops after masking op text, before knowledge.db is rebuilt, leaves
    /// it owed: the next one rebuilds knowledge.db, though it masks nothing more, even once the
    /// rule is taken back (the rules are again those the op text was last scanned with).
    #[test]
    fn a_purge_stopped_after_masking_op_text_finishes_it_later() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let logs = "Logs rotate daily.";
        let r1 = s.said("s", "github.com/o/r", 1_000, logs);
        let claim = claim_op(&device, &[(r1, logs)], &format!("Logs keep {MASKED}."));
        s.raw
            .append_ops(&[window_op(r1, r1, "Its span.", json!({})), claim])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        assert!(store_hits(&home, MASKED.as_bytes()).contains(&"knowledge.db"));
        crate::backup::FAIL_AFTER.set(Some("op_text_masked"));
        assert!(purge(&home).is_err());
        assert!(store_hits(&home, MASKED.as_bytes()).contains(&"knowledge.db"));
        assert!(raw::open(&home).unwrap().ops_text_pending().unwrap());
        std::fs::remove_file(home.join("config.toml")).unwrap();
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.texts, purged.rebuilt), (0, true));
        assert_eq!(store_hits(&home, MASKED.as_bytes()), Vec::<&str>::new());
        assert!(!raw::open(&home).unwrap().ops_text_pending().unwrap());
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// Slice 3e-4 (Codex on #487): a rule that matches more ops than a page of the purge holds
    /// masks every one of them, a page at a time, in raw.db and in a copy of it set aside.
    #[test]
    fn a_rule_matching_many_ops_masks_them_a_page_at_a_time() {
        let mut s = crate::search::b::fixture::Store::new();
        s.said("s", "github.com/o/r", 1_000, "Logs rotate daily.");
        let many = crate::raw::RECORD_PURGE_BATCH * 2 + 1;
        let ops: Vec<_> = (0..many)
            .map(|i| {
                let fields = serde_json::json!({ "task": format!("Rotate {MASKED} {i}.") });
                let op = serde_json::json!({"repo": "github.com/o/r", "list": "deploys",
                    "fields": fields, "clock": i});
                (crate::raw::OpKind::WorkState, op)
            })
            .collect();
        for batch in crate::raw::append_batches(&ops) {
            s.raw.append_ops(batch).unwrap();
        }
        s.run();
        let home = s.home.path().to_owned();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        assert_eq!(purge(&home).unwrap().unwrap().texts, many);
        assert_eq!(store_hits(&home, MASKED.as_bytes()), Vec::<&str>::new());
        let masked = copy_ops(&copy)
            .into_iter()
            .filter(|(_, _, body)| body.contains("Rotate *"))
            .count();
        assert_eq!(masked, many);
        assert!(!holds_text(&copy, MASKED));
    }

    /// Slice 3e-4: while a reader holds raw.db-wal, which may still hold the ops' old pages, or when
    /// a restore during the purge makes raw.db anew from segments that held the op text unmasked,
    /// the op text stays owed, and the next purge finishes it.
    #[test]
    fn a_reader_or_a_restore_during_the_purge_leaves_the_op_text_owed() {
        let mut s = crate::search::b::fixture::Store::new();
        let home = s.home.path().to_owned();
        // A record, so the backup has segments a restore reads.
        s.said("s", "github.com/o/r", 1_000, "Logs rotate daily.");
        let fields = serde_json::json!({ "task": format!("Rotate {MASKED}.") });
        let state = s
            .raw
            .work_state(
                "github.com/o/r",
                "deploys",
                fields.as_object().unwrap(),
                None,
            )
            .unwrap();
        s.run();
        crate::backup::export(&home).unwrap();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let version = crate::capture::Settings::load(&home).unwrap().rules;
        let owed = || {
            raw::open(&home)
                .unwrap()
                .ops_text_owed(version.version())
                .unwrap()
        };
        let task = || op_bodies(&home)[&state]["fields"]["task"].clone();
        let reader = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM ops", [], |r| r.get(0))
            .unwrap();
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!(purged.texts, 1);
        assert!(purged.unfinished.iter().any(|j| j == crate::raw::MASK_JOB));
        assert!(owed());
        reader.execute_batch("COMMIT").unwrap();
        drop(reader);
        purge(&home).unwrap().unwrap();
        assert!(!owed());
        // A rule that finds it again (a wider one), and a restore before step 3 from segments the
        // first purge has not rewritten since.
        crate::backup::export(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"rotate\", regex = 'Rotate' }]\n",
        )
        .unwrap();
        let version = crate::capture::Settings::load(&home).unwrap().rules;
        let owed = || {
            raw::open(&home)
                .unwrap()
                .ops_text_owed(version.version())
                .unwrap()
        };
        let at = home.clone();
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(at.join("raw.db"), b"not a database at all").unwrap();
                for f in ["raw.db-wal", "raw.db-shm"] {
                    let _ = std::fs::remove_file(at.join(f));
                }
                crate::worker::run_once(&at).unwrap();
            }));
        });
        let purged = purge(&home).unwrap().unwrap();
        assert!(purged.unfinished.iter().any(|j| j == crate::raw::MASK_JOB));
        let stars = "*".repeat(MASKED.len());
        assert_eq!(task(), format!("Rotate {stars}."));
        assert!(owed());
        purge(&home).unwrap().unwrap();
        assert_eq!(task(), format!("****** {stars}."));
        assert!(!owed());
    }

    /// Slice 3e-4: a claim's quote keeps what its record holds, masked or not: a rule anchored at
    /// a field's start finds the secret at the quote's start, though not in the middle of the
    /// record's field, which keeps it; the claim still reads in its record, and is current.
    #[test]
    fn a_quote_keeps_what_its_record_holds() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let said = format!("Deploy with {MASKED} on Fridays.");
        let r1 = s.said("s", "github.com/o/r", 1_000, &said);
        let quote = format!("{MASKED} on Fridays.");
        let mut claim = claim_op(&device, &[(r1, &quote)], "Deploys run on Fridays.");
        claim.1["evidence"][0]["offset"] = said.find(MASKED).unwrap().into();
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        let seqs = s
            .raw
            .append_ops(&[window_op(r1, r1, "Its span.", json!({})), claim])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        std::fs::write(
            home.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"start\", regex = '^acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        assert_eq!(op_bodies(&home)[&seqs[1]]["evidence"][0]["quote"], quote);
        let k = crate::knowledge::open(&home).unwrap();
        let current: i64 = k
            .query_row("SELECT COUNT(*) FROM active WHERE uid = ?1", [&uid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(current, 1);
    }

    /// Slice 3e-4: a rule for 64 hex digits, as a user may add for a secret of that form, masks
    /// them in an op's text but not in the uids ops name (a window's candidates, a claim's
    /// supersedes, a correction's uid) or in an import's checkpoint.
    #[test]
    fn a_rule_for_hex_digits_leaves_the_uids_ops_name() {
        use serde_json::json;
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (logs, tests) = ("Logs rotate daily.", "Tests run on CI.");
        let r1 = s.said("s", "github.com/o/r", 1_000, logs);
        let r2 = s.said("s", "github.com/o/r", 2_000, tests);
        let claim = claim_op(&device, &[(r1, logs)], "Logs rotate.");
        let uid = raw::op_uid(claim.0, &claim.1).unwrap();
        let mut later = claim_op(&device, &[(r2, tests)], "Tests run.");
        later.1["supersedes"] = json!([uid]);
        let hex = "ab".repeat(32);
        s.raw
            .append_ops(&[
                window_op(r1, r2, "Its span.", json!({"candidates": [uid]})),
                claim,
                later,
            ])
            .unwrap();
        // An import's checkpoint, which names v1's session by its SHA-256.
        let row = json!({"id": 1, "ts": 1, "session_id": "cd".repeat(32)});
        s.raw
            .append_ops(&[(
                raw::OpKind::Migration,
                json!({"key": "v1", "through": 1, "row": row}),
            )])
            .unwrap();
        let fields = json!({ "task": format!("Key {hex}.") });
        let state = s
            .raw
            .work_state("github.com/o/r", "keys", fields.as_object().unwrap(), None)
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        crate::claims::correct(&home, &uid, Some("done"), None).unwrap();
        // A copy of raw.db set aside: with no record masked, only the op text takes it.
        let copy = copy_raw(
            &home,
            &crate::backup::fresh_name(&home, "raw.db.quarantined-").unwrap(),
        );
        std::fs::write(
            home.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"hex\", regex = '[0-9a-f]{64}' }]\n",
        )
        .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let before = op_bodies(&home);
        let wal = PathBuf::from(format!("{}-wal", copy.display()));
        assert!(holds_text(&copy, &hex) || holds_text(&wal, &hex));
        assert_eq!(purge(&home).unwrap().unwrap().texts, 1);
        let mut after = op_bodies(&home);
        assert_eq!(
            after[&state]["fields"]["task"],
            format!("Key {}.", "*".repeat(64))
        );
        assert!(!holds_text(&copy, &hex) && !holds_text(&wal, &hex));
        after.insert(state, before[&state].clone());
        assert_eq!(after, before);
    }

    /// CodeRabbit on #480: masks a purge left owed are said as a rescan's, not as a job named
    /// `rescan`, which `--status` does not list.
    #[test]
    fn masks_left_owed_are_not_said_as_a_job() {
        let masks = Purged {
            unfinished: vec![crate::raw::MASK_JOB.to_owned()],
            ..Purged::default()
        };
        let said = say_purged(&masks).unwrap_err().to_string();
        assert!(
            said.contains("rescan's masks") && !said.contains("jobs"),
            "{said}"
        );
        let both = Purged {
            unfinished: vec!["j1".to_owned(), crate::raw::MASK_JOB.to_owned()],
            ..Purged::default()
        };
        let said = say_purged(&both).unwrap_err().to_string();
        assert!(said.starts_with("jobs j1 are not done"), "{said}");
    }

    /// Slice 3e-1: a mask a rescan writes while a purge runs is not taken for done: it is owed
    /// until a purge that read it finishes.
    #[test]
    fn a_mask_written_during_a_purge_stays_owed() {
        let mut s = crate::search::b::fixture::Store::new();
        let device = s.raw.device().to_owned();
        let (r1, _) = masked_prompt(&mut s);
        let home = s.home.path().to_owned();
        s.run();
        std::fs::write(home.join("config.toml"), MASK_RULE).unwrap();
        s.run();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let at = home.clone();
        BEFORE_STEP_3.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                raw::open(&at)
                    .unwrap()
                    .append_tombstone(raw::Target::Range {
                        device,
                        seq: r1,
                        offset: 2,
                        length: 4,
                    })
                    .unwrap();
            }));
        });
        purge(&home).unwrap().unwrap();
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_some());
        assert_eq!(purge(&home).unwrap().unwrap().masked, 1);
        assert!(raw::open(&home).unwrap().masks_owed().unwrap().is_none());
    }

    /// Slice 3e-1: a range a rescan masks while a curator's or a turn's call is out cuts its window
    /// again before anything is written (rule 12's fence), as a forget's denial does.
    #[test]
    fn a_mask_during_a_call_fences_its_write() {
        let mut s = crate::search::b::fixture::Store::new();
        let (r1, _) = masked_prompt(&mut s);
        let reading = crate::curate::Reading::now(&s.raw, Default::default()).unwrap();
        reading.still(&s.raw).unwrap();
        let device = s.raw.device().to_owned();
        s.raw
            .append_tombstone(raw::Target::Range {
                device,
                seq: r1,
                offset: 0,
                length: 2,
            })
            .unwrap();
        let changed = reading.still(&s.raw).unwrap_err();
        assert!(changed.is::<crate::curate::ListChanged>(), "{changed:#}");
        let op = serde_json::json!({"outcome": "skipped", "from_seq": r1, "to_seq": r1});
        let fenced = s
            .raw
            .append_ops_fenced(&[(raw::OpKind::Window, op)], reading.fence)
            .unwrap_err();
        assert!(fenced.is::<crate::curate::ListChanged>(), "{fenced:#}");
    }

    /// "A line names its home": a request of another home in a log is reported, not applied.
    #[test]
    fn a_line_of_another_home_is_not_applied() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let a = native(&mut raw, "a", r#"{"prompt":"one"}"#);
        let mut p = raw.forget_preview(record(&raw, a)).unwrap();
        p.validate().unwrap();
        let foreign = Request {
            v: 1,
            home: "elsewhere".into(),
            job: "0".repeat(32),
            started: 1,
            target: p.target.clone(),
            records: std::mem::take(&mut p.records),
        };
        append_log(&home.path().join(LOG), &[&foreign]).unwrap();
        let report = reconcile(home.path(), &mut raw).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        assert!(shown(&raw, a));
    }
