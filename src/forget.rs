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

/// A selection past [`MAX_RECORDS`], its copies counted: the page names it, since only its
/// session's forget can take it (parity row 3).
#[derive(Debug)]
pub(crate) struct TooLarge;

impl std::fmt::Display for TooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "split this selection into spans of at most {MAX_RECORDS} records, their copies counted"
        )
    }
}

impl std::error::Error for TooLarge {}

/// A record of another device's: a record's forget takes only this device's, its session's any
/// device's, which the page names (parity row 3).
#[derive(Debug)]
pub(crate) struct OtherDevice;

impl std::fmt::Display for OtherDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this forget slice accepts only this device's raw records")
    }
}

impl std::error::Error for OtherDevice {}
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
    /// Slice 3b: a session, by its agent and its hash (`session`), never its id: every source's
    /// records of it, and later records of it from before its registration (D3).
    Session {
        agent: String,
        session: String,
    },
    /// Slice 3b-2: a repository's records on every device, by its key's hash (`repo`), never the
    /// key. `device` registered it: its records from before the registration that arrive later are
    /// refused (D3); another device's, past what the preview confirmed of it, are not.
    Repo {
        repo: String,
        device: String,
    },
    /// Slice 3b-2: this device's records with a time in `since..until` (ms, `until` not in it, as
    /// search reads them), and its records of that time that arrive later (D3).
    Range {
        device: String,
        since: i64,
        until: i64,
    },
}

impl Target {
    pub fn bounds(&self) -> Result<(&str, i64, i64)> {
        let (device, from, to) = match self {
            Self::Record { device, seq } => (device.as_str(), *seq, *seq),
            Self::Span { device, from, to } => (device.as_str(), *from, *to),
            Self::Uid { .. } => anyhow::bail!("a uid names no record span"),
            Self::Session { .. } => anyhow::bail!("a session names no record span"),
            Self::Repo { .. } => anyhow::bail!("a repository names no record span"),
            Self::Range { .. } => anyhow::bail!("a time range names no record span"),
        };
        anyhow::ensure!(device_like(device), "invalid device identity");
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

    /// Slice 3b: `--session` takes a record's id from search, timeline or get, `<device>:<seq>`:
    /// no output shows a session's own id.
    pub fn parse_handle(text: &str) -> Result<(String, i64)> {
        let (device, seq) = text
            .split_once(':')
            .context("expected a record's id, <device>:<seq>")?;
        anyhow::ensure!(device_like(device), "invalid device identity");
        let seq: i64 = seq
            .parse()
            .context("expected a record's id, <device>:<seq>")?;
        anyhow::ensure!(seq > 0, "invalid record span");
        Ok((device.into(), seq))
    }

    /// Slice 3b-2: a repository's or a time range's target as a request holds it.
    fn check_scope(&self) -> Result<()> {
        match self {
            Self::Repo { repo, device } => {
                check_identity(repo)?;
                anyhow::ensure!(device_like(device), "invalid device identity");
            }
            Self::Range {
                device,
                since,
                until,
            } => {
                anyhow::ensure!(device_like(device), "invalid device identity");
                anyhow::ensure!(since < until, "invalid time range");
            }
            _ => anyhow::bail!("not a repository or a time range"),
        }
        Ok(())
    }
}

/// Slice 3b-2: a record's session as a repository's or a time range's page holds it: a hash, or
/// none for a record that names no session of its agent's own.
fn check_named_session(session: &str) -> Result<()> {
    if session.is_empty() {
        return Ok(());
    }
    check_identity(session)
}

/// An agent's name as a session target holds it: the hook's own argument, ASCII, bounded.
fn agent_like(agent: &str) -> bool {
    !agent.is_empty()
        && agent.len() <= 64
        && agent
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A device's id as a target and a request name it: ASCII letters and digits, bounded.
fn device_like(device: &str) -> bool {
    !device.is_empty() && device.len() <= 128 && device.bytes().all(|b| b.is_ascii_alphanumeric())
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
    /// The backup segments its purge would rewrite or remove (2c, decision 9); None when the
    /// backup location cannot be read now.
    pub segments: Option<crate::backup::OpsPurge>,
    /// The copies of raw.db set aside, each rewritten where it holds the uid or removed when it
    /// cannot be.
    pub copies: usize,
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
    /// Slice 3a: the records of the same sessions from another source, which may hold a copy of
    /// the target's: forgotten with it.
    pub(crate) copies: Vec<Record>,
    /// The target's records with neither text nor an identity, passed over: a restore kept no
    /// body of them, nor the labels their session would be read from.
    pub passed: usize,
    /// How many deny rows raw held when it was read: the fence's count (rule 12).
    pub(crate) denied: i64,
    /// The sources of the records, for the limits forget prints.
    pub(crate) sources: Vec<String>,
    pub sample: Option<String>,
    /// A uid target's (D5).
    pub uid: Option<UidPreview>,
    /// A session's, a repository's or a time range's target (slice 3b).
    pub scope: Option<ScopePreview>,
    /// Slice 3d-1: the claims that quote one of the records, forgotten by uid with them.
    pub claims: usize,
    /// Slice 3d-2: the windows whose cards, and the turns whose session summaries, lose their
    /// text: what a curator was shown of the records or of those claims.
    pub cards: usize,
    pub summaries: usize,
    /// Slice 3d-4: the claims those windows wrote, which lose their text with their cards until a
    /// recuration writes them again.
    pub written: usize,
}

/// What a session, repository or time range target resolved to (slice 3b, spec 6.2's preview):
/// when its first record was recorded, its records by source and how many devices', and how many
/// records that name no session would stay holding a copy (a session's: its agent's; a
/// repository's or a time range's: its own, whose copies cannot be found); a session's agent; a
/// repository's keys as its records name it, the paths they were recorded at, and how many
/// records' repository is masked (not taken: whether they are its cannot be told).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScopePreview {
    pub agent: Option<String>,
    pub first: Option<i64>,
    pub sources: Vec<(String, usize)>,
    pub devices: usize,
    pub idless: usize,
    pub keys: Vec<String>,
    pub paths: Vec<String>,
    pub masked: usize,
}

impl Preview {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Target::Uid { uid } = &self.target {
            well_formed(uid)?;
            anyhow::ensure!(
                self.records.is_empty() && self.copies.is_empty() && self.uid.is_some(),
                "forget selection differs from its preview"
            );
            return Ok(());
        }
        if let Target::Session { agent, session } = &self.target {
            anyhow::ensure!(agent_like(agent), "invalid agent");
            check_identity(session)?;
            anyhow::ensure!(
                !self.records.is_empty(),
                "no raw records matched; nothing was registered"
            );
            anyhow::ensure!(
                self.copies.is_empty() && self.records.iter().all(|r| &r.session == session),
                "forget selection differs from its preview"
            );
            for r in &self.records {
                anyhow::ensure!(device_like(&r.device), "invalid device identity");
                check_identity(&r.origin)?;
            }
            return Ok(());
        }
        if let Target::Repo { .. } | Target::Range { .. } = &self.target {
            self.target.check_scope()?;
            anyhow::ensure!(
                !self.records.is_empty(),
                "no raw records matched; nothing was registered"
            );
            for r in self.records.iter().chain(&self.copies) {
                anyhow::ensure!(device_like(&r.device), "invalid device identity");
                check_identity(&r.origin)?;
                check_named_session(&r.session)?;
            }
            return Ok(());
        }
        let (device, from, to) = self.target.bounds()?;
        anyhow::ensure!(
            !self.records.is_empty(),
            "no raw records matched; nothing was registered"
        );
        if self.records.len() + self.copies.len() > MAX_RECORDS {
            return Err(TooLarge.into());
        }
        for r in &self.records {
            anyhow::ensure!(
                r.device == device && (from..=to).contains(&r.seq),
                "forget selection differs from its preview"
            );
        }
        for r in self.records.iter().chain(&self.copies) {
            anyhow::ensure!(device_like(&r.device), "invalid device identity");
            check_identity(&r.origin)?;
            check_identity(&r.session)?;
        }
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// Slice 3a: how many records of other sources it takes as copies.
    pub fn copies(&self) -> usize {
        self.copies.len()
    }

