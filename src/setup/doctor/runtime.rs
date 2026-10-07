//! Passive native status. Files and existing locks are read without creating or starting anything.

use std::{
    io::{Read, Seek},
    path::Path,
};

use serde::Serialize;

use super::{CheckState, Count, OpenedSource, SourceStamp, error_state, stability};

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
struct Last {
    state: CheckState,
    kind: Option<&'static str>,
}
impl Last {
    fn unknown(state: CheckState) -> Self {
        Self { state, kind: None }
    }
}

#[derive(Serialize)]
struct Recording {
    state: CheckState,
    failed: Option<bool>,
    class: Option<&'static str>,
    since_ns: Option<i64>,
}
impl Recording {
    fn unknown(state: CheckState) -> Self {
        Self {
            state,
            failed: None,
            class: None,
            since_ns: None,
        }
    }
}

#[derive(Serialize)]
struct Worker {
    configured_resident: Flag,
    resident_supported: bool,
    running: Flag,
    last: Last,
}

#[derive(Serialize)]
struct Viewer {
    configured_port: Count,
    running: Flag,
    last: Last,
    actual_port: Option<u16>,
}

#[derive(Serialize)]
pub(super) struct RuntimeChecks {
    pub(super) state: CheckState,
    recording: Recording,
    worker: Worker,
    resident_viewer: Viewer,
    restore_note: Flag,
}

impl RuntimeChecks {
    pub(super) fn complete(&self) -> bool {
        [
            self.state,
            self.recording.state,
            self.worker.configured_resident.state,
            self.worker.running.state,
            self.worker.last.state,
            self.resident_viewer.configured_port.state,
            self.resident_viewer.running.state,
            self.resident_viewer.last.state,
            self.restore_note.state,
        ]
        .into_iter()
        .all(CheckState::established)
    }

    pub(super) fn unhealthy(&self, codes: &mut Vec<super::UnhealthyCode>) {
        if self.recording.failed == Some(true) {
            codes.push(super::UnhealthyCode::RecordingFailed);
        }
        if matches!(self.worker.last.kind, Some("failed" | "interrupted")) {
            codes.push(super::UnhealthyCode::WorkerFailed);
        }
    }

    pub(super) fn config_changed(&mut self, state: CheckState) {
        self.worker.configured_resident = Flag::unknown(state);
        self.resident_viewer.configured_port = Count::unknown(state);
        self.resident_viewer.last = Last::unknown(state);
        self.state = stability(self.state, state);
    }
}

enum FileFact {
    Text {
        stamp: SourceStamp,
        text: std::result::Result<String, CheckState>,
    },
    Lock {
        identity: String,
        bytes: u64,
        modified: std::time::SystemTime,
        held: std::result::Result<bool, CheckState>,
    },
}
type Observed = std::result::Result<Option<FileFact>, CheckState>;
const FILES: [&str; 6] = [
    "recording-failed",
    "worker-outcome",
    "view-outcome",
    "restored",
    "worker.lock",
    "view.lock",
];

fn capture(home: &Path, name: &str, lock: bool) -> Observed {
    let path = home.join("state").join(name);
    let read = (|| -> anyhow::Result<Option<FileFact>> {
        #[cfg(unix)]
        let metadata = std::fs::symlink_metadata(&path);
        #[cfg(not(unix))]
        let metadata = super::super::diagnostic_metadata(&path);
        match metadata {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(metadata)
                if !metadata.is_file()
                    || (!lock && metadata.len() > super::super::readiness::TEXT_LIMIT) =>
            {
                return Err(std::io::Error::from(std::io::ErrorKind::Unsupported).into());
            }
            Ok(_) => {}
        }
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)?
        };
        #[cfg(not(unix))]
        let file = super::super::diagnostic_read_file(&path)?;
        if lock {
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file(),
                std::io::Error::from(std::io::ErrorKind::Unsupported)
            );
            let held = match crate::worker::try_lock_shared(&file) {
                Ok(()) => Ok(false),
                Err(std::fs::TryLockError::WouldBlock) => Ok(true),
                Err(std::fs::TryLockError::Error(error)) => Err(error_state(&error.into())),
            };
            return Ok(Some(FileFact::Lock {
                identity: crate::db::store_file_from(&file)?,
                bytes: metadata.len(),
                modified: metadata.modified()?,
                held,
            }));
        }
        let mut source =
            OpenedSource::bounded(path.clone(), file, super::super::readiness::TEXT_LIMIT)?;
        source.file.rewind()?;
        let mut bytes = Vec::new();
        (&mut source.file)
            .take(super::super::readiness::TEXT_LIMIT + 1)
            .read_to_end(&mut bytes)?;
        let metadata = source.file.metadata()?;
        anyhow::ensure!(
            bytes.len() as u64 == source.stamp.2
                && crate::forget::hash(&bytes) == source.stamp.4
                && metadata.len() == source.stamp.2
                && metadata.modified()? == source.stamp.3,
            super::PreviewCopyError::Changed
        );
        Ok(Some(FileFact::Text {
            stamp: source.stamp,
            text: String::from_utf8(bytes).map_err(|_| CheckState::Invalid),
        }))
    })();
    read.map_err(|error| error_state(&error))
}

