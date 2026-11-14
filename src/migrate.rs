//! Milestone 4 Task 9: the cut-over from v1's `oboete.db` to Design B's stores (spec 7.4, D6).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::capture::{self, Captured, Settings};
use crate::preview::{
    FailureCode, absolute_identity, config_bytes, failure_code, open_v1, preview_key,
    with_preview_v1,
};
use crate::raw::{
    Checkpoint, IMPORT_BATCH, ImportDoc, ImportIdentity, MAX_BATCH_BYTES, Raw, V1Row,
};

/// The source of what v1's store holds, as records and documents in Design B (D6).
const SOURCE: &str = "oboete-v1";

#[cfg(test)]
thread_local! {
    /// A hook gets a known commit gap without depending on the thread scheduler.
    static BETWEEN_BATCHES: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Refuse a source that a writable raw open would reuse, including a stopped restore's file.
pub fn check_source(home: &Path, from: &Path) -> Result<()> {
    let destination = crate::raw::path(home);
    let source = crate::db::store_file(from);
    anyhow::ensure!(
        source.is_empty() || source != crate::db::store_file(&destination),
        "v1 source aliases destination raw.db: use a separate source and destination"
    );
    Ok(())
}

/// What a pass imported.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Stats {
    /// v1 events read past the checkpoint, and the records they became.
    pub events: u64,
    pub records: u64,
    /// `session_repos` rows recorded as `touch` records.
    pub repos: u64,
    /// Observations, summaries and prompts appended as import ops, and those imported before.
    pub documents: u64,
    pub seen: u64,
    /// Imported v1 sessions whose events oboete.db no longer holds: the old viewer deleted them
    /// after a pass, and the owner may forget them in Design B (spec 7.4, A105).
    pub deleted: Vec<String>,
    /// Stored identifiers whose earlier redaction or clipping prevents a reliable match.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub uncertain: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct Candidates {
    pub events: u64,
    pub records: u64,
    pub repos: u64,
    pub documents: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingsEffect {
    Preserve,
    Copy,
    Defaults,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MissingSetting {
    Curate,
    Inject,
    Capture,
    Redaction,
    Chain,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct SettingsPreview {
    pub effect: SettingsEffect,
    pub version: String,
    pub missing: Vec<MissingSetting>,
}

pub(crate) type SettingsOutcome = SettingsPreview;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct Preview {
    pub key: String,
    pub candidates: Candidates,
    pub settings: SettingsPreview,
}

/// Finalization's read-only preview. Candidate imports are not committed counts.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct FinishPreview {
    pub(crate) key: String,
    pub(crate) candidates: Candidates,
    pub(crate) deletion: FinishDeletion,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct FinishDeletion {
    pub(crate) targets: Vec<String>,
    pub(crate) nodes: u64,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct FinishEffects {
    pub(crate) stores_changed: bool,
    pub(crate) forget_requests_applied: u64,
    pub(crate) forget_log_warnings: u64,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FinishTargetState {
    Removed,
    Failed,
    Changed,
    NotAttempted,
    UnknownExtent,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct FinishTargetOutcome {
    pub(crate) label: String,
    pub(crate) state: FinishTargetState,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct FinishDeletionReport {
    pub(crate) selected: u64,
    pub(crate) attempted: u64,
    pub(crate) removed: u64,
    pub(crate) failed: u64,
    pub(crate) uncertain: u64,
    pub(crate) authorized_bytes: u64,
    pub(crate) removed_bytes: u64,
    pub(crate) targets: Vec<FinishTargetOutcome>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FinishOutcome {
    pub(crate) stats: Stats,
    pub(crate) effects: FinishEffects,
    pub(crate) deletion: FinishDeletionReport,
}

impl FinishOutcome {
    pub(crate) fn committed(&self) -> bool {
        self.effects.stores_changed || self.deletion.removed != 0
    }

    pub(crate) fn uncertain(&self) -> bool {
        self.deletion.uncertain != 0
    }
}

#[derive(Debug)]
pub(crate) struct FinishFailure {
    pub(crate) outcome: Box<FinishOutcome>,
    pub(crate) code: FailureCode,
    pub(crate) cause: anyhow::Error,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", content = "receipt", rename_all = "snake_case")]
pub(crate) enum FinishCommit {
    Import(Committed),
    Effects(FinishEffects),
    Deletion {
        selected: u64,
        attempted: u64,
        removed: u64,
        failed: u64,
        uncertain: u64,
        authorized_bytes: u64,
        removed_bytes: u64,
    },
}

impl FinishCommit {
    pub(crate) fn committed(&self) -> bool {
        match self {
            Self::Import(_) => true,
            Self::Effects(effects) => effects.stores_changed,
            Self::Deletion { removed, .. } => *removed != 0,
        }
    }
}

impl FinishDeletionReport {
    fn planned(manifest: &FinishManifest, rules: &crate::redact::Rules) -> Self {
        Self {
            selected: manifest.targets.len() as u64,
            authorized_bytes: manifest.bytes,
            targets: manifest
                .targets
                .iter()
                .map(|target| FinishTargetOutcome {
                    label: crate::redact::outbound_with(&target.name, rules),
                    state: FinishTargetState::NotAttempted,
                    bytes: target.bytes,
                })
                .collect(),
            ..Self::default()
        }
    }

    fn event(&self) -> FinishCommit {
        FinishCommit::Deletion {
            selected: self.selected,
            attempted: self.attempted,
            removed: self.removed,
            failed: self.failed,
            uncertain: self.uncertain,
            authorized_bytes: self.authorized_bytes,
            removed_bytes: self.removed_bytes,
        }
    }
}

// Bound returned metadata, recursive stack and buffered child digests, not file contents.
const FINISH_TARGETS: usize = 128;
const FINISH_LABEL_BYTES: usize = 16 * 1024;
const FINISH_PATH_BYTES: usize = 4096;
const FINISH_DEPTH: usize = 32;
const FINISH_DIGEST_BYTES: usize = 8 * 1024 * 1024;

#[derive(PartialEq, serde::Serialize)]
struct FinishTarget {
    name: String,
    directory: bool,
    nodes: u64,
    bytes: u64,
    digest: [u8; 32],
}

/// Used by preview; a confirmed finish can recheck the same per-target bindings.
#[derive(PartialEq, serde::Serialize)]
struct FinishManifest {
    targets: Vec<FinishTarget>,
    nodes: u64,
    bytes: u64,
}

#[derive(Default)]
struct FinishWalk {
    nodes: u64,
    bytes: u64,
    buffered: usize,
    directory: bool,
}

// Type, the entry's own identity, logical length and modification time.
type FinishStamp = (u8, (u64, u64), u64, std::time::SystemTime);

#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    pub stats: Stats,
    pub settings: Option<SettingsOutcome>,
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub outcome: Box<Outcome>,
    pub code: FailureCode,
    pub cause: anyhow::Error,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Settings,
    Events,
    Repos,
    Documents,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Committed {
    pub stage: Stage,
    pub records: u64,
    pub repos: u64,
    pub documents: u64,
}

pub(crate) fn run(
    home: &Path,
    from: &Path,
    expected: Option<&str>,
    committed: &mut impl FnMut(&Committed),
) -> std::result::Result<Outcome, Failure> {
    check_source(home, from).map_err(|cause| Failure {
        outcome: Box::default(),
        code: FailureCode::InvalidSource,
        cause,
    })?;
    let _lock = crate::import::lock(home).map_err(|cause| Failure {
        outcome: Box::default(),
        code: FailureCode::Busy,
        cause,
    })?;
    run_holding(home, from, &mut None, expected, committed)
}

/// The caller owns import.lock. Confirmed runs read and apply the same private transaction.
pub(crate) fn run_holding(
    home: &Path,
    from: &Path,
    raw: &mut Option<Raw>,
    expected: Option<&str>,
    committed: &mut impl FnMut(&Committed),
) -> std::result::Result<Outcome, Failure> {
    let mut outcome = Outcome::default();
    let mut code = FailureCode::InvalidSource;
    let result = (|| {
        check_source(home, from)?;
        if let Some(expected) = expected {
            with_preview_v1(from, |v1, source| {
                let _config = crate::settings::config_lock(home)?;
                code = FailureCode::Stale;
                let destination = if raw.is_none() {
                    crate::raw::read_only(home)?
                } else {
                    None
                };
                let shown = preview_connection(home, from, v1, source)?;
                anyhow::ensure!(shown.key == expected, "migration preview is stale");
                let (settings, bytes) = preview_settings(home, from)?;
                anyhow::ensure!(
                    settings == shown.settings,
                    "migration config changed during confirmation"
                );
                check_source(home, from)?;
                code = FailureCode::InvalidConfig;
                apply_settings(
                    home,
                    from,
                    Some((settings, bytes)),
                    &mut outcome.settings,
                    committed,
                )?;
                let capture = Settings {
                    source: SOURCE,
                    ..Settings::load(home)?
                };
                code = FailureCode::Failed;
                if let Some(held) = destination {
                    *raw = Some(held.into_writer(home)?);
                }
                execute_pass(home, raw, v1, &capture, &mut outcome.stats, committed)
            })
        } else {
            let _config = crate::settings::config_lock(home)?;
            code = FailureCode::InvalidConfig;
            apply_settings(home, from, None, &mut outcome.settings, committed)?;
            let capture = Settings {
                source: SOURCE,
                ..Settings::load(home)?
            };
            code = FailureCode::InvalidSource;
            let v1 = open_v1(from)?;
            v1.execute_batch("BEGIN")?;
            code = FailureCode::Failed;
            execute_pass(home, raw, &v1, &capture, &mut outcome.stats, committed)
        }
    })();
    match result {
        Ok(()) => Ok(outcome),
        Err(cause) => Err(Failure {
            outcome: Box::new(outcome),
            code: failure_code(&cause, code),
            cause,
        }),
    }
}

fn execute_pass(
    home: &Path,
    raw: &mut Option<Raw>,
    v1: &Connection,
    settings: &Settings,
    stats: &mut Stats,
    committed: &mut impl FnMut(&Committed),
) -> Result<()> {
    let device = crate::db::device_id(v1).context("not a v1 store")?;
    if raw.is_none() {
        *raw = Some(crate::raw::open(home)?);
    }
    let raw = raw.as_mut().context("migration raw store is absent")?;
    crate::forget::reconcile_or_say(home, raw)?;
    read_connection(v1, raw, &device, settings, stats, committed)
}

fn notify(stats: &Stats, stage: Stage, committed: &mut impl FnMut(&Committed)) {
    committed(&Committed {
        stage,
        records: stats.records,
        repos: stats.repos,
        documents: stats.documents,
    });
}

fn missing_settings(text: &str) -> Result<Vec<MissingSetting>> {
    let table: toml::Table = text
        .parse()
        .map_err(|e| crate::config::toml_error(text, &e))?;
    let mut missing = Vec::new();
    if table.get("summary").and_then(|s| s.get("curate")).is_none() {
        missing.push(MissingSetting::Curate);
    }
    for (name, kind) in [
        ("inject", MissingSetting::Inject),
        ("capture", MissingSetting::Capture),
        ("redaction", MissingSetting::Redaction),
        ("chain", MissingSetting::Chain),
    ] {
        if !table.contains_key(name) {
            missing.push(kind);
        }
    }
    Ok(missing)
}

pub(crate) fn capture_settings(bytes: Option<&[u8]>, source: &'static str) -> Result<Settings> {
    let text = bytes.map(std::str::from_utf8).transpose()?;
    let config = crate::config::parse_capture(text)?;
    Ok(Settings {
        rules: crate::redact::Rules::new(&config.redaction)?,
        store_prompts: config.capture.store_prompts,
        tool_output: config.capture.tool_output,
        source,
    })
}

fn preview_settings(home: &Path, from: &Path) -> Result<(SettingsPreview, Option<Vec<u8>>)> {
    let ours = config_bytes(home)?;
    let (effect, bytes) = if ours.is_some() {
        (SettingsEffect::Preserve, ours)
    } else {
        let theirs = config_bytes(from.parent().context("v1 store has no parent")?)?;
        (
            if theirs.is_some() {
                SettingsEffect::Copy
            } else {
                SettingsEffect::Defaults
            },
            theirs,
        )
    };
    capture_settings(bytes.as_deref(), SOURCE)?;
    let text = bytes
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()?
        .unwrap_or("");
    Ok((
        SettingsPreview {
            effect,
            version: bytes
                .as_deref()
                .map_or_else(|| "none".to_owned(), crate::forget::hash),
            missing: missing_settings(text)?,
        },
        bytes,
    ))
}

pub(crate) fn preview(home: &Path, from: &Path) -> Result<Preview> {
    check_source(home, from)?;
    with_preview_v1(from, |v1, source| {
        preview_connection(home, from, v1, source)
    })
}

fn preview_candidates(v1: &Connection, device: &str, capture: &Settings) -> Result<Candidates> {
    let mut candidates = Candidates::default();
    let mut st = v1.prepare(EVENTS_SQL)?;
    let mut rows = st.query([0])?;
    while let Some(row) = rows.next()? {
        candidates.events += 1;
        let records = captured_row(row, capture)?;
        candidates.records += records.len() as u64;
        candidates.bytes += records
            .iter()
            .map(|r| r.event.body.len() as u64)
            .sum::<u64>();
    }
    let mut st = v1.prepare(REPOS_SQL)?;
    let mut rows = st.query([])?;
    while let Some(row) = rows.next()? {
        let records = captured_repo(row, capture)?;
        candidates.repos += records.len() as u64;
        candidates.bytes += records
            .iter()
            .map(|r| r.event.body.len() as u64)
            .sum::<u64>();
    }
    read_documents(
        v1,
        device,
        capture,
        &mut Default::default(),
        &mut Stats::default(),
        |doc| {
            candidates.documents += 1;
            candidates.bytes += (doc.title.len() + doc.body.len()) as u64;
        },
    )?;
    fingerprint(v1)?;
    Ok(candidates)
}

fn preview_connection(home: &Path, from: &Path, v1: &Connection, source: &str) -> Result<Preview> {
    let device = crate::db::device_id(v1).context("not a v1 store")?;
    let (settings, bytes) = preview_settings(home, from)?;
    let capture = capture_settings(bytes.as_deref(), SOURCE)?;
    let candidates = preview_candidates(v1, &device, &capture)?;
    let key = preview_key(
        home,
        &(
            "v1",
            from,
            absolute_identity(from)?,
            source,
            &settings,
            &candidates,
        ),
    )?;
    Ok(Preview {
        key,
        candidates,
        settings,
    })
}

/// Inspect only: no Raw open, recovery, settings copy, coordination file or final pass.
pub(crate) fn preview_finish(home: &Path) -> Result<FinishPreview> {
    let from = home.join("oboete.db");
    check_source(home, &from)?;
    let destination = preview_key(home, &"finish destination")?;
    let config = finish_config_bytes(home)?;
    let capture = capture_settings(config.as_deref(), SOURCE)?;
    let backup = finish_backup(home, config.as_deref())?;
    let manifest = finish_manifest(home, &backup)?;
    check_finish_source(&from)?;
    let (candidates, source) = with_preview_v1(&from, |v1, source| {
        let device = crate::db::device_id(v1).context("not a v1 store")?;
        Ok((
            preview_candidates(v1, &device, &capture)?,
            source.to_owned(),
        ))
    })?;
    anyhow::ensure!(
        manifest == finish_manifest(home, &backup)? && config == finish_config_bytes(home)?,
        "finish preview inputs changed"
    );
    check_source(home, &from)?;
    let key = finish_key(home, &source, config.as_deref(), &manifest)?;
    anyhow::ensure!(
        destination == preview_key(home, &"finish destination")?,
        "finish destination changed"
    );
    let targets: Vec<String> = manifest
        .targets
        .iter()
        .map(|target| target.name.clone())
        .collect();
    let label_bytes = targets.iter().try_fold(0usize, |sum, label| {
        sum.checked_add(label.len())
            .context("finish labels are too large")
    })?;
    anyhow::ensure!(
        label_bytes <= FINISH_LABEL_BYTES,
        "finish labels are too large"
    );
    Ok(FinishPreview {
        key,
        candidates,
        deletion: FinishDeletion {
            targets,
            nodes: manifest.nodes,
            bytes: manifest.bytes,
        },
    })
}

fn finish_backup(home: &Path, config: Option<&[u8]>) -> Result<PathBuf> {
    let text = config.map(std::str::from_utf8).transpose()?;
    Ok(text
        .map(crate::backup::location)
        .transpose()?
        .flatten()
        .map_or_else(|| home.join("backups"), |path| home.join(path)))
}

fn finish_key(
    home: &Path,
    source: &str,
    config: Option<&[u8]>,
    manifest: &FinishManifest,
) -> Result<String> {
    preview_key(
        home,
        &("finish", source, config.map(crate::forget::hash), manifest),
    )
}

fn check_finish_source(from: &Path) -> Result<()> {
    // A source link may name a regular v1 DB; neither it nor its WAL may be a special file.
    let source_path = from.canonicalize()?;
    anyhow::ensure!(
        finish_stamp(&source_path)?.0 == 0,
        "finish source is not a file"
    );
    let mut wal = source_path.as_os_str().to_owned();
    wal.push("-wal");
    match std::fs::symlink_metadata(Path::new(&wal)) {
        Ok(_) => anyhow::ensure!(
            finish_stamp(Path::new(&wal))?.0 == 0,
            "finish WAL is not a file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn finish_config_bytes(home: &Path) -> Result<Option<Vec<u8>>> {
    // Keep the native config reader/parser; refuse a static pipe/device before reading it.
    match std::fs::metadata(home.join("config.toml")) {
        Ok(metadata) => anyhow::ensure!(metadata.is_file(), "finish config is not a file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    config_bytes(home)
}

fn finish_manifest(home: &Path, backup: &Path) -> Result<FinishManifest> {
    let home = std::path::absolute(home)?;
    let backup_path = std::path::absolute(backup)?;
    let backup = absolute_identity(backup)?;
    let mut names = Vec::new();
    let mut labels = 0usize;
    for entry in std::fs::read_dir(&home)? {
        let entry = entry?;
        let filename = entry.file_name();
        let kind = entry.file_type()?;
        if !old_name(&filename.to_string_lossy(), kind.is_dir()) {
            continue;
        }
        let name = filename
            .to_str()
            .context("finish target is not UTF-8")?
            .to_owned();
        labels = labels
            .checked_add(name.len())
            .context("finish labels are too large")?;
        anyhow::ensure!(
            names.len() < FINISH_TARGETS && labels <= FINISH_LABEL_BYTES,
            "finish target metadata is too large"
        );
        let path = entry.path();
        anyhow::ensure!(
            !backup_path.starts_with(&path),
            "finish target overlaps native backups"
        );
        if kind.is_dir() {
            anyhow::ensure!(
                !backup.starts_with(path.canonicalize()?),
                "finish target overlaps native backups"
            );
        } else if !kind.is_symlink() {
            anyhow::ensure!(
                backup != absolute_identity(&path)?,
                "finish target overlaps native backups"
            );
        }
        names.push(name);
    }
    names.sort();
    let mut walk = FinishWalk::default();
    let mut targets = Vec::new();
    for name in names {
        let (nodes, bytes) = (walk.nodes, walk.bytes);
        let digest = finish_node(&home, Path::new(&name), 0, &mut walk)?;
        targets.push(FinishTarget {
            name,
            directory: walk.directory,
            nodes: walk.nodes - nodes,
            bytes: walk.bytes - bytes,
            digest,
        });
    }
    Ok(FinishManifest {
        targets,
        nodes: walk.nodes,
        bytes: walk.bytes,
    })
}

fn finish_node(
    home: &Path,
    relative: &Path,
    depth: usize,
    walk: &mut FinishWalk,
) -> Result<[u8; 32]> {
    anyhow::ensure!(
        depth <= FINISH_DEPTH && relative.as_os_str().as_encoded_bytes().len() <= FINISH_PATH_BYTES,
        "finish path metadata is too large"
    );
    let path = home.join(relative);
    let stamp = finish_stamp(&path)?;
    if depth == 0 {
        walk.directory = stamp.0 == 1;
    }
    walk.nodes = walk
        .nodes
        .checked_add(1)
        .context("finish node count overflow")?;
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(&(
        relative.as_os_str().as_encoded_bytes(),
        &stamp,
    ))?);
    match stamp.0 {
        0 => {
            walk.bytes = walk
                .bytes
                .checked_add(stamp.2)
                .context("finish byte count overflow")?;
            hash.update(finish_file_hash(&path, &stamp)?);
        }
        1 => {
            // Sort fixed-size child hashes, not whole paths or file bodies. Full relative paths
            // are bound inside each hash, so enumeration order cannot change membership.
            let mut children = Vec::new();
            for entry in std::fs::read_dir(&path)? {
                let child = relative.join(entry?.file_name());
                let digest = finish_node(home, &child, depth + 1, walk)?;
                walk.buffered = walk
                    .buffered
                    .checked_add(32)
                    .context("finish metadata overflow")?;
                anyhow::ensure!(
                    walk.buffered <= FINISH_DIGEST_BYTES,
                    "finish directory metadata is too large"
                );
                children.push(digest);
            }
            children.sort_unstable();
            for child in &children {
                hash.update(child);
            }
            walk.buffered -= children.len() * 32;
        }
        2 => {
            let target = std::fs::read_link(&path)?;
            let bytes = target.as_os_str().as_encoded_bytes();
            anyhow::ensure!(
                bytes.len() <= FINISH_PATH_BYTES,
                "finish link metadata is too large"
            );
            walk.bytes = walk
                .bytes
                .checked_add(stamp.2)
                .context("finish byte count overflow")?;
            hash.update(bytes);
        }
        _ => unreachable!("finish_stamp only accepts fixed entry types"),
    }
    anyhow::ensure!(
        stamp == finish_stamp(&path)?,
        "finish target changed during inspection"
    );
    Ok(hash.finalize().into())
}

fn finish_stamp(path: &Path) -> Result<FinishStamp> {
    let metadata = std::fs::symlink_metadata(path)?;
    let kind = if metadata.file_type().is_symlink() {
        2
    } else if metadata.is_dir() {
        1
    } else if metadata.is_file() {
        0
    } else {
        anyhow::bail!("finish target is not a file, directory or link")
    };
    Ok((
        kind,
        finish_identity(path, &metadata, None)?,
        metadata.len(),
        metadata.modified()?,
    ))
}

fn finish_open(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT.
        options.custom_flags(0x0200_0000 | 0x0020_0000);
    }
    Ok(options.open(path)?)
}

fn finish_identity(
    path: &Path,
    metadata: &std::fs::Metadata,
    file: Option<&std::fs::File>,
) -> Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = (path, file);
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        let _ = metadata;
        let opened;
        let file = match file {
            Some(file) => file,
            None => {
                opened = finish_open(path)?;
                &opened
            }
        };
        let info = winapi_util::file::information(file)?;
        Ok((info.volume_serial_number(), info.file_index()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, metadata, file);
        anyhow::bail!("finish entry identity is unavailable")
    }
}

fn finish_file_hash(path: &Path, stamp: &FinishStamp) -> Result<[u8; 32]> {
    use std::io::Read;
    let mut file = finish_open(path)?;
    let opened = file.metadata()?;
    anyhow::ensure!(
        opened.is_file()
            && finish_identity(path, &opened, Some(&file))? == stamp.1
            && opened.len() == stamp.2
            && opened.modified()? == stamp.3,
        "finish file changed before reading"
    );
    let limit = stamp
        .2
        .checked_add(1)
        .context("finish file length overflow")?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    {
        let mut reader = (&mut file).take(limit);
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            bytes += n as u64; // take(limit) bounds the complete sum.
        }
    }
    anyhow::ensure!(
        bytes == stamp.2 && *stamp == finish_stamp(path)?,
        "finish file changed during reading"
    );
    Ok(hash.finalize().into())
}

/// One pass of the v1 import (spec 7.4, D6): the events past the checkpoint as records, in
/// batches each appended with its checkpoint; the deleted sessions; `session_repos` as `touch`
/// records; then the documents as import ops. Everything is read in one read transaction of the
/// store at `from`, which is never written; a rerun imports nothing twice.
#[cfg(test)]
fn pass(home: &Path, raw: &mut Raw, from: &Path) -> Result<Stats> {
    // Milestone 5 D1: the forget request logs first, so an import never brings back what a log
    // holds and raw.db lost.
    crate::forget::reconcile_or_say(home, raw)?;
    Ok(read_pass(home, raw, from)?.0)
}

/// v1's highest ids (events, observations, summaries, prompts) and its newest event: what
/// `--finish` checks v1 has not moved since its pass.
#[derive(Debug, PartialEq)]
struct Fingerprint {
    highest: [i64; 4],
    newest: Option<V1Row>,
}

fn fingerprint(v1: &Connection) -> Result<Fingerprint> {
    let mut highest = [0; 4];
    for (n, table) in ["events", "observations", "summaries", "prompts"]
        .into_iter()
        .enumerate()
    {
        highest[n] = v1.query_row(
            &format!("SELECT COALESCE(MAX(id), 0) FROM {table}"),
            [],
            |r| r.get(0),
        )?;
    }
    Ok(Fingerprint {
        highest,
        newest: row_at(v1, highest[0])?,
    })
}

/// v1's event `id`, fingerprinted, if there is one.
fn row_at(v1: &Connection, id: i64) -> Result<Option<V1Row>> {
    let sql = "SELECT id, ts, session_id FROM events WHERE id = ?1";
    Ok(v1.query_row(sql, [id], v1_row).optional()?)
}

/// A v1 event's fingerprint from (id, ts, session_id): the session id by its SHA-256 only. The
/// checkpoint is stored text, and only a record's labels pass the gate.
fn v1_row(r: &rusqlite::Row) -> rusqlite::Result<V1Row> {
    Ok(V1Row {
        id: r.get(0)?,
        ts: r.get(1)?,
        session_id: crate::curate::sha256_hex(&r.get::<_, String>(2)?),
    })
}

/// `pass`, with v1's fingerprint read in the same transaction.
#[cfg(test)]
fn read_pass(home: &Path, raw: &mut Raw, from: &Path) -> Result<(Stats, Fingerprint)> {
    let v1 = open_v1(from)?;
    // One read transaction: a consistent snapshot while v1's hooks keep writing.
    v1.execute_batch("BEGIN")?;
    let device = crate::db::device_id(&v1)
        .with_context(|| format!("{} has no device_id: not a v1 store", from.display()))?;
    let settings = Settings {
        source: SOURCE,
        ..Settings::load(home)?
    };
    let mut stats = Stats::default();
    read_connection(&v1, raw, &device, &settings, &mut stats, &mut |_| {})?;
    Ok((stats, fingerprint(&v1)?))
}

fn read_connection(
    v1: &Connection,
    raw: &mut Raw,
    device: &str,
    settings: &Settings,
    stats: &mut Stats,
    committed: &mut impl FnMut(&Committed),
) -> Result<()> {
    events(v1, raw, device, settings, stats, committed)?;
    deleted(v1, raw, settings, stats)?;
    repos(v1, raw, settings, stats, committed)?;
    documents(v1, raw, device, settings, stats, committed)?;
    Ok(())
}

/// Spec 7.4, A106: v1's `config.toml` (beside its store) into a home that has none, unchanged; a
/// home's own file is never edited. Then what the home's file leaves unanswered, one line each.
#[cfg(test)]
fn settings(home: &Path, from: &Path) -> Result<Vec<String>> {
    let _config = crate::settings::config_lock(home)?;
    let mut outcome = None;
    apply_settings(home, from, None, &mut outcome, &mut |_| {})?;
    Ok(settings_lines(
        &outcome.context("migration settings are absent")?.missing,
    ))
}

/// Native CLI notices, derived from the same fixed settings codes the viewer receives.
pub(crate) fn settings_lines(missing: &[MissingSetting]) -> Vec<String> {
    let mut lines = Vec::new();
    if missing.contains(&MissingSetting::Curate) {
        lines.push(
            "[summary] curate is not set: nothing is curated until it is (spec 7.4)".to_owned(),
        );
    }
    let unset: Vec<_> = missing
        .iter()
        .filter_map(|kind| match kind {
            MissingSetting::Curate => None,
            MissingSetting::Inject => Some("inject"),
            MissingSetting::Capture => Some("capture"),
            MissingSetting::Redaction => Some("redaction"),
            MissingSetting::Chain => Some("chain"),
        })
        .collect();
    if !unset.is_empty() {
        lines.push(format!(
            "not set, so their defaults apply: [{}]",
            unset.join("], [")
        ));
    }
    lines
}

fn apply_settings(
    home: &Path,
    from: &Path,
    confirmed: Option<(SettingsPreview, Option<Vec<u8>>)>,
    outcome: &mut Option<SettingsOutcome>,
    committed: &mut impl FnMut(&Committed),
) -> Result<()> {
    let is_confirmed = confirmed.is_some();
    let (effect, bytes) = match confirmed {
        Some((shown, bytes)) => (shown.effect, bytes),
        None => {
            let ours = config_bytes(home)?;
            if ours.is_some() {
                (SettingsEffect::Preserve, ours)
            } else {
                let theirs = config_bytes(from.parent().context("v1 store has no parent")?)?;
                (
                    if theirs.is_some() {
                        SettingsEffect::Copy
                    } else {
                        SettingsEffect::Defaults
                    },
                    theirs,
                )
            }
        }
    };
    let mut result = SettingsOutcome {
        effect,
        version: bytes
            .as_deref()
            .map_or_else(|| "none".to_owned(), crate::forget::hash),
        missing: Vec::new(),
    };
    if effect == SettingsEffect::Copy {
        let copied = copy_config(home, bytes.as_deref().context("source config is absent")?)?;
        if !copied {
            result.effect = SettingsEffect::Preserve;
        }
        // The hard-link committed the settings even if their subsequent parse fails.
        *outcome = Some(result.clone());
        if copied {
            notify(&Stats::default(), Stage::Settings, committed);
        }
        std::fs::remove_file(home.join("config.toml.part"))?;
        anyhow::ensure!(!is_confirmed || copied, FailureCode::Stale);
    }
    let actual = config_bytes(home)?;
    result.version = actual
        .as_deref()
        .map_or_else(|| "none".to_owned(), crate::forget::hash);
    *outcome = Some(result.clone());
    let text = actual
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()?
        .unwrap_or("");
    result.missing = missing_settings(text)
        .with_context(|| format!("parse {}", home.join("config.toml").display()))?;
    *outcome = Some(result);
    Ok(())
}

/// config.lock is held; keep the original atomic no-replace copy semantics.
fn copy_config(home: &Path, bytes: &[u8]) -> Result<bool> {
    use std::io::Write;
    crate::db::private(home, 0o700);
    let part = home.join("config.toml.part");
    match std::fs::remove_file(&part) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut staged = options.open(&part)?;
        staged.write_all(bytes)?;
        staged.sync_all()?;
        drop(staged);
        match std::fs::hard_link(&part, home.join("config.toml")) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e.into()),
        }
    })();
    if result.is_err() {
        std::fs::remove_file(&part).ok();
    }
    // The caller records the committed effect before removing the staging link.
    result
}

pub(crate) fn legacy_counts_in(conn: &Connection) -> [rusqlite::Result<i64>; 4] {
    ["sessions", "events", "observations", "summaries"].map(|table| {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
    })
}

pub(crate) struct LegacyCall {
    pub(crate) provider: String,
    pub(crate) outcome: String,
    pub(crate) ms: i64,
    pub(crate) detail: String,
}

pub(crate) fn legacy_call_rows_in(conn: &Connection) -> Result<Vec<LegacyCall>> {
    let mut statement = conn.prepare(
        "SELECT provider, outcome, ms, COALESCE(detail,'') FROM provider_calls ORDER BY id DESC LIMIT 5",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(LegacyCall {
                provider: row.get(0)?,
                outcome: row.get(1)?,
                ms: row.get(2)?,
                detail: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn legacy_key_in(conn: &Connection) -> Result<Option<String>> {
    let device: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'device_id'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(device.map(|device| format!("{SOURCE}:{device}")))
}

pub(crate) fn legacy_remaining_in(conn: &Connection, through: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM events WHERE id > ?1",
        [through],
        |row| row.get(0),
    )?)
}

/// Doctor's lines on the cut-over (spec 7.4): v1's events not migrated yet, v1's old files until
/// `--finish`, and `eval/`, whose evaluation copies forget does not reach.
pub fn doctor(home: &Path) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let store = home.join("oboete.db");
    if store.exists() {
        let v1 = open_v1(&store)?;
        if let Some(key) = legacy_key_in(&v1)? {
            let through = if crate::raw::exists(home) {
                let raw = crate::raw::open(home)?;
                raw.migration_checkpoints(&key)?
                    .remove(&key)
                    .map_or(0, |c| c.through)
            } else {
                0
            };
            let waiting = legacy_remaining_in(&v1, through)?;
            lines.push(format!(
                "v1 events not migrated yet: {waiting} (`oboete migrate` imports them)"
            ));
        }
    }
    let old: Vec<String> = old_files(home)?
        .iter()
        .map(|(path, bytes)| format!("{} ({bytes} bytes)", path.display()))
        .collect();
    if !old.is_empty() {
        lines.push(format!(
            "v1's old files, kept until `oboete migrate --finish`: {}",
            old.join(", ")
        ));
    }
    let eval = home.join("eval");
    if eval.is_dir() {
        lines.push(format!(
            "evaluation copies, which forget does not reach: {} ({} bytes)",
            eval.display(),
            size(&eval)?
        ));
    }
    Ok(lines)
}

/// `oboete migrate --finish` (spec 7.4, A58): one more pass; then v1's old files in the home with
/// their sizes, and the imported v1 sessions oboete.db no longer holds; then one line from
/// `answer`. Only on `yes`, and only while v1 has written nothing since the pass, the files are
/// deleted, each failure reported. It reads the home's own store: the files are the home's, so
/// `--from` is refused with it.
pub fn finish(
    home: &Path,
    mut answer: impl std::io::BufRead,
    out: &mut impl std::io::Write,
) -> Result<()> {
    let mode = FinishMode::Cli {
        answer: &mut answer,
        out,
    };
    finish_operation(
        home,
        crate::executable::CommandCaller::Worker,
        mode,
        &mut |_| {},
    )
    .map(|_| ())
    .map_err(|failure| failure.cause)
}

pub(crate) fn finish_report(
    home: &Path,
    expected_key: &str,
    caller: crate::executable::CommandCaller,
    progress: &mut impl FnMut(&FinishCommit),
) -> std::result::Result<FinishOutcome, FinishFailure> {
    finish_operation(home, caller, FinishMode::Prepared(expected_key), progress)
}

/// Only the two native entrypoints choose confirmation timing; no request supplies IO or paths.
enum FinishMode<'a> {
    Prepared(&'a str),
    Cli {
        answer: &'a mut dyn std::io::BufRead,
        out: &'a mut dyn std::io::Write,
    },
}

impl FinishMode<'_> {
    fn expected(&self) -> Option<&str> {
        match self {
            Self::Prepared(key) => Some(key),
            Self::Cli { .. } => None,
        }
    }

    fn confirm(&mut self, home: &Path, stats: &Stats, files: &FinishManifest) -> Result<bool> {
        let Self::Cli { answer, out } = self else {
            return Ok(true);
        };
        writeln!(out, "v1's old files in {}:", home.display())?;
        for target in &files.targets {
            writeln!(
                out,
                "  {} ({} bytes)",
                home.join(&target.name).display(),
                target.bytes
            )?;
        }
        if !stats.deleted.is_empty() {
            writeln!(
                out,
                "v1 sessions deleted from oboete.db after they were imported (forget them to remove \
                 them here too): {}",
                stats.deleted.join(", ")
            )?;
        }
        if !stats.uncertain.is_empty() {
            writeln!(
                out,
                "v1 session deletion cannot be determined (earlier redaction or clipping): {}",
                stats.uncertain.join(", ")
            )?;
        }
        write!(out, "Delete these files? Type yes to delete them: ")?;
        out.flush()?;
        let mut line = String::new();
        answer.read_line(&mut line)?;
        if line.trim() == "yes" {
            Ok(true)
        } else {
            writeln!(out, "Nothing was deleted.")?;
            Ok(false)
        }
    }

    fn removed(&mut self, path: &Path, result: &std::io::Result<()>) -> Result<()> {
        if let Self::Cli { out, .. } = self {
            match result {
                Ok(()) => writeln!(out, "deleted {}", path.display())?,
                Err(error) => writeln!(out, "not deleted {}: {error}", path.display())?,
            }
        }
        Ok(())
    }
}

fn finish_operation(
    home: &Path,
    caller: crate::executable::CommandCaller,
    mut mode: FinishMode<'_>,
    progress: &mut impl FnMut(&FinishCommit),
) -> std::result::Result<FinishOutcome, FinishFailure> {
    let mut outcome = FinishOutcome::default();
    let mut code = FailureCode::Busy;
    let result = (|| -> Result<()> {
        let proof = crate::executable::CommandHome::new(home, caller)?;
        let from = home.join("oboete.db");
        code = FailureCode::InvalidSource;
        check_source(home, &from)?;
        let expected = mode.expected();
        if let Some(expected) = expected {
            code = FailureCode::Stale;
            anyhow::ensure!(
                preview_finish(home)?.key == expected,
                "finish preview is stale"
            );
        }
        code = FailureCode::Busy;
        proof.check(home)?;
        let _import = crate::import::lock(home)?;
        proof.check(home)?;
        let _config = crate::settings::config_lock(home)?;
        proof.check(home)?;
        code = FailureCode::InvalidConfig;
        let config = finish_config_bytes(home)?;
        let capture = capture_settings(config.as_deref(), SOURCE)?;
        let backup = finish_backup(home, config.as_deref())?;
        code = FailureCode::InvalidSource;
        let mut manifest = finish_manifest(home, &backup)?;
        check_finish_source(&from)?;
        let mut raw = None;
        let (source, before) = with_preview_v1(&from, |v1, source| {
            code = FailureCode::Stale;
            proof.check(home)?;
            anyhow::ensure!(
                config == finish_config_bytes(home)? && manifest == finish_manifest(home, &backup)?,
                "finish inputs changed during admission"
            );
            if let Some(expected) = expected {
                anyhow::ensure!(
                    finish_key(home, source, config.as_deref(), &manifest)? == expected,
                    "finish preview is stale"
                );
            }
            check_source(home, &from)?;
            let device = crate::db::device_id(v1).context("not a v1 store")?;
            outcome.deletion = FinishDeletionReport::planned(&manifest, &capture.rules);
            code = FailureCode::Failed;
            raw = Some(crate::raw::open_report(home, Some(&proof), &mut |_| {
                outcome.effects.stores_changed = true;
                progress(&FinishCommit::Effects(outcome.effects.clone()));
                Ok(())
            })?);
            let raw = raw.as_mut().context("finish raw is absent")?;
            proof.check(home)?;
            let report = crate::forget::reconcile_report(home, raw, &mut |_, applied| {
                outcome.effects.stores_changed = true;
                outcome.effects.forget_requests_applied += applied as u64;
                progress(&FinishCommit::Effects(outcome.effects.clone()));
            })?;
            outcome.effects.forget_log_warnings = report.problems.len() as u64;
            if !report.problems.is_empty() {
                progress(&FinishCommit::Effects(outcome.effects.clone()));
            }
            if matches!(&mode, FinishMode::Cli { .. }) {
                for problem in &report.problems {
                    eprintln!("oboete: forget request log: {problem}");
                }
            }
            proof.check(home)?;
            let effects = &mut outcome.effects;
            read_connection(
                v1,
                raw,
                &device,
                &capture,
                &mut outcome.stats,
                &mut |event| {
                    effects.stores_changed = true;
                    progress(&FinishCommit::Import(event.clone()));
                },
            )?;
            Ok((source.to_owned(), fingerprint(v1)?))
        })?;
        // Retain the shared Raw swap hold through CLI input and deletion, after scratch cleanup.
        let _raw_hold = raw.context("finish raw is absent")?;
        proof.check(home)?;
        if matches!(&mode, FinishMode::Cli { .. }) {
            manifest = finish_manifest(home, &backup)?;
            outcome.deletion = FinishDeletionReport::planned(&manifest, &capture.rules);
        }
        progress(&outcome.deletion.event());
        if !mode.confirm(home, &outcome.stats, &manifest)? {
            return Ok(());
        }

        code = FailureCode::Stale;
        proof.check(home)?;
        anyhow::ensure!(
            config == finish_config_bytes(home)?,
            "finish config changed after the import pass"
        );
        let current = finish_manifest(home, &backup)?;
        if current != manifest {
            for (target, reported) in manifest.targets.iter().zip(&mut outcome.deletion.targets) {
                if current.targets.iter().find(|now| now.name == target.name) != Some(target) {
                    reported.state = FinishTargetState::Changed;
                }
            }
            progress(&outcome.deletion.event());
            anyhow::bail!("v1's old files changed after the import pass: nothing was deleted");
        }
        check_finish_source(&from)?;
        with_preview_v1(&from, |v1, now| {
            anyhow::ensure!(
                now == source && fingerprint(v1)? == before,
                "oboete.db changed after the import pass (an old hook still writes to it): nothing was \
                 deleted; run `oboete migrate --finish` again"
            );
            Ok(())
        })?;
        proof.check(home)?;

        let (deletion_code, removed) = remove_finish_targets(
            home,
            &proof,
            config.as_deref(),
            &manifest,
            &mut outcome.deletion,
            &mut mode,
            progress,
        );
        code = deletion_code;
        removed
    })();
    match result {
        Ok(()) => Ok(outcome),
        Err(cause) => Err(FinishFailure {
            outcome: Box::new(outcome),
            code: failure_code(&cause, code),
            cause,
        }),
    }
}

fn remove_finish_targets(
    home: &Path,
    proof: &crate::executable::CommandHome,
    config: Option<&[u8]>,
    manifest: &FinishManifest,
    deletion: &mut FinishDeletionReport,
    mode: &mut FinishMode<'_>,
    progress: &mut impl FnMut(&FinishCommit),
) -> (FailureCode, Result<()>) {
    let mut code = FailureCode::Stale;
    let result = (|| -> Result<()> {
        // The last check/unlink gap and old v1 writers retaining an unlinked handle remain
        // spec 7.5's cutover precondition; import.lock does not stop that old writer.
        let mut order: Vec<usize> = (0..manifest.targets.len()).collect();
        order.sort_by_key(|&index| !manifest.targets[index].name.starts_with("oboete.db"));
        for index in order {
            code = FailureCode::Stale;
            proof.check(home)?;
            anyhow::ensure!(
                config == finish_config_bytes(home)?.as_deref(),
                "finish config changed during deletion"
            );
            let target = &manifest.targets[index];
            let path = home.join(&target.name);
            let current = (|| -> Result<()> {
                let mut walk = FinishWalk::default();
                let digest = finish_node(home, Path::new(&target.name), 0, &mut walk)?;
                anyhow::ensure!(
                    digest == target.digest
                        && walk.nodes == target.nodes
                        && walk.bytes == target.bytes
                        && walk.directory == target.directory,
                    "finish target changed before deletion"
                );
                Ok(())
            })();
            if let Err(error) = current {
                deletion.targets[index].state = FinishTargetState::Changed;
                progress(&deletion.event());
                return Err(error);
            }
            proof.check(home)?;
            code = FailureCode::Failed;
            // Preserve the CLI's directory-symlink removal on Windows too. The standard
            // remove_dir_all unlinks a symlink without traversing its outside target.
            let removed = if target.directory || path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            deletion.attempted += 1;
            match &removed {
                Ok(()) => {
                    deletion.removed += 1;
                    deletion.removed_bytes += target.bytes;
                    deletion.targets[index].state = FinishTargetState::Removed;
                }
                Err(_) => {
                    deletion.failed += 1;
                    deletion.targets[index].state = if target.directory {
                        deletion.uncertain += 1;
                        FinishTargetState::UnknownExtent
                    } else {
                        FinishTargetState::Failed
                    };
                }
            }
            // Preserve actual effects before either an observer or stdout can fail.
            progress(&deletion.event());
            mode.removed(&path, &removed)?;
        }
        anyhow::ensure!(
            deletion.failed == 0,
            "{} file(s) not deleted",
            deletion.failed
        );
        Ok(())
    })();
    (code, result)
}

/// v1's runtime files in the home, spec 7.4's list (what ~/.oboete held on WSL on 2026-09-25):
/// oboete.db and memory.db with their -wal and -shm, the pre-*.db snapshots with theirs, and the
/// pre-rollout-*, spool, cache and logs directories; with their sizes, a directory's in all.
/// `eval/` is not among them.
fn old_files(home: &Path) -> Result<Vec<(std::path::PathBuf, u64)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(home)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.file_type()?.is_dir();
        if old_name(&name, dir) {
            files.push((entry.path(), size(&entry.path())?));
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OldFileCategory {
    V1Database,
    MemoryDatabase,
    Snapshot,
    RolloutDirectory,
    Spool,
    Cache,
    Logs,
}

impl OldFileCategory {
    pub(crate) const ALL: [Self; 7] = [
        Self::V1Database,
        Self::MemoryDatabase,
        Self::Snapshot,
        Self::RolloutDirectory,
        Self::Spool,
        Self::Cache,
        Self::Logs,
    ];
}

/// The native retained-file selector. `dir` is the entry's own, non-followed type.
/// A symlink is not a directory here, even when its target is one.
pub(crate) fn old_category(name: &str, dir: bool) -> Option<OldFileCategory> {
    if dir {
        return match name {
            "spool" => Some(OldFileCategory::Spool),
            "cache" => Some(OldFileCategory::Cache),
            "logs" => Some(OldFileCategory::Logs),
            name if name.starts_with("pre-rollout-") => Some(OldFileCategory::RolloutDirectory),
            _ => None,
        };
    }
    let db = |stem: &str| {
        ["", "-wal", "-shm"]
            .iter()
            .any(|end| name == format!("{stem}{end}"))
    };
    if db("oboete.db") {
        return Some(OldFileCategory::V1Database);
    }
    if db("memory.db") {
        return Some(OldFileCategory::MemoryDatabase);
    }
    (name.starts_with("pre-")
        && [".db", ".db-wal", ".db-shm"]
            .iter()
            .any(|end| name.ends_with(end)))
    .then_some(OldFileCategory::Snapshot)
}

fn old_name(name: &str, dir: bool) -> bool {
    old_category(name, dir).is_some()
}

fn size(path: &Path) -> Result<u64> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        total += size(&entry?.path())?;
    }
    Ok(total)
}

/// v1's events past the checkpoint, read in id order and cut into batches by `IMPORT_BATCH`
/// records and `MAX_BATCH_BYTES` of bodies, each appended in (ts, id) order with its checkpoint:
/// its highest id and that event's fingerprint (D6).
fn events(
    v1: &Connection,
    raw: &mut Raw,
    device: &str,
    settings: &Settings,
    stats: &mut Stats,
    committed: &mut impl FnMut(&Committed),
) -> Result<()> {
    let key = format!("{SOURCE}:{device}");
    let through = match raw.migration_checkpoints(&key)?.remove(&key) {
        None => 0,
        Some(c) => {
            // v1 reuses the ids of its newest events when the old viewer deletes their session.
            let now = row_at(v1, c.through)?;
            anyhow::ensure!(
                now.is_some() && now == c.row,
                "v1's event {} is not the one an earlier pass imported up to: the old viewer \
                 deleted the newest session since, and v1 may give its ids to new events, which \
                 this pass would skip. Nothing was imported",
                c.through
            );
            c.through
        }
    };
    // ponytail: an event whose session row is gone is passed over; v1 deletes a session's events
    // with it (`db::delete_session`), and the owner's store held none on 2026-10-01.
    let mut st = v1.prepare(EVENTS_SQL)?;
    let mut rows = st.query([through])?;
    let mut batch = Batch::default();
    while let Some(r) = rows.next()? {
        let row = v1_row(r)?;
        let (session, stored): (String, String) = (r.get(2)?, r.get(4)?);
        let agent: String = r.get(5)?;
        let captured = captured_row(r, settings)?;
        let bytes: usize = captured.iter().map(|c| c.event.body.len()).sum();
        if batch.records.len() + captured.len() > IMPORT_BATCH
            || batch.bytes + bytes > MAX_BATCH_BYTES
        {
            stats.records += batch.append(raw, &key, settings)?;
            notify(stats, Stage::Events, committed);
            #[cfg(test)]
            if let Some(between) = BETWEEN_BATCHES.with(|seam| seam.borrow_mut().take()) {
                between();
            }
        }
        batch.bytes += bytes;
        let identity = serde_json::to_string(&(&row, crate::forget::hash(stored.as_bytes())))?;
        let session = crate::forget::session(&agent, &session);
        batch
            .records
            .extend(captured.into_iter().enumerate().map(|(i, c)| {
                (
                    c,
                    ImportIdentity {
                        origin: crate::forget::origin(&key, &format!("{identity}:{i}")),
                        session: session.clone(),
                        ambiguous: None,
                        unverified: false,
                    },
                )
            }));
        batch.last = Some(row);
        stats.events += 1;
    }
    if batch.last.is_some() {
        stats.records += batch.append(raw, &key, settings)?;
        notify(stats, Stage::Events, committed);
    }
    Ok(())
}

const EVENTS_SQL: &str =
    "SELECT e.id, e.ts, e.session_id, e.event, e.payload, s.agent, s.repo, s.cwd
    FROM events e JOIN sessions s ON s.id = e.session_id WHERE e.id > ?1 ORDER BY e.id";

fn captured_row(r: &rusqlite::Row, settings: &Settings) -> Result<Vec<Captured>> {
    let (session, event, stored): (String, String, String) = (r.get(2)?, r.get(3)?, r.get(4)?);
    let (agent, repo, cwd): (String, String, Option<String>) = (r.get(5)?, r.get(6)?, r.get(7)?);
    Ok(capture::imported(
        &agent,
        &event,
        &payload(&session, &stored),
        r.get(1)?,
        &repo,
        cwd.as_deref(),
        settings,
    ))
}

const REPOS_SQL: &str = "SELECT r.session_id, r.repo, s.agent, s.cwd, s.started_at
    FROM session_repos r JOIN sessions s ON s.id = r.session_id
    ORDER BY s.started_at, r.session_id, r.repo";

fn captured_repo(r: &rusqlite::Row, settings: &Settings) -> Result<Vec<Captured>> {
    let (session, repo, agent): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
    let (cwd, started): (Option<String>, i64) = (r.get(3)?, r.get(4)?);
    Ok(capture::imported(
        &agent,
        "Touch",
        &json!({"session_id":session}),
        started,
        &repo,
        cwd.as_deref(),
        settings,
    ))
}

/// v1 events read since the last append, in v1's id order: their records, and the last one's
/// fingerprint.
#[derive(Default)]
struct Batch {
    records: Vec<(Captured, ImportIdentity)>,
    bytes: usize,
    last: Option<V1Row>,
}

impl Batch {
    /// The records in (ts, id) order with the checkpoint, also when none was recorded; nothing
    /// when no event was read. The records appended.
    fn append(&mut self, raw: &mut Raw, key: &str, settings: &Settings) -> Result<u64> {
        let Some(last) = self.last.take() else {
            return Ok(0);
        };
        let mut records = std::mem::take(&mut self.records);
        // Stable, so (ts, id): at most one record per event, read in id order.
        records.sort_by_key(|(c, _)| c.event.ts);
        let (records, origins): (Vec<_>, Vec<_>) = records.into_iter().unzip();
        self.bytes = 0;
        let checkpoint = Checkpoint {
            key: key.to_owned(),
            through: last.id,
            row: Some(last),
            prefix: None,
        };
        Ok(raw
            .append_imported_under(&records, &origins, settings, Some(&checkpoint))?
            .len() as u64)
    }
}

/// A v1 event's stored body (v1's hook kept these fields of each event) as the hook payload
/// capture reads. It has no `transcript_path`, so capture reads no transcript.
fn payload(session: &str, stored: &str) -> Value {
    let stored: Value = serde_json::from_str(stored).unwrap_or(Value::Null);
    let mut payload = json!({"session_id": session});
    for (v1, hook) in [
        ("prompt", "prompt"),
        ("source", "source"),
        ("tool", "tool_name"),
        ("input", "tool_input"),
        ("output", "tool_response"),
        ("assistant", "last_assistant_message"),
        ("summary", "compact_summary"),
        ("reason", "reason"),
    ] {
        if let Some(v) = stored.get(v1) {
            payload[hook] = v.clone();
        }
    }
    payload
}

/// The imported v1 sessions none of whose events oboete.db still holds.
fn deleted(v1: &Connection, raw: &Raw, settings: &Settings, stats: &mut Stats) -> Result<()> {
    // Design B's labels passed the gate: v1's ids are compared as capture labels them, and each
    // is printed as the rules read now.
    let label = |id: &str| {
        let payload = json!({ "session_id": id });
        capture::imported("claude", "Touch", &payload, 0, "", None, settings)
            .pop()
            .map(|c| c.event.session)
            .unwrap_or_default()
    };
    let mut st = v1.prepare("SELECT DISTINCT session_id FROM events")?;
    let held = st
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|id| id.map(|id| label(&id)))
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    for stored in raw.sessions_of(SOURCE)? {
        let current = label(&stored);
        if held.contains(&current) {
            continue;
        }
        if stored.contains(crate::redact::MASK) || stored.contains("\n…[cut: ") {
            stats.uncertain.push(current);
        } else {
            stats.deleted.push(current);
        }
    }
    Ok(())
}

/// v1's `session_repos` rows as `touch` records at their session's start (A57), so the
/// repositories a v1 session touched stay known; a row whose labels Design B holds is passed over.
fn repos(
    v1: &Connection,
    raw: &mut Raw,
    settings: &Settings,
    stats: &mut Stats,
    committed: &mut impl FnMut(&Committed),
) -> Result<()> {
    let mut known = raw.touches(SOURCE)?;
    let mut st = v1.prepare(REPOS_SQL)?;
    let mut rows = st.query([])?;
    let mut batch = Vec::new();
    while let Some(r) = rows.next()? {
        for c in captured_repo(r, settings)? {
            if known.insert((c.event.session.clone(), c.event.repo.clone())) {
                batch.push(c);
            }
        }
    }
    for chunk in batch.chunks(IMPORT_BATCH) {
        stats.repos += raw.append_imported_under(chunk, &[], settings, None)?.len() as u64;
        notify(stats, Stage::Repos, committed);
    }
    Ok(())
}

/// v1's observations, summaries and prompts as import ops (D5), under their uids (`<device_id>:
/// <o|s|p><id>` where v1 left it NULL) and through the gate, each once by its source id.
fn documents(
    v1: &Connection,
    raw: &mut Raw,
    device: &str,
    settings: &Settings,
    stats: &mut Stats,
    committed: &mut impl FnMut(&Committed),
) -> Result<()> {
    let source = format!("{SOURCE}:{device}");
    let mut known = raw.import_keys(&source)?;
    // Validate and shape every document before appending, as the native pass always has.
    let mut docs = Vec::new();
    read_documents(v1, device, settings, &mut known, stats, |doc| {
        docs.push(doc)
    })?;
    // Owner decision 43: documents gated under rules turned off owe the op text scan.
    raw.gates_with(&settings.rules);
    raw.append_imports_counted(docs, |n| {
        stats.documents += n;
        notify(stats, Stage::Documents, committed);
    })?;
    Ok(())
}

fn read_documents(
    v1: &Connection,
    device: &str,
    settings: &Settings,
    known: &mut std::collections::HashSet<String>,
    stats: &mut Stats,
    mut document: impl FnMut(ImportDoc),
) -> Result<()> {
    let source = format!("{SOURCE}:{device}");
    let gate = |text: String| crate::redact::outbound_with(&text, &settings.rules);
    for (letter, sql) in [
        (
            "o",
            "SELECT id, uid, session_id, repo, ts, kind, title, body FROM observations ORDER BY id",
        ),
        (
            "s",
            "SELECT id, uid, session_id, repo, ts, 'summary', '', body FROM summaries ORDER BY id",
        ),
        (
            "p",
            "SELECT id, uid, session_id, repo, ts, 'prompt', '', body FROM prompts ORDER BY id",
        ),
    ] {
        let mut st = v1.prepare(sql)?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let source_id = format!("{letter}{}", r.get::<_, i64>(0)?);
            if !known.insert(source_id.clone()) {
                stats.seen += 1;
                continue;
            }
            let uid: Option<String> = r.get(1)?;
            document(ImportDoc {
                uid: uid.unwrap_or_else(|| format!("{device}:{source_id}")),
                source: source.clone(),
                source_id,
                kind: r.get(5)?,
                // Labels are stored text too (spec 2.2), as capture gates a record's.
                repo: gate(r.get(3)?),
                session: gate(r.get(2)?),
                ts: r.get(4)?,
                title: gate(r.get(6)?),
                body: gate(r.get(7)?),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
