//! docs/provenance.md P1: what an op was made from, read in one place for every kind, as each
//! kind stores it. Forget's judge, the backup's purge and count, a copy's rewrite and a restore's
//! cut read it here, so a kind added later adds its case here.

use crate::raw::OpKind;
use serde_json::Value;

/// What one op was made from.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Provenance<'a> {
    /// Its `forgotten` as stored: a forget took its text, with the job that took it.
    pub(crate) forgotten: Option<&'a Value>,
    /// Whether it holds text made from what it read: a window only when curated.
    pub(crate) holds: bool,
    /// Its own span of its device's records, by seq: a window's, a turn's, a migration's.
    pub(crate) own: Option<(i64, i64)>,
    /// The other spans of its device's records it read: a turn's (whose cards it read too), a My
    /// Work op's.
    pub(crate) read: Vec<(i64, i64)>,
    /// Its device's records carried in as goals, by seq.
    pub(crate) goals: Vec<i64>,
    /// Its device's ops it read, by op seq.
    pub(crate) ops: Vec<i64>,
    /// The claims it was shown, by uid: a window's candidates, then those carried.
    pub(crate) uids: Vec<&'a str>,
    /// The records it quotes, by device and seq: a claim's evidence, a correction's anchor.
    pub(crate) quotes: Vec<(&'a str, i64)>,
    /// What was taken out of what it was shown, as stored.
    pub(crate) removed: &'a [Value],
}

impl Provenance<'_> {
    /// The job of the forget that took its text, when one did.
    pub(crate) fn taken_by(&self) -> Option<&str> {
        self.forgotten.and_then(Value::as_str)
    }
}

/// The kinds whose body holds text made from what they read: a forget judges each by it, and
/// rewrites it without that text.
pub(crate) const DERIVED: [OpKind; 3] = [OpKind::Window, OpKind::Turn, OpKind::MyWork];

/// What an op of `kind` whose body is `body` was made from.
pub(crate) fn of(kind: OpKind, body: &Value) -> Provenance<'_> {
    let forgotten = body.get("forgotten");
    let removed = items(&body["removed"]);
    match kind {
        OpKind::Window => Provenance {
            forgotten,
            holds: body["outcome"] == "curated",
            own: span(&body["from_seq"], &body["to_seq"]),
            goals: seqs(&body["goals"]),
            uids: (items(&body["candidates"]).iter())
                .chain(items(&body["carried"]))
                .filter_map(Value::as_str)
                .collect(),
            removed,
            ..Provenance::default()
        },
        OpKind::Turn => Provenance {
            forgotten,
            holds: true,
            own: span(&body["from"], &body["through"]),
            read: pairs(&body["read"]),
            goals: seqs(&body["goals"]),
            removed,
            ..Provenance::default()
        },
        OpKind::MyWork => Provenance {
            forgotten,
            holds: true,
            read: pairs(&body["read"]["spans"]),
            ops: seqs(&body["read"]["ops"]),
            removed,
            ..Provenance::default()
        },
        OpKind::Migration => Provenance {
            own: span(&body["from_seq"], &body["to_seq"]),
            ..Provenance::default()
        },
        OpKind::Claim => Provenance {
            quotes: items(&body["evidence"]).iter().filter_map(quote).collect(),
            ..Provenance::default()
        },
        OpKind::Correction => Provenance {
            quotes: quote(&body["anchor"]).into_iter().collect(),
            ..Provenance::default()
        },
        _ => Provenance::default(),
    }
}

/// The items of a list field; none when it is not a list.
fn items(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn span(from: &Value, to: &Value) -> Option<(i64, i64)> {
    Some((from.as_i64()?, to.as_i64()?))
}

fn pairs(v: &Value) -> Vec<(i64, i64)> {
    (items(v).iter())
        .filter_map(|r| Some((r[0].as_i64()?, r[1].as_i64()?)))
        .collect()
}

fn seqs(v: &Value) -> Vec<i64> {
    items(v).iter().filter_map(Value::as_i64).collect()
}

fn quote(e: &Value) -> Option<(&str, i64)> {
    Some((e["device"].as_str()?, e["seq"].as_i64()?))
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_turn_reads_its_own_records_and_the_spans_it_read() {
        let body = json!({"from": 5, "through": 8, "read": [[1, 2], [3], [4, 4]], "goals": [2],
            "removed": []});
        let p = of(OpKind::Turn, &body);
        assert_eq!(
            (p.holds, p.own, &p.read[..], &p.goals[..]),
            (true, Some((5, 8)), &[(1, 2), (4, 4)][..], &[2][..])
        );
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
}
