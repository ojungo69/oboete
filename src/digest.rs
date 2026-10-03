//! Digests (spec 3.4, 4.4; milestone 3 Task 9): a session's current claims in a repository, in a
//! few lines that each cite the claims they rest on. The curation phase wrote one as a digest op
//! once a session's windows were covered, until session summaries took its place
//! (docs/summaries.md); `consumer::digest` keeps the ops in knowledge.db, and SessionStart shows
//! the repository's newest one only while every claim it cites is still current.

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
    /// Each cited claim's `version` when the digest was written, in `uids`' order: a claim
    /// corrected or derived again since reads differently, and the digest is stale (#161). None
    /// in a digest op that predates them; then only the uids are checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seen: Vec<String>,
}

/// A claim's version as a digest records it: what its active derivation says, with the owner's
/// correction over it, so a digest is judged by content, never by clocks (#161).
pub fn version(status: &str, body: &str) -> String {
    crate::curate::sha256_hex(&format!("{status}\u{0}{body}"))[..16].to_owned()
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
        if self
            .lines
            .iter()
            .any(|l| !l.seen.is_empty() && l.seen.len() != l.uids.len())
        {
            return Some("a line's versions do not match its uids");
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
/// `repo` (spec 3.4: a stale digest is not used) and none is `hidden` (`claims::Pending`). Read
/// only: `None` where no digest was kept.
pub fn fresh(
    k: &Connection,
    repo: &str,
    hidden: impl Fn(&str) -> Result<bool>,
) -> Result<Option<Vec<String>>> {
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
    // One indexed lookup per cited uid: a chain tip of `repo` that is not retracted, and that
    // still says what it said when the digest was written (#161).
    let sql = format!("{} AND a.uid = ?2 AND a.muted = 0", crate::claims::TIPS);
    let mut tip = k.prepare(&sql)?;
    for l in &lines {
        for (i, uid) in l.uids.iter().enumerate() {
            let now = tip
                .query_row(params![repo, uid], |r| {
                    Ok(version(&r.get::<_, String>(2)?, &r.get::<_, String>(5)?))
                })
                .optional()?;
            match now {
                Some(v) if l.seen.get(i).is_none_or(|seen| *seen == v) && !hidden(uid)? => {}
                _ => return Ok(None),
            }
        }
    }
    Ok(Some(lines.into_iter().map(|l| l.text).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{Event, OpKind, test_event};
    use serde_json::json;

    /// A home with one session of `repo` whose records are curated: a prompt per body at `ts`,
    /// each with a decided claim quoting all of it, then an `end`. The claims' uids.
    fn home(bodies: &[&str], ts: i64) -> (tempfile::TempDir, Vec<String>) {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let event = |kind: &str, body: &str| Event {
            kind: kind.into(),
            session: "s1".into(),
            repo: Some("r".into()),
            ts,
            ..test_event(body)
        };
        let mut ops = Vec::new();
        let mut uids = Vec::new();
        for body in bodies {
            let e = event("prompt", &json!({ "prompt": body }).to_string());
            let seq = raw.append(&e).unwrap();
            let quote = crate::curate::long_text(&e).unwrap();
            let evidence = crate::claims::Evidence {
                device: raw.device().to_owned(),
                seq,
                offset: 0,
                length: quote.len() as i64,
                sentence: 0,
                quote: quote.clone(),
                claim_at: None,
            };
            uids.push(crate::claims::uid("decision", &evidence));
            let op = crate::claims::ClaimOp {
                id: format!("c{seq}"),
                kind: "decision".into(),
                status: "decided".into(),
                speaker: "user".into(),
                scope: "repo".into(),
                body: quote,
                evidence: vec![evidence],
                supersedes: Vec::new(),
                recipe: "test".into(),
                tier: 1,
                why: String::new(),
                tainted: false,
            };
            ops.push((OpKind::Claim, serde_json::to_value(op).unwrap()));
        }
        raw.append(&event("end", "")).unwrap();
        let to = raw.max_seq().unwrap();
        let window = json!({"outcome": "covered", "from_seq": 1, "from_offset": null,
            "to_seq": to, "to_offset": null, "elided": []});
        ops.insert(0, (OpKind::Window, window));
        raw.append_ops(&ops).unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        (home, uids)
    }

    /// A digest op of the session in `home`, one line citing `uids` with the claims' bodies as
    /// they read when it was written, as the digest phase wrote them; the consumers run.
    fn save(home: &std::path::Path, text: &str, cited: &[(&String, &str)]) {
        let mut raw = crate::raw::open(home).unwrap();
        let op = DigestOp {
            agent: "claude".into(),
            session: "s1".into(),
            repo: Some("r".into()),
            through: Through {
                device: raw.device().to_owned(),
                seq: raw.max_seq().unwrap(),
            },
            lines: vec![Line {
                text: text.into(),
                uids: cited.iter().map(|(u, _)| (*u).clone()).collect(),
                seen: cited.iter().map(|(_, b)| version("decided", b)).collect(),
            }],
        };
        raw.append_ops(&[(OpKind::Digest, serde_json::to_value(op).unwrap())])
            .unwrap();
        drop(raw);
        crate::worker::run_once(home).unwrap();
    }

    #[test]
    fn a_saved_digest_is_hidden_while_a_cited_claim_is_muted() {
        let (home, uids) = home(&["Use tabs."], 1_000);
        save(
            home.path(),
            "Use tabs for indentation.",
            &[(&uids[0], "Use tabs.")],
        );
        let k = crate::knowledge::open(home.path()).unwrap();
        assert_eq!(
            fresh(&k, "r", |_| Ok(false)).unwrap(),
            Some(vec!["Use tabs for indentation.".into()])
        );
        crate::claims::mute(home.path(), &uids[0], true).unwrap();
        assert_eq!(fresh(&k, "r", |_| Ok(false)).unwrap(), None);
        crate::claims::mute(home.path(), &uids[0], false).unwrap();
        assert_eq!(
            fresh(&k, "r", |_| Ok(false)).unwrap(),
            Some(vec!["Use tabs for indentation.".into()])
        );
    }

    /// #161: a digest whose cited claim reads otherwise since is stale.
    #[test]
    fn a_digest_whose_cited_claim_says_something_else_is_stale() {
        let (home, uids) = home(&["Use tabs."], 1_000);
        save(home.path(), "Tabs.", &[(&uids[0], "Use tabs.")]);
        let shown = || {
            let k = crate::knowledge::open(home.path()).unwrap();
            fresh(&k, "r", |_| Ok(false)).unwrap()
        };
        assert_eq!(shown(), Some(vec!["Tabs.".to_owned()]));
        // The same claim (its uid) from a higher tier, in other words: now its active one.
        let mut raw = crate::raw::open(home.path()).unwrap();
        let first = raw
            .ops_after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .find(|o| o.kind == OpKind::Claim)
            .unwrap();
        let mut again = first.body.clone();
        (again["body"], again["tier"]) = ("Use tabs, four wide.".into(), 2.into());
        raw.append_ops(&[(OpKind::Claim, again)]).unwrap();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(shown(), None);
    }
}
