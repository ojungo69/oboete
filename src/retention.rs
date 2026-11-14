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

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// How often the worker looks for records past the period, once a week (decision 49: each batch
/// builds knowledge.db again); an owed batch is looked for at every `forget::continue_due`
/// (docs/retention.md).
const LOOK_EVERY_MS: i64 = 7 * DAY_MS;
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

/// Whether a pass is due: a batch is owed, or a period is set and the last look is a week old.
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

/// Whether the last look (`state/retention-look`) is less than a week old.
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
        // A new batch at most once a week, however often `forget --continue` runs for a forget or
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
mod tests;