fn text(source: &Observed) -> std::result::Result<&str, CheckState> {
    match source {
        Ok(Some(FileFact::Text { text, .. })) => text.as_deref().map_err(|state| *state),
        Ok(Some(FileFact::Lock { .. })) => Err(CheckState::Unavailable),
        Ok(None) => Err(CheckState::Absent),
        Err(state) => Err(*state),
    }
}

fn held(source: &Observed) -> Flag {
    match source {
        Ok(Some(FileFact::Lock { held, .. })) => match held {
            Ok(value) => Flag {
                state: CheckState::Known,
                value: Some(*value),
            },
            Err(state) => Flag::unknown(*state),
        },
        Ok(Some(FileFact::Text { .. })) => Flag::unknown(CheckState::Unavailable),
        Ok(None) => Flag::unknown(CheckState::Absent),
        Err(state) => Flag::unknown(*state),
    }
}

fn worker_last(source: &Observed, running: &Flag) -> Last {
    let body = match text(source) {
        Ok(text) => text,
        Err(state) => return Last::unknown(state),
    };
    let Some((_, why)) = crate::worker::parse_outcome(body) else {
        return Last::unknown(CheckState::Invalid);
    };
    let kind = if why.is_empty() {
        "clean"
    } else if why == crate::worker::STOPPED {
        match running.value {
            Some(true) => "running",
            Some(false) => "interrupted",
            None => return Last::unknown(running.state),
        }
    } else {
        "failed"
    };
    Last {
        state: CheckState::Known,
        kind: Some(kind),
    }
}

fn viewer_last(source: &Observed, running: &Flag, port: &Count) -> (Last, Option<u16>) {
    let body = match text(source) {
        Ok(text) => text.trim(),
        Err(state) => return (Last::unknown(state), None),
    };
    let actual = body
        .strip_prefix("listening ")
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0 && body == format!("listening {port}"));
    let kind = if let Some(actual) = actual {
        let Some(held) = running.value else {
            return (Last::unknown(running.state), None);
        };
        let Some(port) = port.value else {
            return (Last::unknown(port.state), held.then_some(actual));
        };
        if crate::view::resident_up(Some(body), port as u16, held) {
            "up"
        } else if held {
            "other_port"
        } else {
            "stopped"
        }
    } else {
        match body {
            "starting" => "starting",
            crate::view::PORT_IN_USE => "port_in_use",
            crate::view::NOT_PRIVATE => "not_private",
            "" => return (Last::unknown(CheckState::Invalid), None),
            _ => "failed",
        }
    };
    (
        Last {
            state: CheckState::Known,
            kind: Some(kind),
        },
        running.value.filter(|held| *held).and(actual),
    )
}

fn difference(before: &Observed, after: &Observed) -> Option<CheckState> {
    match (before, after) {
        (
            Ok(Some(FileFact::Text { stamp: before, .. })),
            Ok(Some(FileFact::Text { stamp: after, .. })),
        ) if before == after => None,
        (
            Ok(Some(FileFact::Lock {
                identity: before_id,
                bytes: before_bytes,
                modified: before_modified,
                held: before,
            })),
            Ok(Some(FileFact::Lock {
                identity: after_id,
                bytes: after_bytes,
                modified: after_modified,
                held: after,
            })),
        ) if (before_id, before_bytes, before_modified)
            == (after_id, after_bytes, after_modified) =>
        {
            match (before, after) {
                (Ok(before), Ok(after)) if before != after => Some(CheckState::Changed),
                (Err(before), Err(after)) => Some(stability(*before, *after)),
                (Err(state), _) | (_, Err(state)) => Some(*state),
                _ => None,
            }
        }
        (Ok(None), Ok(None)) => None,
        (Err(before), Err(after)) => Some(stability(*before, *after)),
        (Err(state), _) | (_, Err(state)) => Some(*state),
        _ => Some(CheckState::Changed),
    }
}

