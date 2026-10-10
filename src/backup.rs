//! Milestone 2 Task 8 (MUST-M15, spec 2.6, plan D11): raw.db's records as sealed, checksummed
//! zstd segments in the backup directory, and the quarantine and restore that use them.
//!
//! A segment is `<device>-<first seq>-<last seq>.seg.zst` with a `.sha256` beside it: the
//! records' backup lines (`Raw::export_lines`: as `Raw::after` returns them, masked, with their
//! ledger rows). The last backed-up seq is read from the names, so it survives a lost
//! knowledge.db.

use crate::raw::{self, Raw};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaintenanceOperation {
    Rebuild,
    Restore,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaintenanceCode {
    Stale,
    InvalidSource,
    InvalidConfig,
    Busy,
    RecoveryRequired,
    Failed,
}
impl std::fmt::Display for MaintenanceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MaintenanceCode {}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct StoreCounts {
    pub records: u64,
    pub ops: u64,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct BackupPreview {
    pub compressed_bytes: u64,
    pub record_segments: u64,
    pub invalid_record_segments: u64,
    pub op_segments: u64,
    pub skipped_op_segments: u64,
    pub record_first: Option<i64>,
    pub record_last: Option<i64>,
    pub op_prefix_through: i64,
    pub record_lines: Option<u64>,
    pub op_lines: Option<u64>,
    pub exact_drops_known: bool,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct MaintenancePreview {
    pub key: String,
    pub operation: MaintenanceOperation,
    pub raw: Option<StoreCounts>,
    pub knowledge_present: bool,
    pub cached_vectors: Option<u64>,
    pub kept_files: u64,
    pub staged_partial: bool,
    pub forget_requests: u64,
    pub forget_log_warnings: u64,
    pub backup: Option<BackupPreview>,
    #[serde(skip)]
    pub backup_dir: PathBuf,
    pub hybrid_ready: bool,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct Effects {
    pub knowledge_files_moved: u64,
    pub knowledge_put_back: bool,
    pub raw_files_quarantined: u64,
    pub raw_put_back: bool,
    pub raw_swapped: bool,
    pub stopped_restore_finished: bool,
    pub staging_created: bool,
    pub staging_complete: bool,
    pub segments_quarantined: u64,
    pub directory_sync_complete: bool,
}
impl Effects {
    pub(crate) fn committed(&self) -> bool {
        self.knowledge_files_moved != 0
            || self.raw_files_quarantined != 0
            || self.raw_swapped
            || self.stopped_restore_finished
            || self.segments_quarantined != 0
    }
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct RestoreReceipt {
    pub effects: Effects,
    pub replayed_records: u64,
    pub replayed_ops: u64,
    pub retained_ops: u64,
    pub dropped_ops: u64,
    pub skipped_segments: u64,
    pub old_raw_kept_files: u64,
    pub old_knowledge_kept_files: u64,
    pub backup_files_kept: u64,
    pub forget_log_warnings: u64,
    pub note_written: bool,
    #[serde(skip)]
    pub note: String,
}
#[derive(Debug)]
pub(crate) struct RestoreFailure {
    pub receipt: Box<RestoreReceipt>,
    pub code: MaintenanceCode,
    pub cause: anyhow::Error,
}

#[cfg(test)]
thread_local! {
    /// One deterministic native failure after the named, already recorded boundary.
    pub(crate) static FAIL_AFTER: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    pub(crate) static BEFORE_SWAP: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_REOPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_CONSENT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
pub(crate) fn before_swap() {
    #[cfg(test)]
    if let Some(before) = BEFORE_SWAP.with(|hook| hook.borrow_mut().take()) {
        before();
    }
}
pub(crate) fn effect(stage: &'static str, committed: &mut impl FnMut(&'static str)) -> Result<()> {
    committed(stage);
    fail_after(stage)
}
pub(crate) fn fail_after(stage: &'static str) -> Result<()> {
    #[cfg(test)]
    if FAIL_AFTER.get() == Some(stage) {
        FAIL_AFTER.set(None);
        anyhow::bail!("controlled failure after {stage}");
    }
    let _ = stage;
    Ok(())
}

/// A transient version manifest, discarded by preview and never kept in viewer state.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
struct FileVersion {
    identity: String,
    bytes: u64,
    modified: std::time::SystemTime,
    hash: String,
}
fn version(path: &Path) -> Result<Option<FileVersion>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(
        metadata.is_file(),
        "maintenance source is not a regular file"
    );
    let identity = crate::db::store_file(path);
    anyhow::ensure!(
        !identity.is_empty(),
        "maintenance source identity unavailable"
    );
    Ok(Some(FileVersion {
        identity,
        bytes: metadata.len(),
        modified: metadata.modified()?,
        hash: crate::migrate::file_version(path)?,
    }))
}

fn kept_name(name: &str) -> bool {
    [
        "raw.db.quarantined-",
        "raw.db.restored.quarantined-",
        "knowledge.db.quarantined-",
        "knowledge.db.rebuilding-",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}
fn store_name(name: &str) -> bool {
    name.starts_with("raw.db") || name.starts_with("knowledge.db")
}
fn preview_paths(home: &Path, backup: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = [
        "config.toml",
        "raw.db",
        "raw.db-wal",
        "raw.db-shm",
        "knowledge.db",
        "knowledge.db-wal",
        "knowledge.db-shm",
        "raw.db.restoring",
        "raw.db.restoring-journal",
        "raw.db.restored",
        "raw.db.restored-wal",
        "raw.db.restored-shm",
        "forget.log",
        "state/restore-wanted",
    ]
    .into_iter()
    .map(|name| home.join(name))
    .collect();
    if home.try_exists()? {
        for entry in std::fs::read_dir(home)? {
            let entry = entry?;
            if kept_name(&entry.file_name().to_string_lossy()) {
                paths.push(entry.path());
            }
        }
    }
    paths.push(backup.join("forget.log"));
    for segment in segments(backup, Kind::Records)?
        .into_iter()
        .chain(segments(backup, Kind::Ops)?)
    {
        paths.push(PathBuf::from(format!("{}.sha256", segment.path.display())));
        paths.push(segment.path);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}
fn preview_files(home: &Path, backup: &Path) -> Result<Vec<(PathBuf, Option<FileVersion>)>> {
    let paths = preview_paths(home, backup)?;
    let versions: Vec<_> = paths
        .into_iter()
        .map(|path| Ok((path.clone(), version(&path)?)))
        .collect::<Result<_>>()?;
    // A backup, config, log or staging pathname cannot double as a live/kept store.
    let mut store_ids = std::collections::BTreeSet::new();
    for (path, value) in &versions {
        if path.parent() == Some(home)
            && path
                .file_name()
                .is_some_and(|name| store_name(&name.to_string_lossy()))
            && let Some(value) = value
        {
            anyhow::ensure!(
                store_ids.insert(&value.identity),
                "maintenance store paths alias"
            );
        }
    }
    for (path, value) in &versions {
        if path.parent() != Some(home)
            || path
                .file_name()
                .is_none_or(|name| !store_name(&name.to_string_lossy()))
        {
            anyhow::ensure!(
                value
                    .as_ref()
                    .is_none_or(|value| !store_ids.contains(&value.identity)),
                "maintenance source aliases a store"
            );
        }
    }
    Ok(versions)
}

struct RawInfo {
    counts: Option<StoreCounts>,
    device: Option<String>,
    requests: Vec<crate::forget::Request>,
    healthy: bool,
}
fn raw_info(home: &Path) -> Result<RawInfo> {
    let path = raw::path(home);
    if !path.try_exists()? {
        return Ok(RawInfo {
            counts: None,
            device: None,
            requests: Vec::new(),
            healthy: false,
        });
    }
    let result = crate::migrate::with_preview_v1(&path, |conn, _| {
        let healthy = crate::db::quick_check(conn, "raw.db").is_ok();
        let counts = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM records), (SELECT COUNT(*) FROM ops)",
                [],
                |row| {
                    Ok(StoreCounts {
                        records: row.get::<_, i64>(0)? as u64,
                        ops: row.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .ok();
        Ok(RawInfo {
            counts,
            device: crate::db::device_id(conn).ok(),
            requests: requests_in(conn)?,
            healthy,
        })
    });
    match result {
        Err(error) if damaged(&error) => Ok(RawInfo {
            counts: None,
            device: None,
            requests: Vec::new(),
            healthy: false,
        }),
        result => result,
    }
}

struct Selection {
    device: String,
    records: Vec<Segment>,
    bad_records: Vec<Segment>,
    ops: Vec<Segment>,
    bad_ops: Vec<Segment>,
}
fn select(backup: &Path, device: Option<String>) -> Result<Selection> {
    let all = segments(backup, Kind::Records)?;
    let device = match device {
        Some(device) => device,
        None => unique_device(&all)?,
    };
    let (records, bad_records): (Vec<_>, Vec<_>) = all
        .into_iter()
        .filter(|s| s.device == device)
        .partition(|s| damage(&s.path).is_none());
    anyhow::ensure!(
        !records.is_empty(),
        "no usable backup segment of device {device} in {}",
        backup.display()
    );
    let (mut ops, mut bad_ops, mut next) = (Vec::new(), Vec::new(), 1);
    for segment in segments(backup, Kind::Ops)?
        .into_iter()
        .filter(|s| s.device == device)
    {
        if bad_ops.is_empty() && segment.first == next && damage(&segment.path).is_none() {
            next = segment.last + 1;
            ops.push(segment);
        } else {
            bad_ops.push(segment);
        }
    }
    Ok(Selection {
        device,
        records,
        bad_records,
        ops,
        bad_ops,
    })
}
fn backup_preview(selection: &Selection) -> Result<BackupPreview> {
    let lines = |segments: &[Segment]| -> Option<u64> {
        segments.iter().try_fold(0, |total, segment| {
            Some(
                total
                    + read_segment(&segment.path)
                        .ok()?
                        .lines()
                        .filter(|line| !line.is_empty())
                        .count() as u64,
            )
        })
    };
    let compressed_bytes = selection
        .records
        .iter()
        .chain(&selection.bad_records)
        .chain(&selection.ops)
        .chain(&selection.bad_ops)
        .try_fold(0, |sum, s| {
            Ok::<_, anyhow::Error>(sum + std::fs::metadata(&s.path)?.len())
        })?;
    Ok(BackupPreview {
        compressed_bytes,
        record_segments: selection.records.len() as u64,
        invalid_record_segments: selection.bad_records.len() as u64,
        op_segments: selection.ops.len() as u64,
        skipped_op_segments: selection.bad_ops.len() as u64,
        record_first: selection.records.iter().map(|s| s.first).min(),
        record_last: selection.records.iter().map(|s| s.last).max(),
        op_prefix_through: selection.ops.last().map_or(0, |s| s.last),
        record_lines: lines(&selection.records),
        op_lines: lines(&selection.ops),
        exact_drops_known: false,
    })
}

pub(crate) fn preview_restore(home: &Path) -> Result<MaintenancePreview> {
    preview_maintenance(home, MaintenanceOperation::Restore)
}
pub(crate) fn preview_maintenance(
    home: &Path,
    operation: MaintenanceOperation,
) -> Result<MaintenancePreview> {
    preview_plan(home, operation).map(|(shown, _, _)| shown)
}
type Versions = Vec<(PathBuf, Option<FileVersion>)>;
type LogSnapshot = (Vec<crate::forget::Request>, crate::forget::Report);
fn preview_plan(
    home: &Path,
    operation: MaintenanceOperation,
) -> Result<(MaintenancePreview, Versions, LogSnapshot)> {
    let backup_dir = dir(home).map_err(|e| e.context(MaintenanceCode::InvalidConfig))?;
    let capture = crate::capture::Settings::load(home)
        .map_err(|e| e.context(MaintenanceCode::InvalidConfig))?;
    let files =
        preview_files(home, &backup_dir).map_err(|e| e.context(MaintenanceCode::InvalidSource))?;
    anyhow::ensure!(
        !home.join("raw.db.restored").try_exists()?,
        MaintenanceCode::RecoveryRequired
    );
    let info = raw_info(home).map_err(|e| e.context(MaintenanceCode::InvalidSource))?;
    if operation == MaintenanceOperation::Rebuild {
        anyhow::ensure!(info.healthy, MaintenanceCode::RecoveryRequired);
        anyhow::ensure!(info.counts.is_some(), MaintenanceCode::InvalidSource);
    }
    let knowledge_present = home.join("knowledge.db").try_exists()?;
    let cached_vectors = if knowledge_present {
        crate::migrate::with_preview_v1(&home.join("knowledge.db"), |conn, _| {
            let has: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='vectors')",
                [],
                |r| r.get(0),
            )?;
            Ok(if has {
                conn.query_row("SELECT COUNT(*) FROM vectors", [], |r| r.get::<_, i64>(0))? as u64
            } else {
                0
            })
        })
        .ok()
    } else {
        None
    };
    let (logged, report) = crate::forget::logged(home);
    let forget_requests = info
        .requests
        .iter()
        .chain(&logged)
        .map(|r| &r.job)
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u64;
    let backup = if operation == MaintenanceOperation::Restore {
        Some(backup_preview(&select(&backup_dir, info.device)?)?)
    } else {
        None
    };
    let mut shown = MaintenancePreview {
        key: String::new(),
        operation,
        raw: info.counts,
        knowledge_present,
        cached_vectors,
        kept_files: files
            .iter()
            .filter(|(p, v)| {
                p.file_name()
                    .is_some_and(|n| kept_name(&n.to_string_lossy()))
                    && v.is_some()
            })
            .count() as u64,
        staged_partial: home.join("raw.db.restoring").try_exists()?,
        forget_requests,
        forget_log_warnings: report.problems.len() as u64,
        backup,
        backup_dir: backup_dir.clone(),
        hybrid_ready: false,
    };
    anyhow::ensure!(
        files == preview_files(home, &backup_dir)?,
        MaintenanceCode::Stale
    );
    let canonical = if home.try_exists()? {
        home.canonicalize()?
    } else {
        std::env::current_dir()?.join(home)
    };
    shown.key = crate::forget::hash(&serde_json::to_vec(&(
        "oboete:maintenance-preview:v1",
        canonical,
        crate::db::store_file(home),
        &backup_dir,
        crate::db::store_file(&backup_dir),
        capture.rules.version(),
        &files,
        &shown,
    ))?);
    Ok((shown, files, (logged, report)))
}

pub(crate) struct Consent {
    files: Versions,
    logs: LogSnapshot,
    home: String,
    backup: PathBuf,
    backup_identity: String,
}
pub(crate) fn consent(
    home: &Path,
    operation: MaintenanceOperation,
    expected: Option<&str>,
) -> Result<Option<Consent>> {
    let Some(expected) = expected else {
        return Ok(None);
    };
    let (shown, files, logs) =
        preview_plan(home, operation).map_err(|e| e.context(MaintenanceCode::Stale))?;
    anyhow::ensure!(shown.key == expected, MaintenanceCode::Stale);
    Ok(Some(Consent {
        files,
        logs,
        home: crate::db::store_file(home),
        backup_identity: crate::db::store_file(&shown.backup_dir),
        backup: shown.backup_dir,
    }))
}
impl Consent {
    pub(crate) fn check_logs(&self) -> Result<()> {
        for (path, before) in &self.files {
            if path.file_name() == Some(std::ffi::OsStr::new("forget.log")) {
                anyhow::ensure!(
                    version(path).map_err(|e| e.context(MaintenanceCode::Stale))? == *before,
                    MaintenanceCode::Stale
                );
            }
        }
        Ok(())
    }

    /// The caller owns exclusive raw.lock. Logs were read outside it; here only their identity,
    /// length and modification time are compared, so no log I/O occurs under a raw write fence.
    pub(crate) fn check_locked(&self, home: &Path) -> Result<()> {
        anyhow::ensure!(
            self.home == crate::db::store_file(home),
            MaintenanceCode::Stale
        );
        anyhow::ensure!(
            self.backup_identity == crate::db::store_file(&self.backup),
            MaintenanceCode::Stale
        );
        let paths =
            preview_paths(home, &self.backup).map_err(|e| e.context(MaintenanceCode::Stale))?;
        anyhow::ensure!(
            paths
                == self
                    .files
                    .iter()
                    .map(|(p, _)| p.clone())
                    .collect::<Vec<_>>(),
            MaintenanceCode::Stale
        );
        for (path, before) in &self.files {
            if path.file_name() == Some(std::ffi::OsStr::new("forget.log")) {
                let now = match std::fs::symlink_metadata(path) {
                    Ok(metadata) => {
                        anyhow::ensure!(metadata.is_file(), MaintenanceCode::Stale);
                        Some((
                            crate::db::store_file(path),
                            metadata.len(),
                            metadata.modified()?,
                        ))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e).context(MaintenanceCode::Stale),
                };
                anyhow::ensure!(
                    now == before
                        .as_ref()
                        .map(|v| (v.identity.clone(), v.bytes, v.modified)),
                    MaintenanceCode::Stale
                );
            } else {
                anyhow::ensure!(
                    version(path).map_err(|e| e.context(MaintenanceCode::Stale))? == *before,
                    MaintenanceCode::Stale
                );
            }
        }
        Ok(())
    }
}

/// D11: the most record bytes one segment holds (Task 12 may halve it).
pub const SEGMENT_BYTES: usize = 8 << 20;
/// D11: how often a running worker backs up besides at its idle exit.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// The most a segment may decompress to: its cap plus one record or one append of ops past it
/// (capture keeps a body far below this, `raw::MAX_BATCH_OPS` and `MAX_BATCH_BYTES` an append).
const MAX_SEGMENT_BYTES: u64 = (SEGMENT_BYTES as u64) + (128 << 20);

/// `[backup]` in config.toml (spec 1.5: the location is a user setting).
#[derive(Debug, Default, serde::Deserialize)]
struct Settings {
    #[serde(default)]
    backup: Location,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    dir: Option<PathBuf>,
}

/// The backup directory: `[backup] dir` (relative to the home), else `<home>/backups`. Only
/// `[backup]` is read, so a mistake elsewhere in config.toml does not stop backups.
pub fn dir(home: &Path) -> Result<PathBuf> {
    let path = home.join("config.toml");
    let dir = match std::fs::read_to_string(&path) {
        Ok(text) => location(&text)
            .with_context(|| format!("{}: [backup]", path.display()))?
            .map(|d| home.join(d)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    Ok(dir.unwrap_or_else(|| home.join("backups")))
}

/// `[backup] dir` as config.toml's text has it, as `dir` reads it.
pub(crate) fn location(text: &str) -> Result<Option<PathBuf>> {
    Ok(toml::from_str::<Settings>(text)?.backup.dir)
}

/// Records and ops are backed up alike, each in its own segments with its own cursor (milestone 3
/// D1): an op is often appended when no record is, and the records' cursor would never reach it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Records,
    Ops,
}

impl Kind {
    fn suffix(self) -> &'static str {
        match self {
            Kind::Records => ".seg.zst",
            Kind::Ops => ".ops.zst",
        }
    }
    /// This device's highest seq (records) or op seq (ops) in raw.
    fn top(self, raw: &Raw) -> Result<i64> {
        match self {
            Kind::Records => raw.max_seq(),
            Kind::Ops => raw.max_op_seq(),
        }
    }
    /// The first one raw holds after `at`.
    fn next(self, raw: &Raw, at: i64) -> Result<Option<i64>> {
        Ok(match self {
            Kind::Records => raw.after(raw.device(), at, 1)?.first().map(|r| r.seq),
            Kind::Ops => raw
                .ops_after(raw.device(), at, 1)?
                .first()
                .map(|o| o.op_seq),
        })
    }
    fn lines(self, raw: &Raw, at: i64) -> Result<Vec<(i64, String)>> {
        match self {
            Kind::Records => raw.export_lines(at, SEGMENT_BYTES),
            Kind::Ops => raw.export_op_lines(at, SEGMENT_BYTES),
        }
    }
}

/// One segment file, by the seqs its name gives.
struct Segment {
    device: String,
    first: i64,
    last: i64,
    path: PathBuf,
}

fn name(device: &str, first: i64, last: i64, kind: Kind) -> String {
    format!("{device}-{first:012}-{last:012}{}", kind.suffix())
}

/// Native segment-name grammar, without directory or file I/O. The bool identifies ops.
pub(crate) fn segment_name(name: &std::ffi::OsStr) -> Option<(bool, String, i64, i64)> {
    let name = name.to_str()?;
    for (ops, suffix) in [(false, Kind::Records.suffix()), (true, Kind::Ops.suffix())] {
        let Some(stem) = name.strip_suffix(suffix) else {
            continue;
        };
        let mut parts = stem.rsplitn(3, '-');
        let (Some(last), Some(first), Some(device)) = (parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        return Some((
            ops,
            device.to_owned(),
            first.parse().ok()?,
            last.parse().ok()?,
        ));
    }
    None
}

/// The segments of `kind` in `dir`, in (device, first seq) order; files of other names are not
/// ours.
fn segments(dir: &Path, kind: Kind) -> Result<Vec<Segment>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if let Some((ops, device, first, last)) = path.file_name().and_then(segment_name)
            && ops == (kind == Kind::Ops)
        {
            out.push(Segment {
                device,
                first,
                last,
                path,
            });
        }
    }
    out.sort_by(|a, b| (&a.device, a.first).cmp(&(&b.device, b.first)));
    Ok(out)
}

/// The records of this home's raw.db above the last backed-up seq, then its ops above the last
/// backed-up op seq, as sealed segments (records first: a restore then never holds a window op
/// whose records it lacks).
/// Returns the last segment written, `None` when there was nothing new. (The worker calls `run`.)
#[cfg(test)]
pub fn export(home: &Path) -> Result<Option<PathBuf>> {
    Ok(export_from(&raw::open(home)?, &dir(home)?)?
        .pop()
        .map(|(p, _)| p))
}

/// `export`, with the time each segment took (Task 12 measures D11's segment cap with it).
pub fn export_timed(home: &Path) -> Result<Vec<(PathBuf, std::time::Duration)>> {
    export_from(&raw::open(home)?, &dir(home)?)
}

/// Each segment written, with the time it took to read, compress and seal.
fn export_from(raw: &Raw, dir: &Path) -> Result<Vec<(PathBuf, std::time::Duration)>> {
    export_from_report(raw, dir, &mut |_| {})
}
fn export_from_report(
    raw: &Raw,
    dir: &Path,
    committed: &mut impl FnMut(&'static str),
) -> Result<Vec<(PathBuf, std::time::Duration)>> {
    let device = raw.device();
    anyhow::ensure!(
        !device.is_empty() && device.chars().all(|c| c.is_ascii_alphanumeric()),
        "device id {device:?} cannot name a segment"
    );
    let mut wrote = Vec::new();
    for kind in [Kind::Records, Kind::Ops] {
        let mut last = cursor_report(raw, dir, kind, committed)?;
        if kind.top(raw)? <= last {
            continue;
        }
        if !dir.exists() {
            // Private when oboete makes it; a directory the user chose keeps its permissions.
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            crate::db::private(dir, 0o700);
        }
        loop {
            let started = std::time::Instant::now();
            let lines = kind.lines(raw, last)?;
            let (Some(first), Some(end)) = (lines.first(), lines.last()) else {
                break;
            };
            let (first, end) = (first.0, end.0);
            let mut text = String::new();
            for (_, l) in &lines {
                text.push_str(l);
                text.push('\n');
            }
            let path = seal_report(
                dir,
                &name(device, first, end, kind),
                text.as_bytes(),
                committed,
            )?;
            wrote.push((path, started.elapsed()));
            last = end;
        }
    }
    Ok(wrote)
}

/// Where this device's backups end. MUST-M14: raw lost commits the backups hold, and its next
/// records reuse those seqs. The segments past raw's end are set aside, so that range is backed
/// up again from raw as it is reused (a loss that new records have already covered again goes
/// unseen here, as it does for the consumers' rewind: #83).
fn cursor_report(
    raw: &Raw,
    dir: &Path,
    kind: Kind,
    committed: &mut impl FnMut(&'static str),
) -> Result<i64> {
    let max = kind.top(raw)?;
    let mut mine: Vec<Segment> = segments(dir, kind)?
        .into_iter()
        .filter(|s| s.device == raw.device())
        .collect();
    mine.sort_by_key(|s| s.first);
    set_aside_report(mine.iter().filter(|s| s.last > max), &mut |_| {
        effect("backup_quarantined", committed)
    })?;
    mine.retain(|s| s.last <= max);
    // A range raw holds that no segment covers (a segment file was removed): the segments after
    // it are set aside too, so the next export writes that range and the rest again. A range raw
    // does not hold either (a segment a restore skipped) stays a gap.
    let mut end = 0;
    for (i, s) in mine.iter().enumerate() {
        if s.first > end + 1 && kind.next(raw, end)?.is_some_and(|seq| seq < s.first) {
            set_aside_report(&mine[i..], &mut |_| effect("backup_quarantined", committed))?;
            break;
        }
        end = end.max(s.last);
    }
    Ok(end)
}

/// Segments no longer trusted, renamed (the segment before its checksum: a checksum left alone
/// is written again by the next export). Their files stay on disk.
fn set_aside_report<'a>(
    segs: impl IntoIterator<Item = &'a Segment>,
    moved: &mut impl FnMut(bool) -> Result<()>,
) -> Result<()> {
    for s in segs {
        let mut first = true;
        for path in [
            s.path.clone(),
            PathBuf::from(format!("{}.sha256", s.path.display())),
        ] {
            if path.exists() {
                let name = path
                    .file_name()
                    .context("segment filename")?
                    .to_string_lossy();
                let parent = path.parent().context("segment directory")?;
                let aside = parent.join(fresh_name(parent, &format!("{name}.quarantined-"))?);
                std::fs::rename(&path, aside)?;
                moved(first)?;
                first = false;
            }
        }
    }
    Ok(())
}

/// At a worker's start, when raw may have lost commits: the backups past its end set aside
/// before new records reuse their seqs.
pub fn check(home: &Path, raw: &Raw) {
    check_report(home, raw, &mut |_| {});
}
pub(crate) fn check_report(
    home: &Path,
    raw: &Raw,
    committed: &mut impl FnMut(&'static str),
) -> u64 {
    let mut warnings = 0;
    for kind in [Kind::Records, Kind::Ops] {
        if let Err(e) = dir(home).and_then(|d| cursor_report(raw, &d, kind, committed)) {
            eprintln!("oboete: backup: {e:#}");
            warnings += 1;
        }
    }
    warnings
}

/// Sealing: the checksum, then the segment, each written to a temporary name, synced, renamed
/// and its directory entry synced before the next. A segment never exists without its checksum,
/// even after a power loss; a checksum left without its segment is written again by the next
/// export.
fn seal_report(
    dir: &Path,
    name: &str,
    data: &[u8],
    committed: &mut impl FnMut(&'static str),
) -> Result<PathBuf> {
    let z = zstd::bulk::compress(data, 3)?;
    let sum = format!("{}  {name}\n", hex(&Sha256::digest(&z)));
    write_synced(dir, &format!("{name}.sha256"), sum.as_bytes())?;
    effect("backup_written", committed)?;
    let path = write_synced(dir, name, &z)?;
    effect("backup_written", committed)?;
    Ok(path)
}

fn write_synced(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    crate::db::private(&tmp, 0o600);
    f.write_all(data)?;
    f.sync_all()?;
    let path = dir.join(name);
    std::fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A segment that does not match its checksum, or has none.
#[derive(Debug)]
pub struct Problem {
    pub path: PathBuf,
    pub what: String,
}

pub fn verify(dir: &Path) -> Result<Vec<Problem>> {
    let mut out = Vec::new();
    for s in segments(dir, Kind::Records)?
        .into_iter()
        .chain(segments(dir, Kind::Ops)?)
    {
        if let Some(what) = damage(&s.path) {
            out.push(Problem { path: s.path, what });
        }
    }
    Ok(out)
}

/// Why a segment cannot be trusted, if it cannot.
fn damage(seg: &Path) -> Option<String> {
    let sum_path = PathBuf::from(format!("{}.sha256", seg.display()));
    let Ok(sum) = std::fs::read_to_string(&sum_path) else {
        return Some("no checksum".into());
    };
    let Ok(data) = std::fs::read(seg) else {
        return Some("unreadable".into());
    };
    (!checksum_matches(&sum, &hex(&Sha256::digest(&data)))).then(|| "checksum mismatch".into())
}

pub(crate) fn checksum_matches(text: &str, sha256: &str) -> bool {
    text.split_whitespace().next() == Some(sha256)
}

/// `<name>` (and its -wal and -shm, which SQLite binds to the name) moved aside as
/// `<name>.quarantined-<ms>`, its sidecars under the names SQLite looks for beside that (`...-wal`,
/// `...-shm`), so the kept file opens with its last commits. Returns the new name of the main file.
#[cfg(test)]
pub(crate) fn quarantine(home: &Path, name: &str) -> Result<PathBuf> {
    quarantine_report(home, name, None, &mut |_| Ok(()))
}
pub(crate) fn fresh_name(home: &Path, prefix: &str) -> Result<String> {
    let mut stamp = crate::db::now_ms();
    loop {
        let name = format!("{prefix}{stamp}");
        let mut occupied = false;
        for ext in ["", "-wal", "-shm"] {
            match std::fs::symlink_metadata(home.join(format!("{name}{ext}"))) {
                Ok(_) => occupied = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        if !occupied {
            return Ok(name);
        }
        stamp = stamp
            .checked_add(1)
            .context("recovery filename exhausted")?;
    }
}
enum FileMove {
    Moved,
    Returned,
    PutBack,
}
fn quarantine_report(
    home: &Path,
    name: &str,
    guard: Option<&crate::executable::CommandHome>,
    changed: &mut impl FnMut(FileMove) -> Result<()>,
) -> Result<PathBuf> {
    let kept = fresh_name(home, &format!("{name}.quarantined-"))?;
    let main = home.join(&kept);
    let mut moved = Vec::new();
    for ext in ["-wal", "-shm", ""] {
        if let Some(guard) = guard {
            guard.check(home)?;
        }
        let from = home.join(format!("{name}{ext}"));
        if from.exists() {
            let to = home.join(format!("{kept}{ext}"));
            let result = std::fs::rename(&from, &to)
                .map_err(anyhow::Error::from)
                .and_then(|()| {
                    moved.push((from.clone(), to));
                    changed(FileMove::Moved)
                });
            if let Err(error) = result {
                // Preserve the original committed WAL on a failed quarantine. Main first if it
                // moved too; never put anything back into a replacement home.
                let put_back: Result<()> = (|| {
                    for (from, to) in moved.iter().rev() {
                        if let Some(guard) = guard {
                            guard
                                .check(home)
                                .context("quarantine put-back home changed")?;
                        }
                        anyhow::ensure!(
                            !from.try_exists()?,
                            "quarantine put-back destination exists"
                        );
                        std::fs::rename(to, from)
                            .with_context(|| format!("quarantine put back {}", from.display()))?;
                        changed(FileMove::Returned)?;
                    }
                    if !moved.is_empty() {
                        changed(FileMove::PutBack)?;
                    }
                    Ok(())
                })();
                if let Err(back) = put_back {
                    return Err(error)
                        .with_context(|| format!("quarantine put-back also failed: {back:#}"));
                }
                return Err(error).with_context(|| format!("quarantine {}", from.display()));
            }
        }
    }
    Ok(main)
}
fn restore_file_change(
    receipt: &mut RestoreReceipt,
    name: &str,
    event: FileMove,
    committed: &mut impl FnMut(&'static str),
) -> Result<()> {
    let (moves, kept, put_back, stage, returned) = if name == "knowledge.db" {
        (
            &mut receipt.effects.knowledge_files_moved,
            &mut receipt.old_knowledge_kept_files,
            &mut receipt.effects.knowledge_put_back,
            "knowledge_set_aside",
            "knowledge_put_back",
        )
    } else {
        (
            &mut receipt.effects.raw_files_quarantined,
            &mut receipt.old_raw_kept_files,
            &mut receipt.effects.raw_put_back,
            "raw_quarantined",
            "raw_put_back",
        )
    };
    match event {
        FileMove::Moved => {
            *moves += 1;
            *kept += 1;
            effect(stage, committed)
        }
        FileMove::Returned => {
            *kept -= 1;
            Ok(())
        }
        FileMove::PutBack => {
            *put_back = true;
            effect(returned, committed)
        }
    }
}

/// The device whose history the backups hold: the damaged file's own id when it can still be
/// read, else the one device the segments name. Never a guess between two.
fn device_of(home: &Path, segs: &[Segment]) -> Result<String> {
    let from_file = rusqlite::Connection::open_with_flags(
        raw::path(home),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .and_then(|conn| crate::db::device_id(&conn).map_err(|_| rusqlite::Error::InvalidQuery));
    if let Ok(id) = from_file {
        return Ok(id);
    }
    unique_device(segs)
}
fn unique_device(segs: &[Segment]) -> Result<String> {
    let mut devices: Vec<&str> = segs.iter().map(|s| s.device.as_str()).collect();
    devices.dedup();
    match devices.as_slice() {
        [one] => Ok((*one).to_owned()),
        [] => anyhow::bail!("no backup segments"),
        _ => anyhow::bail!(
            "raw.db's device id is unreadable and the backups hold {} devices",
            devices.len()
        ),
    }
}

/// MUST-M15: quarantine a damaged raw.db and rebuild it, with its device id, from the segments
/// that verify, in seq order. Returns what was done, for stderr and doctor
/// (`<home>/state/restored`).
pub fn restore(home: &Path) -> Result<String> {
    let _config = crate::settings::config_lock(home)?;
    restore_report_holding(home, None, None, &mut |_| {})
        .map(|r| r.note)
        .map_err(|f| f.cause)
}

/// Complete native raw restoration. The caller holds config.lock and owns worker admission.
pub(crate) fn restore_report_holding(
    home: &Path,
    expected: Option<&str>,
    guard: Option<&crate::executable::CommandHome>,
    committed: &mut impl FnMut(&'static str),
) -> std::result::Result<RestoreReceipt, RestoreFailure> {
    let mut receipt = RestoreReceipt::default();
    let result: Result<()> = (|| {
        if let Some(guard) = guard {
            guard.check(home)?;
        }
        let mut consent = consent(home, MaintenanceOperation::Restore, expected)?;
        #[cfg(test)]
        if let Some(after) = AFTER_CONSENT.with(|hook| hook.borrow_mut().take()) {
            after();
        }
        // Milestone 5 D1: the forget request logs are read before raw's swap lock, and written after
        // it from the restored raw.db; no log I/O under the lock.
        let (logged, mut report) = consent
            .as_mut()
            .map(|consent| std::mem::take(&mut consent.logs))
            .unwrap_or_else(|| crate::forget::logged(home));
        receipt.forget_log_warnings = report.problems.len() as u64;
        let mut note = restore_locked(
            home,
            logged,
            consent.as_ref(),
            guard,
            &mut receipt,
            committed,
        )?;
        if let Some(guard) = guard {
            guard.check(home)?;
        }
        #[cfg(test)]
        if let Some(before) = BEFORE_REOPEN.with(|hook| hook.borrow_mut().take()) {
            before();
        }
        match raw::open_report(home, guard, &mut |stage| {
            if stage == "stopped_restore_finished" {
                receipt.effects.stopped_restore_finished = true;
                receipt.effects.raw_swapped = true;
            }
            effect(stage, committed)
        })
        .and_then(|mut raw| {
            if let Some(guard) = guard {
                guard.check(home)?;
            }
            crate::forget::reconcile(home, &mut raw)
        }) {
            Ok(r) => report.problems.extend(r.problems),
            Err(e) => report.problems.push(format!("{e:#}")),
        }
        receipt.forget_log_warnings = report.problems.len() as u64;
        if let Some(guard) = guard {
            guard.check(home)?;
        }
        for p in &report.problems {
            note.push_str(&format!("; forget request log: {p}"));
        }
        receipt.note = note;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(receipt),
        Err(cause) => Err(RestoreFailure {
            receipt: Box::new(receipt),
            code: cause
                .downcast_ref::<MaintenanceCode>()
                .copied()
                .unwrap_or(MaintenanceCode::Failed),
            cause,
        }),
    }
}

/// The forget requests a raw.db that still reads holds (D1 rule 14), read under the swap lock
/// without opening it as a store. A lost database or unreadable table has none (F1); once the
/// table opens, a bad row must stop the restore rather than discard other live requests.
fn held_requests(home: &Path) -> Result<Vec<crate::forget::Request>> {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        raw::path(home),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return Ok(Vec::new());
    };
    requests_in(&conn)
}

fn requests_in(conn: &rusqlite::Connection) -> Result<Vec<crate::forget::Request>> {
    let Ok(mut st) = conn.prepare("SELECT request FROM forget_jobs") else {
        return Ok(Vec::new());
    };
    let rows = st.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let text = row.context("read live forget request row")?;
        let request: crate::forget::Request =
            serde_json::from_str(&text).context("parse live forget request")?;
        request.check().context("validate live forget request")?;
        out.push(request);
    }
    if !out.is_empty() {
        let live_home: String = conn
            .query_row("SELECT value FROM meta WHERE key='home_id'", [], |r| {
                r.get(0)
            })
            .context("read live home identity for forget requests")?;
        // A request corrupt in this live store must not be silently discarded as foreign
        // when it is later compared with the backup's potentially different home.
        anyhow::ensure!(
            out.iter().all(|request| request.home == live_home),
            "live forget request home does not match raw home identity"
        );
    }
    Ok(out)
}

fn restore_locked(
    home: &Path,
    logged: Vec<crate::forget::Request>,
    consent: Option<&Consent>,
    guard: Option<&crate::executable::CommandHome>,
    receipt: &mut RestoreReceipt,
    committed: &mut impl FnMut(&'static str),
) -> Result<String> {
    // No store is open while the file is read, rebuilt and swapped; a hook waits (or fails with
    // MUST-M16's marker) instead of writing into the file that is moved aside.
    before_swap();
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    if let Some(consent) = consent {
        consent.check_logs()?;
    }
    let _swap = raw::lock_for_swap(home)?;
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    if let Some(consent) = consent {
        consent.check_locked(home)?;
    }
    let held = held_requests(home)?;
    let dir = dir(home)?;
    let Selection {
        device,
        records: ok,
        bad_records: bad,
        ops: ops_ok,
        bad_ops: ops_bad,
    } = select(
        &dir,
        Some(device_of(home, &segments(&dir, Kind::Records)?)?),
    )?;
    receipt.skipped_segments = (bad.len() + ops_bad.len()) as u64;
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    // Validate/read live requests first. The sole complete stopped file stays authority until
    // this shared native recovery finishes it; never unlink the last store or its forgets.
    if raw::finish_stopped_restore(home)? {
        receipt.effects.stopped_restore_finished = true;
        effect("stopped_restore_finished", committed)?;
    }
    if home.join("raw.db.restored").exists() {
        quarantine_report(home, "raw.db.restored", guard, &mut |event| {
            restore_file_change(receipt, "raw.db.restored", event, committed)
        })?;
    }
    // A build left by a restore that stopped is made again: `.restoring` may be partial, and a
    // `.restored` beside a raw.db still in place came from segments that may have changed since.
    let tmp = home.join("raw.db.restoring");
    for name in ["raw.db.restoring", "raw.db.restoring-journal"] {
        let _ = std::fs::remove_file(home.join(name));
    }
    let mut rebuild = raw::Rebuild::new(&tmp, &device)?;
    receipt.effects.staging_created = true;
    fail_after("staging_created")?;
    for s in &ok {
        for line in read_segment(&s.path)?.lines().filter(|l| !l.is_empty()) {
            rebuild.add(line)?;
            receipt.replayed_records += 1;
        }
    }
    // The ops after the records, in op order up to the first segment that is damaged or does not
    // start where the one before it ended. The op log after such a hole is set aside with it: its
    // windows would move the curation checkpoint past the lost ones, whose claims go with
    // knowledge.db, and their records would never be curated again.
    for s in &ops_ok {
        for line in read_segment(&s.path)?.lines().filter(|l| !l.is_empty()) {
            rebuild.add_op(line)?;
            receipt.replayed_ops += 1;
        }
    }
    // Rule 14: what the live raw.db and the logs hold of this home's forgets, before the swap.
    // A copied store may append as a new device while keeping the same home lineage.
    let home_id = rebuild.home_id()?;
    let mut forget: Vec<_> = held.into_iter().filter(|r| r.home == home_id).collect();
    for r in logged.into_iter().filter(|r| r.home == home_id) {
        if !forget.iter().any(|f| f.job == r.job) {
            forget.push(r);
        }
    }
    rebuild.forget(forget);
    let dropped = rebuild.finish()?;
    receipt.dropped_ops = dropped as u64;
    receipt.retained_ops = receipt.replayed_ops - receipt.dropped_ops;
    let whole = home.join("raw.db.restored");
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    std::fs::rename(&tmp, &whole)?;
    receipt.effects.staging_complete = true;
    fail_after("staging_complete")?;
    // Durable before the damaged file goes: an open that finds neither would make an empty store.
    #[cfg(unix)]
    std::fs::File::open(home)?.sync_all()?;
    fail_after("staging_synced")?;
    // Reconstruction was isolated. Its denials become live at the swap: order that moment
    // after any sender's actual transmission, outside every raw SQLite transaction.
    let _dispatch = crate::dispatch::exclusive(home)?;
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    // Everything that must not outlive the old raw.db goes before the swap, so a restore that
    // stops at any point is either redone (raw.db still damaged) or complete.
    // Derived data is rebuilt from what was restored: a skipped segment leaves a gap below raw's
    // highest seq that the old index and checkpoints would still cover.
    if home.join("knowledge.db").exists() {
        quarantine_report(home, "knowledge.db", guard, &mut |event| {
            restore_file_change(receipt, "knowledge.db", event, committed)
        })?;
    }
    // A skipped segment is moved aside, so the export cursor never trusts its name and the seqs
    // it claimed are backed up again as they are reused.
    set_aside_report(bad.iter().chain(&ops_bad), &mut |first| {
        receipt.effects.segments_quarantined += u64::from(first);
        receipt.backup_files_kept += 1;
        effect("segments_quarantined", committed)
    })?;
    let kept = quarantine_report(home, "raw.db", guard, &mut |event| {
        restore_file_change(receipt, "raw.db", event, committed)
    })?;
    fail_after("raw_quarantine_complete")?;
    std::fs::rename(&whole, home.join("raw.db"))?;
    receipt.effects.raw_swapped = true;
    effect("raw_swapped", committed)?;
    #[cfg(unix)]
    {
        std::fs::File::open(home)?.sync_all()?;
        receipt.effects.directory_sync_complete = true;
        effect("directory_synced", committed)?;
    }
    let skipped: Vec<String> = bad
        .iter()
        .chain(&ops_bad)
        .filter_map(|s| s.path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let note = format!(
        "raw.db restored at {} from {} segment(s), {} record(s) and {} op(s); the damaged file is kept as {}{}{}",
        rusqlite::Connection::open_in_memory()?.query_row(
            "SELECT strftime('%Y-%m-%d %H:%M UTC', ?1 / 1000, 'unixepoch')",
            [crate::db::now_ms()],
            |r| r.get::<_, String>(0)
        )?,
        ok.len() + ops_ok.len(),
        receipt.replayed_records,
        receipt.retained_ops,
        kept.file_name().unwrap_or_default().to_string_lossy(),
        if dropped > 0 {
            format!("; {dropped} op(s) past the restored records dropped")
        } else {
            String::new()
        },
        if skipped.is_empty() {
            String::new()
        } else {
            format!(
                "; skipped segment(s), damaged or after a hole in the op log: {}",
                skipped.join(", ")
            )
        }
    );
    let state = home.join("state");
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    std::fs::create_dir_all(&state)?;
    std::fs::write(state.join("restored"), format!("{note}\n"))?;
    receipt.note_written = true;
    fail_after("restore_note")?;
    Ok(note)
}

/// A segment's lines, decompressed up to the cap.
fn read_segment(path: &Path) -> Result<String> {
    let mut text = String::new();
    zstd::Decoder::new(std::fs::File::open(path)?)?
        .take(MAX_SEGMENT_BYTES)
        .read_to_string(&mut text)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(text)
}

/// A hook that found raw.db damaged while a worker held the lock asks that worker to restore it.
fn restore_request(home: &Path) -> PathBuf {
    home.join("state").join("restore-wanted")
}

pub fn request_restore(home: &Path) {
    let path = restore_request(home);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, "");
}

pub fn restore_requested(home: &Path) -> bool {
    restore_request(home).exists()
}

/// Removes the request and says whether there was one.
pub fn take_restore_request(home: &Path) -> bool {
    std::fs::remove_file(restore_request(home)).is_ok()
}

/// Whether an error says the database file is damaged (not busy, not missing).
pub fn corrupt(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<rusqlite::Error>().is_some_and(|r| {
            matches!(
                r.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
            )
        })
    })
}

/// raw.db for the worker: restored from the segments when opening it says it is damaged or its
/// `quick_check` fails. Any other error stays an error.
pub fn open_raw(home: &Path) -> Result<Raw> {
    match raw::open(home).and_then(|r| r.quick_check().map(|()| r)) {
        Ok(raw) => Ok(raw),
        Err(error) if damaged(&error) => {
            eprintln!("oboete: raw.db: {error:#}; {}", restore(home)?);
            raw::open(home)
        }
        Err(error) => Err(error),
    }
}
pub(crate) fn open_raw_report(
    home: &Path,
    receipt: &mut Option<RestoreReceipt>,
    guard: Option<&crate::executable::CommandHome>,
    committed: &mut impl FnMut(&'static str),
) -> Result<Raw> {
    // Only complete reported commands use this entry; they retain config.lock across the drain.
    match raw::open_report(home, guard, &mut |stage| {
        if stage == "stopped_restore_finished" {
            let receipt = receipt.get_or_insert_with(RestoreReceipt::default);
            receipt.effects.stopped_restore_finished = true;
            receipt.effects.raw_swapped = true;
        }
        effect(stage, committed)
    })
    .and_then(|r| r.quick_check().map(|()| r))
    {
        Ok(r) => Ok(r),
        Err(e) if damaged(&e) => {
            let restored = restore_report_holding(home, None, guard, committed);
            match restored {
                Ok(report) => {
                    eprintln!("oboete: raw.db: {e:#}; {}", report.note);
                    *receipt = Some(report);
                }
                Err(failure) => {
                    *receipt = Some(*failure.receipt);
                    return Err(failure.cause);
                }
            }
            raw::open_report(home, guard, &mut |stage| {
                if stage == "stopped_restore_finished" {
                    let receipt = receipt.get_or_insert_with(RestoreReceipt::default);
                    receipt.effects.stopped_restore_finished = true;
                    receipt.effects.raw_swapped = true;
                }
                effect(stage, committed)
            })
        }
        Err(e) => Err(e),
    }
}

/// knowledge.db for the worker: a damaged one is quarantined and started empty. Every consumer
/// then rebuilds from seq 0 (spec 1.7); raw.db and the segments are not touched.
pub fn open_knowledge(home: &Path) -> Result<rusqlite::Connection> {
    open_knowledge_report(home, None, &mut |_| {})
}

pub(crate) fn open_knowledge_report(
    home: &Path,
    guard: Option<&crate::executable::CommandHome>,
    committed: &mut impl FnMut(&'static str),
) -> Result<rusqlite::Connection> {
    if let Some(guard) = guard {
        guard.check(home)?;
    }
    let checked = crate::knowledge::open_report(home, &mut || effect("stores_changed", committed))
        .and_then(|k| {
            crate::db::quick_check_without_vtabs(&home.join("knowledge.db"), "knowledge.db")
                .map(|()| k)
        });
    match checked {
        Ok(k) => Ok(k),
        Err(e) if damaged(&e) => {
            let kept = quarantine_report(home, "knowledge.db", guard, &mut |_| {
                effect("stores_changed", committed)
            })?;
            eprintln!(
                "oboete: knowledge.db: {e:#}; kept as {} and rebuilt from raw.db",
                kept.display()
            );
            // Its vectors, when they still read, come with the embedding phase's first poll.
            if let Some(guard) = guard {
                guard.check(home)?;
            }
            crate::knowledge::open_report(home, &mut || effect("stores_changed", committed))
        }
        Err(e) => Err(e),
    }
}

/// An open or `quick_check` error that says the file is damaged: SQLite's corrupt or not a
/// database, or a check that ran and reported a problem (the one error that is no
/// `rusqlite::Error`). Busy, permission and the like are not.
pub(crate) fn damaged(e: &anyhow::Error) -> bool {
    corrupt(e)
        || (e
            .chain()
            .all(|c| c.downcast_ref::<rusqlite::Error>().is_none())
            && e.to_string().contains("quick_check"))
}

/// The worker's backup step (D11): export what is new. A failure is reported and never stops
/// the worker; doctor shows how far the backups reach.
pub fn run(home: &Path, raw: &Raw) {
    run_report(home, raw, &mut |_| {});
}
pub(crate) fn run_report(home: &Path, raw: &Raw, committed: &mut impl FnMut(&'static str)) -> u64 {
    if let Err(e) = dir(home).and_then(|d| export_from_report(raw, &d, committed)) {
        eprintln!("oboete: backup: {e:#}");
        return 1;
    }
    0
}

/// Spec 7 (files): a folder a sync client uploads.
fn in_cloud_folder(path: &Path) -> Option<String> {
    path.components().find_map(|c| {
        let c = c.as_os_str().to_string_lossy();
        [
            "OneDrive",
            "iCloud Drive",
            "iCloudDrive",
            "Mobile Documents",
            "Dropbox",
            "Google Drive",
            "GoogleDrive",
        ]
        .iter()
        .any(|m| c.starts_with(m))
        .then(|| c.into_owned())
    })
}

/// doctor's backup lines, and whether they are all well (false turns doctor red).
pub fn doctor(home: &Path) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut well = true;
    let dir = match dir(home) {
        Ok(d) => d,
        Err(e) => return (vec![format!("backup: {e:#}")], false),
    };
    for (what, path) in [
        ("the data directory", home),
        ("the backup directory", dir.as_path()),
    ] {
        if let Some(folder) = in_cloud_folder(path) {
            lines.push(format!(
                "warning: {what} is inside {folder}, which a sync client uploads: {}",
                path.display()
            ));
        }
    }
    let segs = segments(&dir, Kind::Records).unwrap_or_default();
    let op_segs = segments(&dir, Kind::Ops).unwrap_or_default();
    if raw::exists(home) {
        match raw::open(home) {
            Ok(raw) => {
                let through = segs
                    .iter()
                    .filter(|s| s.device == raw.device())
                    .map(|s| s.last)
                    .max()
                    .unwrap_or(0);
                let ops_through = op_segs
                    .iter()
                    .filter(|s| s.device == raw.device())
                    .map(|s| s.last)
                    .max()
                    .unwrap_or(0);
                lines.push(format!(
                    "backup: {} segment(s) in {}, through seq {through} of {} and op {ops_through} of {}",
                    segs.len() + op_segs.len(),
                    dir.display(),
                    raw.max_seq().unwrap_or(0),
                    raw.max_op_seq().unwrap_or(0)
                ));
                // The full check reads every page: doctor only, never on an automatic path.
                match raw.integrity_check() {
                    Ok(()) => lines.push("raw.db integrity_check: ok".into()),
                    Err(e) => {
                        well = false;
                        lines.push(format!("{e:#}"));
                    }
                }
            }
            Err(e) => {
                well = false;
                lines.push(format!("raw.db: {e:#}"));
            }
        }
    }
    match verify(&dir) {
        Ok(problems) => {
            for p in problems {
                well = false;
                lines.push(format!("backup segment {}: {}", p.path.display(), p.what));
            }
        }
        Err(e) => {
            well = false;
            lines.push(format!("backup: {e:#}"));
        }
    }
    if let Ok(note) = std::fs::read_to_string(home.join("state").join("restored")) {
        lines.push(note.trim_end().to_owned());
    }
    (lines, well)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::raw::{Item, Target};

    pub(crate) fn w5b_files(root: &Path) -> Vec<(PathBuf, String)> {
        fn walk(root: &Path, path: &Path, files: &mut Vec<(PathBuf, String)>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    walk(root, &path, files);
                } else {
                    files.push((
                        path.strip_prefix(root).unwrap().into(),
                        crate::migrate::file_version(&path).unwrap(),
                    ));
                }
            }
        }
        let mut files = Vec::new();
        walk(root, root, &mut files);
        files.sort();
        files
    }

    pub(crate) fn w5b_log_change(p: &Path) -> (Vec<u8>, Vec<u8>) {
        let mut raw = raw::open(p).unwrap();
        let mut event = raw::test_event(r#"{"prompt":"synthetic confirmed log canary"}"#);
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: crate::forget::origin("synthetic", "confirmed-log"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        export(p).unwrap();
        let shown = raw
            .forget_preview(crate::forget::Target::Record {
                device: raw.device().into(),
                seq,
            })
            .unwrap();
        drop(raw);
        crate::forget::start(p, &shown).unwrap();
        let original = std::fs::read(p.join("forget.log")).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let mut changed: crate::forget::Request =
            serde_json::from_value(value["request"].clone()).unwrap();
        changed.job = "0".repeat(32);
        let body = serde_json::to_string(&changed).unwrap();
        let changed = format!(
            "{{\"sum\":\"{}\",\"request\":{body}}}\n",
            crate::forget::hash(body.as_bytes())
        )
        .into_bytes();
        assert_ne!(original, changed);
        assert_eq!(original.len(), changed.len());
        (original, changed)
    }

    #[test]
    fn w5b_same_size_and_mtime_log_changes_require_fresh_confirmation() {
        for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
            for backup_log in [false, true] {
                let home = tempfile::tempdir().unwrap();
                let p = home.path();
                let (_, changed) = w5b_log_change(p);
                let path = if backup_log {
                    dir(p).unwrap().join("forget.log")
                } else {
                    p.join("forget.log")
                };
                let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                let identity = crate::db::store_file(&path);
                let before = std::fs::read(p.join("raw.db")).unwrap();
                let preview = preview_maintenance(p, operation).unwrap();
                BEFORE_SWAP.with_borrow_mut(|hook| {
                    *hook = Some(Box::new(move || {
                        std::fs::write(&path, changed).unwrap();
                        std::fs::File::options()
                            .write(true)
                            .open(&path)
                            .unwrap()
                            .set_times(std::fs::FileTimes::new().set_modified(modified))
                            .unwrap();
                        assert_eq!(crate::db::store_file(&path), identity);
                        assert_eq!(
                            std::fs::metadata(&path).unwrap().modified().unwrap(),
                            modified
                        );
                    }));
                });
                let mut commits = Vec::new();
                let failure = match operation {
                    MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                        p,
                        Some(&preview.key),
                        crate::executable::CommandCaller::Worker,
                        &mut |event| commits.push(event.clone()),
                    ),
                    MaintenanceOperation::Restore => crate::worker::restore_report(
                        p,
                        Some(&preview.key),
                        crate::executable::CommandCaller::Worker,
                        &mut |event| commits.push(event.clone()),
                    ),
                }
                .unwrap_err();
                assert_eq!(failure.code, MaintenanceCode::Stale);
                assert!(!failure.outcome.committed());
                assert!(commits.is_empty());
                assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), before);
                assert!(!quarantined(p, "raw.db"));
                assert!(!quarantined(p, "knowledge.db"));
            }
        }
    }

    #[test]
    fn w5b_restore_uses_the_confirmed_log_snapshot_through_an_aba_change() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let (original, changed) = w5b_log_change(p);
        let paths: Vec<_> = [p.join("forget.log"), dir(p).unwrap().join("forget.log")]
            .into_iter()
            .map(|path| {
                let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                (path, modified)
            })
            .collect();
        let original_job = raw::open(p).unwrap().forget_requests().unwrap()[0]
            .job
            .clone();
        let preview = preview_restore(p).unwrap();
        let changed_paths = paths.clone();
        AFTER_CONSENT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                for (path, modified) in changed_paths {
                    std::fs::write(&path, &changed).unwrap();
                    std::fs::File::options()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(modified))
                        .unwrap();
                }
            }));
        });
        BEFORE_SWAP.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                for (path, modified) in paths {
                    std::fs::write(&path, &original).unwrap();
                    std::fs::File::options()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(modified))
                        .unwrap();
                }
            }));
        });
        crate::worker::restore_report(
            p,
            Some(&preview.key),
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap();
        let requests = raw::open(p).unwrap().forget_requests().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "a transient unconfirmed request entered the restored store"
        );
        assert_eq!(requests[0].job, original_job);
        assert!(
            crate::search::raw(p, "confirmed log canary", None, 5)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn w5b_quarantined_segments_are_counted_once_with_each_kept_file() {
        for fail_after_first_file in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 6, 3);
            let bad = segments(&p.join("backups"), Kind::Records)
                .unwrap()
                .remove(1);
            std::fs::write(&bad.path, "synthetic damaged segment").unwrap();
            let preview = preview_restore(p).unwrap();
            if fail_after_first_file {
                FAIL_AFTER.set(Some("segments_quarantined"));
            }
            let result = crate::worker::restore_report(
                p,
                Some(&preview.key),
                crate::executable::CommandCaller::Worker,
                &mut |_| {},
            );
            FAIL_AFTER.set(None);
            let outcome = if fail_after_first_file {
                *result.unwrap_err().outcome
            } else {
                result.unwrap()
            };
            let receipt = outcome.restore.unwrap();
            assert_eq!(receipt.skipped_segments, 1);
            assert_eq!(receipt.effects.segments_quarantined, 1);
            assert_eq!(
                receipt.backup_files_kept,
                if fail_after_first_file { 1 } else { 2 }
            );
            assert!(receipt.effects.committed());
        }
    }

    #[cfg(unix)]
    #[test]
    fn w5b_a_home_replaced_during_post_restore_reopen_is_untouched() {
        for stopped in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("memory");
            let replacement = root.path().join("replacement");
            let retired = root.path().join("retired");
            std::fs::create_dir(&home).unwrap();
            segmented(&home, 3, 3);
            std::fs::create_dir_all(replacement.join("state")).unwrap();
            std::fs::write(replacement.join("state/worker.lock"), "other home").unwrap();
            std::fs::write(replacement.join("canary"), "synthetic unrelated home").unwrap();
            if stopped {
                drop(raw::open(&replacement).unwrap());
                std::fs::rename(
                    replacement.join("raw.db"),
                    replacement.join("raw.db.restored"),
                )
                .unwrap();
            }
            let before = w5b_files(&replacement);
            let path = home.clone();
            BEFORE_REOPEN.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    let held = raw::lock_for_swap(&path).unwrap();
                    raw::SWAP_BLOCKED.with_borrow_mut(|hook| {
                        *hook = Some(Box::new(move || {
                            std::fs::rename(&path, &retired).unwrap();
                            std::fs::rename(&replacement, &path).unwrap();
                            drop(held);
                        }));
                    });
                }));
            });
            let result = crate::worker::restore_report(
                &home,
                None,
                crate::executable::CommandCaller::Worker,
                &mut |_| {},
            );
            BEFORE_REOPEN.with_borrow_mut(|hook| *hook = None);
            raw::SWAP_BLOCKED.with_borrow_mut(|hook| *hook = None);
            assert_eq!(
                w5b_files(&home),
                before,
                "post-restore reopen wrote into the replacement home (stopped={stopped})"
            );
            let failure = result.unwrap_err();
            assert!(failure.outcome.restore.unwrap().effects.raw_swapped);
            assert_eq!(
                failure.outcome.index.state,
                crate::worker::IndexState::Failed
            );
        }
    }

    #[test]
    fn w5b_readonly_previews_keep_the_native_stores_wals_logs_and_paths_unchanged() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("synthetic hot WAL")).unwrap();
        let knowledge = crate::knowledge::open(p).unwrap();
        knowledge
            .execute_batch("CREATE TABLE preview_canary(x); INSERT INTO preview_canary VALUES(1)")
            .unwrap();
        let before = w5b_files(p);
        let rebuild = crate::worker::preview_rebuild(p).unwrap();
        let restore = preview_restore(p).unwrap();
        assert_eq!(rebuild.raw.unwrap().records, 4);
        assert_eq!(restore.backup.unwrap().record_lines, Some(3));
        assert!(!rebuild.hybrid_ready && !restore.hybrid_ready);
        assert_ne!(rebuild.key, restore.key);
        assert_eq!(w5b_files(p), before);
        assert!(!p.join("state/rebuild.lock").exists());
        assert!(!p.join("providers.db").exists());
        let absent = p.join("unconfigured");
        assert!(crate::worker::preview_rebuild(&absent).is_err());
        assert!(preview_restore(&absent).is_err());
        assert!(!absent.exists());
    }

    #[test]
    fn w5b_changed_confirmation_refuses_before_any_data_effect() {
        for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 3, 3);
            drop(crate::knowledge::open(p).unwrap());
            let shown = preview_maintenance(p, operation).unwrap();
            std::fs::write(p.join("config.toml"), "[capture]\nstore_prompts = false\n").unwrap();
            let before = w5b_files(p);
            let mut commits = Vec::new();
            let failure = match operation {
                MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                    p,
                    Some(&shown.key),
                    crate::executable::CommandCaller::Worker,
                    &mut |event| commits.push(event.clone()),
                ),
                MaintenanceOperation::Restore => crate::worker::restore_report(
                    p,
                    Some(&shown.key),
                    crate::executable::CommandCaller::Worker,
                    &mut |event| commits.push(event.clone()),
                ),
            }
            .unwrap_err();
            assert_eq!(failure.code, MaintenanceCode::Stale);
            assert!(!failure.outcome.committed());
            assert!(commits.is_empty());
            assert_eq!(w5b_files(p), before);
        }
    }

    #[test]
    fn w5b_a_failure_after_quarantine_keeps_the_completed_swap_and_old_store_receipt() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        let shown = preview_restore(p).unwrap();
        FAIL_AFTER.set(Some("raw_quarantine_complete"));
        let failure = crate::worker::restore_report(
            p,
            Some(&shown.key),
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        FAIL_AFTER.set(None);
        let restored = failure.outcome.restore.as_ref().unwrap();
        assert_eq!(
            failure.outcome.index.state,
            crate::worker::IndexState::Complete
        );
        assert!(restored.effects.raw_swapped);
        assert!(restored.effects.stopped_restore_finished);
        assert_eq!(restored.replayed_records, 3);
        assert!(restored.old_raw_kept_files > 0);
        assert!(failure.outcome.committed());
        assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 3);
        assert!(quarantined(p, "raw.db"));
    }

    #[test]
    fn w5b_a_first_sidecar_failure_keeps_the_live_wal_and_unlogged_forget() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let mut event = raw::test_event(r#"{"prompt":"synthetic live WAL denial"}"#);
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: crate::forget::origin("synthetic", "live-wal-denial"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        let device = raw.device().to_owned();
        export(p).unwrap();
        drop(raw);
        let raw = raw::open(p).unwrap();
        let shown = raw
            .forget_preview(crate::forget::Target::Record {
                device: device.clone(),
                seq,
            })
            .unwrap();
        crate::forget::start(p, &shown).unwrap();
        for ext in ["", "-wal"] {
            std::fs::copy(
                p.join(format!("raw.db{ext}")),
                p.join(format!("saved{ext}")),
            )
            .unwrap();
        }
        drop(raw);
        for ext in ["", "-wal"] {
            std::fs::copy(
                p.join(format!("saved{ext}")),
                p.join(format!("raw.db{ext}")),
            )
            .unwrap();
        }
        for log in [p.join("forget.log"), p.join("backups/forget.log")] {
            std::fs::remove_file(log).unwrap();
        }
        let shown = preview_restore(p).unwrap();
        FAIL_AFTER.set(Some("raw_quarantined"));
        let failed = crate::worker::restore_report(
            p,
            Some(&shown.key),
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        FAIL_AFTER.set(None);
        assert!(!failed.outcome.restore.as_ref().unwrap().effects.raw_swapped);
        assert!(
            failed
                .outcome
                .restore
                .as_ref()
                .unwrap()
                .effects
                .raw_put_back
        );
        assert_eq!(
            failed.outcome.restore.as_ref().unwrap().old_raw_kept_files,
            0
        );
        assert!(failed.outcome.committed());
        let live = raw::open(p).unwrap();
        assert_eq!(live.forget_requests().unwrap().len(), 1);
        assert!(matches!(
            live.after(&device, seq - 1, 1).unwrap()[0].item,
            Item::Removed
        ));
    }

    #[test]
    fn w5b_stopped_or_damaged_rebuild_previews_require_recovery_without_finishing_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        std::fs::rename(p.join("raw.db"), p.join("raw.db.restored")).unwrap();
        let before = w5b_files(p);
        for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
            assert_eq!(
                preview_maintenance(p, operation)
                    .unwrap_err()
                    .downcast_ref::<MaintenanceCode>(),
                Some(&MaintenanceCode::RecoveryRequired)
            );
        }
        assert_eq!(w5b_files(p), before);
        std::fs::rename(p.join("raw.db.restored"), p.join("raw.db")).unwrap();
        damage_raw(p);
        let before = w5b_files(p);
        assert_eq!(
            crate::worker::preview_rebuild(p)
                .unwrap_err()
                .downcast_ref::<MaintenanceCode>(),
            Some(&MaintenanceCode::RecoveryRequired)
        );
        let restore = preview_restore(p).unwrap();
        assert!(restore.raw.is_none());
        assert_eq!(restore.backup.unwrap().record_lines, Some(3));
        assert_eq!(w5b_files(p), before);
    }

    #[test]
    fn w5b_backup_preview_reports_bad_segments_and_the_contiguous_op_prefix() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 6, 3);
        let records = segments(&p.join("backups"), Kind::Records).unwrap();
        std::fs::write(&records[1].path, b"synthetic damaged segment").unwrap();
        let mut raw = raw::open(p).unwrap();
        for _ in 0..3 {
            raw.append_ops(&[(
                raw::OpKind::Exclusion,
                serde_json::json!({"repo":"synthetic","undo":true}),
            )])
            .unwrap();
            export(p).unwrap();
        }
        drop(raw);
        let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
        std::fs::remove_file(&ops[1].path).unwrap();
        let before = w5b_files(p);
        let shown = preview_restore(p).unwrap();
        let backup = shown.backup.unwrap();
        assert_eq!(
            (backup.record_segments, backup.invalid_record_segments),
            (1, 1)
        );
        assert_eq!(
            (
                backup.op_segments,
                backup.skipped_op_segments,
                backup.op_prefix_through
            ),
            (1, 1, 1)
        );
        assert_eq!(backup.record_lines, Some(3));
        assert!(!backup.exact_drops_known);
        assert_eq!(w5b_files(p), before);
        let safe = serde_json::to_string(&preview_restore(p).unwrap()).unwrap();
        assert!(!safe.contains(&p.to_string_lossy().into_owned()));
        assert!(!safe.contains("damaged segment"));
    }

    #[test]
    fn w5b_preview_refuses_aliases_and_does_not_guess_unreadable_mixed_device_metadata() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        let segment = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        std::fs::remove_file(&segment).unwrap();
        std::fs::hard_link(p.join("raw.db"), &segment).unwrap();
        let before = w5b_files(p);
        assert!(preview_restore(p).is_err());
        assert_eq!(w5b_files(p), before);

        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        let segment = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        let other = p.join("backups/otherdevice-000000000001-000000000003.seg.zst");
        std::fs::copy(&segment, &other).unwrap();
        std::fs::copy(
            PathBuf::from(format!("{}.sha256", segment.display())),
            PathBuf::from(format!("{}.sha256", other.display())),
        )
        .unwrap();
        damage_raw(p);
        let before = w5b_files(p);
        let failed = preview_restore(p).unwrap_err();
        assert!(failed.to_string().contains("2 devices"));
        assert_eq!(w5b_files(p), before);
    }

    #[test]
    fn w5b_partial_staging_is_disclosed_and_is_not_finished_by_preview() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        std::fs::rename(p.join("raw.db"), p.join("raw.db.restoring")).unwrap();
        let before = w5b_files(p);
        let shown = preview_restore(p).unwrap();
        assert!(shown.staged_partial && shown.raw.is_none());
        assert_eq!(shown.backup.unwrap().record_lines, Some(3));
        assert!(!p.join("raw.db").exists());
        assert_eq!(w5b_files(p), before);
    }

    #[cfg(unix)]
    #[test]
    fn w5b_readonly_preview_copies_a_closed_hot_wal_without_source_sidecars() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        {
            let mut raw = raw::open(p).unwrap();
            raw.append(&raw::test_event("synthetic WAL-only event"))
                .unwrap();
            for ext in ["", "-wal"] {
                std::fs::copy(p.join(format!("raw.db{ext}")), p.join(format!("copy{ext}")))
                    .unwrap();
            }
        }
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(p.join(format!("raw.db{ext}")));
        }
        for ext in ["", "-wal"] {
            std::fs::rename(p.join(format!("copy{ext}")), p.join(format!("raw.db{ext}"))).unwrap();
        }
        let before = w5b_files(p);
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o500)).unwrap();
        let shown = crate::worker::preview_rebuild(p);
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(shown.unwrap().raw.unwrap().records, 1);
        assert_eq!(w5b_files(p), before);
        assert!(!p.join("raw.db-shm").exists());
    }

    #[test]
    fn w5b_raw_writes_after_admission_are_rechecked_under_the_swap_fence() {
        for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 3, 3);
            let shown = preview_maintenance(p, operation).unwrap();
            let root = p.to_owned();
            BEFORE_SWAP.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    raw::open(&root)
                        .unwrap()
                        .append(&raw::test_event("synthetic unconfirmed hook event"))
                        .unwrap();
                }))
            });
            let mut commits = Vec::new();
            let failure = match operation {
                MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                    p,
                    Some(&shown.key),
                    crate::executable::CommandCaller::Worker,
                    &mut |event| commits.push(event.clone()),
                ),
                MaintenanceOperation::Restore => crate::worker::restore_report(
                    p,
                    Some(&shown.key),
                    crate::executable::CommandCaller::Worker,
                    &mut |event| commits.push(event.clone()),
                ),
            }
            .unwrap_err();
            assert_eq!(failure.code, MaintenanceCode::Stale);
            assert!(!failure.outcome.committed());
            assert!(commits.is_empty());
            assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 4);
            assert!(!quarantined(p, "raw.db"));
            assert!(!quarantined(p, "knowledge.db"));
        }
    }

    #[test]
    fn w5b_the_confirmed_configuration_remains_locked_during_consumer_commits() {
        for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 3, 3);
            let shown = preview_maintenance(p, operation).unwrap();
            let mut observed = false;
            let mut committed = |event: &crate::worker::MaintenanceCommit| {
                if matches!(event, crate::worker::MaintenanceCommit::Index { .. }) {
                    let lock = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(p.join("state/config.lock"))
                        .unwrap();
                    observed = matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock));
                }
            };
            match operation {
                MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                    p,
                    Some(&shown.key),
                    crate::executable::CommandCaller::Worker,
                    &mut committed,
                ),
                MaintenanceOperation::Restore => crate::worker::restore_report(
                    p,
                    Some(&shown.key),
                    crate::executable::CommandCaller::Worker,
                    &mut committed,
                ),
            }
            .unwrap();
            assert!(
                observed,
                "configuration writers could change the confirmed rules during {operation:?}"
            );
        }
    }

    #[test]
    fn w5b_restore_receipts_retain_post_swap_failures_and_search_completion() {
        for stage in [
            "raw_swapped",
            "directory_synced",
            "restore_note",
            "index_commit",
        ] {
            if stage == "directory_synced" && !cfg!(unix) {
                continue;
            }
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 3, 3);
            let shown = preview_restore(p).unwrap();
            FAIL_AFTER.set(Some(stage));
            let failure = crate::worker::restore_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut |_| {},
            )
            .unwrap_err();
            FAIL_AFTER.set(None);
            let restored = failure.outcome.restore.as_ref().unwrap();
            assert!(restored.effects.raw_swapped, "{stage}: {failure:?}");
            assert!(failure.outcome.committed());
            assert_eq!(restored.replayed_records, 3);
            assert!(restored.old_raw_kept_files > 0);
            assert_eq!(
                failure.outcome.index.state,
                if stage == "index_commit" {
                    crate::worker::IndexState::Failed
                } else {
                    crate::worker::IndexState::Complete
                }
            );
            assert!(!failure.outcome.hybrid_ready);
        }
    }

    #[test]
    fn w5b_stopped_complete_restore_keeps_the_live_forget_authority() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let mut event = raw::test_event(r#"{"prompt":"synthetic stopped-store canary"}"#);
        event.source = "transcript".into();
        let identity = raw::ImportIdentity {
            origin: crate::forget::origin("synthetic", "stopped-authority"),
            session: crate::forget::session(&event.agent, &event.session),
            ambiguous: None,
            unverified: false,
        };
        let seq = raw
            .append_imported_origins(
                &[crate::capture::Captured {
                    event,
                    ledger: Vec::new(),
                }],
                &[identity],
                "",
                None,
            )
            .unwrap()[0];
        export(p).unwrap(); // The backup predates the sole live denial.
        let preview = raw
            .forget_preview(crate::forget::Target::Record {
                device: raw.device().into(),
                seq,
            })
            .unwrap();
        crate::forget::start(p, &preview).unwrap();
        drop(raw);
        for log in [p.join("forget.log"), p.join("backups/forget.log")] {
            std::fs::remove_file(log).unwrap();
        }
        std::fs::rename(p.join("raw.db"), p.join("raw.db.restored")).unwrap();
        restore(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.forget_requests().unwrap().len(), 1);
        assert!(matches!(
            restored.after(restored.device(), seq - 1, 1).unwrap()[0].item,
            Item::Removed
        ));
        assert!(
            crate::search::raw(p, "stopped-store canary", None, 5)
                .unwrap()
                .is_empty()
        );
    }

    fn overwrite(path: &Path, at: u64, bytes: &[u8]) {
        use std::io::{Seek, SeekFrom};
        let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.seek(SeekFrom::Start(at)).unwrap();
        f.write_all(bytes).unwrap();
    }

    fn quarantined(home: &Path, name: &str) -> bool {
        std::fs::read_dir(home).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{name}.quarantined-"))
        })
    }

    #[test]
    fn a_damaged_raw_db_is_quarantined_and_rebuilt_from_its_segments() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        for i in 0..100 {
            raw.append(&raw::test_event(&format!("event {i}"))).unwrap();
        }
        let (before, device) = (raw.hashes().unwrap(), raw.device().to_owned());
        assert!(export(p).unwrap().is_some());
        assert!(export(p).unwrap().is_none()); // nothing new
        drop(raw); // the last close checkpoints the WAL
        overwrite(&p.join("raw.db"), 4096, &[0xA5; 4096]); // page 2
        crate::worker::run_once(p).unwrap(); // quick_check fails: quarantine, restore
        assert!(quarantined(p, "raw.db"));
        let restored = raw::open(p).unwrap();
        assert_eq!(
            (restored.hashes().unwrap(), restored.device().to_owned()),
            (before, device)
        );
        assert!(
            std::fs::read_to_string(p.join("state/restored"))
                .unwrap()
                .contains("100 record(s)")
        );
        let seg = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        overwrite(&seg, 10, &[0xA5; 16]);
        assert_eq!(verify(&p.join("backups")).unwrap().len(), 1); // the checksum names it
    }

    #[test]
    fn tombstones_ledger_and_removed_records_survive_a_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let (masked, found) = crate::redact::scan(
            &format!("alpha {key} zqx-private-words tail"),
            &crate::redact::Rules::default(),
        );
        let finding = found.into_iter().next().unwrap();
        let a = raw
            .append_with_ledger(
                &raw::test_event(&masked),
                &[("/prompt".into(), finding)],
                "test",
            )
            .unwrap();
        let b = raw.append(&raw::test_event("bravo visible")).unwrap();
        let dev = raw.device().to_owned();
        raw.append_tombstone(Target::Record {
            device: dev.clone(),
            seq: b,
        })
        .unwrap();
        let at = masked.find("zqx").unwrap() as i64;
        raw.append_tombstone(Target::Range {
            device: dev.clone(),
            seq: a,
            offset: at,
            length: 17,
        })
        .unwrap();
        let before = raw.hashes().unwrap();
        let ledger = |r: &Raw| r.export_lines(0, usize::MAX).unwrap()[0].1.clone();
        let line = ledger(&raw);
        assert!(line.contains("\"ledger\":[{") && !line.contains(&key));
        export(p).unwrap();
        let seg = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        let mut text = String::new();
        zstd::Decoder::new(std::fs::File::open(&seg).unwrap())
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(!text.contains("zqx-private-words") && !text.contains("bravo"));
        drop(raw);
        std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        // The id cannot be read from the file: it comes from the one device the segments name.
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.device(), dev);
        assert_eq!(restored.hashes().unwrap(), before); // masking twice changes nothing
        assert_eq!(ledger(&restored), line);
        let recs = restored.after(&dev, 0, 10).unwrap();
        assert!(matches!(recs[1].item, Item::Removed));
    }

    #[test]
    fn a_damaged_knowledge_db_is_rebuilt_and_raw_and_segments_stay() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        for i in 0..200 {
            raw.append(&raw::test_event(&format!("note zq{i:03}x kept")))
                .unwrap();
        }
        drop(raw);
        crate::worker::run_once(p).unwrap(); // indexes and backs up at idle exit
        let seg = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        let (raw_bytes, seg_bytes) = (
            std::fs::read(p.join("raw.db")).unwrap(),
            std::fs::read(&seg).unwrap(),
        );
        overwrite(&p.join("knowledge.db"), 4096, &[0xA5; 4096]);
        crate::worker::run_once(p).unwrap();
        assert!(quarantined(p, "knowledge.db"));
        assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), raw_bytes);
        assert_eq!(std::fs::read(&seg).unwrap(), seg_bytes);
        let hits = crate::search::raw(p, "zq007x", None, 5).unwrap();
        assert_eq!(hits.first().map(|h| h.seq), Some(8));
    }

    #[test]
    fn a_knowledge_db_opens_without_checking_its_search_indexes_against_their_text() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("note zq001x kept")).unwrap();
        drop(raw);
        crate::worker::run_once(p).unwrap();
        let k = crate::knowledge::open(p).unwrap();
        // A search index whose text is gone while the index keeps its terms (raw_fts keeps no
        // text since #317; the claims' and the cards' do).
        k.execute_batch(
            "CREATE VIRTUAL TABLE kept_fts USING fts5(text, tokenize='trigram');
             INSERT INTO kept_fts(rowid, text) VALUES (1, 'note zq001x kept');
             DELETE FROM kept_fts_content;",
        )
        .unwrap();
        assert!(crate::db::quick_check(&k, "knowledge.db").is_err());
        drop(k);
        open_knowledge(p).unwrap();
        assert!(!quarantined(p, "knowledge.db"));
        let e = crate::setup::doctor(p).unwrap_err().to_string();
        assert!(e.contains("knowledge.db is damaged"), "{e}");
    }

    #[test]
    fn a_knowledge_db_whose_two_tables_share_a_page_is_rebuilt() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        drop(crate::knowledge::open(p).unwrap());
        rusqlite::Connection::open(p.join("knowledge.db"))
            .unwrap()
            .execute_batch(
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_schema SET rootpage =
                   (SELECT rootpage FROM sqlite_schema WHERE name = 'checkpoints')
                 WHERE name = 'rewinds';",
            )
            .unwrap();
        open_knowledge(p).unwrap();
        assert!(quarantined(p, "knowledge.db"));
    }

    #[test]
    fn doctor_finds_a_damaged_table_before_it_reads_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let (root, size): (i64, i64) = crate::knowledge::open(p)
            .unwrap()
            .query_row(
                "SELECT rootpage, (SELECT page_size FROM pragma_page_size)
                 FROM sqlite_schema WHERE name = 'rewinds'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let (root, size) = (root as u64, size as usize);
        overwrite(
            &p.join("knowledge.db"),
            (root - 1) * size as u64,
            &vec![0xA5; size],
        );
        let e = crate::setup::doctor(p).unwrap_err().to_string();
        assert!(e.contains("knowledge.db is damaged"), "{e}");
    }

    /// raw.db of `n` events, one segment exported after each `per` of them.
    fn segmented(p: &Path, n: usize, per: usize) {
        let mut raw = raw::open(p).unwrap();
        for i in 0..n {
            raw.append(&raw::test_event(&format!("rec zq{i:03}x")))
                .unwrap();
            if (i + 1) % per == 0 {
                export(p).unwrap();
            }
        }
    }

    fn damage_raw(p: &Path) {
        std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
    }

    #[test]
    fn a_skipped_segment_leaves_neither_its_records_in_search_nor_its_seqs_unbacked() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
        crate::worker::run_once(p).unwrap(); // index all 30
        let segs = segments(&p.join("backups"), Kind::Records).unwrap();
        assert_eq!(segs.len(), 3);
        std::fs::write(&segs[1].path, b"damaged").unwrap(); // the middle one
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        // Derived data was rebuilt: nothing of 11-20 is found, 21-30 is.
        let hits = crate::search::raw(p, "zq0", None, 100).unwrap();
        assert!(!hits.is_empty() && hits.iter().all(|h| !(11..=20).contains(&h.seq)));
        assert_eq!(crate::search::raw(p, "zq024x", None, 5).unwrap()[0].seq, 25);
        assert!(verify(&p.join("backups")).unwrap().is_empty()); // set aside, not trusted
        // The newest segment damaged: its seqs are reused and backed up again.
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 20, 10);
        let last = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&last, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let mut raw = raw::open(p).unwrap();
        assert_eq!(raw.max_seq().unwrap(), 10);
        raw.append(&raw::test_event("after the restore")).unwrap();
        drop(raw);
        export(p).unwrap();
        let segs = segments(&p.join("backups"), Kind::Records).unwrap();
        assert_eq!(segs.last().map(|s| (s.first, s.last)), Some((11, 11)));
    }

    #[test]
    fn no_store_opens_while_a_restore_swaps_the_file_and_a_stopped_swap_is_finished() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 5, 5);
        let device = raw::open(p).unwrap().device().to_owned();
        let held = raw::lock_for_swap(p).unwrap();
        let t = std::time::Instant::now();
        assert!(raw::open(p).is_err()); // a hook's write waits, then fails (MUST-M16's marker)
        assert!(t.elapsed() >= std::time::Duration::from_secs(1));
        drop(held);
        // Stopped after the damaged file was moved aside: the rebuilt one is renamed in.
        std::fs::rename(p.join("raw.db"), p.join("raw.db.restored")).unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        // A search finishes it too, before it reads.
        crate::search::raw(p, "event", None, 5).unwrap();
        assert!(p.join("raw.db").exists() && !p.join("raw.db.restored").exists());
        let raw = raw::open(p).unwrap();
        assert_eq!(
            (raw.device().to_owned(), raw.max_seq().unwrap()),
            (device, 5)
        );
    }

    #[test]
    fn a_removed_segment_is_written_again_from_raw() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
        let ranges = |p: &Path| {
            let mut r: Vec<(i64, i64)> = segments(&p.join("backups"), Kind::Records)
                .unwrap()
                .iter()
                .map(|s| (s.first, s.last))
                .collect();
            r.sort();
            r
        };
        let middle = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .into_iter()
            .find(|s| s.first == 11)
            .unwrap()
            .path;
        std::fs::remove_file(&middle).unwrap();
        export(p).unwrap();
        assert_eq!(ranges(p), [(1, 10), (11, 30)]);
    }

    #[test]
    fn records_raw_lost_are_backed_up_again_as_their_seqs_are_reused() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10); // one segment: 1-10
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq > 6", []) // lost commits (MUST-M14)
            .unwrap();
        let mut raw = raw::open(p).unwrap();
        for i in 0..2 {
            raw.append(&raw::test_event(&format!("new zn{i}x")))
                .unwrap();
        }
        drop(raw);
        export(p).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap(); // restored from the backups
        let raw = raw::open(p).unwrap();
        let bodies: Vec<String> = raw
            .after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.item {
                raw::Item::Event(e) => Some(e.body),
                _ => None,
            })
            .collect();
        assert_eq!(bodies.len(), 8, "{bodies:?}");
        assert!(
            bodies[7].contains("zn1x") && !bodies.iter().any(|b| b.contains("zq009x")),
            "{bodies:?}"
        );
    }

    #[test]
    fn a_tombstoned_key_never_reaches_a_segment_through_the_ledger() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let (masked, found) = crate::redact::scan(
            &format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"),
            &crate::redact::Rules::default(),
        );
        let body = serde_json::json!({"trigger": {"zqx-private-words": masked}}).to_string();
        let finding = found.into_iter().next().unwrap();
        let seq = raw
            .append_with_ledger(
                &raw::test_event(&body),
                &[("/trigger/zqx-private-words".into(), finding)],
                "test",
            )
            .unwrap();
        let at = body.find("zqx").unwrap() as i64;
        raw.append_tombstone(raw::Target::Range {
            device: raw.device().to_owned(),
            seq,
            offset: at,
            length: 17,
        })
        .unwrap();
        let lines = raw.export_lines(0, usize::MAX).unwrap();
        assert!(!lines[0].1.contains("zqx"), "{}", lines[0].1);
        assert!(lines[0].1.contains("~tombstoned"));
    }

    #[test]
    fn a_partial_rebuild_is_never_renamed_in() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        // A build that stopped before its records were committed keeps the `.restoring` name.
        std::fs::rename(p.join("raw.db"), p.join("raw.db.restoring")).unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 0);
        assert!(p.join("raw.db.restoring").exists());
    }

    #[test]
    fn only_damage_restores_not_a_busy_or_missing_file() {
        assert!(damaged(&anyhow::anyhow!(
            "raw.db: quick_check: *** in database main *** Page 2: btreeInitPage() returns error code 11"
        )));
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        );
        assert!(!damaged(
            &anyhow::Error::from(busy).context("raw.db: quick_check")
        ));
        let corrupt = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            None,
        );
        assert!(damaged(&anyhow::Error::from(corrupt)));
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(!damaged(&anyhow::Error::from(denied)));
    }

    #[test]
    fn a_backup_in_a_synced_folder_is_named_in_doctor() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[backup]\ndir = \"OneDrive - Contoso/oboete\"\n",
        )
        .unwrap();
        let (lines, _) = doctor(home.path());
        assert!(
            lines
                .iter()
                .any(|l| l.contains("inside OneDrive - Contoso")),
            "{lines:?}"
        );
        for p in [
            "/Users/a/Library/Mobile Documents/x",
            "/Users/a/Dropbox/x",
            "/g/Google Drive/x",
        ] {
            assert!(in_cloud_folder(Path::new(p)).is_some(), "{p}");
        }
        assert!(in_cloud_folder(Path::new("/home/a/.oboete/backups")).is_none());
    }

    fn window(to_seq: i64) -> (raw::OpKind, serde_json::Value) {
        (
            raw::OpKind::Window,
            serde_json::json!({"from_seq": 1, "to_seq": to_seq, "to_offset": null, "outcome": "curated"}),
        )
    }

    fn claim(text: &str) -> (raw::OpKind, serde_json::Value) {
        (raw::OpKind::Claim, serde_json::json!({"text": text}))
    }

    #[test]
    fn ops_survive_a_backup_and_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10);
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(10), claim("keep all timestamps in UTC")])
            .unwrap();
        let before = raw.ops_after(&dev, 0, 100).unwrap();
        drop(raw);
        export(p).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.ops_after(&dev, 0, 100).unwrap(), before);
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (10, None));
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(note.contains("10 record(s) and 2 op(s)"), "{note}");
    }

    /// docs/work-state.md L4: work state is in the op log, so a restore brings it back as it was.
    #[test]
    fn work_state_survives_a_backup_and_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10);
        let mut raw = raw::open(p).unwrap();
        let fields = |v: serde_json::Value| v.as_object().unwrap().clone();
        let state = fields(serde_json::json!({"phase": "rc2"}));
        raw.work_state("github.com/o/r", "release", &state).unwrap();
        let task = fields(serde_json::json!({"task": "notes", "status": "doing"}));
        raw.work_state("github.com/o/r", "release", &task).unwrap();
        let before = raw.work_state_entries("github.com/o/r").unwrap();
        drop(raw);
        export(p).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(
            restored.work_state_entries("github.com/o/r").unwrap(),
            before
        );
    }

    #[test]
    fn an_op_appended_after_its_records_were_backed_up_is_exported() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 5, 5);
        let mut raw = raw::open(p).unwrap();
        raw.append_ops(&[window(5)]).unwrap();
        export(p).unwrap();
        // A correction with no new record: the records' cursor is already at the top.
        raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "fix"}))])
            .unwrap();
        let wrote = export(p).unwrap().unwrap();
        assert!(
            wrote
                .to_string_lossy()
                .ends_with("-000000000002-000000000002.ops.zst")
        );
        assert!(export(p).unwrap().is_none()); // nothing new
    }

    /// D6: a restore that lost an imported batch's records, the newest or one with later
    /// segments restored, drops its checkpoint and every later op, so the next pass imports the
    /// batch again; restored again, the checkpoint stays dropped.
    #[test]
    fn a_restore_that_lost_a_batch_drops_its_checkpoint() {
        // The segment lost, its seqs, the highest seq restored and the checkpoint left.
        for (lost, seqs, max, through) in [(3, (11, 12), 10, 5), (2, (9, 10), 12, 3)] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 5, 5); // records 1-5
            let batch = |n: usize| -> Vec<crate::capture::Captured> {
                (0..n)
                    .map(|i| crate::capture::Captured {
                        event: raw::Event {
                            source: "oboete-v1".into(),
                            ..raw::test_event(&format!("v1 zq{i:03}x"))
                        },
                        ledger: Vec::new(),
                    })
                    .collect()
            };
            let checkpoint = |through| raw::Checkpoint {
                key: "oboete-v1:d1".into(),
                through,
                row: None,
                prefix: None,
            };
            // Records 6-8, 9-10 and 11-12, a segment each.
            for (n, through) in [(3, 3), (2, 5), (2, 7)] {
                let mut raw = raw::open(p).unwrap();
                raw.append_imported(&batch(n), "v", Some(&checkpoint(through)))
                    .unwrap();
                drop(raw);
                export(p).unwrap();
            }
            let lost = segments(&p.join("backups"), Kind::Records)
                .unwrap()
                .remove(lost);
            assert_eq!((lost.first, lost.last), seqs);
            std::fs::remove_file(&lost.path).unwrap();
            for _ in 0..2 {
                damage_raw(p);
                crate::worker::run_once(p).unwrap();
                let restored = raw::open(p).unwrap();
                assert_eq!(restored.max_seq().unwrap(), max);
                let checkpoints = restored.migration_checkpoints("oboete-v1:").unwrap();
                assert_eq!(checkpoints["oboete-v1:d1"].through, through);
            }
        }
    }

    #[test]
    fn a_window_op_past_the_restored_records_goes_with_every_op_after_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // segments 1-5, 6-10
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(3), claim("a")]).unwrap();
        raw.append_ops(&[window(10), claim("b")]).unwrap();
        raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "c"}))])
            .unwrap();
        drop(raw);
        export(p).unwrap();
        // The newest record segment is damaged: seqs 6-10 are not restored.
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.max_seq().unwrap(), 5);
        let kept: Vec<i64> = restored
            .ops_after(&dev, 0, 100)
            .unwrap()
            .iter()
            .map(|o| o.op_seq)
            .collect();
        assert_eq!(kept, [1, 2]);
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(
            note.contains("3 op(s) past the restored records dropped"),
            "{note}"
        );
        // Their op seqs are reused and backed up again, as record seqs are.
        drop(restored);
        let mut raw = raw::open(p).unwrap();
        raw.append_ops(&[window(5)]).unwrap();
        drop(raw);
        export(p).unwrap();
        let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
        assert_eq!(ops.last().map(|s| (s.first, s.last)), Some((3, 3)));
    }

    /// Milestone 5 D5 (Codex's adversarial review of slice 2a): a forget op past the restored
    /// records is appended again, not dropped with the ops it follows, so its uid stays forgotten
    /// with no request log to bring it back.
    #[test]
    fn a_forget_op_past_the_restored_records_is_appended_again() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // segments 1-5, 6-10
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(3), claim("a")]).unwrap();
        raw.append_ops(&[window(10), claim("b")]).unwrap();
        drop(raw);
        // As a registration writes it, with no request log beside it.
        let uid = "f".repeat(64);
        let body = serde_json::json!({"uid": uid, "job": "j"}).to_string();
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute(
                "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                 VALUES(?1, 5, 'forget', 0, ?2, 5)",
                rusqlite::params![dev, body],
            )
            .unwrap();
        export(p).unwrap();
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert!(restored.forgotten(&uid).unwrap());
        let kept: Vec<(i64, raw::OpKind)> = restored
            .ops_after(&dev, 0, 100)
            .unwrap()
            .iter()
            .map(|o| (o.op_seq, o.kind))
            .collect();
        use raw::OpKind::{Claim, Forget, Window};
        assert_eq!(kept, [(1, Window), (2, Claim), (3, Forget)]);
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(
            note.contains("2 op(s) past the restored records dropped"),
            "{note}"
        );
    }

    /// docs/cards.md K4 after a restore that lost records: a removal made at a seq the lost
    /// records had still hides the card of a window cut before the loss, as any removal its op
    /// does not list does.
    #[test]
    fn a_removal_at_a_reused_seq_hides_the_card_of_a_window_cut_before_the_loss() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        for i in 0..10 {
            let event = raw::Event {
                kind: "tool".into(),
                repo: Some("r".into()),
                ..raw::test_event("{}")
            };
            raw.append(&event).unwrap();
            if (i + 1) % 5 == 0 {
                export(p).unwrap(); // segments 1-5, 6-10
            }
        }
        // Cut over records 1 to 3 while the store held ten.
        let op = serde_json::json!({"outcome": "curated", "summary": "Kept.", "from_seq": 1,
            "from_offset": null, "to_seq": 3, "to_offset": null});
        raw.append_ops(&[(raw::OpKind::Window, op)]).unwrap();
        drop(raw);
        export(p).unwrap();
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let cards = |raw: &Raw| {
            let k = crate::knowledge::open(p).unwrap();
            let rules = crate::redact::Rules::default();
            crate::cards::recent(&k, raw, "r", 10, &rules)
                .unwrap()
                .len()
        };
        let mut raw = raw::open(p).unwrap();
        assert_eq!((raw.max_seq().unwrap(), cards(&raw)), (5, 1));
        let device = raw.device().to_owned();
        let removed = raw.append_tombstone(raw::Target::Record { device, seq: 2 });
        assert_eq!(removed.unwrap(), 6);
        assert_eq!(cards(&raw), 0);
    }

    #[test]
    fn a_hole_in_the_op_log_sets_aside_every_op_after_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10);
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        // One ops segment per export: windows to 3, 6 and 9.
        for to in [3, 6, 9] {
            raw.append_ops(&[window(to), claim("x")]).unwrap();
            export(p).unwrap();
        }
        drop(raw);
        let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
        assert_eq!(ops.len(), 3);
        std::fs::write(&ops[1].path, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        // The window to 9 verifies, but the one to 6 is lost: curation goes on after 3.
        assert_eq!(restored.max_op_seq().unwrap(), 2);
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
        let left = segments(&p.join("backups"), Kind::Ops).unwrap();
        assert_eq!(left.iter().map(|s| s.first).collect::<Vec<_>>(), [1]);
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(note.contains("after a hole in the op log"), "{note}");
        // A missing segment is a hole too.
        drop(restored);
        let mut raw = raw::open(p).unwrap();
        for to in [6, 9] {
            raw.append_ops(&[window(to)]).unwrap();
            export(p).unwrap();
        }
        drop(raw);
        let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
        std::fs::remove_file(&ops[1].path).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
    }
}
