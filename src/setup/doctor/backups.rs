//! Native backup selection/checksum facts over admitted directory and file handles.

use std::{
    ffi::OsString,
    io::{Read, Seek},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Serialize;

use super::{
    CheckState, Count, OpenedSource, PreviewCopyError, SourceStamp, error_state, stability,
};

#[derive(Serialize)]
struct Verification {
    missing_checksum: Count,
    checksum_mismatch: Count,
}

#[derive(Serialize)]
pub(super) struct BackupChecks {
    pub(super) state: CheckState,
    record_segments: Count,
    op_segments: Count,
    pub(super) through_seq: Count,
    pub(super) through_op_seq: Count,
    verification: Verification,
}

impl BackupChecks {
    pub(super) fn complete(&self) -> bool {
        [
            self.state,
            self.record_segments.state,
            self.op_segments.state,
            self.through_seq.state,
            self.through_op_seq.state,
            self.verification.missing_checksum.state,
            self.verification.checksum_mismatch.state,
        ]
        .into_iter()
        .all(CheckState::established)
    }

    pub(super) fn unhealthy(&self, codes: &mut Vec<super::UnhealthyCode>) {
        for (count, code) in [
            (
                &self.verification.missing_checksum,
                super::UnhealthyCode::BackupChecksumMissing,
            ),
            (
                &self.verification.checksum_mismatch,
                super::UnhealthyCode::BackupChecksumMismatch,
            ),
        ] {
            if matches!(count.state, CheckState::Known)
                && count.value.is_some_and(|value| value > 0)
            {
                codes.push(code);
            }
        }
    }

    pub(super) fn unknown(state: CheckState) -> Self {
        Self {
            state,
            record_segments: Count::unknown(state),
            op_segments: Count::unknown(state),
            through_seq: Count::unknown(state),
            through_op_seq: Count::unknown(state),
            verification: Verification {
                missing_checksum: Count::unknown(state),
                checksum_mismatch: Count::unknown(state),
            },
        }
    }
}

#[derive(PartialEq, Eq)]
struct Member {
    name: OsString,
    ops: bool,
    device: String,
    first: i64,
    last: i64,
}

#[derive(PartialEq, Eq)]
struct Manifest {
    identity: String,
    resolved: PathBuf,
    members: Vec<Member>,
}

struct Scan {
    manifest: Option<Manifest>,
    stamps: std::result::Result<Vec<SourceStamp>, CheckState>,
    missing: i64,
    mismatch: i64,
}

fn segment(
    directory: &super::super::DiagnosticDirectory,
    name: &std::ffi::OsStr,
) -> Result<(Vec<SourceStamp>, bool, bool)> {
    let source = OpenedSource::new(
        directory.resolved_path().join(name),
        directory.open_file(name)?,
    )?;
    let mut sum_name = name.to_owned();
    sum_name.push(".sha256");
    let sum_file = match directory.open_file(&sum_name) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((vec![source.stamp], true, false));
        }
        Err(error) => return Err(error.into()),
    };
    let mut sum = OpenedSource::bounded(
        directory.resolved_path().join(sum_name),
        sum_file,
        super::super::readiness::TEXT_LIMIT,
    )?;
    sum.file.rewind()?;
    let mut text = String::new();
    (&mut sum.file)
        .take(super::super::readiness::TEXT_LIMIT + 1)
        .read_to_string(&mut text)?;
    let metadata = sum.file.metadata()?;
    anyhow::ensure!(
        text.len() as u64 == sum.stamp.2
            && crate::forget::hash(text.as_bytes()) == sum.stamp.4
            && metadata.len() == sum.stamp.2
            && metadata.modified()? == sum.stamp.3,
        PreviewCopyError::Changed
    );
    let mismatch = !crate::backup::checksum_matches(&text, &source.stamp.4);
    Ok((vec![source.stamp, sum.stamp], false, mismatch))
}

