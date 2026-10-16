//! Milestone 5 slice 3e-3 (spec 6.2, docs/retention.md): a raw record's text expires once
//! `[retention] days` passed and curation and the turn summaries are done with it. Its labels,
//! ledger and identity stay: an expired record is read as `raw::Item::Expired`.

use crate::curate::Span;
use crate::raw::{Raw, ScanRow};
use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// How often the worker looks for records past the period; an owed batch is looked for at every
/// `forget::continue_due` (docs/retention.md).
const LOOK_EVERY_MS: i64 = 24 * 60 * 60 * 1000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The worker's last look, in unix ms.
const LOOK: &str = "retention-look";
/// Step 1's copy of the batch, which a restore applies when raw.db is lost.
pub(crate) const OWED: &str = "expiry-owed";

/// What one pass did.
#[derive(Debug, Default, PartialEq)]
pub struct Pass {
    /// Records whose text went.
    pub expired: usize,
    /// Records past the period that wait (for curation, a summary, a session, a quote).
    pub waiting: usize,
    /// A batch is still owed: a mask is, or a reader kept raw.db-wal from truncating.
    pub owed: bool,
}

/// Whether a pass is due: a batch is owed, or a period is set and the last look is a day old.
pub(crate) fn due(home: &Path, raw: &Raw) -> Result<bool> {
    if !raw.expiry_owed()?.is_empty() {
        return Ok(true);
    }
    // A wrong table expires nothing until it loads: doctor says so.
    if !crate::config::retention(home).is_ok_and(|r| r.days.is_some()) {
        return Ok(false);
    }
    Ok(!looked_lately(home))
}

/// Whether the last look (`state/retention-look`) is less than a day old.
fn looked_lately(home: &Path) -> bool {
    std::fs::read_to_string(home.join("state").join(LOOK))
        .ok()
        .and_then(|t| t.trim().parse::<i64>().ok())
        .is_some_and(|at| (0..LOOK_EVERY_MS).contains(&(crate::db::now_ms() - at)))
}

/// One pass (docs/retention.md, "How a pass runs"): an owed batch finished first, whatever the
/// setting says; else, with a period set and no mask owed, the windows every rule lets go begun as
/// a batch and finished. Under `state/purge.lock` (None when a purge or another pass holds it) and
/// the worker lock.
pub fn pass(home: &Path) -> Result<Option<Pass>> {
    let Some(_purge) = crate::forget::purge_lock(home)? else {
        return Ok(None);
    };
    let mut done = Pass::default();
    if !crate::raw::exists(home) {
        return Ok(Some(done));
    }
    // A wrong table expires nothing until it loads: doctor says so. With no period and no batch
    // owed there is nothing to do, and the worker is not asked to stop for it.
    let days = crate::config::retention(home).ok().and_then(|r| r.days);
    if days.is_none() && crate::raw::open(home)?.expiry_owed()?.is_empty() {
        return Ok(Some(done));
    }
    if !rows(home, days, &mut done)? {
        return Ok(Some(done));
    }
    // Step 5: knowledge.db built again from raw.db, as a forget's purge builds it (its step 3). A
    // full-text index keeps a deleted record's trigrams and their places in its segments until they
    // merge, so only a new file holds none of the expired text (the commit security review of part
    // 2). No carry takes an expired record's vector (`forgotten_record_keys`). The rebuild takes
    // the worker lock and raw itself.
    crate::worker::rebuild(home).context("knowledge.db was not rebuilt")?;
    crate::backup::fail_after("expiry_knowledge")?;
    let _worker = crate::worker::lock_asking(home)?;
    let raw = crate::raw::open(home)?;
    // The copies of knowledge.db set aside held what the old one did: their vectors carried, as a
    // purge carries them, then removed.
    let aside = crate::forget::aside_files(home)?;
    if !aside.is_empty() {
        crate::embed_phase::carry_set_aside(home, &crate::knowledge::open(home)?, &raw)?;
        for file in aside {
            std::fs::remove_file(&file)
                .with_context(|| format!("{} was not removed", file.display()))?;
        }
    }
    raw.clear_expiry_owed()?;
    remove_owed(home)?;
    Ok(Some(done))
}

/// Steps 1 to 4 under the worker lock: whether the batch's rows and raw.db's WAL are done, so
/// step 5 may run. A batch owed is finished first, whatever the setting says; else, with a period
/// set and no mask owed, the windows every rule lets go begin as one.
fn rows(home: &Path, days: Option<u32>, done: &mut Pass) -> Result<bool> {
    let _worker = crate::worker::lock_asking(home)?;
    let mut raw = crate::raw::open(home)?;
    let mut owed = raw.expiry_owed()?;
    if owed.is_empty() {
        // A new batch at most once a day, however often `forget --continue` runs for a forget or
        // a mask (Codex on #552); an owed one is finished at every pass.
        let Some(days) = days.filter(|_| !looked_lately(home)) else {
            return Ok(false);
        };
        // Stamped first: a look that fails is not tried again at once.
        let state = home.join("state");
        std::fs::create_dir_all(&state)?;
        std::fs::write(state.join(LOOK), crate::db::now_ms().to_string())?;
        let k = crate::knowledge::open(home)?;
        let plan = plan(home, &raw, &k, days, crate::db::now_ms())?;
        done.waiting = plan.waiting;
        // Rule 7: while a mask is owed no batch begins, and its windows wait too.
        if raw.masks_owed()?.is_some() {
            done.waiting += plan.records.len();
            return Ok(false);
        }
        if plan.records.is_empty() {
            return Ok(false);
        }
        // The batch's records and the curation ops made from them sealed in the backups first: a
        // restore that finishes a batch on records it expires brings back their claims and
        // summaries too, or nothing begins (Codex on part 2 of 3e-3).
        crate::backup::export_timed(home).context("no expiry begins without its backup")?;
        // A session that moved since it was read waits for the next look.
        if !raw.begin_expiry(&plan.records, &plan.sessions)? {
            return Ok(false);
        }
        owed = raw.expiry_owed()?;
        crate::backup::fail_after("expiry_committed")?;
    }
    write_owed(home, &owed)?;
    crate::backup::fail_after("expiry_begun")?;
    let at = crate::db::now_ms();
    let owed_map: HashMap<(String, i64), i64> = owed
        .iter()
        .map(|(device, seq, ts)| ((device.clone(), *seq), *ts))
        .collect();
    // Step 2: no backup or copy holds the text once raw.db does not.
    crate::backup::expire_record_segments(home, &owed_map, at)
        .context("the backups of the expiring records were not rewritten")?;
    let mut expired = raw.expired_identities()?;
    expired.extend(owed_map);
    crate::raw::expire_copies(home, &expired, at)?;
    crate::backup::fail_after("expiry_backups")?;
    // Step 3, once no mask is owed: `forget --continue` purges the masks first.
    if raw.masks_owed()?.is_some() {
        done.owed = true;
        return Ok(false);
    }
    done.expired = raw.expire_owed(at)?;
    crate::backup::fail_after("expiry_rows")?;
    // Step 4.
    if !raw.truncate_wal(crate::forget::TRUNCATE_WAIT)? {
        done.owed = true;
        return Ok(false);
    }
    crate::backup::fail_after("expiry_wal")?;
    Ok(true)
}

