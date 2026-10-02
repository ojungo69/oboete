//! M5 slice 1: a bodyless, durable request to forget. Physical purge is a later slice;
//! accepting a request never reports it complete. `privacy.db` is outside raw's restore.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

/// One bounded registration. Larger selections use several spans until paged purge lands.
pub const MAX_RECORDS: usize = 500;

#[cfg(test)]
thread_local! {
    // Only the process-crash test pauses between the two durable stores.
    pub(crate) static REGISTERED: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
    static INITIALIZING: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
    static JOURNAL_WRITING: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
    static FULL_JOURNAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

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
        let (device, range) = text.split_once(':').context("expected <device>:<seq> or <device>:<from>-<to>; uid and document forget is not available yet")?;
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Head {
    version: u32,
    identity: String,
    through: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Version {
    pub device: String,
    pub seq: i64,
    pub op_seq: i64,
    pub control: Option<Head>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub device: String,
    pub seq: i64,
    pub fingerprint: String,
    pub origin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Preview {
    pub(crate) target: Target,
    pub(crate) version: Version,
    pub(crate) records: Vec<Record>,
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
            self.records.len() <= MAX_RECORDS
                && self
                    .version
                    .seq
                    .checked_add(self.records.len() as i64)
                    .is_some(),
            "forget selection exceeds its bounds"
        );
        for record in &self.records {
            anyhow::ensure!(
                record.device == device && (from..=to).contains(&record.seq),
                "forget selection differs from its preview"
            );
            let origin = record.origin.as_deref().with_context(|| format!("record {}:{} has no native source identity; this legacy or hook record cannot be forgotten safely by this slice; nothing was registered", record.device,record.seq))?;
            check_identity(origin)?;
            check_identity(&record.fingerprint)?;
        }
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }
    pub(crate) fn token(&self) -> Result<String> {
        Ok(hash(&serde_json::to_vec(&(
            &self.target,
            &self.version,
            &self.records,
        ))?))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub job: String,
    pub target: Target,
    pub started: i64,
    pub local: &'static str,
    pub hub: &'static str,
}

pub fn preview(home: &Path, target: Target) -> Result<Preview> {
    anyhow::ensure!(crate::raw::exists(home), "no raw store to forget from");
    crate::raw::open(home)?.forget_preview(target)
}

pub fn start(home: &Path, preview: &Preview) -> Result<Status> {
    crate::raw::open(home)?.forget_start(preview)
}

/// Raw open recovers an intent committed before its raw transaction. No purge is claimed.
pub fn resume(home: &Path) -> Result<Vec<Status>> {
    if crate::raw::exists(home) {
        let _raw = crate::raw::open(home)?;
    }
    status(home)
}

pub fn status(home: &Path) -> Result<Vec<Status>> {
    let Some(journal) = Journal::read(home)? else {
        if crate::raw::exists(home) {
            let conn = Connection::open_with_flags(
                crate::raw::path(home),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let expected: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key='privacy_head')",
                [],
                |r| r.get(0),
            )?;
            anyhow::ensure!(
                !expected,
                "privacy journal is missing; refusing an empty history"
            );
        }
        return Ok(Vec::new());
    };
    journal
        .controls(0)?
        .into_iter()
        .map(|c| c.status(&journal.head.identity))
        .collect()
}

pub fn run(
    home: &Path,
    record: Option<&str>,
    span: Option<&str>,
    yes: bool,
    show: bool,
    retry: bool,
) -> Result<()> {
    if show || retry {
        let jobs = if retry { resume(home)? } else { status(home)? };
        for job in jobs {
            println!("{}", serde_json::to_string(&job)?);
        }
        return Ok(());
    }
    let target = match (record, span) {
        (Some(r), None) => Target::parse(r, false)?,
        (None, Some(s)) => Target::parse(s, true)?,
        _ => anyhow::bail!("choose --record, --span, --status or --resume"),
    };
    let p = preview(home, target)?;
    println!("Target: {}", serde_json::to_string(&p.target)?);
    println!("raw records: {}", p.count());
    if let Some(sample) = &p.sample {
        println!("sample: {}", crate::redact::outbound(sample));
    }
    p.validate()?;
    println!(
        "Physical purge is not implemented yet. This request will remain unfinished.\nAgent transcripts, migration snapshots and evaluation copies remain outside this request."
    );
    if !yes {
        println!("Register this irreversible request? Type yes:");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != "yes" {
            println!("Nothing was registered.");
            return Ok(());
        }
    }
    println!("{}", serde_json::to_string(&start(home, &p)?)?);
    Ok(())
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Same event imported through another path. Source/path/capture location are not its identity.
pub(crate) fn fingerprint(e: &crate::raw::Event) -> Result<String> {
    let body = serde_json::from_str::<serde_json::Value>(&e.body)
        .unwrap_or_else(|_| e.body.clone().into());
    Ok(format!(
        "v1:{}",
        hash(&serde_json::to_vec(&(
            &e.agent, &e.session, &e.kind, e.ts, body
        ))?)
    ))
}

/// A native event identity, versioned and hashed before persistence. No original id/text kept.
pub(crate) fn origin(source: &str, id: &str) -> String {
    format!(
        "v1:{}",
        hash(&serde_json::to_vec(&(source, id)).expect("string pair"))
    )
}

pub(crate) fn check_identity(s: &str) -> Result<()> {
    anyhow::ensure!(
        s.strip_prefix("v1:")
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())),
        "unknown or damaged privacy identity"
    );
    Ok(())
}

