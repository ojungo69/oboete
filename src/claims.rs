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
    /// A change's reason as the record gives it, or `unknown` (spec 3.3); empty for other kinds.
    #[serde(default)]
    pub why: String,
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
         -- `decisions` walks a repository's newest first and stops at its limit.
         CREATE INDEX IF NOT EXISTS derivations_repo
           ON derivations(repo, valid_from, anchor_device, anchor_seq);
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
         CREATE INDEX IF NOT EXISTS claims_op ON claims(op_device, op_seq);
         -- The active derivations' bodies and quotes, for Task 7's candidates.
         CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(text, tokenize='trigram');
         -- Windows whose claims lost a quote to a mask or a removal since: Task 11 sends them
         -- again. With the claim op that lost it, so a rewind that loses the op takes it back.
         CREATE TABLE IF NOT EXISTS recurate(
           device TEXT NOT NULL, from_seq INTEGER NOT NULL, to_seq INTEGER NOT NULL,
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );
         -- The owner's corrections (spec 3.4, MUST-M21): each field the newest one gives
         -- applies over whatever derivation of the uid is active, so it survives recuration,
         -- re-derivation and rebuild. One for a uid with no claim yet waits for it.
         CREATE TABLE IF NOT EXISTS corrections(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, ts INTEGER NOT NULL,
           uid TEXT NOT NULL, status TEXT, body TEXT,
           PRIMARY KEY (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS corrections_uid ON corrections(uid, ts);
         -- Each claim as it is now: its active derivation with the owner's corrections over it.
         CREATE VIEW IF NOT EXISTS active AS
           SELECT c.uid, d.kind, d.speaker, d.scope, d.repo, d.valid_from, d.anchor_device,
             d.anchor_seq,
             COALESCE((SELECT x.status FROM corrections x WHERE x.uid = c.uid
                       AND x.status IS NOT NULL
                       ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                      d.status) AS status,
             COALESCE((SELECT x.body FROM corrections x WHERE x.uid = c.uid
                       AND x.body IS NOT NULL
                       ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                      d.body) AS body
           FROM claims c JOIN derivations d ON d.op_device = c.op_device AND d.op_seq = c.op_seq;
         -- Claim ops that gave no claim, and why: doctor counts them.
         CREATE TABLE IF NOT EXISTS claim_skips(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, reason TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );",
    )?;
    Ok(())
}

/// The body of a correction op, which `oboete correct` writes (spec 3.4, MUST-M21): the owner's
/// status or body for a claim, by uid and by the raw record the claim anchors on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrectionOp {
    pub uid: String,
    pub anchor: Anchor,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

/// A raw record: a claim's first quote's event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub device: String,
    pub seq: i64,
}

/// A claim body, at most (spec 6.5).
pub const MAX_BODY_CHARS: usize = 1_000;

impl CorrectionOp {
    /// Why it corrects nothing, if it does not: a synced op may come from any device.
    pub fn fault(&self) -> Option<&'static str> {
        let uid = self.uid.len() == 64 && self.uid.bytes().all(|b| b.is_ascii_hexdigit());
        if !uid {
            return Some("not a claim uid");
        }
        if self.status.is_none() && self.body.is_none() {
            return Some("corrects nothing");
        }
        if self
            .status
            .as_deref()
            .is_some_and(|s| !STATUSES.contains(&s))
        {
            return Some("an unknown status");
        }
        match self.body.as_deref() {
            Some(b) if b.trim().is_empty() => Some("an empty body"),
            Some(b) if b.chars().count() > MAX_BODY_CHARS => Some("over the 1,000-character cap"),
            _ => None,
        }
    }
}

