//! What My Work's jobs write (docs/my-work-model.md S1 to S4): one op kind, `my_work`, whose body
//! names its job and what it is about, the entry that wrote it, what it was shown and the fields
//! kept. The consumer (`consumer::my_work`) keeps every op; a reader takes the newest of a job and
//! subject, whichever device wrote it.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::raw::Removal;

/// The jobs (S1): the curators' six, and `rule`, the owner's action on a suggested rule, which holds
/// no model text.
pub(crate) const JOBS: &[&str] = &[
    "group", "review", "links", "story", "rules", "goals", "rule",
];

/// What a job was shown (S2): the spans of this device's records, and the op seqs of the turn,
/// window and `my_work` ops whose text it read, the previous review or goals among them. A forget
/// follows them (S5), and so does an exclusion (S6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Read {
    #[serde(default)]
    pub spans: Vec<(i64, i64)>,
    #[serde(default)]
    pub ops: Vec<i64>,
}

/// One `my_work` op's body (S2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MyWorkOp {
    /// One of `JOBS`.
    pub job: String,
    /// What it is about: a repository (`group`), a request (`review`), a week (`story`), a day
    /// (`goals`) or a suggestion (`rule`); empty for `links` and `rules`.
    pub about: String,
    /// The chain entry that wrote it; empty for `rule`.
    pub entry: String,
    /// When it was written, unix ms.
    pub ts: i64,
    pub read: Read,
    /// What was removed from the records it read when it was asked, as a turn op lists it.
    #[serde(default)]
    pub removed: Vec<Removal>,
    /// The fields kept, by the job's names; none for a skip.
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
    #[serde(default)]
    pub skipped: bool,
    /// The job of the forget that took its text (S5): its job is due again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forgotten: Option<String>,
}

/// `op` as it can be kept within an op's cap (`raw::MAX_OP_BYTES`): whole, else without its
/// removals, else as a skip of its labels; none when even that passes the cap. A job whose op could
/// not be kept even as a skip is not asked for, as nothing paid for could be kept (as
/// `turns::fitted`). A job that drops quotes first does so before this.
pub(crate) fn fitted(op: MyWorkOp) -> Result<Option<Value>> {
    Ok(crate::raw::fitted(serde_json::to_value(op)?))
}

/// knowledge.db's `my_work` (S4): every op kept, as it came; a reader takes the newest of a job and
/// subject (`newest`), so a rewind of one device leaves the other devices' ops as they were.
pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS my_work(
           device TEXT NOT NULL,
           op_seq INTEGER NOT NULL,
           ts INTEGER NOT NULL,
           job TEXT NOT NULL,
           about TEXT NOT NULL,
           entry TEXT NOT NULL,
           read TEXT NOT NULL,
           removed TEXT NOT NULL,
           fields TEXT NOT NULL,
           skipped INTEGER NOT NULL,
           forgotten TEXT,
           PRIMARY KEY(device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS my_work_about ON my_work(job, about, ts);",
    )?;
    Ok(())
}

/// S6: the ops of any job left out while the exclusions stand as `reading` read them, by device and
/// op seq: one shown a record of an excluded session or repository, and every op built on one
/// (`read.ops`), followed in op order, as a forget follows them (S5). One built on a left-out op
/// was shown its text, though its own records may miss the excluded conversation's (Codex on
/// #669). Read once a round; nothing is left out while nothing is excluded.
pub(crate) fn hidden(
    raw: &crate::raw::Raw,
    k: &Connection,
    reading: &crate::curate::Reading,
) -> Result<std::collections::HashSet<(String, i64)>> {
    let mut hidden = std::collections::HashSet::new();
    if (reading.excluded.is_empty() && reading.exclusions.is_empty())
        || !crate::consumer::manifest::exists(k, "table", "my_work")?
    {
        return Ok(hidden);
    }
    let mut st = k.prepare("SELECT device, op_seq, read FROM my_work ORDER BY device, op_seq")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (device, op_seq, read) = row?;
        let read: Read = serde_json::from_str(&read)?;
        let built_on_hidden = (read.ops.iter()).any(|seq| hidden.contains(&(device.clone(), *seq)));
        if built_on_hidden || !shown_now(raw, reading, &device, &read)? {
            hidden.insert((device, op_seq));
        }
    }
    Ok(hidden)
}

