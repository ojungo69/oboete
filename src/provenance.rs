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
    /// That span's end alone, which a restore's cut reads with or without its start (#690).
    pub(crate) end: Option<i64>,
    /// The other spans of its device's records it read: a turn's (whose cards it read too), a My
    /// Work op's, and a window's in a conversation, the stretches its curator was sent again before
    /// its own (decision 107, docs/curator-conversation.md).
    pub(crate) read: Vec<(i64, i64)>,
    /// A turn's: those of `read` that are stretches only, the span of no window whose card it
    /// read (decision 107).
    pub(crate) stretches: Vec<(i64, i64)>,
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

    /// The highest seq of `device`'s records it names, `device` being its own: its span's end, the
    /// spans it read, its goals and its quotes of that device. A restore's cut reads it (F1).
    pub(crate) fn top(&self, device: &str) -> Option<i64> {
        (self.end.into_iter())
            .chain(self.read.iter().map(|r| r.1))
            .chain(self.goals.iter().copied())
            .chain((self.quotes.iter()).filter(|q| q.0 == device).map(|q| q.1))
            .max()
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
            end: body["to_seq"].as_i64(),
            read: pairs(&body["read"]),
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
            end: body["through"].as_i64(),
            read: pairs(&body["read"]),
            stretches: pairs(&body["stretches"]),
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
            end: body["to_seq"].as_i64(),
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

pub(crate) fn pairs(v: &Value) -> Vec<(i64, i64)> {
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
mod tests;
