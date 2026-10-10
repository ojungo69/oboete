//! Milestone 5 slice 1 (docs/milestone-5-plan.md, D1): a bodyless request to forget imported raw
//! records. raw.db is the one authority: one raw transaction writes the deny rows, the tombstones
//! and the job row. Two bodyless request logs, in the home and beside the backups, are its
//! redundancy: reconciled in both directions, never compared, and never a reason to refuse an open.
//! Physical purge is a later slice.
//!
//! Slice 2 (D5) adds a claim's or an imported document's uid as a target: one raw transaction
//! appends a bodyless `forget` op naming it and the job row, and from that commit every reader,
//! sender and writer of claims and documents passes it over (`raw::Raw::forgotten`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{BufRead, IsTerminal, Read, Seek, Write};
use std::path::{Path, PathBuf};

/// One request's records; a larger selection is several requests (rule 9).
pub const MAX_RECORDS: usize = 500;
/// One log line's bytes at most (rule 9).
const MAX_LINE: usize = 256 << 10;
/// What a log's name is, in the home and in the backup directory (rule 3).
const LOG: &str = "forget.log";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Record {
        device: String,
        seq: i64,
    },
    Span {
        device: String,
        from: i64,
        to: i64,
    },
    /// D5: a claim's uid, or an imported document's.
    Uid {
        uid: String,
    },
}

impl Target {
    pub fn bounds(&self) -> Result<(&str, i64, i64)> {
        let (device, from, to) = match self {
            Self::Record { device, seq } => (device.as_str(), *seq, *seq),
            Self::Span { device, from, to } => (device.as_str(), *from, *to),
            Self::Uid { .. } => anyhow::bail!("a uid names no record span"),
        };
        anyhow::ensure!(
            !device.is_empty()
                && device.len() <= 128
                && device.bytes().all(|b| b.is_ascii_alphanumeric()),
            "invalid device identity"
        );
        anyhow::ensure!(
            from > 0 && to >= from && to < i64::MAX,
            "invalid record span"
        );
        Ok((device, from, to))
    }

    pub fn parse(text: &str, span: bool) -> Result<Self> {
        let (device, range) = text
            .split_once(':')
            .context("expected <device>:<seq> or <device>:<from>-<to>")?;
        let target = if span {
            let (from, to) = range
                .split_once('-')
                .context("expected <device>:<from>-<to>")?;
            Self::Span {
                device: device.into(),
                from: from.parse()?,
                to: to.parse()?,
            }
        } else {
            Self::Record {
                device: device.into(),
                seq: range.parse()?,
            }
        };
        target.bounds()?;
        Ok(target)
    }

    /// D5: a claim's uid or an imported document's, as `--uid` takes it. A record's id from
    /// search names a record, which `--record` forgets; a card's or a summary's goes with the
    /// records it was made from (slice 3).
    pub fn parse_uid(text: &str) -> Result<Self> {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if let Some((device, seq)) = text.split_once(':')
            && !device.is_empty()
            && device.bytes().all(|b| b.is_ascii_alphanumeric())
            && digits(seq)
        {
            anyhow::bail!("{text} is a raw record's id: forget it with --record");
        }
        anyhow::ensure!(
            crate::cards::id_parts(text, "").is_none()
                && crate::turns::id_parts(text, "").is_none(),
            "{text} is a card's or a session summary's id: they go with the records they were \
             made from (forget those with --record or --span)"
        );
        well_formed(text)?;
        Ok(Self::Uid { uid: text.into() })
    }
}

/// A uid as a request carries it (D5): a claim's (64 hex digits) or an imported document's
/// (`<source>:<key>`), printable ASCII with no space, bounded.
pub(crate) fn well_formed(uid: &str) -> Result<()> {
    anyhow::ensure!(
        (1..=256).contains(&uid.len()) && uid.bytes().all(|b| b.is_ascii_graphic()),
        "invalid uid"
    );
    Ok(())
}

/// What forgetting a uid takes (D5), read from knowledge.db, the index of its ops.
#[derive(Debug, Clone, PartialEq)]
pub struct UidPreview {
    /// `claim` or `document`.
    pub kind: &'static str,
    /// Its ops in raw: a claim's derivations and corrections, a document's import ops.
    pub ops: usize,
    /// Its vectors in knowledge.db.
    pub vectors: usize,
    /// The first line of each claim it supersedes: with it gone, none of them is superseded by
    /// it any more, so each is current again unless another claim supersedes it.
    pub again: Vec<String>,
}

/// One record a request forgets, bodyless (rule 3): where it was, its import origin (the one
/// identity, rule 5), its session's hash, and its time only when it counted toward the transcript
/// cut (rule 10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub device: String,
    pub seq: i64,
    pub origin: String,
    pub session: String,
    pub ts: Option<i64>,
}

/// What forget would register, shown before it is confirmed.
#[derive(Debug, Clone)]
pub struct Preview {
    pub(crate) target: Target,
    pub(crate) records: Vec<Record>,
    /// How many deny rows raw held when it was read: the fence's count (rule 12).
    pub(crate) denied: i64,
    /// The sources of the records, for the limits forget prints.
    pub(crate) sources: Vec<String>,
    pub sample: Option<String>,
    /// A uid target's (D5).
    pub uid: Option<UidPreview>,
}

impl Preview {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Target::Uid { uid } = &self.target {
            well_formed(uid)?;
            anyhow::ensure!(
                self.records.is_empty() && self.uid.is_some(),
                "forget selection differs from its preview"
            );
            return Ok(());
        }
        let (device, from, to) = self.target.bounds()?;
        anyhow::ensure!(
            !self.records.is_empty(),
            "no raw records matched; nothing was registered"
        );
        anyhow::ensure!(
            self.records.len() <= MAX_RECORDS,
            "split this selection into spans of at most {MAX_RECORDS} records"
        );
        for r in &self.records {
            anyhow::ensure!(
                r.device == device && (from..=to).contains(&r.seq),
                "forget selection differs from its preview"
            );
            check_identity(&r.origin)?;
            check_identity(&r.session)?;
        }
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// The same records, under the same deny-list: a start that reads another is stale.
    pub(crate) fn token(&self) -> Result<String> {
        Ok(hash(&serde_json::to_vec(&(
            &self.target,
            &self.records,
            self.denied,
        ))?))
    }
}

/// A registered request as raw's job row and each log line hold it (rules 3, 4): bodyless, with
/// the store's lineage, stable across a copied raw.db (a line of another lineage is not applied).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub v: u32,
    pub home: String,
    pub job: String,
    pub started: i64,
    pub target: Target,
    pub records: Vec<Record>,
}

/// A transcript request accepted before mixed native provenance made registration unsafe.
/// These fixtures must still exercise replay, dispatch and worker recovery with native data.
#[cfg(test)]
pub(crate) fn previous_transcript_request(
    raw: &crate::raw::Raw,
    seq: i64,
    identity: &crate::raw::ImportIdentity,
) -> Request {
    Request {
        v: 1,
        home: raw.home_id().into(),
        job: "f1000000000000000000000000000001".into(),
        started: 1_000,
        target: Target::Record {
            device: raw.device().into(),
            seq,
        },
        records: vec![Record {
            device: raw.device().into(),
            seq,
            origin: identity.origin.clone(),
            session: identity.session.clone(),
            ts: None,
        }],
    }
}

impl Request {
    pub(crate) fn check(&self) -> Result<()> {
        anyhow::ensure!(
            self.job.len() == 32 && self.job.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid job id"
        );
        // Version 2 is a uid's (D5); an older binary skips its line as of an unknown version.
        match (&self.target, self.v) {
            (Target::Uid { uid }, 2) => {
                well_formed(uid)?;
                anyhow::ensure!(self.records.is_empty(), "invalid record count");
                return Ok(());
            }
            (Target::Record { .. } | Target::Span { .. }, 1) => {}
            _ => anyhow::bail!("unknown request version {}", self.v),
        }
        let (device, from, to) = self.target.bounds()?;
        anyhow::ensure!(
            !self.records.is_empty() && self.records.len() <= MAX_RECORDS,
            "invalid record count"
        );
        for r in &self.records {
            anyhow::ensure!(
                r.device == device && (from..=to).contains(&r.seq),
                "a record outside its target"
            );
            check_identity(&r.origin)?;
            check_identity(&r.session)?;
        }
        Ok(())
    }

    /// Its log line: the request and a checksum of it, so a damaged line is skipped alone.
    fn line(&self) -> Result<String> {
        let request = serde_json::to_string(self)?;
        let line = format!(
            "{{\"sum\":\"{}\",\"request\":{request}}}",
            hash(request.as_bytes())
        );
        anyhow::ensure!(line.len() <= MAX_LINE, "a request line over its cap");
        Ok(line)
    }

    /// A line as `line` writes it: the checksum is of the request as it is written again, which
    /// is the text it was written from.
    fn parse(line: &str) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Line {
            sum: String,
            request: Request,
        }
        anyhow::ensure!(line.len() <= MAX_LINE, "a line over its cap");
        let l: Line = serde_json::from_str(line).context("not a request line")?;
        anyhow::ensure!(
            l.sum == hash(serde_json::to_string(&l.request)?.as_bytes()),
            "a line whose checksum does not match"
        );
        l.request.check()?;
        Ok(l.request)
    }
}

/// A job as `forget --status` shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub job: String,
    pub target: Target,
    pub records: usize,
    pub started: i64,
    pub local: &'static str,
}

pub fn preview(home: &Path, target: Target) -> Result<Preview> {
    anyhow::ensure!(crate::raw::exists(home), "no raw store to forget from");
    let raw = crate::raw::open(home)?;
    // Milestone 5 D5 (#439): every target's sample is read under the read fence, so a forget by
    // another process cannot commit between its checks and its sample (a record's or a span's too:
    // Codex on #446). Dropped before any registration: `start` takes the lock alone.
    let _read = crate::dispatch::reading(home, crate::db::OPEN_WRITE_WAIT)?;
    #[cfg(test)]
    if let Some(between) = SAMPLING.take() {
        between();
    }
    match target {
        Target::Uid { uid } => uid_preview(home, &raw, uid),
        target => raw.forget_preview(target),
    }
}

