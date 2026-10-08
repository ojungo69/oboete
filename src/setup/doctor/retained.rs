//! Fixed retained-runtime categories and evaluation copies; metadata only, never file contents.
//! ponytail: reuse native directory name buffers and top-level bindings. Stream enumeration
//! if large directories or deep paths make those remaining buffers the memory bottleneck.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::{DiagnosticDirectory, EntryKind, EntryMetadata};
use super::{CheckState, Count, PreviewCopyError, error_state, stability};
use crate::migrate::{OldFileCategory, old_category};

type Observed<T> = std::result::Result<T, CheckState>;

#[derive(Serialize)]
struct Flag {
    state: CheckState,
    value: Option<bool>,
}
impl Flag {
    fn unknown(state: CheckState) -> Self {
        Self { state, value: None }
    }
}

#[derive(Serialize)]
struct Bytes {
    state: CheckState,
    value: Option<u64>,
}
impl Bytes {
    fn unknown(state: CheckState) -> Self {
        Self { state, value: None }
    }
}

#[derive(Serialize)]
struct CategoryChecks {
    category: OldFileCategory,
    present: Flag,
    targets: Count,
    bytes: Bytes,
}

#[derive(Serialize)]
struct EvaluationChecks {
    present: Flag,
    bytes: Bytes,
}

#[derive(Serialize)]
pub(super) struct RetainedChecks {
    pub(super) state: CheckState,
    categories: [CategoryChecks; 7],
    evaluation_copies: EvaluationChecks,
}

impl RetainedChecks {
    fn unknown(state: CheckState) -> Self {
        Self {
            state,
            categories: OldFileCategory::ALL.map(|category| CategoryChecks {
                category,
                present: Flag::unknown(state),
                targets: Count::unknown(state),
                bytes: Bytes::unknown(state),
            }),
            evaluation_copies: EvaluationChecks {
                present: Flag::unknown(state),
                bytes: Bytes::unknown(state),
            },
        }
    }

    fn refresh_state(&mut self) {
        let mut state = CheckState::Known;
        for row in &self.categories {
            for next in [row.present.state, row.targets.state, row.bytes.state] {
                state = stability(state, next);
            }
        }
        for next in [
            self.evaluation_copies.present.state,
            self.evaluation_copies.bytes.state,
        ] {
            state = stability(state, next);
        }
        self.state = stability(self.state, state);
    }
}

#[derive(PartialEq, Eq)]
struct Binding {
    name: OsString,
    kind: EntryKind,
    identity: String,
}
impl Binding {
    fn new(name: &OsStr, metadata: &EntryMetadata) -> Self {
        Self {
            name: name.to_owned(),
            kind: metadata.kind,
            identity: metadata.identity.clone(),
        }
    }
}

#[derive(Default, PartialEq, Eq)]
struct Tree {
    bytes: u64,
    fingerprint: [u8; 32],
}

// Length prefixes preserve field boundaries and non-UTF-8 OS names. Only metadata is
// hashed: the same four fields EntryMetadata equality previously retained per node.
fn hash_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

fn hash_entry(hash: &mut Sha256, path: &OsStr, metadata: &EntryMetadata) {
    hash_field(hash, path.as_encoded_bytes());
    hash.update([match metadata.kind {
        EntryKind::Directory => 0,
        EntryKind::Symlink => 1,
        EntryKind::File => 2,
        EntryKind::Other => 3,
    }]);
    hash_field(hash, metadata.identity.as_bytes());
    hash.update(metadata.bytes.to_le_bytes());
    let (negative, modified) = match metadata.modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(modified) => (false, modified),
        Err(error) => (true, error.duration()),
    };
    hash.update([u8::from(negative)]);
    hash.update(modified.as_secs().to_le_bytes());
    hash.update(modified.subsec_nanos().to_le_bytes());
}

struct CategorySnapshot {
    targets: Observed<Vec<Binding>>,
    bytes: Observed<Tree>,
}
impl CategorySnapshot {
    fn empty() -> Self {
        Self {
            targets: Ok(Vec::new()),
            bytes: Ok(Tree::default()),
        }
    }
}

struct EvaluationSnapshot {
    target: Observed<Option<Binding>>,
    bytes: Observed<Tree>,
}
impl EvaluationSnapshot {
    fn empty() -> Self {
        Self {
            target: Ok(None),
            bytes: Ok(Tree::default()),
        }
    }
}

