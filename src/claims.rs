//! Claims (spec 3.2, milestone 3 Task 6): what curation keeps from a window, one claim op each,
//! and the current claims derived from those ops in knowledge.db.

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Where a claim's quote is: its event (`device`, `seq`), its byte range in that event's long
/// text (`curate::long_text`), the start of the sentence it starts in, and the quote itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub device: String,
    pub seq: i64,
    pub offset: i64,
    pub length: i64,
    pub sentence: i64,
    pub quote: String,
}

/// The body of a claim op, which the curation phase writes (Task 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimOp {
    /// Local to its window: a sibling's `supersedes` names it (MUST-M2).
    pub id: String,
    pub kind: String,
    pub status: String,
    pub speaker: String,
    pub scope: String,
    pub body: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// Sibling ids, or the uids of candidates it was shown.
    #[serde(default)]
    pub supersedes: Vec<String>,
    /// How it was derived (the provider and model), kept with it so a rebuild picks the same
    /// derivation (MUST-M18).
    pub recipe: String,
    /// 0 code only, 1 free or local, 2 subscription, 3 paid (spec 1.4): the highest is active.
    pub tier: i64,
}

/// Spec 3.2's kinds.
pub const KINDS: [&str; 7] = [
    "decision",
    "preference",
    "lesson",
    "fix",
    "open item",
    "repo fact",
    "change",
];

/// Spec 3.2's statuses, and `unverified` for a kind or status it does not know (D13).
const STATUSES: [&str; 4] = ["decided", "proposed", "retracted", "done"];

/// `(kind, status)` as stored: the old kinds mapped (feature to change, discovery to repo fact)
/// and anything else a repo fact with status unverified (spec 3.2, D13).
pub fn normalize(kind: &str, status: &str) -> (&'static str, &'static str) {
    let kind = match kind {
        "feature" => Some("change"),
        "discovery" => Some("repo fact"),
        k => KINDS.iter().copied().find(|&known| known == k),
    };
    let status = STATUSES.iter().copied().find(|&s| s == status);
    match (kind, status) {
        (Some(k), Some(s)) => (k, s),
        (Some(k), None) => (k, "unverified"),
        (None, _) => ("repo fact", "unverified"),
    }
}

/// MUST-M18: a claim's uid is its kind and the sentence its first quote starts in, never a name
/// the model gives, so a recuration that rewords it derives the same uid.
pub fn uid(kind: &str, first: &Evidence) -> String {
    crate::curate::sha256_hex(&format!(
        "{kind}\0{}\0{}\0{}",
        first.device, first.seq, first.sentence
    ))
}

pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "-- Every derivation of a claim: one claim op each (op_device, op_seq).
         CREATE TABLE IF NOT EXISTS derivations(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, uid TEXT NOT NULL,
           ts INTEGER NOT NULL, tier INTEGER NOT NULL, recipe TEXT NOT NULL,
           kind TEXT NOT NULL, status TEXT NOT NULL, speaker TEXT NOT NULL, scope TEXT NOT NULL,
           repo TEXT, body TEXT NOT NULL, valid_from INTEGER NOT NULL,
           anchor_device TEXT NOT NULL, anchor_seq INTEGER NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS derivations_uid ON derivations(uid);
         -- Each derivation's quotes, where they are in raw.
         CREATE TABLE IF NOT EXISTS evidence(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, idx INTEGER NOT NULL,
           device TEXT NOT NULL, seq INTEGER NOT NULL, offset INTEGER NOT NULL,
           length INTEGER NOT NULL, sentence INTEGER NOT NULL, quote TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq, idx)
         );
         CREATE INDEX IF NOT EXISTS evidence_anchor ON evidence(device, seq);
         -- What a derivation supersedes: it counts while that derivation is active.
         CREATE TABLE IF NOT EXISTS edges(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, to_uid TEXT NOT NULL,
           type TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq, to_uid)
         );
         CREATE INDEX IF NOT EXISTS edges_to ON edges(to_uid);
         -- Each uid's active derivation: the highest tier, then the newest (MUST-M18).
         CREATE TABLE IF NOT EXISTS claims(
           rowid INTEGER PRIMARY KEY, uid TEXT NOT NULL UNIQUE,
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL
         );
         -- The active derivations' bodies and quotes, for Task 7's candidates.
         CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(text, tokenize='trigram');
         -- Windows whose claims lost a quote to a mask or a removal since: Task 11 sends them
         -- again.
         CREATE TABLE IF NOT EXISTS recurate(
           device TEXT NOT NULL, from_seq INTEGER NOT NULL, to_seq INTEGER NOT NULL,
           PRIMARY KEY (device, from_seq, to_seq)
         );
         -- Claim ops that gave no claim, and why: doctor counts them.
         CREATE TABLE IF NOT EXISTS claim_skips(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, reason TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );",
    )?;
    Ok(())
}

/// A current claim.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub uid: String,
    pub kind: String,
    pub status: String,
    pub speaker: String,
    pub scope: String,
    pub body: String,
    pub valid_from: i64,
    /// Its first quote's event.
    pub device: String,
    pub seq: i64,
}

/// `repo`'s current claims: the chain tips (no active derivation supersedes or retracts them)
/// that are not retracted, in spec 3.4's order, (valid_from, device, seq), with the uid last so
/// two claims of one event keep one order on every device (MUST-M7).
#[allow(dead_code)] // Task 7's candidates read it.
pub fn current(k: &Connection, repo: &str) -> Result<Vec<Claim>> {
    schema(k)?;
    let mut st = k.prepare(
        "SELECT c.uid, d.kind, d.status, d.speaker, d.scope, d.body, d.valid_from,
                d.anchor_device, d.anchor_seq
         FROM claims c JOIN derivations d ON d.op_device = c.op_device AND d.op_seq = c.op_seq
         WHERE d.repo = ?1 AND d.status <> 'retracted'
           AND NOT EXISTS (
             SELECT 1 FROM edges e
             JOIN claims a ON a.op_device = e.op_device AND a.op_seq = e.op_seq
             WHERE e.to_uid = c.uid AND a.uid <> c.uid)
         ORDER BY d.valid_from, d.anchor_device, d.anchor_seq, c.uid",
    )?;
    let rows = st.query_map([repo], |r| {
        Ok(Claim {
            uid: r.get(0)?,
            kind: r.get(1)?,
            status: r.get(2)?,
            speaker: r.get(3)?,
            scope: r.get(4)?,
            body: r.get(5)?,
            valid_from: r.get(6)?,
            device: r.get(7)?,
            seq: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
