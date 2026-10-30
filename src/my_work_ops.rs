//! What My Work's jobs write (docs/my-work-model.md S1 to S4): one op kind, `my_work`, whose body
//! names its job and what it is about, the entry that wrote it, what it was shown and the fields
//! kept. The consumer (`consumer::my_work`) keeps every op; a reader takes the newest of a job and
//! subject, whichever device wrote it.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::raw::Removal;

/// The jobs (S1): the curators', `rule`, the owner's action on a suggested rule, which holds no
/// model text, `focus`, My Focus's areas and placements (F8), and `judge`, the request judge's
/// decisions (J3). `links`, the goal links of slice 6, writes no more since slice 10c-3b; its ops
/// stay known, so a forget and an exclusion still reach them.
pub(crate) const JOBS: &[&str] = &[
    "group", "review", "links", "story", "rules", "goals", "rule", "focus", "place", "judge",
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
    /// What it is about: a repository (`group`, `judge`), a request (`review`), a week (`story`),
    /// a day (`goals`) or a suggestion (`rule`); empty for `links` and `rules`.
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
    let more = op.fields.get("more").cloned();
    let mut kept = crate::raw::fitted(serde_json::to_value(op)?);
    // M2 (#719): a skip kept in a goals op's place still says that its read stopped at the
    // budget, so the next chunk of a first run goes on at once; only while the cap holds.
    if let (Some(v), Some(more)) = (kept.as_mut(), more)
        && v["fields"].get("more").is_none()
    {
        let mut with = v.clone();
        with["fields"]["more"] = more;
        if with.to_string().len() <= crate::raw::MAX_OP_BYTES {
            *v = with;
        }
    }
    Ok(kept)
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
/// op seq: one shown a record of an excluded session or repository, or a summary or a card of one,
/// and every op built on one (`read.ops`), followed in op order, as a forget follows them (S5). One
/// built on a left-out op was shown its text, though its own records may miss the excluded
/// conversation's (Codex on #669). And while a forget is not finished, from its tombstone to its
/// purge (spec 6.2's local purge, step 1, the backstop; S5): one shown a record it took that the op
/// does not list as removed, or a summary or a card its reader leaves out for one (T7, K4), and
/// every op built on one, so no review's or group's text made from a forgotten record is shown,
/// nor sent as a previous review (Codex on #694). Read once a round; nothing is left out while
/// nothing is excluded and no forget waits for its rebuild.
pub(crate) fn hidden(
    raw: &crate::raw::Raw,
    k: &Connection,
    reading: &crate::curate::Reading,
) -> Result<std::collections::HashSet<(String, i64)>> {
    let mut hidden = std::collections::HashSet::new();
    // No session excluded, none of their records, summaries or cards to leave out, whatever the
    // list holds: a pattern no recorded session matched, as the owner's from claude-mem. A forget
    // past its rebuild marked what it took (S5), and the readers pass over what is marked.
    let forgetting = raw.forget_unrebuilt()?;
    if (reading.excluded.is_empty() && !forgetting)
        || !crate::consumer::manifest::exists(k, "table", "my_work")?
    {
        return Ok(hidden);
    }
    // The summaries and cards of an excluded session, judged by their session as their readers
    // show or keep them back (D13): a review left out for one a reader still shows would be due
    // again at once, call after call. A card of two sessions has none, and no reader shows it.
    let mut excluded_ops = std::collections::HashSet::new();
    if !reading.excluded.is_empty() {
        let sessions: Vec<&String> = reading.excluded.iter().collect();
        let sessions = serde_json::to_string(&sessions)?;
        for table in ["turns", "cards"] {
            if !crate::consumer::manifest::exists(k, "table", table)? {
                continue;
            }
            let mut st = k.prepare(&format!(
                "SELECT DISTINCT device, op_seq FROM {table}
                 WHERE agent IS NOT NULL AND session IS NOT NULL
                   AND agent || char(0) || session IN (SELECT value FROM json_each(?1))"
            ))?;
            for op in st.query_map([&sessions], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                excluded_ops.insert(op?);
            }
        }
    }
    // The summaries and cards a forget's tombstone leaves out, each looked up once.
    let mut taken = std::collections::HashMap::new();
    let mut st = k.prepare(
        "SELECT device, op_seq, read, removed, forgotten IS NOT NULL FROM my_work
         ORDER BY device, op_seq",
    )?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let (device, op_seq): (String, i64) = (r.get(0)?, r.get(1)?);
        let read: Read = serde_json::from_str(&r.get::<_, String>(2)?)?;
        // A forgotten op is the forget's already, as its readers take it.
        let forget = forgetting && !r.get::<_, bool>(4)?;
        let mut shown = true;
        for &seq in &read.ops {
            let op = (device.clone(), seq);
            if hidden.contains(&op) || excluded_ops.contains(&op) {
                shown = false;
            } else if forget {
                if !taken.contains_key(&op) {
                    let at = (device.as_str(), seq);
                    let t = derived_taken(k, raw, TURNS, at)? || derived_taken(k, raw, CARDS, at)?;
                    taken.insert(op.clone(), t);
                }
                shown = shown && !taken[&op];
            }
        }
        for &span in &read.spans {
            shown = shown && !raw.holds_session(&device, span, &reading.excluded)?;
        }
        if shown && forget {
            let listed: Vec<Removal> = serde_json::from_str(&r.get::<_, String>(3)?)?;
            for &(a, b) in &read.spans {
                if raw
                    .removed_in(&device, a, b, None)?
                    .iter()
                    .any(|x| !listed.contains(x))
                {
                    shown = false;
                }
            }
        }
        if !shown {
            hidden.insert((device, op_seq));
        }
    }
    Ok(hidden)
}