#[derive(PartialEq, Eq)]
struct HomeBinding {
    identity: String,
    resolved: PathBuf,
}

struct Scan {
    home: Option<HomeBinding>,
    categories: [CategorySnapshot; 7],
    evaluation: EvaluationSnapshot,
}
impl Scan {
    fn empty(home: Option<HomeBinding>) -> Self {
        Self {
            home,
            categories: std::array::from_fn(|_| CategorySnapshot::empty()),
            evaluation: EvaluationSnapshot::empty(),
        }
    }
}

fn fail<T>(value: &mut Observed<T>, state: CheckState) {
    let state = value
        .as_ref()
        .err()
        .map_or(state, |before| stability(*before, state));
    *value = Err(state);
}

fn difference<T: PartialEq>(before: &Observed<T>, after: &Observed<T>) -> Option<CheckState> {
    match (before, after) {
        (Ok(before), Ok(after)) if before != after => Some(CheckState::Changed),
        (Err(before), Err(after)) => Some(stability(*before, *after)),
        (Err(state), _) | (_, Err(state)) => Some(*state),
        _ => None,
    }
}

fn possible(name: &OsStr) -> [Option<OldFileCategory>; 2] {
    let name = name.to_string_lossy();
    [old_category(&name, false), old_category(&name, true)]
}

fn slot(category: OldFileCategory) -> usize {
    OldFileCategory::ALL
        .iter()
        .position(|candidate| *candidate == category)
        .expect("the fixed category list is exhaustive")
}

fn fail_old(scan: &mut Scan, name: &OsStr, state: CheckState) {
    // Type was unavailable: every possible native category is affected, not just dir=false.
    for category in possible(name).into_iter().flatten() {
        let row = &mut scan.categories[slot(category)];
        fail(&mut row.targets, state);
        fail(&mut row.bytes, state);
    }
}

fn sorted_names(directory: &mut DiagnosticDirectory) -> Result<Vec<OsString>> {
    let mut names = directory.names()?;
    names.sort();
    anyhow::ensure!(
        !names.windows(2).any(|pair| pair[0] == pair[1]),
        PreviewCopyError::Changed
    );
    Ok(names)
}

struct Frame {
    directory: DiagnosticDirectory,
    leaf: OsString,
    path: PathBuf,
    metadata: EntryMetadata,
    names: Vec<OsString>,
    next: usize,
    entries: Sha256,
}

fn frame(
    parent: &DiagnosticDirectory,
    leaf: &OsStr,
    path: PathBuf,
    metadata: EntryMetadata,
) -> Result<Frame> {
    let mut directory = match parent.open_directory(leaf) {
        Ok(directory) => directory,
        Err(error) => {
            if parent
                .entry_metadata(leaf, false)
                .is_ok_and(|now| now != metadata)
            {
                return Err(PreviewCopyError::Changed.into());
            }
            return Err(error);
        }
    };
    anyhow::ensure!(
        directory.identity()? == metadata.identity,
        PreviewCopyError::Changed
    );
    let names = sorted_names(&mut directory)?;
    Ok(Frame {
        directory,
        leaf: leaf.to_owned(),
        path,
        metadata,
        names,
        next: 0,
        entries: Sha256::new(),
    })
}