/// Whether an op `device` wrote, which was shown `read`, may be shown and sent while the
/// exclusions stand as `reading` read them (S6): none of its records is of an excluded session or
/// repository.
fn shown_now(
    raw: &crate::raw::Raw,
    reading: &crate::curate::Reading,
    device: &str,
    read: &Read,
) -> Result<bool> {
    for &(from, to) in &read.spans {
        let labels = raw.labels_in(device, from, to)?;
        let session = labels.session.map(|(a, s)| format!("{a}\0{s}"));
        if session.is_some_and(|s| reading.excluded.contains(&s))
            || labels.repo.is_some_and(|r| reading.exclusions.contains(&r))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A kept op as a reader takes it: who wrote it, where, and its body.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Kept {
    pub device: String,
    pub op_seq: i64,
    pub op: MyWorkOp,
}

/// The newest op of `job` about `about`, whichever device wrote it: the latest time, then the
/// greater device and op seq, as the newest hide op wins (card-hide.md). The older ones are
/// replaced (S4).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn newest(k: &Connection, job: &str, about: &str) -> Result<Option<Kept>> {
    schema(k)?;
    let row = k
        .query_row(
            "SELECT device, op_seq, ts, entry, read, removed, fields, skipped, forgotten FROM my_work
             WHERE job = ?1 AND about = ?2 ORDER BY ts DESC, device DESC, op_seq DESC LIMIT 1",
            params![job, about],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, bool>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()?;
    let Some((device, op_seq, ts, entry, read, removed, fields, skipped, forgotten)) = row else {
        return Ok(None);
    };
    Ok(Some(Kept {
        device,
        op_seq,
        op: MyWorkOp {
            job: job.to_owned(),
            about: about.to_owned(),
            entry,
            ts,
            read: serde_json::from_str(&read)?,
            removed: serde_json::from_str(&removed)?,
            fields: serde_json::from_str(&fields)?,
            skipped,
            forgotten,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::OpKind;
    use crate::search::b::fixture::Store;
    use crate::worker::Consumer;
    use serde_json::json;

    fn op(job: &str, about: &str, ts: i64, fields: Value) -> MyWorkOp {
        MyWorkOp {
            job: job.into(),
            about: about.into(),
            entry: "anthropic".into(),
            ts,
            read: Read {
                spans: vec![(1, 2)],
                ops: vec![3],
            },
            removed: vec![],
            fields: serde_json::from_value(fields).unwrap(),
            skipped: false,
            forgotten: None,
        }
    }

    fn stamp(path: &std::path::Path) -> i64 {
        rusqlite::Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    /// S3: the first My Work op raises the home to format 5 in the transaction that writes it,
    /// above a hide op's 4 in the same batch, and a rebuild from the backup's ops stamps it too.
    #[test]
    fn a_my_work_op_raises_the_home_to_the_format_that_reads_it() {
        let mut s = Store::new();
        let home = s.home.path().to_owned();
        s.run();
        assert_eq!(stamp(&home.join("raw.db")), crate::raw::OPEN_FORMAT);
        let goals = serde_json::to_value(op("goals", "2026-10-26", 1, json!({}))).unwrap();
        s.raw
            .append_ops(&[(OpKind::Hide, json!({})), (OpKind::MyWork, goals)])
            .unwrap();
        assert_eq!(stamp(&home.join("raw.db")), crate::raw::MY_WORK_FORMAT);
        let (device, lines, ops) = (
            s.raw.device().to_owned(),
            s.raw.export_lines(0, usize::MAX).unwrap(),
            s.raw.export_op_lines(0, usize::MAX).unwrap(),
        );
        let rebuilt = home.join("raw.db.rebuilt");
        let mut rebuild = crate::raw::Rebuild::new(&rebuilt, &device).unwrap();
        for (_, line) in &lines {
            rebuild.add(line).unwrap();
        }
        for (_, line) in &ops {
            rebuild.add_op(line).unwrap();
        }
        rebuild.finish().unwrap();
        assert_eq!(stamp(&rebuilt), crate::raw::MY_WORK_FORMAT);
    }

    /// S4: the newest op of a job and subject is read, whichever device wrote it and in whatever
    /// order the devices are read; a rewind of one device takes its ops away and the one before is
    /// read again; an op of a job this binary does not know is none.
    #[test]
    fn the_newest_op_of_a_job_and_subject_is_read_whichever_device_wrote_it() {
        for (older, newer) in [("aaaa", "zzzz"), ("zzzz", "aaaa")] {
            let s = Store::new();
            let put = |(device, op_seq): (&str, i64), w: &MyWorkOp| {
                rusqlite::Connection::open(s.home.path().join("raw.db"))
                    .unwrap()
                    .execute(
                        "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                         VALUES(?1, ?2, 'my_work', ?3, ?4, ?2)",
                        params![device, op_seq, w.ts, serde_json::to_string(w).unwrap()],
                    )
                    .unwrap();
            };
            // The older op has the higher op seq: only the time orders them.
            put(
                (older, 9),
                &op("review", "R1", 1_000, json!({"summary": "Old."})),
            );
            put(
                (newer, 1),
                &op("review", "R1", 2_000, json!({"summary": "New."})),
            );
            put((newer, 2), &op("unknown", "R1", 3_000, json!({})));
            s.run();
            let k = crate::knowledge::open(s.home.path()).unwrap();
            let kept = newest(&k, "review", "R1").unwrap().unwrap();
            let summary = &kept.op.fields["summary"];
            assert_eq!((kept.device.as_str(), summary), (newer, &json!("New.")));
            assert_eq!(kept.op.read, op("", "", 0, json!({})).read);
            assert_eq!(newest(&k, "unknown", "R1").unwrap(), None);
            crate::consumer::my_work::MyWork
                .rewind(&k, newer, 0)
                .unwrap();
            let kept = newest(&k, "review", "R1").unwrap().unwrap();
            assert_eq!(kept.op.fields["summary"], json!("Old."), "{older} {newer}");
        }
    }

    /// S2: an op over the op cap loses its removals first, then is kept as a skip of its labels.
    #[test]
    fn an_op_over_the_cap_is_kept_without_its_removals_then_as_a_skip() {
        let big = "x".repeat(crate::raw::MAX_OP_BYTES);
        let mut w = op("review", "R1", 1, json!({"summary": "Kept."}));
        w.removed = (0..10_000).map(|i| (i, Some(i), Some(i))).collect();
        let v = fitted(w.clone()).unwrap().unwrap();
        assert_eq!(
            (v["removed"].clone(), v["fields"]["summary"].clone()),
            (json!([]), json!("Kept."))
        );
        w.fields.insert("summary".into(), json!(big));
        let v = fitted(w).unwrap().unwrap();
        assert_eq!(
            (v["skipped"].clone(), v["fields"].clone()),
            (json!(true), json!({}))
        );
        assert_eq!(
            (v["job"].clone(), v["about"].clone()),
            (json!("review"), json!("R1"))
        );
        let mut w = op("review", "R1", 1, json!({}));
        w.about = big;
        assert_eq!(
            fitted(w).unwrap(),
            None,
            "labels past the cap are kept nowhere"
        );
    }
}