/// D5: what forgetting `uid` takes. knowledge.db is its index: a claim's derivations and
/// corrections, or a document's import ops; a uid it holds neither of is looked for in the op log,
/// whose ops no consumer may have read yet, and refused when that has none. Nothing is written to
/// either store.
fn uid_preview(home: &Path, raw: &crate::raw::Raw, uid: String) -> Result<Preview> {
    use rusqlite::OptionalExtension;
    well_formed(&uid)?;
    anyhow::ensure!(
        raw.home_id_proven()?,
        "this store has no verified home identity: forget was not registered"
    );
    anyhow::ensure!(!raw.forgotten(&uid)?, "{uid} is forgotten already");
    let path = home.join("knowledge.db");
    // A store with no knowledge.db yet reads as an empty one: the op log decides below.
    let k = if path.exists() {
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?
    } else {
        rusqlite::Connection::open_in_memory()?
    };
    let has = |table: &str| -> Result<bool> {
        Ok(k.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
            [table],
            |r| r.get(0),
        )?)
    };
    let count = |sql: &str| -> Result<usize> {
        Ok(k.query_row(sql, [&uid], |r| r.get::<_, i64>(0))? as usize)
    };
    let line = |s: &str| {
        s.lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>()
    };
    let mut shown = None;
    if uid.len() == 64 && uid.bytes().all(|b| b.is_ascii_hexdigit()) && has("derivations")? {
        let ops = count(
            "SELECT (SELECT COUNT(*) FROM derivations WHERE uid = ?1)
                  + (SELECT COUNT(*) FROM corrections WHERE uid = ?1)",
        )?;
        if ops > 0 {
            // Its active derivation's body, else its newest.
            let body: Option<String> = k
                .query_row(
                    "SELECT body FROM derivations WHERE uid = ?1
                     ORDER BY EXISTS(SELECT 1 FROM claims c WHERE c.op_device = derivations.op_device
                       AND c.op_seq = derivations.op_seq) DESC, op_seq DESC LIMIT 1",
                    [&uid],
                    |r| r.get(0),
                )
                .optional()?;
            let mut again = Vec::new();
            let mut st = k.prepare(
                "SELECT DISTINCT c.uid, d2.body FROM edges e
                 JOIN derivations d ON d.op_device = e.op_device AND d.op_seq = e.op_seq
                 JOIN claims c ON c.uid = e.to_uid
                 JOIN derivations d2 ON d2.op_device = c.op_device AND d2.op_seq = c.op_seq
                 WHERE d.uid = ?1 AND e.type = 'supersedes'",
            )?;
            for row in st.query_map([&uid], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                let (superseded, body): (String, String) = row?;
                // One forgotten already comes back as nothing (Codex's adversarial review).
                if !raw.forgotten(&superseded)? {
                    again.push(line(&body));
                }
            }
            shown = Some((
                UidPreview {
                    kind: "claim",
                    ops,
                    vectors: 0,
                    again,
                },
                body,
            ));
        }
    }
    if shown.is_none() && has("imported")? {
        let ops = count("SELECT COUNT(*) FROM imported WHERE uid = ?1")?;
        if ops > 0 {
            let title: Option<String> = k
                .query_row(
                    "SELECT title FROM imported WHERE uid = ?1 LIMIT 1",
                    [&uid],
                    |r| r.get(0),
                )
                .optional()?;
            shown = Some((
                UidPreview {
                    kind: "document",
                    ops,
                    vectors: 0,
                    again: Vec::new(),
                },
                title,
            ));
        }
    }
    // The op log decides what knowledge.db has not read yet (right after an import, or while the
    // worker lags): the uid itself, or its ops since (Codex on #435).
    match (&mut shown, raw.uid_ops(&uid)?) {
        (Some((preview, _)), Some((_, ops, _))) => preview.ops = ops,
        (None, Some((kind, ops, first))) => {
            let preview = UidPreview {
                kind,
                ops,
                vectors: 0,
                again: Vec::new(),
            };
            shown = Some((preview, Some(first)));
        }
        _ => {}
    }
    let (mut uid_preview, sample) =
        shown.with_context(|| format!("{uid} is neither a claim's nor a document's uid here"))?;
    if has("vector_keys")? {
        uid_preview.vectors = count(
            "SELECT COUNT(*) FROM vector_keys
             WHERE key = ?1 AND kind IN ('c', 'k', 'p') AND skipped IS NULL",
        )?;
    }
    Ok(Preview {
        target: Target::Uid { uid },
        records: Vec::new(),
        denied: raw.denied_count()?,
        sources: Vec::new(),
        sample: sample.map(|s| line(&s)),
        uid: Some(uid_preview),
    })
}

#[cfg(test)]
thread_local! {
    /// Run in a uid's preview between its check and its sample (#439).
    static SAMPLING: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests (#439): a forget of `uid` registering on another thread, until it waits on the fence: a
/// read that holds the fence keeps it waiting, so the uid is not forgotten yet. Join it once the
/// read is done.
#[cfg(test)]
pub(crate) fn registering(home: &Path, uid: &str) -> std::thread::JoinHandle<()> {
    let t = registering_target(home, Target::parse_uid(uid).unwrap());
    assert!(!crate::raw::open(home).unwrap().forgotten(uid).unwrap());
    t
}

/// `registering` for any target.
#[cfg(test)]
fn registering_target(home: &Path, target: Target) -> std::thread::JoinHandle<()> {
    let h = home.to_owned();
    crate::dispatch::blocked(home); // an earlier registration's wait is not this one's
    let t = std::thread::spawn(move || {
        let p = preview(&h, target).unwrap();
        start(&h, &p).unwrap();
    });
    // Until the registration has met the read's fence: a thread that has not reached it yet
    // would pass without one (Greptile on #446).
    let started = std::time::Instant::now();
    while !crate::dispatch::blocked(home) {
        assert!(!t.is_finished(), "the forget did not wait for the read");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!t.is_finished(), "the forget did not wait for the read");
    t
}

/// Registers `preview` in one raw transaction (rule 1), then writes it to both logs, after the
/// commit and outside raw's lock (rule 2). What each log copy got is in the report.
pub fn start(home: &Path, preview: &Preview) -> Result<(Status, Report)> {
    let mut raw = crate::raw::open(home)?;
    let mut id = [0_u8; 16];
    getrandom::fill(&mut id)?;
    let job: String = id.iter().map(|b| format!("{b:02x}")).collect();
    let request = raw.forget_start(preview, &job, crate::db::now_ms())?;
    // It is registered already. A later log failure is reported alongside that true status.
    let report = reconcile(home, &mut raw).unwrap_or_else(|e| Report {
        problems: vec![format!("the request logs were not reconciled: {e:#}")],
        ..Report::default()
    });
    Ok((status_of(&request, PENDING), report))
}

const PENDING: &str = "hidden; physical purge pending";
/// D5 item 7: past its rebuild, knowledge.db still holds the uid.
const STOPPED: &str = "stopped: knowledge.db still holds it after its rebuild";

/// Each job, its local state read from the stores (D5 item 9): a uid job past step 3 whose ops
/// hold no text is purged here, or stopped while knowledge.db still holds its uid.
pub fn status(home: &Path) -> Result<Vec<Status>> {
    if !crate::raw::exists(home) {
        return Ok(Vec::new());
    }
    let raw = crate::raw::open(home)?;
    let mut past = Vec::new();
    for (request, step) in raw.forget_jobs()? {
        let purged = match &request.target {
            Target::Uid { uid } => step >= 3 && !raw.uid_has_ops(uid)?,
            _ => false,
        };
        past.push((request, purged));
    }
    let uids: HashSet<String> = past
        .iter()
        .filter_map(|(r, purged)| match &r.target {
            Target::Uid { uid } if *purged => Some(uid.clone()),
            _ => None,
        })
        .collect();
    let holding = knowledge_holding(home, &uids)?;
    Ok(past
        .iter()
        .map(|(r, purged)| {
            let local = match &r.target {
                Target::Uid { uid } if *purged && holding.contains(uid) => STOPPED,
                _ if *purged => "purged here; backups pending",
                _ => PENDING,
            };
            status_of(r, local)
        })
        .collect())
}

fn status_of(r: &Request, local: &'static str) -> Status {
    Status {
        job: r.job.clone(),
        target: r.target.clone(),
        records: r.records.len(),
        started: r.started,
        local,
    }
}

/// Doctor's lines (D5 item 7): each stopped job. True when one is.
pub(crate) fn doctor_lines(home: &Path) -> Result<(Vec<String>, bool)> {
    let lines: Vec<String> = status(home)?
        .into_iter()
        .filter(|s| s.local == STOPPED)
        .map(|s| {
            format!(
                "forget job {} stopped: knowledge.db still holds its uid after the rebuild (an op \
                 it came from was missed); `oboete forget --status` lists it",
                s.job
            )
        })
        .collect();
    let stopped = !lines.is_empty();
    Ok((lines, stopped))
}

/// How long a purge tries to truncate raw.db-wal past the readers that hold it (D5, 2b).
#[cfg(not(test))]
const TRUNCATE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(test)]
const TRUNCATE_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// What a purge did (D5, 2b).
#[derive(Debug, Default, PartialEq)]
pub struct Purged {
    /// The uid jobs it went on with: not past step 3, or whose ops hold their text again.
    pub jobs: usize,
    pub ops: usize,
    /// Jobs whose step 2 waits: readers kept raw.db-wal from truncating.
    pub unfinished: Vec<String>,
    /// Whether knowledge.db was rebuilt without them.
    pub rebuilt: bool,
    /// The files of knowledge.db copies set aside (by a rebuild that stopped, or quarantined)
    /// removed.
    pub aside: usize,
    /// Sessions whose shown value lost a forgotten uid's entry.
    pub shown: usize,
    /// Jobs whose uid knowledge.db still holds after the rebuild: an op it came from was missed.
    pub stopped: Vec<String>,
}

/// Each uid job a purge goes on with, by state rather than step (D5 item 7): one not past step 3,
/// or whose ops hold text again (a restore can bring the bodies back): its job, uid and step, and
/// whether its ops hold text.
fn due(raw: &crate::raw::Raw) -> Result<Vec<(String, String, i64, bool)>> {
    let mut due = Vec::new();
    for (request, step) in raw.forget_jobs()? {
        if let Target::Uid { uid } = request.target {
            let held = raw.uid_has_ops(&uid)?;
            if held || step < 3 {
                due.push((request.job, uid, step, held));
            }
        }
    }
    Ok(due)
}

/// The `uids` knowledge.db still holds a row of (D5 item 7): a claim, a derivation, a correction,
/// an imported document or a vector key.
fn knowledge_holding(home: &Path, uids: &HashSet<String>) -> Result<HashSet<String>> {
    let path = home.join("knowledge.db");
    let mut holding = HashSet::new();
    if uids.is_empty() || !path.exists() {
        return Ok(holding);
    }
    let k =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tables: HashSet<String> = k
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let reads: Vec<String> = [
        ("claims", "uid = ?1"),
        ("derivations", "uid = ?1"),
        ("corrections", "uid = ?1"),
        ("imported", "uid = ?1"),
        ("vector_keys", "kind IN ('c', 'k', 'p') AND key = ?1"),
    ]
    .into_iter()
    .filter(|(table, _)| tables.contains(*table))
    .map(|(table, row)| format!("EXISTS(SELECT 1 FROM {table} WHERE {row})"))
    .collect();
    if reads.is_empty() {
        return Ok(holding);
    }
    let mut st = k.prepare(&format!("SELECT {}", reads.join(" OR ")))?;
    for uid in uids {
        if st.query_row([uid], |r| r.get(0))? {
            holding.insert(uid.clone());
        }
    }
    Ok(holding)
}

/// The files of knowledge.db copies set aside, sidecars too: a rebuild's that stopped and the
/// quarantined (spec 1.7).
fn aside_files(home: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name().to_str().is_some_and(|n| {
                n.starts_with("knowledge.db.quarantined-")
                    || n.starts_with("knowledge.db.rebuilding-")
            })
        })
        .map(|e| e.path())
        .collect()
}