/// Native size(): directories contribute child totals; every other entry contributes own len.
/// The explicit stack retains parent descriptors without recursive Rust calls or a depth cap.
/// The saved Tree is fixed size. Walking still retains the names of active ancestor
/// directories plus one frame/descriptor per depth, not constant traversal memory.
fn tree(parent: &DiagnosticDirectory, leaf: &OsStr, metadata: EntryMetadata) -> Result<Tree> {
    let root_path = PathBuf::from(leaf);
    let mut result = Tree::default();
    let mut fingerprint = Sha256::new();
    hash_entry(&mut fingerprint, root_path.as_os_str(), &metadata);
    if metadata.kind != EntryKind::Directory {
        result.bytes = metadata.bytes;
        result.fingerprint = fingerprint.finalize().into();
        return Ok(result);
    }
    let mut frames = vec![frame(parent, leaf, root_path, metadata)?];
    while !frames.is_empty() {
        let index = frames.len() - 1;
        let next = {
            let current = &mut frames[index];
            let name = current.names.get(current.next).cloned();
            current.next += usize::from(name.is_some());
            name
        };
        if let Some(name) = next {
            let current = &mut frames[index];
            let metadata = current.directory.entry_metadata(&name, false)?;
            let path = current.path.join(&name);
            hash_entry(&mut current.entries, &name, &metadata);
            hash_entry(&mut fingerprint, path.as_os_str(), &metadata);
            if metadata.kind == EntryKind::Directory {
                // An ancestry repeat can arise from unusual mount topology. Never recurse forever.
                anyhow::ensure!(
                    !frames
                        .iter()
                        .any(|ancestor| ancestor.metadata.identity == metadata.identity),
                    std::io::Error::from(std::io::ErrorKind::Unsupported)
                );
                let child = frame(&frames[index].directory, &name, path, metadata)?;
                frames.push(child);
            } else {
                result.bytes = result
                    .bytes
                    .checked_add(metadata.bytes)
                    .context("retained byte count overflow")?;
            }
            continue;
        }
        let mut completed = frames.pop().expect("the walker stack is nonempty");
        anyhow::ensure!(
            sorted_names(&mut completed.directory)? == completed.names,
            PreviewCopyError::Changed
        );
        let mut entries = Sha256::new();
        for name in &completed.names {
            let metadata = completed.directory.entry_metadata(name, false)?;
            hash_entry(&mut entries, name, &metadata);
        }
        anyhow::ensure!(
            entries.finalize() == completed.entries.finalize(),
            PreviewCopyError::Changed
        );
        let owner = frames.last().map_or(parent, |frame| &frame.directory);
        anyhow::ensure!(
            owner.entry_metadata(&completed.leaf, false)? == completed.metadata,
            PreviewCopyError::Changed
        );
    }
    result.fingerprint = fingerprint.finalize().into();
    Ok(result)
}

fn append_tree(total: &mut Observed<Tree>, next: Result<Tree>) {
    let next = match next {
        Ok(next) => next,
        Err(error) => {
            fail(total, error_state(&error));
            return;
        }
    };
    if let Ok(current) = total {
        let Some(bytes) = current.bytes.checked_add(next.bytes) else {
            *total = Err(CheckState::Unavailable);
            return;
        };
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"oboete:retained-forest:v1\0");
        fingerprint.update(current.fingerprint);
        fingerprint.update(next.fingerprint);
        current.bytes = bytes;
        current.fingerprint = fingerprint.finalize().into();
    }
}

fn eval_target(
    directory: &DiagnosticDirectory,
    name: &OsStr,
    own: &EntryMetadata,
) -> Observed<Option<Binding>> {
    let is_directory = match own.kind {
        EntryKind::Directory => true,
        EntryKind::Symlink => match directory.entry_metadata(name, true) {
            Ok(target) => target.kind == EntryKind::Directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error_state(&error.into())),
        },
        _ => false,
    };
    Ok(is_directory.then(|| Binding::new(name, own)))
}

fn eval_snapshot(
    directory: &DiagnosticDirectory,
    name: &OsStr,
    own: &EntryMetadata,
) -> EvaluationSnapshot {
    let target = eval_target(directory, name, own);
    let bytes = match &target {
        Ok(Some(_)) => tree(directory, name, own.clone()).map_err(|error| error_state(&error)),
        Ok(None) => Ok(Tree::default()),
        Err(state) => Err(*state),
    };
    EvaluationSnapshot { target, bytes }
}

fn recheck_top(
    scan: &mut Scan,
    directory: &DiagnosticDirectory,
    name: &OsStr,
    before: &EntryMetadata,
) {
    let after = match directory.entry_metadata(name, false) {
        Ok(after) => after,
        Err(error) => {
            let state = error_state(&error.into());
            fail_old(scan, name, state);
            if name == "eval" {
                fail(&mut scan.evaluation.target, state);
                fail(&mut scan.evaluation.bytes, state);
            }
            return;
        }
    };
    let selected_before =
        old_category(&name.to_string_lossy(), before.kind == EntryKind::Directory);
    let selected_after = old_category(&name.to_string_lossy(), after.kind == EntryKind::Directory);
    for category in possible(name).into_iter().flatten() {
        let row = &mut scan.categories[slot(category)];
        let was = selected_before == Some(category);
        let now = selected_after == Some(category);
        if was != now || (was && (before.kind != after.kind || before.identity != after.identity)) {
            fail(&mut row.targets, CheckState::Changed);
            fail(&mut row.bytes, CheckState::Changed);
        } else if was && *before != after {
            fail(&mut row.bytes, CheckState::Changed);
        }
    }
    if name == "eval" {
        let target = eval_target(directory, name, &after);
        if let Some(state) = difference(&scan.evaluation.target, &target) {
            fail(&mut scan.evaluation.target, state);
            fail(&mut scan.evaluation.bytes, state);
        } else if matches!(target, Ok(Some(_))) && *before != after {
            fail(&mut scan.evaluation.bytes, CheckState::Changed);
        }
    }
}