    /// The same records, under the same deny-list: a start that reads another is stale.
    pub(crate) fn token(&self) -> Result<String> {
        Ok(hash(&serde_json::to_vec(&(
            &self.target,
            &self.records,
            &self.copies,
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
        // Version 2 is a uid's (D5), version 3 a span's with the copies of other sources outside it
        // (slice 3a); an older binary skips its line as of an unknown version.
        let copies = match (&self.target, self.v) {
            (Target::Uid { uid }, 2) => {
                well_formed(uid)?;
                anyhow::ensure!(self.records.is_empty(), "invalid record count");
                return Ok(());
            }
            (Target::Record { .. } | Target::Span { .. }, 1) => false,
            (Target::Record { .. } | Target::Span { .. }, 3) => true,
            // Version 4 is one page of a session's records (slice 3b), of any device.
            (Target::Session { agent, session }, 4) => {
                anyhow::ensure!(agent_like(agent), "invalid agent");
                check_identity(session)?;
                anyhow::ensure!(
                    !self.records.is_empty() && self.records.len() <= MAX_RECORDS,
                    "invalid record count"
                );
                for r in &self.records {
                    anyhow::ensure!(
                        device_like(&r.device) && &r.session == session,
                        "a record outside its target"
                    );
                    check_identity(&r.origin)?;
                }
                return Ok(());
            }
            // Version 5 is one page of a repository's or a time range's records with their copies
            // (slice 3b-2), of any device; a record that names no session has none.
            (Target::Repo { .. } | Target::Range { .. }, 5) => {
                self.target.check_scope()?;
                anyhow::ensure!(
                    !self.records.is_empty() && self.records.len() <= MAX_RECORDS,
                    "invalid record count"
                );
                for r in &self.records {
                    anyhow::ensure!(device_like(&r.device), "invalid device identity");
                    check_identity(&r.origin)?;
                    check_named_session(&r.session)?;
                }
                return Ok(());
            }
            _ => anyhow::bail!("unknown request version {}", self.v),
        };
        let (device, from, to) = self.target.bounds()?;
        anyhow::ensure!(
            !self.records.is_empty() && self.records.len() <= MAX_RECORDS,
            "invalid record count"
        );
        for r in &self.records {
            // Version 3's copies are of any device whose records this store holds.
            anyhow::ensure!(
                device_like(&r.device)
                    && (copies || (r.device == device && (from..=to).contains(&r.seq))),
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
    /// Slice 3b: a session's registration is pages of at most `MAX_RECORDS`, a job each.
    #[serde(skip_serializing_if = "one")]
    pub pages: usize,
}

fn one(n: &usize) -> bool {
    *n == 1
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
        target => {
            raw.session_indexes()?;
            if let Target::Repo { .. } = target {
                raw.repo_index()?;
            }
            let mut preview = raw.forget_preview(target)?;
            raw.scope_summary(&mut preview)?;
            // Read from raw's op log, as the purge reads them: knowledge.db may not hold them yet,
            // or not be there (CodeRabbit on #471). Not by a record a curator is shown no line of,
            // as the purge judges (`raw::LINELESS`).
            let keys = raw.with_lines(
                preview
                    .records
                    .iter()
                    .chain(&preview.copies)
                    .map(|r| (r.device.clone(), r.seq))
                    .collect(),
            )?;
            let quoting = raw.record_claims(&crate::raw::Forgotten::of(
                keys.iter().cloned(),
                std::iter::empty(),
            ))?;
            preview.claims = quoting.uids.len();
            let mut made = crate::raw::Forgotten::of(keys, quoting.uids.into_keys());
            made.texts = quoting.texts;
            let made = raw.record_ops(made)?.0;
            (preview.cards, preview.summaries) = (made.windows.len(), made.turns.len());
            preview.written = made
                .windows
                .iter()
                .filter_map(|(device, op_seq, _)| made.claims.get(&(device.clone(), *op_seq)))
                .map(|c| c.ops.len())
                .sum();
            Ok(preview)
        }
    }
}

/// Slice 3b: the session the record `handle` (`<device>:<seq>`) was recorded in, by its agent and
/// its hash. A live record whose label is not its agent's own id names none.
pub fn session_target(home: &Path, handle: &str) -> Result<Target> {
    anyhow::ensure!(crate::raw::exists(home), "no raw store to forget from");
    let (device, seq) = Target::parse_handle(handle)?;
    let (agent, session) = crate::raw::open(home)?.session_of(&device, seq)?;
    Ok(Target::Session { agent, session })
}

/// Slice 3b-2: `--repo` takes a directory, read as capture reads one (`repo::key`), or a key as
/// records name it; the request keeps its hash, registered by this device.
pub fn repo_target(home: &Path, text: &str) -> Result<Target> {
    anyhow::ensure!(crate::raw::exists(home), "no raw store to forget from");
    let dir = Path::new(text);
    let key = if dir.is_dir() {
        crate::repo::key(dir)
    } else {
        text.to_owned()
    };
    anyhow::ensure!(!key.trim().is_empty(), "a repository's key is empty");
    Ok(Target::Repo {
        repo: repo(&key),
        device: crate::raw::open(home)?.device().to_owned(),
    })
}

/// Slice 3b-2: `--since` and `--until` as search reads them (`search::b::time`), this device's.
pub fn range_target(home: &Path, since: &str, until: &str) -> Result<Target> {
    anyhow::ensure!(crate::raw::exists(home), "no raw store to forget from");
    let (since, until) = (
        crate::search::b::time(since, false)?,
        crate::search::b::time(until, true)?,
    );
    anyhow::ensure!(since < until, "--since is not before --until");
    Ok(Target::Range {
        device: crate::raw::open(home)?.device().to_owned(),
        since,
        until,
    })
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
                    segments: None,
                    copies: 0,
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
                    segments: None,
                    copies: 0,
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
                segments: None,
                copies: 0,
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
    // 2c, decision 9: what step 4 would rewrite or remove (a placeholder job: only the count
    // matters here).
    let only = std::collections::HashMap::from([(uid.clone(), String::new())]);
    uid_preview.segments = crate::backup::ops_to_purge(home, &only).ok();
    uid_preview.copies = crate::raw::copies_set_aside(home)?.len();
    Ok(Preview {
        target: Target::Uid { uid },
        records: Vec::new(),
        copies: Vec::new(),
        passed: 0,
        denied: raw.denied_count()?,
        sources: Vec::new(),
        sample: sample.map(|s| line(&s)),
        uid: Some(uid_preview),
        scope: None,
        claims: 0,
        cards: 0,
        summaries: 0,
        written: 0,
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
    let mut job = || -> Result<String> {
        let mut id = [0_u8; 16];
        getrandom::fill(&mut id)?;
        Ok(id.iter().map(|b| format!("{b:02x}")).collect())
    };
    // A session's records are pages of at most MAX_RECORDS, a job each, registered together.
    let requests = raw.forget_start(preview, &mut job, crate::db::now_ms())?;
    // It is registered already. A later log failure is reported alongside that true status.
    let report = reconcile(home, &mut raw).unwrap_or_else(|e| Report {
        problems: vec![format!("the request logs were not reconciled: {e:#}")],
        ..Report::default()
    });
    let status = grouped(requests.iter().map(|r| status_of(r, PENDING)))
        .pop()
        .context("no request was registered")?;
    Ok((status, report))
}

const PENDING: &str = "hidden; physical purge pending";
/// D5 item 7: past its rebuild, knowledge.db still holds the uid.
const STOPPED: &str = "stopped: knowledge.db still holds it after its rebuild";
/// 2c: past step 4, no backup segment or copy of raw.db holds its text; step 5's last look at the
/// hook state is to come (it follows at once, unless the purge stopped between the two).
const BACKUPS_PURGED: &str = "purged here and in the backups; its last check pending";
/// 2c, decision 7: past step 5.
const DONE: &str = "done";
/// Slice 3c: a record job past its rebuild with no row, claim, window or turn op holding its text
/// (slice 3d); its backups are to come (D4: no more than is true). Past step 4 it is a uid job's
/// `BACKUPS_PURGED`, past step 5 `DONE`.
const RAW_PURGED: &str =
    "purged from raw and its index here, with what was made from it; the backups pending";

/// Each job, its local state read from the stores (D5 item 9): a uid job past step 3 whose ops
/// hold no text is purged here, or stopped while knowledge.db still holds its uid.
pub fn status(home: &Path) -> Result<Vec<Status>> {
    if !crate::raw::exists(home) {
        return Ok(Vec::new());
    }
    let raw = crate::raw::open(home)?;
    let records = record_jobs_with_text(&raw)?;
    let mut past = Vec::new();
    for (request, step) in raw.forget_jobs()? {
        let purged = match &request.target {
            Target::Uid { uid } => step >= 3 && !raw.uid_has_ops(uid)?,
            _ => step >= 3 && !records.contains(&request.job),
        };
        past.push((request, step, purged));
    }
    let uids: HashSet<String> = past
        .iter()
        .filter_map(|(r, _, purged)| match &r.target {
            Target::Uid { uid } if *purged => Some(uid.clone()),
            _ => None,
        })
        .collect();
    let holding = knowledge_holding(home, &uids)?;
    Ok(grouped(past.iter().map(|(r, step, purged)| {
        let local = match &r.target {
            Target::Uid { uid } if *purged && holding.contains(uid) => STOPPED,
            Target::Uid { .. } if *purged && *step >= 5 => DONE,
            Target::Uid { .. } if *purged && *step >= 4 => BACKUPS_PURGED,
            Target::Uid { .. } if *purged => "purged here; backups pending",
            _ if *purged && *step >= 5 => DONE,
            _ if *purged && *step >= 4 => BACKUPS_PURGED,
            _ if *purged => RAW_PURGED,
            _ => PENDING,
        };
        status_of(r, local)
    })))
}

fn status_of(r: &Request, local: &'static str) -> Status {
    Status {
        job: r.job.clone(),
        target: r.target.clone(),
        records: r.records.len(),
        started: r.started,
        local,
        pages: 1,
    }
}

/// Slice 3b: the pages of one registration (one target, one start), which follow one another in
/// `forget_jobs`, as one status: the first page's job, every page's records.
fn grouped(statuses: impl IntoIterator<Item = Status>) -> Vec<Status> {
    let mut out: Vec<Status> = Vec::new();
    for s in statuses {
        match out.last_mut() {
            Some(last)
                if last.target == s.target
                    && last.started == s.started
                    && last.local == s.local =>
            {
                last.records += s.records;
                last.pages += 1;
            }
            _ => out.push(s),
        }
    }
    out
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
    let mut lines = lines;
    // Slice 3e-1: not stopped, owed (a continuation or the worker's next look finishes it).
    if crate::raw::exists(home) && crate::raw::open(home)?.masks_owed()?.is_some() {
        lines.push(
            "redaction masks: a rescan's masks are not yet in raw.db's rows, knowledge.db and the \
             backups; `oboete forget --continue` puts them there"
                .to_owned(),
        );
    }
    Ok((lines, stopped))
}

/// How long a purge tries to truncate raw.db-wal past the readers that hold it (D5, 2b).
#[cfg(not(test))]
pub(crate) const TRUNCATE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(test)]
pub(crate) const TRUNCATE_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// What a purge did (D5, 2b).
#[derive(Debug, Default, PartialEq)]
pub struct Purged {
    /// The jobs it went on with: a uid job not past step 5 or whose ops hold their text again, a
    /// record job not past step 3 or whose rows hold it again (slice 3c).
    pub jobs: usize,
    /// Ops rewritten without their text: a forgotten uid's, and those of the claims quoting a
    /// forgotten record (slice 3d-1).
    pub ops: usize,
    /// Rows of forgotten records rewritten without their text (slice 3c).
    pub records: usize,
    /// Rows of records a rescan masked, rewritten with the masks (slice 3e-1).
    pub masked: usize,
    /// Whether it went on with a rescan's masks (slice 3e-1), or with op text owed a scan with
    /// the redaction rules now (slice 3e-4): what `forget --continue` reports with no job.
    pub masks: bool,
    /// Ops whose text the redaction rules now masked in raw.db (slice 3e-4).
    pub texts: usize,
    /// Preferences written again from the masked text of their directive (slice 3e-2).
    pub preferences: usize,
    /// Jobs not done this time: readers kept raw.db-wal from truncating, or a restore put the
    /// text back meanwhile.
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
    /// Backup segments whose ops of a forgotten uid (2c) or lines of a forgotten record (slice
    /// 3c-2) were rewritten without their text.
    pub segments: usize,
    /// Backup segments removed: no seal vouched for them and they could not be read whole (2c).
    pub segments_removed: usize,
    /// Copies of raw.db set aside whose ops of a forgotten uid or rows of a forgotten record were
    /// rewritten without their text (2c, slice 3c-2).
    pub copies: usize,
    /// Copies of raw.db removed: they did not open or could not be rewritten, and a restore's
    /// partial build (2c).
    pub copies_removed: usize,
}

/// A restore (spec 6.3): every job whose text came back owes its rebuild again (step 2, as its purge
/// records when it finds the text), now rather than at that purge's next look, so until the purge
/// marks what it took again, the readers of what was made from its records go by its tombstones
/// (`Raw::forget_unrebuilt`, docs/my-work-model.md S5).
pub(crate) fn owe_rebuild_again(raw: &crate::raw::Raw) -> Result<()> {
    let holding = record_jobs_with_text(raw)?;
    for (request, step) in raw.forget_jobs()? {
        let held = match &request.target {
            Target::Uid { uid } => raw.uid_has_ops(uid)?,
            _ => holding.contains(&request.job),
        };
        if held && step > 2 {
            raw.restart_forget_step(&request.job, 2)?;
        }
    }
    Ok(())
}

/// Each uid job a purge goes on with, by state rather than step (D5 item 7): one not past step 5,
/// or whose ops hold text again (a restore can bring the bodies back): its job, uid and step, and
/// whether its ops hold text.
fn due(raw: &crate::raw::Raw) -> Result<Vec<(String, String, i64, bool)>> {
    let mut due = Vec::new();
    for (request, step) in raw.forget_jobs()? {
        if let Target::Uid { uid } = request.target {
            let held = raw.uid_has_ops(&uid)?;
            if held || step < 5 {
                due.push((request.job, uid, step, held));
            }
        }
    }
    Ok(due)
}

/// The record jobs a forgotten record of which still has its body (slice 3c), or what was made
/// from one still holds its text: a claim's ops (3d-1), a window or turn op (3d-2). What forget
/// took is read once, and each kind of op once.
fn record_jobs_with_text(raw: &crate::raw::Raw) -> Result<HashSet<String>> {
    let mut jobs = raw.record_jobs_holding()?;
    let mut forgotten = raw.forgotten_records()?;
    let claims = raw.record_claims(&forgotten)?;
    jobs.extend(claims.uids.values().cloned());
    // A claim its purge has not tombstoned yet is one a window may have shown too.
    forgotten.uids.extend(claims.uids);
    let (ops, _) = raw.record_ops(forgotten)?;
    jobs.extend(
        ops.windows
            .into_iter()
            .chain(ops.turns)
            .chain(ops.my_work)
            .map(|(_, _, job)| job),
    );
    // Slice 3d-4: and a window's claims that hold text, of a window rewritten before too.
    jobs.extend(ops.claims.into_values().map(|c| c.job));
    Ok(jobs)
}

/// Each record job a purge goes on with (slice 3c), by state as a uid job: one not past step 5,
/// or a forgotten record of which, or a claim made from one, holds its text again: its job and
/// step, and whether one does.
/// A denial whose record holds its text with no job row (a restore from segments alone brings
/// back the denial, not the job: spec 6.3) is its job's, at step 0: the purge goes on with it by
/// the state, with no step to record (Codex on #469).
fn records_due(raw: &crate::raw::Raw) -> Result<Vec<(String, i64, bool)>> {
    let mut holding = record_jobs_with_text(raw)?;
    let mut due = Vec::new();
    for (request, step) in raw.forget_jobs()? {
        if !matches!(request.target, Target::Uid { .. }) {
            let held = holding.remove(&request.job);
            if held || step < 5 {
                due.push((request.job, step, held));
            }
        }
    }
    due.extend(holding.into_iter().map(|job| (job, 0, true)));
    Ok(due)
}

/// The `uids` knowledge.db still holds a row of (D5 item 7): a claim, a derivation, a correction,
/// an imported document or a vector key.
fn knowledge_holding(home: &Path, uids: &HashSet<String>) -> Result<HashSet<String>> {
    let path = home.join("knowledge.db");
    let mut holding = HashSet::new();
    // Only a file that is not there is none: one whose metadata cannot be read may hold them
    // (Codex on #444).
    if uids.is_empty() || !path.try_exists()? {
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
        // A document's files (docs/file-note.md F12): they name what it read.
        ("imported_files", "uid = ?1"),
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
pub(crate) fn aside_files(home: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    // A listing or an entry that cannot be read may hide one: an error then, so step 3 waits
    // (Codex on #444).
    for entry in std::fs::read_dir(home)? {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(|n| {
            n.starts_with("knowledge.db.quarantined-") || n.starts_with("knowledge.db.rebuilding-")
        }) {
            files.push(entry.path());
        }
    }
    Ok(files)
}

/// `state/purge.lock`, held: one purge or retention pass at a time (docs/retention.md); none when
/// another holds it.
pub(crate) fn purge_lock(home: &Path) -> Result<Option<std::fs::File>> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("purge.lock"))?;
    match lock.try_lock() {
        Ok(()) => {
            // When the pause it causes began, for doctor and the page (#855).
            crate::worker::stamp(&lock);
            Ok(Some(lock))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// D5 (2b): each uid job purged by the state it finds (item 7). Step 2: its ops that hold its text
/// rewritten without it and raw.db-wal truncated, under the worker lock as `oboete rebuild` takes
/// it. Step 3: knowledge.db rebuilt from raw.db once for every job not past step 3 (item 4),
/// the copies of knowledge.db set aside carried from by the same rule and removed (item 5), and
/// the uids taken out of every session's shown value (item 6). A uid knowledge.db holds after the
/// rebuild came from an op the purge did not find: its job is stopped, reported and not done.
/// Only one runs at a time (`state/purge.lock`, not waited for: None when another holds it).
pub fn purge(home: &Path) -> Result<Option<Purged>> {
    let Some(_lock) = purge_lock(home)? else {
        return Ok(None);
    };
    let mut purged = Purged::default();
    if !crate::raw::exists(home) {
        return Ok(Some(purged));
    }
    let (due, records_due, owed, masks, mut text) = {
        let raw = crate::raw::open(home)?;
        let owed = raw.record_backups_owed()?;
        let text = text_owed(home, &raw)?;
        (
            due(&raw)?,
            records_due(&raw)?,
            owed,
            raw.masks_owed()?,
            text,
        )
    };
    purged.jobs = due.len() + records_due.len();
    purged.masks = masks.is_some() || text.is_some();
    if purged.jobs == 0 && !owed && !purged.masks {
        return Ok(Some(purged));
    }
    // Slice 3e-1: whether the ranges a rescan masked reached raw.db's rows, what was made from them,
    // raw.db-wal and knowledge.db: they are paid only then, and after step 4. Slice 3e-4: and the
    // op text masked with the rules now, whether raw.db-wal was truncated after it.
    let mut masked = masks.is_none();
    let mut text_truncated = false;
    // Owner decision 43: this purge's own mark on the scan of the op text it begins.
    let mut text_mark: Option<String> = None;
    // Step 2 for the jobs whose ops (a record job: whose rows, slice 3c) hold text, or that never
    // took it; the rebuild for those and the ones stopped after step 2. A job past step 3 does not
    // go back to the rebuild (2c, decision 8): a stopped one would rebuild at every continuation.
    let (mut rebuild, mut record_rebuild) = (Vec::new(), Vec::new());
    let step_2 = |step: i64, held: bool| step < 2 || held;
    let records_step_2 = records_due
        .iter()
        .any(|(_, step, held)| step_2(*step, *held));
    if records_step_2
        || masks.is_some()
        || text.is_some()
        || due.iter().any(|(_, _, step, held)| step_2(*step, *held))
    {
        let _worker = crate::worker::lock_asking(home)?;
        let mut raw = crate::raw::open(home)?;
        #[cfg(test)]
        if let Some(meanwhile) = AFTER_PURGE_LOCK.take() {
            meanwhile();
        }
        // The rules the op text owes its scan to, read again under the worker lock: rules changed
        // while it was awaited (redaction turned off, say) mask nothing with the ones before, and
        // the scan stays owed to the rules now (owner decision 43; Codex on #502).
        if let Some(rules) = &text
            && text_owed(home, &raw)?.is_none_or(|now| now.version() != rules.version())
        {
            text = None;
        }
        for (job, uid, step, held) in &due {
            if step_2(*step, *held) {
                purged.ops += raw.purge_uid(uid, job)?;
            }
        }
        if records_step_2 || masks.is_some() {
            raw.owe_record_backups(true)?;
            let forgotten = |raw: &crate::raw::Raw| {
                if masks.is_some() {
                    raw.forgotten_and_masked()
                } else {
                    raw.forgotten_records()
                }
            };
            // What was made from them first: a claim the anchors consumer keeps (its active
            // derivation quotes another record) is shown until its uid is tombstoned, and the rows
            // can take minutes. The claims before the window and turn ops: a window that showed
            // one is found by its forget op (3d-2). Slice 3e-1: a masked record's claim, whose uid
            // is not tombstoned, after them: until then its op is how they are found. Slice 3d-4:
            // all of it judged before any is written, while the claims still hold the text a
            // window's claims may have copied.
            let mut shown = forgotten(&raw)?;
            let mut claims = raw.record_claims(&shown)?;
            shown.uids.extend(claims.uids.clone());
            // #483: kept before any of their ops is rewritten, with those a stopped purge kept.
            shown.masked = raw.keep_masked_uids(&claims.masked_uids)?;
            shown.texts = std::mem::take(&mut claims.texts);
            let made = raw.record_ops(shown)?.0;
            purged.ops += raw.purge_record_claims(&claims)?;
            purged.ops += raw.purge_record_ops(&made)?;
            purged.ops += raw.purge_masked_claims(&claims.masked)?;
            // Slice 3e-2 (#186): a directive a rescan masked has its preference written again
            // from its masked text, as no curator makes one from it.
            if masks.is_some() {
                crate::backup::fail_after("masked_claims_purged")?;
                let again = raw.masked_preferences(&forgotten(&raw)?)?;
                for batch in crate::raw::append_batches(&again) {
                    raw.append_ops(batch)?;
                }
                purged.preferences = again.len();
            }
            crate::backup::fail_after("record_claims_purged")?;
            purged.records = raw.purge_records()?;
            if masks.is_some() {
                purged.masked = raw.mask_rows()?;
            }
        }
        // Slice 3e-4 (#478): the text of every op masked with the rules now, after what 3e-1 and
        // 3e-2 wrote: the claims, windows and turns made from a masked record lose theirs by its
        // time, as before, and what is left (a correction's body, a work state, an import, a
        // curator's own words) by the rules.
        let mut text_stopped = false;
        if let Some(rules) = &text {
            text_mark = Some(raw.op_text_scan_begun()?);
            let mut after = None;
            loop {
                let page =
                    raw.op_text_masks(rules, after.as_ref(), crate::raw::RECORD_PURGE_BATCH)?;
                let Some((device, op_seq, ..)) = page.last() else {
                    break;
                };
                after = Some((device.clone(), *op_seq));
                // Rules changed meanwhile mask no further page; the scan stays owed to the rules
                // now (owner decision 43).
                let Some(masked) = raw.mask_op_text(&page, rules)? else {
                    text_stopped = true;
                    break;
                };
                purged.texts += masked;
            }
            crate::backup::fail_after("op_text_masked")?;
        }
        if text_stopped {
            text = None;
        }
        let truncated = raw.truncate_wal(TRUNCATE_WAIT)?;
        masked |= truncated;
        text_truncated = truncated;
        let holding = record_jobs_with_text(&raw)?;
        for (job, step, held) in records_due {
            if !step_2(step, held) {
                if step == 2 {
                    record_rebuild.push(job);
                }
            } else if truncated && !holding.contains(&job) {
                raw.restart_forget_step(&job, 2)?;
                record_rebuild.push(job);
            } else {
                purged.unfinished.push(job);
            }
        }
        for (job, uid, step, held) in due {
            if !step_2(step, held) {
                if step == 2 {
                    rebuild.push((job, uid));
                }
            } else if truncated && !raw.uid_has_ops(&uid)? {
                // Step 2 exactly, for a job past it whose text came back too: its rebuild and the
                // steps after it stay owed if this purge stops before them (Codex on #453).
                raw.restart_forget_step(&job, 2)?;
                rebuild.push((job, uid));
            } else {
                purged.unfinished.push(job);
            }
        }
    } else {
        rebuild = due
            .into_iter()
            .filter(|(_, _, step, _)| *step == 2)
            .map(|(job, uid, ..)| (job, uid))
            .collect();
        record_rebuild = records_due
            .into_iter()
            .filter(|(_, step, _)| *step == 2)
            .map(|(job, ..)| job)
            .collect();
    }
    // The worker lock and raw are let go: the rebuild takes them itself.
    let uids: HashSet<String> = rebuild.iter().map(|(_, uid)| uid.clone()).collect();
    // Slice 3e-4: and for op text masked, by this purge or one that stopped.
    let rebuilds = !rebuild.is_empty()
        || !record_rebuild.is_empty()
        || masks.is_some()
        || (text.is_some() && crate::raw::open(home)?.ops_text_pending()?);
    if rebuilds {
        // Rebuilt for every job not past step 3, though knowledge.db holds no row of its uid: rows
        // a rewind took out (a raw.db put back from an older copy) leave the text in the vector
        // cache and the full-text segments (Greptile on #444), and nothing records that an earlier
        // continuation's rebuild finished (Codex on #444).
        crate::worker::rebuild(home).context("knowledge.db was not rebuilt")?;
        purged.rebuilt = true;
        // And the claims made from the forgotten records, as soon as knowledge.db is without them:
        // a step 4 that cannot finish leaves them in no session's shown value (CodeRabbit on
        // #471). Step 5 takes them out once more.
        let mut swept = uids.clone();
        if !record_rebuild.is_empty() {
            swept.extend(crate::raw::open(home)?.record_derived_uids()?.into_keys());
        }
        if !swept.is_empty() {
            purged.shown = crate::hookstate::update_all(home, "shown", |v| {
                crate::hook::without_uids(v, &swept)
            })?;
        }
    }
    #[cfg(test)]
    if let Some(before) = BEFORE_STEP_3.with(|hook| hook.borrow_mut().take()) {
        before();
    }
    // What knowledge.db holds, the copies set aside, then step 3, under the worker lock as a
    // restore takes it: a restore that put the bodies back since step 2 leaves the job to the next
    // continuation (Codex on #444). Step 4 under the same lock (2c, decision 1).
    let _worker = crate::worker::lock_asking(home)?;
    let raw = crate::raw::open(home)?;
    if rebuilds {
        let holding = knowledge_holding(home, &uids)?;
        if !aside_files(home)?.is_empty() {
            crate::embed_phase::carry_set_aside(home, &crate::knowledge::open(home)?, &raw)?;
            for file in aside_files(home)? {
                std::fs::remove_file(&file)
                    .with_context(|| format!("{} was not removed", file.display()))?;
                purged.aside += 1;
            }
        }
        for (job, uid) in rebuild {
            if raw.uid_has_ops(&uid)? {
                purged.unfinished.push(job);
                continue;
            }
            // Recorded for a stopped job too: it is not tried again, and `status` says it stopped.
            raw.finish_forget_step(&job, 3)?;
            if holding.contains(&uid) {
                purged.stopped.push(job);
            }
        }
        // Slice 3c: a record job whose rows a restore gave their text back since step 2 waits for
        // the next continuation, as a uid job does.
        let held = record_jobs_with_text(&raw)?;
        for job in record_rebuild {
            if held.contains(&job) {
                purged.unfinished.push(job);
            } else {
                raw.finish_forget_step(&job, 3)?;
            }
        }
    }
    // Step 4 (2c, decisions 2-5): the backup's ops segments and the copies of raw.db set aside,
    // for every uid job past step 3 whose ops hold no text, stopped ones too (decision 8), in one
    // pass for all their uids. Slice 3c-2: the record segments and the copies' rows too, for every
    // record job past step 3 whose rows hold no text, when one has not had them yet: they are read
    // whole each time, by every forgotten record's identity.
    let mut jobs = std::collections::HashMap::new();
    let (mut past, mut records_past) = (Vec::new(), Vec::new());
    // The sessions those record jobs forget, whose checks on the My Work page go at step 5.
    let mut sessions = Vec::new();
    // Rows a purge rewrote may be in them, a denial's with no job row too (a restore from segments
    // alone, Codex on #469 and #470), which has no step to owe them by.
    let mut records_owed = raw.record_backups_owed()?;
    let held = record_jobs_with_text(&raw)?;
    for (request, step) in raw.forget_jobs()? {
        match request.target {
            Target::Uid { uid } => {
                if step >= 3 && !raw.uid_has_ops(&uid)? {
                    // One job's id per uid for the bodies, and every job of it recorded: the logs
                    // can bring back a second (CodeRabbit on #453).
                    jobs.entry(uid).or_insert_with(|| request.job.clone());
                    past.push(request.job);
                }
            }
            target if step >= 3 && !held.contains(&request.job) => {
                // Steps 4 and 5 to do (a job 3c-2 took to step 4 had no step 5).
                records_owed |= step < 5;
                records_past.push(request.job);
                if let Target::Session { session, .. } = target {
                    sessions.push(session);
                }
            }
            _ => {}
        }
    }
    // Slice 3d-1: the claims made from the forgotten records, by their uids: their ops in the
    // backup's ops segments and the copies of raw.db, and in each session's shown value.
    if records_owed {
        for (uid, job) in raw.record_derived_uids()? {
            jobs.entry(uid).or_insert(job);
        }
    }
    if !jobs.is_empty() || records_owed || text.is_some() {
        // Owner decision 43 (Codex on #502): the rules the op text is masked with, read again under
        // the hold a save that changes them takes, kept through the rewrites below. Rules changed
        // since the look at raw.db (redaction turned off, say) mask no op the backups or the copies
        // hold, which may be one stored as typed while they were off, and the scan stays owed to
        // the rules now.
        let _dispatch = text
            .is_some()
            .then(|| crate::dispatch::exclusive(home).context("the backups were not purged"))
            .transpose()?;
        if let Some(rules) = &text
            && text_owed(home, &raw)?.is_none_or(|now| now.version() != rules.version())
        {
            text = None;
        }
        // And the window and turn ops 3d-2 rewrote, to the bodies raw.db holds now, and those it
        // does not hold judged by the same rules: every ops segment, as one set aside may hold
        // such an op (Codex and CodeRabbit on #472).
        let (origins, mut judge) = if records_owed {
            let mut judge = raw.judge(masks.is_some())?;
            // And what only they hold, read first: the claims made from a forgotten record, every
            // op of their uids, and the spans of the windows (Codex on #472).
            judge.widen(home)?;
            for (uid, job) in judge.uids() {
                jobs.entry(uid.clone()).or_insert_with(|| job.clone());
            }
            // #483: and the masked claims' uids only the backups hold, before their lines lose them.
            raw.keep_masked_uids(judge.masked())?;
            (raw.forgotten_origins()?, judge)
        } else {
            Default::default()
        };
        // Slice 3e-4: and every op's text, as raw.db's.
        judge.text = text.clone();
        // Slice 3e-1: and the lines and rows of the records a rescan masked, masked as raw.db's.
        let masks_of = if masks.is_some() {
            raw.masks()?
        } else {
            crate::raw::Masks::new()
        };
        let ops = crate::backup::purge_ops(home, &jobs, &judge)?;
        (purged.segments, purged.segments_removed) = (ops.live + ops.aside, ops.removed);
        crate::backup::fail_after("ops_segments_purged")?;
        if records_owed {
            let lines = crate::backup::purge_record_segments(home, &origins, &masks_of)?;
            purged.segments += lines.live + lines.aside;
            purged.segments_removed += lines.removed;
        }
        (purged.copies, purged.copies_removed) =
            crate::raw::purge_copies(home, &jobs, &origins, &masks_of, &judge)?;
        for job in past.iter().chain(&records_past) {
            raw.finish_forget_step(job, 4)?;
        }
        crate::backup::fail_after("backups_purged")?;
        // Step 5 (2c, decision 7): every session's shown value loses the uids once more. A hook
        // that read before the registration wrote it before the registration committed (#439's
        // fence), so this finds nothing as a rule.
        let forgotten: HashSet<String> = jobs.keys().cloned().collect();
        if !forgotten.is_empty() {
            purged.shown += crate::hookstate::update_all(home, "shown", |v| {
                crate::hook::without_uids(v, &forgotten)
            })?;
        }
        // Owed until step 5 is done too: a denial with no job row has no step to record it by
        // (Codex on #471).
        if records_owed {
            raw.owe_record_backups(false)?;
        }
        // And the checks My Work holds of a forgotten session (docs/my-work.md rule 6), swept
        // again by a purge that stops before step 5 is recorded.
        crate::my_work::forget_reviews(home, &sessions)?;
        // And the copies of files a forgotten turn made (docs/deliverables.md); the page and the
        // links already refuse them by the same denials.
        crate::deliverables::sweep(home, &raw, false)?;
        for job in past.iter().chain(&records_past) {
            raw.finish_forget_step(job, 5)?;
        }
    }
    match masks.as_deref() {
        // A restore since step 2 may have put a row back as it was (Codex on #480).
        Some(seen) if masked && !raw.rows_hold_masks()? => raw.masks_paid(seen)?,
        Some(_) => purged.unfinished.push(crate::raw::MASK_JOB.to_owned()),
        None => {}
    }
    // A restore since step 2 made raw.db anew from the segments it read then (Codex on #480).
    if let Some(rules) = &text
        && !(text_truncated
            && raw.ops_text_scanned(rules.version(), text_mark.as_deref().unwrap_or_default())?)
        && !purged.unfinished.iter().any(|j| j == crate::raw::MASK_JOB)
    {
        purged.unfinished.push(crate::raw::MASK_JOB.to_owned());
    }
    Ok(Some(purged))
}

/// Slice 3e-4 (#478): the redaction rules now when raw.db's op text is owed a scan with them
/// (`Raw::ops_text_owed`); none while the settings do not load (capture stops then too, and doctor
/// names them).
fn text_owed(home: &Path, raw: &crate::raw::Raw) -> Result<Option<crate::redact::Rules>> {
    let Ok(settings) = crate::capture::Settings::load(home) else {
        return Ok(None);
    };
    Ok(raw
        .ops_text_owed(settings.rules.version())?
        .then_some(settings.rules))
}

/// Slice 3e-4: whether raw.db's op text waits for a purge: owed a scan with the rules now, and an
/// op they mask, or masked by a purge that stopped, or a backup's ops segment or a copy of raw.db
/// set aside, which may hold an op raw.db does not (a restore that left it out), and only a
/// purge's step 4 reads (Codex on #487); rules turned off find nothing there either (owner
/// decision 43). With none of them, the scan is recorded here and no purge starts: a new store, or
/// a rule that finds nothing in an op.
fn text_waits(home: &Path, raw: &crate::raw::Raw) -> Result<bool> {
    let Some(rules) = text_owed(home, raw)? else {
        return Ok(false);
    };
    // Begun before the look: an append under rules turned off meanwhile takes the mark away, and
    // the scan is not recorded (owner decision 43; Codex's security review of the switch). The
    // worker holds its lock: no restore makes raw.db anew meanwhile.
    let store = crate::raw::open(home)?;
    let mark = store.op_text_scan_begun()?;
    if !rules.is_off()
        && (crate::backup::any_ops_segment(home)?
            || !crate::raw::copies_set_aside(home)?.is_empty())
        || raw.ops_text_pending()?
        || !raw.op_text_masks(&rules, None, 1)?.is_empty()
    {
        return Ok(true);
    }
    #[cfg(test)]
    if let Some(between) = AFTER_TEXT_LOOK.take() {
        between();
    }
    store.ops_text_scanned(rules.version(), &mark)?;
    Ok(false)
}

/// How often a worker looks for a uid job its purge left (D5 item 8).
const CONTINUE_EVERY_MS: i64 = 10 * 60 * 1000;

#[cfg(test)]
thread_local! {
    /// The `oboete forget --continue` a worker on this thread would have started.
    pub(crate) static CONTINUED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// What happens to the home after a purge's rebuild on this thread, before its step 3 takes
    /// the worker lock.
    static BEFORE_STEP_3: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// What happens to the home between a worker's look at the op text and the scan it records.
    static AFTER_TEXT_LOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// What happens to the home once a purge holds the worker lock, before its step 2.
    static AFTER_PURGE_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
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
    let waiting = due(raw)
        .and_then(|due| {
            Ok(!due.is_empty()
                || !records_due(raw)?.is_empty()
                || raw.record_backups_owed()?
                || raw.masks_owed()?.is_some()
                || text_waits(home, raw)?
                || crate::retention::due(home, raw)?)
        })
        .unwrap_or_else(|e| {
            eprintln!("oboete: forget jobs not read: {e:#}");
            false
        });
    if waiting {
        continue_now(home, false);
    }
}

/// `oboete forget --continue` started now: from `continue_due`, and from the page's forget, which
/// registers without the CLI's purge and does not wait for the worker's next look (parity row 3).
/// A purge that runs read its jobs when it began. The worker starts none beside it (D5 item 8): it
/// looks again in ten minutes. The page's (`behind`) waits for it to end (`--wait`), with no worker
/// needed to look again (Codex on #655). None when none was started; else whether it waits.
pub(crate) fn continue_now(home: &Path, behind: bool) -> Option<bool> {
    // `purge_lock` makes `state/` first: a page that serves no worker may have none yet.
    let running = match purge_lock(home) {
        Ok(Some(_)) => false,
        Ok(None) if behind => true,
        Ok(None) | Err(_) => return None,
    };
    #[cfg(test)]
    {
        CONTINUED.set(CONTINUED.get() + 1);
        Some(running)
    }
    #[cfg(not(test))]
    {
        let args: &[&str] = if running {
            &["forget", "--continue", "--wait"]
        } else {
            &["forget", "--continue"]
        };
        let mut child = crate::hook::spawn_detached(home, args)?;
        // Reaped when it leaves, as a resident worker's viewer is.
        std::thread::spawn(move || child.wait());
        Some(running)
    }
}

/// While another process's purge holds `state/purge.lock`: `forget --continue --wait`, which a
/// forget started while one runs goes on after (`continue_now`).
pub fn wait_for_purge(home: &Path) -> Result<()> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("purge.lock"))?;
    // Held only to wait: the purge that follows takes it again.
    lock.lock()?;
    Ok(())
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
        // A log under a file is no log but a place it cannot be read from: Windows says the path
        // is not found where Linux says a part of it is not a directory.
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && !path.parent().is_some_and(Path::is_file) =>
        {
            return Some(Vec::new());
        }
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
    let (requests, unreadable) = raw.forget_requests_read()?;
    for job in unreadable {
        report.problems.push(format!(
            "forget job {job} holds a request this oboete cannot read (a later oboete's), passed over"
        ));
    }
    let held: std::collections::HashSet<String> = requests.into_iter().map(|r| r.job).collect();
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

/// D5 (2b, item 5): `forgotten`, raw.db's forgotten uids, with those whose forget only the logs hold
/// yet (a raw.db put back from an older copy, which the next pass reconciles): a carry passes all
/// of them over (Codex on #444). Only the requests of `home_id`, the store's lineage, count: a
/// backup directory two homes share holds the other's too (Codex on #444). A log copy that cannot
/// be read, or whose place cannot be found, is an error: the carry stops, as for a copy whose
/// vectors cannot be read (CodeRabbit on #444). Damaged lines are skipped, as everywhere (rule 9).
pub(crate) fn with_logged(
    home: &Path,
    mut forgotten: Forgotten,
    home_id: Option<&str>,
) -> Result<Forgotten> {
    let mut report = Report::default();
    let copies = logs(home, &mut report);
    anyhow::ensure!(report.problems.is_empty(), "{}", report.problems.join("; "));
    for copy in copies {
        let Some(requests) = read_log(&copy, home_id, &mut report) else {
            anyhow::bail!("{}", report.problems.join("; "));
        };
        for r in requests {
            // Slice 3c: a record a logged request names, by where it was (a seq reused since takes
            // another record's vector too, which is embedded again).
            forgotten.records.extend(
                r.records
                    .iter()
                    .map(|rec| format!("{}:{}", rec.device, rec.seq)),
            );
            if let Target::Uid { uid } = r.target {
                forgotten.uids.insert(uid);
            }
        }
    }
    Ok(forgotten)
}

/// What a carry takes no vector of (milestone 5 D5): the forgotten uids, and the forgotten
/// records' vector keys (`<device>:<seq>`, slice 3c).
#[derive(Default)]
pub(crate) struct Forgotten {
    pub(crate) uids: std::collections::HashSet<String>,
    pub(crate) records: std::collections::HashSet<String>,
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
         of its memory or none; two forgets one after the other rebuild twice. Then the \
         backups are rewritten without it: the backup segments, those set aside too, and the \
         copies of raw.db set aside; one that cannot be read or rewritten is removed. A backup \
         location in a cloud folder may keep earlier versions of a segment in its history, which \
         forget does not reach.\n\
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
        "Its row in raw.db is rewritten without the text right after registration (its seq and \
         time stay), then knowledge.db is rebuilt from raw.db without it: minutes on a large \
         store, and a session that starts meanwhile gets part of its memory or none. Then the \
         backup's segments and the copies of raw.db set aside are rewritten without it too; one \
         that cannot be read or rewritten, which no reader of oboete uses, is removed. The claims \
         quoting it are forgotten with it, by uid: a recuration does not make them again. The \
         cards and session summaries of what a curator was shown of it, or of those claims, lose \
         their text, and those windows are listed for `oboete recurate`; a window curated before \
         this version did not record the claims it carried in, so its cards stay when only such a \
         claim was forgotten. A claim whose status stood on it (its acceptance, a passing run) \
         holds none of its text and stays; once none of the records its status stood on is left, \
         it is a proposal.\n\
         A packet already handed to an agent and a curation call already sent are not taken \
         back.\n\
         If raw.db, both request logs and the backups made since the request are lost (an \
         older copy of the whole home put back), the request is lost and the text can come back \
         from older backups.\n\
         After an older file replaces raw.db, SessionStart may show a forgotten record until \
         the worker reopens the stores and applies the request logs.\n\
         A crash between raw's commit and the first log line, followed by loss of raw.db \
         before the worker starts, loses the request too.\n\
         Restoring segments with no surviving request log recovers denials but not the \
         request's progress rows.\n",
    );
    // Slice 3a: a live record's identity hashes where and when it was recorded, not its text.
    if sources.iter().any(|s| !crate::raw::is_live(s)) {
        out.push_str(
            "The request is logged without the text, by hashes; for an imported record, someone \
             who holds a log or the backups can check a guess of the exact stored text against \
             them.\n",
        );
    }
    let mut outside: Vec<&str> = Vec::new();
    for s in sources {
        let what = match s.as_str() {
            "oboete-v1" => "the old oboete v1 store it was moved from",
            "transcript" | "hook" | "replay" => "the agent's own transcript files",
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

/// Slice 3b: what a session, a repository or a time range resolved to (spec 6.2's preview).
fn say_scope(target: &Target, s: &ScopePreview) {
    use chrono::TimeZone;
    let local = |ms: i64| {
        chrono::Local.timestamp_millis_opt(ms).single().map_or_else(
            || ms.to_string(),
            |t| t.format("%Y-%m-%d %H:%M (%:z)").to_string(),
        )
    };
    let first = s
        .first
        .map_or_else(|| "at a time no record keeps".into(), local);
    let sources: Vec<String> = s.sources.iter().map(|(k, n)| format!("{k} {n}")).collect();
    let by = format!(
        "its records by source: {}; on {} device(s)",
        sources.join(", "),
        s.devices
    );
    match target {
        Target::Session { agent, .. } => {
            println!("A session of {agent}, first recorded {first}: {by}");
            if s.idless > 0 {
                println!(
                    "Records of {agent} that name no session, a copy of this session's among them \
                     staying: {}",
                    s.idless
                );
            }
        }
        Target::Repo { .. } => {
            let paths = s
                .paths
                .iter()
                .map(|p| crate::redact::outbound(p))
                .collect::<Vec<_>>();
            println!(
                "A repository, {}, first recorded {first}: {by}",
                s.keys
                    .iter()
                    .map(|k| crate::redact::outbound(k))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if !paths.is_empty() {
                println!("Recorded at: {}", paths.join(", "));
            }
            if s.masked > 0 {
                println!(
                    "Records whose repository is masked, not taken (whether they are this one's \
                     cannot be told): {}",
                    s.masked
                );
            }
        }
        Target::Range { since, until, .. } => println!(
            "This device's records from {} ({}) until {} ({}), not that time itself: {by}",
            local(*since),
            crate::db::utc(*since),
            local(*until),
            crate::db::utc(*until)
        ),
        _ => {}
    }
    if !matches!(target, Target::Session { .. }) && s.idless > 0 {
        println!(
            "Records that name no session, whose copies from another source cannot be found and \
             stay: {}",
            s.idless
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    home: &Path,
    record: Option<&str>,
    span: Option<&str>,
    uid: Option<&str>,
    session: Option<&str>,
    repo: Option<&str>,
    range: Option<(&str, &str)>,
    yes: bool,
    show: bool,
    resume: bool,
) -> Result<()> {
    if crate::raw::exists(home) {
        reconcile_or_say(home, &mut crate::raw::open(home)?)?;
    }
    if resume {
        // Nothing to continue says nothing: the worker starts it when a job waits (D5 item 8).
        if let Some(p) = purge(home)?.filter(|p| p.jobs > 0 || p.masks) {
            say_purged(&p)?;
        }
        // docs/retention.md: a retention pass after the purge, whose masks come first.
        if let Some(r) = crate::retention::pass(home)? {
            if r.expired > 0 {
                println!(
                    "Raw records whose text expired under [retention]: {}",
                    r.expired
                );
            }
            if r.owed {
                println!(
                    "A retention batch is still owed: a mask's purge or a reader of raw.db comes \
                     first, and the worker tries it again."
                );
            }
        }
        return Ok(());
    }
    if show {
        for job in status(home)? {
            println!("{}", serde_json::to_string(&job)?);
        }
        return Ok(());
    }
    let target = match (record, span, uid, session, repo, range) {
        (Some(r), None, None, None, None, None) => Target::parse(r, false)?,
        (None, Some(s), None, None, None, None) => Target::parse(s, true)?,
        (None, None, Some(u), None, None, None) => Target::parse_uid(u)?,
        (None, None, None, Some(h), None, None) => session_target(home, h)?,
        (None, None, None, None, Some(k), None) => repo_target(home, k)?,
        (None, None, None, None, None, Some((since, until))) => range_target(home, since, until)?,
        _ => anyhow::bail!(
            "choose --record, --span, --uid, --session, --repo, --since with --until, --status or \
             --continue"
        ),
    };
    let p = preview(home, target)?;
    println!("Target: {}", serde_json::to_string(&p.target)?);
    println!("Raw records: {}", p.count());
    if p.copies() > 0 {
        println!(
            "Records of the same sessions from another source, which may hold a copy of them \
             (forgotten with them): {}",
            p.copies()
        );
    }
    if p.passed > 0 {
        println!(
            "Records with no text left and no identity, passed over: {}",
            p.passed
        );
    }
    if p.claims > 0 {
        println!(
            "Claims quoting them, forgotten by uid with them: {}",
            p.claims
        );
    }
    if p.cards > 0 {
        println!(
            "Windows whose cards lose their text, then listed for `oboete recurate`: {}",
            p.cards
        );
    }
    if p.written > 0 {
        println!(
            "Claims those windows wrote, which lose their text until `oboete recurate` writes \
             them again: {}",
            p.written
        );
    }
    if p.summaries > 0 {
        println!("Session summaries that lose their text: {}", p.summaries);
    }
    if let Some(s) = &p.scope {
        say_scope(&p.target, s);
    }
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
        match &u.segments {
            Some(s) if s.live + s.aside + s.removed > 0 => println!(
                "Backup segments rewritten without it: {} live, {} set aside; unsealed ones that \
                 cannot be read whole or hold a line that cannot be judged, removed: {}",
                s.live, s.aside, s.removed
            ),
            Some(_) => {}
            None => println!(
                "The backup location cannot be read now: the purge rewrites its segments once it \
                 can."
            ),
        }
        if u.copies > 0 {
            println!(
                "Copies of raw.db set aside, rewritten where they hold it or removed when they \
                 cannot be: {}",
                u.copies
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
    // D5 (2b), slice 3c for records: registered, and purged in this process.
    match purge(home)
        .context("the purge did not finish; `oboete forget --continue` goes on with it")?
    {
        Some(p) => say_purged(&p),
        None => {
            println!(
                "Another oboete is purging what was forgotten; `oboete forget --continue` purges this \
                 one once it ends."
            );
            Ok(())
        }
    }
}

fn say_purged(p: &Purged) -> Result<()> {
    if p.records > 0 {
        println!(
            "Records rewritten without their text in raw.db: {}",
            p.records
        );
    }
    if p.masked > 0 {
        println!(
            "Records a redaction rescan masked, rewritten with the masks in raw.db: {}",
            p.masked
        );
    }
    if p.preferences > 0 {
        println!(
            "Preferences written again from their masked text: {}",
            p.preferences
        );
    }
    if p.ops > 0 {
        println!("Ops rewritten without their text in raw.db: {}", p.ops);
    }
    if p.texts > 0 {
        println!(
            "Ops whose text the current redaction rules masked in raw.db: {}",
            p.texts
        );
    }
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
    if p.segments > 0 {
        println!("Backup segments rewritten without it: {}", p.segments);
    }
    if p.segments_removed > 0 {
        println!(
            "Damaged backup segments that could not be read, removed: {}",
            p.segments_removed
        );
    }
    if p.copies > 0 {
        println!(
            "Copies of raw.db set aside, rewritten without it: {}",
            p.copies
        );
    }
    if p.copies_removed > 0 {
        println!(
            "Copies of raw.db that could not be rewritten, and a restore's partial build, removed: {}",
            p.copies_removed
        );
    }
    // A rescan's masks are owed by a count, not a job `--status` lists (CodeRabbit on #480).
    let (masks, jobs): (Vec<&String>, Vec<&String>) = p
        .unfinished
        .iter()
        .partition(|j| *j == crate::raw::MASK_JOB);
    anyhow::ensure!(
        jobs.is_empty(),
        "jobs {} are not done (readers kept raw.db-wal from truncating, or a restore put the text \
         back): `oboete forget --continue` finishes them",
        jobs.iter()
            .map(|j| j.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    anyhow::ensure!(
        masks.is_empty(),
        "a redaction rescan's masks are not in every stored byte yet (readers kept raw.db-wal from \
         truncating, or a restore put a row back): `oboete forget --continue` finishes them"
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

/// Slice 3b-2: a repository's key as a log line keeps it, hashed.
pub(crate) fn repo(key: &str) -> String {
    origin("repo", key)
}

/// Slice 3a: a live record's identity, from where and when it was recorded and never its text: a
/// seq that a raw.db put back from an older copy gives again holds a record of another time.
pub(crate) fn live_origin(device: &str, seq: i64, ts: i64) -> String {
    origin("live", &format!("{device}:{seq}:{ts}"))
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
mod tests;