/// D5 (2b): each uid job purged by the state it finds (item 7). Step 2: its ops that hold its text
/// rewritten without it and raw.db-wal truncated, under the worker lock as `oboete rebuild` takes
/// it. Step 3: knowledge.db rebuilt from raw.db once for every job that it still holds (item 4),
/// the copies of knowledge.db set aside carried from by the same rule and removed (item 5), and
/// the uids taken out of every session's shown value (item 6). A uid knowledge.db holds after the
/// rebuild came from an op the purge did not find: its job is stopped, reported and not done.
/// Only one runs at a time (`state/purge.lock`, not waited for: None when another holds it).
pub fn purge(home: &Path) -> Result<Option<Purged>> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("purge.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let mut purged = Purged::default();
    if !crate::raw::exists(home) {
        return Ok(Some(purged));
    }
    let due = due(&crate::raw::open(home)?)?;
    purged.jobs = due.len();
    let ready = if due.iter().any(|(_, _, step, held)| *step < 2 || *held) {
        let _worker = crate::worker::lock_asking(home)?;
        let mut raw = crate::raw::open(home)?;
        for (job, uid, ..) in &due {
            purged.ops += raw.purge_uid(uid, job)?;
        }
        let truncated = raw.truncate_wal(TRUNCATE_WAIT)?;
        let mut ready = Vec::new();
        for (job, uid, ..) in due {
            if truncated && !raw.uid_has_ops(&uid)? {
                raw.finish_forget_step(&job, 2)?;
                ready.push((job, uid));
            } else {
                purged.unfinished.push(job);
            }
        }
        ready
    } else {
        due.into_iter().map(|(job, uid, ..)| (job, uid)).collect()
    };
    // The worker lock and raw are let go: the rebuild takes them itself.
    if ready.is_empty() {
        return Ok(Some(purged));
    }
    let uids: HashSet<String> = ready.iter().map(|(_, uid)| uid.clone()).collect();
    if !knowledge_holding(home, &uids)?.is_empty() {
        crate::worker::rebuild(home).context("knowledge.db was not rebuilt")?;
        purged.rebuilt = true;
        #[cfg(test)]
        if let Some(after) = AFTER_REBUILD.with(|hook| hook.borrow_mut().take()) {
            after();
        }
    }
    let holding = knowledge_holding(home, &uids)?;
    if !aside_files(home).is_empty() {
        let _worker = crate::worker::lock_asking(home)?;
        let raw = crate::raw::open(home)?;
        crate::embed_phase::carry_set_aside(home, &crate::knowledge::open(home)?, &raw)?;
        for file in aside_files(home) {
            std::fs::remove_file(&file)
                .with_context(|| format!("{} was not removed", file.display()))?;
            purged.aside += 1;
        }
    }
    purged.shown =
        crate::hookstate::update_all(home, "shown", |v| crate::hook::without_uids(v, &uids))?;
    let raw = crate::raw::open(home)?;
    for (job, uid) in ready {
        // Recorded for a stopped job too: it is not tried again, and `status` says it stopped.
        raw.finish_forget_step(&job, 3)?;
        if holding.contains(&uid) {
            purged.stopped.push(job);
        }
    }
    Ok(Some(purged))
}

/// How often a worker looks for a uid job its purge left (D5 item 8).
const CONTINUE_EVERY_MS: i64 = 10 * 60 * 1000;

#[cfg(test)]
thread_local! {
    /// The `oboete forget --continue` a worker on this thread would have started.
    pub(crate) static CONTINUED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// What happens to knowledge.db right after a purge's rebuild on this thread.
    static AFTER_REBUILD: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// D5 item 8: from a worker's round, `oboete forget --continue` started when a uid job waits for
/// its purge and none runs. Looked for at most once in ten minutes, the time of the last look in
/// `state/forget-continue`, written before it: a look that fails is not made again at once.
pub(crate) fn continue_due(home: &Path, raw: &crate::raw::Raw) {
    let stamp = home.join("state").join("forget-continue");
    let now = crate::db::now_ms();
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|t| t.trim().parse::<i64>().ok());
    if last.is_some_and(|at| (0..CONTINUE_EVERY_MS).contains(&(now - at))) {
        return;
    }
    if let Err(e) = std::fs::write(&stamp, now.to_string()) {
        eprintln!("oboete: forget jobs not looked for: {e}");
        return;
    }
    let waiting = due(raw).map_or_else(
        |e| {
            eprintln!("oboete: forget jobs not read: {e:#}");
            false
        },
        |due| !due.is_empty(),
    );
    // A purge that runs goes on with them itself.
    let free = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("state").join("purge.lock"))
        .is_ok_and(|f| f.try_lock().is_ok());
    if !waiting || !free {
        return;
    }
    #[cfg(test)]
    CONTINUED.set(CONTINUED.get() + 1);
    // Reaped when it leaves, as a resident worker's viewer is.
    #[cfg(not(test))]
    if let Some(mut child) = crate::hook::spawn_detached(home, &["forget", "--continue"]) {
        std::thread::spawn(move || child.wait());
    }
}

/// What a reconcile found: lines skipped, copies it could not read or write. Never a refusal.
#[derive(Debug, Default)]
pub struct Report {
    /// Requests the logs held that raw.db lacked, now applied.
    pub applied: usize,
    pub problems: Vec<String>,
    /// The copies written or already whole.
    pub copies: Vec<PathBuf>,
}

/// The two log copies: the home's, and the backup directory's when it has one (rule 3).
fn logs(home: &Path, report: &mut Report) -> Vec<PathBuf> {
    let mut out = vec![home.join(LOG)];
    match crate::backup::dir(home) {
        Ok(dir) if dir != home => out.push(dir.join(LOG)),
        Ok(_) => {}
        // Keep the safe path/[backup] context: inner TOML diagnostics can quote values.
        Err(e) => report.problems.push(format!(
            "backup request log directory could not be resolved: {e}"
        )),
    }
    out
}

/// One log copy's requests of this home, oldest first; damaged lines and lines of another home
/// are reported and skipped (rules 9, "a line names its home").
fn read_log(path: &Path, device: Option<&str>, report: &mut Report) -> Option<Vec<Request>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(e) => {
            report
                .problems
                .push(format!("{} could not be read: {e}", path.display()));
            return None;
        }
    };
    match read_lines(std::io::BufReader::new(file), path, device, report) {
        Ok(requests) => Some(requests),
        Err(e) => {
            report
                .problems
                .push(format!("{} could not be read: {e}", path.display()));
            None
        }
    }
}

/// Buffer at most one capped line, including its optional CRLF. Drain an oversized line
/// through the reader's fixed buffer, then continue with the next request.
fn read_lines(
    mut reader: impl BufRead,
    path: &Path,
    device: Option<&str>,
    report: &mut Report,
) -> std::io::Result<Vec<Request>> {
    let mut bytes = Vec::with_capacity(MAX_LINE + 2);
    let mut out = Vec::new();
    let mut n = 0_usize;
    loop {
        bytes.clear();
        if (&mut reader)
            .take((MAX_LINE + 2) as u64)
            .read_until(b'\n', &mut bytes)?
            == 0
        {
            break;
        }
        n += 1;
        let ended = bytes.last() == Some(&b'\n');
        if ended {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        if bytes.len() > MAX_LINE {
            if !ended {
                skip_line(&mut reader)?;
            }
            report.problems.push(format!(
                "{} line {n}: a line over its cap, skipped",
                path.display()
            ));
            continue;
        }
        if bytes.is_empty() {
            continue;
        }
        // Invalid UTF-8 spoils only this bounded line, as before.
        match Request::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(r) if device.is_none_or(|d| r.home == d) => out.push(r),
            Ok(_) => report.problems.push(format!(
                "{} line {n}: a request of another home, skipped",
                path.display()
            )),
            Err(e) => report
                .problems
                .push(format!("{} line {n}: {e:#}, skipped", path.display())),
        }
    }
    Ok(out)
}

fn skip_line(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok(());
        }
        let newline = bytes.iter().position(|b| *b == b'\n');
        let consumed = newline.map_or(bytes.len(), |at| at + 1);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(());
        }
    }
}

/// Appends `requests` to the log at `path`, one at a time under an exclusive lock on the file: a
/// newline first when the last byte is not one, each line written whole and synced, the
/// directory synced when the file is made (rule 9).
#[cfg(test)]
fn append_log(path: &Path, requests: &[&Request]) -> Result<()> {
    append_log_report(path, requests, &mut |_| {})
}

