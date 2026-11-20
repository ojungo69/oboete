use super::*;
use serde_json::json;

#[test]
fn old_corrections_keep_their_shape_and_mute_alone_is_a_correction() {
    let old = json!({
        "uid": "a".repeat(64), "anchor": {"device": "d", "seq": 1},
        "status": "decided", "body": null
    });
    let op: CorrectionOp = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(op.fault(), None);
    assert_eq!(serde_json::to_value(op).unwrap(), old);
    for muted in [true, false] {
        let value = json!({
            "uid": "a".repeat(64), "anchor": {"device": "d", "seq": 1},
            "status": null, "body": null, "muted": muted
        });
        let op: CorrectionOp = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(op.fault(), None);
        assert_eq!(serde_json::to_value(op).unwrap(), value);
    }
}

/// #1083 (R2a): `decided_by` names, for each of status, body and mute, the correction the `active`
/// view takes, with what the view reads: of one device's corrections the later in its op log
/// whatever its clock says, between devices the newer time; and the derivation's own where no
/// correction sets a field.
#[test]
fn decided_by_names_the_corrections_the_active_view_takes() {
    let k = rusqlite::Connection::open_in_memory().unwrap();
    schema(&k).unwrap();
    let uid = "c".repeat(64);
    k.execute(
        "INSERT INTO derivations(op_device, op_seq, uid, ts, tier, recipe, kind, status, speaker,
           scope, repo, body, valid_from, anchor_device, anchor_seq)
         VALUES('d1', 1, ?1, 0, 0, 'r', 'preference', 'decided', 'user', 'global', NULL,
           'The first body.', 0, 'd1', 1)",
        [&uid],
    )
    .unwrap();
    k.execute(
        "INSERT INTO claims(uid, op_device, op_seq) VALUES(?1, 'd1', 1)",
        [&uid],
    )
    .unwrap();
    let corrections = [
        // d1: its later op wins on its own device though its clock is older.
        (
            "d1",
            2,
            300,
            None::<&str>,
            Some("Body of d1, op 2."),
            Some(1),
        ),
        ("d1", 3, 100, None, Some("Body of d1, op 3."), Some(0)),
        // d2: newer than d1's op 3 by its clock, so its mute stands between the devices.
        ("d2", 1, 200, None, None, Some(1)),
        // A status d1 decides, and the same status d2 decides later by its clock: d2's decides it.
        ("d1", 4, 240, Some("decided"), None, None),
        ("d2", 2, 250, Some("decided"), None, None),
    ];
    for (device, seq, ts, status, body, muted) in corrections {
        k.execute(
            "INSERT INTO corrections(op_device, op_seq, ts, uid, status, body, muted)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![device, seq, ts, uid, status, body, muted],
        )
        .unwrap();
    }
    let by = decided_by(&k, &uid).unwrap().unwrap();
    let (status, body, muted): (String, String, i64) = k
        .query_row(
            "SELECT status, body, muted FROM active WHERE uid = ?1",
            [&uid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(by["derivation"], serde_json::json!(["d1", 1]));
    assert_eq!(
        (by["status"].as_str(), status.as_str()),
        (Some("decided"), "decided")
    );
    assert_eq!(
        (by["body"].as_str(), body.as_str()),
        (Some("Body of d1, op 3."), "Body of d1, op 3.")
    );
    assert_eq!((by["muted"].as_bool(), muted), (Some(true), 1));
    assert_eq!(
        by["by"],
        serde_json::json!([["d2", 2], ["d1", 3], ["d2", 1]]),
        "d2's newer status of the same value, d1's later body, d2's newer mute"
    );
    assert_eq!(decided_by(&k, &"d".repeat(64)).unwrap(), None);
}