fn capture(home: &Path) -> Observed<Scan> {
    let read = (|| -> Result<Scan> {
        let mut directory = match DiagnosticDirectory::open(home) {
            Ok(directory) => directory,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(Scan::empty(None));
            }
            Err(error) => return Err(error),
        };
        let mut scan = Scan::empty(Some(HomeBinding {
            identity: directory.identity()?,
            resolved: directory.resolved_path().to_owned(),
        }));
        let relevant = |name: &OsString| {
            name == "eval" || possible(name).into_iter().any(|kind| kind.is_some())
        };
        let names: Vec<_> = sorted_names(&mut directory)?
            .into_iter()
            .filter(relevant)
            .collect();
        let mut entries = Vec::new();
        for name in &names {
            let metadata = match directory.entry_metadata(name, false) {
                Ok(metadata) => metadata,
                Err(error) => {
                    let state = error_state(&error.into());
                    fail_old(&mut scan, name, state);
                    if name == "eval" {
                        fail(&mut scan.evaluation.target, state);
                        fail(&mut scan.evaluation.bytes, state);
                    }
                    continue;
                }
            };
            if let Some(category) = old_category(
                &name.to_string_lossy(),
                metadata.kind == EntryKind::Directory,
            ) {
                let row = &mut scan.categories[slot(category)];
                if let Ok(targets) = &mut row.targets {
                    targets.push(Binding::new(name, &metadata));
                }
                if row.bytes.is_ok() {
                    append_tree(&mut row.bytes, tree(&directory, name, metadata.clone()));
                }
            }
            if name == "eval" {
                scan.evaluation = eval_snapshot(&directory, name, &metadata);
            }
            entries.push((name.clone(), metadata));
        }
        let after: Vec<_> = sorted_names(&mut directory)?
            .into_iter()
            .filter(relevant)
            .collect();
        for category in OldFileCategory::ALL {
            let belonging = |name: &&OsString| possible(name).contains(&Some(category));
            if names
                .iter()
                .filter(belonging)
                .ne(after.iter().filter(belonging))
            {
                let row = &mut scan.categories[slot(category)];
                fail(&mut row.targets, CheckState::Changed);
                fail(&mut row.bytes, CheckState::Changed);
            }
        }
        if names.iter().any(|name| name == "eval") != after.iter().any(|name| name == "eval") {
            fail(&mut scan.evaluation.target, CheckState::Changed);
            fail(&mut scan.evaluation.bytes, CheckState::Changed);
        }
        for (name, metadata) in entries {
            recheck_top(&mut scan, &directory, &name, &metadata);
        }
        Ok(scan)
    })();
    read.map_err(|error| error_state(&error))
}

fn bytes(value: &Observed<Tree>) -> Bytes {
    match value {
        Ok(tree) => Bytes {
            state: CheckState::Known,
            value: Some(tree.bytes),
        },
        Err(state) => Bytes::unknown(*state),
    }
}

fn row(category: OldFileCategory, snapshot: &CategorySnapshot) -> CategoryChecks {
    let (present, targets) = match &snapshot.targets {
        Ok(targets) => (
            Flag {
                state: CheckState::Known,
                value: Some(!targets.is_empty()),
            },
            match i64::try_from(targets.len()) {
                Ok(value) => Count {
                    state: CheckState::Known,
                    value: Some(value),
                },
                Err(_) => Count::unknown(CheckState::Unavailable),
            },
        ),
        Err(state) => (Flag::unknown(*state), Count::unknown(*state)),
    };
    CategoryChecks {
        category,
        present,
        targets,
        bytes: bytes(&snapshot.bytes),
    }
}