fn append_log_report(
    path: &Path,
    requests: &[&Request],
    committed: &mut impl FnMut(&'static str),
) -> Result<()> {
    if requests.is_empty() {
        return Ok(());
    }
    let made = !path.exists();
    // As the export makes the backup directory: private when oboete makes it.
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        std::fs::create_dir_all(dir)?;
        crate::db::private(dir, 0o700);
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = opts.open(path)?;
    f.lock()?;
    #[cfg(unix)]
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    let len = f.metadata()?.len();
    let mut out = Vec::new();
    if len > 0 {
        let mut last = [0_u8; 1];
        f.seek(std::io::SeekFrom::Start(len - 1))?;
        f.read_exact(&mut last)?;
        if last[0] != b'\n' {
            out.push(b'\n');
        }
    }
    for r in requests {
        out.extend(r.line()?.as_bytes());
        out.push(b'\n');
    }
    let written = f.write_all(&out);
    // The held log lock excludes other native appenders, including on a partial write error.
    let changed = written.is_ok() || f.metadata().is_ok_and(|metadata| metadata.len() != len);
    let result = (|| {
        written?;
        #[cfg(test)]
        crate::backup::fail_after("forget_log_sync")?;
        f.sync_all()?;
        if made && let Some(dir) = path.parent() {
            #[cfg(all(test, unix))]
            crate::backup::fail_after("forget_log_directory_sync")?;
            #[cfg(unix)]
            std::fs::File::open(dir)?.sync_all()?;
            #[cfg(not(unix))]
            let _ = dir;
        }
        Ok(())
    })();
    // A complete copy follows its syncs; known partial effects retain the existing warning.
    if result.is_ok() {
        committed("forget_reconciled");
    } else if changed {
        committed("stores_changed");
    }
    result
}

/// Rule 4, in both directions and never refusing: every request a readable log copy holds that
/// raw.db lacks is applied to it (by identity, rule 5), and every request raw.db holds is
/// appended to each copy that lacks it. A copy that cannot be read or written is reported.
pub(crate) fn reconcile(home: &Path, raw: &mut crate::raw::Raw) -> Result<Report> {
    reconcile_report(home, raw, &mut |_, _| {})
}

pub(crate) fn reconcile_report(
    home: &Path,
    raw: &mut crate::raw::Raw,
    committed: &mut impl FnMut(&'static str, usize),
) -> Result<Report> {
    let mut report = Report::default();
    let home_id = raw.home_id().to_owned();
    let copies: Vec<(PathBuf, Option<Vec<Request>>)> = logs(home, &mut report)
        .into_iter()
        .map(|p| {
            let read = read_log(&p, Some(&home_id), &mut report);
            (p, read)
        })
        .collect();
    let held: std::collections::HashSet<String> =
        raw.forget_requests()?.into_iter().map(|r| r.job).collect();
    let mut missing: Vec<Request> = Vec::new();
    for r in copies.iter().filter_map(|(_, c)| c.as_ref()).flatten() {
        if !held.contains(&r.job) && !missing.iter().any(|m| m.job == r.job) {
            missing.push(r.clone());
        }
    }
    report.applied = raw.forget_apply(&missing)?;
    if report.applied != 0 {
        committed("forget_reconciled", report.applied);
    }
    let all = raw.forget_requests()?;
    for (path, copy) in copies {
        let Some(copy) = copy else {
            continue;
        };
        let lacking: Vec<&Request> = all
            .iter()
            .filter(|r| !copy.iter().any(|c| c.job == r.job))
            .collect();
        match append_log_report(&path, &lacking, &mut |stage| committed(stage, 0)) {
            Ok(()) => report.copies.push(path),
            Err(e) => report
                .problems
                .push(format!("{} could not be written: {e:#}", path.display())),
        }
    }
    Ok(report)
}

/// Copy problems are warnings; failing to apply a parsed request to raw must stop the caller
/// before any import, consumer or provider phase proceeds without its deny rows.
pub(crate) fn reconcile_or_say(home: &Path, raw: &mut crate::raw::Raw) -> Result<()> {
    let report = reconcile(home, raw)?;
    for p in report.problems {
        eprintln!("oboete: forget request log: {p}");
    }
    Ok(())
}

/// Both log copies' requests, of any home, read before a restore takes raw's swap lock (no log I/O
/// under it); the restore keeps those of its device.
pub(crate) fn logged(home: &Path) -> (Vec<Request>, Report) {
    let mut report = Report::default();
    let mut out: Vec<Request> = Vec::new();
    for p in logs(home, &mut report) {
        for r in read_log(&p, None, &mut report).into_iter().flatten() {
            if !out.iter().any(|o| o.job == r.job) {
                out.push(r);
            }
        }
    }
    (out, report)
}

/// What a uid's forget cannot reach yet (D5), printed before it asks.
fn uid_limits(kind: &str) -> String {
    let mut out = String::from(
        "Its ops in raw.db are rewritten without the text right after registration, then \
         knowledge.db is rebuilt from raw.db without it. That takes minutes on a large store \
         (a minute and a half for 70,000 records), and a session that starts meanwhile gets part \
         of its memory or none; two forgets one after the other rebuild twice. The backup \
         segments keep it until their purge is built, hidden from every reader and never sent \
         again.\n\
         A packet already handed to an agent and a call already sent are not taken back.\n\
         The request is logged by this uid, without the text.\n",
    );
    out.push_str(if kind == "claim" {
        "The raw records it was curated from stay, and search still finds them: forget their \
         span with --record or --span when the text itself must go.\n"
    } else {
        "The place it was imported from keeps it: delete it there too.\n"
    });
    out
}

/// What forget cannot reach, printed before it asks (docs/milestone-5-plan.md, Limits).
fn limits(sources: &[String]) -> String {
    let mut out = String::from(
        "Physical purge is not built yet: the record stays in raw.db, hidden, and older backup \
         segments still hold its text, hidden again when they are restored.\n\
         A packet already handed to an agent and a curation call already sent are not taken \
         back.\n\
         The request is logged without the text, by hashes; someone who holds a log or the \
         backups can check a guess of the exact stored text against them.\n\
         If raw.db and both request logs are lost, the request is lost and the text can come \
         back from older backups.\n\
         After an older file replaces raw.db, SessionStart may show a forgotten record until \
         the worker reopens the stores and applies the request logs.\n\
         A crash between raw's commit and the first log line, followed by loss of raw.db \
         before the worker starts, loses the request too.\n\
         Restoring segments with no surviving request log recovers denials but not the \
         request's progress rows.\n",
    );
    let mut outside: Vec<&str> = Vec::new();
    for s in sources {
        let what = match s.as_str() {
            "oboete-v1" => "the old oboete v1 store it was moved from",
            "transcript" => "the agent's own transcript files",
            _ => "the place it was imported from",
        };
        if !outside.contains(&what) {
            outside.push(what);
        }
    }
    if outside.is_empty() {
        out.push_str("oboete knows no copy of it outside its own store.\n");
    } else {
        out.push_str(&format!(
            "Copies outside oboete keep it: {}. Delete it there too.\n",
            outside.join(", ")
        ));
    }
    out
}

pub fn run(
    home: &Path,
    record: Option<&str>,
    span: Option<&str>,
    uid: Option<&str>,
    yes: bool,
    show: bool,
    resume: bool,
) -> Result<()> {
    if crate::raw::exists(home) {
        reconcile_or_say(home, &mut crate::raw::open(home)?)?;
    }
    if resume {
        // Nothing to continue says nothing: the worker starts it when a job waits (D5 item 8).
        return match purge(home)? {
            Some(p) if p.jobs > 0 => say_purged(&p),
            _ => Ok(()),
        };
    }
    if show {
        for job in status(home)? {
            println!("{}", serde_json::to_string(&job)?);
        }
        return Ok(());
    }
    let target = match (record, span, uid) {
        (Some(r), None, None) => Target::parse(r, false)?,
        (None, Some(s), None) => Target::parse(s, true)?,
        (None, None, Some(u)) => Target::parse_uid(u)?,
        _ => anyhow::bail!("choose --record, --span, --uid, --status or --continue"),
    };
    let p = preview(home, target)?;
    println!("Target: {}", serde_json::to_string(&p.target)?);
    println!("Raw records: {}", p.count());
    if let Some(sample) = &p.sample {
        println!("Sample: {}", crate::redact::outbound(sample));
    }
    if let Some(u) = &p.uid {
        println!(
            "A {}: its ops in raw.db: {}; its vectors: {}",
            u.kind, u.ops, u.vectors
        );
        for again in &u.again {
            println!(
                "It supersedes, and with it gone this is current again unless another claim \
                 supersedes it (mute or forget it too if it should not be): {}",
                crate::redact::outbound(again)
            );
        }
    }
    p.validate()?;
    match &p.uid {
        Some(u) => print!("{}", uid_limits(u.kind)),
        None => print!("{}", limits(&p.sources)),
    }
    if !yes {
        // A pipe cannot answer for the owner: `--yes` says it on the command line.
        anyhow::ensure!(
            std::io::stdin().is_terminal(),
            "the confirmation needs a terminal; run it in one, or pass --yes"
        );
        println!("Register this irreversible request? Type yes:");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != "yes" {
            println!("Nothing was registered.");
            return Ok(());
        }
    }
    let (status, report) = start(home, &p)?;
    println!("{}", serde_json::to_string(&status)?);
    for c in &report.copies {
        println!("Request logged in {}", c.display());
    }
    for p in &report.problems {
        println!(
            "Not logged: {p}. raw.db holds the request; a restore from the backups alone would not."
        );
    }
    if p.uid.is_none() {
        return Ok(());
    }
    // D5 (2b): registered, and purged in this process.
    match purge(home)
        .context("the purge did not finish; `oboete forget --continue` goes on with it")?
    {
        Some(p) => say_purged(&p),
        None => {
            println!(
                "Another oboete is purging forgotten uids; `oboete forget --continue` purges this \
                 one once it ends."
            );
            Ok(())
        }
    }
}

fn say_purged(p: &Purged) -> Result<()> {
    println!("Ops rewritten without their text in raw.db: {}", p.ops);
    if p.rebuilt {
        println!("knowledge.db rebuilt from raw.db without it.");
    }
    if p.aside > 0 {
        println!(
            "Files of knowledge.db copies set aside, removed: {}",
            p.aside
        );
    }
    if p.shown > 0 {
        println!("Sessions whose shown memory lost it: {}", p.shown);
    }
    anyhow::ensure!(
        p.unfinished.is_empty(),
        "raw.db-wal did not truncate while readers held it: `oboete forget --continue` finishes \
         jobs {}",
        p.unfinished.join(", ")
    );
    anyhow::ensure!(
        p.stopped.is_empty(),
        "knowledge.db still holds the uid of jobs {} after the rebuild: an op it came from was \
         missed, so they stopped and are not done",
        p.stopped.join(", ")
    );
    Ok(())
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A native event identity, versioned and hashed before it is kept: no original id or text.
pub(crate) fn origin(source: &str, id: &str) -> String {
    format!(
        "v1:{}",
        hash(&serde_json::to_vec(&(source, id)).expect("string pair"))
    )
}

/// A session's labels as a log line keeps them, hashed.
pub(crate) fn session(agent: &str, session: &str) -> String {
    origin(agent, session)
}

pub(crate) fn check_identity(s: &str) -> Result<()> {
    anyhow::ensure!(
        s.strip_prefix("v1:")
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())),
        "unknown or damaged identity"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{self, Item};

    /// D5 (#439): a forget registered by another process while a preview reads waits for the
    /// preview: its sample is the one read before the commit.
    #[test]
    fn a_forget_registered_inside_a_preview_waits_for_it() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let (h, u) = (home.clone(), uid.clone());
        let (sent, started) = std::sync::mpsc::channel();
        SAMPLING.set(Some(Box::new(move || {
            sent.send(registering(&h, &u)).unwrap();
        })));
        let p = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert!(p.sample.as_deref().is_some_and(|s| s.contains("Use tabs.")));
        started.recv().unwrap().join().unwrap();
        assert!(preview(&home, Target::parse_uid(&uid).unwrap()).is_err());
        // A record's preview holds it too (Codex on #446).
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let seq = native(&mut raw, "a", r#"{"prompt":"a synthetic canary"}"#);
        let (h, target) = (home.path().to_owned(), record(&raw, seq));
        let (sent, started) = std::sync::mpsc::channel();
        let registered = target.clone();
        SAMPLING.set(Some(Box::new(move || {
            sent.send(registering_target(&h, registered)).unwrap();
        })));
        let p = preview(home.path(), target).unwrap();
        assert!(p.sample.as_deref().is_some_and(|s| s.contains("canary")));
        started.recv().unwrap().join().unwrap();
        assert!(!shown(&raw, seq));
    }

    #[test]
    fn bounded_log_lines_keep_crlf_the_cap_and_a_final_request() {
        let request = Request {
            v: 1,
            home: "synthetic".into(),
            job: "a".repeat(32),
            started: 0,
            target: Target::Record {
                device: "synthetic".into(),
                seq: 1,
            },
            records: vec![Record {
                device: "synthetic".into(),
                seq: 1,
                origin: origin("synthetic", "bounded-record"),
                session: session("claude", "bounded-session"),
                ts: None,
            }],
        };
        let line = request.line().unwrap();
        let mut input = vec![b'\n'];
        input.extend(std::iter::repeat_n(b' ', MAX_LINE - line.len()));
        input.extend(line.as_bytes());
        input.extend(b"\r\n");
        input.extend(std::iter::repeat_n(b'x', MAX_LINE + 1));
        input.extend(b"\n\xff\n");
        input.extend(line.as_bytes());
        let mut report = Report::default();
        let read = read_lines(
            std::io::Cursor::new(input),
            Path::new("synthetic.log"),
            Some("synthetic"),
            &mut report,
        )
        .unwrap();
        assert_eq!(read, vec![request.clone(), request]);
        assert_eq!(report.problems.len(), 2);
        assert!(report.problems[0].contains("line 3") && report.problems[0].contains("cap"));
        assert!(report.problems[1].contains("line 4"));
    }

    /// An imported record of origin `id`, as an importer appends it.
    fn native(raw: &mut raw::Raw, id: &str, body: &str) -> i64 {
        let mut event = raw::test_event(body);
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", id),
            session: session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        raw.append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            &[identity],
            "",
            None,
        )
        .unwrap()
        .first()
        .copied()
        .unwrap_or(0)
    }

    fn record(raw: &raw::Raw, seq: i64) -> Target {
        Target::Record {
            device: raw.device().into(),
            seq,
        }
    }

    fn shown(raw: &raw::Raw, seq: i64) -> bool {
        raw.after(raw.device(), seq - 1, 1)
            .unwrap()
            .first()
            .is_some_and(|r| r.seq == seq && matches!(r.item, Item::Event(_)))
    }

    /// Rule 1: one raw transaction hides the record, denies its origin and keeps the job; rule 5:
    /// an import of the same origin is not recorded again, another origin is.
    #[test]
    fn a_forget_hides_the_record_and_its_origin_is_not_imported_again() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let seq = native(&mut raw, "a", r#"{"prompt":"a synthetic canary"}"#);
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        assert_eq!(p.count(), 1);
        let (status, report) = start(home.path(), &p).unwrap();
        assert_eq!(status.records, 1);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(!shown(&raw, seq));
        assert_eq!(
            native(&mut raw, "a", r#"{"prompt":"a synthetic canary"}"#),
            0
        );
        let other = native(&mut raw, "b", r#"{"prompt":"another record"}"#);
        assert!(shown(&raw, other));
        assert_eq!(raw.forget_requests().unwrap().len(), 1);
        let log = std::fs::read_to_string(home.path().join(LOG)).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert!(!log.contains("canary"), "{log}");
    }

    /// D5: a uid's preview is read from knowledge.db: a claim's ops and the claims it supersedes,
    /// a document's ops and title, no raw record. What names no claim or document here, and a
    /// record's, a card's or a summary's id, is refused before anything is registered.
    #[test]
    fn a_uid_preview_counts_its_ops_and_refuses_what_names_no_uid() {
        let mut s = crate::search::b::fixture::Store::new();
        let older = s.decided("github.com/o/r", 1_000, "Use spaces.", &[]);
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[&older]);
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        s.run();
        let home = s.home.path();
        let p = preview(home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert_eq!((p.count(), p.sample.as_deref()), (0, Some("Use tabs.")));
        let again = vec!["Use spaces.".to_owned()];
        let shown = UidPreview {
            kind: "claim",
            ops: 1,
            vectors: 0,
            again,
        };
        assert_eq!(p.uid, Some(shown));
        p.validate().unwrap();
        let p = preview(home, Target::parse_uid(&doc).unwrap()).unwrap();
        let u = p.uid.as_ref().unwrap();
        assert_eq!(
            (u.kind, u.ops, p.sample.as_deref()),
            ("document", 1, Some("Deploy notes"))
        );
        let device = s.raw.device();
        let e = Target::parse_uid(&format!("{device}:1")).unwrap_err();
        assert!(e.to_string().contains("--record"), "{e}");
        for id in [
            format!("{device}.4.0"),
            "4.0".into(),
            format!("S{device}.4"),
        ] {
            let e = Target::parse_uid(&id).unwrap_err();
            assert!(e.to_string().contains("card"), "{id}: {e}");
        }
        let e = preview(home, Target::parse_uid(&"a".repeat(64)).unwrap()).unwrap_err();
        assert!(e.to_string().contains("neither"), "{e}");
        assert!(Target::parse_uid("a b").is_err());
        assert!(
            raw::open(home)
                .unwrap()
                .forget_requests()
                .unwrap()
                .is_empty()
        );
        // A claim it supersedes that is forgotten already comes back as nothing (Codex's
        // adversarial review of slice 2a).
        start(
            home,
            &preview(home, Target::parse_uid(&older).unwrap()).unwrap(),
        )
        .unwrap();
        let p = preview(home, Target::parse_uid(&uid).unwrap()).unwrap();
        assert_eq!(p.uid.map(|u| u.again), Some(Vec::new()));
    }

    /// D5 (Codex on #435): a uid whose ops no consumer has read yet, right after an import or
    /// while the worker lags, is found in the op log and forgotten; once the consumers read them,
    /// search still passes it over.
    #[test]
    fn a_uid_knowledge_has_not_read_yet_is_forgotten_from_the_op_log() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs for the parser.", &[]);
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Deploy with the parser.");
        let home = s.home.path().to_owned();
        let p = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        let shown = p.uid.as_ref().map(|u| (u.kind, u.ops));
        assert_eq!(
            (shown, p.sample.as_deref()),
            (Some(("claim", 1)), Some("Use tabs for the parser."))
        );
        start(&home, &p).unwrap();
        let p = preview(&home, Target::parse_uid(&doc).unwrap()).unwrap();
        let shown = p.uid.as_ref().map(|u| (u.kind, u.ops));
        assert_eq!(
            (shown, p.sample.as_deref()),
            (Some(("document", 1)), Some("Deploy notes"))
        );
        start(&home, &p).unwrap();
        let e = preview(&home, Target::parse_uid(&"b".repeat(64)).unwrap()).unwrap_err();
        assert!(e.to_string().contains("neither"), "{e}");
        s.run();
        let q = crate::search::b::Query {
            text: "parser".into(),
            all: true,
            limit: 20,
            ..Default::default()
        };
        let found: Vec<String> = s.query(&q).hits.into_iter().map(|h| h.key).collect();
        assert!(!found.contains(&uid) && !found.contains(&doc));
    }

    /// D5 (Codex and CodeRabbit on #435): the op log gives the preview its count while knowledge.db
    /// lags it, and a uid of 64 hex digits that no claim has is looked for among the imports.
    #[test]
    fn the_preview_counts_what_the_op_log_holds() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs for the parser.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let ops = |uid: &str| {
            preview(&home, Target::parse_uid(uid).unwrap())
                .unwrap()
                .uid
                .map(|u| (u.kind, u.ops))
        };
        assert_eq!(ops(&uid), Some(("claim", 1)));
        // An owner's change no consumer has read yet.
        let change = serde_json::json!({"uid": uid, "muted": true});
        s.raw
            .append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        assert_eq!(ops(&uid), Some(("claim", 2)));
        let hex = "c".repeat(64);
        s.raw
            .append_imports(vec![raw::ImportDoc {
                uid: hex.clone(),
                source: "claude-mem:test".into(),
                source_id: "h".into(),
                kind: "decision".into(),
                repo: crate::import::repo("r"),
                session: "cm".into(),
                ts: 3_000,
                title: "Hex notes".into(),
                body: "Hex.".into(),
            }])
            .unwrap();
        assert_eq!(ops(&hex), Some(("document", 1)));
    }

    /// Every op raw.db holds: (device, op_seq, type, body), in key order.
    fn op_rows(home: &Path) -> Vec<(String, i64, String, String)> {
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        let mut st = conn
            .prepare("SELECT device, op_seq, type, body FROM ops ORDER BY device, op_seq")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// Each forget job's (target uid, job id, step).
    fn uid_jobs(home: &Path) -> Vec<(String, String, i64)> {
        raw::open(home)
            .unwrap()
            .forget_jobs()
            .unwrap()
            .into_iter()
            .filter_map(|(r, step)| match r.target {
                Target::Uid { uid } => Some((uid, r.job, step)),
                _ => None,
            })
            .collect()
    }

    /// D5 (2b-1): a purge rewrites what holds a forgotten uid's text, its claim's Claim op and the
    /// Correction that names it, or a document's Import op, to `{"forgotten": job}` with its
    /// type, op_seq and batch kept; every other op stays byte for byte; steps 2 and 3 are recorded,
    /// and the next purge finds nothing to do.
    #[test]
    fn a_purge_rewrites_the_forgotten_uids_ops_and_no_other() {
        let mut s = crate::search::b::fixture::Store::new();
        let older = s.decided("github.com/o/r", 1_000, "Use spaces.", &[]);
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[&older]);
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        let change = serde_json::json!({"uid": uid, "muted": true});
        s.raw
            .append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        let before = op_rows(&home);
        for u in [&uid, &doc] {
            start(
                &home,
                &preview(&home, Target::parse_uid(u).unwrap()).unwrap(),
            )
            .unwrap();
        }
        let jobs = uid_jobs(&home);
        let job = |u: &str| jobs.iter().find(|j| j.0 == u).unwrap().1.clone();
        // A rebuild wants raw.db open nowhere else.
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops, purged.rebuilt), (2, 3, true));
        assert!(purged.unfinished.is_empty());
        let after = op_rows(&home);
        let mut rewritten = 0;
        for (device, seq, kind, body) in &before {
            let now = after
                .iter()
                .find(|a| (&a.0, a.1) == (device, *seq))
                .unwrap();
            assert_eq!(&now.2, kind);
            let owner = match kind.as_str() {
                "claim" if body.contains("Use tabs.") => Some(&uid),
                "correction" => Some(&uid),
                "import" => Some(&doc),
                _ => None,
            };
            match owner {
                Some(u) => {
                    let expected = serde_json::json!({ "forgotten": job(u) });
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&now.3).unwrap(),
                        expected
                    );
                    rewritten += 1;
                }
                None => assert_eq!(&now.3, body, "op {seq} changed"),
            }
        }
        assert_eq!(rewritten, 3);
        assert!(uid_jobs(&home).iter().all(|j| j.2 == 3));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// D5 (2b-1): ops a purge rewrote derive nothing, read by a pass that had not reached them and
    /// by a rebuild: the worker goes on, a rewritten correction is passed for good, and after the
    /// rebuild the uids keep no claim, derivation, correction or imported row and the claim the
    /// forgotten one superseded is current again.
    #[test]
    fn rewritten_ops_derive_nothing_and_what_they_superseded_is_current_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let older = s.decided("github.com/o/r", 1_000, "Use spaces.", &[]);
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[&older]);
        s.run();
        let home = s.home.path().to_owned();
        let k = |home: &Path| crate::knowledge::open(home).unwrap();
        // What supersedes `older`: with the forgotten claim gone, nothing, and it is current again.
        let superseded = |home: &Path| -> i64 {
            k(home)
                .query_row(
                    "SELECT COUNT(*) FROM edges WHERE to_uid = ?1 AND type = 'supersedes'",
                    [&older],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(superseded(&home), 1);
        // Not read by any consumer yet when they are purged.
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        let change = serde_json::json!({"uid": uid, "muted": true});
        s.raw
            .append_ops(&[(raw::OpKind::Correction, change)])
            .unwrap();
        for u in [&uid, &doc] {
            start(
                &home,
                &preview(&home, Target::parse_uid(u).unwrap()).unwrap(),
            )
            .unwrap();
        }
        // Rewritten as a purge's step 2 rewrites them, before any consumer has read them.
        let rewritten: usize = uid_jobs(&home)
            .iter()
            .map(|(u, job, _)| s.raw.purge_uid(u, job).unwrap())
            .sum();
        assert_eq!(rewritten, 3);
        let skips = |home: &Path| -> Vec<String> {
            k(home)
                .prepare("SELECT reason FROM claim_skips ORDER BY op_seq")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let held = |home: &Path| -> i64 {
            k(home)
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM claims WHERE uid = ?1)
                          + (SELECT COUNT(*) FROM derivations WHERE uid = ?1)
                          + (SELECT COUNT(*) FROM corrections WHERE uid = ?1)
                          + (SELECT COUNT(*) FROM imported WHERE uid = ?2)",
                    [&uid, &doc],
                    |r| r.get(0),
                )
                .unwrap()
        };
        s.run();
        assert_eq!(skips(&home), ["forgotten"]);
        let imported: i64 = k(&home)
            .query_row("SELECT COUNT(*) FROM imported", [], |r| r.get(0))
            .unwrap();
        assert_eq!(imported, 0);
        // A rebuild wants raw.db open nowhere else.
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::worker::rebuild(&home).unwrap();
        assert_eq!(held(&home), 0);
        assert_eq!(superseded(&home), 0);
        assert_eq!(skips(&home), ["forgotten", "forgotten"]);
    }

    /// D5 (2b, item 10, test 2): a claim op's text that no record holds leaves no byte in raw.db,
    /// knowledge.db, their WALs or any file under `state/` once purged: the ops are rewritten with
    /// secure_delete and the WAL truncated, knowledge.db is rebuilt without the claim, and every
    /// session's shown value (OpenCode's receipts too, and a part a crashed change left) loses it.
    #[test]
    fn a_purge_leaves_no_byte_of_the_claims_own_text_in_the_stores_or_the_hook_state() {
        const CANARY: &str = "canary-7f3a";
        let mut s = crate::search::b::fixture::Store::new();
        let seq = s.said("s", "github.com/o/r", 1_000, "We deploy on Fridays.");
        let evidence = crate::claims::Evidence {
            device: s.raw.device().to_owned(),
            seq,
            offset: 0,
            length: "We deploy on Fridays.".len() as i64,
            sentence: 0,
            quote: "We deploy on Fridays.".into(),
            claim_at: None,
        };
        let uid = crate::claims::uid("decision", &evidence);
        let op = crate::claims::ClaimOp {
            id: "c1".into(),
            kind: "decision".into(),
            status: "decided".into(),
            speaker: "user".into(),
            scope: "repo".into(),
            body: format!("Deploys happen on Friday, {CANARY}."),
            evidence: vec![evidence],
            supersedes: Vec::new(),
            recipe: "test".into(),
            tier: 1,
            why: String::new(),
            tainted: false,
        };
        s.raw
            .append_ops(&[(raw::OpKind::Claim, serde_json::to_value(op).unwrap())])
            .unwrap();
        s.run();
        let home = s.home.path().to_owned();
        // What sessions were shown: Claude Code's form, OpenCode's with a receipt not yet taken,
        // and the part a change that crashed left in a session not shown it yet.
        let entry = format!(r#"{{"fp":"f","body":"Deploys happen on Friday, {CANARY}."}}"#);
        let kept = r#"{"fp":"g","body":"Kept."}"#;
        let flat = format!(r#"{{"{uid}":{entry},"kept-uid":{kept}}}"#);
        let receipts = format!(
            r#"{{"entries":{{"{uid}":{entry}}},"pending":[{{"id":"{}","at":1,"changes":[{{"uid":"{uid}","before":null,"after":{entry}}}],"replace":{{"{uid}":{entry}}}}}]}}"#,
            "0".repeat(32)
        );
        let before_crash = format!(r#"{{"kept-uid":{kept}}}"#);
        crate::hookstate::update(&home, "claude", "s", "shown", |_| Some(flat)).unwrap();
        crate::hookstate::update(&home, "claude", "crashed", "shown", |_| Some(before_crash))
            .unwrap();
        crate::hookstate::update(&home, "opencode", "oc", "shown", |_| Some(receipts)).unwrap();
        let crashed = std::fs::read_dir(home.join("state/hooks/claude"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|d| {
                !std::fs::read_to_string(d.join("shown"))
                    .unwrap()
                    .contains(&uid)
            })
            .unwrap();
        std::fs::write(crashed.join(".shown.part"), &entry).unwrap();
        // Every store file and every file under `state/` that holds the canary.
        fn holding(dir: &Path, found: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    holding(&path, found);
                } else if std::fs::read(&path)
                    .unwrap()
                    .windows(CANARY.len())
                    .any(|w| w == CANARY.as_bytes())
                {
                    found.push(path);
                }
            }
        }
        let hits = |home: &Path| -> Vec<PathBuf> {
            let mut found = Vec::new();
            holding(&home.join("state"), &mut found);
            for f in ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"] {
                let path = home.join(f);
                if std::fs::read(&path)
                    .is_ok_and(|b| b.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()))
                {
                    found.push(path);
                }
            }
            found
        };
        let before = hits(&home);
        assert!(
            before
                .iter()
                .any(|p| p.ends_with("knowledge.db") || p.ends_with("knowledge.db-wal"))
        );
        assert_eq!(
            before
                .iter()
                .filter(|p| p.starts_with(home.join("state")))
                .count(),
            3
        );
        start(
            &home,
            &preview(&home, Target::parse_uid(&uid).unwrap()).unwrap(),
        )
        .unwrap();
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.ops, purged.rebuilt, purged.shown), (1, true, 2));
        assert_eq!(hits(&home), Vec::<PathBuf>::new());
        let shown = crate::hookstate::value(&home, "claude", "s", "shown").unwrap();
        assert!(shown.contains("kept-uid"), "{shown}");
    }

    /// D5 (2b-1): a purged Import op no longer names its source id, so the import's own check
    /// would take the document for a new one: the forgotten uid is what keeps it out.
    #[test]
    fn a_purged_document_is_not_imported_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let doc = s.imported("o1", "r", 3_000, "Deploy notes", "Notes.");
        let home = s.home.path().to_owned();
        start(
            &home,
            &preview(&home, Target::parse_uid(&doc).unwrap()).unwrap(),
        )
        .unwrap();
        purge(&home).unwrap().unwrap();
        assert!(s.raw.import_repos("claude-mem:test").unwrap().is_empty());
        let again = raw::ImportDoc {
            uid: doc.clone(),
            source: "claude-mem:test".into(),
            source_id: "o1".into(),
            kind: "decision".into(),
            repo: crate::import::repo("r"),
            session: "cm".into(),
            ts: 3_000,
            title: "Deploy notes".into(),
            body: "Notes.".into(),
        };
        assert_eq!(s.raw.append_imports(vec![again]).unwrap(), 0);
        assert!(!op_rows(&home).iter().any(|o| o.3.contains("Deploy notes")));
    }

    /// D5 (2b-1): while a reader holds raw.db-wal, the purge rewrites the ops but cannot truncate
    /// the WAL, which may still hold their old pages: step 2 is not recorded, and the next purge,
    /// with the reader gone, records it and goes on to step 3.
    #[test]
    fn a_wal_a_reader_holds_keeps_the_step() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        start(
            &home,
            &preview(&home, Target::parse_uid(&uid).unwrap()).unwrap(),
        )
        .unwrap();
        let reader = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM ops", [], |r| r.get(0))
            .unwrap();
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.ops, purged.unfinished.len()), (1, 1));
        assert_eq!(uid_jobs(&home)[0].2, 1);
        reader.execute_batch("COMMIT").unwrap();
        drop(reader);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops, purged.rebuilt), (1, 0, true));
        assert!(purged.unfinished.is_empty());
        assert_eq!(uid_jobs(&home)[0].2, 3);
    }

    /// D5 (2b-1): one purge at a time: while another holds `state/purge.lock`, a purge does
    /// nothing and says so.
    #[test]
    fn one_purge_at_a_time() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        let home = s.home.path().to_owned();
        start(
            &home,
            &preview(&home, Target::parse_uid(&uid).unwrap()).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(home.join("state")).unwrap();
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.join("state/purge.lock"))
            .unwrap();
        held.lock().unwrap();
        assert_eq!(purge(&home).unwrap(), None);
        assert_eq!(uid_jobs(&home)[0].2, 1);
        drop(held);
        assert_eq!(purge(&home).unwrap().unwrap().ops, 1);
    }

    /// Registers the forget of `uid` in `home`.
    fn forget(home: &Path, uid: &str) {
        start(
            home,
            &preview(home, Target::parse_uid(uid).unwrap()).unwrap(),
        )
        .unwrap();
    }

    /// The knowledge.db copies set aside in `home`: a rebuild that stopped leaves one.
    fn set_aside_copy(home: &Path, to: &Path) {
        let k = crate::knowledge::open(home).unwrap();
        k.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        drop(k);
        std::fs::copy(home.join("knowledge.db"), to).unwrap();
    }

    /// D5 (2b, item 4, test 5): one rebuild takes every job past step 2: two forgets registered
    /// before a purge leave knowledge.db together, and the next purge has nothing to do.
    #[test]
    fn one_rebuild_takes_every_job_registered_before_it() {
        let mut s = crate::search::b::fixture::Store::new();
        let tabs = s.decided("github.com/o/r", 1_000, "Use tabs.", &[]);
        let doc = s.imported("o1", "r", 2_000, "Deploy notes", "Notes.");
        s.run();
        let home = s.home.path().to_owned();
        forget(&home, &tabs);
        forget(&home, &doc);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.rebuilt), (2, true));
        assert!(purged.stopped.is_empty());
        let uids = HashSet::from([tabs, doc]);
        assert!(knowledge_holding(&home, &uids).unwrap().is_empty());
        assert!(uid_jobs(&home).iter().all(|j| j.2 == 3));
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
        let local: Vec<&str> = status(&home).unwrap().iter().map(|s| s.local).collect();
        assert_eq!(local, ["purged here; backups pending"; 2]);
    }

    /// D5 (2b, item 5, test 6): no forgotten uid's vector comes back, neither from the knowledge.db
    /// the purge's rebuild sets aside nor from one an earlier rebuild left set aside when it
    /// stopped; a kept claim's comes back, and the copy is removed.
    #[test]
    fn no_forgotten_vector_comes_back_from_a_copy_set_aside() {
        let mut s = crate::search::b::fixture::Store::new();
        // A quote shorter than its record: no record's vector is the claim's.
        let said = "Parser caches stay in Redis, the team said.";
        let seq = s.said("s", "github.com/o/r", 1_000, said);
        let quote = "Parser caches stay in Redis";
        let gone = s.claim(seq, quote, ("decision", "decided", "user"), &[]);
        let kept = s.decided("github.com/o/r", 2_000, "Deploys run on Fridays.", &[]);
        s.run();
        crate::embed_phase::fixture::vectors(&s);
        let home = s.home.path().to_owned();
        let sha = |uid: &str| -> String {
            crate::knowledge::open(&home)
                .unwrap()
                .query_row(
                    "SELECT src_sha FROM vector_keys WHERE kind = 'c' AND key = ?1",
                    [uid],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let (gone_sha, kept_sha) = (sha(&gone), sha(&kept));
        let others: i64 = crate::knowledge::open(&home)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM vector_keys WHERE src_sha = ?1 AND key <> ?2",
                [&gone_sha, &gone],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(others, 0);
        set_aside_copy(
            &home,
            &home.join(format!("knowledge.db.rebuilding-{}", crate::db::now_ms())),
        );
        forget(&home, &gone);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let purged = purge(&home).unwrap().unwrap();
        assert!(purged.rebuilt && purged.aside > 0, "{purged:?}");
        assert!(aside_files(&home).is_empty());
        let k = crate::knowledge::open(&home).unwrap();
        let has = |sha: &str| -> bool {
            k.query_row(
                "SELECT EXISTS(SELECT 1 FROM vectors WHERE src_sha = ?1)",
                [sha],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(!has(&gone_sha));
        assert!(has(&kept_sha));
    }

    /// D5 (2b, item 7, test 6): a purge stopped after its rebuild, before it removed the copy set
    /// aside, goes on without a second rebuild: the copy is removed and step 3 recorded.
    #[test]
    fn a_purge_stopped_after_its_rebuild_removes_the_copy_without_rebuilding_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let copy = home.join("copy");
        set_aside_copy(&home, &copy);
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        assert!(purge(&home).unwrap().unwrap().rebuilt);
        // Stopped there: the old file left set aside, step 3 not recorded.
        let aside = home.join(format!("knowledge.db.rebuilding-{}", crate::db::now_ms()));
        std::fs::rename(&copy, aside).unwrap();
        rusqlite::Connection::open(home.join("raw.db"))
            .unwrap()
            .execute("UPDATE forget_jobs SET step = 2", [])
            .unwrap();
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops, purged.rebuilt), (1, 0, false));
        assert!(purged.aside > 0);
        assert!(aside_files(&home).is_empty());
        assert_eq!(uid_jobs(&home)[0].2, 3);
    }

    /// D5 (2b, item 7, test 6): a restore that brings a purged op's text back is purged again,
    /// though its job recorded step 3: the state decides, and a worker starts the continuation.
    #[test]
    fn a_text_a_restore_brings_back_is_purged_again() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        let home = s.home.path().to_owned();
        let before = op_rows(&home);
        forget(&home, &uid);
        assert_eq!(purge(&home).unwrap().unwrap().ops, 1);
        assert_eq!(uid_jobs(&home)[0].2, 3);
        // The bodies an older raw.db held, back in place.
        let conn = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        for (device, seq, _, body) in &before {
            conn.execute(
                "UPDATE ops SET body = ?3 WHERE device = ?1 AND op_seq = ?2",
                rusqlite::params![device, seq, body],
            )
            .unwrap();
        }
        drop(conn);
        CONTINUED.set(0);
        continue_due(&home, &s.raw);
        assert_eq!(CONTINUED.get(), 1);
        let purged = purge(&home).unwrap().unwrap();
        assert_eq!((purged.jobs, purged.ops), (1, 1));
        assert_eq!(uid_jobs(&home)[0].2, 3);
        assert!(!op_rows(&home).iter().any(|o| o.3.contains("Use tabs.")));
    }

    /// D5 (2b, item 8, test 7): a worker that finds a uid job its purge left starts `oboete forget
    /// --continue`, at most once in ten minutes and not while a purge runs; none once it is done.
    #[test]
    fn the_worker_starts_a_continuation_once_in_ten_minutes() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        let home = s.home.path().to_owned();
        forget(&home, &uid);
        CONTINUED.set(0);
        s.run();
        assert_eq!(CONTINUED.get(), 1);
        s.run();
        assert_eq!(CONTINUED.get(), 1, "looked again within ten minutes");
        let ago = |home: &Path| {
            let at = crate::db::now_ms() - 11 * 60_000;
            std::fs::write(home.join("state/forget-continue"), at.to_string()).unwrap();
        };
        ago(&home);
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.join("state/purge.lock"))
            .unwrap();
        held.lock().unwrap();
        s.run();
        assert_eq!(CONTINUED.get(), 1, "started beside a purge that runs");
        drop(held);
        ago(&home);
        s.run();
        assert_eq!(CONTINUED.get(), 2);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        purge(&home).unwrap().unwrap();
        ago(&home);
        crate::worker::run_once(&home).unwrap();
        assert_eq!(CONTINUED.get(), 2, "started with nothing to continue");
    }

    /// D5 (2b, item 7): a uid knowledge.db still holds after the purge's rebuild came from an op the
    /// purge did not find: the job stops, the purge fails saying so, `status` and doctor name it,
    /// and neither a worker nor the next purge goes on with it.
    #[test]
    fn a_uid_the_rebuild_leaves_in_knowledge_db_stops_its_job() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        forget(&home, &uid);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        let (at, left) = (home.clone(), uid.clone());
        AFTER_REBUILD.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                crate::knowledge::open(&at)
                    .unwrap()
                    .execute(
                        "INSERT INTO claims(uid, op_device, op_seq) VALUES (?1, 'd', 1)",
                        [&left],
                    )
                    .unwrap();
            }))
        });
        let purged = purge(&home).unwrap().unwrap();
        let job = uid_jobs(&home)[0].1.clone();
        assert_eq!(purged.stopped, std::slice::from_ref(&job));
        assert!(say_purged(&purged).unwrap_err().to_string().contains(&job));
        assert_eq!(status(&home).unwrap()[0].local, STOPPED);
        let (lines, stopped) = doctor_lines(&home).unwrap();
        assert!(stopped && lines.len() == 1 && lines[0].contains(&job));
        CONTINUED.set(0);
        continue_due(&home, &raw::open(&home).unwrap());
        assert_eq!(CONTINUED.get(), 0);
        assert_eq!(purge(&home).unwrap().unwrap(), Purged::default());
    }

    /// D5: from the commit of its request, version 2 in both logs, a uid is passed over by every
    /// reader though knowledge.db still holds it: search, get, the viewer's claim, cite and the
    /// timeline.
    #[test]
    fn a_forgotten_uid_is_hidden_from_every_reader_at_once() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.decided("github.com/o/r", 2_000, "Use tabs for the parser.", &[]);
        let doc = s.imported(
            "o1",
            "r",
            3_000,
            "Deploy notes",
            "Deploy with the parser script.",
        );
        s.run();
        let home = s.home.path().to_owned();
        let found = |s: &crate::search::b::fixture::Store| -> Vec<String> {
            let q = crate::search::b::Query {
                text: "parser".into(),
                all: true,
                limit: 20,
                ..Default::default()
            };
            s.query(&q).hits.into_iter().map(|h| h.key).collect()
        };
        let before = found(&s);
        assert!(before.contains(&uid) && before.contains(&doc), "{before:?}");
        for u in [&uid, &doc] {
            let p = preview(&home, Target::parse_uid(u).unwrap()).unwrap();
            let (status, report) = start(&home, &p).unwrap();
            assert_eq!(status.records, 0);
            assert!(report.problems.is_empty(), "{:?}", report.problems);
        }
        let k = crate::knowledge::open(&home).unwrap();
        let held: i64 = k
            .query_row(
                "SELECT (SELECT COUNT(*) FROM claims WHERE uid = ?1)
                      + (SELECT COUNT(*) FROM imported WHERE uid = ?2)",
                [&uid, &doc],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(held, 2, "knowledge.db keeps both until the purge");
        let after = found(&s);
        assert!(!after.contains(&uid) && !after.contains(&doc), "{after:?}");
        for u in [&uid, &doc] {
            assert_eq!(crate::search::b::get(&home, u).unwrap(), None);
        }
        assert!(crate::search::b::claim(&home, &uid).unwrap().is_none());
        let cited = crate::search::b::cite(&home, std::slice::from_ref(&uid)).unwrap();
        assert_eq!(cited[0]["error"], "not a claim");
        let line = crate::search::b::timeline(&home, None, None, None, 10).unwrap();
        assert!(
            line.iter().all(|i| i.key != uid && i.key != doc),
            "{line:?}"
        );
        for log in [home.join(LOG), crate::backup::dir(&home).unwrap().join(LOG)] {
            let text = std::fs::read_to_string(&log).unwrap();
            assert_eq!(text.lines().filter(|l| l.contains("\"v\":2")).count(), 2);
            assert!(!text.contains("parser"), "{text}");
        }
    }

    /// D5: a forgotten uid comes back with a lost raw.db: from the request logs, which hold a
    /// record's request (version 1) beside it, when the backup segments predate it; then from the
    /// ops segment that holds its forget op when the logs are gone too. A reconcile adds no second
    /// op.
    #[test]
    fn a_forgotten_uid_survives_a_lost_raw_db() {
        // Imported records and documents only: a record's forget refuses a home with hook records
        // (D2).
        let mut s = crate::search::b::fixture::Store::new();
        let seq = native(&mut s.raw, "a", r#"{"prompt":"a synthetic canary"}"#);
        let uid = s.imported(
            "o1",
            "r",
            3_000,
            "Deploy notes",
            "Deploy with the parser script.",
        );
        s.run();
        let crate::search::b::fixture::Store { home: dir, raw } = s;
        let home = dir.path().to_owned();
        crate::backup::export(&home).unwrap();
        start(&home, &raw.forget_preview(record(&raw, seq)).unwrap()).unwrap();
        let p = preview(&home, Target::parse_uid(&uid).unwrap()).unwrap();
        start(&home, &p).unwrap();
        let logged = std::fs::read_to_string(home.join(LOG)).unwrap();
        assert!(
            logged.contains("\"v\":1") && logged.contains("\"v\":2"),
            "{logged}"
        );
        drop(raw);
        let lose = |home: &Path| {
            std::fs::write(home.join("raw.db"), b"not a database at all").unwrap();
            for f in ["raw.db-wal", "raw.db-shm"] {
                let _ = std::fs::remove_file(home.join(f));
            }
        };
        let forgets = |raw: &raw::Raw| {
            raw.ops_after(raw.device(), 0, 1_000)
                .unwrap()
                .iter()
                .filter(|o| o.kind == raw::OpKind::Forget)
                .count()
        };
        lose(&home);
        let mut raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forgotten(&uid).unwrap() && !shown(&raw, seq));
        reconcile(&home, &mut raw).unwrap();
        assert_eq!(forgets(&raw), 1);
        crate::backup::export(&home).unwrap();
        drop(raw);
        for log in [home.join(LOG), crate::backup::dir(&home).unwrap().join(LOG)] {
            std::fs::remove_file(log).unwrap();
        }
        lose(&home);
        let raw = crate::backup::open_raw(&home).unwrap();
        assert!(raw.forgotten(&uid).unwrap());
        assert_eq!(forgets(&raw), 1);
    }

    /// Existing request-log copies keep their hashes private even if earlier permissions were
    /// broad; the public registration path restricts the opened files before appending.
    #[cfg(unix)]
    #[test]
    fn existing_request_logs_are_private_when_registration_appends() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o644, 0o666] {
            let home = tempfile::tempdir().unwrap();
            let h = home.path();
            let mut raw = raw::open(h).unwrap();
            let seq = native(
                &mut raw,
                "existing-log-permissions",
                r#"{"prompt":"a synthetic permissions canary"}"#,
            );
            let paths = [h.join(LOG), crate::backup::dir(h).unwrap().join(LOG)];
            for path in &paths {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "").unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            let p = preview(h, record(&raw, seq)).unwrap();
            let (registered, report) = start(h, &p).unwrap();
            assert!(report.problems.is_empty(), "{report:?}");
            assert_eq!(report.copies.len(), 2);
            for path in paths {
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                    0o600,
                    "the existing request log stayed readable by other users"
                );
                let log = std::fs::read_to_string(path).unwrap();
                assert_eq!(Request::parse(log.trim()).unwrap().job, registered.job);
                assert!(!log.contains("canary"));
            }
        }
    }

    /// A malformed backup setting or unreadable config cannot undo registration or hide the
    /// missing second log copy; fixing it lets the status command reconcile that copy.
    #[test]
    fn a_backup_directory_failure_is_reported_after_registration_and_recovers() {
        for unreadable in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let h = home.path();
            let mut raw = raw::open(h).unwrap();
            let seq = native(
                &mut raw,
                "backup-log-report",
                r#"{"prompt":"a synthetic backup log canary"}"#,
            );
            let config = h.join("config.toml");
            if unreadable {
                std::fs::create_dir(&config).unwrap();
            } else {
                std::fs::write(&config, "[backup]\ndir = 42\n").unwrap();
            }
            let p = preview(h, record(&raw, seq)).unwrap();
            let (registered, report) = start(h, &p).expect("the request must stay registered");
            assert_eq!(registered.records, 1);
            assert_eq!(registered.local, "hidden; physical purge pending");
            assert_eq!(status(h).unwrap()[0].job, registered.job);
            assert!(!shown(&raw, seq));
            let primary = h.join(LOG);
            assert_eq!(report.copies, vec![primary.clone()]);
            let log = std::fs::read_to_string(&primary).unwrap();
            assert_eq!(log.lines().count(), 1);
            assert_eq!(Request::parse(log.trim()).unwrap().job, registered.job);
            assert!(!log.contains("canary"));
            assert_eq!(
                report.problems.len(),
                1,
                "backup directory resolution failed without warning: {report:?}"
            );
            assert!(
                report.problems[0].contains("backup request log directory could not be resolved")
            );
            // Hooks use raw directly: a copy problem must not stop a new event.
            raw::open(h)
                .unwrap()
                .append(&raw::test_event("a synthetic event after the log warning"))
                .unwrap();
            if unreadable {
                std::fs::remove_dir(&config).unwrap();
            }
            std::fs::write(&config, "[backup]\ndir = 'repaired-backups'\n").unwrap();
            run(h, None, None, None, false, true, false).unwrap();
            let second = h.join("repaired-backups").join(LOG);
            assert_eq!(std::fs::read_to_string(second).unwrap(), log);
            assert_eq!(std::fs::read_to_string(primary).unwrap(), log);
            assert_eq!(status(h).unwrap()[0].job, registered.job);
            assert_eq!(
                status(h).unwrap()[0].local,
                "hidden; physical purge pending"
            );
            assert!(reconcile(h, &mut raw).unwrap().problems.is_empty());
        }
    }

    /// Registration committed even if a later reconciliation cannot read another job. The
    /// caller receives its true registered state and the log problem, never "not registered".
    #[test]
    fn a_registered_request_keeps_its_status_when_later_reconciliation_fails() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let seq = native(
            &mut raw,
            "registered",
            r#"{"prompt":"a synthetic registered request"}"#,
        );
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO forget_jobs(id, request, started, step) VALUES('broken', 'not JSON', 0, 1)",
                [],
            )
            .unwrap();
        let (status, report) = start(home.path(), &p).expect("the request was already registered");
        assert_eq!(status.records, 1);
        assert_eq!(status.local, "hidden; physical purge pending");
        assert!(!report.problems.is_empty());
        assert!(!shown(&raw, seq));
    }

    /// The importer may have prepared a transcript batch before forget registered its cut.
    /// The append's transaction checks the current request, even with another source origin.
    #[test]
    fn a_prepared_transcript_batch_overlapping_a_legacy_forget_is_refused_before_the_cut() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let mut event = raw::test_event(r#"{"prompt":"a synthetic forgotten prompt"}"#);
        event.source = "oboete-v1".into();
        event.ts = 1_001;
        let identity = raw::ImportIdentity {
            origin: origin("synthetic", "native-cut"),
            session: session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event: event.clone(),
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        event.source = "transcript".into();
        event.ts -= 1;
        assert_eq!(
            raw.transcript_cut(&event.agent, &event.session, None)
                .unwrap(),
            None
        );
        // A request accepted before the cross-source check: replay must retain it, including
        // when this transcript batch was prepared before the request reached raw authority.
        let request = Request {
            v: 1,
            home: raw.home_id().to_owned(),
            job: "b1000000000000000000000000000001".into(),
            started: 1_000,
            target: record(&raw, seq),
            records: vec![Record {
                device: raw.device().into(),
                seq,
                origin: origin("synthetic", "native-cut"),
                session: session(&event.agent, &event.session),
                ts: Some(event.ts + 1),
            }],
        };
        assert_eq!(raw.forget_apply(&[request]).unwrap(), 1);
        let before = raw.after(raw.device(), 0, 100).unwrap().len();
        let append = |raw: &mut raw::Raw, event: raw::Event, id: &str| {
            let identity = raw::ImportIdentity {
                origin: origin("synthetic-transcript", id),
                session: session(&event.agent, &event.session),
                ambiguous: None,
                unverified: false,
            };
            raw.append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
        };
        let refused = append(&mut raw, event.clone(), "skewed");
        assert!(
            refused.is_err(),
            "a prepared transcript batch restored an earlier-stamped copy: {refused:?}"
        );
        assert!(format!("{:#}", refused.unwrap_err()).contains("cross-source event identity"));
        assert_eq!(raw.after(raw.device(), 0, 100).unwrap().len(), before);
        event.session = "unrelated-native-session".into();
        event.body = r#"{"prompt":"unrelated history"}"#.into();
        let kept = append(&mut raw, event, "unrelated").unwrap();
        assert_eq!(kept.len(), 1);
        assert!(shown(&raw, kept[0]));
    }

    /// Rule 5: a request hides every record of its origin in the store it is applied to, at
    /// whatever seq it has, and never another record that took the old seq.
    #[test]
    fn a_request_follows_its_origin_and_never_a_reused_seq() {
        let first = tempfile::tempdir().unwrap();
        let mut raw = raw::open(first.path()).unwrap();
        let seq = native(&mut raw, "a", r#"{"prompt":"forgotten"}"#);
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        let (_, _) = start(first.path(), &p).unwrap();
        let request = raw.forget_requests().unwrap().remove(0);
        // Another store of the same device where the seq holds another record, and the origin
        // came in later at another seq.
        let second = tempfile::tempdir().unwrap();
        std::fs::copy(first.path().join("raw.db"), second.path().join("raw.db")).unwrap();
        let mut other = raw::open(second.path()).unwrap();
        let conn = rusqlite::Connection::open(second.path().join("raw.db")).unwrap();
        conn.execute_batch(
            "DELETE FROM records; DELETE FROM import_origins; DELETE FROM denied_records;
             DELETE FROM forget_jobs;",
        )
        .unwrap();
        let reused = native(&mut other, "b", r#"{"prompt":"took the seq"}"#);
        assert_eq!(reused, seq);
        let moved = native(&mut other, "a", r#"{"prompt":"forgotten"}"#);
        assert_eq!(
            other.forget_apply(std::slice::from_ref(&request)).unwrap(),
            1
        );
        assert!(shown(&other, reused));
        assert!(!shown(&other, moved));
        // Applied once.
        assert_eq!(other.forget_apply(&[request]).unwrap(), 0);
    }

    /// Rule 12's count is in the preview: a forget between a preview and its start makes it
    /// stale, and nothing is registered.
    #[test]
    fn a_preview_read_before_another_forget_is_stale() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let a = native(&mut raw, "a", r#"{"prompt":"one"}"#);
        let b = native(&mut raw, "b", r#"{"prompt":"two"}"#);
        let late = raw.forget_preview(record(&raw, a)).unwrap();
        let p = raw.forget_preview(record(&raw, b)).unwrap();
        start(home.path(), &p).unwrap();
        let e = start(home.path(), &late).unwrap_err();
        assert!(e.to_string().contains("stale"), "{e:#}");
        assert_eq!(raw.forget_requests().unwrap().len(), 1);
        assert!(shown(&raw, a));
    }

    /// Rule 9: a torn line is skipped alone, and the next append starts on a line of its own.
    #[test]
    fn a_torn_line_is_skipped_alone_and_the_next_line_starts_on_its_own() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let a = native(&mut raw, "a", r#"{"prompt":"one"}"#);
        start(home.path(), &raw.forget_preview(record(&raw, a)).unwrap()).unwrap();
        let path = home.path().join(LOG);
        let whole = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 20]).unwrap();
        let b = native(&mut raw, "b", r#"{"prompt":"two"}"#);
        start(home.path(), &raw.forget_preview(record(&raw, b)).unwrap()).unwrap();
        let mut report = Report::default();
        let read = read_log(&path, Some(raw.device()), &mut report).unwrap();
        // The torn first line is skipped; the second request, and the first written again from
        // raw.db by the reconcile after it, read.
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        let jobs: std::collections::HashSet<String> = read.into_iter().map(|r| r.job).collect();
        assert_eq!(jobs.len(), 2);
    }

    /// "A line names its home": a request of another home in a log is reported, not applied.
    #[test]
    fn a_line_of_another_home_is_not_applied() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let a = native(&mut raw, "a", r#"{"prompt":"one"}"#);
        let mut p = raw.forget_preview(record(&raw, a)).unwrap();
        p.validate().unwrap();
        let foreign = Request {
            v: 1,
            home: "elsewhere".into(),
            job: "0".repeat(32),
            started: 1,
            target: p.target.clone(),
            records: std::mem::take(&mut p.records),
        };
        append_log(&home.path().join(LOG), &[&foreign]).unwrap();
        let report = reconcile(home.path(), &mut raw).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        assert!(shown(&raw, a));
    }
}