pub(crate) struct Control {
    seq: i64,
    started: i64,
    target: Target,
    watermark: i64,
    records: Vec<Record>,
}

impl Control {
    fn status(&self, identity: &str) -> Result<Status> {
        self.target.bounds()?;
        Ok(Status {
            job: format!("{identity}:{}", self.seq),
            target: self.target.clone(),
            started: self.started,
            local: "pending_physical_purge",
            hub: "not_connected",
        })
    }
}

struct Journal {
    conn: Connection,
    head: Head,
}

impl Journal {
    fn read(home: &Path) -> Result<Option<Self>> {
        let head = match std::fs::File::open(home.join("privacy.head")) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(4097).read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 4096, "privacy head is over its cap");
                Some(serde_json::from_slice::<Head>(&bytes).context("privacy head is unreadable")?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("read privacy head"),
        };
        let path = home.join("privacy.db");
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && head.is_none() => {
                return Ok(None);
            }
            Err(e) => return Err(e).context("privacy journal is missing or unreadable"),
            Ok(m) => anyhow::ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "privacy journal is not a regular file"
            ),
        }
        // No CREATE: missing controls remain an error. Read-write permits SQLite to roll back
        // a hot journal after a registrar died before commit, before we validate its authority.
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .context("open privacy journal")?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(version == 1, "unknown privacy journal version {version}");
        let identity: String = conn.query_row("SELECT identity FROM journal", [], |r| r.get(0))?;
        let (through, count): (i64, i64) = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0), COUNT(*) FROM requests",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        anyhow::ensure!(through == count, "privacy journal has a missing control");
        if let Some(head) = &head {
            anyhow::ensure!(
                head.version == 1 && head.identity == identity && head.through <= through,
                "privacy journal is older than its durable head or has another identity"
            );
        } else {
            anyhow::ensure!(
                through == 0,
                "privacy head is missing for a nonempty journal"
            );
        }
        Ok(Some(Self {
            conn,
            head: Head {
                version: 1,
                identity,
                through,
            },
        }))
    }

    fn create(home: &Path) -> Result<Self> {
        if let Some(j) = Self::read(home)? {
            return Ok(j);
        }
        let path = home.join("privacy.db.initializing");
        // No request is written to this name. A killed initializer leaves raw readable and the
        // next registrar (under raw's writer lock) replaces its empty, partial schema.
        for name in ["privacy.db.initializing", "privacy.db.initializing-journal"] {
            match std::fs::remove_file(home.join(name)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(e).context("remove interrupted privacy initialization");
                }
                _ => {}
            }
        }
        let conn = Connection::open(&path)?;
        #[cfg(test)]
        if let Some(initializing) = INITIALIZING.get() {
            initializing();
        }
        crate::db::private(&path, 0o600);
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;
          BEGIN IMMEDIATE;
          PRAGMA user_version=1;
          CREATE TABLE journal(identity TEXT NOT NULL);
          CREATE TABLE requests(seq INTEGER PRIMARY KEY, started INTEGER NOT NULL, target TEXT NOT NULL,
            watermark INTEGER NOT NULL, token TEXT NOT NULL UNIQUE);
          CREATE TABLE targets(request INTEGER NOT NULL, device TEXT NOT NULL, seq INTEGER NOT NULL,
            fingerprint TEXT NOT NULL, origin TEXT, PRIMARY KEY(request,device,seq));")?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)?;
        let identity = hash(&random);
        conn.execute("INSERT INTO journal VALUES(?1)", [&identity])?;
        conn.execute_batch("COMMIT")?;
        conn.close().map_err(|(_, e)| e)?;
        std::fs::rename(&path, home.join("privacy.db"))?;
        sync_dir(home)?;
        let head = Head {
            version: 1,
            identity,
            through: 0,
        };
        write_head(home, &head)?;
        Self::read(home)?.context("new privacy journal is missing")
    }

    fn controls(&self, after: i64) -> Result<Vec<Control>> {
        let mut out = Vec::new();
        let mut st = self.conn.prepare("SELECT seq,started,target,watermark FROM requests WHERE seq>?1 AND seq<=?2 ORDER BY seq")?;
        let mut rows = st.query(params![after, self.head.through])?;
        while let Some(r) = rows.next()? {
            let seq = r.get(0)?;
            let text: String = r.get(2)?;
            anyhow::ensure!(
                text.len() <= crate::raw::MAX_OP_BYTES,
                "privacy target is over the op cap"
            );
            let target: Target = serde_json::from_str(&text)?;
            target.bounds()?;
            let records = self.conn.prepare("SELECT device,seq,fingerprint,origin FROM targets WHERE request=?1 ORDER BY device,seq")?
                .query_map([seq], |r| Ok(Record { device:r.get(0)?,seq:r.get(1)?,fingerprint:r.get(2)?,origin:r.get(3)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            anyhow::ensure!(
                !records.is_empty() && records.len() <= MAX_RECORDS,
                "privacy request has an invalid target count"
            );
            let (device, from, to) = target.bounds()?;
            let watermark: i64 = r.get(3)?;
            for r in &records {
                anyhow::ensure!(
                    r.device == device && (from..=to).contains(&r.seq) && r.seq <= watermark,
                    "privacy record is outside its target"
                );
                check_identity(&r.fingerprint)?;
                if let Some(origin) = &r.origin {
                    check_identity(origin)?;
                }
            }
            anyhow::ensure!(
                watermark.checked_add(records.len() as i64).is_some(),
                "privacy sequence is exhausted"
            );
            out.push(Control {
                seq,
                started: r.get(1)?,
                target,
                watermark,
                records,
            });
        }
        Ok(out)
    }
}

pub(crate) fn version(home: &Path) -> Result<Option<Head>> {
    Ok(Journal::read(home)?.map(|j| j.head))
}

/// Called while raw's writer transaction is held: another registration cannot interleave.
pub(crate) fn register(home: &Path, p: &Preview) -> Result<()> {
    let journal = Journal::create(home)?;
    let token = p.token()?;
    if journal
        .conn
        .query_row("SELECT seq FROM requests WHERE token=?1", [&token], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .is_some()
    {
        return Ok(());
    }
    write_head(home, &journal.head)?; // also completes an empty journal's interrupted creation
    drop(journal.conn);
    let mut conn = Connection::open(home.join("privacy.db"))?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch("PRAGMA synchronous=EXTRA")?;
    #[cfg(target_os = "macos")]
    conn.execute_batch("PRAGMA fullfsync=ON")?;
    #[cfg(test)]
    if FULL_JOURNAL.get() {
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        conn.pragma_update(None, "max_page_count", pages)?;
    }
    #[cfg(test)]
    if JOURNAL_WRITING.get().is_some() {
        conn.pragma_update(None, "cache_size", 1)?;
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let seq = journal
        .head
        .through
        .checked_add(1)
        .context("privacy sequence exhausted")?;
    let target = serde_json::to_string(&p.target)?;
    anyhow::ensure!(
        target.len() <= crate::raw::MAX_OP_BYTES,
        "privacy target is over the op cap"
    );
    tx.execute(
        "INSERT INTO requests VALUES(?1,?2,?3,?4,?5)",
        params![seq, crate::db::now_ms(), target, p.version.seq, token],
    )?;
    for record in &p.records {
        tx.execute(
            "INSERT INTO targets VALUES(?1,?2,?3,?4,?5)",
            params![
                seq,
                record.device,
                record.seq,
                record.fingerprint,
                record.origin
            ],
        )?;
    }
    #[cfg(test)]
    if let Some(writing) = JOURNAL_WRITING.get() {
        writing();
    }
    tx.commit()?;
    write_head(
        home,
        &Head {
            through: seq,
            ..journal.head
        },
    )
}

/// Replay into raw's transaction, including a newly restored store before its file is swapped.
pub(crate) fn apply(conn: &Connection, home: &Path, device: &str) -> Result<()> {
    let applied = applied_head(conn)?;
    let journal = Journal::read(home)?;
    check_applied(applied.as_ref(), journal.as_ref().map(|j| &j.head))?;
    let Some(journal) = journal else {
        return Ok(());
    };
    for c in journal.controls(applied.as_ref().map_or(0, |h| h.through))? {
        let status = c.status(&journal.head.identity)?;
        for r in c.records {
            let inserted = conn.execute("INSERT OR IGNORE INTO denied_records(device,seq,fingerprint,origin) VALUES(?1,?2,?3,?4)", params![r.device,r.seq,r.fingerprint,r.origin])?;
            if inserted != 0 {
                let next: i64 = conn.query_row(
                    "SELECT MAX(COALESCE(MAX(seq),0),?2)+1 FROM records WHERE device=?1",
                    params![device, c.watermark],
                    |r| r.get(0),
                )?;
                conn.execute("INSERT INTO records(device,seq,type,ts,source,target_device,target_seq) VALUES(?1,?2,'tombstone',?3,'forget',?4,?5)", params![device,next,c.started,r.device,r.seq])?;
            }
        }
        conn.execute(
            "INSERT OR IGNORE INTO forget_jobs(id,target,started,step) VALUES(?1,?2,?3,1)",
            params![
                status.job,
                serde_json::to_string(&status.target)?,
                c.started
            ],
        )?;
    }
    conn.execute("INSERT INTO meta(key,value) VALUES('privacy_head',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(&journal.head)?])?;
    Ok(())
}

pub(crate) fn needs_apply(conn: &Connection, home: &Path) -> Result<bool> {
    let got = applied_head(conn)?;
    let expected = version(home)?;
    check_applied(got.as_ref(), expected.as_ref())?;
    Ok(got != expected)
}

fn applied_head(conn: &Connection) -> Result<Option<Head>> {
    let head: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='privacy_head'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(head.map(|s| serde_json::from_str(&s)).transpose()?)
}

fn check_applied(applied: Option<&Head>, current: Option<&Head>) -> Result<()> {
    let Some(applied) = applied else {
        return Ok(());
    };
    let current = current.context("privacy journal is missing; refusing an empty deny-list")?;
    anyhow::ensure!(
        applied.version == 1
            && applied.identity == current.identity
            && applied.through <= current.through,
        "privacy journal was rolled back"
    );
    Ok(())
}

/// Before a restore discards the current raw store, preserve its independent evidence that
/// controls existed. A fresh Rebuild cannot check that: its meta table has no privacy head yet.
/// Called under raw.lock exclusively, before any file is moved or a staged restore is removed.
pub(crate) fn before_restore(home: &Path) -> Result<()> {
    let current = version(home)?;
    if !crate::raw::exists(home) {
        return Ok(());
    }
    let read = || -> Result<Option<Head>> {
        let conn = Connection::open_with_flags(
            crate::raw::path(home),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        applied_head(&conn)
    };
    match read() {
        Ok(applied) => check_applied(applied.as_ref(), current.as_ref()),
        // A corrupt source cannot supply its meta table; the independently checked journal is
        // still applied to the rebuilt file. This also retains pre-forget corruption recovery.
        Err(e) if crate::backup::corrupt(&e) => Ok(()),
        Err(e) => Err(e).context("check current raw deletion authority before restore"),
    }
}

fn write_head(home: &Path, head: &Head) -> Result<()> {
    let path = home.join("privacy.head.part");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts.open(&path)?;
    file.write_all(&serde_json::to_vec(head)?)?;
    file.sync_all()?;
    std::fs::rename(&path, home.join("privacy.head"))?;
    sync_dir(home)
}

fn sync_dir(home: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(home)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = home;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{self, Item};

    fn record(home: &Path) -> (raw::Raw, Target) {
        let mut raw = raw::open(home).unwrap();
        let seq = native(&mut raw, 1, r#"{"prompt":"synthetic forget canary"}"#);
        let target = Target::Record {
            device: raw.device().into(),
            seq,
        };
        (raw, target)
    }

    fn native(raw: &mut raw::Raw, id: usize, body: &str) -> i64 {
        let mut event = raw::test_event(body);
        event.source = "transcript".into();
        raw.append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            &[origin("synthetic", &id.to_string())],
            "",
            None,
        )
        .unwrap()[0]
    }

    #[test]
    fn a_stale_preview_registers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (mut raw, target) = record(dir.path());
        let p = preview(dir.path(), target).unwrap();
        raw.append(&raw::test_event(r#"{"prompt":"a later record"}"#))
            .unwrap();
        assert!(
            start(dir.path(), &p)
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        assert!(status(dir.path()).unwrap().is_empty());
        assert!(matches!(
            raw.after(raw.device(), 0, 1).unwrap()[0].item,
            Item::Event(_)
        ));
    }

    #[test]
    fn a_failed_raw_commit_replays_the_durable_request_once() {
        let dir = tempfile::tempdir().unwrap();
        let (mut raw, target) = record(dir.path());
        let p = raw.forget_preview(target).unwrap();
        crate::crash::at(1);
        let failed = raw.forget_start(&p);
        crate::crash::off();
        assert!(failed.is_err());
        drop(raw);
        assert_eq!(resume(dir.path()).unwrap().len(), 1);
        assert_eq!(resume(dir.path()).unwrap().len(), 1);
        let raw = raw::open(dir.path()).unwrap();
        assert!(matches!(
            raw.after(raw.device(), 0, 1).unwrap()[0].item,
            Item::Removed
        ));
        assert_eq!(raw.after(raw.device(), 0, 10).unwrap().len(), 2);
    }

    #[test]
    fn a_head_write_failure_registers_no_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let (raw, target) = record(dir.path());
        let p = preview(dir.path(), target).unwrap();
        std::fs::create_dir(dir.path().join("privacy.head.part")).unwrap();
        assert!(start(dir.path(), &p).is_err());
        assert!(status(dir.path()).unwrap().is_empty());
        assert!(matches!(
            raw.after(raw.device(), 0, 1).unwrap()[0].item,
            Item::Event(_)
        ));
    }

    #[test]
    fn a_window_composed_before_forget_cannot_commit_its_answer() {
        let dir = tempfile::tempdir().unwrap();
        let (mut raw, target) = record(dir.path());
        let window =
            serde_json::json!({"from_seq":1,"to_seq":1,"summary":"synthetic forget canary"});
        let p = raw.forget_preview(target).unwrap();
        raw.forget_start(&p).unwrap();
        assert!(raw.append_ops(&[(raw::OpKind::Window, window)]).is_err());
        assert_eq!(
            raw.max_op_seq().unwrap(),
            0,
            "a rejected answer moved its checkpoint"
        );
    }

    #[test]
    fn another_native_event_with_the_same_text_is_not_denied() {
        let dir = tempfile::tempdir().unwrap();
        let (mut raw, target) = record(dir.path());
        let p = raw.forget_preview(target).unwrap();
        raw.forget_start(&p).unwrap();
        let second = native(&mut raw, 2, r#"{"prompt":"synthetic forget canary"}"#);
        assert!(matches!(
            raw.after(raw.device(), second - 1, 1).unwrap()[0].item,
            Item::Event(_)
        ));
        let mut event = raw::test_event(r#"{"prompt":"masked differently"}"#);
        event.source = "transcript".into();
        let unsupported = raw.append_imported(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            "",
            None,
        );
        assert!(
            unsupported
                .unwrap_err()
                .to_string()
                .contains("native source identity")
        );
    }

    #[test]
    fn sqlite_full_accepts_no_part_of_a_span() {
        let dir = tempfile::tempdir().unwrap();
        let mut raw = raw::open(dir.path()).unwrap();
        for n in 0..MAX_RECORDS {
            native(&mut raw, n, &format!("{{\"prompt\":\"synthetic-{n}\"}}"));
        }
        let p = raw
            .forget_preview(Target::Span {
                device: raw.device().into(),
                from: 1,
                to: MAX_RECORDS as i64,
            })
            .unwrap();
        FULL_JOURNAL.set(true);
        let failed = raw.forget_start(&p);
        FULL_JOURNAL.set(false);
        let e = failed.unwrap_err();
        assert!(e.chain().any(|e| matches!(e.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(e,_)) if e.code==rusqlite::ErrorCode::DiskFull)), "{e:#}");
        assert!(status(dir.path()).unwrap().is_empty());
        assert_eq!(
            raw.after(raw.device(), 0, MAX_RECORDS + 1)
                .unwrap()
                .into_iter()
                .filter(|r| matches!(r.item, Item::Event(_)))
                .count(),
            MAX_RECORDS
        );
    }

    #[test]
    fn registrar_process() {
        let Some(home) = std::env::var_os("OBOETE_TEST_CRASH_HOME") else {
            return;
        };
        let home = Path::new(&home);
        let (mut raw, mut target) = record(home);
        let phase = std::env::var("OBOETE_TEST_CRASH_PHASE").unwrap_or_default();
        if phase == "journal" {
            for n in 2..=MAX_RECORDS {
                native(&mut raw, n, "{\"prompt\":\"synthetic\"}");
            }
            target = Target::Span {
                device: raw.device().into(),
                from: 1,
                to: MAX_RECORDS as i64,
            };
        }
        let p = raw.forget_preview(target).unwrap();
        let stopped = || {
            let home = std::env::var_os("OBOETE_TEST_CRASH_HOME").unwrap();
            std::fs::write(Path::new(&home).join("registered"), b"ready").unwrap();
            loop {
                std::thread::park();
            }
        };
        if phase == "initialize" {
            INITIALIZING.set(Some(stopped));
        } else if phase == "journal" {
            JOURNAL_WRITING.set(Some(stopped));
        } else {
            REGISTERED.set(Some(stopped));
        }
        let _ = raw.forget_start(&p);
        panic!("the parent should kill this process at the durable boundary");
    }

    #[test]
    fn a_killed_registrar_recovers_without_a_second_request() {
        let home = tempfile::tempdir().unwrap();
        kill_at(home.path(), "register");
        let jobs = resume(home.path()).unwrap();
        assert_eq!(jobs.len(), 1);
        let raw = raw::open(home.path()).unwrap();
        assert!(matches!(
            raw.after(raw.device(), 0, 1).unwrap()[0].item,
            Item::Removed
        ));
        assert_eq!(resume(home.path()).unwrap()[0].job, jobs[0].job);
    }

    #[test]
    fn a_killed_journal_writer_rolls_back_and_can_resume() {
        let home = tempfile::tempdir().unwrap();
        kill_at(home.path(), "journal");
        assert!(
            std::fs::metadata(home.path().join("privacy.db-journal"))
                .unwrap()
                .len()
                > 0
        );
        assert!(
            resume(home.path()).unwrap().is_empty(),
            "an uncommitted request survived"
        );
        let raw = raw::open(home.path()).unwrap();
        let records = raw.after(raw.device(), 0, MAX_RECORDS + 1).unwrap();
        assert_eq!(records.len(), MAX_RECORDS);
        assert!(records.iter().all(|r| matches!(r.item, Item::Event(_))));
        let p = raw
            .forget_preview(Target::Record {
                device: raw.device().into(),
                seq: 1,
            })
            .unwrap();
        assert!(start(home.path(), &p).is_ok());
    }

    fn kill_at(home: &Path, phase: &str) {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "forget::tests::registrar_process", "--nocapture"])
            .env("OBOETE_TEST_CRASH_HOME", home)
            .env("OBOETE_TEST_CRASH_PHASE", phase)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !home.join("registered").exists() {
            if child.try_wait().unwrap().is_some() || std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the child never reached the durable registration");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    fn a_killed_initializer_can_retry_without_losing_data() {
        let home = tempfile::tempdir().unwrap();
        kill_at(home.path(), "initialize");
        let raw = raw::open(home.path()).unwrap();
        assert!(matches!(
            raw.after(raw.device(), 0, 1).unwrap()[0].item,
            Item::Event(_)
        ));
        let target = Target::Record {
            device: raw.device().into(),
            seq: 1,
        };
        let p = preview(home.path(), target).unwrap();
        start(home.path(), &p).unwrap();
        assert_eq!(status(home.path()).unwrap().len(), 1);
    }
}