pub(super) struct Observation {
    home: PathBuf,
    before: Observed<Scan>,
}

impl Observation {
    pub(super) fn begin(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            before: capture(home),
        }
    }

    pub(super) fn checks(&self) -> RetainedChecks {
        let scan = match &self.before {
            Ok(scan) => scan,
            Err(state) => return RetainedChecks::unknown(*state),
        };
        let mut checks = RetainedChecks {
            state: CheckState::Known,
            categories: std::array::from_fn(|index| {
                row(OldFileCategory::ALL[index], &scan.categories[index])
            }),
            evaluation_copies: EvaluationChecks {
                present: match &scan.evaluation.target {
                    Ok(target) => Flag {
                        state: CheckState::Known,
                        value: Some(target.is_some()),
                    },
                    Err(state) => Flag::unknown(*state),
                },
                bytes: bytes(&scan.evaluation.bytes),
            },
        };
        checks.refresh_state();
        checks
    }

    pub(super) fn finish(&self, checks: &mut RetainedChecks) -> Option<CheckState> {
        let after = capture(&self.home);
        match (&self.before, &after) {
            (Ok(before), Ok(after)) if before.home == after.home => {
                for (index, row) in checks.categories.iter_mut().enumerate() {
                    let before = &before.categories[index];
                    let after = &after.categories[index];
                    if let Some(state) = difference(&before.targets, &after.targets) {
                        row.present = Flag::unknown(stability(row.present.state, state));
                        row.targets = Count::unknown(stability(row.targets.state, state));
                        row.bytes = Bytes::unknown(stability(row.bytes.state, state));
                    }
                    if let Some(state) = difference(&before.bytes, &after.bytes) {
                        row.bytes = Bytes::unknown(stability(row.bytes.state, state));
                    }
                }
                let row = &mut checks.evaluation_copies;
                if let Some(state) = difference(&before.evaluation.target, &after.evaluation.target)
                {
                    row.present = Flag::unknown(stability(row.present.state, state));
                    row.bytes = Bytes::unknown(stability(row.bytes.state, state));
                }
                if let Some(state) = difference(&before.evaluation.bytes, &after.evaluation.bytes) {
                    row.bytes = Bytes::unknown(stability(row.bytes.state, state));
                }
            }
            (Ok(_), Ok(_)) => {
                *checks = RetainedChecks::unknown(stability(checks.state, CheckState::Changed));
            }
            (Err(before), Err(after)) => {
                *checks =
                    RetainedChecks::unknown(stability(checks.state, stability(*before, *after)));
            }
            (Err(state), _) | (_, Err(state)) => {
                *checks = RetainedChecks::unknown(stability(checks.state, *state));
            }
        }
        checks.refresh_state();
        (!matches!(checks.state, CheckState::Known)).then_some(checks.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn saved_tree_has_no_owned_descendant_allocation() {
        // A structural bound, independent of input size or an arbitrary RSS/node threshold.
        // The previous Vec of descendant metadata fails this invariant even when empty.
        assert!(!std::hint::black_box(std::mem::needs_drop::<Tree>()));
    }

    fn entry_digest(path: &OsStr, metadata: &EntryMetadata) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash_entry(&mut hash, path, metadata);
        hash.finalize().into()
    }

    #[test]
    fn entry_digest_binds_framed_name_identity_kind_bytes_and_signed_mtime() {
        let base = EntryMetadata {
            kind: EntryKind::File,
            identity: "owned:one".into(),
            bytes: 4,
            modified: UNIX_EPOCH + Duration::from_nanos(100),
        };
        let before = entry_digest(OsStr::new("cache/item"), &base);
        let changed = [
            EntryMetadata {
                identity: "owned:two".into(),
                ..base.clone()
            },
            EntryMetadata {
                kind: EntryKind::Symlink,
                ..base.clone()
            },
            EntryMetadata {
                bytes: 5,
                ..base.clone()
            },
            EntryMetadata {
                modified: UNIX_EPOCH - Duration::from_nanos(100),
                ..base.clone()
            },
            EntryMetadata {
                modified: UNIX_EPOCH + Duration::from_nanos(200),
                ..base.clone()
            },
        ];
        for metadata in changed {
            assert_ne!(before, entry_digest(OsStr::new("cache/item"), &metadata));
        }
        assert_ne!(before, entry_digest(OsStr::new("cache/other"), &base));
        // Without field framing these path/kind/identity bytes could be concatenated alike.
        let left = EntryMetadata {
            identity: "b\u{2}c".into(),
            ..base.clone()
        };
        let right = EntryMetadata {
            identity: "c".into(),
            ..base
        };
        assert_ne!(
            entry_digest(OsStr::new("a"), &left),
            entry_digest(OsStr::new("a\u{2}b"), &right)
        );
    }

    #[cfg(unix)]
    #[test]
    fn entry_digest_keeps_non_utf8_names_distinct_from_lossy_replacements() {
        use std::os::unix::ffi::OsStringExt;
        let metadata = EntryMetadata {
            kind: EntryKind::File,
            identity: "owned:one".into(),
            bytes: 4,
            modified: UNIX_EPOCH,
        };
        let raw = OsString::from_vec(vec![b'a', 0xff]);
        let replacement = OsStr::new("a\u{fffd}");
        assert_eq!(raw.to_string_lossy(), replacement.to_string_lossy());
        assert_ne!(
            entry_digest(&raw, &metadata),
            entry_digest(replacement, &metadata)
        );
    }

    #[test]
    fn same_byte_descendant_name_or_identity_change_keeps_top_target_facts() {
        for replace_identity in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let cache = root.path().join("cache");
            std::fs::create_dir(&cache).unwrap();
            let item = cache.join("item");
            std::fs::write(&item, b"same").unwrap();
            let initial = DiagnosticDirectory::open(&cache)
                .unwrap()
                .entry_metadata(OsStr::new("item"), false)
                .unwrap();
            let before = Observation::begin(root.path());
            let mut checks = before.checks();
            let index = slot(OldFileCategory::Cache);
            assert_eq!(checks.categories[index].bytes.value, Some(4));
            let name = if replace_identity {
                let replacement = root.path().join("replacement");
                std::fs::write(&replacement, b"same").unwrap();
                std::fs::rename(&item, root.path().join("retired")).unwrap();
                std::fs::rename(replacement, &item).unwrap();
                "item"
            } else {
                std::fs::rename(&item, cache.join("other")).unwrap();
                "other"
            };
            let after = DiagnosticDirectory::open(&cache)
                .unwrap()
                .entry_metadata(OsStr::new(name), false)
                .unwrap();
            assert_eq!(initial.bytes, after.bytes);
            if replace_identity {
                assert_ne!(initial.identity, after.identity);
            } else {
                assert_eq!(initial.identity, after.identity);
            }
            assert!(matches!(
                before.finish(&mut checks),
                Some(CheckState::Changed)
            ));
            let row = &checks.categories[index];
            assert!(matches!(row.present.state, CheckState::Known));
            assert_eq!(row.present.value, Some(true));
            assert!(matches!(row.targets.state, CheckState::Known));
            assert_eq!(row.targets.value, Some(1));
            assert!(matches!(row.bytes.state, CheckState::Changed));
            assert_eq!(row.bytes.value, None);
            assert_eq!(
                checks.categories[slot(OldFileCategory::Logs)].bytes.value,
                Some(0)
            );
        }
    }

    #[test]
    fn same_byte_top_level_identity_change_still_invalidates_targets() {
        let root = tempfile::tempdir().unwrap();
        let item = root.path().join("memory.db");
        std::fs::write(&item, b"same").unwrap();
        let before = Observation::begin(root.path());
        let mut checks = before.checks();
        let replacement = root.path().join("replacement");
        std::fs::write(&replacement, b"same").unwrap();
        std::fs::rename(&item, root.path().join("retired")).unwrap();
        std::fs::rename(replacement, &item).unwrap();
        assert!(matches!(
            before.finish(&mut checks),
            Some(CheckState::Changed)
        ));
        let row = &checks.categories[slot(OldFileCategory::MemoryDatabase)];
        assert!(matches!(row.present.state, CheckState::Changed));
        assert!(matches!(row.targets.state, CheckState::Changed));
        assert!(matches!(row.bytes.state, CheckState::Changed));
        assert_eq!(
            (row.present.value, row.targets.value, row.bytes.value),
            (None, None, None)
        );
    }
}