/// A derived table as `derived_taken` reads it: its name, its reader's columns, and its reader's
/// judge of a row.
type Derived = (
    &'static str,
    &'static str,
    fn(&rusqlite::Row, &crate::raw::Raw) -> Result<bool>,
);
const TURNS: Derived = ("turns", crate::turns::COLUMNS, crate::turns::taken_row);
const CARDS: Derived = ("cards", crate::cards::COLUMNS, crate::cards::taken_row);

/// Whether op `at`'s row in a derived table is one its reader leaves out now (T7, K4): an op built
/// on it is left out with it (S5). False with no row (an op that is none of the table's).
fn derived_taken(
    k: &Connection,
    raw: &crate::raw::Raw,
    (table, columns, taken): Derived,
    (device, op_seq): (&str, i64),
) -> Result<bool> {
    if !crate::consumer::manifest::exists(k, "table", table)? {
        return Ok(false);
    }
    let mut st = k.prepare_cached(&format!(
        "SELECT {columns} FROM {table} WHERE device = ?1 AND op_seq = ?2 LIMIT 1"
    ))?;
    let mut rows = st.query(params![device, op_seq])?;
    match rows.next()? {
        Some(r) => taken(r, raw),
        None => Ok(false),
    }
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
    newest_shown(
        k,
        (job, about),
        None,
        &std::collections::HashSet::new(),
        false,
    )
}

/// The owner's ops of `job` (R3, F6), each `(about, fields)`, holding no record it read: whether the
/// my_work consumer applied them within 10 seconds; an error when nothing was recorded.
pub(crate) fn record(home: &std::path::Path, job: &str, ops: Vec<(String, Value)>) -> Result<bool> {
    let rules = crate::capture::Settings::load(home)?.rules;
    let mut raw = crate::raw::open(home)?;
    let k = crate::knowledge::open(home)?;
    let ts = crate::db::now_ms();
    let mut bodies = Vec::new();
    for (about, fields) in ops {
        let op = MyWorkOp {
            job: job.to_owned(),
            about,
            entry: String::new(),
            ts,
            read: Read::default(),
            removed: Vec::new(),
            fields: fields
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            skipped: false,
            forgotten: None,
        };
        bodies.push((crate::raw::OpKind::MyWork, serde_json::to_value(op)?));
    }
    raw.gates_with(&rules);
    let seqs = raw.append_ops(&bodies)?;
    let device = raw.device().to_owned();
    drop(raw);
    Ok(crate::claims::applied(home, &k, &device, &seqs, (job, "my_work")).is_ok())
}

/// `newest` of `device`'s ops when one is named (a job shows only its own device's, S2), passing
/// over those left out (`hidden`, S6) and, when `answered`, the skips and the forgotten ones.
pub(crate) fn newest_shown(
    k: &Connection,
    about: (&str, &str),
    device: Option<&str>,
    hidden: &std::collections::HashSet<(String, i64)>,
    answered: bool,
) -> Result<Option<Kept>> {
    Ok(shown(k, (about.0, Some(about.1)), device, hidden, answered, 1)?.pop())
}

/// `newest_shown`'s `limit` newest, the newest first: a request's readings (P5). With no `about`,
/// the newest of every subject of the job: the goals of every day (M2).
pub(crate) fn shown(
    k: &Connection,
    (job, about): (&str, Option<&str>),
    device: Option<&str>,
    hidden: &std::collections::HashSet<(String, i64)>,
    answered: bool,
    limit: usize,
) -> Result<Vec<Kept>> {
    schema(k)?;
    let mut st = k.prepare(
        "SELECT device, op_seq, ts, entry, read, removed, fields, skipped, forgotten, about
         FROM my_work WHERE job = ?1 AND (?2 IS NULL OR about = ?2) AND (?3 IS NULL OR device = ?3)
           AND (?4 = 0 OR (skipped = 0 AND forgotten IS NULL))
         ORDER BY ts DESC, device DESC, op_seq DESC",
    )?;
    let mut rows = st.query(params![job, about, device, answered])?;
    let mut out = Vec::new();
    while out.len() < limit
        && let Some(r) = rows.next()?
    {
        let (device, op_seq): (String, i64) = (r.get(0)?, r.get(1)?);
        if hidden.contains(&(device.clone(), op_seq)) {
            continue;
        }
        out.push(Kept {
            device,
            op_seq,
            op: MyWorkOp {
                job: job.to_owned(),
                about: r.get(9)?,
                entry: r.get(3)?,
                ts: r.get(2)?,
                read: serde_json::from_str(&r.get::<_, String>(4)?)?,
                removed: serde_json::from_str(&r.get::<_, String>(5)?)?,
                fields: serde_json::from_str(&r.get::<_, String>(6)?)?,
                skipped: r.get(7)?,
                forgotten: r.get(8)?,
            },
        });
    }
    Ok(out)
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

    /// M2 (#719): a goals op over the cap, kept as a skip, still says that its read stopped at
    /// the budget; nothing else of its fields is kept.
    #[test]
    fn a_skip_kept_in_a_goals_ops_place_keeps_more() {
        let big = "x".repeat(crate::raw::MAX_OP_BYTES);
        let w = op(
            "goals",
            "2026-10-27",
            1,
            json!({"items": big, "more": true}),
        );
        let v = fitted(w).unwrap().unwrap();
        assert_eq!(
            (v["skipped"].clone(), v["fields"].clone()),
            (json!(true), json!({"more": true}))
        );
        let w = op("goals", "2026-10-27", 1, json!({"items": big}));
        assert_eq!(fitted(w).unwrap().unwrap()["fields"], json!({}));
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
