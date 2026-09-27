//! Digests (spec 3.4, 4.4; milestone 3 Task 9): a session's current claims in a repository, in a
//! few lines that each cite the claims they rest on. The curation phase writes one as a digest op
//! once a session's windows are covered; `consumer::digest` keeps them in knowledge.db; SessionStart
//! shows the repository's newest one only while every claim it cites is still current.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// A digest's text, all lines together, at most (spec 6.5).
pub const MAX_CHARS: usize = 2_000;

/// The body of a digest op.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DigestOp {
    pub agent: String,
    pub session: String,
    /// The repository its claims are of (`None` for claims anchored outside any).
    pub repo: Option<String>,
    /// The last record of the session it covers.
    pub through: Through,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Through {
    pub device: String,
    pub seq: i64,
}

/// One line and the uids of the claims it rests on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub text: String,
    pub uids: Vec<String>,
}

impl DigestOp {
    /// Why it is no digest to keep, if it is not: a synced op may come from any device.
    pub fn fault(&self) -> Option<&'static str> {
        if self.lines.is_empty() {
            return Some("no lines");
        }
        if self.lines.iter().any(|l| l.text.trim().is_empty()) {
            return Some("an empty line");
        }
        if self.lines.iter().any(|l| l.uids.is_empty()) {
            return Some("a line cites no claim");
        }
        if !self.lines.iter().flat_map(|l| &l.uids).all(|u| is_uid(u)) {
            return Some("a cited uid is not a claim uid");
        }
        let chars: usize = self.lines.iter().map(|l| l.text.chars().count()).sum();
        (chars > MAX_CHARS).then_some("over the 2,000-character cap")
    }
}

fn is_uid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "-- Every digest op kept: the newest of a repository is the one shown.
         CREATE TABLE IF NOT EXISTS digests(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, ts INTEGER NOT NULL,
           agent TEXT NOT NULL, session TEXT NOT NULL, repo TEXT,
           through_device TEXT NOT NULL, through_seq INTEGER NOT NULL,
           lines TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS digests_repo ON digests(repo, ts);
         -- The uids each digest cites: a deleted claim's digests are found by it (spec 6.2).
         CREATE TABLE IF NOT EXISTS digest_cites(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, uid TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq, uid)
         );
         CREATE INDEX IF NOT EXISTS digest_cites_uid ON digest_cites(uid);
         -- Digest ops that gave no digest, and why: doctor counts them.
         CREATE TABLE IF NOT EXISTS digest_skips(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, reason TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );",
    )?;
    Ok(())
}

/// `repo`'s newest digest, its lines' text, when every claim it cites is still a current claim of
/// `repo` (spec 3.4: a stale digest is not used). Read only: `None` where no digest was kept.
pub fn fresh(k: &Connection, repo: &str) -> Result<Option<Vec<String>>> {
    let kept = k
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'digests'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !kept {
        return Ok(None);
    }
    let Some(lines) = k
        .query_row(
            "SELECT lines FROM digests WHERE repo = ?1
             ORDER BY ts DESC, op_device DESC, op_seq DESC LIMIT 1",
            [repo],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let lines: Vec<Line> = serde_json::from_str(&lines)?;
    // One indexed lookup per cited uid: a chain tip of `repo` that is not retracted.
    let sql = format!("{} AND c.uid = ?2", crate::claims::TIPS);
    let mut tip = k.prepare(&sql)?;
    for uid in lines.iter().flat_map(|l| &l.uids) {
        if !tip.exists(params![repo, uid])? {
            return Ok(None);
        }
    }
    Ok(Some(lines.into_iter().map(|l| l.text).collect()))
}