fn scan(path: &Path) -> Result<Scan> {
    let mut directory = match super::super::DiagnosticDirectory::open(path) {
        Ok(directory) => directory,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(Scan {
                manifest: None,
                stamps: Ok(Vec::new()),
                missing: 0,
                mismatch: 0,
            });
        }
        Err(error) => return Err(error),
    };
    let mut members: Vec<_> = directory
        .names()?
        .into_iter()
        .filter_map(|name| {
            let (ops, device, first, last) = crate::backup::segment_name(&name)?;
            Some(Member {
                name,
                ops,
                device,
                first,
                last,
            })
        })
        .collect();
    members.sort_by(|a, b| {
        (a.ops, &a.device, a.first, &a.name).cmp(&(b.ops, &b.device, b.first, &b.name))
    });
    let manifest = Manifest {
        identity: directory.identity()?,
        resolved: directory.resolved_path().to_owned(),
        members,
    };
    let mut stamps = Vec::new();
    let (mut missing, mut mismatch, mut failure) = (0i64, 0i64, CheckState::Known);
    for member in &manifest.members {
        match segment(&directory, &member.name) {
            Ok((files, no_sum, bad_sum)) => {
                stamps.extend(files);
                missing = missing
                    .checked_add(i64::from(no_sum))
                    .context("backup count overflow")?;
                mismatch = mismatch
                    .checked_add(i64::from(bad_sum))
                    .context("backup count overflow")?;
            }
            Err(error) => failure = stability(failure, error_state(&error)),
        }
    }
    // Repeating the same admitted directory scan also catches changes during file hashing.
    let mut after: Vec<_> = directory
        .names()?
        .into_iter()
        .filter_map(|name| crate::backup::segment_name(&name).map(|_| name))
        .collect();
    let mut before: Vec<_> = manifest
        .members
        .iter()
        .map(|member| member.name.clone())
        .collect();
    after.sort();
    before.sort();
    anyhow::ensure!(before == after, PreviewCopyError::Changed);
    Ok(Scan {
        manifest: Some(manifest),
        stamps: if matches!(failure, CheckState::Known) {
            Ok(stamps)
        } else {
            Err(failure)
        },
        missing,
        mismatch,
    })
}

pub(super) struct Observation {
    path: std::result::Result<PathBuf, CheckState>,
    before: std::result::Result<Scan, CheckState>,
}

fn known(value: i64) -> Count {
    Count {
        state: CheckState::Known,
        value: Some(value),
    }
}

impl Observation {
    pub(super) fn begin(home: &Path, text: std::result::Result<&str, CheckState>) -> Self {
        let path = text.and_then(|text| {
            crate::backup::location(text)
                .map(|path| home.join(path.unwrap_or_else(|| PathBuf::from("backups"))))
                .map_err(|_| CheckState::Invalid)
        });
        let before = path
            .as_ref()
            .map_err(|state| *state)
            .and_then(|path| scan(path).map_err(|error| error_state(&error)));
        Self { path, before }
    }

    pub(super) fn checks(&self, device: std::result::Result<&str, CheckState>) -> BackupChecks {
        let scan = match &self.before {
            Ok(scan) => scan,
            Err(state) => return BackupChecks::unknown(*state),
        };
        let members = scan
            .manifest
            .as_ref()
            .map_or(&[][..], |manifest| manifest.members.as_slice());
        let records = members.iter().filter(|member| !member.ops).count();
        let ops = members.len() - records;
        let counts = (i64::try_from(records), i64::try_from(ops));
        let (records, ops) = match counts {
            (Ok(records), Ok(ops)) => (records, ops),
            _ => return BackupChecks::unknown(CheckState::Unavailable),
        };
        let through = |ops| match device {
            Ok(device) => known(
                members
                    .iter()
                    .filter(|member| member.ops == ops && member.device == device)
                    .map(|member| member.last)
                    .max()
                    .unwrap_or(0),
            ),
            Err(state) => Count::unknown(state),
        };
        let verification = match &scan.stamps {
            Ok(_) => Verification {
                missing_checksum: known(scan.missing),
                checksum_mismatch: known(scan.mismatch),
            },
            Err(state) => Verification {
                missing_checksum: Count::unknown(*state),
                checksum_mismatch: Count::unknown(*state),
            },
        };
        BackupChecks {
            state: scan
                .stamps
                .as_ref()
                .err()
                .copied()
                .unwrap_or(CheckState::Known),
            record_segments: known(records),
            op_segments: known(ops),
            through_seq: through(false),
            through_op_seq: through(true),
            verification,
        }
    }

    pub(super) fn finish(&self, checks: &mut BackupChecks) -> Option<CheckState> {
        let (state, same_manifest) = match (&self.before, &self.path) {
            (Ok(before), Ok(path)) => match scan(path) {
                Ok(after) if before.manifest != after.manifest => {
                    (Some(CheckState::Changed), false)
                }
                Ok(after) => (
                    match (&before.stamps, &after.stamps) {
                        (Ok(before), Ok(after)) if before != after => Some(CheckState::Changed),
                        (Err(before), Err(after)) => Some(stability(*before, *after)),
                        (Err(state), _) | (_, Err(state)) => Some(*state),
                        _ => None,
                    },
                    true,
                ),
                Err(error) => (Some(error_state(&error)), false),
            },
            (Err(state), _) | (_, Err(state)) => (Some(*state), false),
        };
        if let Some(state) = state {
            if matches!(state, CheckState::Changed) || !same_manifest {
                *checks = BackupChecks::unknown(state);
            } else {
                checks.state = state;
                checks.verification = Verification {
                    missing_checksum: Count::unknown(state),
                    checksum_mismatch: Count::unknown(state),
                };
            }
        }
        state
    }
}