/// doctor's lines: the period, the last look and how many records expired, a batch owed; and
/// whether `[retention]` is wrong, which expires nothing until it loads. With no store yet the
/// table is read all the same (Codex on #552).
pub(crate) fn doctor_lines(home: &Path, raw: Option<&Raw>) -> Result<(Vec<String>, bool)> {
    let mut lines = Vec::new();
    let wrong = match crate::config::retention(home) {
        Ok(r) => {
            lines.push(match r.days {
                None => "retention: forever".to_owned(),
                Some(days) => {
                    let last = std::fs::read_to_string(home.join("state").join(LOOK))
                        .ok()
                        .and_then(|t| t.trim().parse::<i64>().ok())
                        .map_or_else(|| "none yet".to_owned(), crate::db::utc);
                    format!(
                        "retention: {days} day{} (a window expires once curated and past it; last \
                         look {last}, {} records expired)",
                        if days == 1 { "" } else { "s" },
                        raw.map(Raw::expired_count).transpose()?.unwrap_or(0)
                    )
                }
            });
            false
        }
        Err(e) => {
            lines.push(format!("retention: {e:#}; nothing expires until it loads"));
            true
        }
    };
    let owed = raw
        .map(Raw::expiry_owed)
        .transpose()?
        .map_or(0, |o| o.len());
    if owed > 0 {
        lines.push(format!(
            "retention: a batch of {owed} records is begun and not finished (the worker finishes it)"
        ));
    }
    Ok((lines, wrong))
}

/// The batch a pass may begin: its records (seq and time), each session they belong to with its
/// newest record as read, and how many records past the period wait.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    pub(crate) records: Vec<(i64, i64)>,
    pub(crate) sessions: Vec<(Option<String>, Option<String>, i64)>,
    pub(crate) waiting: usize,
}

type Session = (Option<String>, Option<String>);

/// docs/retention.md, "What expires": this device's windows in seq order, each expiring whole once
/// rules 1 to 6 and 8 hold (7 is the pass's).
pub(crate) fn plan(home: &Path, raw: &Raw, k: &Connection, days: u32, now: i64) -> Result<Plan> {
    crate::claims::schema(k)?;
    crate::turns::schema(k)?;
    let device = raw.device().to_owned();
    let rows = raw.expiry_scan()?;
    let ops = raw.window_bodies()?;
    let before = now - i64::from(days) * DAY_MS;
    let checkpoint = crate::curate::curated_through_raw(raw)?;
    let mut unread = crate::curate::unread_spans(&ops);
    unread.extend(queued(k, &device)?);
    // Rule 5: every record consumer has read up to here.
    let mut read = i64::MAX;
    for c in crate::worker::consumers(home)
        .iter()
        .filter(|c| !c.reads_ops())
    {
        read = read.min(crate::knowledge::checkpoint::get(k, c.name(), &device)?);
    }
    let (last_asked, kept_back) = turns(k, &device)?;
    let key = |r: &ScanRow| -> Option<Session> {
        r.session
            .is_some()
            .then(|| (r.agent.clone(), r.session.clone()))
    };
    let mut newest: HashMap<Session, i64> = HashMap::new();
    let mut replies: HashMap<Session, Vec<i64>> = HashMap::new();
    for r in &rows {
        if let Some(s) = key(r) {
            newest.insert(s.clone(), r.seq);
            if r.kind == "reply" && live(r) {
                replies.entry(s).or_default().push(r.seq);
            }
        }
    }
    let unquoted = unquoted(raw)?;
    let mut blocked: HashSet<Session> = HashSet::new();
    let mut sessions: HashSet<Session> = HashSet::new();
    let mut plan = Plan::default();
    for (from, to) in units(&ops) {
        let start = rows.partition_point(|r| r.seq < from);
        let end = rows.partition_point(|r| r.seq <= to);
        let expiring: Vec<&ScanRow> = rows[start..end]
            .iter()
            .filter(|r| !r.expired && !exempt(r))
            .collect();
        if expiring.is_empty() {
            continue;
        }
        let keys: HashSet<Session> = expiring.iter().filter_map(|r| key(r)).collect();
        // Rule 1, each record's own time: a window inside the period stays, and is no wait.
        if expiring.iter().any(|r| r.ts >= before) {
            blocked.extend(keys);
            continue;
        }
        let asked = |r: &&ScanRow| {
            let Some(s) = key(r).filter(|_| live(r)) else {
                return true;
            };
            if kept_back.contains(&s) {
                return false;
            }
            let list = replies.get(&s).map(Vec::as_slice).unwrap_or(&[]);
            list.get(list.partition_point(|seq| *seq < r.seq))
                .is_none_or(|reply| *reply <= last_asked)
        };
        let eligible = to <= checkpoint
            && keys.iter().all(|s| newest[s] <= checkpoint)
            && !unread.iter().any(|u| u.from <= to && from <= u.to)
            && expiring.iter().all(asked)
            && to <= read
            && keys.is_disjoint(&blocked)
            && !expiring.iter().any(|r| unquoted.contains(&r.seq));
        if !eligible {
            plan.waiting += expiring.len();
            blocked.extend(keys);
            continue;
        }
        plan.records.extend(expiring.iter().map(|r| (r.seq, r.ts)));
        sessions.extend(keys);
    }
    plan.sessions = sessions
        .into_iter()
        .map(|s| {
            let at = newest[&s];
            (s.0, s.1, at)
        })
        .collect();
    plan.sessions.sort();
    Ok(plan)
}