/// `oboete correct`: the owner's correction of the claim `uid`, appended as a correction op. The
/// body goes through the same gate as every stored string; the claims consumer applies it, and
/// this returns once it has (at most 10 seconds, else an error that says it is recorded).
pub fn correct(
    home: &std::path::Path,
    uid: &str,
    status: Option<&str>,
    body: Option<&str>,
) -> Result<()> {
    use rusqlite::OptionalExtension;
    let rules = crate::capture::Settings::load(home)?.rules;
    // raw.db first, as every reader of knowledge.db holds it (a rebuild's swap waits for it).
    let mut raw = crate::raw::open(home)?;
    let k = crate::knowledge::open(home)?;
    schema(&k)?;
    let anchor = k
        .query_row(
            "SELECT anchor_device, anchor_seq FROM active WHERE uid = ?1",
            [uid],
            |r| {
                Ok(Anchor {
                    device: r.get(0)?,
                    seq: r.get(1)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no claim has the uid {uid}"))?;
    let op = CorrectionOp {
        uid: uid.to_owned(),
        anchor,
        status: status.map(str::to_owned),
        // As a typed prompt is stored: private blocks out (an unclosed one hides the rest), then
        // the scanners.
        body: body.map(|b| crate::redact::scan(&crate::hook::strip_blocks(b, true), &rules).0),
    };
    if let Some(why) = op.fault() {
        anyhow::bail!("the correction is refused: {why}");
    }
    let seqs = raw.append_ops(&[(crate::raw::OpKind::Correction, serde_json::to_value(&op)?)])?;
    let device = raw.device().to_owned();
    drop(raw);
    // Applied before this returns, by this process or by the worker already running (which wakes
    // on the new op): a search or a SessionStart right after never shows the old claim.
    crate::worker::run_once(home)?;
    let at = seqs.last().copied().unwrap_or(0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let applied = || -> Result<bool> {
        let got = crate::knowledge::checkpoint::get_in(
            &k,
            crate::knowledge::checkpoint::OPS,
            "claims",
            &device,
        )?;
        Ok(got >= at)
    };
    while !applied()? {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("the correction is recorded; the worker applies it when it next runs");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
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

/// A claim `a` (a row of the `active` view) is current: a chain tip (no active derivation of
/// another uid supersedes or retracts it) that is not retracted, by its derivation or the owner.
const TIP: &str = "a.status <> 'retracted'
    AND NOT EXISTS (
      SELECT 1 FROM edges e
      JOIN claims x ON x.op_device = e.op_device AND x.op_seq = e.op_seq
      WHERE e.to_uid = a.uid AND x.uid <> a.uid)";

/// When `uid` is a current claim, its repository (`None` for one anchored outside any) and its
/// active derivation.
pub fn tip(k: &Connection, uid: &str) -> Result<Option<(Option<String>, Claim)>> {
    use rusqlite::OptionalExtension;
    schema(k)?;
    Ok(k.query_row(
        &format!(
            "SELECT a.repo, a.uid, a.kind, a.status, a.speaker, a.scope, a.body, a.valid_from,
                    a.anchor_device, a.anchor_seq
             FROM active a WHERE a.uid = ?1 AND {TIP}"
        ),
        [uid],
        |r| {
            let claim = Claim {
                uid: r.get(1)?,
                kind: r.get(2)?,
                status: r.get(3)?,
                speaker: r.get(4)?,
                scope: r.get(5)?,
                body: r.get(6)?,
                valid_from: r.get(7)?,
                device: r.get(8)?,
                seq: r.get(9)?,
            };
            Ok((r.get(0)?, claim))
        },
    )
    .optional()?)
}

/// `repo`'s current claims: the chain tips (no active derivation supersedes or retracts them)
/// that are not retracted, in spec 3.4's order, (valid_from, device, seq), with the uid last so
/// two claims of one event keep one order on every device (MUST-M7).
pub fn current(k: &Connection, repo: &str) -> Result<Vec<Claim>> {
    tips(
        k,
        &format!("{TIPS} ORDER BY a.valid_from, a.anchor_device, a.anchor_seq, a.uid"),
        (repo,),
    )
}

/// `repo`'s current claims that are decided, or open items not done: at most `limit`, the newest
/// first (`current`'s order reversed). The manifest reads these at every SessionStart, so the
/// filter, the order and the limit are the query's.
pub fn decisions(k: &Connection, repo: &str, limit: usize) -> Result<Vec<Claim>> {
    tips(k, &format!("{TIPS} {DECIDED}"), (repo, limit as i64))
}

/// `decisions`' filter, order and limit (`?2`), which `derivations_repo` serves in order.
pub(crate) const DECIDED: &str =
    "AND (a.status = 'decided' OR (a.kind = 'open item' AND a.status <> 'done'))
     ORDER BY a.valid_from DESC, a.anchor_device DESC, a.anchor_seq DESC, a.uid DESC
     LIMIT ?2";

/// A repository's current claims (`TIP`, over the `active` view); `?1` is the repository.
pub(crate) const TIPS: &str =
    "SELECT a.uid, a.kind, a.status, a.speaker, a.scope, a.body, a.valid_from,
            a.anchor_device, a.anchor_seq
     FROM active a
     WHERE a.repo = ?1 AND a.status <> 'retracted'
       AND NOT EXISTS (
         SELECT 1 FROM edges e
         JOIN claims x ON x.op_device = e.op_device AND x.op_seq = e.op_seq
         WHERE e.to_uid = a.uid AND x.uid <> a.uid)";

fn tips(k: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<Claim>> {
    let mut st = k.prepare(sql)?;
    let rows = st.query_map(params, |r| {
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
