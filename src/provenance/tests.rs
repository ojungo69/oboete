    use super::*;
    use serde_json::json;

    #[test]
    fn a_window_reads_its_span_goals_uids_and_removals() {
        let body = json!({"outcome": "curated", "from_seq": 3, "to_seq": 9, "goals": [1, "x", 2],
            "candidates": ["u1", 7], "carried": ["u2"], "removed": [[4, null]]});
        let p = of(OpKind::Window, &body);
        assert_eq!(
            (p.holds, p.own, &p.goals[..], &p.uids[..]),
            (true, Some((3, 9)), &[1, 2][..], &["u1", "u2"][..])
        );
        assert_eq!(
            (p.removed.len(), p.forgotten, p.taken_by()),
            (1, None, None)
        );
        // Decision 107: none read before its own until an entry was sent a stretch again.
        assert_eq!(p.read, []);
        let skipped = json!({"outcome": "skipped", "from_seq": 3});
        let p = of(OpKind::Window, &skipped);
        assert_eq!((p.holds, p.own), (false, None));
        let gone = json!({"from_seq": 3, "to_seq": 9, "forgotten": "j1"});
        assert_eq!(of(OpKind::Window, &gone).taken_by(), Some("j1"));
        // A `forgotten` that is no job is still there, as the judge reads it.
        let odd = json!({"forgotten": null});
        let p = of(OpKind::Window, &odd);
        assert_eq!((p.forgotten.is_some(), p.taken_by()), (true, None));
    }

    /// Decision 107 (docs/curator-conversation.md): a window in a conversation lists `read`, the
    /// spans of the stretches its curator was sent again before its own, as a turn lists the spans
    /// it read; an item that is no pair of seqs is none.
    #[test]
    fn a_window_reads_the_stretches_sent_before_its_own() {
        let body = json!({"outcome": "curated", "from_seq": 9, "to_seq": 12,
            "read": [[1, 4], [5], "x", [5, 8]]});
        let p = of(OpKind::Window, &body);
        assert_eq!((p.own, &p.read[..]), (Some((9, 12)), &[(1, 4), (5, 8)][..]));
    }

    #[test]
    fn a_turn_reads_its_own_records_and_the_spans_it_read() {
        let body = json!({"from": 5, "through": 8, "read": [[1, 2], [3], [4, 4]], "goals": [2],
            "removed": []});
        let p = of(OpKind::Turn, &body);
        assert_eq!(
            (p.holds, p.own, &p.read[..], &p.goals[..]),
            (true, Some((5, 8)), &[(1, 2), (4, 4)][..], &[2][..])
        );
        assert_eq!(p.stretches, []);
        // Decision 107: those of the spans it read that are stretches only; a window's are none.
        let body = json!({"from": 5, "through": 8, "read": [[1, 2], [4, 4]],
            "stretches": [[1, 2], [3], "x"]});
        assert_eq!(of(OpKind::Turn, &body).stretches, [(1, 2)]);
        let window = json!({"from_seq": 5, "to_seq": 8, "read": [[1, 2]], "stretches": [[1, 2]]});
        assert_eq!(of(OpKind::Window, &window).stretches, []);
    }

    #[test]
    fn a_my_work_op_reads_its_spans_and_the_ops_it_read() {
        let body = json!({"read": {"spans": [[1, 4], [6, 6]], "ops": [10, 12]},
            "removed": [[2, null]]});
        let p = of(OpKind::MyWork, &body);
        assert_eq!(
            (p.holds, p.own, &p.read[..], &p.ops[..], p.removed.len()),
            (true, None, &[(1, 4), (6, 6)][..], &[10, 12][..], 1)
        );
    }

    /// F1, #690: a restore's cut reads the highest record an op names on its own device: a span's
    /// end even without its start, the spans it read, its goals, and its quotes of that device.
    #[test]
    fn the_top_is_the_highest_record_an_op_names_on_its_device() {
        let window = json!({"to_seq": 9, "goals": [12]});
        assert_eq!(of(OpKind::Window, &window).end, Some(9));
        assert_eq!(of(OpKind::Window, &window).top("d"), Some(12));
        let turn = json!({"from": 5, "through": 8, "read": [[1, 20]]});
        assert_eq!(of(OpKind::Turn, &turn).top("d"), Some(20));
        let read = json!({"from_seq": 5, "to_seq": 8, "read": [[1, 20]]});
        assert_eq!(of(OpKind::Window, &read).top("d"), Some(20));
        let claim = json!({"evidence": [{"device": "d", "seq": 3}, {"device": "e", "seq": 50}]});
        assert_eq!(of(OpKind::Claim, &claim).top("d"), Some(3));
        assert_eq!(of(OpKind::Claim, &claim).top("f"), None);
        assert_eq!(of(OpKind::Forget, &json!({"uid": "u"})).top("d"), None);
    }

    #[test]
    fn a_migration_a_claim_and_a_correction_read_what_they_name() {
        let migration = json!({"from_seq": 1, "to_seq": 40});
        let p = of(OpKind::Migration, &migration);
        assert_eq!((p.own, p.holds), (Some((1, 40)), false));
        let claim = json!({"evidence": [{"device": "d", "seq": 3}, {"seq": 4},
            {"device": "e", "seq": 5}]});
        assert_eq!(of(OpKind::Claim, &claim).quotes, vec![("d", 3), ("e", 5)]);
        let correction = json!({"anchor": {"device": "d", "seq": 7}});
        assert_eq!(of(OpKind::Correction, &correction).quotes, vec![("d", 7)]);
        assert_eq!(
            of(OpKind::Forget, &json!({"uid": "u"})),
            Provenance::default()
        );
    }