/// docs/retention.md: the windows records expire by, in seq order: each window op's records, and
/// the windows a recuration's range or `covers` overlaps joined into one.
fn units(ops: &[(i64, Value)]) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = ops
        .iter()
        .flat_map(|(_, body)| {
            [
                crate::curate::op_span(body),
                crate::curate::op_span(&body["covers"]),
            ]
        })
        .flatten()
        .map(|s| (s.from, s.to))
        .collect();
    spans.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (from, to) in spans {
        match out.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    out
}

/// A record that never expires: an `oboete pref add` directive, a preference's only evidence, and
/// a v1 touch, labels only.
fn exempt(r: &ScanRow) -> bool {
    matches!(r.kind.as_str(), "directive" | "touch")
}

/// A record the agent's hooks captured (or a replay of them): its session has turns.
fn live(r: &ScanRow) -> bool {
    crate::raw::is_live(&r.source)
}

/// Rule 3: the spans queued for recuration on `device`.
fn queued(k: &Connection, device: &str) -> Result<Vec<Span>> {
    let mut st = k.prepare("SELECT from_seq, to_seq FROM recurate WHERE device = ?1")?;
    let rows = st.query_map([device], |r| Ok(Span::records(r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Rule 4: the reply of the last turn asked for (`turns::last_asked`), and the sessions with a turn
/// the exclusion list keeps back (excluded, and not asked for since).
fn turns(k: &Connection, device: &str) -> Result<(i64, HashSet<Session>)> {
    let last: i64 = k.query_row(
        "SELECT COALESCE(MAX(through), 0) FROM turns WHERE device = ?1",
        [device],
        |r| r.get(0),
    )?;
    let mut st = k.prepare(
        "SELECT DISTINCT agent, session FROM turns t WHERE device = ?1 AND excluded = 1
           AND NOT EXISTS (SELECT 1 FROM turns a WHERE a.device = t.device AND a.agent = t.agent
             AND a.session = t.session AND a.through = t.through AND a.excluded = 0)",
    )?;
    let kept = st.query_map([device], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok((last, kept.collect::<rusqlite::Result<_>>()?))
}

/// Rule 8: this device's records a claim op quotes where the quote does not read, the ops a forget
/// or a mask rewrote aside.
fn unquoted(raw: &Raw) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    for e in raw.claim_quotes()? {
        if !matches!(
            crate::consumer::claims::quote(raw, &e)?,
            crate::consumer::claims::Quote::Reads(_)
        ) {
            out.insert(e.seq);
        }
    }
    Ok(out)
}

/// Step 1's file: the batch's records, one `device seq ts` line each, synced, renamed into place,
/// its directory synced.
fn write_owed(home: &Path, owed: &[(String, i64, i64)]) -> Result<()> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let text: String = owed
        .iter()
        .map(|(device, seq, ts)| format!("{device} {seq} {ts}\n"))
        .collect();
    let path = state.join(OWED);
    if std::fs::read_to_string(&path).is_ok_and(|now| now == text) {
        return Ok(());
    }
    let tmp = state.join(format!(".{OWED}.tmp"));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    std::fs::File::open(&state)?.sync_all()?;
    Ok(())
}

/// `state/expiry-owed`'s records, for a restore; none when there is no file.
pub(crate) fn read_owed(home: &Path) -> Result<Vec<(String, i64, i64)>> {
    let path = home.join("state").join(OWED);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    text.lines()
        .map(|line| {
            let mut parts = line.split(' ');
            let (Some(device), Some(seq), Some(ts), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                anyhow::bail!("{} is damaged", path.display());
            };
            Ok((device.to_owned(), seq.parse()?, ts.parse()?))
        })
        .collect()
}

fn remove_owed(home: &Path) -> Result<()> {
    match std::fs::remove_file(home.join("state").join(OWED)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{Event, Item, OpKind, Target};
    use serde_json::json;

    /// Words only the expiring records hold.
    const CANARY: &str = "rcanary-7d2f9a4c1e8b3d6f0a5c2e9b";

    /// A record of `session` the agent's hooks captured: its seq.
    fn said(raw: &mut Raw, session: &str, kind: &str, ts: i64, text: &str) -> i64 {
        raw.append(&Event {
            kind: kind.into(),
            session: session.into(),
            repo: Some("github.com/o/r".into()),
            ts,
            ..crate::raw::test_event(&json!({ "prompt": text }).to_string())
        })
        .unwrap()
    }

    /// A window op over `from` to `to`, `extra` merged into it.
    fn window(raw: &mut Raw, from: i64, to: i64, extra: Value) {
        let mut op = json!({"from_seq": from, "from_offset": null, "to_seq": to,
            "to_offset": null, "outcome": "curated"});
        if let (Some(o), Value::Object(x)) = (op.as_object_mut(), extra) {
            o.extend(x);
        }
        raw.append_ops(&[(OpKind::Window, op)]).unwrap();
    }

    /// `session`'s turn `from` to `through` asked for, or kept back by the exclusion list.
    fn asked(raw: &mut Raw, session: &str, from: i64, through: i64, kept_back: bool) {
        let op = json!({"agent": "claude", "session": session, "repo": null, "ts": 0,
            "from": from, "through": through, "read": [], "goals": [], "removed": [],
            "skipped": kept_back, "excluded": kept_back});
        raw.append_ops(&[(OpKind::Turn, op)]).unwrap();
    }

    fn period(home: &Path, days: Option<u32>) {
        let text = days.map_or_else(String::new, |d| format!("[retention]\ndays = {d}\n"));
        std::fs::write(home.join("config.toml"), text).unwrap();
    }

    /// A turn of session `s` long past: a prompt holding the canary and its reply, curated and
    /// summarized, and read by every consumer. Their seqs.
    fn finished(home: &Path) -> (i64, i64) {
        let mut raw = crate::raw::open(home).unwrap();
        let a = said(
            &mut raw,
            "s",
            "prompt",
            1_000,
            &format!("Build it {CANARY} zqxjkvwy."),
        );
        let b = said(&mut raw, "s", "reply", 2_000, "Built.");
        window(&mut raw, a, b, json!({}));
        asked(&mut raw, "s", a, b, false);
        drop(raw);
        crate::worker::run_once(home).unwrap();
        (a, b)
    }

    fn expired(home: &Path, seq: i64) -> bool {
        let raw = crate::raw::open(home).unwrap();
        let device = raw.device().to_owned();
        matches!(
            raw.after(&device, seq - 1, 1).unwrap()[0].item,
            Item::Expired(_)
        )
    }

    /// Every file under `home` that holds the canary: raw.db, its WAL and copies as they are,
    /// backup segments decompressed.
    fn holding(home: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut dirs = vec![home.to_owned()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                let mut bytes = std::fs::read(&path).unwrap();
                if crate::backup::segment_name(path.file_name().unwrap()).is_some() {
                    bytes = zstd::stream::decode_all(&bytes[..]).unwrap_or(bytes);
                }
                if bytes.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()) {
                    found.push(name);
                }
            }
        }
        found
    }

    #[test]
    fn a_recuration_joins_the_windows_it_overlaps_and_a_split_record_its_two() {
        let ops: Vec<(i64, Value)> = [
            json!({"from_seq": 1, "to_seq": 4}),
            json!({"from_seq": 4, "to_seq": 6}),
            json!({"from_seq": 7, "to_seq": 9}),
            json!({"from_seq": 10, "to_seq": 12}),
            json!({"from_seq": 13, "to_seq": 15}),
            json!({"recurate": true, "from_seq": 8, "to_seq": 11}),
            json!({"recurate": true, "from_seq": 20, "to_seq": 20, "covers": {"from_seq": 13, "to_seq": 13}}),
            json!({"from_seq": 21, "to_seq": 24}),
            json!({"from_seq": 25, "to_seq": 28}),
            json!({"recurate": true, "from_seq": 22, "to_seq": 23, "covers": {"from_seq": 22, "to_seq": 26}}),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, b)| (i as i64 + 1, b))
        .collect();
        assert_eq!(units(&ops), [(1, 6), (7, 12), (13, 15), (20, 20), (21, 28)]);
    }

    /// A31, A55: a curated, summarized window past the period expires whole; its text leaves
    /// raw.db, its WAL, a copy set aside and the backups, and its labels stay.
    #[test]
    fn a_finished_window_past_the_period_expires_whole_and_its_text_leaves_every_file() {
        let home = tempfile::tempdir().unwrap();
        let (a, b) = finished(home.path());
        crate::backup::export(home.path()).unwrap();
        let copy = home.path().join("raw.db.quarantined-1");
        std::fs::copy(home.path().join("raw.db"), &copy).unwrap();
        let aside = home.path().join("knowledge.db.quarantined-1");
        std::fs::copy(home.path().join("knowledge.db"), &aside).unwrap();
        assert!(!holding(home.path()).is_empty());
        // The full-text index keeps a deleted row's trigrams in its segments until they merge:
        // `kvw` is one of the canary's that no other record has.
        let trigram = |home: &Path| -> bool {
            let k = crate::knowledge::open(home).unwrap();
            let blocks: Vec<Vec<u8>> = k
                .prepare("SELECT block FROM raw_fts_data WHERE block IS NOT NULL")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            blocks.iter().any(|b| b.windows(3).any(|w| w == b"kvw"))
        };
        assert!(trigram(home.path()));
        period(home.path(), None);
        assert_eq!(pass(home.path()).unwrap().unwrap(), Pass::default());
        period(home.path(), Some(1));
        let done = pass(home.path()).unwrap().unwrap();
        assert_eq!(
            done,
            Pass {
                expired: 2,
                waiting: 0,
                owed: false
            }
        );
        assert!(expired(home.path(), a) && expired(home.path(), b));
        assert_eq!(holding(home.path()), Vec::<String>::new());
        // Step 5 built knowledge.db again: no trigram of the text, and no copy set aside.
        assert!(!trigram(home.path()));
        assert!(!aside.exists());
        // No text row is left to search; the labels' rows stay.
        let indexed = |home: &Path| -> (i64, i64) {
            let k = crate::knowledge::open(home).unwrap();
            let count = |sql: &str| k.query_row(sql, [], |r| r.get(0)).unwrap();
            (
                count("SELECT COUNT(*) FROM raw_fts"),
                count("SELECT COUNT(*) FROM raw_docs"),
            )
        };
        assert_eq!(indexed(home.path()), (0, 2));
        crate::worker::rebuild(home.path()).unwrap();
        assert_eq!(indexed(home.path()), (0, 2));
        let raw = crate::raw::open(home.path()).unwrap();
        assert_eq!(
            raw.session_key(raw.device(), a).unwrap().unwrap(),
            "claude\0s"
        );
        assert!(raw.expiry_owed().unwrap().is_empty());
        assert!(!home.path().join("state").join(OWED).exists());
        // Nothing is left to do, and doctor says what was done.
        drop(raw);
        assert_eq!(pass(home.path()).unwrap().unwrap(), Pass::default());
        let raw = crate::raw::open(home.path()).unwrap();
        let (lines, wrong) = doctor_lines(home.path(), Some(&raw)).unwrap();
        assert!(
            !wrong && lines[0].starts_with("retention: 1 day ("),
            "{lines:?}"
        );
        assert!(lines[0].contains("2 records expired"), "{lines:?}");
    }

    /// A31, A55: each rule that keeps a window keeps all of it, and doctor counts what waits.
    #[test]
    fn a_window_waiting_on_any_rule_stays_whole() {
        type Change = fn(&Path, i64, i64);
        let cases: [(&str, Change, usize); 11] = [
            (
                "a mask owed",
                |home, _, _| {
                    let mut raw = crate::raw::open(home).unwrap();
                    let device = raw.device().to_owned();
                    let other = said(&mut raw, "t", "prompt", 3_000, "Key 1234567890.");
                    let target = Target::Range {
                        device,
                        seq: other,
                        offset: 0,
                        length: 3,
                    };
                    raw.append_tombstone(target).unwrap();
                },
                2,
            ),
            (
                "a record above the checkpoint",
                |home, _, b| {
                    let mut raw = crate::raw::open(home).unwrap();
                    said(&mut raw, "s", "prompt", 3_000, "Later.");
                    let _ = b;
                },
                2,
            ),
            (
                "a skipped window",
                |home, a, b| {
                    let mut raw = crate::raw::open(home).unwrap();
                    window(
                        &mut raw,
                        a,
                        b,
                        json!({"outcome": "skipped", "reason": "parked"}),
                    );
                },
                2,
            ),
            (
                "an excluded session's record",
                |home, a, b| {
                    let mut raw = crate::raw::open(home).unwrap();
                    window(&mut raw, a, b, json!({"recurate": true, "excluded": [a]}));
                },
                2,
            ),
            (
                "a queued span",
                |home, a, b| {
                    let k = crate::knowledge::open(home).unwrap();
                    let device = crate::raw::open(home).unwrap().device().to_owned();
                    k.execute(
                        "INSERT INTO recurate(device, from_seq, to_seq, op_device, op_seq)
                     VALUES(?1, ?2, ?3, ?1, 99)",
                        rusqlite::params![device, a, b],
                    )
                    .unwrap();
                },
                2,
            ),
            (
                "a turn not asked for",
                |home, _, _| {
                    let k = crate::knowledge::open(home).unwrap();
                    k.execute("DELETE FROM turns", []).unwrap();
                },
                2,
            ),
            (
                "a turn kept back",
                |home, _, _| {
                    let k = crate::knowledge::open(home).unwrap();
                    k.execute("UPDATE turns SET excluded = 1, skipped = 1", [])
                        .unwrap();
                },
                2,
            ),
            (
                "a newest record inside the period",
                |home, _, b| {
                    let raw = crate::raw::open(home).unwrap();
                    let device = raw.device().to_owned();
                    drop(raw);
                    rusqlite::Connection::open(home.join("raw.db"))
                        .unwrap()
                        .execute(
                            "UPDATE records SET ts = ?1 WHERE device = ?2 AND seq = ?3",
                            rusqlite::params![crate::db::now_ms(), device, b],
                        )
                        .unwrap();
                },
                0,
            ),
            (
                "a consumer behind",
                |home, _, _| {
                    let k = crate::knowledge::open(home).unwrap();
                    k.execute("UPDATE checkpoints SET seq = 0 WHERE consumer = 'fts'", [])
                        .unwrap();
                },
                2,
            ),
            (
                "a claim's quote that does not read",
                |home, a, _| {
                    let mut raw = crate::raw::open(home).unwrap();
                    let device = raw.device().to_owned();
                    let op = json!({"id": "c", "kind": "decision", "status": "decided",
                    "speaker": "user", "scope": "repo", "body": "Build it.",
                    "evidence": [{"device": device, "seq": a, "offset": 0, "length": 5,
                        "sentence": 0, "quote": "Other"}],
                    "supersedes": [], "recipe": "t", "tier": 1, "why": "", "tainted": false,
                    "support": []});
                    raw.append_ops(&[(OpKind::Claim, op)]).unwrap();
                },
                2,
            ),
            (
                "a forget registered",
                |home, a, _| {
                    let mut raw = crate::raw::open(home).unwrap();
                    let device = raw.device().to_owned();
                    let op = json!({"id": "c", "kind": "decision", "status": "decided",
                    "speaker": "user", "scope": "repo", "body": "Build it.",
                    "evidence": [{"device": device, "seq": a, "offset": 0, "length": 5,
                        "sentence": 0, "quote": "Build"}],
                    "supersedes": [], "recipe": "t", "tier": 1, "why": "", "tainted": false,
                    "support": []});
                    raw.append_ops(&[(OpKind::Claim, op)]).unwrap();
                    raw.append_tombstone(Target::Record { device, seq: a })
                        .unwrap();
                },
                2,
            ),
        ];
        for (name, change, waiting) in cases {
            let home = tempfile::tempdir().unwrap();
            let (a, b) = finished(home.path());
            change(home.path(), a, b);
            period(home.path(), Some(1));
            let done = pass(home.path()).unwrap().unwrap();
            assert_eq!((done.expired, done.waiting), (0, waiting), "{name}");
            assert!(!expired(home.path(), b), "{name}");
        }
    }

    /// A directive, the owner's own words and a preference's only evidence, never expires; the
    /// rest of its window expires around it.
    #[test]
    fn a_directive_stays_and_the_rest_of_its_window_expires() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let a = said(&mut raw, "s", "prompt", 1_000, "Build it.");
        let d = raw
            .append(&Event {
                kind: "directive".into(),
                ts: 1_500,
                ..crate::raw::test_event(&json!({ "text": "Use tabs." }).to_string())
            })
            .unwrap();
        let b = said(&mut raw, "s", "reply", 2_000, "Built.");
        window(&mut raw, a, b, json!({}));
        asked(&mut raw, "s", a, b, false);
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        period(home.path(), Some(1));
        assert_eq!(pass(home.path()).unwrap().unwrap().expired, 2);
        assert!(expired(home.path(), a) && expired(home.path(), b));
        assert!(!expired(home.path(), d));
    }

    /// Rule 6: a session's windows expire in order, so a later one waits behind an earlier one
    /// that waits; another session's window does not.
    #[test]
    fn a_sessions_windows_expire_in_order() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let turn = |raw: &mut Raw, session: &str, ts: i64| {
            let a = said(raw, session, "prompt", ts, "Do it.");
            let b = said(raw, session, "reply", ts + 1, "Done.");
            window(raw, a, b, json!({}));
            asked(raw, session, a, b, false);
            (a, b)
        };
        let first = turn(&mut raw, "s", 1_000);
        let second = turn(&mut raw, "s", 2_000);
        let other = turn(&mut raw, "t", 3_000);
        let device = raw.device().to_owned();
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        crate::knowledge::open(home.path())
            .unwrap()
            .execute(
                "INSERT INTO recurate(device, from_seq, to_seq, op_device, op_seq)
                 VALUES(?1, ?2, ?3, ?1, 99)",
                rusqlite::params![device, first.0, first.1],
            )
            .unwrap();
        period(home.path(), Some(1));
        let done = pass(home.path()).unwrap().unwrap();
        assert_eq!((done.expired, done.waiting), (2, 4));
        assert!(!expired(home.path(), first.0) && !expired(home.path(), second.0));
        assert!(expired(home.path(), other.0) && expired(home.path(), other.1));
    }

    /// A55: imported records a window parked wait, with no turn of their own to wait for; once a
    /// recuration curated them, they expire.
    #[test]
    fn imported_records_expire_once_curated_again() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let imported = |raw: &mut Raw, kind: &str, ts: i64| {
            raw.append(&Event {
                kind: kind.into(),
                session: "old".into(),
                source: "transcript".into(),
                ts,
                ..crate::raw::test_event(&json!({ "prompt": "Old words." }).to_string())
            })
            .unwrap()
        };
        let a = imported(&mut raw, "prompt", 1_000);
        let b = imported(&mut raw, "reply", 2_000);
        window(
            &mut raw,
            a,
            b,
            json!({"outcome": "skipped", "reason": "parked"}),
        );
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        period(home.path(), Some(1));
        let done = pass(home.path()).unwrap().unwrap();
        assert_eq!((done.expired, done.waiting), (0, 2));
        let mut raw = crate::raw::open(home.path()).unwrap();
        window(&mut raw, a, b, json!({"recurate": true}));
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        std::fs::remove_file(home.path().join("state").join(LOOK)).unwrap();
        assert_eq!(pass(home.path()).unwrap().unwrap().expired, 2);
    }

    /// Step 1's boundary: a session that took a record after the pass read it begins nothing,
    /// and one that did not is owed, stamped with the format that reads an expired record.
    #[test]
    fn a_batch_begins_only_on_the_sessions_as_read() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let a = said(&mut raw, "s", "prompt", 1_000, "Build it.");
        let session = (Some("claude".to_owned()), Some("s".to_owned()));
        let b = said(&mut raw, "s", "reply", 2_000, "Built.");
        assert!(
            !raw.begin_expiry(&[(a, 1_000)], &[(session.0.clone(), session.1.clone(), a)])
                .unwrap()
        );
        assert!(raw.expiry_owed().unwrap().is_empty());
        assert!(
            raw.begin_expiry(&[(a, 1_000), (b, 2_000)], &[(session.0, session.1, b)])
                .unwrap()
        );
        assert_eq!(raw.expiry_owed().unwrap().len(), 2);
        let format: i64 = rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(format, crate::raw::EXPIRED_FORMAT);
    }

    /// A pass stopped after any step is finished by the next, the setting removed meanwhile: a
    /// batch begun is finished whatever the setting says.
    #[test]
    fn a_pass_stopped_after_each_step_is_finished_by_the_next() {
        for stage in [
            "expiry_committed",
            "expiry_begun",
            "expiry_backups",
            "expiry_rows",
            "expiry_wal",
            "expiry_knowledge",
        ] {
            let home = tempfile::tempdir().unwrap();
            let (a, b) = finished(home.path());
            crate::backup::export(home.path()).unwrap();
            period(home.path(), Some(1));
            crate::backup::FAIL_AFTER.set(Some(stage));
            assert!(pass(home.path()).is_err(), "{stage}");
            period(home.path(), None);
            let raw = crate::raw::open(home.path()).unwrap();
            assert!(due(home.path(), &raw).unwrap(), "{stage}");
            drop(raw);
            pass(home.path()).unwrap().unwrap();
            assert!(
                expired(home.path(), a) && expired(home.path(), b),
                "{stage}"
            );
            assert_eq!(holding(home.path()), Vec::<String>::new(), "{stage}");
            let raw = crate::raw::open(home.path()).unwrap();
            assert!(raw.expiry_owed().unwrap().is_empty(), "{stage}");
            assert!(!due(home.path(), &raw).unwrap(), "{stage}");
            assert!(!home.path().join("state").join(OWED).exists(), "{stage}");
        }
    }

    /// Step 4: a reader that keeps raw.db-wal from truncating leaves the batch owed, and the next
    /// look finishes it.
    #[test]
    fn a_reader_holding_the_wal_leaves_the_batch_owed() {
        let home = tempfile::tempdir().unwrap();
        let (a, _) = finished(home.path());
        period(home.path(), Some(1));
        let reader = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
        reader
            .execute_batch("BEGIN; SELECT COUNT(*) FROM records;")
            .unwrap();
        let done = pass(home.path()).unwrap().unwrap();
        assert_eq!((done.expired, done.owed), (2, true));
        reader.execute_batch("COMMIT").unwrap();
        drop(reader);
        assert!(expired(home.path(), a));
        assert_eq!(pass(home.path()).unwrap().unwrap(), Pass::default());
        let raw = crate::raw::open(home.path()).unwrap();
        assert!(raw.expiry_owed().unwrap().is_empty());
    }

    /// A restore while a batch is owed and raw.db is lost applies `state/expiry-owed` to the raw.db
    /// it rebuilds from the segments, which hold the text yet, and leaves none of the text in the
    /// file's free space. The pass sealed the curation ops before it began (here the backups held
    /// none of them), so they come back with the records. The next pass finishes the batch on it.
    #[test]
    fn a_restore_during_a_batch_finishes_it_on_the_rebuilt_store() {
        let home = tempfile::tempdir().unwrap();
        // A body long and varied enough to be stored compressed, over overflow pages.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let noise: String = (0..12_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                char::from(b"0123456789abcdef"[(x & 15) as usize])
            })
            .collect();
        let body = json!({ "prompt": format!("{CANARY} {noise}") }).to_string();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let a = raw
            .append(&Event {
                session: "s".into(),
                repo: Some("github.com/o/r".into()),
                ts: 1_000,
                ..crate::raw::test_event(&body)
            })
            .unwrap();
        let b = said(&mut raw, "s", "reply", 2_000, "Built.");
        window(&mut raw, a, b, json!({}));
        asked(&mut raw, "s", a, b, false);
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        // The worker stopped before its backup of the ops.
        let dir = crate::backup::dir(home.path()).unwrap();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            if crate::backup::segment_name(&entry.file_name()).is_some_and(|(ops, ..)| ops) {
                std::fs::remove_file(entry.path()).unwrap();
            }
        }
        period(home.path(), Some(1));
        crate::backup::FAIL_AFTER.set(Some("expiry_begun"));
        assert!(pass(home.path()).is_err());
        period(home.path(), None);
        // Only the file knows the batch.
        for name in ["raw.db", "raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.path().join(name));
        }
        crate::backup::restore(home.path()).unwrap();
        assert!(expired(home.path(), a) && expired(home.path(), b));
        let raw = crate::raw::open(home.path()).unwrap();
        assert_eq!(raw.window_bodies().unwrap().len(), 1);
        drop(raw);
        // The compressed body the rebuild stored before it expired the row is not in the file.
        let stored = zstd::bulk::compress(body.as_bytes(), 3).unwrap();
        let chunk = &stored[stored.len() / 2..stored.len() / 2 + 64];
        let file = std::fs::read(home.path().join("raw.db")).unwrap();
        assert!(!file.windows(64).any(|w| w == chunk));
        pass(home.path()).unwrap().unwrap();
        assert_eq!(holding(home.path()), Vec::<String>::new());
        assert!(!home.path().join("state").join(OWED).exists());
    }

    /// A stop between step 1's commit and its file leaves the batch in raw.db alone: a restore then
    /// reads it from the raw.db it replaces, and finishes it on the rebuilt store (Codex on #552).
    #[test]
    fn a_restore_before_the_batch_has_its_file_reads_it_from_raw_db() {
        let home = tempfile::tempdir().unwrap();
        let (a, b) = finished(home.path());
        let owed = || {
            crate::raw::open(home.path())
                .unwrap()
                .expiry_owed()
                .unwrap()
                .len()
        };
        crate::backup::export(home.path()).unwrap();
        period(home.path(), Some(1));
        crate::backup::FAIL_AFTER.set(Some("expiry_committed"));
        assert!(pass(home.path()).is_err());
        assert!(!home.path().join("state").join(OWED).exists());
        assert_eq!(owed(), 2);
        period(home.path(), None);
        crate::backup::restore(home.path()).unwrap();
        assert!(expired(home.path(), a) && expired(home.path(), b));
        assert_eq!(owed(), 2);
        pass(home.path()).unwrap().unwrap();
        assert_eq!(holding(home.path()), Vec::<String>::new());
        assert!(!home.path().join("state").join(OWED).exists());
    }

    /// A copy set aside that a reader holds keeps its WAL from truncating: it is removed rather
    /// than left with the text. Unix only, as forget's own: Windows does not remove a file open.
    #[cfg(unix)]
    #[test]
    fn a_copy_a_reader_holds_is_removed_not_left_with_the_text() {
        let home = tempfile::tempdir().unwrap();
        finished(home.path());
        let copy = home.path().join("raw.db.quarantined-1");
        std::fs::copy(home.path().join("raw.db"), &copy).unwrap();
        let wal = home.path().join("raw.db-wal");
        if wal.exists() {
            std::fs::copy(&wal, home.path().join("raw.db.quarantined-1-wal")).unwrap();
        }
        let reader = rusqlite::Connection::open(&copy).unwrap();
        reader
            .execute_batch("BEGIN; SELECT COUNT(*) FROM records;")
            .unwrap();
        period(home.path(), Some(1));
        assert_eq!(pass(home.path()).unwrap().unwrap().expired, 2);
        assert!(!copy.exists());
        drop(reader);
        assert_eq!(holding(home.path()), Vec::<String>::new());
    }

    /// doctor: forever with no table, and a wrong table as a failure that expires nothing.
    #[test]
    fn doctor_says_forever_and_a_wrong_table_fails() {
        let home = tempfile::tempdir().unwrap();
        finished(home.path());
        let raw = crate::raw::open(home.path()).unwrap();
        period(home.path(), None);
        assert_eq!(
            doctor_lines(home.path(), Some(&raw)).unwrap(),
            (vec!["retention: forever".to_owned()], false)
        );
        std::fs::write(home.path().join("config.toml"), "[retention]\ndays = 0\n").unwrap();
        let (lines, wrong) = doctor_lines(home.path(), Some(&raw)).unwrap();
        assert!(wrong && lines[0].contains("nothing expires"), "{lines:?}");
        drop(raw);
        assert_eq!(pass(home.path()).unwrap().unwrap(), Pass::default());
    }

    /// Step 3 waits while a mask is owed (masks come first), and the batch is finished once the
    /// masks' purge is paid.
    #[test]
    fn an_owed_batch_waits_for_a_mask_before_its_rows() {
        let home = tempfile::tempdir().unwrap();
        let (a, _) = finished(home.path());
        period(home.path(), Some(1));
        crate::backup::FAIL_AFTER.set(Some("expiry_begun"));
        assert!(pass(home.path()).is_err());
        let mut raw = crate::raw::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        let other = said(&mut raw, "t", "prompt", 3_000, "Key 1234567890.");
        raw.append_tombstone(Target::Range {
            device,
            seq: other,
            offset: 0,
            length: 3,
        })
        .unwrap();
        drop(raw);
        let done = pass(home.path()).unwrap().unwrap();
        assert_eq!((done.expired, done.owed), (0, true));
        assert!(!expired(home.path(), a));
        let raw = crate::raw::open(home.path()).unwrap();
        raw.masks_paid(&raw.masks_owed().unwrap().unwrap()).unwrap();
        drop(raw);
        assert_eq!(pass(home.path()).unwrap().unwrap().expired, 2);
        assert!(expired(home.path(), a));
    }

    /// The worker's look: `forget --continue` runs a pass after the purge, and is started while a
    /// batch is owed or a look is due.
    #[test]
    fn forget_continue_runs_the_pass_the_worker_starts_it_for() {
        let home = tempfile::tempdir().unwrap();
        let (a, _) = finished(home.path());
        period(home.path(), Some(1));
        let raw = crate::raw::open(home.path()).unwrap();
        // The worker's run looked a moment ago.
        std::fs::remove_file(home.path().join("state").join("forget-continue")).unwrap();
        crate::forget::CONTINUED.set(0);
        crate::forget::continue_due(home.path(), &raw);
        assert_eq!(crate::forget::CONTINUED.get(), 1);
        drop(raw);
        crate::forget::run(
            home.path(),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            true,
        )
        .unwrap();
        assert!(expired(home.path(), a));
        let raw = crate::raw::open(home.path()).unwrap();
        assert!(!due(home.path(), &raw).unwrap());
    }

    /// Step 5: an expired record's vector, a cached vector no other key uses, and the facts its
    /// text gave go; the facts its labels give stay.
    #[test]
    fn step_5_takes_the_vectors_and_the_facts_of_the_text() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let a = raw
            .append(&Event {
                session: "s".into(),
                repo: Some("github.com/o/r".into()),
                branch: Some("main".into()),
                ts: 1_000,
                ..crate::raw::test_event(&json!({ "prompt": "Always use tabs." }).to_string())
            })
            .unwrap();
        let b = said(&mut raw, "s", "reply", 2_000, "Built.");
        window(&mut raw, a, b, json!({}));
        asked(&mut raw, "s", a, b, false);
        let key = format!("{}:{a}", raw.device());
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        let facts = |k: &Connection| -> Vec<String> {
            k.prepare("SELECT fact FROM manifest_facts WHERE seq = ?1 ORDER BY fact")
                .unwrap()
                .query_map([a], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let k = crate::knowledge::open(home.path()).unwrap();
        assert!(facts(&k).contains(&"owner".to_owned()), "{:?}", facts(&k));
        k.execute(
            "INSERT INTO vectors(embedder, src_sha, vec) VALUES('e', 'x', X'00')",
            [],
        )
        .unwrap();
        k.execute(
            "INSERT INTO vector_keys(embedder, kind, key, src_sha) VALUES('e', 'r', ?1, 'x')",
            [&key],
        )
        .unwrap();
        drop(k);
        period(home.path(), Some(1));
        assert_eq!(pass(home.path()).unwrap().unwrap().expired, 2);
        let k = crate::knowledge::open(home.path()).unwrap();
        assert_eq!(facts(&k), ["event", "prompt"]);
        let count = |sql: &str| -> i64 { k.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM vectors"), 0);
        assert_eq!(
            count("SELECT COUNT(*) FROM vector_keys WHERE src_sha IS NOT NULL"),
            0
        );
    }

    /// A record that took an owed record's seq (a restore from older segments can give the seq
    /// to another record) is another record: its row is left, and the batch finishes.
    #[test]
    fn an_owed_seq_another_record_took_is_left() {
        let home = tempfile::tempdir().unwrap();
        let (a, b) = finished(home.path());
        period(home.path(), Some(1));
        crate::backup::FAIL_AFTER.set(Some("expiry_begun"));
        assert!(pass(home.path()).is_err());
        let device = crate::raw::open(home.path()).unwrap().device().to_owned();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE records SET ts = 1500 WHERE device = ?1 AND seq = ?2",
                rusqlite::params![device, a],
            )
            .unwrap();
        pass(home.path()).unwrap().unwrap();
        assert!(!expired(home.path(), a) && expired(home.path(), b));
        assert!(
            crate::raw::open(home.path())
                .unwrap()
                .expiry_owed()
                .unwrap()
                .is_empty()
        );
        // Its text row stays to be found (Codex on part 2 of 3e-3).
        let k = crate::knowledge::open(home.path()).unwrap();
        let indexed: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM raw_fts WHERE rowid = (SELECT rowid FROM raw_docs WHERE seq = ?1)",
                [a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
    }

    /// The look is daily, however often `forget --continue` runs for a forget or a mask: a pass
    /// within the day begins nothing, though a window became eligible (Codex on #552).
    #[test]
    fn a_new_batch_waits_a_day_after_the_last_look() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let now = crate::db::now_ms();
        let a = said(&mut raw, "s", "prompt", now, "Build it.");
        let b = said(&mut raw, "s", "reply", now + 1, "Built.");
        window(&mut raw, a, b, json!({}));
        asked(&mut raw, "s", a, b, false);
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        period(home.path(), Some(1));
        assert_eq!(pass(home.path()).unwrap().unwrap(), Pass::default());
        // A day later by the records' time, within the day by the look's.
        let device = crate::raw::open(home.path()).unwrap().device().to_owned();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute(
                "UPDATE records SET ts = ts - 2 * 86400000 WHERE device = ?1",
                [&device],
            )
            .unwrap();
        assert_eq!(pass(home.path()).unwrap().unwrap(), Pass::default());
        assert!(!expired(home.path(), a));
        std::fs::remove_file(home.path().join("state").join(LOOK)).unwrap();
        assert_eq!(pass(home.path()).unwrap().unwrap().expired, 2);
    }

    /// Segments a change of backup location left behind (the settings page moves none) are
    /// rewritten where they are, as the default directory's are (Codex on #552).
    #[test]
    fn segments_left_in_an_earlier_backup_location_are_rewritten() {
        let home = tempfile::tempdir().unwrap();
        let (a, _) = finished(home.path());
        // A location may hold a newline where the system allows one (Codex on #552).
        let old = if cfg!(unix) { "old\\nline" } else { "old" };
        std::fs::write(
            home.path().join("config.toml"),
            format!("[backup]\ndir = \"{old}\"\n"),
        )
        .unwrap();
        // The worker's own backup, as on any run.
        crate::worker::run_once(home.path()).unwrap();
        assert!(holding(home.path()).iter().any(|f| f.ends_with(".seg.zst")));
        std::fs::write(
            home.path().join("config.toml"),
            "[backup]\ndir = \"new\"\n[retention]\ndays = 1\n",
        )
        .unwrap();
        assert_eq!(pass(home.path()).unwrap().unwrap().expired, 2);
        assert!(expired(home.path(), a));
        assert_eq!(holding(home.path()), Vec::<String>::new());
    }
}
