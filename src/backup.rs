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
    /// docs/backup-points.md B4: the sound segments that end past the point, set aside at the
    /// swap. Its records past the point are not replayed; its ops are, so that the gates' ops past
    /// it are appended again (B5).
    after: Vec<Segment>,
}
fn select(backup: &Path, device: Option<String>, point: Option<&Point>) -> Result<Selection> {
    let all = segments(backup, Kind::Records)?;
    let device = match device {
        Some(device) => device,
        None => unique_device(&all)?,
    };
    let (mut records, bad_records): (Vec<_>, Vec<_>) = all
        .into_iter()
        .filter(|s| s.device == device)
        .partition(|s| damage(&s.path).is_none());
    let mut after: Vec<Segment> = Vec::new();
    if let Some(point) = point {
        after.extend(records.iter().filter(|s| s.last > point.records).cloned());
        records.retain(|s| s.first <= point.records);
    }
    // A point noted before the first record keeps none (Codex on #880).
    anyhow::ensure!(
        !records.is_empty() || point.is_some_and(|p| p.records == 0),
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
    if let Some(point) = point {
        after.extend(ops.iter().filter(|s| s.last > point.ops).cloned());
    }
    Ok(Selection {
        device,
        records,
        bad_records,
        ops,
        bad_ops,
        after,
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
        Some(backup_preview(&select(&backup_dir, info.device, None)?)?)
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
#[derive(Clone)]
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
    Ok(export_from(&raw::open(home)?, &export_dir(home)?)?
        .pop()
        .map(|(p, _)| p))
}

/// `export`, with the time each segment took (Task 12 measures D11's segment cap with it).
pub fn export_timed(home: &Path) -> Result<Vec<(PathBuf, std::time::Duration)>> {
    export_from(&raw::open(home)?, &export_dir(home)?)
}

/// Where earlier `[backup] dir` values are remembered, one a line as config.toml gave them, each a
/// JSON string (a location may hold a newline), so a forget's or a retention pass's rewrite still
/// reaches the segments a change of location left behind: the settings page moves none (Codex on
/// #552).
const DIRS: &str = "backup-dirs";

/// `dir`, its configured location remembered in `state/backup-dirs` first.
fn export_dir(home: &Path) -> Result<PathBuf> {
    if let Ok(text) = std::fs::read_to_string(home.join("config.toml"))
        && let Some(location) = location(&text)?.as_deref().and_then(Path::to_str)
    {
        let state = home.join("state");
        std::fs::create_dir_all(&state)?;
        let known = match std::fs::read_to_string(state.join(DIRS)) {
            Ok(known) => known,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).context("read state/backup-dirs"),
        };
        let line = serde_json::to_string(location)?;
        if !known.lines().any(|l| l == line) {
            write_synced(&state, DIRS, format!("{known}{line}\n").as_bytes())?;
        }
    }
    dir(home)
}

/// Every directory segments may be in: the one in use, the default, and the remembered ones.
fn segment_dirs(home: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = vec![dir(home)?, home.join("backups")];
    match std::fs::read_to_string(home.join("state").join(DIRS)) {
        Ok(text) => {
            for line in text.lines().filter(|l| !l.is_empty()) {
                let location: String =
                    serde_json::from_str(line).context("state/backup-dirs is damaged")?;
                dirs.push(home.join(location));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("read state/backup-dirs"),
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    Ok(dirs)
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
    let mut ends = [0; 2];
    for (end_of, kind) in ends.iter_mut().zip([Kind::Records, Kind::Ops]) {
        let mut last = cursor_report(raw, dir, kind, committed)?;
        *end_of = last;
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
            *end_of = end;
            // R14: a first backup of a large store is many segments, each one progress.
            crate::watchdog::progressed();
        }
    }
    if !wrote.is_empty() {
        let point = Point {
            at: crate::db::now_ms(),
            device: device.to_owned(),
            records: ends[0],
            ops: ends[1],
        };
        // The segments are whole: a point that could not be noted loses a choice, not a record.
        if let Err(e) = note_point(dir, &point) {
            eprintln!("oboete: backup point in {}: {e:#}", dir.display());
        }
    }
    Ok(wrote)
}

/// docs/backup-points.md B1: the points of a backup directory, one JSON line each.
const POINTS: &str = "points.jsonl";

/// docs/backup-points.md B1: what a backup directory held of a device when an export ended: the
/// seq and op seq its segments then ended at.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Point {
    pub(crate) at: i64,
    pub(crate) device: String,
    pub(crate) records: i64,
    pub(crate) ops: i64,
}

/// `point` appended to the directory's points and synced, on a line of its own even after a torn
/// one.
fn note_point(dir: &Path, point: &Point) -> Result<()> {
    use std::io::Seek;
    let path = dir.join(POINTS);
    let torn = match std::fs::File::open(&path) {
        Ok(mut f) if f.metadata()?.len() > 0 => {
            f.seek(std::io::SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            f.read_exact(&mut last)?;
            last[0] != b'\n'
        }
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    crate::db::private(&path, 0o600);
    let line = format!(
        "{}{}\n",
        if torn { "\n" } else { "" },
        serde_json::to_string(point)?
    );
    file.write_all(line.as_bytes())?;
    file.sync_data()?;
    Ok(())
}

/// The points a backup directory names, in the order they were noted; a line that does not read
/// as one is skipped (B1).
pub(crate) fn points(dir: &Path) -> Result<Vec<Point>> {
    match std::fs::read_to_string(dir.join(POINTS)) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", dir.join(POINTS).display())),
    }
}

/// A point's time as `oboete backup list` prints it and `restore --to` reads it.
pub(crate) fn point_time(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at).map_or_else(
        || at.to_string(),
        |t| t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
    )
}

/// docs/backup-points.md B4: `--to`'s time, RFC 3339 or `YYYY-MM-DD HH:MM[:SS]` taken as UTC, as
/// unix ms.
pub(crate) fn parse_time(text: &str) -> Result<i64> {
    let text = text.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(text) {
        return Ok(t.timestamp_millis());
    }
    [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ]
    .iter()
    .find_map(|f| chrono::NaiveDateTime::parse_from_str(text, f).ok())
    .map(|t| t.and_utc().timestamp_millis())
    .with_context(|| {
        format!("{text:?} is no time: give it as 2026-11-07T05:30:12Z or 2026-11-07 05:30 (UTC)")
    })
}

/// This store's device and the backup directory's points of it, oldest first.
fn my_points(home: &Path) -> Result<(PathBuf, String, Vec<Point>)> {
    let dir = dir(home)?;
    let device = device_of(home, &segments(&dir, Kind::Records)?)?;
    let mut mine: Vec<Point> = points(&dir)?
        .into_iter()
        .filter(|p| p.device == device)
        .collect();
    mine.sort_by_key(|p| p.at);
    Ok((dir, device, mine))
}

/// docs/backup-points.md B4: the newest point of this store's device at or before `at`.
pub(crate) fn point_at(home: &Path, at: i64) -> Result<Point> {
    let (_, _, mine) = my_points(home)?;
    mine.into_iter()
        .rev()
        .find(|p| p.at <= at)
        .with_context(|| {
            format!(
                "no backup point at or before {}: `oboete backup list` shows them",
                point_time(at)
            )
        })
}

/// docs/backup-points.md B4: the point of this store's device noted at exactly `at`, as the
/// preview and the list print its time: what `--yes` goes back to, so it is the point previewed
/// even when a backup since added a newer one (cubic on #880).
pub(crate) fn point_named(home: &Path, at: i64) -> Result<Point> {
    let (_, _, mine) = my_points(home)?;
    mine.into_iter().find(|p| p.at == at).with_context(|| {
        format!(
            "no backup point at {}: with --yes, give a point's time exactly as the preview or `oboete backup list` prints it",
            point_time(at)
        )
    })
}

/// raw.db read without changing a store (SQLite may add its `-wal` and `-shm` beside it, as any
/// open of a WAL store does), and its device.
fn raw_read(home: &Path) -> Result<(rusqlite::Connection, String)> {
    let conn = rusqlite::Connection::open_with_flags(
        raw::path(home),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .context("open raw.db to read")?;
    let device = crate::db::device_id(&conn)?;
    Ok((conn, device))
}

/// docs/backup-points.md B4: what going back to `point` keeps and takes, for its preview.
pub(crate) fn point_preview(home: &Path, point: &Point) -> Result<String> {
    let (conn, device) = raw_read(home)?;
    // Rows, not seq distance: a damaged segment a restore skipped leaves a gap (cubic on #880).
    let count = |sql: &str, at: i64| -> Result<i64> {
        Ok(conn.query_row(sql, rusqlite::params![device, at], |r| r.get(0))?)
    };
    let records = count(
        "SELECT COUNT(*) FROM records WHERE device = ?1 AND seq > ?2",
        point.records,
    )?;
    let ops = count(
        "SELECT COUNT(*) FROM ops WHERE device = ?1 AND op_seq > ?2",
        point.ops,
    )?;
    let gates = count(
        "SELECT COUNT(*) FROM ops WHERE device = ?1 AND op_seq > ?2
           AND type IN ('forget', 'exclusion')",
        point.ops,
    )?;
    Ok(format!(
        "Going back to the backup point of {}:\n\
         - keeps the records through seq {} and the ops through op {};\n\
         - takes back the {records} record(s) and {} op(s) after them: \
         the corrections, mutes, imports, summaries and work states made after the point go with them;\n\
         - keeps the {gates} forget and exclusion op(s) made after the point: forgotten text stays gone, \
         and excluded repositories stay excluded.\n\
         raw.db and knowledge.db as they are now, and the backup segments past the point, are set aside on disk, not deleted.\n\
         To go back to it, run: oboete restore --to {} --yes",
        point_time(point.at),
        point.records,
        point.ops,
        // The gates' ops after the point are kept, not taken back.
        (ops - gates).max(0),
        point_time(point.at),
    ))
}

/// docs/backup-points.md B2: this device's points, newest first: every one of the last 24 hours
/// and the newest of each day before that, or all of them.
pub(crate) fn list_points(home: &Path, all: bool) -> Result<Vec<String>> {
    let (dir, _, mine) = my_points(home)?;
    if mine.is_empty() {
        return Ok(vec![format!(
            "no backup point yet in {}: the next backup makes one (`oboete backup run`)",
            dir.display()
        )]);
    }
    let (conn, device) = raw_read(home)?;
    let seqs = |sql: &str| -> Result<Vec<i64>> {
        let mut st = conn.prepare(sql)?;
        let rows = st.query_map([&device], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    let records = seqs("SELECT seq FROM records WHERE device = ?1 ORDER BY seq")?;
    let ops = seqs("SELECT op_seq FROM ops WHERE device = ?1 ORDER BY op_seq")?;
    let after = |all: &[i64], at: i64| all.len() - all.partition_point(|&s| s <= at);
    let day = |at: i64| point_time(at)[..10].to_owned();
    let since = crate::db::now_ms() - 86_400_000;
    let mut lines = Vec::new();
    let mut days = std::collections::HashSet::new();
    for p in mine.iter().rev() {
        let newest_of_day = days.insert(day(p.at));
        if all || p.at >= since || newest_of_day {
            lines.push(format!(
                "{}  through seq {} ({} after) and op {} ({} after)",
                point_time(p.at),
                p.records,
                after(&records, p.records),
                p.ops,
                after(&ops, p.ops)
            ));
        }
    }
    Ok(lines)
}

/// docs/backup-points.md B6: the directory's points rewritten without those of `device` that
/// `keep` refuses (and any line that does not read as a point), when there are such.
fn keep_points(dir: &Path, device: &str, keep: impl Fn(&Point) -> bool) -> Result<()> {
    let text = match std::fs::read_to_string(dir.join(POINTS)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", dir.join(POINTS).display())),
    };
    let mut kept = String::new();
    let mut changed = false;
    for line in text.lines() {
        match serde_json::from_str::<Point>(line) {
            Ok(p) if p.device != device || keep(&p) => {
                kept.push_str(line);
                kept.push('\n');
            }
            _ => changed = true,
        }
    }
    if changed {
        write_synced(dir, POINTS, kept.as_bytes())?;
    }
    Ok(())
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
    if mine.iter().any(|s| s.last > max) {
        set_aside_report(mine.iter().filter(|s| s.last > max), &mut |_| {
            effect("backup_quarantined", committed)
        })?;
        // docs/backup-points.md B6: the seqs past raw's end are used again by new records.
        keep_points(dir, raw.device(), |p| match kind {
            Kind::Records => p.records <= max,
            Kind::Ops => p.ops <= max,
        })?;
    }
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

pub(crate) fn write_synced(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf> {
    write_synced_at(dir, name, data, None)
}

/// `write_synced`, which a test can stop at `stage` after the write and before the rename: what
/// such a stop of a purge's write leaves is `leftover`'s (2c, Greptile on #453).
fn write_synced_at(
    dir: &Path,
    name: &str,
    data: &[u8],
    stage: Option<&'static str>,
) -> Result<PathBuf> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    crate::db::private(&tmp, 0o600);
    f.write_all(data)?;
    f.sync_all()?;
    if let Some(stage) = stage {
        fail_after(stage)?;
    }
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

/// Whether a seal vouches for a segment of hash `sha256`: any of its lines (a purge's replacement
/// gives it two while it runs, milestone 5 D5 2c), by the first word, as `sha256sum` writes one.
pub(crate) fn checksum_matches(text: &str, sha256: &str) -> bool {
    text.lines()
        .any(|line| line.split_whitespace().next() == Some(sha256))
}

/// What a purge of the ops segments did, or would do (milestone 5 D5, 2c), or of the record
/// segments (slice 3c-2).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OpsPurge {
    /// Live segments whose ops of a forgotten uid (whose lines of a forgotten record) were
    /// rewritten without their text.
    pub live: usize,
    /// Segments set aside, likewise.
    pub aside: usize,
    /// Segments removed: no seal vouched for them, and they could not be read whole or held a line
    /// of which it cannot be told whether it holds the text (`Line::Unknown`).
    pub removed: usize,
}

/// Milestone 5 D5 (2c, decisions 2-4): every ops segment of the backup location, of any device,
/// live or set aside, with each line of an op of a uid in `jobs` given the body
/// `{"forgotten":"<job>"}` and every other line byte for byte. A segment its seal vouches for is
/// replaced so that no moment leaves it without a seal line it matches (`select` would drop it and
/// every ops segment after it); one no seal vouches for, a damaged one or one set aside, which no
/// restore reads, is rewritten in place, or removed when it cannot be read whole or holds a line
/// that cannot be judged; what a write that stopped left under a hidden name is removed
/// (`leftover`). Run under the worker lock, as export and restore are, so no one reads or writes a
/// segment meanwhile.
pub(crate) fn purge_ops(
    home: &Path,
    jobs: &std::collections::HashMap<String, String>,
    judge: &raw::Judge,
) -> Result<OpsPurge> {
    walk(
        home,
        Kind::Ops,
        &|text| purge_lines(text, |l| purged_line(l, jobs, judge)),
        true,
    )
}

/// Slice 3e-4: whether the backup location holds an ops segment, live or set aside, by its name
/// alone: one may hold an op raw.db does not (Codex on #487).
pub(crate) fn any_ops_segment(home: &Path) -> Result<bool> {
    let dir = dir(home)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let name = entry?.file_name();
        if name
            .to_str()
            .is_some_and(|n| segment(n, Kind::Ops) || set_aside(n, Kind::Ops))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What `purge_ops` would do for `jobs`, counted without changing a file (2c, decision 9).
pub(crate) fn ops_to_purge(
    home: &Path,
    jobs: &std::collections::HashMap<String, String>,
) -> Result<OpsPurge> {
    walk(
        home,
        Kind::Ops,
        &|text| purge_lines(text, |l| purged_line(l, jobs, &raw::Judge::default())),
        false,
    )
}

/// Slice 3c-2: every record segment of the backup location, of any device, live or set aside, with
/// each event line of a forgotten record (its identity in `origins`) made the Removed line an export
/// writes for it now, as `purge_ops` rewrites the ops segments: by the same steps, and with the
/// same removals.
pub(crate) fn purge_record_segments(
    home: &Path,
    origins: &raw::ForgottenOrigins,
    masks: &raw::Masks,
) -> Result<OpsPurge> {
    walk(
        home,
        Kind::Records,
        &|text| purge_lines(text, |l| purged_record_line(l, origins, masks)),
        true,
    )
}

/// The segments of `kind` walked, each one's text given to `judge` (its rewrite, if any line
/// changes, and whether a line could not be judged).
fn walk(
    home: &Path,
    kind: Kind,
    judge: &dyn Fn(&str) -> (Option<String>, bool),
    apply: bool,
) -> Result<OpsPurge> {
    walk_some(home, kind, &|_| true, judge, apply)
}

/// `walk` over the segments whose name `wanted` takes.
fn walk_some(
    home: &Path,
    kind: Kind,
    wanted: &dyn Fn(&str) -> bool,
    judge: &dyn Fn(&str) -> (Option<String>, bool),
    apply: bool,
) -> Result<OpsPurge> {
    let mut done = OpsPurge::default();
    for dir in segment_dirs(home)? {
        walk_dir(&dir, kind, wanted, judge, apply, &mut done)?;
    }
    Ok(done)
}

/// `walk_some` in one directory.
fn walk_dir(
    dir: &Path,
    kind: Kind,
    wanted: &dyn Fn(&str) -> bool,
    judge: &dyn Fn(&str) -> (Option<String>, bool),
    apply: bool,
    done: &mut OpsPurge,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", dir.display())),
    };
    let mut segments = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            if apply && leftover(name) {
                std::fs::remove_file(&path)?;
            }
        } else if segment(name, kind) && wanted(name) {
            // Under the names oboete gives only, as for the set-aside ones (Codex on #453).
            segments.push((path, false));
        } else if set_aside(name, kind) && wanted(name) {
            segments.push((path, true));
        }
    }
    // In name order, a device's in the order it wrote them: a purge that stops between two stops
    // at the same one each time.
    segments.sort();
    for (path, aside) in segments {
        let name = path.file_name().and_then(|n| n.to_str()).context("name")?;
        let seal_path = dir.join(format!("{name}.sha256"));
        let seal = match std::fs::read_to_string(&seal_path) {
            Ok(seal) if !aside => Some(seal),
            Ok(_) => None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("read {}", seal_path.display())),
        };
        let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let sha = hex(&Sha256::digest(&data));
        let rewritten = if aside {
            &mut done.aside
        } else {
            &mut done.live
        };
        if let Some(seal) = seal.filter(|seal| checksum_matches(seal, &sha)) {
            // Stopped after its rename: the seal's second line is this content's (d).
            if apply && seal.lines().count() > 1 {
                write_synced(
                    dir,
                    &format!("{name}.sha256"),
                    seal_line(&sha, name).as_bytes(),
                )?;
            }
            if let (Some(text), _) = judge(&read_whole(&data)?) {
                if apply {
                    replace(dir, name, &sha, text.as_bytes())?;
                }
                *rewritten += 1;
            }
            continue;
        }
        // 2c, decision 4: no seal vouches for it, and no reader of oboete uses it. It is removed
        // when it is not read whole, or holds a line of which it cannot be told (`Line::Unknown`,
        // Codex on #453): it may hold the text.
        match read_whole(&data).map(|text| judge(&text)) {
            Ok((None, false)) => {}
            Ok((Some(text), false)) => {
                if apply {
                    let z = zstd::bulk::compress(text.as_bytes(), 3)?;
                    write_synced_at(dir, name, &z, Some("purge_rewrite_unrenamed"))?;
                    fail_after("purge_rewrite_renamed")?;
                }
                *rewritten += 1;
            }
            Err(_) | Ok((_, true)) => {
                if apply {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("remove {}", path.display()))?;
                    if !aside && seal_path.exists() {
                        std::fs::remove_file(&seal_path)?;
                    }
                }
                done.removed += 1;
            }
        }
    }
    // What was removed stays removed past a power loss before step 4 is recorded, though no
    // rename after it synced the directory (Greptile on #453).
    #[cfg(unix)]
    if apply {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// docs/retention.md, step 2: each record segment, live or set aside, that may hold an owed record
/// (its name's seq range holds one, or it is set aside under a name that gives none) with that
/// record's `event` line made its `expired` line: its text goes, its labels, ledger and identity
/// stay. A line of another time is another record that took the seq.
pub(crate) fn expire_record_segments(
    home: &Path,
    owed: &std::collections::HashMap<(String, i64), i64>,
    at: i64,
) -> Result<OpsPurge> {
    let mut seqs: std::collections::HashMap<&str, Vec<i64>> = std::collections::HashMap::new();
    for (device, seq) in owed.keys() {
        seqs.entry(device.as_str()).or_default().push(*seq);
    }
    for list in seqs.values_mut() {
        list.sort_unstable();
    }
    let wanted = |name: &str| {
        let base = name.split(".quarantined-").next().unwrap_or(name);
        let Some((false, device, first, last)) = segment_name(std::ffi::OsStr::new(base)) else {
            return true;
        };
        seqs.get(device.as_str()).is_some_and(|list| {
            let i = list.partition_point(|s| *s < first);
            list.get(i).is_some_and(|s| *s <= last)
        })
    };
    walk_some(
        home,
        Kind::Records,
        &wanted,
        &|text| purge_lines(text, |l| expired_record_line(l, owed, at)),
        true,
    )
}

/// An `event` line of an owed record as its `expired` line; any other line as it is.
fn expired_record_line(
    line: &str,
    owed: &std::collections::HashMap<(String, i64), i64>,
    at: i64,
) -> Line {
    if line.trim().is_empty() {
        return Line::Kept;
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Line::Unknown(None);
    };
    match v["type"].as_str() {
        Some("event") => {}
        Some("removed" | "tombstone" | "expired") => return Line::Kept,
        _ => return Line::Unknown(None),
    }
    let (Some(device), Some(seq), Some(ts)) =
        (v["device"].as_str(), v["seq"].as_i64(), v["ts"].as_i64())
    else {
        return Line::Unknown(None);
    };
    if owed.get(&(device.to_owned(), seq)) != Some(&ts) {
        return Line::Kept;
    }
    let Some(fields) = v.as_object_mut() else {
        return Line::Unknown(None);
    };
    fields.remove("body");
    fields.remove("original_bytes");
    fields.insert("type".into(), "expired".into());
    fields.insert("expired".into(), at.into());
    Line::Purged(v.to_string())
}

/// The suffix of a replacement's temporary name, `.<segment>.purge` (2c, decision 3): the segment
/// grammar never matches it.
const PURGE_PART: &str = ".purge";

/// A name an ops segment's write left when it stopped before its rename, which may hold the text:
/// an export's segment or a purge's rewrite in place (`write_synced`'s `.<segment>.tmp`, of a live
/// segment or a set-aside one), a purge's replacement (`.<segment>.purge`, written again from what
/// the segment holds now) or `write_synced`'s file for it (`..<segment>.purge.tmp`, Greptile on
/// #453). No reader takes any, and under the worker lock no write is under way.
pub(crate) fn leftover(name: &str) -> bool {
    let Some(n) = name.strip_prefix('.') else {
        return false;
    };
    n.strip_prefix('.')
        .and_then(|n| n.strip_suffix(".tmp")?.strip_suffix(PURGE_PART))
        .or_else(|| n.strip_suffix(".tmp"))
        .or_else(|| n.strip_suffix(PURGE_PART))
        .is_some_and(|n| {
            [Kind::Ops, Kind::Records]
                .into_iter()
                .any(|kind| segment(n, kind) || set_aside(n, kind))
        })
}

/// Whether `name` is a segment's of `kind` exactly as export names one (`name`: a device of letters
/// and digits, the range zero-padded): `segment_name` takes any device, an empty or a hidden one's
/// too, and any number (Codex on #453).
fn segment(name: &str, kind: Kind) -> bool {
    segment_name(std::ffi::OsStr::new(name)).is_some_and(|(ops, device, first, last)| {
        ops == (kind == Kind::Ops)
            && !device.is_empty()
            && device.chars().all(|c| c.is_ascii_alphanumeric())
            // An export's range starts at 1 and never runs backwards (Codex on #453).
            && first >= 1
            && last >= first
            && self::name(&device, first, last, kind) == name
    })
}

/// Whether `name` is a segment's of `kind` set aside as an export names it (`<segment>.quarantined-
/// <ms>`, `fresh_name`): a file of another name is not oboete's to rewrite or remove (Codex on
/// #453).
fn set_aside(name: &str, kind: Kind) -> bool {
    name.rsplit_once(".quarantined-")
        .is_some_and(|(seg, stamp)| {
            !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit()) && segment(seg, kind)
        })
}

fn seal_line(sha: &str, name: &str) -> String {
    format!("{sha}  {name}\n")
}

/// 2c, decision 3: segment `name` (of hash `old`) replaced by `text` in four steps, each synced
/// with its directory before the next: (a) the new content written under a name no reader takes;
/// (b) the seal given a second line, the new hash; (c) the new content renamed over the segment;
/// (d) the seal left with the new hash alone. A stop between any two leaves a segment its seal
/// vouches for, and the next purge goes on from what it finds.
fn replace(dir: &Path, name: &str, old: &str, text: &[u8]) -> Result<()> {
    let z = zstd::bulk::compress(text, 3)?;
    let new = hex(&Sha256::digest(&z));
    let part = format!(".{name}{PURGE_PART}");
    write_synced_at(dir, &part, &z, Some("purge_part_unrenamed"))?;
    fail_after("purge_segment_written")?;
    let doubled = format!("{}{}", seal_line(old, name), seal_line(&new, name));
    write_synced(dir, &format!("{name}.sha256"), doubled.as_bytes())?;
    fail_after("purge_seal_doubled")?;
    std::fs::rename(dir.join(&part), dir.join(name))?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    fail_after("purge_segment_renamed")?;
    write_synced(
        dir,
        &format!("{name}.sha256"),
        seal_line(&new, name).as_bytes(),
    )?;
    fail_after("purge_seal_single")
}

/// A segment's whole text, or an error when it does not decompress to text within the size a
/// segment can have (`read_segment` would cut it there).
fn read_whole(data: &[u8]) -> Result<String> {
    let mut text = String::new();
    zstd::Decoder::new(data)?
        .take(MAX_SEGMENT_BYTES + 1)
        .read_to_string(&mut text)?;
    anyhow::ensure!(
        text.len() as u64 <= MAX_SEGMENT_BYTES,
        "a segment past its size"
    );
    Ok(text)
}

/// `text`'s lines as `judge` makes each: an op of a uid in the jobs given the body
/// `{"forgotten":"<job>"}` that raw.db's purge gives it (2c, decision 2), a forgotten record's line
/// made a Removed line (3c-2), every other line byte for byte (None when no line changes), and
/// whether a line was kept of which it cannot be told (`Line::Unknown`).
fn purge_lines(text: &str, judge: impl Fn(&str) -> Line) -> (Option<String>, bool) {
    let mut out = String::with_capacity(text.len());
    let (mut changed, mut unknown) = (false, false);
    for piece in text.split_inclusive('\n') {
        let (line, end) = piece
            .strip_suffix('\n')
            .map_or((piece, ""), |line| (line, "\n"));
        match judge(line) {
            Line::Purged(new) => {
                out.push_str(&new);
                changed = true;
            }
            Line::Kept => out.push_str(line),
            Line::Unknown(masked) => {
                changed |= masked.is_some();
                out.push_str(masked.as_deref().unwrap_or(line));
                unknown = true;
            }
        }
        out.push_str(end);
    }
    (changed.then_some(out), unknown)
}

/// What a purge makes of one backup line (`Raw::export_op_lines`).
enum Line {
    /// Its op holds the text of a uid in the jobs: the line with its body rewritten.
    Purged(String),
    /// Its op holds no such text.
    Kept,
    /// Whether it holds such text cannot be told: not an op line, or an op of a kind that holds a
    /// uid's text (`raw::op_uid`) whose body names none. Slice 3e-4: with the line its text masked
    /// with the rules makes, if they mask any.
    Unknown(Option<String>),
}

/// An ops segment's line: its fields, its op's kind and its op's body.
fn op_line(line: &str) -> Option<(serde_json::Value, raw::OpKind, serde_json::Value)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = raw::OpKind::from_name(v["type"].as_str()?)?;
    let body: serde_json::Value = serde_json::from_str(v["body"].as_str()?).ok()?;
    Some((v, kind, body))
}

/// Each op of every ops segment, live or set aside, as `purge_ops` reads it, with no file
/// changed: a segment or a line that cannot be read is passed over (`purge_ops` removes it when it
/// cannot judge it).
pub(crate) fn each_op(home: &Path, f: &mut raw::EachOp) -> Result<()> {
    let f = std::cell::RefCell::new(f);
    walk(
        home,
        Kind::Ops,
        &|text| {
            for (v, kind, body) in text.lines().filter_map(op_line) {
                let at = (
                    v["device"].as_str().unwrap_or(""),
                    v["op_seq"].as_i64().unwrap_or(0),
                );
                let (ts, batch) = (v["ts"].as_i64().unwrap_or(0), v["batch"].as_i64());
                (f.borrow_mut())(kind, at, ts, batch, &body);
            }
            (None, false)
        },
        false,
    )?;
    Ok(())
}

/// One backup line, judged as raw.db's purge judges its op (`raw::op_uid`); a line rewritten keeps
/// its other fields' values, in serde_json's key order: every reader parses the line. Slice 3e-4:
/// and the text left masked with the rules raw.db's ops are masked with (`Judge::text`).
fn purged_line(
    line: &str,
    jobs: &std::collections::HashMap<String, String>,
    judge: &raw::Judge,
) -> Line {
    let judged = judged_line(line, jobs, judge);
    let Some(rules) = &judge.text else {
        return judged;
    };
    let text = match &judged {
        Line::Purged(text) => text.as_str(),
        _ => line,
    };
    let masked = op_line(text).and_then(|(mut v, kind, _)| {
        let now = raw::masked_op_text(kind, v["body"].as_str()?, rules)?;
        v["body"] = now.into();
        Some(v.to_string())
    });
    match judged {
        // An op whose uid cannot be told has its text masked too, and stays one that cannot be
        // told (Codex on #487).
        Line::Unknown(_) => Line::Unknown(masked),
        judged => masked.map_or(judged, Line::Purged),
    }
}

/// `purged_line` by what forget took.
fn judged_line(
    line: &str,
    jobs: &std::collections::HashMap<String, String>,
    judge: &raw::Judge,
) -> Line {
    if line.trim().is_empty() {
        return Line::Kept;
    }
    let Some((mut v, kind, body)) = op_line(line) else {
        return Line::Unknown(None);
    };
    // Slice 3d-2: a window or turn op given the body raw.db holds now, or one raw.db does not hold
    // (a segment set aside) judged by its own (Codex and CodeRabbit on #472), and a My Work op
    // (docs/my-work-model.md S5). A line with no time is judged as written at once.
    let at = (
        v["device"].as_str().unwrap_or(""),
        v["op_seq"].as_i64().unwrap_or(0),
    );
    let (ts, batch) = (v["ts"].as_i64().unwrap_or(0), v["batch"].as_i64());
    // One whose holding a uid's text cannot be told is Unknown (`Judge::verdict`).
    let Ok(now) = judge.verdict(kind, at, ts, batch, &body, jobs) else {
        return Line::Unknown(None);
    };
    match now {
        Some(now) if v["body"].as_str() != Some(now.as_str()) => {
            v["body"] = now.into();
            Line::Purged(v.to_string())
        }
        _ => Line::Kept,
    }
}

/// One record backup line (`Raw::export_lines`), judged as raw.db's record purge judges its row
/// (3c-2): an event line whose identity a denial names becomes the Removed line an export writes for
/// that row now (its device, seq, original metadata, home lineage and import identity); a Removed or
/// tombstone line stays. The identity is a live record's from its device, seq and time, an import's
/// its `import_identity`. An imported event line without one stays: an import is given its identity
/// when it is recorded and never later, so it had none and no forget can name it (a scope forget
/// passes it over). A line that does not parse, or a live one without its time, cannot be judged.
/// Slice 3e-1: an event line of a record a rescan masked, not forgotten, is the line an export
/// writes for it now (`masked_record_line`).
fn purged_record_line(line: &str, origins: &raw::ForgottenOrigins, masks: &raw::Masks) -> Line {
    if line.trim().is_empty() {
        return Line::Kept;
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Line::Unknown(None);
    };
    match v["type"].as_str() {
        // docs/retention.md: an expired record is forgotten as an event is.
        Some("event" | "expired") => {}
        Some("removed" | "tombstone") => return Line::Kept,
        _ => return Line::Unknown(None),
    }
    let identity = match v["source"].as_str() {
        Some(source) if raw::is_live(source) => {
            match (v["device"].as_str(), v["seq"].as_i64(), v["ts"].as_i64()) {
                (Some(device), Some(seq), Some(ts)) => {
                    Some(crate::forget::live_origin(device, seq, ts))
                }
                _ => return Line::Unknown(None),
            }
        }
        Some(_) => v["import_identity"]["origin"].as_str().map(str::to_owned),
        None => return Line::Unknown(None),
    };
    let Some((identity, held)) = identity.and_then(|id| origins.get(&id).map(|held| (id, held)))
    else {
        return if masked_record_line(&mut v, masks) {
            Line::Purged(v.to_string())
        } else {
            Line::Kept
        };
    };
    let mut removed =
        serde_json::json!({"device": v["device"], "seq": v["seq"], "type": "removed"});
    for key in [
        "source",
        "kind",
        "ts",
        "home_id",
        "home_id_proven",
        "import_identity",
    ] {
        if let Some(value) = v.get(key) {
            removed[key] = value.clone();
        }
    }
    // As the store holds it now: a live record has one since its forget (slice 3a).
    if let Some((session, ambiguous)) = held {
        removed["import_identity"] =
            serde_json::json!({ "origin": identity, "session": session, "ambiguous": ambiguous });
    }
    Line::Purged(removed.to_string())
}

/// Slice 3e-1: event line `v` of a record a rescan masked (`masks`, by its device and seq, and its
/// time when raw.db holds its row: a line of another record at a seq used again is not it) with
/// every range of it applied to its body, as `Raw::mask_rows` writes the row, and its ledger rows
/// without their field. Whether that changed the line.
fn masked_record_line(v: &mut serde_json::Value, masks: &raw::Masks) -> bool {
    let (Some(device), Some(seq)) = (v["device"].as_str(), v["seq"].as_i64()) else {
        return false;
    };
    let Some((ts, ranges)) = masks.get(&(device.to_owned(), seq)) else {
        return false;
    };
    if !ts.is_none_or(|ts| v["ts"] == ts) {
        return false;
    }
    let mut changed = false;
    // docs/retention.md: an expired line has no body left to mask; its ledger's fields go as any
    // masked record's.
    if let Some(body) = v["body"].as_str() {
        let now = ranges.iter().fold(body.to_owned(), |t, &(offset, length)| {
            raw::masked(&t, offset, length)
        });
        changed = now != body;
        v["body"] = now.into();
    } else if v["type"] != "expired" {
        return false;
    }
    for row in v["ledger"].as_array_mut().into_iter().flatten() {
        if row["field"] != "~tombstoned" {
            row["field"] = "~tombstoned".into();
            changed = true;
        }
    }
    changed
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
    restore_report_holding_to(home, expected, guard, None, committed)
}

/// `restore_report_holding`, back to `point` when one is given (docs/backup-points.md B4).
pub(crate) fn restore_report_holding_to(
    home: &Path,
    expected: Option<&str>,
    guard: Option<&crate::executable::CommandHome>,
    point: Option<&Point>,
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
            point,
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
            // Each on its own, both kept as warnings (CodeRabbit on #694). docs/my-work-model.md
            // S5: a forget whose text this restore brought back owes its rebuild again, so what was
            // made from it is left out until its purge marks it.
            let mut report = crate::forget::reconcile(home, &mut raw).unwrap_or_else(|e| {
                crate::forget::Report {
                    problems: vec![format!("{e:#}")],
                    ..Default::default()
                }
            });
            if let Err(e) = crate::forget::owe_rebuild_again(&raw) {
                report.problems.push(format!(
                    "a forget whose text came back was not set back to its rebuild: {e:#}"
                ));
            }
            Ok(report)
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

/// A retention batch's records: device, seq and time.
type Owed = Vec<(String, i64, i64)>;

/// The forget requests (D1 rule 14) and the retention batch begun (docs/retention.md) a raw.db
/// that still reads holds, read under the swap lock without opening it as a store. A lost database
/// or unreadable table has none (F1); once the table opens, a bad row must stop the restore rather
/// than discard other live requests.
/// docs/backup-points.md B5: `device`'s exclusion ops as the live raw.db holds them, in op order,
/// those no export has backed up yet too. None when it cannot be read: a damaged store's restore
/// keeps the backups' own.
fn live_exclusions(home: &Path, device: &str) -> Vec<(i64, String)> {
    let read = || -> Result<Vec<(i64, String)>> {
        let conn = rusqlite::Connection::open_with_flags(
            raw::path(home),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut st = conn.prepare(
            "SELECT op_seq, body FROM ops WHERE device = ?1 AND type = 'exclusion' ORDER BY op_seq",
        )?;
        let rows = st.query_map([device], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    read().unwrap_or_default()
}

fn held_requests(home: &Path) -> Result<(Vec<crate::forget::Request>, Owed)> {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        raw::path(home),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return Ok(Default::default());
    };
    // docs/store-format.md: a raw.db a newer oboete wrote is not restored over while it reads:
    // the rebuild would put an older home in its place, from segments it may not read right.
    if let Ok(found) = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        && found > raw::FORMAT
    {
        return Err(raw::NewerFormat { found }.into());
    }
    Ok((requests_in(&conn)?, owed_in(&conn)?))
}

fn owed_in(conn: &rusqlite::Connection) -> Result<Owed> {
    let Ok(mut st) = conn.prepare("SELECT device, seq, ts FROM expiry_owed") else {
        return Ok(Vec::new());
    };
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect::<rusqlite::Result<_>>()
        .context("read live expiry_owed row")
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
    point: Option<&Point>,
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
    let (held, mut owed) = held_requests(home)?;
    let dir = dir(home)?;
    let Selection {
        device,
        records: ok,
        bad_records: bad,
        ops: ops_ok,
        bad_ops: ops_bad,
        after,
    } = select(
        &dir,
        Some(device_of(home, &segments(&dir, Kind::Records)?)?),
        point,
    )?;
    // A point is this device's: one noted for another names other seqs. It is checked again
    // here, under the locks, as listed still: a worker that stepped aside may have dropped it
    // since it was chosen, with the seqs past it used again (B6, cubic on #880).
    if let Some(point) = point {
        anyhow::ensure!(
            point.device == device,
            "the backup point is device {}'s, and this store is device {device}'s",
            point.device
        );
        anyhow::ensure!(
            points(&dir)?.contains(point),
            "the backup point of {} is no longer listed: `oboete backup list` shows the points there are now",
            point_time(point.at)
        );
    }
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
    #[derive(serde::Deserialize)]
    struct Seq {
        seq: i64,
    }
    for s in &ok {
        for line in read_segment(&s.path)?.lines().filter(|l| !l.is_empty()) {
            // docs/backup-points.md B4: a record past the point is not replayed.
            if let Some(point) = point
                && serde_json::from_str::<Seq>(line)
                    .context("a backup line without its seq")?
                    .seq
                    > point.records
            {
                continue;
            }
            rebuild.add(line)?;
            receipt.replayed_records += 1;
        }
        // R14: a restore of a large store, which a worker runs when asked to, is many segments,
        // each one progress (CodeRabbit on #613).
        crate::watchdog::progressed();
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
        crate::watchdog::progressed();
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
    // docs/retention.md: a retention batch begun before raw.db was lost is finished on this one:
    // the one its table holds, which a stop before `state/expiry-owed` is written leaves alone
    // (Codex on #552), and the file's, for a raw.db that no longer reads.
    owed.extend(crate::retention::read_owed(home)?);
    rebuild.expire(&owed, crate::db::now_ms())?;
    if let Some(point) = point {
        rebuild.keep_ops_through(point.ops);
    }
    rebuild.carry_exclusions(live_exclusions(home, &device));
    let raw::Finished {
        dropped,
        ops_through,
    } = rebuild.finish_report()?;
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
    // it claimed are backed up again as they are reused. docs/backup-points.md B4: so is each one
    // that ends past the point, here and not at the next export, so that an automatic restore
    // in between cannot replay what was taken back.
    set_aside_report(bad.iter().chain(&ops_bad).chain(&after), &mut |first| {
        receipt.effects.segments_quarantined += u64::from(first);
        receipt.backup_files_kept += 1;
        effect("segments_quarantined", committed)
    })?;
    // B6: before the swap, so a restore that stops leaves no point past what raw.db will hold.
    // The highest seq replayed: past it, seqs are used again. A gap below it (a damaged segment
    // skipped) is never used again, so a point past the gap still names what it named.
    let replayed = ok.iter().map(|s| s.last).max().unwrap_or(0);
    let records_through = point.map_or(replayed, |p| p.records.min(replayed));
    keep_points(&dir, &device, |p| {
        p.records <= records_through && p.ops <= ops_through
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
        "raw.db {} at {} from {} segment(s), {} record(s) and {} op(s); the {} file is kept as {}{}{}{}",
        match point {
            Some(p) => format!(
                "taken back to the backup point of {} (seq {}, op {})",
                point_time(p.at),
                p.records,
                p.ops
            ),
            None => "restored".to_owned(),
        },
        rusqlite::Connection::open_in_memory()?.query_row(
            "SELECT strftime('%Y-%m-%d %H:%M UTC', ?1 / 1000, 'unixepoch')",
            [crate::db::now_ms()],
            |r| r.get::<_, String>(0)
        )?,
        ok.len() + ops_ok.len(),
        receipt.replayed_records,
        receipt.retained_ops,
        if point.is_some() {
            "earlier"
        } else {
            "damaged"
        },
        kept.file_name().unwrap_or_default().to_string_lossy(),
        if dropped > 0 {
            format!(
                "; {dropped} op(s) past the {} dropped",
                if point.is_some() {
                    "point"
                } else {
                    "restored records"
                }
            )
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
        },
        if after.is_empty() {
            String::new()
        } else {
            format!(
                "; {} segment(s) past the point set aside, kept on disk",
                after.len()
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
    // A raw.db a newer oboete wrote is not damaged: a restore would put an older home in its
    // place (docs/store-format.md).
    if e.chain().any(|c| c.is::<crate::raw::NewerFormat>()) {
        return false;
    }
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
    if let Err(e) = export_dir(home).and_then(|d| export_from_report(raw, &d, committed)) {
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

/// docs/backup-points.md B7: doctor's line on the newest point, and whether it is late: more
/// than two days old, measured from the export's own time, with records or ops after it.
fn point_line(dir: &Path, device: &str, records: i64, ops: i64, now: i64) -> (String, bool) {
    let newest = match points(dir) {
        Ok(points) => points
            .into_iter()
            .filter(|p| p.device == device)
            .max_by_key(|p| p.at),
        Err(e) => return (format!("backup point: {e:#}"), false),
    };
    let Some(p) = newest else {
        return (
            "backup point: none yet (the next backup makes one)".to_owned(),
            false,
        );
    };
    let (after_records, after_ops) = ((records - p.records).max(0), (ops - p.ops).max(0));
    let days = (now - p.at) / 86_400_000;
    let late = days >= 2 && (after_records > 0 || after_ops > 0);
    let said = format!(
        "backup point: newest {}, {after_records} record(s) and {after_ops} op(s) after it",
        point_time(p.at)
    );
    if late {
        (
            format!("warning: {said}, {days} days old: run `oboete backup run`"),
            true,
        )
    } else {
        (said, false)
    }
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
                let (records, ops) = (raw.max_seq().unwrap_or(0), raw.max_op_seq().unwrap_or(0));
                lines.push(format!(
                    "backup: {} segment(s) in {}, through seq {through} of {records} and op {ops_through} of {ops}",
                    segs.len() + op_segs.len(),
                    dir.display(),
                ));
                let (line, late) =
                    point_line(&dir, raw.device(), records, ops, crate::db::now_ms());
                well &= !late;
                lines.push(line);
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

    /// D5 (2c, Codex and Greptile on #453): a backup line is purged when its op holds the text of
    /// a uid a job forgets, kept when it holds none (a body a purge wrote, a kind whose ops hold no
    /// uid's text, a blank line) and unknown when that cannot be told; a purge takes for oboete's
    /// only the names an export or a purge gives.
    #[test]
    fn a_purge_judges_each_backup_line_and_name() {
        let line = |kind: &str, body: serde_json::Value| {
            serde_json::json!({"device": "d0", "op_seq": 7, "type": kind, "ts": 1,
                "body": body.to_string(), "batch": 3})
            .to_string()
        };
        let jobs = std::collections::HashMap::from([("u1".to_owned(), "j1".to_owned())]);
        let secret = line(
            "correction",
            serde_json::json!({"uid": "u1", "body": "secret"}),
        );
        let Line::Purged(new) = purged_line(&secret, &jobs, &raw::Judge::default()) else {
            panic!("{secret}");
        };
        assert_eq!(
            new,
            line("correction", serde_json::json!({"forgotten": "j1"}))
        );
        for kept in [
            line(
                "correction",
                serde_json::json!({"uid": "u2", "body": "kept"}),
            ),
            line("claim", serde_json::json!({"forgotten": "j0"})),
            line("window", serde_json::json!({"note": "u1"})),
            " ".to_owned(),
        ] {
            assert!(
                matches!(
                    purged_line(&kept, &jobs, &raw::Judge::default()),
                    Line::Kept
                ),
                "{kept}"
            );
        }
        for unknown in [
            line("claim", serde_json::json!({"quote": "secret"})),
            line(
                "claim",
                serde_json::json!({"forgotten": "j0", "quote": "secret"}),
            ),
            line(
                "correction",
                serde_json::json!({"forgotten": null, "body": "secret"}),
            ),
            line("future", serde_json::json!({"uid": "u1"})),
            secret[..40].to_owned(),
        ] {
            assert!(
                matches!(
                    purged_line(&unknown, &jobs, &raw::Judge::default()),
                    Line::Unknown(None)
                ),
                "{unknown}"
            );
        }

        let seg = "d0-000000000001-000000000002.ops.zst";
        assert!(segment(seg, Kind::Ops) && !segment(&format!(".{seg}"), Kind::Ops));
        for name in [
            "d-1-2.ops.zst",
            "-000000000001-000000000002.ops.zst",
            "d0-0000000000001-000000000002.ops.zst",
            "d0-000000000001-000000000002.records.zst",
            "d.0-000000000001-000000000002.ops.zst",
        ] {
            assert!(!segment(name, Kind::Ops), "{name}");
        }
        assert!(set_aside(
            &format!("{seg}.quarantined-1700000000000"),
            Kind::Ops
        ));
        for name in [
            "notes.ops.zst.quarantined-old".to_owned(),
            format!("{seg}.quarantined-"),
            format!("{seg}.quarantined-1.sha256"),
            format!("{seg}.sha256.quarantined-1"),
            "d0-000000000001-000000000002.records.zst.quarantined-1".to_owned(),
        ] {
            assert!(!set_aside(&name, Kind::Ops), "{name}");
        }
        for name in [
            format!(".{seg}.tmp"),
            format!(".{seg}.purge"),
            format!("..{seg}.purge.tmp"),
            format!(".{seg}.quarantined-1.tmp"),
        ] {
            assert!(leftover(&name), "{name}");
        }
        for name in [
            format!("{seg}.tmp"),
            format!(".{seg}.sha256.tmp"),
            format!("...{seg}.purge.tmp"),
            format!(".{seg}.purge.purge"),
            ".notes.tmp".to_owned(),
        ] {
            assert!(!leftover(&name), "{name}");
        }
        // Slice 3c-2: a record segment's names, likewise.
        let rec = "d0-000000000001-000000000002.seg.zst";
        assert!(segment(rec, Kind::Records) && !segment(rec, Kind::Ops));
        assert!(!segment(seg, Kind::Records));
        let aside = format!("{rec}.quarantined-1700000000000");
        assert!(set_aside(&aside, Kind::Records) && !set_aside(&aside, Kind::Ops));
        for name in [
            format!(".{rec}.tmp"),
            format!("..{rec}.purge.tmp"),
            format!(".{aside}.tmp"),
        ] {
            assert!(leftover(&name), "{name}");
        }
    }

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

    /// docs/store-format.md: a refused open is never damage, whatever context names it.
    #[test]
    fn a_newer_format_is_never_damage() {
        let newer = anyhow::Error::new(raw::NewerFormat { found: 9 }).context("quick_check");
        assert!(!damaged(&newer));
    }

    /// docs/store-format.md: `oboete restore` does not put an older home in place of a raw.db a
    /// newer oboete wrote: it is refused, and raw.db stays where it is, as it was.
    #[test]
    fn a_restore_refuses_a_raw_db_a_newer_oboete_wrote() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("kept")).unwrap();
        assert!(export(p).unwrap().is_some());
        drop(raw);
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute_batch(&format!("PRAGMA user_version = {}", raw::FORMAT + 1))
            .unwrap();
        let before = std::fs::read(p.join("raw.db")).unwrap();
        let e = crate::worker::restore(p).unwrap_err();
        assert!(e.is::<raw::NewerFormat>(), "{e:#}");
        assert!(!quarantined(p, "raw.db"));
        assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), before);
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
            .unwrap()
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

    /// R14: a restore marks each segment it replays, so that a large one, which a worker runs
    /// when asked to after its loop has begun, is not taken for a stuck loop (CodeRabbit on #613).
    #[test]
    fn a_restore_marks_each_segment_it_replays() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
        damage_raw(p);
        let watched = crate::watchdog::Watched::here();
        open_raw(p).unwrap();
        assert_eq!(watched.marks(), 3);
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
            .unwrap()
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

    /// docs/backup-points.md test 1: an export that writes a segment notes one point with both
    /// cursors, one that writes nothing notes none, and a torn line costs only itself.
    #[test]
    fn an_export_notes_a_point_only_when_it_writes() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // two exports: 1-5, 6-10
        let dir = p.join("backups");
        let cursors = |dir: &Path| -> Vec<(i64, i64)> {
            points(dir)
                .unwrap()
                .iter()
                .map(|x| (x.records, x.ops))
                .collect()
        };
        assert_eq!(cursors(&dir), [(5, 0), (10, 0)]);
        export(p).unwrap(); // nothing new
        assert_eq!(cursors(&dir), [(5, 0), (10, 0)]);
        // A line an export stopped in the middle of.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join(POINTS))
            .unwrap();
        f.write_all(b"{\"at\": 17").unwrap();
        drop(f);
        let mut raw = raw::open(p).unwrap();
        raw.append_ops(&[window(10), claim("a")]).unwrap();
        drop(raw);
        export(p).unwrap(); // ops only
        assert_eq!(cursors(&dir), [(5, 0), (10, 0), (10, 2)]);
    }

    /// A store of three points: A (5 records, no op), B (A and two ops) and C, past B with a mute
    /// (before any op made from records, so only the point's cut takes it), five records whose own
    /// times are older than every point (as an import's are), a window, an exclusion and a forget.
    /// The home, the device and the forgotten uid.
    fn three_points() -> (tempfile::TempDir, String, String) {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 5, 5); // A
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(5), claim("a")]).unwrap();
        drop(raw);
        export(p).unwrap(); // B
        let mut raw = raw::open(p).unwrap();
        raw.append_ops(&[(
            raw::OpKind::Correction,
            serde_json::json!({"uid": "a".repeat(64), "muted": true}),
        )])
        .unwrap();
        for i in 0..5 {
            let mut old = raw::test_event(&format!("imported zi{i}x"));
            old.ts = 1_000 + i; // 1970: older than every point
            raw.append(&old).unwrap();
        }
        raw.append_ops(&[window(10), claim("b")]).unwrap();
        raw.exclude("github.com/o/secret", false).unwrap();
        let uid = "f".repeat(64);
        raw.append_ops(&[(
            raw::OpKind::Forget,
            serde_json::json!({"uid": uid, "job": "j"}),
        )])
        .unwrap();
        drop(raw);
        export(p).unwrap(); // C
        (home, dev, uid)
    }

    fn op_kinds(raw: &Raw, dev: &str) -> Vec<(i64, raw::OpKind)> {
        raw.ops_after(dev, 0, 100)
            .unwrap()
            .iter()
            .map(|o| (o.op_seq, o.kind))
            .collect()
    }

    /// docs/backup-points.md tests 2, 4 and 5: going back to a point keeps its records and ops,
    /// whatever the records' own times, and the forget and exclusion ops after it; a mute after
    /// it goes; no point past it stays; and an older point can be gone back to after a new one.
    #[test]
    fn going_back_to_a_point_keeps_its_prefix_and_the_gates_after_it() {
        let (home, dev, uid) = three_points();
        let p = home.path();
        let dir = p.join("backups");
        let [a, b, c]: [Point; 3] = points(&dir).unwrap().try_into().unwrap();
        assert_eq!((b.records, b.ops, c.records, c.ops), (5, 2, 10, 7));
        let said = crate::worker::restore_to(p, &b).unwrap();
        assert!(said.contains("taken back to the backup point"), "{said}");
        let raw = raw::open(p).unwrap();
        assert_eq!(raw.max_seq().unwrap(), 5);
        use raw::OpKind::{Claim, Exclusion, Forget, Window};
        // The gates in their op order: the exclusion (op 6) before the forget (op 7).
        assert_eq!(
            op_kinds(&raw, &dev),
            [(1, Window), (2, Claim), (3, Exclusion), (4, Forget)]
        );
        assert!(raw.forgotten(&uid).unwrap());
        assert_eq!(raw.exclusions().unwrap(), ["github.com/o/secret"]);
        drop(raw);
        // C is gone; the restore's own drain backed up what it appended again, a point of its own.
        let after_b = points(&dir).unwrap();
        assert_eq!(after_b[..2], [a.clone(), b.clone()]);
        assert!(!after_b.contains(&c), "{after_b:?}");
        assert!(after_b.iter().all(|x| x.records <= 5), "{after_b:?}");
        let live = |kind| -> Vec<(i64, i64)> {
            segments(&dir, kind)
                .unwrap()
                .iter()
                .map(|s| (s.first, s.last))
                .collect()
        };
        assert_eq!(live(Kind::Records), [(1, 5)]);
        assert_eq!(live(Kind::Ops), [(1, 2), (3, 4)]);
        // A new point after it, then back to A: the gates are kept again.
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("after zaq")).unwrap();
        drop(raw);
        export(p).unwrap();
        assert_eq!(points(&dir).unwrap().last().map(|x| x.records), Some(6));
        crate::worker::restore_to(p, &a).unwrap();
        let raw = raw::open(p).unwrap();
        assert_eq!(raw.max_seq().unwrap(), 5);
        assert_eq!(op_kinds(&raw, &dev), [(1, Exclusion), (2, Forget)]);
        assert_eq!(raw.exclusions().unwrap(), ["github.com/o/secret"]);
        let after_a = points(&dir).unwrap();
        assert_eq!(after_a.first(), Some(&a));
        assert!(!after_a.contains(&b), "{after_a:?}");
        assert!(after_a.iter().all(|x| x.records <= 5), "{after_a:?}");
    }

    /// docs/backup-points.md test 3: what a point took back stays back when raw.db is damaged
    /// right after the swap, before any export, and is restored: the segments past the point were
    /// set aside at the swap.
    #[test]
    fn a_damaged_store_after_going_back_is_restored_to_the_point() {
        let (home, dev, _) = three_points();
        let p = home.path();
        let b = points(&p.join("backups")).unwrap().remove(1);
        let config = crate::settings::config_lock(p).unwrap();
        restore_report_holding_to(p, None, None, Some(&b), &mut |_| {}).unwrap();
        drop(config);
        damage_raw(p);
        let raw = open_raw(p).unwrap(); // the automatic restore
        assert_eq!(raw.max_seq().unwrap(), 5);
        // The ops the backups held through the point. The gates' ops appended again after the swap
        // are backed up by the restore's own drain, which this test skips to reach the moment
        // before it, as any new op waits for the next backup.
        use raw::OpKind::{Claim, Window};
        assert_eq!(op_kinds(&raw, &dev), [(1, Window), (2, Claim)]);
    }

    /// docs/backup-points.md test 6: the preview changes no file and names the point that `--yes`
    /// then goes back to, by the time the list prints.
    #[test]
    fn the_preview_changes_nothing_and_names_the_point_yes_takes() {
        let (home, _, _) = three_points();
        let p = home.path();
        let b = points(&p.join("backups")).unwrap().remove(1);
        fn files(p: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
            let mut out = Vec::new();
            for d in [p.to_owned(), p.join("backups")] {
                for e in std::fs::read_dir(d).unwrap() {
                    let e = e.unwrap();
                    let m = e.metadata().unwrap();
                    let name = e.file_name().to_string_lossy().into_owned();
                    // SQLite makes these beside a WAL store on any open, a hook's too.
                    if m.is_file() && !name.ends_with("-wal") && !name.ends_with("-shm") {
                        out.push((e.path(), m.len(), m.modified().unwrap()));
                    }
                }
            }
            out.sort();
            out
        }
        let before = files(p);
        let to = point_time(b.at);
        let preview = point_preview(p, &point_at(p, parse_time(&to).unwrap()).unwrap()).unwrap();
        assert!(preview.contains(&to), "{preview}");
        assert!(
            preview.contains("the 5 record(s) and 3 op(s) after them"),
            "{preview}"
        );
        assert!(
            preview.contains("keeps the 2 forget and exclusion op(s)"),
            "{preview}"
        );
        assert_eq!(files(p), before);
        let chosen = point_at(p, parse_time(&to).unwrap()).unwrap();
        assert_eq!(chosen, b);
        assert!(
            preview.contains(&format!("oboete restore --to {to} --yes")),
            "{preview}"
        );
        // `--yes` takes the point at exactly the time the preview printed, and no other.
        assert_eq!(point_named(p, b.at).unwrap(), b);
        let off = point_named(p, b.at + 1).unwrap_err().to_string();
        assert!(off.contains("exactly as the preview"), "{off}");
        let none = point_at(p, b.at - 86_400_000).unwrap_err().to_string();
        assert!(none.contains("no backup point at or before"), "{none}");
        assert_eq!(
            parse_time("2026-11-07 05:30").unwrap(),
            parse_time("2026-11-07T05:30:00Z").unwrap()
        );
        assert_eq!(
            parse_time("2026-11-07T14:30:00+09:00").unwrap(),
            parse_time("2026-11-07T05:30:00Z").unwrap()
        );
        assert!(parse_time("yesterday").is_err());
    }

    /// docs/backup-points.md test 11 (B5, Codex on #880): an exclusion raw.db holds and no export
    /// backed up yet is carried across the swap, by a restore to a point and by a restore of the
    /// whole backup alike, and the same op is never appended twice.
    #[test]
    fn an_exclusion_no_backup_holds_yet_is_carried_across_a_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 5, 5); // a point at seq 5, no op
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.exclude("github.com/o/new", false).unwrap(); // after the last export
        drop(raw);
        let a = points(&p.join("backups")).unwrap().remove(0);
        crate::worker::restore_to(p, &a).unwrap();
        let raw = raw::open(p).unwrap();
        assert_eq!(raw.exclusions().unwrap(), ["github.com/o/new"]);
        use raw::OpKind::Exclusion;
        assert_eq!(op_kinds(&raw, &dev), [(1, Exclusion)]);
        drop(raw);
        let mut raw = raw::open(p).unwrap();
        raw.exclude("github.com/o/new", true).unwrap(); // the undo, not backed up either
        drop(raw);
        crate::worker::restore(p).unwrap();
        let raw = raw::open(p).unwrap();
        assert!(raw.exclusions().unwrap().is_empty());
        assert_eq!(op_kinds(&raw, &dev), [(1, Exclusion), (2, Exclusion)]);
    }

    /// Codex on #880: a point noted before the first record (an export of ops alone) can be gone
    /// back to: no record is kept, and its ops are.
    #[test]
    fn a_point_before_any_record_can_be_gone_back_to() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.exclude("github.com/o/first", false).unwrap();
        drop(raw);
        export(p).unwrap();
        segmented(p, 3, 3);
        let first = points(&p.join("backups")).unwrap().remove(0);
        assert_eq!((first.records, first.ops), (0, 1));
        crate::worker::restore_to(p, &first).unwrap();
        let raw = raw::open(p).unwrap();
        assert_eq!(raw.max_seq().unwrap(), 0);
        assert_eq!(raw.exclusions().unwrap(), ["github.com/o/first"]);
    }

    /// cubic on #880: a point dropped after it was chosen (raw.db lost records past it, and their
    /// seqs were used again) is refused under the locks, and raw.db is left as it was.
    #[test]
    fn a_point_no_longer_listed_is_refused() {
        let (home, _, _) = three_points();
        let p = home.path();
        let dir = p.join("backups");
        let [a, b, c]: [Point; 3] = points(&dir).unwrap().try_into().unwrap();
        let kept = [a, c]
            .iter()
            .map(|x| format!("{}\n", serde_json::to_string(x).unwrap()))
            .collect::<String>();
        std::fs::write(dir.join(POINTS), kept).unwrap();
        let why = crate::worker::restore_to(p, &b).unwrap_err().to_string();
        assert!(why.contains("no longer listed"), "{why}");
        assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 10);
    }

    /// cubic on #880 (B6): when the segment that ends at the point is damaged, the restore brings
    /// back less than the point named, and the seqs past what came back are used again: that
    /// point goes too.
    #[test]
    fn a_point_whose_records_did_not_all_come_back_is_dropped() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // points (5, 0) and (10, 0)
        let dir = p.join("backups");
        let [five, ten]: [Point; 2] = points(&dir).unwrap().try_into().unwrap();
        let newest = segments(&dir, Kind::Records).unwrap().remove(1).path;
        std::fs::write(&newest, b"damaged").unwrap();
        crate::worker::restore_to(p, &ten).unwrap();
        assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 5);
        assert_eq!(points(&dir).unwrap(), [five]);
    }

    /// docs/backup-points.md test 7: a segment the point cuts is replayed up to it and set aside,
    /// and the next export writes its first part again.
    #[test]
    fn a_segment_the_point_cuts_is_replayed_up_to_it_and_written_again() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10); // one segment: 1-10
        let dir = p.join("backups");
        let mut mid = points(&dir).unwrap().remove(0);
        mid.records = 4; // as after a rewind wrote the segments again with other bounds
        std::fs::write(
            dir.join(POINTS),
            format!("{}\n", serde_json::to_string(&mid).unwrap()),
        )
        .unwrap();
        crate::worker::restore_to(p, &mid).unwrap();
        assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 4);
        export(p).unwrap();
        let ranges: Vec<(i64, i64)> = segments(&dir, Kind::Records)
            .unwrap()
            .iter()
            .map(|s| (s.first, s.last))
            .collect();
        assert_eq!(ranges, [(1, 4)]);
    }

    /// docs/backup-points.md test 8 (B5): a restore of a damaged store that lost records keeps
    /// the exclusion ops after its first lost record, as it keeps the forget ops.
    #[test]
    fn a_restore_that_lost_records_keeps_the_exclusions_after_them() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // segments 1-5, 6-10
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(3), claim("a")]).unwrap();
        raw.append_ops(&[window(10), claim("b")]).unwrap();
        raw.exclude("github.com/o/secret", false).unwrap();
        drop(raw);
        export(p).unwrap();
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.max_seq().unwrap(), 5);
        use raw::OpKind::{Claim, Exclusion, Window};
        assert_eq!(
            op_kinds(&restored, &dev),
            [(1, Window), (2, Claim), (3, Exclusion)]
        );
        assert_eq!(restored.exclusions().unwrap(), ["github.com/o/secret"]);
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(
            note.contains("2 op(s) past the restored records dropped"),
            "{note}"
        );
    }

    /// docs/backup-points.md B6: an export that sets aside the segments past raw's end (raw lost
    /// commits, MUST-M14) drops the points past it.
    #[test]
    fn points_past_raws_end_go_with_its_segments() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // points (5, 0) and (10, 0)
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq > 6", [])
            .unwrap();
        export(p).unwrap();
        let cursors: Vec<(i64, i64)> = points(&p.join("backups"))
            .unwrap()
            .iter()
            .map(|x| (x.records, x.ops))
            .collect();
        assert_eq!(cursors, [(5, 0), (6, 0)]);
    }

    /// docs/backup-points.md test 9: doctor's newest point is late when it is two days old or more,
    /// by the export's own time, with records or ops after it.
    #[test]
    fn doctor_warns_of_an_old_point_with_records_after_it() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path();
        let day = 86_400_000;
        let point = Point {
            at: 10 * day,
            device: "d0".into(),
            records: 5,
            ops: 2,
        };
        note_point(dir, &point).unwrap();
        let (line, late) = point_line(dir, "d0", 7, 2, 12 * day);
        assert!(
            late && line.starts_with("warning:") && line.contains("2 record(s)"),
            "{line}"
        );
        assert!(line.contains("`oboete backup run`"), "{line}");
        assert!(!point_line(dir, "d0", 5, 2, 12 * day).1);
        assert!(!point_line(dir, "d0", 7, 2, 11 * day).1);
        assert!(point_line(dir, "d0", 5, 3, 12 * day).1);
        assert_eq!(
            point_line(dir, "d1", 7, 2, 12 * day),
            (
                "backup point: none yet (the next backup makes one)".to_owned(),
                false
            )
        );
    }

    /// docs/backup-points.md test 10: a backup run exports what is new and notes a point.
    #[test]
    fn a_backup_run_exports_and_notes_a_point() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("run zr1")).unwrap();
        drop(raw);
        let wrote = crate::worker::backup_run(p).unwrap();
        assert_eq!(wrote.len(), 1);
        assert_eq!(points(&p.join("backups")).unwrap().len(), 1);
        assert!(crate::worker::backup_run(p).unwrap().is_empty());
        let listed = list_points(p, false).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].ends_with("through seq 1 (0 after) and op 0 (0 after)"),
            "{listed:?}"
        );
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

    /// #775: the owner's answer to a judge's suggestion (J4) is an op as any other: a restore
    /// brings it back as it was.
    #[test]
    fn the_owners_answer_to_a_suggestion_survives_a_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10);
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        let fields = serde_json::json!({"conversation": ["claude", "s2"], "action": "join",
            "groups": [{"conversations": [["claude", "s1"], ["claude", "s2"]], "title": null,
                "links": []}]});
        let answer = crate::my_work_ops::MyWorkOp {
            job: "judge".into(),
            about: "github.com/o/r".into(),
            entry: String::new(),
            ts: 1,
            read: crate::my_work_ops::Read::default(),
            removed: vec![],
            fields: serde_json::from_value(fields).unwrap(),
            skipped: false,
            forgotten: None,
        };
        let answer = serde_json::to_value(answer).unwrap();
        raw.append_ops(&[(raw::OpKind::MyWork, answer)]).unwrap();
        let before = raw.ops_after(&dev, 0, 100).unwrap();
        drop(raw);
        export(p).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.ops_after(&dev, 0, 100).unwrap(), before);
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
        raw.work_state("github.com/o/r", "release", &state, None)
            .unwrap();
        let task = fields(serde_json::json!({"task": "notes", "status": "doing"}));
        raw.work_state("github.com/o/r", "release", &task, None)
            .unwrap();
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

    /// docs/provenance.md F1: a turn op whose own records, or a My Work op whose read spans, pass
    /// the restored records goes with every op after it, as a window op does.
    #[test]
    fn a_turn_or_my_work_op_past_the_restored_records_goes_with_every_op_after_it() {
        let turn = serde_json::json!({"agent": "claude", "session": "s", "repo": "github.com/o/r",
            "ts": 1, "from": 6, "through": 10, "read": [], "goals": [], "removed": [],
            "fields": {"request": "r", "learned": "l"}, "skipped": false, "excluded": false});
        let my_work = serde_json::json!({"job": "review", "about": "R1", "entry": "anthropic",
            "ts": 1, "read": {"spans": [[6, 10]], "ops": []}, "removed": [],
            "fields": {"summary": "s"}, "skipped": false});
        let mut kept = Vec::new();
        for (kind, body) in [(raw::OpKind::Turn, turn), (raw::OpKind::MyWork, my_work)] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 10, 5); // segments 1-5, 6-10
            let mut raw = raw::open(p).unwrap();
            let dev = raw.device().to_owned();
            raw.append_ops(&[window(3), claim("a")]).unwrap();
            raw.append_ops(&[(kind, body)]).unwrap();
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
            let seqs: Vec<i64> = restored
                .ops_after(&dev, 0, 100)
                .unwrap()
                .iter()
                .map(|o| o.op_seq)
                .collect();
            kept.push((kind, seqs));
        }
        use raw::OpKind::{MyWork, Turn};
        assert_eq!(kept, [(Turn, vec![1, 2]), (MyWork, vec![1, 2])]);
    }

    /// #690 item 3: a window op is judged by its `to_seq` alone, as the SQL before #687 did, so one
    /// with no `from_seq` past the restored records goes with every op after it; and a claim that
    /// quotes, or a window whose goals name, a record the restore did not bring back goes the same
    /// way (F1).
    #[test]
    fn an_op_judged_by_its_end_alone_or_its_quotes_goes_with_every_op_after_it() {
        let mut kept = Vec::new();
        for case in ["end alone", "quote", "goal"] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 10, 5); // segments 1-5, 6-10
            let mut raw = raw::open(p).unwrap();
            let dev = raw.device().to_owned();
            raw.append_ops(&[window(3), claim("a")]).unwrap();
            let op = match case {
                "end alone" => serde_json::json!({"to_seq": 10, "outcome": "skipped"}),
                "quote" => {
                    serde_json::json!({"text": "q", "evidence": [{"device": dev, "seq": 8}]})
                }
                _ => {
                    serde_json::json!({"from_seq": 1, "to_seq": 3, "goals": [8], "outcome": "curated"})
                }
            };
            let kind = match case {
                "quote" => raw::OpKind::Claim,
                _ => raw::OpKind::Window,
            };
            raw.append_ops(&[(kind, op)]).unwrap();
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
            let seqs: Vec<i64> = (restored.ops_after(&dev, 0, 100).unwrap().iter())
                .map(|o| o.op_seq)
                .collect();
            kept.push((case, seqs));
        }
        assert_eq!(
            kept,
            [
                ("end alone", vec![1, 2]),
                ("quote", vec![1, 2]),
                ("goal", vec![1, 2])
            ]
        );
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
