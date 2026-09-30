//! Milestone 3 Task 9: the digests consumer. It reads the op log of every device that has ops and
//! keeps each digest op, with the uids it cites, in knowledge.db (`digest::schema`).

use crate::digest::{DigestOp, schema};
use crate::raw::{OpKind, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Digests;

/// Ops per step, one knowledge.db transaction each.
const BATCH: usize = 500;

impl Consumer for Digests {
    fn name(&self) -> &'static str {
        "digests"
    }

    fn reads_ops(&self) -> bool {
        true
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let ops = raw.ops_after(device, after, BATCH)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        for op in ops.iter().filter(|o| o.kind == OpKind::Digest) {
            let d = serde_json::from_value::<DigestOp>(op.body.clone());
            let fault = match &d {
                Ok(d) => d.fault(),
                Err(_) => Some("not a digest"),
            };
            if let Some(reason) = fault {
                k.execute(
                    "INSERT OR REPLACE INTO digest_skips(op_device, op_seq, reason)
                     VALUES(?1, ?2, ?3)",
                    params![device, op.op_seq, reason],
                )?;
                continue;
            }
            let d = d?;
            k.execute(
                "INSERT INTO digests(op_device, op_seq, ts, agent, session, repo, through_device,
                   through_seq, lines)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    device,
                    op.op_seq,
                    op.ts,
                    d.agent,
                    d.session,
                    d.repo,
                    d.through.device,
                    d.through.seq,
                    serde_json::to_string(&d.lines)?
                ],
            )?;
            for uid in d.lines.iter().flat_map(|l| &l.uids) {
                k.execute(
                    "INSERT OR IGNORE INTO digest_cites(op_device, op_seq, uid)
                     VALUES(?1, ?2, ?3)",
                    params![device, op.op_seq, uid],
                )?;
            }
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        for table in ["digests", "digest_cites", "digest_skips"] {
            k.execute(
                &format!("DELETE FROM {table} WHERE op_device = ?1 AND op_seq > ?2"),
                params![device, to],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A restore that lost ops takes their digests, citations and skips out with them.
    #[test]
    fn a_rewind_drops_the_lost_digests_and_what_they_cite() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let uid = "a".repeat(64);
        let digest = serde_json::json!({"agent": "claude", "session": "s", "repo": "r",
            "through": {"device": "d", "seq": 1}, "lines": [{"text": "Tabs.", "uids": [uid]}]});
        let broken = serde_json::json!({"lines": "no"});
        raw.append_ops(&[(OpKind::Digest, digest), (OpKind::Digest, broken)])
            .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        let count = |table: &str| -> i64 {
            k.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(Digests.step(&raw, &k, &device, 0).unwrap(), 2);
        let tables = ["digests", "digest_cites", "digest_skips"];
        assert_eq!(tables.map(count), [1, 1, 1]);
        Digests.rewind(&k, &device, 0).unwrap();
        assert_eq!(tables.map(count), [0, 0, 0]);
    }
}
