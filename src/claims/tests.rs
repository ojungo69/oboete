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
