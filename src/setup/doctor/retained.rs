//! Fixed retained-runtime categories and evaluation copies; metadata only, never file contents.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Serialize;

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
    nodes: Vec<(PathBuf, EntryMetadata)>,
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
    entries: Vec<(OsString, EntryMetadata)>,
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
        entries: Vec::new(),
    })
}

/// Native size(): directories contribute child totals; every other entry contributes own len.
/// The explicit stack retains parent descriptors without recursive Rust calls or a depth cap.
fn tree(parent: &DiagnosticDirectory, leaf: &OsStr, metadata: EntryMetadata) -> Result<Tree> {
    let root_path = PathBuf::from(leaf);
    let mut result = Tree {
        bytes: 0,
        nodes: vec![(root_path.clone(), metadata.clone())],
    };
    if metadata.kind != EntryKind::Directory {
        result.bytes = metadata.bytes;
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
            current.entries.push((name.clone(), metadata.clone()));
            result.nodes.push((path.clone(), metadata.clone()));
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
        for (name, before) in &completed.entries {
            anyhow::ensure!(
                completed.directory.entry_metadata(name, false)? == *before,
                PreviewCopyError::Changed
            );
        }
        let owner = frames.last().map_or(parent, |frame| &frame.directory);
        anyhow::ensure!(
            owner.entry_metadata(&completed.leaf, false)? == completed.metadata,
            PreviewCopyError::Changed
        );
    }
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
        current.bytes = bytes;
        current.nodes.extend(next.nodes);
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
