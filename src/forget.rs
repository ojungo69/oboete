//! Milestone 5 slice 1 (docs/milestone-5-plan.md, D1): a bodyless request to forget imported raw
//! records. raw.db is the one authority: one raw transaction writes the deny rows, the tombstones
//! and the job row. Two bodyless request logs, in the home and beside the backups, are its
//! redundancy: reconciled in both directions, never compared, and never a reason to refuse an open.
//! Physical purge is a later slice.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{IsTerminal, Read, Seek, Write};
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
    Record { device: String, seq: i64 },
    Span { device: String, from: i64, to: i64 },
}

impl Target {
    pub fn bounds(&self) -> Result<(&str, i64, i64)> {
        let (device, from, to) = match self {
            Self::Record { device, seq } => (device.as_str(), *seq, *seq),
            Self::Span { device, from, to } => (device.as_str(), *from, *to),
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
        let (device, range) = text.split_once(':').context(
            "expected <device>:<seq> or <device>:<from>-<to>; uid and document forget is not \
             available yet",
        )?;
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
}

impl Preview {
    pub(crate) fn validate(&self) -> Result<()> {
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

impl Request {
    pub(crate) fn check(&self) -> Result<()> {
        anyhow::ensure!(self.v == 1, "unknown request version {}", self.v);
        anyhow::ensure!(
            self.job.len() == 32 && self.job.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid job id"
        );
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
    crate::raw::open(home)?.forget_preview(target)
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
    Ok((status_of(&request), report))
}

pub fn status(home: &Path) -> Result<Vec<Status>> {
    if !crate::raw::exists(home) {
        return Ok(Vec::new());
    }
    Ok(crate::raw::open(home)?
        .forget_requests()?
        .iter()
        .map(status_of)
        .collect())
}

fn status_of(r: &Request) -> Status {
    Status {
        job: r.job.clone(),
        target: r.target.clone(),
        records: r.records.len(),
        started: r.started,
        local: "hidden; physical purge pending",
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
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(e) => {
            report
                .problems
                .push(format!("{} could not be read: {e}", path.display()));
            return None;
        }
    };
    // Bytes that are not text spoil only their own line.
    let text = String::from_utf8_lossy(&bytes);
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
        match Request::parse(line) {
            Ok(r) if device.is_none_or(|d| r.home == d) => out.push(r),
            Ok(_) => report.problems.push(format!(
                "{} line {}: a request of another home, skipped",
                path.display(),
                n + 1
            )),
            Err(e) => {
                report
                    .problems
                    .push(format!("{} line {}: {e:#}, skipped", path.display(), n + 1))
            }
        }
    }
    Some(out)
}

/// Appends `requests` to the log at `path`, one at a time under an exclusive lock on the file: a
/// newline first when the last byte is not one, each line written whole and synced, the
/// directory synced when the file is made (rule 9).
fn append_log(path: &Path, requests: &[&Request]) -> Result<()> {
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
    f.write_all(&out)?;
    f.sync_all()?;
    if made && let Some(dir) = path.parent() {
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
        #[cfg(not(unix))]
        let _ = dir;
    }
    Ok(())
}

/// Rule 4, in both directions and never refusing: every request a readable log copy holds that
/// raw.db lacks is applied to it (by identity, rule 5), and every request raw.db holds is
/// appended to each copy that lacks it. A copy that cannot be read or written is reported.
pub(crate) fn reconcile(home: &Path, raw: &mut crate::raw::Raw) -> Result<Report> {
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
    let all = raw.forget_requests()?;
    for (path, copy) in copies {
        let Some(copy) = copy else {
            continue;
        };
        let lacking: Vec<&Request> = all
            .iter()
            .filter(|r| !copy.iter().any(|c| c.job == r.job))
            .collect();
        match append_log(&path, &lacking) {
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
    yes: bool,
    show: bool,
) -> Result<()> {
    if crate::raw::exists(home) {
        reconcile_or_say(home, &mut crate::raw::open(home)?)?;
    }
    if show {
        for job in status(home)? {
            println!("{}", serde_json::to_string(&job)?);
        }
        return Ok(());
    }
    let target = match (record, span) {
        (Some(r), None) => Target::parse(r, false)?,
        (None, Some(s)) => Target::parse(s, true)?,
        _ => anyhow::bail!("choose --record, --span or --status"),
    };
    let p = preview(home, target)?;
    println!("Target: {}", serde_json::to_string(&p.target)?);
    println!("Raw records: {}", p.count());
    if let Some(sample) = &p.sample {
        println!("Sample: {}", crate::redact::outbound(sample));
    }
    p.validate()?;
    print!("{}", limits(&p.sources));
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

    /// An imported record of origin `id`, as an importer appends it.
    fn native(raw: &mut raw::Raw, id: &str, body: &str) -> i64 {
        let mut event = raw::test_event(body);
        event.source = "oboete-v1".into();
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
            run(h, None, None, false, true).unwrap();
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
    fn a_prepared_transcript_batch_cannot_cross_a_new_forget_cut() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let seq = native(
            &mut raw,
            "native-cut",
            r#"{"prompt":"a synthetic forgotten prompt"}"#,
        );
        let Item::Event(event) = raw.after(raw.device(), seq - 1, 1).unwrap().remove(0).item else {
            panic!("the imported record was not an event");
        };
        let mut event = *event;
        event.source = "transcript".into();
        let p = raw.forget_preview(record(&raw, seq)).unwrap();
        start(home.path(), &p).unwrap();
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
            .unwrap()
        };
        assert!(append(&mut raw, event.clone(), "late").is_empty());
        event.ts -= 1;
        event.body = r#"{"prompt":"earlier unrelated history"}"#.into();
        let kept = append(&mut raw, event, "earlier");
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