pub(super) struct Observation {
    files: [Observed; 6],
    worker: std::result::Result<bool, CheckState>,
    port: std::result::Result<u16, CheckState>,
}

impl Observation {
    pub(super) fn begin(home: &Path, config: std::result::Result<&str, CheckState>) -> Self {
        Self {
            files: std::array::from_fn(|index| capture(home, FILES[index], index >= 4)),
            worker: config.and_then(|text| {
                crate::config::parse_worker(text)
                    .map(|worker| worker.resident)
                    .map_err(|_| CheckState::Invalid)
            }),
            port: config.and_then(|text| {
                crate::config::parse_view(text)
                    .map(|view| view.port.get())
                    .map_err(|_| CheckState::Invalid)
            }),
        }
    }

    pub(super) fn checks(&self) -> RuntimeChecks {
        let mut state = CheckState::Known;
        for source in &self.files {
            if let Err(error) = source {
                state = stability(state, *error);
            }
        }
        for error in [self.worker.as_ref().err(), self.port.as_ref().err()]
            .into_iter()
            .flatten()
        {
            state = stability(state, *error);
        }
        let recording = match text(&self.files[0])
            .and_then(|text| crate::failure::parse_since(text).ok_or(CheckState::Invalid))
        {
            Ok(None) => Recording {
                state: CheckState::Known,
                failed: Some(false),
                class: None,
                since_ns: None,
            },
            Ok(Some((class, first))) => Recording {
                state: CheckState::Known,
                failed: Some(true),
                class: Some(match class {
                    crate::failure::Class::DiskFull => "disk_full",
                    crate::failure::Class::Io => "io",
                    crate::failure::Class::Busy => "busy",
                    crate::failure::Class::Other => "other",
                }),
                since_ns: Some(first),
            },
            Err(state) => Recording::unknown(state),
        };
        let configured_resident = match self.worker {
            Ok(value) => Flag {
                state: CheckState::Known,
                value: Some(value),
            },
            Err(state) => Flag::unknown(state),
        };
        let configured_port = match self.port {
            Ok(value) => Count {
                state: CheckState::Known,
                value: Some(i64::from(value)),
            },
            Err(state) => Count::unknown(state),
        };
        let worker_running = held(&self.files[4]);
        let viewer_running = held(&self.files[5]);
        let worker_last = worker_last(&self.files[1], &worker_running);
        let (viewer_last, actual_port) =
            viewer_last(&self.files[2], &viewer_running, &configured_port);
        let restore_note = match text(&self.files[3]) {
            Ok(_) => Flag {
                state: CheckState::Known,
                value: Some(true),
            },
            Err(state) => Flag::unknown(state),
        };
        RuntimeChecks {
            state,
            recording,
            worker: Worker {
                configured_resident,
                resident_supported: cfg!(target_os = "linux"),
                running: worker_running,
                last: worker_last,
            },
            resident_viewer: Viewer {
                configured_port,
                running: viewer_running,
                last: viewer_last,
                actual_port,
            },
            restore_note,
        }
    }

    pub(super) fn finish(&self, home: &Path, checks: &mut RuntimeChecks) -> Option<CheckState> {
        let mut error = None;
        for (index, before) in self.files.iter().enumerate() {
            if let Some(state) = difference(before, &capture(home, FILES[index], index >= 4)) {
                error = Some(stability(error.unwrap_or(CheckState::Known), state));
                checks.state = stability(checks.state, state);
                match index {
                    0 => checks.recording = Recording::unknown(state),
                    1 => checks.worker.last = Last::unknown(state),
                    2 => {
                        checks.resident_viewer.last = Last::unknown(state);
                        checks.resident_viewer.actual_port = None;
                    }
                    3 => checks.restore_note = Flag::unknown(state),
                    4 => {
                        checks.worker.running = Flag::unknown(state);
                        if matches!(checks.worker.last.kind, Some("running" | "interrupted")) {
                            checks.worker.last = Last::unknown(state);
                        }
                    }
                    5 => {
                        checks.resident_viewer.running = Flag::unknown(state);
                        checks.resident_viewer.last = Last::unknown(state);
                        checks.resident_viewer.actual_port = None;
                    }
                    _ => unreachable!(),
                }
            }
        }
        error
    }
}
