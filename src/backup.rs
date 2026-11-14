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
    static BETWEEN_POINTS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
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
    // config.toml is read through a symlink everywhere else (a dotfile manager's link); a store,
    // a kept file, a log, a segment or its seal that is a symlink is refused (the forget log's
    // writer opens it with O_NOFOLLOW).
    let follow = path.file_name().is_some_and(|name| name == "config.toml");
    let metadata = if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    };
    let metadata = match metadata {
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
        hash: crate::preview::file_version(path)?,
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
        paths.push(seal_of(&segment.path));
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
    let result = crate::preview::with_preview_v1(&path, |conn, _| {
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
    anyhow::ensure!(
        !records.is_empty(),
        "no usable backup segment of device {device} in {}",
        backup.display()
    );
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
        crate::preview::with_preview_v1(&home.join("knowledge.db"), |conn, _| {
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
    // A point once a record is backed up: before it, a store has nothing to go back to that a
    // restore does not keep anyway (its forgets and exclusions survive every restore).
    if !wrote.is_empty() && ends[0] > 0 {
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
    let _points = points_lock(dir, crate::db::OPEN_WRITE_WAIT);
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

/// The hold on a backup directory's points, around an append and from a rewrite's read to its
/// rename: the directory may be shared by homes whose workers do not hold each other's locks, and
/// a point one notes while another rewrites would otherwise be lost. Nothing else is taken while
/// it is held. None, with a line on stderr, when the directory cannot give it or another keeps
/// it past `wait`: the points are then written without it, as they were before there was one.
/// What that can lose is a point, a choice of where to go back to, and never a record or a
/// restore; a point of this device past what raw.db holds, which another home's rewrite brought
/// back, is dropped again by the next export (B6) (#999).
fn points_lock(dir: &Path, wait: std::time::Duration) -> Option<std::fs::File> {
    let path = dir.join("points.jsonl.lock");
    let held = || -> Result<Option<std::fs::File>> {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        let until = std::time::Instant::now() + wait;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(Some(lock)),
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
                Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
    };
    let why = match held() {
        Ok(Some(lock)) => return Some(lock),
        Ok(None) => "another keeps it".to_owned(),
        Err(e) => format!("{e:#}"),
    };
    eprintln!(
        "oboete: {} not held ({why}): the backup points are written without it",
        path.display()
    );
    None
}

/// The points a backup directory names, in the order they were noted; a line that does not read
/// as one is skipped (B1).
pub(crate) fn points(dir: &Path) -> Result<Vec<Point>> {
    // Read as bytes: a line that is not UTF-8 is one that does not read as a point.
    match std::fs::read(dir.join(POINTS)) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes)
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

/// What `oboete backup list` and the `restore --to` preview show for a count raw.db cannot give:
/// the points and the restore need no raw.db, so a damaged one does not hide them.
const UNKNOWN_AFTER: &str =
    "raw.db cannot be read now, so how many records and ops it holds past a point is shown as ?";

/// docs/backup-points.md B4: what going back to `point` keeps and takes, for its preview.
pub(crate) fn point_preview(home: &Path, point: &Point) -> Result<String> {
    let counts = || -> Result<(i64, i64, i64)> {
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
        Ok((records, ops, gates))
    };
    let (records, ops, gates, unknown) = match counts() {
        // The gates' ops after the point are kept, not taken back.
        Ok((records, ops, gates)) => (
            records.to_string(),
            (ops - gates).max(0).to_string(),
            gates.to_string(),
            String::new(),
        ),
        Err(_) => (
            "?".to_owned(),
            "?".to_owned(),
            "?".to_owned(),
            format!("{UNKNOWN_AFTER}.\n"),
        ),
    };
    Ok(format!(
        "Going back to the backup point of {}:\n\
         - keeps the records through seq {} and the ops through op {};\n\
         - takes back the {records} record(s) and {ops} op(s) after them: \
         the corrections, mutes, imports, summaries and work states made after the point go with them;\n\
         - keeps the {gates} forget and exclusion op(s) made after the point: forgotten text stays gone, \
         and excluded repositories stay excluded.\n\
         {unknown}\
         raw.db and knowledge.db as they are now, and the backup segments past the point, are set aside on disk, not deleted.\n\
         To go back to it, run: oboete restore --to {} --yes",
        point_time(point.at),
        point.records,
        point.ops,
        point_time(point.at),
    ))
}

/// docs/backup-points.md B2: this device's points, newest first: every one of the last 24 hours
/// and the newest of each day before that, or all of them.
pub(crate) fn list_points(home: &Path, all: bool) -> Result<Vec<String>> {
    let (dir, device, mine) = my_points(home)?;
    if mine.is_empty() {
        return Ok(vec![format!(
            "no backup point yet in {}: the next backup makes one (`oboete backup run`)",
            dir.display()
        )]);
    }
    let held = || -> Result<(Vec<i64>, Vec<i64>)> {
        let (conn, _) = raw_read(home)?;
        let seqs = |sql: &str| -> Result<Vec<i64>> {
            let mut st = conn.prepare(sql)?;
            let rows = st.query_map([&device], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        Ok((
            seqs("SELECT seq FROM records WHERE device = ?1 ORDER BY seq")?,
            seqs("SELECT op_seq FROM ops WHERE device = ?1 ORDER BY op_seq")?,
        ))
    };
    let held = held().ok();
    let after = |all: &[i64], at: i64| all.len() - all.partition_point(|&s| s <= at);
    let shown = |n: Option<usize>| n.map_or_else(|| "?".to_owned(), |n| n.to_string());
    let day = |at: i64| point_time(at)[..10].to_owned();
    let since = crate::db::now_ms() - 86_400_000;
    let mut lines = Vec::new();
    if held.is_none() {
        lines.push(format!("{UNKNOWN_AFTER}:"));
    }
    let mut days = std::collections::HashSet::new();
    for p in mine.iter().rev() {
        let newest_of_day = days.insert(day(p.at));
        if all || p.at >= since || newest_of_day {
            lines.push(format!(
                "{}  through seq {} ({} after) and op {} ({} after)",
                point_time(p.at),
                p.records,
                shown(held.as_ref().map(|(records, _)| after(records, p.records))),
                p.ops,
                shown(held.as_ref().map(|(_, ops)| after(ops, p.ops)))
            ));
        }
    }
    Ok(lines)
}

/// docs/backup-points.md B6: the directory's points rewritten without those of `device` that
/// `keep` refuses (and any line that does not read as a point), when there are such.
fn keep_points(dir: &Path, device: &str, keep: impl Fn(&Point) -> bool) -> Result<()> {
    let kept = || -> Result<Option<String>> {
        let text = match std::fs::read(dir.join(POINTS)) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| format!("read {}", dir.join(POINTS).display()));
            }
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
        Ok(changed.then_some(kept))
    };
    // The lock is taken only for a rewrite, so a directory that cannot hold it still reads, and
    // the points are read again under it.
    if kept()?.is_none() {
        return Ok(());
    }
    let _points = points_lock(dir, crate::db::OPEN_WRITE_WAIT);
    let Some(kept) = kept()? else {
        return Ok(());
    };
    #[cfg(test)]
    if let Some(between) = BETWEEN_POINTS.with(|hook| hook.borrow_mut().take()) {
        between();
    }
    write_synced(dir, POINTS, kept.as_bytes())?;
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
    // docs/backup-points.md B6: the seqs past raw's end are used again by new records, so the
    // points past it go: at every export and before the segments past it, so that a stop or a
    // failure between the two is retried by the next export (Codex on #880).
    keep_points(dir, raw.device(), |p| match kind {
        Kind::Records => p.records <= max,
        Kind::Ops => p.ops <= max,
    })?;
    if mine.iter().any(|s| s.last > max) {
        set_aside_report(mine.iter().filter(|s| s.last > max), &mut |_| {
            effect("backup_quarantined", committed)
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
        for path in [s.path.clone(), seal_of(&s.path)] {
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

/// The checksum beside segment `seg`, built on the path's own bytes: a directory whose path is not
/// UTF-8 would lose them through `display`.
fn seal_of(seg: &Path) -> PathBuf {
    let mut path = seg.as_os_str().to_owned();
    path.push(".sha256");
    path.into()
}

/// Why a segment cannot be trusted, if it cannot.
fn damage(seg: &Path) -> Option<String> {
    let sum_path = seal_of(seg);
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

/// Slice 3e-4: whether a backup location holds an ops segment, live or set aside, by its name
/// alone: one may hold an op raw.db does not (Codex on #487). Every directory `purge_ops` walks
/// is looked in, the remembered ones too: a location given up keeps its segments.
pub(crate) fn any_ops_segment(home: &Path) -> Result<bool> {
    for dir in segment_dirs(home)? {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
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
    let mut unread = None;
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
            // One its seal vouches for that does not read whole is left as it is, as a restore
            // trusts it by its seal; the segments after it are still walked, and the walk fails at
            // its end, so that a purge it may hold the text for is not taken as done.
            let text = match read_whole(&data) {
                Ok(text) => text,
                Err(e) => {
                    unread.get_or_insert(e.context(format!("read {}", path.display())));
                    continue;
                }
            };
            if let (Some(text), _) = judge(&text) {
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
    unread.map_or(Ok(()), Err)
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
/// changed: a line that cannot be read, or a segment no seal vouches for that cannot be, is passed
/// over (`purge_ops` removes it when it cannot judge it); a segment its seal vouches for that
/// cannot be read whole fails the walk once the segments after it are read, as `purge_ops` does.
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

/// docs/backup-points.md B5: `device`'s exclusion ops as the live raw.db holds them, in op order,
/// those no export has backed up yet too. None when there is no raw.db or it is damaged: that
/// store's restore keeps the backups' own, and so does one with no ops table (an empty file).
/// Any other failure to read them stops the restore, so an exclusion is never dropped unsaid
/// (cubic on #880).
fn live_exclusions(home: &Path, device: &str) -> Result<Vec<(i64, String)>> {
    let path = raw::path(home);
    if !path.try_exists()? {
        return Ok(Vec::new());
    }
    let read = || -> Result<Vec<(i64, String)>> {
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        // The table is asked of the schema, so that a file that does not read still stops here.
        let has_ops: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ops')",
            [],
            |r| r.get(0),
        )?;
        if !has_ops {
            return Ok(Vec::new());
        }
        let mut st = conn.prepare(
            "SELECT op_seq, body FROM ops WHERE device = ?1 AND type = 'exclusion' ORDER BY op_seq",
        )?;
        let rows = st.query_map([device], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    match read() {
        Err(e) if damaged(&e) => Ok(Vec::new()),
        read => read.context("read raw.db's exclusions before the restore"),
    }
}

/// The forget requests (D1 rule 14) and the retention batch begun (docs/retention.md) a raw.db
/// that still reads holds, read under the swap lock without opening it as a store. A lost database
/// or unreadable table has none (F1), and a table whose page is damaged holds the rows read before
/// it, as `live_exclusions` takes a damaged file: the restore is what repairs it, and the logs and
/// `state/expiry-owed` still bring theirs. A row that reads but does not parse or validate must
/// stop the restore rather than discard other live requests.
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
    let mut out = Vec::new();
    for row in st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
        match row.context("read live expiry_owed row") {
            Ok(row) => out.push(row),
            Err(e) if damaged(&e) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

fn requests_in(conn: &rusqlite::Connection) -> Result<Vec<crate::forget::Request>> {
    let Ok(mut st) = conn.prepare("SELECT request FROM forget_jobs") else {
        return Ok(Vec::new());
    };
    let rows = st.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let text = match row.context("read live forget request row") {
            Ok(text) => text,
            Err(e) if damaged(&e) => break,
            Err(e) => return Err(e),
        };
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
    // B6: the highest seq replayed, as the lines gave it: past it, seqs are used again. A
    // segment's name can overstate it, when the segment spans a range raw.db had lost (Codex on
    // #880). A gap below it is never used again, so a point past the gap still names what it named.
    let replayed = rebuild.max_seq()?;
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
    rebuild.carry_exclusions(live_exclusions(home, &device)?);
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
    // So is each sound ops segment that holds an op past the ops the rebuild kept from the backups:
    // the ops appended again after a cut take those op seqs with other bodies, so the segment no
    // longer says what raw.db holds, though raw's highest op seq may not have fallen below its
    // end (Codex on #880). The next export writes that range again from raw.db.
    let mut aside: Vec<&Segment> = bad.iter().chain(&ops_bad).chain(&after).collect();
    for s in ops_ok.iter().filter(|s| s.last > ops_through) {
        if !aside.iter().any(|a| a.path == s.path) {
            aside.push(s);
        }
    }
    set_aside_report(aside, &mut |first| {
        receipt.effects.segments_quarantined += u64::from(first);
        receipt.backup_files_kept += 1;
        effect("segments_quarantined", committed)
    })?;
    // B6: before the swap, so a restore that stops leaves no point past what raw.db will hold.
    keep_points(&dir, &device, |p| {
        p.records <= replayed && p.ops <= ops_through
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
pub(crate) mod tests;
