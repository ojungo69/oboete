//! Explicit readonly diagnostics, without store initialization or repair.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use super::readiness::Readiness;
use crate::migrate::{OpenedSource, PreviewCopyError, SourceStamp};

mod ledger;
use ledger::LedgerChecks;
mod backups;
use backups::BackupChecks;
mod runtime;
use runtime::RuntimeChecks;
mod retained;
use retained::RetainedChecks;

#[cfg(test)]
thread_local! {
    static BEFORE_CONFIG: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_COPY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_INVENTORY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_COLLECTION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[derive(Serialize)]
pub(crate) struct DoctorReport {
    complete: bool,
    unhealthy: Vec<UnhealthyCode>,
    inventory: Readiness,
    stores: Stores,
    checks: Checks,
}

#[derive(Serialize)]
struct Checks {
    raw: RawChecks,
    knowledge: KnowledgeChecks,
    legacy: LegacyChecks,
    embeddings: EmbeddingChecks,
    providers: LedgerChecks,
    backups: BackupChecks,
    runtime: RuntimeChecks,
    disk: DiskChecks,
    retained: RetainedChecks,
    source_stability: CheckState,
    remaining: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum UnhealthyCode {
    RecordingFailed,
    LowFreeSpace,
    RawDamaged,
    KnowledgeDamaged,
    LegacyDamaged,
    ProvidersDamaged,
    BackupChecksumMissing,
    BackupChecksumMismatch,
    WorkerFailed,
    ConfigInvalid,
}

impl CheckState {
    fn established(self) -> bool {
        matches!(self, Self::Known | Self::Absent | Self::Off)
    }
}

impl Checks {
    fn complete(&self) -> bool {
        let raw = matches!(self.raw.integrity, CheckState::Absent)
            || [
                self.raw.integrity,
                self.raw.max_seq.state,
                self.raw.max_op_seq.state,
                self.raw.curated_through.state,
                self.raw.parking.state,
            ]
            .into_iter()
            .all(|state| matches!(state, CheckState::Known));
        let knowledge =
            matches!(self.knowledge.integrity, CheckState::Absent)
                || [
                    self.knowledge.integrity,
                    self.knowledge.rewinds.count.state,
                    self.knowledge.gaps.state,
                ]
                .into_iter()
                .all(|state| matches!(state, CheckState::Known))
                    && self.knowledge.rewinds.count.value.is_some_and(|count| {
                        count == 0 || self.knowledge.rewinds.last_at_ms.is_some()
                    });
        let legacy = matches!(self.legacy.integrity, CheckState::Absent)
            || [
                self.legacy.integrity,
                self.legacy.sessions.state,
                self.legacy.events.state,
                self.legacy.observations.state,
                self.legacy.summaries.state,
                self.legacy.remaining_events.state,
            ]
            .into_iter()
            .all(|state| matches!(state, CheckState::Known))
                && self.legacy.recent.complete();
        let embedding = matches!(self.embeddings.state, CheckState::Off | CheckState::Absent)
            || [
                self.embeddings.state,
                self.embeddings.generation.state,
                self.embeddings.waiting.claims.state,
                self.embeddings.waiting.imports.state,
                self.embeddings.waiting.records.state,
                self.embeddings.skipped.state,
            ]
            .into_iter()
            .all(|state| matches!(state, CheckState::Known));
        raw && knowledge
            && legacy
            && embedding
            && self.providers.complete()
            && self.backups.complete()
            && self.runtime.complete()
            && self.disk.state.established()
            && matches!(self.retained.state, CheckState::Known)
            && matches!(self.source_stability, CheckState::Known)
    }

    fn unhealthy(&self) -> Vec<UnhealthyCode> {
        let mut codes = Vec::new();
        self.runtime.unhealthy(&mut codes);
        if self.disk.low_space == Some(true) {
            codes.push(UnhealthyCode::LowFreeSpace);
        }
        for (state, code) in [
            (self.raw.integrity, UnhealthyCode::RawDamaged),
            (self.knowledge.integrity, UnhealthyCode::KnowledgeDamaged),
            (self.legacy.integrity, UnhealthyCode::LegacyDamaged),
            (self.providers.integrity, UnhealthyCode::ProvidersDamaged),
        ] {
            if matches!(state, CheckState::Damaged) {
                codes.push(code);
            }
        }
        self.backups.unhealthy(&mut codes);
        codes
    }
}

#[derive(Clone, Copy)]
enum Database {
    Raw,
    Knowledge,
    Legacy,
    Providers,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckState {
    Known,
    Off,
    Invalid,
    Absent,
    SchemaMissing,
    Damaged,
    Unreadable,
    Busy,
    Changed,
    Unavailable,
}

#[derive(Serialize)]
struct DiskChecks {
    state: CheckState,
    free_bytes: Option<u64>,
    low_space: Option<bool>,
}

impl DiskChecks {
    fn from_bytes(value: Option<u64>) -> Self {
        Self {
            state: if value.is_some() {
                CheckState::Known
            } else {
                CheckState::Unavailable
            },
            free_bytes: value,
            low_space: value.map(|bytes| bytes < super::LOW_FREE_BYTES),
        }
    }

    fn unknown(state: CheckState) -> Self {
        Self {
            state,
            free_bytes: None,
            low_space: None,
        }
    }
}

struct DiskObservation(Result<(super::DiagnosticDirectory, String), CheckState>);

impl DiskObservation {
    fn begin(home: &Path) -> Self {
        if matches!(super::diagnostic_metadata(home), Err(error) if error.kind() == ErrorKind::NotFound)
        {
            return Self(Err(CheckState::Absent));
        }
        Self(
            (|| -> Result<_> {
                let directory = super::DiagnosticDirectory::open(home)?;
                let identity = directory.identity()?;
                Ok((directory, identity))
            })()
            .map_err(|error| error_state(&error)),
        )
    }

    fn checks(&self) -> DiskChecks {
        match &self.0 {
            Ok((directory, _)) => DiskChecks::from_bytes(directory.available_bytes().ok()),
            Err(state) => DiskChecks::unknown(*state),
        }
    }

    fn finish(&self, home: &Path, checks: &mut DiskChecks) -> Option<CheckState> {
        let (_, before) = match &self.0 {
            Ok(value) => value,
            Err(state) => return matches!(state, CheckState::Changed).then_some(*state),
        };
        let after =
            super::DiagnosticDirectory::open(home).and_then(|directory| Ok(directory.identity()?));
        let state = match after {
            Ok(after) if before == &after => return None,
            Ok(_) => CheckState::Changed,
            Err(error) => error_state(&error),
        };
        *checks = DiskChecks::unknown(state);
        Some(state)
    }
}

#[derive(Serialize)]
struct Count {
    state: CheckState,
    value: Option<i64>,
}

impl Count {
    fn unknown(state: CheckState) -> Self {
        Self { state, value: None }
    }
}

#[derive(Serialize)]
struct RawChecks {
    #[serde(skip)]
    device: Option<String>,
    integrity: CheckState,
    max_seq: Count,
    max_op_seq: Count,
    curated_through: Count,
    parking: ParkingChecks,
}

impl RawChecks {
    fn unknown(state: CheckState) -> Self {
        Self {
            device: None,
            integrity: state,
            max_seq: Count::unknown(state),
            max_op_seq: Count::unknown(state),
            curated_through: Count::unknown(state),
            parking: ParkingChecks { state, rows: None },
        }
    }
}

#[derive(Serialize)]
struct ParkingRow {
    source: &'static str,
    parked: i64,
    waiting: i64,
}

#[derive(Serialize)]
struct ParkingChecks {
    state: CheckState,
    rows: Option<Vec<ParkingRow>>,
}

fn parking_checks(conn: &Connection, device: &str) -> ParkingChecks {
    let result = (|| -> Result<Option<Vec<ParkingRow>>> {
        if !has_columns(
            conn,
            "ops",
            &["device", "op_seq", "type", "ts", "body", "batch"],
        )? || !has_columns(
            conn,
            "records",
            &["device", "seq", "type", "kind", "source"],
        )? {
            return Ok(None);
        }
        let (parked, waiting) = crate::curate::parked_counts_in(conn, device)?;
        let mut rows: Vec<_> = ["oboete_v1", "transcript", "other"]
            .into_iter()
            .map(|source| ParkingRow {
                source,
                parked: 0,
                waiting: 0,
            })
            .collect();
        for (counts, is_parked) in [(parked, true), (waiting, false)] {
            for (source, count) in counts {
                let row = &mut rows[match source.as_str() {
                    "oboete-v1" => 0,
                    "transcript" => 1,
                    _ => 2,
                }];
                let value = if is_parked {
                    &mut row.parked
                } else {
                    &mut row.waiting
                };
                *value = value.checked_add(count).context("parking count overflow")?;
            }
        }
        Ok(Some(rows))
    })();
    match result {
        Ok(Some(rows)) => ParkingChecks {
            state: CheckState::Known,
            rows: Some(rows),
        },
        Ok(None) => ParkingChecks {
            state: CheckState::SchemaMissing,
            rows: None,
        },
        Err(error) => ParkingChecks {
            state: error_state(&error),
            rows: None,
        },
    }
}

fn error_state(error: &anyhow::Error) -> CheckState {
    if let Some(copy) = error.downcast_ref::<PreviewCopyError>() {
        return match copy {
            PreviewCopyError::Changed => CheckState::Changed,
            PreviewCopyError::Cleanup => CheckState::Unavailable,
        };
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        return match error.kind() {
            ErrorKind::NotFound => CheckState::Changed,
            ErrorKind::Unsupported => CheckState::Unavailable,
            _ => CheckState::Unreadable,
        };
    }
    if let Some(rusqlite::Error::SqliteFailure(error, _)) = error.downcast_ref::<rusqlite::Error>()
    {
        return match error.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                CheckState::Busy
            }
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                CheckState::Damaged
            }
            _ => CheckState::Unreadable,
        };
    }
    CheckState::Unavailable
}

fn opened(path: &Path) -> Result<OpenedSource> {
    opened_with_limit(path, u64::MAX)
}

fn opened_with_limit(path: &Path, limit: u64) -> Result<OpenedSource> {
    let metadata = super::diagnostic_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(std::io::Error::from(ErrorKind::Unsupported).into());
    }
    let file = super::diagnostic_read_file(path)?;
    #[cfg(windows)]
    let resolved = super::diagnostic_file_name(&file)?;
    #[cfg(not(windows))]
    let resolved = path.canonicalize()?;
    let binding = super::diagnostic_read_file(&resolved)?;
    if crate::db::store_file_from(&file)? != crate::db::store_file_from(&binding)? {
        return Err(PreviewCopyError::Changed.into());
    }
    OpenedSource::bounded(resolved, file, limit)
}

fn raw_path(home: &Path) -> Result<Option<PathBuf>> {
    for name in ["raw.db", "raw.db.restored"] {
        let path = home.join(name);
        match super::diagnostic_metadata(&path) {
            Ok(_) => return Ok(Some(path)),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

fn database_path(home: &Path, database: Database) -> Result<Option<PathBuf>> {
    let path = match database {
        Database::Raw => return raw_path(home),
        Database::Knowledge => home.join("knowledge.db"),
        Database::Legacy => home.join("oboete.db"),
        Database::Providers => home.join("providers.db"),
    };
    match super::diagnostic_metadata(&path) {
        Ok(_) => Ok(Some(path)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn sources(
    home: &Path,
    database: Database,
) -> Result<Option<(OpenedSource, Option<OpenedSource>)>> {
    let Some(path) = database_path(home, database)? else {
        return Ok(None);
    };
    let db = opened(&path)?;
    let mut wal = db.stamp.0.as_os_str().to_owned();
    wal.push("-wal");
    let wal = match opened(&PathBuf::from(wal)) {
        Ok(wal) => Some(wal),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    Ok(Some((db, wal)))
}

fn stamps(db: &OpenedSource, wal: Option<&OpenedSource>) -> Vec<SourceStamp> {
    std::iter::once(db)
        .chain(wal)
        .map(|source| source.stamp.clone())
        .collect()
}

fn current(home: &Path, database: Database, before: &[SourceStamp]) -> Result<()> {
    if observed(home, database)? != before {
        return Err(PreviewCopyError::Changed.into());
    }
    Ok(())
}

fn observed(home: &Path, database: Database) -> Result<Vec<SourceStamp>> {
    Ok(match sources(home, database)? {
        Some((db, wal)) => stamps(&db, wal.as_ref()),
        None => Vec::new(),
    })
}

fn config_stamp(home: &Path) -> Result<Vec<SourceStamp>> {
    let path = home.join("config.toml");
    match super::diagnostic_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    Ok(vec![
        opened_with_limit(&path, super::readiness::TEXT_LIMIT)?.stamp,
    ])
}

fn has_columns(conn: &Connection, table: &str, columns: &[&str]) -> Result<bool> {
    if !crate::consumer::manifest::exists(conn, "table", table)?
        && !crate::consumer::manifest::exists(conn, "view", table)?
    {
        return Ok(false);
    }
    let mut statement = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let found = statement
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
    Ok(columns.iter().all(|column| found.contains(*column)))
}

fn config_text(
    home: &Path,
    observation: &Result<Vec<SourceStamp>>,
) -> std::result::Result<String, CheckState> {
    let before = observation.as_ref().map_err(error_state)?;
    let text = super::readiness::text(&home.join("config.toml")).map_err(|state| match state {
        super::readiness::State::Invalid => CheckState::Invalid,
        super::readiness::State::Unavailable => CheckState::Unavailable,
        _ => CheckState::Unreadable,
    })?;
    match (before.as_slice(), text) {
        ([], None) => Ok(String::new()),
        ([stamp], Some(text)) if crate::forget::hash(text.as_bytes()) == stamp.4 => Ok(text),
        _ => Err(CheckState::Changed),
    }
}

#[derive(Serialize)]
struct Generation {
    state: CheckState,
    value: Option<&'static str>,
}

#[derive(Serialize)]
struct Waiting {
    claims: Count,
    cards: Count,
    summaries: Count,
    imports: Count,
    records: Count,
}

#[derive(Serialize)]
struct SkippedRow {
    reason: &'static str,
    count: i64,
}

#[derive(Serialize)]
struct Skipped {
    state: CheckState,
    rows: Option<Vec<SkippedRow>>,
}

#[derive(Serialize)]
struct EmbeddingChecks {
    state: CheckState,
    generation: Generation,
    waiting: Waiting,
    skipped: Skipped,
}

impl EmbeddingChecks {
    fn unknown(state: CheckState) -> Self {
        Self {
            state,
            generation: Generation { state, value: None },
            waiting: Waiting {
                claims: Count::unknown(state),
                cards: Count::unknown(state),
                summaries: Count::unknown(state),
                imports: Count::unknown(state),
                records: Count::unknown(state),
            },
            skipped: Skipped { state, rows: None },
        }
    }
}

fn embedding_queries(conn: &Connection) -> EmbeddingChecks {
    let facts = crate::embed_phase::doctor_facts_in(conn, crate::embed::EMBEDDER);
    let generation = match has_columns(conn, "vec_generation", &["embedder", "state"]) {
        Ok(false) => Generation {
            state: CheckState::SchemaMissing,
            value: None,
        },
        Err(error) => Generation {
            state: error_state(&error),
            value: None,
        },
        Ok(true) => match facts.generation {
            Ok(value) => Generation {
                state: CheckState::Known,
                value: value.as_deref().map(|value| match value {
                    "active" => "active",
                    _ => "other",
                }),
            },
            Err(error) => Generation {
                state: error_state(&error),
                value: None,
            },
        },
    };
    let vectors = has_columns(conn, "vector_keys", &["embedder", "kind", "key"]);
    let waiting = match vectors {
        Ok(true) => Waiting {
            claims: count(conn, "active", &["uid"], || facts.claims),
            cards: count(
                conn,
                "cards",
                &["device", "op_seq", "n", "replaced_by"],
                || facts.cards,
            ),
            summaries: count(conn, "turns", &["device", "op_seq", "skipped"], || {
                facts.summaries
            }),
            imports: count(conn, "imported", &["uid"], || facts.imports),
            records: count(conn, "raw_docs", &["device", "seq"], || facts.records),
        },
        other => {
            let state = other
                .err()
                .map_or(CheckState::SchemaMissing, |error| error_state(&error));
            Waiting {
                claims: Count::unknown(state),
                cards: Count::unknown(state),
                summaries: Count::unknown(state),
                imports: Count::unknown(state),
                records: Count::unknown(state),
            }
        }
    };
    let skipped = (|| -> Result<Option<Vec<SkippedRow>>> {
        if !has_columns(conn, "vector_keys", &["embedder", "skipped"])? {
            return Ok(None);
        }
        let mut rows: Vec<_> = ["excluded", "held", "refused", "empty", "other"]
            .into_iter()
            .map(|reason| SkippedRow { reason, count: 0 })
            .collect();
        for (reason, count) in facts.skipped? {
            let index = rows
                .iter()
                .position(|row| row.reason == reason)
                .unwrap_or(4);
            rows[index].count = rows[index]
                .count
                .checked_add(count)
                .context("skipped count overflow")?;
        }
        Ok(Some(rows))
    })();
    let skipped = match skipped {
        Ok(Some(rows)) => Skipped {
            state: CheckState::Known,
            rows: Some(rows),
        },
        Ok(None) => Skipped {
            state: CheckState::SchemaMissing,
            rows: None,
        },
        Err(error) => Skipped {
            state: error_state(&error),
            rows: None,
        },
    };
    EmbeddingChecks {
        state: CheckState::Known,
        generation,
        waiting,
        skipped,
    }
}

fn count(
    conn: &Connection,
    table: &str,
    columns: &[&str],
    read: impl FnOnce() -> Result<i64>,
) -> Count {
    match has_columns(conn, table, columns) {
        Ok(false) => Count::unknown(CheckState::SchemaMissing),
        Err(error) => Count::unknown(error_state(&error)),
        Ok(true) => match read() {
            Ok(value) if value >= 0 => Count {
                state: CheckState::Known,
                value: Some(value),
            },
            Ok(_) => Count::unknown(CheckState::Unavailable),
            Err(error) => Count::unknown(error_state(&error)),
        },
    }
}

fn raw_queries(conn: &Connection) -> RawChecks {
    let integrity = match crate::raw::integrity_check_in(conn) {
        Ok(()) => CheckState::Known,
        Err(error) if error.downcast_ref::<rusqlite::Error>().is_none() => CheckState::Damaged,
        Err(error) => error_state(&error),
    };
    if !matches!(integrity, CheckState::Known) {
        return RawChecks::unknown(integrity);
    }
    let device = (|| -> Result<Option<String>> {
        if !has_columns(conn, "meta", &["key", "value"])? {
            return Ok(None);
        }
        crate::db::device_id(conn).map(Some)
    })();
    let device = match device {
        Ok(Some(device)) => device,
        other => {
            let state = match other {
                Ok(None) => CheckState::SchemaMissing,
                Err(error) => error_state(&error),
                Ok(Some(_)) => unreachable!(),
            };
            let mut result = RawChecks::unknown(state);
            result.integrity = integrity;
            return result;
        }
    };
    RawChecks {
        integrity,
        max_seq: count(conn, "records", &["device", "seq"], || {
            crate::raw::max_seq_in(conn, &device)
        }),
        max_op_seq: count(conn, "ops", &["device", "op_seq"], || {
            crate::raw::max_op_seq_in(conn, &device)
        }),
        curated_through: count(conn, "ops", &["device", "op_seq", "type", "body"], || {
            crate::curate::curated_through_in(conn, &device)
        }),
        parking: parking_checks(conn, &device),
        device: Some(device),
    }
}

fn copy_checks<T>(
    home: &Path,
    database: Database,
    observation: &Result<Vec<SourceStamp>>,
    inspect: impl FnOnce(&Connection) -> T,
) -> std::result::Result<T, CheckState> {
    let before = observation.as_ref().map_err(error_state)?;
    let result = (|| -> Result<Option<T>> {
        let Some((db, wal)) = sources(home, database)? else {
            if !before.is_empty() {
                return Err(PreviewCopyError::Changed.into());
            }
            return Ok(None);
        };
        if stamps(&db, wal.as_ref()) != *before {
            return Err(PreviewCopyError::Changed.into());
        }
        let source_parent = db.stamp.0.parent().context("database has no parent")?;
        anyhow::ensure!(
            !std::env::temp_dir()
                .canonicalize()?
                .starts_with(source_parent),
            std::io::Error::from(ErrorKind::Unsupported)
        );
        if matches!(database, Database::Knowledge) {
            crate::db::register_sqlite_vec();
        }
        let checks = crate::migrate::with_preview_files(db, wal, |conn| {
            current(home, database, before)?;
            Ok(inspect(conn))
        })?;
        current(home, database, before)?;
        Ok(Some(checks))
    })();
    match result {
        Ok(Some(checks)) => Ok(checks),
        Ok(None) => Err(CheckState::Absent),
        Err(error) => Err(error_state(&error)),
    }
}

fn recheck(
    home: &Path,
    database: Database,
    before: &Result<Vec<SourceStamp>>,
) -> Option<CheckState> {
    match before {
        Ok(before) => current(home, database, before)
            .err()
            .map(|error| error_state(&error)),
        Err(error) => Some(error_state(error)),
    }
}

fn stability(previous: CheckState, next: CheckState) -> CheckState {
    if matches!(previous, CheckState::Changed) || matches!(next, CheckState::Changed) {
        CheckState::Changed
    } else if matches!(previous, CheckState::Known) {
        next
    } else {
        previous
    }
}

#[derive(Serialize)]
struct Rewinds {
    count: Count,
    last_at_ms: Option<i64>,
}

#[derive(Serialize)]
struct GapCounts {
    agent: &'static str,
    ended: i64,
    checked: i64,
    short: i64,
    missing: i64,
}

#[derive(Serialize)]
struct GapChecks {
    state: CheckState,
    rows: Option<Vec<GapCounts>>,
}

#[derive(Serialize)]
struct KnowledgeChecks {
    integrity: CheckState,
    rewinds: Rewinds,
    gaps: GapChecks,
}

impl KnowledgeChecks {
    fn unknown(state: CheckState) -> Self {
        Self {
            integrity: state,
            rewinds: Rewinds {
                count: Count::unknown(state),
                last_at_ms: None,
            },
            gaps: GapChecks { state, rows: None },
        }
    }
}

fn rewind_checks(conn: &Connection) -> Rewinds {
    let result = (|| -> Result<Option<(i64, Option<i64>)>> {
        if !has_columns(conn, "rewinds", &["ts"])? {
            return Ok(None);
        }
        let (count, last_at, _) = crate::knowledge::rewind_facts(conn)?;
        Ok(Some((count, last_at)))
    })();
    match result {
        Ok(Some((value, last_at_ms))) => Rewinds {
            count: Count {
                state: CheckState::Known,
                value: Some(value),
            },
            last_at_ms,
        },
        Ok(None) => Rewinds {
            count: Count::unknown(CheckState::SchemaMissing),
            last_at_ms: None,
        },
        Err(error) => Rewinds {
            count: Count::unknown(error_state(&error)),
            last_at_ms: None,
        },
    }
}

fn gap_checks(conn: &Connection) -> GapChecks {
    let result = (|| -> Result<Option<Vec<GapCounts>>> {
        if !has_columns(conn, "gaps", &["agent", "transcript_turns", "raw_turns"])? {
            return Ok(None);
        }
        let mut rows: Vec<_> = super::AGENTS
            .iter()
            .copied()
            .chain(["other"])
            .map(|agent| GapCounts {
                agent,
                ended: 0,
                checked: 0,
                short: 0,
                missing: 0,
            })
            .collect();
        for (agent, ended, checked, short, missing) in crate::consumer::gaps::doctor_rows(conn)? {
            let index = super::AGENTS
                .iter()
                .position(|known| *known == agent)
                .unwrap_or(7);
            let row = &mut rows[index];
            for (target, value) in [
                (&mut row.ended, ended),
                (&mut row.checked, checked),
                (&mut row.short, short),
                (&mut row.missing, missing),
            ] {
                *target = target
                    .checked_add(value)
                    .context("gap aggregate overflow")?;
            }
        }
        Ok(Some(rows))
    })();
    match result {
        Ok(Some(rows)) => GapChecks {
            state: CheckState::Known,
            rows: Some(rows),
        },
        Ok(None) => GapChecks {
            state: CheckState::SchemaMissing,
            rows: None,
        },
        Err(error) => GapChecks {
            state: error_state(&error),
            rows: None,
        },
    }
}

fn knowledge_queries(conn: &Connection) -> KnowledgeChecks {
    let integrity = match crate::db::quick_check(conn, "knowledge.db") {
        Ok(()) => CheckState::Known,
        Err(error) if error.downcast_ref::<rusqlite::Error>().is_none() => CheckState::Damaged,
        Err(error) => error_state(&error),
    };
    if !matches!(integrity, CheckState::Known) {
        return KnowledgeChecks::unknown(integrity);
    }
    KnowledgeChecks {
        integrity,
        rewinds: rewind_checks(conn),
        gaps: gap_checks(conn),
    }
}

#[derive(Serialize)]
struct CallSummary {
    outcome: &'static str,
    ms: Option<i64>,
}

#[derive(Serialize)]
struct RecentCalls {
    state: CheckState,
    calls: Option<Vec<CallSummary>>,
}

impl RecentCalls {
    fn complete(&self) -> bool {
        matches!(self.state, CheckState::Known)
            && self
                .calls
                .as_ref()
                .is_some_and(|calls| calls.iter().all(|call| call.ms.is_some()))
    }
}

#[derive(Serialize)]
struct LegacyChecks {
    integrity: CheckState,
    sessions: Count,
    events: Count,
    observations: Count,
    summaries: Count,
    recent: RecentCalls,
    remaining_events: Count,
    #[serde(skip)]
    remaining_dependency: bool,
}

impl LegacyChecks {
    fn unknown(state: CheckState) -> Self {
        Self {
            integrity: state,
            sessions: Count::unknown(state),
            events: Count::unknown(state),
            observations: Count::unknown(state),
            summaries: Count::unknown(state),
            recent: RecentCalls { state, calls: None },
            remaining_events: Count::unknown(state),
            remaining_dependency: false,
        }
    }
}

fn recent_calls(conn: &Connection) -> RecentCalls {
    let result = (|| -> Result<Option<Vec<CallSummary>>> {
        if !has_columns(
            conn,
            "provider_calls",
            &["id", "provider", "outcome", "ms", "detail"],
        )? {
            return Ok(None);
        }
        Ok(Some(
            crate::migrate::legacy_call_rows_in(conn)?
                .into_iter()
                .map(|row| CallSummary {
                    outcome: call_outcome(&row.outcome),
                    ms: (row.ms >= 0).then_some(row.ms),
                })
                .collect(),
        ))
    })();
    match result {
        Ok(Some(calls)) => RecentCalls {
            state: CheckState::Known,
            calls: Some(calls),
        },
        Ok(None) => RecentCalls {
            state: CheckState::SchemaMissing,
            calls: None,
        },
        Err(error) => RecentCalls {
            state: error_state(&error),
            calls: None,
        },
    }
}

fn call_outcome(outcome: &str) -> &'static str {
    match outcome {
        "ok" => "ok",
        "error" => "error",
        "invalid" => "invalid",
        "wait" => "wait",
        "empty" => "empty",
        "timeout" => "timeout",
        "sent" => "sent",
        "reserved" => "reserved",
        "prose" => "prose",
        "shape" => "shape",
        "over_cap" => "over_cap",
        "unanchored" => "unanchored",
        "budget" => "budget",
        "gate" => "gate",
        "too_big" => "too_big",
        _ => "other",
    }
}

fn legacy_through(
    raw: std::result::Result<Option<&Connection>, CheckState>,
    key: &str,
) -> std::result::Result<i64, CheckState> {
    let Some(raw) = raw? else {
        return Ok(0);
    };
    if !has_columns(raw, "ops", &["op_seq", "type", "body"]).map_err(|error| error_state(&error))? {
        return Err(CheckState::SchemaMissing);
    }
    let mut checkpoints =
        crate::raw::migration_checkpoints_in(raw, key).map_err(|error| error_state(&error))?;
    Ok(checkpoints
        .remove(key)
        .map_or(0, |checkpoint| checkpoint.through))
}

fn remaining_events(
    conn: &Connection,
    raw: std::result::Result<Option<&Connection>, CheckState>,
) -> (Count, bool) {
    let key = match has_columns(conn, "meta", &["key", "value"]) {
        Ok(false) => return (Count::unknown(CheckState::SchemaMissing), false),
        Err(error) => return (Count::unknown(error_state(&error)), false),
        Ok(true) => match crate::migrate::legacy_key_in(conn) {
            Ok(Some(key)) => key,
            Ok(None) => return (Count::unknown(CheckState::Unavailable), false),
            Err(error) => return (Count::unknown(error_state(&error)), false),
        },
    };
    match has_columns(conn, "events", &["id"]) {
        Ok(false) => return (Count::unknown(CheckState::SchemaMissing), false),
        Err(error) => return (Count::unknown(error_state(&error)), false),
        Ok(true) => {}
    }
    let through = match legacy_through(raw, &key) {
        Ok(through) => through,
        Err(state) => return (Count::unknown(state), true),
    };
    let count = match crate::migrate::legacy_remaining_in(conn, through) {
        Ok(value) => Count {
            state: CheckState::Known,
            value: Some(value),
        },
        Err(error) => Count::unknown(error_state(&error)),
    };
    (count, true)
}

fn legacy_queries(
    conn: &Connection,
    raw: std::result::Result<Option<&Connection>, CheckState>,
) -> LegacyChecks {
    let integrity = match crate::db::quick_check(conn, "oboete.db") {
        Ok(()) => CheckState::Known,
        Err(error) if error.downcast_ref::<rusqlite::Error>().is_none() => CheckState::Damaged,
        Err(error) => error_state(&error),
    };
    if !matches!(integrity, CheckState::Known) {
        return LegacyChecks::unknown(integrity);
    }
    let [sessions, events, observations, summaries] = crate::migrate::legacy_counts_in(conn);
    let (remaining_events, remaining_dependency) = remaining_events(conn, raw);
    LegacyChecks {
        integrity,
        sessions: count(conn, "sessions", &[], || Ok(sessions?)),
        events: count(conn, "events", &[], || Ok(events?)),
        observations: count(conn, "observations", &[], || Ok(observations?)),
        summaries: count(conn, "summaries", &[], || Ok(summaries?)),
        recent: recent_calls(conn),
        remaining_events,
        remaining_dependency,
    }
}

fn raw_and_legacy(
    home: &Path,
    raw_before: &Result<Vec<SourceStamp>>,
    legacy_before: &Result<Vec<SourceStamp>>,
) -> (RawChecks, LegacyChecks) {
    let mut legacy_result = None;
    let raw_result = copy_checks(home, Database::Raw, raw_before, |conn| {
        let checks = raw_queries(conn);
        let evidence = if matches!(checks.integrity, CheckState::Known) {
            Ok(Some(conn))
        } else {
            Err(checks.integrity)
        };
        legacy_result = Some(copy_checks(home, Database::Legacy, legacy_before, |v1| {
            legacy_queries(v1, evidence)
        }));
        checks
    });
    let raw_failure = raw_result.as_ref().err().copied();
    let raw = raw_result.unwrap_or_else(RawChecks::unknown);
    let mut legacy = legacy_result
        .unwrap_or_else(|| {
            copy_checks(home, Database::Legacy, legacy_before, |v1| {
                let evidence = match raw_failure {
                    Some(CheckState::Absent) => Ok(None),
                    Some(state) => Err(state),
                    None => Err(CheckState::Unavailable),
                };
                legacy_queries(v1, evidence)
            })
        })
        .unwrap_or_else(LegacyChecks::unknown);
    if let Some(state) = raw_failure
        && !matches!(state, CheckState::Absent)
        && legacy.remaining_dependency
    {
        legacy.remaining_events = Count::unknown(state);
    }
    (raw, legacy)
}

#[derive(Serialize)]
struct Stores {
    raw: StorePresence,
    raw_restored: StorePresence,
    knowledge: StorePresence,
    legacy: StorePresence,
    providers: StorePresence,
}

#[derive(Serialize)]
struct StorePresence {
    file: Presence,
    wal: Presence,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Presence {
    Absent,
    Present,
    Unreadable,
    Unavailable,
}

impl Stores {
    fn complete(&self) -> bool {
        [
            &self.raw,
            &self.raw_restored,
            &self.knowledge,
            &self.legacy,
            &self.providers,
        ]
        .into_iter()
        .all(|store| {
            [&store.file, &store.wal]
                .into_iter()
                .all(|state| matches!(state, Presence::Absent | Presence::Present))
        })
    }
}

fn presence(path: &Path) -> Presence {
    match super::diagnostic_metadata(path) {
        Ok(metadata) if metadata.is_file() => Presence::Present,
        Ok(_) => Presence::Unavailable,
        Err(error) if error.kind() == ErrorKind::NotFound => Presence::Absent,
        Err(error) if error.kind() == ErrorKind::Unsupported => Presence::Unavailable,
        Err(_) => Presence::Unreadable,
    }
}

fn store(home: &Path, name: &str) -> StorePresence {
    StorePresence {
        file: presence(&home.join(name)),
        wal: presence(&home.join(format!("{name}-wal"))),
    }
}

pub(crate) fn doctor_report(home: &Path) -> DoctorReport {
    let raw_before = observed(home, Database::Raw);
    let knowledge_before = observed(home, Database::Knowledge);
    let legacy_before = observed(home, Database::Legacy);
    let providers_before = observed(home, Database::Providers);
    let config_before = config_stamp(home);
    #[cfg(test)]
    if let Some(hook) = BEFORE_CONFIG.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let config_contents = config_text(home, &config_before);
    let config = config_contents
        .as_ref()
        .map_err(|state| *state)
        .and_then(|text| {
            crate::config::from_text(&home.join("config.toml"), text)
                .map_err(|_| CheckState::Invalid)
        });
    let backup_observation =
        backups::Observation::begin(home, config_contents.as_deref().map_err(|state| *state));
    let runtime_observation =
        runtime::Observation::begin(home, config_contents.as_deref().map_err(|state| *state));
    let embedding_on = config
        .as_ref()
        .is_ok_and(|config| config.embedding.provider != "none");
    let mut embeddings = EmbeddingChecks::unknown(match config.as_ref() {
        Ok(_) if embedding_on => CheckState::Unavailable,
        Ok(_) => CheckState::Off,
        Err(state) => *state,
    });
    #[cfg(test)]
    if let Some(hook) = BEFORE_INVENTORY.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let inventory_config = config_contents
        .as_deref()
        .map(|text| {
            if config_before.as_ref().is_ok_and(|stamps| stamps.is_empty()) {
                None
            } else {
                Some(text)
            }
        })
        .map_err(|state| match state {
            CheckState::Invalid => super::readiness::State::Invalid,
            _ => super::readiness::State::Unreadable,
        });
    let inventory = super::readiness::readiness_from_text(home, inventory_config);
    let stores = Stores {
        raw: store(home, "raw.db"),
        raw_restored: store(home, "raw.db.restored"),
        knowledge: store(home, "knowledge.db"),
        legacy: store(home, "oboete.db"),
        providers: store(home, "providers.db"),
    };
    #[cfg(test)]
    if let Some(hook) = BEFORE_COPY.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let (mut raw, mut legacy) = raw_and_legacy(home, &raw_before, &legacy_before);
    let mut knowledge = copy_checks(home, Database::Knowledge, &knowledge_before, |conn| {
        let checks = knowledge_queries(conn);
        if embedding_on {
            embeddings = if matches!(checks.integrity, CheckState::Known) {
                embedding_queries(conn)
            } else {
                EmbeddingChecks::unknown(checks.integrity)
            };
        }
        checks
    })
    .unwrap_or_else(KnowledgeChecks::unknown);
    if embedding_on && !matches!(knowledge.integrity, CheckState::Known) {
        embeddings = EmbeddingChecks::unknown(knowledge.integrity);
    }
    let mut providers = copy_checks(home, Database::Providers, &providers_before, |conn| {
        ledger::queries(conn, config.as_ref().map_err(|state| *state))
    })
    .unwrap_or_else(LedgerChecks::unknown);
    let backup_device_dependency = raw.device.is_some();
    let mut backups = backup_observation.checks(raw.device.as_deref().ok_or(raw.max_seq.state));
    let mut runtime = runtime_observation.checks();
    let retained_observation = retained::Observation::begin(home);
    let mut retained = retained_observation.checks();
    let disk_observation = DiskObservation::begin(home);
    let mut disk = disk_observation.checks();
    #[cfg(test)]
    if let Some(hook) = AFTER_COLLECTION.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let mut source_stability = if [
        raw.integrity,
        knowledge.integrity,
        legacy.integrity,
        providers.integrity,
        embeddings.state,
        backups.state,
        runtime.state,
        retained.state,
        disk.state,
    ]
    .into_iter()
    .any(|state| matches!(state, CheckState::Changed))
    {
        CheckState::Changed
    } else {
        CheckState::Known
    };
    if let Some(state) = recheck(home, Database::Raw, &raw_before) {
        raw = RawChecks::unknown(state);
        if legacy.remaining_dependency {
            legacy.remaining_events = Count::unknown(state);
        }
        if backup_device_dependency {
            backups.through_seq = Count::unknown(state);
            backups.through_op_seq = Count::unknown(state);
        }
        source_stability = stability(source_stability, state);
    }
    if let Some(state) = recheck(home, Database::Knowledge, &knowledge_before) {
        knowledge = KnowledgeChecks::unknown(state);
        if embedding_on {
            embeddings = EmbeddingChecks::unknown(state);
        }
        source_stability = stability(source_stability, state);
    }
    if let Some(state) = recheck(home, Database::Legacy, &legacy_before) {
        legacy = LegacyChecks::unknown(state);
        source_stability = stability(source_stability, state);
    }
    if let Some(state) = recheck(home, Database::Providers, &providers_before) {
        providers = LedgerChecks::unknown(state);
        source_stability = stability(source_stability, state);
    }
    providers
        .embedding_quota
        .configure(config.as_ref().map_err(|state| *state));
    if let Some(state) = backup_observation.finish(&mut backups) {
        source_stability = stability(source_stability, state);
    }
    if let Some(state) = runtime_observation.finish(home, &mut runtime) {
        source_stability = stability(source_stability, state);
    }
    if let Some(state) = disk_observation.finish(home, &mut disk) {
        source_stability = stability(source_stability, state);
    }
    if let Some(state) = retained_observation.finish(&mut retained) {
        source_stability = stability(source_stability, state);
    }
    let config_after = config_stamp(home);
    let config_state = match (config_before, config_after) {
        (Ok(before), Ok(after)) if before == after => CheckState::Known,
        (Ok(_), Ok(_)) => CheckState::Changed,
        (Err(error), _) | (_, Err(error)) => error_state(&error),
    };
    source_stability = stability(source_stability, config_state);
    if !matches!(config_state, CheckState::Known) {
        embeddings = EmbeddingChecks::unknown(config_state);
        providers.usage = ledger::Usage::unknown(config_state);
        providers.embedding_quota = ledger::EmbeddingQuota::unknown(config_state);
        backups = BackupChecks::unknown(config_state);
        runtime.config_changed(config_state);
    }
    let checks = Checks {
        raw,
        knowledge,
        legacy,
        embeddings,
        providers,
        backups,
        runtime,
        disk,
        retained,
        source_stability,
        remaining: "none",
    };
    let complete = checks.complete() && inventory.complete() && stores.complete() && config.is_ok();
    let mut unhealthy = checks.unhealthy();
    if matches!(config_state, CheckState::Known) && matches!(config, Err(CheckState::Invalid)) {
        unhealthy.push(UnhealthyCode::ConfigInvalid);
    }
    DoctorReport {
        complete,
        unhealthy,
        inventory,
        stores,
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn w6d_inventory_uses_the_same_admitted_config_even_during_an_unobserved_aba() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        std::fs::write(&config, b"").unwrap();
        let modified = std::fs::metadata(&config).unwrap().modified().unwrap();
        let during = config.clone();
        BEFORE_INVENTORY.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(during, b"[broken").unwrap();
            }))
        });
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(&config, b"").unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(config)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(
            report["checks"]["source_stability"], "known",
            "fixture must restore the before/after witness"
        );
        assert_eq!(
            report["inventory"]["config"], "valid",
            "inventory independently consumed the transient config"
        );
        assert_eq!(report["complete"], true);
    }

    #[test]
    fn w6d_negative_native_position_is_unknown_instead_of_a_known_count() {
        let root = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(root.path()).unwrap();
        raw.append(&crate::raw::test_event("invented count boundary"))
            .unwrap();
        drop(raw);
        assert!(
            doctor_report(root.path()).complete,
            "native positions start established"
        );
        Connection::open(root.path().join("raw.db"))
            .unwrap()
            .execute("UPDATE records SET seq=-1", [])
            .unwrap();
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(
            report["checks"]["raw"]["max_seq"],
            serde_json::json!({"state":"unavailable","value":null})
        );
        assert_eq!(report["complete"], false);
    }

    #[test]
    fn w6d_nullable_call_duration_is_incomplete_but_an_empty_history_is_complete() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.toml"), b"providers=[]").unwrap();
        let db = crate::providers_db::open(root.path()).unwrap();
        assert!(
            doctor_report(root.path()).complete,
            "empty native ledger should establish its facts"
        );
        db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms) VALUES(1,'invented','curator','ok',-1)",[]).unwrap();
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(report["checks"]["providers"]["recent"]["state"], "known");
        assert_eq!(
            report["checks"]["providers"]["recent"]["calls"][0]["ms"],
            serde_json::Value::Null
        );
        assert_eq!(report["complete"], false);
        assert_eq!(report["unhealthy"], serde_json::json!([]));
    }

    #[test]
    fn w6d_nullable_rewind_time_is_incomplete_but_no_rewinds_is_complete() {
        let root = tempfile::tempdir().unwrap();
        let db = Connection::open(root.path().join("knowledge.db")).unwrap();
        db.execute_batch("CREATE TABLE rewinds(ts); CREATE TABLE gaps(agent TEXT,transcript_turns INTEGER,raw_turns INTEGER);").unwrap();
        assert!(
            doctor_report(root.path()).complete,
            "empty rewind history is an established fact"
        );
        db.execute("INSERT INTO rewinds VALUES(NULL)", []).unwrap();
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(
            report["checks"]["knowledge"]["rewinds"]["count"]["value"],
            1
        );
        assert_eq!(
            report["checks"]["knowledge"]["rewinds"]["last_at_ms"],
            serde_json::Value::Null
        );
        assert_eq!(report["complete"], false);
    }

    #[test]
    fn w6d_corrected_config_drops_the_original_invalid_warning() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(&path, b"[broken").unwrap();
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(path, b"").unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(report["checks"]["source_stability"], "changed");
        assert_eq!(report["complete"], false);
        assert!(
            !report["unhealthy"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("config_invalid"))
        );
        assert_eq!(report["checks"]["disk"]["state"], "known");
    }

    #[test]
    fn w6d_complete_reports_established_absence_and_fixed_known_problems() {
        let root = tempfile::tempdir().unwrap();
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(report["complete"], true);
        assert_eq!(report["checks"]["remaining"], "none");
        assert_eq!(report["unhealthy"], serde_json::json!([]));
        std::fs::write(root.path().join("raw.db"), b"invented damaged store").unwrap();
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(report["complete"], false);
        assert_eq!(report["unhealthy"], serde_json::json!(["raw_damaged"]));
        std::fs::write(root.path().join("config.toml"), b"[broken").unwrap();
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(report["complete"], false);
        assert!(
            report["unhealthy"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("config_invalid"))
        );
        assert_eq!(report["checks"]["remaining"], "none");
    }

    #[test]
    fn w6d_unknown_runtime_leaf_is_incomplete_without_inventing_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("worker-outcome"), b"not a native outcome").unwrap();
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(report["checks"]["runtime"]["state"], "known");
        assert_eq!(
            report["checks"]["runtime"]["worker"]["last"]["state"],
            "invalid"
        );
        assert_eq!(report["complete"], false);
        assert_eq!(report["unhealthy"], serde_json::json!([]));
    }

    #[test]
    fn w6d_retained_metadata_has_fixed_categories_and_native_byte_totals() {
        let root = tempfile::tempdir().unwrap();
        let empty = serde_json::to_value(doctor_report(root.path())).unwrap();
        let rows = empty["checks"]["retained"]["categories"]
            .as_array()
            .unwrap();
        assert_eq!(rows.len(), 7);
        for row in rows {
            assert_eq!(row["present"]["value"], false);
            assert_eq!(row["targets"]["value"], 0);
            assert_eq!(row["bytes"]["value"], 0);
        }
        for (name, size) in [
            ("oboete.db", 4),
            ("memory.db", 3),
            ("memory.db-wal", 5),
            ("pre-snapshot.db", 6),
        ] {
            std::fs::write(root.path().join(name), vec![b'x'; size]).unwrap();
        }
        for (name, size) in [
            ("pre-rollout-private", 7),
            ("spool", 0),
            ("cache", 8),
            ("logs", 9),
            ("eval", 10),
        ] {
            let directory = root.path().join(name);
            std::fs::create_dir(&directory).unwrap();
            if size > 0 {
                std::fs::write(
                    directory.join("private-child-never-public"),
                    vec![b'y'; size],
                )
                .unwrap();
            }
        }
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        let retained = &report["checks"]["retained"];
        assert_eq!(retained["state"], "known");
        for (index, expected) in [4, 8, 6, 7, 0, 8, 9].into_iter().enumerate() {
            let row = &retained["categories"][index];
            assert_eq!(row["present"]["value"], true);
            assert_eq!(row["bytes"]["value"], expected);
            assert_eq!(row["targets"]["value"], if index == 1 { 2 } else { 1 });
        }
        assert_eq!(retained["evaluation_copies"]["present"]["value"], true);
        assert_eq!(retained["evaluation_copies"]["bytes"]["value"], 10);
        assert!(!report.to_string().contains("private-child-never-public"));
    }

    #[test]
    fn w6d_retained_growth_invalidates_only_affected_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("memory.db");
        std::fs::write(&path, b"one").unwrap();
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(path, b"longer content").unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        let retained = &report["checks"]["retained"];
        assert_eq!(retained["state"], "changed");
        let row = &retained["categories"][1];
        assert_eq!(row["present"]["value"], true);
        assert_eq!(row["targets"]["value"], 1);
        assert_eq!(
            row["bytes"],
            serde_json::json!({"state":"changed","value":null})
        );
        assert_eq!(retained["categories"][0]["bytes"]["value"], 0);
        assert_eq!(report["checks"]["raw"]["integrity"], "absent");
        assert_eq!(report["checks"]["source_stability"], "changed");
    }

    #[cfg(unix)]
    #[test]
    fn w6d_retained_symlinks_count_the_link_without_walking_its_target() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let target = root.path().join("outside");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("never-count-this"), [0u8; 1000]).unwrap();
        symlink(&target, home.join("eval")).unwrap();
        symlink(&target, home.join("memory.db")).unwrap();
        symlink(&target, home.join("cache")).unwrap();
        std::fs::create_dir(home.join("spool")).unwrap();
        symlink(&target, home.join("spool/nested")).unwrap();
        let own = std::fs::symlink_metadata(home.join("eval")).unwrap().len();
        let report = serde_json::to_value(doctor_report(&home)).unwrap();
        let retained = &report["checks"]["retained"];
        assert_eq!(retained["state"], "known");
        assert_eq!(retained["categories"][1]["bytes"]["value"], own);
        assert_eq!(retained["categories"][4]["bytes"]["value"], own);
        assert_eq!(retained["categories"][5]["present"]["value"], false);
        assert_eq!(retained["evaluation_copies"]["present"]["value"], true);
        assert_eq!(retained["evaluation_copies"]["bytes"]["value"], own);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6d_retained_matching_fifo_is_metadata_only_without_a_data_open() {
        use std::{
            io::Read,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::ffi::OsStrExt,
            },
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pre-only-metadata.db");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: owned synthetic path, no existing file.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // SAFETY: fresh descriptor transferred to File exactly once.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0);
        let mut watcher = unsafe { std::fs::File::from_raw_fd(fd) };
        // SAFETY: watcher and terminated path live through registration.
        assert!(
            unsafe { libc::inotify_add_watch(watcher.as_raw_fd(), name.as_ptr(), libc::IN_OPEN) }
                >= 0
        );
        let report = serde_json::to_value(doctor_report(root.path())).unwrap();
        assert_eq!(
            report["checks"]["retained"]["categories"][2]["targets"]["value"],
            1
        );
        assert_eq!(
            report["checks"]["retained"]["categories"][2]["bytes"]["value"],
            0
        );
        assert!(
            matches!(watcher.read(&mut [0u8;64]),Err(error) if error.kind()==ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn w6d_disk_admission_change_remains_observed_at_report_finish() {
        let observation = DiskObservation(Err(CheckState::Changed));
        let mut checks = observation.checks();
        assert!(matches!(checks.state, CheckState::Changed));
        assert!(matches!(
            observation.finish(Path::new("unused"), &mut checks),
            Some(CheckState::Changed)
        ));
        assert!(checks.free_bytes.is_none());
    }

    #[test]
    fn w6d_initially_missing_home_is_absent_instead_of_changed() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("not-initialized");
        let report = serde_json::to_value(doctor_report(&home)).unwrap();
        assert_eq!(report["checks"]["disk"]["state"], "absent");
        assert_eq!(report["checks"]["source_stability"], "known");
        assert!(!home.exists());
    }

    #[test]
    fn w6d_disk_binding_change_is_not_hidden_by_absent_stores() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let kept = root.path().join("kept");
        std::fs::create_dir(&home).unwrap();
        let path = home.clone();
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, kept).unwrap();
                std::fs::create_dir(path).unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(&home)).unwrap();
        assert_eq!(report["checks"]["disk"]["state"], "changed");
        assert_eq!(
            report["checks"]["disk"]["free_bytes"],
            serde_json::Value::Null
        );
        assert_eq!(report["checks"]["source_stability"], "changed");
        assert_eq!(report["checks"]["raw"]["integrity"], "absent");
    }

    #[test]
    fn w6d_disk_unknown_and_zero_keep_the_native_low_space_boundary() {
        let unknown = serde_json::to_value(DiskChecks::from_bytes(None)).unwrap();
        assert_eq!(
            unknown,
            serde_json::json!({"state":"unavailable","free_bytes":null,"low_space":null})
        );
        for (value, low) in [
            (0, true),
            (super::super::LOW_FREE_BYTES - 1, true),
            (super::super::LOW_FREE_BYTES, false),
        ] {
            let report = serde_json::to_value(DiskChecks::from_bytes(Some(value))).unwrap();
            assert_eq!(report["state"], "known");
            assert_eq!(report["free_bytes"], value);
            assert_eq!(report["low_space"], low);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn w6d_disk_observation_is_explicit_and_does_not_create_state() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let before = crate::backup::tests::w5b_files(p);
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["disk"]["state"], "known");
        assert!(report["checks"]["disk"]["free_bytes"].as_u64().is_some());
        assert!(report["checks"]["disk"]["low_space"].is_boolean());
        assert!(
            before == crate::backup::tests::w5b_files(p),
            "disk observation changed source files"
        );
    }

    #[test]
    fn w6d_runtime_lock_status_does_not_depend_on_lock_file_contents() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let state = p.join("state");
        std::fs::create_dir(&state).unwrap();
        for length in [3, super::super::readiness::TEXT_LIMIT + 1] {
            let worker = std::fs::File::create(state.join("worker.lock")).unwrap();
            worker.set_len(length).unwrap();
            worker.lock().unwrap();
            let viewer = std::fs::File::create(state.join("view.lock")).unwrap();
            viewer.set_len(length).unwrap();
            viewer.lock().unwrap();
            assert!(crate::worker::lock_held(&state.join("worker.lock")));
            let report = serde_json::to_value(doctor_report(p)).unwrap();
            assert_eq!(
                report["checks"]["runtime"]["worker"]["running"],
                serde_json::json!({"state":"known","value":true})
            );
            assert_eq!(
                report["checks"]["runtime"]["resident_viewer"]["running"],
                serde_json::json!({"state":"known","value":true})
            );
        }
    }

    #[test]
    fn w6d_runtime_distinguishes_missing_invalid_and_stale_status() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["runtime"]["recording"]["state"], "absent");
        let state = p.join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(p.join("config.toml"), "[view]\nport=23123\n").unwrap();
        std::fs::write(state.join("recording-failed"), "invalid marker").unwrap();
        std::fs::write(state.join("worker-outcome"), "private-invalid-generation").unwrap();
        std::fs::write(state.join("view-outcome"), "listening 23124").unwrap();
        let viewer = std::fs::File::create(state.join("view.lock")).unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["runtime"]["recording"]["state"], "invalid");
        assert_eq!(
            report["checks"]["runtime"]["worker"]["last"]["state"],
            "invalid"
        );
        assert_eq!(
            report["checks"]["runtime"]["resident_viewer"]["last"]["kind"],
            "stopped"
        );
        viewer.lock().unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["runtime"]["resident_viewer"]["last"]["kind"],
            "other_port"
        );
        assert_eq!(
            report["checks"]["runtime"]["resident_viewer"]["actual_port"],
            23124
        );
        #[cfg(unix)]
        {
            let target = p.join("private-target");
            std::fs::write(&target, "failed io 1 2").unwrap();
            std::fs::remove_file(state.join("recording-failed")).unwrap();
            std::os::unix::fs::symlink(target, state.join("recording-failed")).unwrap();
            let report = serde_json::to_value(doctor_report(p)).unwrap();
            assert_eq!(
                report["checks"]["runtime"]["recording"]["state"],
                "unavailable"
            );
            assert_eq!(
                report["checks"]["runtime"]["recording"]["failed"],
                serde_json::Value::Null
            );
        }
    }

    #[test]
    fn w6d_runtime_invalidates_changed_lock_and_config_dependencies() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let state = p.join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(
            state.join("worker-outcome"),
            format!("3\n{}", crate::worker::STOPPED),
        )
        .unwrap();
        let worker = std::fs::File::create(state.join("worker.lock")).unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["runtime"]["worker"]["last"]["kind"],
            "interrupted"
        );
        worker.lock().unwrap();
        AFTER_COLLECTION.with(|hook| *hook.borrow_mut() = Some(Box::new(move || drop(worker))));
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["runtime"]["worker"]["running"]["state"],
            "changed"
        );
        assert_eq!(
            report["checks"]["runtime"]["worker"]["last"]["state"],
            "changed"
        );
        assert_eq!(report["checks"]["source_stability"], "changed");
        std::fs::write(state.join("recording-failed"), "ok 1\n").unwrap();
        let path = p.join("config.toml");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(path, "[view]\nport=23125\n").unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["runtime"]["resident_viewer"]["configured_port"]["state"],
            "changed"
        );
        assert_eq!(report["checks"]["runtime"]["recording"]["failed"], false);
    }

    #[test]
    fn w6d_runtime_reads_fixed_status_without_starting_or_renumbering() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let state = p.join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(
            p.join("config.toml"),
            "[worker]\nresident=true\n[view]\nport=23123\n[embedding]\nprovider='unsupported'\n",
        )
        .unwrap();
        std::fs::write(state.join("recording-failed"), "failed disk-full 17 22\n").unwrap();
        std::fs::write(state.join("worker-outcome"), "7\nprivate-worker-error").unwrap();
        std::fs::write(state.join("view-outcome"), "listening 23123").unwrap();
        std::fs::write(state.join("restored"), "private-restored-note").unwrap();
        let worker = std::fs::File::create(state.join("worker.lock")).unwrap();
        let viewer = std::fs::File::create(state.join("view.lock")).unwrap();
        // Snapshot the bytes before taking Windows' exclusive byte-range locks.
        let before = crate::backup::tests::w5b_files(p);
        worker.lock().unwrap();
        viewer.lock().unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        let runtime = &report["checks"]["runtime"];
        assert_eq!(
            runtime["recording"],
            serde_json::json!({"state":"known","failed":true,"class":"disk_full","since_ns":17})
        );
        assert_eq!(
            runtime["worker"]["configured_resident"],
            serde_json::json!({"state":"known","value":true})
        );
        assert_eq!(
            runtime["worker"]["running"],
            serde_json::json!({"state":"known","value":true})
        );
        assert_eq!(runtime["worker"]["last"]["kind"], "failed");
        assert_eq!(
            runtime["resident_viewer"]["configured_port"]["value"],
            23123
        );
        assert_eq!(runtime["resident_viewer"]["last"]["kind"], "up");
        assert_eq!(runtime["resident_viewer"]["actual_port"], 23123);
        assert_eq!(runtime["restore_note"]["value"], true);
        assert!(!report.to_string().contains("private-"));
        worker.unlock().unwrap();
        viewer.unlock().unwrap();
        assert!(
            before == crate::backup::tests::w5b_files(p),
            "runtime diagnosis changed files"
        );
        worker.lock().unwrap();
        viewer.lock().unwrap();
        let path = state.join("worker-outcome");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(path, "8\n").unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["runtime"]["worker"]["last"]["state"],
            "changed"
        );
        assert_eq!(
            report["checks"]["runtime"]["resident_viewer"]["last"]["kind"],
            "up"
        );
    }

    #[test]
    fn w6d_embedding_quota_uses_native_shared_pool_and_query_reserve() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let db = crate::providers_db::open(p).unwrap();
        let now = crate::db::now_ms();
        db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms,detail,usd) VALUES(?1,?2,'embed','error',1,'private-first-error',0.1),(?1,?2,'query','ok',1,'private-query',0.2),(1,?2,'private-role','error',1,'private-last-error',0)",rusqlite::params![now,crate::embed::CALLS]).unwrap();
        crate::providers_db::set_state(
            &db,
            crate::embed::CALLS,
            crate::providers_db::State {
                down_until: now + 600_000,
                ..Default::default()
            },
        )
        .unwrap();
        for cap in [39u32, 40, 41] {
            std::fs::write(p.join("config.toml"),format!("[embedding]\nprovider='workers-ai'\naccount_id='private-account'\ndaily_requests={cap}\nmonthly_usd=2.5\n")).unwrap();
            let before = crate::backup::tests::w5b_files(p);
            let report = serde_json::to_value(doctor_report(p)).unwrap();
            let quota = &report["checks"]["providers"]["embedding_quota"];
            assert_eq!(quota["document_daily_requests"], cap.saturating_sub(40));
            assert_eq!(quota["query_daily_requests"], cap);
            assert_eq!(quota["monthly_usd_cap"], 2.5);
            assert_eq!(
                quota["calls_last_day"],
                serde_json::json!({"state":"known","value":2})
            );
            assert_eq!(quota["rest"]["kind"], "resting");
            assert_eq!(quota["last_error"]["role"], "other");
            assert_eq!(quota["last_error"]["at_ms"], 1);
            assert!(!report.to_string().contains("private-"));
            assert_eq!(report["checks"]["embeddings"]["state"], "absent");
            assert_eq!(before, crate::backup::tests::w5b_files(p));
        }
        let path = p.join("providers.db");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                Connection::open(path)
                    .unwrap()
                    .execute_batch("UPDATE provider_calls SET usd=usd+1;")
                    .unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["providers"]["embedding_quota"]["query_daily_requests"],
            41
        );
        assert_eq!(
            report["checks"]["providers"]["embedding_quota"]["calls_last_day"]["state"],
            "changed"
        );
        std::fs::write(p.join("config.toml"), "").unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["providers"]["embedding_quota"]["state"],
            "off"
        );
    }

    #[test]
    fn w6d_failed_final_backup_validation_invalidates_dependent_facts() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        raw.append(&crate::raw::test_event("invented final-check record"))
            .unwrap();
        crate::backup::export(p).unwrap();
        let directory = p.join("backups");
        let checksum = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.to_string_lossy().ends_with(".sha256"))
            .unwrap();
        let original = std::fs::read(&checksum).unwrap();
        let path = checksum.clone();
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(path, [0xff]).unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["backups"]["state"], "unreadable");
        assert_eq!(
            report["checks"]["backups"]["verification"]["checksum_mismatch"],
            serde_json::json!({"state":"unreadable","value":null})
        );
        assert_eq!(report["checks"]["backups"]["record_segments"]["value"], 1);
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 1);
        std::fs::write(checksum, original).unwrap();
        let kept = p.join("kept");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&directory, kept).unwrap();
                std::fs::write(directory, b"not a directory").unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["backups"]["record_segments"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 1);
    }

    #[test]
    fn w6d_backup_directory_enumeration_and_children_stay_on_the_admitted_object() {
        use std::io::Read;
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        std::fs::create_dir(&selected).unwrap();
        let mut expected = Vec::new();
        for index in 0..400 {
            let name = format!("entry-{index:04}-{}.data", "あ".repeat(70));
            std::fs::write(selected.join(&name), b"owned original").unwrap();
            expected.push(std::ffi::OsString::from(name));
        }
        expected.sort();
        let mut directory = super::super::DiagnosticDirectory::open(&selected).unwrap();
        for _ in 0..2 {
            let mut names = directory.names().unwrap();
            names.sort();
            assert_eq!(names, expected);
        }
        std::fs::rename(&selected, root.path().join("kept")).unwrap();
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join(&expected[0]), b"replacement").unwrap();
        let mut value = String::new();
        directory
            .open_file(&expected[0])
            .unwrap()
            .read_to_string(&mut value)
            .unwrap();
        assert_eq!(value, "owned original");
        let replacement = super::super::DiagnosticDirectory::open(&selected).unwrap();
        assert_ne!(
            directory.identity().unwrap(),
            replacement.identity().unwrap()
        );
        assert_eq!(
            directory
                .open_file(std::ffi::OsStr::new("../outside"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6d_backup_fifo_is_refused_before_a_data_open() {
        use std::{
            io::Read,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::ffi::OsStrExt,
            },
        };
        let root = tempfile::tempdir().unwrap();
        let name = "private-fifo-000000000001-000000000002.seg.zst";
        let path = root.path().join(name);
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: all native calls use this owned synthetic path and newly owned descriptors.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        // SAFETY: these flags return a nonblocking close-on-exec descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0);
        // SAFETY: successful init returned a newly owned descriptor exactly once.
        let mut watcher = unsafe { std::fs::File::from_raw_fd(fd) };
        // SAFETY: the watcher and terminated path live through registration.
        assert!(
            unsafe { libc::inotify_add_watch(watcher.as_raw_fd(), path.as_ptr(), libc::IN_OPEN) }
                >= 0
        );
        let directory = super::super::DiagnosticDirectory::open(root.path()).unwrap();
        assert_eq!(
            directory
                .open_file(std::ffi::OsStr::new(name))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
        let mut events = [0u8; 64];
        assert!(
            matches!(watcher.read(&mut events),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        );
    }

    #[cfg(windows)]
    #[test]
    fn w6d_backup_windows_junctions_do_not_redirect_directory_or_child_reads() {
        use std::io::Read;
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        let target = root.path().join("target");
        std::fs::create_dir(&selected).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(selected.join("owned"), b"original").unwrap();
        std::fs::write(target.join("owned"), b"target").unwrap();
        let mut directory = super::super::DiagnosticDirectory::open(&selected).unwrap();
        std::fs::rename(&selected, root.path().join("kept")).unwrap();
        let system = std::env::var_os("SystemRoot").unwrap();
        let junction = |link: &Path| {
            let output =
                std::process::Command::new(PathBuf::from(&system).join("System32").join("cmd.exe"))
                    .env_clear()
                    .env("SystemRoot", &system)
                    .env("TEMP", root.path())
                    .env("TMP", root.path())
                    .current_dir(root.path())
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(link)
                    .arg(&target)
                    .output()
                    .unwrap();
            assert!(output.status.success(), "private junction fixture failed");
        };
        junction(&selected);
        assert!(super::super::DiagnosticDirectory::open(&selected).is_err());
        assert_eq!(
            directory.names().unwrap(),
            vec![std::ffi::OsString::from("owned")]
        );
        let mut text = String::new();
        directory
            .open_file(std::ffi::OsStr::new("owned"))
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "original");
        junction(&root.path().join("kept").join("child"));
        // A real directory would open here; a junction must not be followed.
        assert!(
            directory
                .open_directory(std::ffi::OsStr::new("child"))
                .is_err()
        );
        assert!(directory.open_file(std::ffi::OsStr::new("child")).is_err());
        assert_eq!(std::fs::read(target.join("owned")).unwrap(), b"target");
    }

    #[test]
    fn w6d_non_null_malformed_covers_is_not_silently_ignored() {
        for (covers, invalid) in [
            (None, false),
            (Some(serde_json::Value::Null), false),
            (Some(serde_json::json!({"from_seq":1,"to_seq":1})), false),
            (
                Some(
                    serde_json::json!({"from_seq":1,"to_seq":1,"from_offset":null,"to_offset":null}),
                ),
                false,
            ),
            (
                Some(serde_json::json!({"from_seq":1,"to_seq":1,"from_offset":"bad"})),
                true,
            ),
            (
                Some(serde_json::json!({"from_seq":1,"to_seq":1,"to_offset":1.5})),
                true,
            ),
            (
                Some(serde_json::json!({"from_seq":1,"to_seq":1,"from_offset":u64::MAX})),
                true,
            ),
            (Some(serde_json::json!({"from_seq":1})), true),
            (Some(serde_json::json!({})), true),
            (Some(serde_json::json!("invalid")), true),
        ] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = crate::raw::open(home.path()).unwrap();
            let mut body = serde_json::json!({"from_seq":1,"to_seq":1,"outcome":"skipped","reason":"imported:oboete-v1"});
            if let Some(covers) = covers {
                body["covers"] = covers;
            }
            raw.append_ops(&[(crate::raw::OpKind::Window, body)])
                .unwrap();
            let report = serde_json::to_value(doctor_report(home.path())).unwrap();
            assert_eq!(
                report["checks"]["raw"]["parking"]["state"],
                if invalid { "unavailable" } else { "known" }
            );
            assert_eq!(
                crate::curate::parked_spans(&raw, "oboete-v1").is_err(),
                invalid,
                "native recuration must reject corrupt ranges before arithmetic or sends"
            );
        }
    }

    #[test]
    fn w6d_non_null_window_offsets_require_integers() {
        for field in ["from_offset", "to_offset"] {
            for (offset, invalid) in [
                (serde_json::Value::Null, false),
                (serde_json::json!("bad"), true),
                (serde_json::json!(1.5), true),
                (serde_json::json!(u64::MAX), true),
            ] {
                let home = tempfile::tempdir().unwrap();
                let mut raw = crate::raw::open(home.path()).unwrap();
                let mut body = serde_json::json!({"from_seq":1,"to_seq":1,"outcome":"skipped","reason":"imported:oboete-v1"});
                body[field] = offset;
                raw.append_ops(&[(crate::raw::OpKind::Window, body)])
                    .unwrap();
                let report = serde_json::to_value(doctor_report(home.path())).unwrap();
                assert_eq!(
                    report["checks"]["raw"]["parking"]["state"],
                    if invalid { "unavailable" } else { "known" }
                );
                assert_eq!(
                    crate::curate::parked_spans(&raw, "oboete-v1").is_err(),
                    invalid
                );
            }
        }
    }

    #[test]
    fn w6d_logically_damaged_window_ranges_leave_parking_unknown() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_ops(&[
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":i64::MIN,"from_offset":-1,"to_seq":1,"outcome":"skipped","reason":"imported:oboete-v1"})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":i64::MIN,"to_seq":0,"recurate":true})),
        ]).unwrap();
        let report = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(report["checks"]["raw"]["integrity"], "known");
        assert_eq!(report["checks"]["raw"]["max_op_seq"]["value"], 2);
        assert_eq!(report["checks"]["raw"]["parking"]["state"], "unavailable");
        assert_eq!(
            report["checks"]["raw"]["parking"]["rows"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn w6d_backup_verification_uses_selected_membership_and_native_checksums() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        raw.append(&crate::raw::test_event("invented backup record"))
            .unwrap();
        raw.append(&crate::raw::test_event("another invented backup record"))
            .unwrap();
        raw.append_ops(&[(
            crate::raw::OpKind::Migration,
            serde_json::json!({"key":"invented","through":1}),
        )])
        .unwrap();
        crate::backup::export(p).unwrap();
        let selected = p.join("selected");
        std::fs::rename(p.join("backups"), &selected).unwrap();
        std::fs::write(p.join("config.toml"), "[backup]\ndir='selected'\n").unwrap();
        let record = std::fs::read_dir(&selected)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.to_string_lossy().ends_with(".seg.zst"))
            .unwrap();
        let foreign = selected.join("private-foreign-000000000001-000000000090.seg.zst");
        std::fs::copy(&record, &foreign).unwrap();
        let sum = foreign.with_file_name(format!(
            "{}.sha256",
            foreign.file_name().unwrap().to_str().unwrap()
        ));
        std::fs::write(&sum, crate::forget::hash(&std::fs::read(&foreign).unwrap())).unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        let backup = &report["checks"]["backups"];
        assert_eq!(
            backup["record_segments"],
            serde_json::json!({"state":"known","value":2})
        );
        assert_eq!(backup["op_segments"]["value"], 1);
        assert_eq!(backup["through_seq"]["value"], 2);
        assert_eq!(backup["through_op_seq"]["value"], 1);
        assert_eq!(backup["verification"]["missing_checksum"]["value"], 0);
        assert_eq!(backup["verification"]["checksum_mismatch"]["value"], 0);
        assert!(!report.to_string().contains("private-"));
        assert_eq!(before, crate::backup::tests::w5b_files(p));
        std::fs::write(&sum, "wrong synthetic checksum\n").unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["backups"]["verification"]["checksum_mismatch"]["value"],
            1
        );
        std::fs::remove_file(&sum).unwrap();
        std::fs::write(
            p.join("config.toml"),
            "[backup]\ndir='selected'\n[embedding]\nprovider='unsupported'\n",
        )
        .unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["backups"]["verification"]["missing_checksum"]["value"],
            1
        );
        assert_eq!(report["checks"]["backups"]["record_segments"]["value"], 2);
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::remove_file(foreign).unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["backups"]["state"], "changed");
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 2);
    }

    #[test]
    fn w6d_curation_uses_native_leftover_spans_and_partial_checkpoint() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        for seq in 1..=9 {
            let mut event = crate::raw::test_event("invented imported turn");
            event.source = if seq <= 6 {
                "oboete-v1"
            } else if seq == 7 {
                "private-source"
            } else {
                "hook"
            }
            .into();
            if seq == 9 {
                event.kind = "touch".into();
            }
            raw.append(&event).unwrap();
        }
        raw.append_ops(&[
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":1,"to_seq":5,"outcome":"skipped","reason":"imported:oboete-v1"})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":2,"to_seq":4,"recurate":true})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":6,"to_seq":7,"to_offset":1,"outcome":"ok"})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":1,"to_seq":99,"outcome":"ok"})),
        ]).unwrap();
        let writer = Connection::open(p.join("raw.db")).unwrap();
        writer
            .execute_batch("UPDATE ops SET device='private-foreign-device' WHERE op_seq=4;")
            .unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["raw"]["curated_through"],
            serde_json::json!({"state":"known","value":6})
        );
        assert_eq!(
            report["checks"]["raw"]["parking"]["rows"],
            serde_json::json!([
                {"source":"oboete_v1","parked":2,"waiting":0},
                {"source":"transcript","parked":0,"waiting":0},
                {"source":"other","parked":0,"waiting":1}
            ])
        );
        assert!(!report.to_string().contains("private-"));
        assert_eq!(before, crate::backup::tests::w5b_files(p));
        writer
            .execute_batch("ALTER TABLE ops DROP COLUMN batch;")
            .unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 9);
        assert_eq!(report["checks"]["raw"]["curated_through"]["value"], 6);
        assert_eq!(
            report["checks"]["raw"]["parking"]["state"],
            "schema_missing"
        );
        writer.execute_batch("UPDATE ops SET body=json_set(body,'$.to_seq',-9223372036854775808,'$.to_offset',1) WHERE op_seq=3;").unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["raw"]["curated_through"]["state"],
            "unavailable"
        );
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 9);
        writer.execute_batch("ALTER TABLE ops ADD COLUMN batch INTEGER NOT NULL DEFAULT 0; UPDATE ops SET body=json_set(body,'$.to_seq',7,'$.to_offset',1) WHERE op_seq=3;").unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["raw"]["parking"]["state"], "known");
        writer.execute_batch("UPDATE ops SET body=json_set(body,'$.to_seq',9223372036854775807,'$.to_offset',NULL) WHERE op_seq=3;").unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["raw"]["parking"]["state"], "unavailable");
        assert_eq!(
            report["checks"]["raw"]["curated_through"]["value"],
            i64::MAX
        );
    }

    #[test]
    fn w6d_config_used_for_facts_must_match_its_admitted_bytes() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        let kept = home.path().join("original.toml");
        std::fs::write(&path, "providers=[]\n").unwrap();
        let _ledger = crate::providers_db::open(home.path()).unwrap();
        let (from, to) = (path.clone(), kept.clone());
        BEFORE_CONFIG.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&from, &to).unwrap();
                std::fs::write(
                    from,
                    "providers=[]\n[embedding]\nprovider='workers-ai'\naccount_id='synthetic'\n",
                )
                .unwrap();
            }))
        });
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::remove_file(&path).unwrap();
                std::fs::rename(kept, path).unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(report["checks"]["source_stability"], "changed");
        assert_eq!(report["checks"]["embeddings"]["state"], "changed");
        assert_eq!(
            report["checks"]["providers"]["usage"]["daily_calls"]["state"],
            "changed"
        );
    }

    #[test]
    fn w6d_cached_ledger_counts_reservations_without_double_spend_or_key_reads() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "[[providers]]\nkind='cli'\nname='private-shared'\ncli='claude'\n[[providers]]\nkind='cli'\nname='private-shared'\ncli='claude'\n").unwrap();
        let db = crate::providers_db::open(p).unwrap();
        let now = crate::db::now_ms();
        for (provider, role, outcome, usd, detail, prompt, completion, bytes) in [
            (
                "private-shared",
                "curator",
                "error",
                2.0,
                "transport",
                Some(3),
                Some(2),
                10,
            ),
            (
                "private-shared",
                "curator",
                "reserved",
                1.0,
                "[10,20]",
                None,
                None,
                0,
            ),
            (
                crate::embed::CALLS,
                "query",
                "ok",
                0.25,
                "private-detail",
                None,
                None,
                10,
            ),
            (
                crate::embed::CALLS,
                "embed",
                "ok",
                0.5,
                "private-detail",
                None,
                None,
                10,
            ),
            (
                "private-removed",
                "private-role",
                "ok",
                0.5,
                "private-detail",
                None,
                None,
                10,
            ),
        ] {
            db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms,detail,usd,prompt_tokens,completion_tokens,bytes_out) VALUES(?1,?2,?3,?4,12,?5,?6,?7,?8,?9)",rusqlite::params![now,provider,role,outcome,detail,usd,prompt,completion,bytes]).unwrap();
        }
        crate::providers_db::set_state(
            &db,
            "private-shared",
            crate::providers_db::State {
                down_until: crate::providers_db::OWNER_HOLD,
                ..Default::default()
            },
        )
        .unwrap();
        crate::providers_db::set_state(
            &db,
            "private-resting",
            crate::providers_db::State {
                down_until: now + 600_000,
                ..Default::default()
            },
        )
        .unwrap();
        crate::providers_db::set_key_limit(
            &db,
            "private-shared",
            Some(100),
            now,
            "private-key-sha",
        )
        .unwrap();
        db.execute_batch("INSERT INTO isolation VALUES('private-cli','private-version',1,'private-detail',1),('private-cli','private-version2',0,'private-detail',2);
         INSERT INTO pending VALUES('private-device',1,NULL,2,NULL,'private-reason','private-hold',1,0,0,'private-prompt');").unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        let ledger = &report["checks"]["providers"];
        assert_eq!(
            ledger["spend"]["curation_usd"],
            serde_json::json!({"state":"known","value":3.5})
        );
        assert_eq!(ledger["spend"]["embed_query_usd"]["value"], 0.75);
        assert_eq!(ledger["spend"]["reserved_usd"]["value"], 1.0);
        assert_eq!(ledger["usage"]["daily_calls"]["value"], 2);
        assert_eq!(ledger["usage"]["tokens"]["value"], 5);
        assert_eq!(ledger["usage"]["reserved_calls"]["value"], 1);
        assert_eq!(ledger["usage"]["reserved_tokens"]["value"], 30.0);
        assert_eq!(ledger["usage"]["key_binding"], "unchecked");
        assert_eq!(ledger["stopped"]["owner"]["value"], 1);
        assert_eq!(ledger["stopped"]["resting"]["value"], 1);
        assert_eq!(ledger["isolation"]["passed"]["value"], 0);
        assert_eq!(ledger["isolation"]["failed"]["value"], 1);
        assert_eq!(ledger["pending"]["value"], 1);
        assert_eq!(ledger["recent"]["calls"].as_array().unwrap().len(), 5);
        assert!(!report.to_string().contains("private-"));
        assert_eq!(before, crate::backup::tests::w5b_files(p));
        assert!(!p.join("raw.db").exists());
        db.execute_batch("DROP TABLE pending;").unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["providers"]["pending"],
            serde_json::json!({"state":"schema_missing","value":null})
        );
        assert_eq!(
            report["checks"]["providers"]["spend"]["curation_usd"]["value"],
            3.5
        );
        let path = p.join("config.toml");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(path, "providers=[]\n").unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["providers"]["usage"]["daily_calls"],
            serde_json::json!({"state":"changed","value":null})
        );
        assert_eq!(
            report["checks"]["providers"]["spend"]["curation_usd"]["value"],
            3.5
        );
        let path = p.join("providers.db");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                Connection::open(path)
                    .unwrap()
                    .execute_batch("UPDATE provider_calls SET usd=usd+1;")
                    .unwrap();
            }))
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["providers"]["integrity"], "changed");
        assert_eq!(
            report["checks"]["providers"]["spend"]["curation_usd"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(report["checks"]["raw"]["integrity"], "absent");
    }

    #[test]
    fn w6d_embeddings_off_and_native_waiting_counts_need_no_writes() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let k = Connection::open(p.join("knowledge.db")).unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["embeddings"]["state"], "off");
        std::fs::write(
            p.join("config.toml"),
            "[embedding]\nprovider='workers-ai'\naccount_id='synthetic-account'\n",
        )
        .unwrap();
        k.execute_batch(
            "CREATE TABLE claims(uid TEXT);
             INSERT INTO claims VALUES('a'),('b');
             CREATE VIEW active AS SELECT uid FROM claims;
             CREATE TABLE imported(uid TEXT);
             INSERT INTO imported VALUES('i'),('i'),('j');
             CREATE TABLE raw_docs(device TEXT,seq INTEGER,kind TEXT);
             INSERT INTO raw_docs VALUES('private-device',1,'prompt'),('private-device',2,'reply'),
               ('private-device',3,'tool');
             CREATE TABLE cards(device TEXT,op_seq INTEGER,n INTEGER,replaced_by INTEGER);
             INSERT INTO cards VALUES('private-device',5,0,NULL),('private-device',5,1,NULL),
               ('private-device',4,0,5);
             CREATE TABLE turns(device TEXT,op_seq INTEGER,skipped INTEGER);
             INSERT INTO turns VALUES('private-device',6,0),('private-device',7,0),
               ('private-device',8,1);
             CREATE TABLE vec_generation(embedder TEXT,state TEXT);
             CREATE TABLE vector_keys(embedder TEXT,kind TEXT,key TEXT,skipped TEXT);",
        )
        .unwrap();
        let embedder = crate::embed::EMBEDDER;
        k.execute("INSERT INTO vec_generation VALUES(?1,'active')", [embedder])
            .unwrap();
        for (kind, key, skipped) in [
            ("c", "a", None),
            ("o", "private-device.5.0", None),
            ("s", "Sprivate-device.6", None),
            ("k", "i", Some("excluded")),
            ("r", "private-device:1", Some("private-reason")),
        ] {
            k.execute(
                "INSERT INTO vector_keys VALUES(?1,?2,?3,?4)",
                rusqlite::params![embedder, kind, key, skipped],
            )
            .unwrap();
        }
        let before = crate::backup::tests::w5b_files(p);
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        let checks = &report["checks"]["embeddings"];
        assert_eq!(checks["generation"]["value"], "active");
        for name in ["claims", "cards", "summaries", "imports", "records"] {
            assert_eq!(
                checks["waiting"][name],
                serde_json::json!({"state":"known","value":1})
            );
        }
        assert_eq!(
            checks["skipped"]["rows"],
            serde_json::json!([{"reason":"excluded","count":1},{"reason":"held","count":0},{"reason":"refused","count":0},{"reason":"empty","count":0},{"reason":"other","count":1}])
        );
        let public = report.to_string();
        for secret in ["private-reason", "private-device", "synthetic-account"] {
            assert!(!public.contains(secret));
        }
        assert_eq!(before, crate::backup::tests::w5b_files(p));
        assert!(!p.join("providers.db").exists());
        k.execute_batch("DROP TABLE raw_docs;").unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["embeddings"]["waiting"]["claims"]["value"],
            1
        );
        assert_eq!(
            report["checks"]["embeddings"]["waiting"]["records"],
            serde_json::json!({"state":"schema_missing","value":null})
        );
    }

    fn legacy_fixture(home: &Path) -> Connection {
        let conn = Connection::open(home.join("oboete.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta(key TEXT,value TEXT);
             INSERT INTO meta VALUES('device_id','owned-v1');
             CREATE TABLE events(id INTEGER);
             INSERT INTO events VALUES(1),(2),(3);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn w6d_absent_raw_establishes_zero_through_but_damaged_raw_does_not() {
        let home = tempfile::tempdir().unwrap();
        let _legacy = legacy_fixture(home.path());
        let absent = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(absent["checks"]["legacy"]["remaining_events"]["value"], 3);
        assert_eq!(
            absent["checks"]["legacy"]["remaining_events"]["state"],
            "known"
        );
        std::fs::write(home.path().join("raw.db"), b"owned damaged fixture").unwrap();
        let damaged = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(
            damaged["checks"]["legacy"]["remaining_events"]["state"],
            "damaged"
        );
        assert_eq!(
            damaged["checks"]["legacy"]["remaining_events"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(damaged["checks"]["legacy"]["events"]["value"], 3);
    }

    #[test]
    fn w6d_raw_change_invalidates_remaining_but_keeps_legacy_counts() {
        let home = tempfile::tempdir().unwrap();
        let _legacy = legacy_fixture(home.path());
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_ops(&[(
            crate::raw::OpKind::Migration,
            serde_json::json!({"key":"oboete-v1:owned-v1","through":1}),
        )])
        .unwrap();
        let source = home.path().join("raw.db");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let writer = Connection::open(source).unwrap();
                writer
                    .execute_batch("UPDATE ops SET body=json_set(body,'$.through',2);")
                    .unwrap();
            }));
        });
        let report = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(report["checks"]["raw"]["integrity"], "changed");
        assert_eq!(
            report["checks"]["legacy"]["remaining_events"]["state"],
            "changed"
        );
        assert_eq!(
            report["checks"]["legacy"]["remaining_events"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(report["checks"]["legacy"]["events"]["value"], 3);
    }

    #[test]
    fn w6d_an_observed_change_stays_changed_after_the_original_file_returns() {
        let home = tempfile::tempdir().unwrap();
        let original = home.path().join("raw.db");
        let retired = home.path().join("retired.db");
        let conn = Connection::open(&original).unwrap();
        conn.execute_batch("CREATE TABLE fixture(value INTEGER);")
            .unwrap();
        drop(conn);
        let (from, kept) = (original.clone(), retired.clone());
        BEFORE_COPY.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&from, &kept).unwrap();
                std::fs::copy(&kept, &from).unwrap();
            }));
        });
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::remove_file(&original).unwrap();
                std::fs::rename(&retired, &original).unwrap();
            }));
        });
        let report = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(report["checks"]["raw"]["integrity"], "changed");
        assert_eq!(
            report["checks"]["source_stability"], "changed",
            "Doctor forgot a change already observed during collection"
        );
    }

    #[test]
    fn w6d_late_legacy_creation_is_changed_instead_of_absent() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("oboete.db");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let writer = Connection::open(source).unwrap();
                writer
                    .execute_batch("CREATE TABLE events(id INTEGER); INSERT INTO events VALUES(1);")
                    .unwrap();
            }));
        });
        let report = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(report["checks"]["legacy"]["events"]["state"], "changed");
        assert_eq!(
            report["checks"]["legacy"]["events"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(report["checks"]["raw"]["integrity"], "absent");
        assert_eq!(report["checks"]["source_stability"], "changed");
    }

    #[test]
    fn w6d_config_change_does_not_discard_independent_raw_facts() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let seq = raw
            .append(&crate::raw::test_event("owned config consistency fixture"))
            .unwrap();
        let config = home.path().join("config.toml");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(config, "[summary]\ncurate=false\n").unwrap();
            }));
        });
        let report = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(report["checks"]["source_stability"], "changed");
        assert_eq!(report["checks"]["embeddings"]["state"], "changed");
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], seq);
        assert_eq!(report["checks"]["raw"]["max_seq"]["state"], "known");
    }

    #[test]
    fn w6d_changed_raw_after_collection_keeps_independent_knowledge() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        raw.append(&crate::raw::test_event("owned consistency fixture"))
            .unwrap();
        let k = crate::knowledge::open(p).unwrap();
        k.execute_batch("INSERT INTO rewinds VALUES(1000,'claims','private-device',2,1);")
            .unwrap();
        let source = p.join("raw.db");
        AFTER_COLLECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let writer = Connection::open(source).unwrap();
                writer
                    .execute_batch("UPDATE records SET seq=seq+1;")
                    .unwrap();
            }));
        });
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["raw"]["max_seq"]["state"], "changed",
            "Doctor kept a source-derived value after its source changed"
        );
        assert_eq!(
            report["checks"]["raw"]["max_seq"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(
            report["checks"]["knowledge"]["rewinds"]["count"]["value"],
            1
        );
    }
}
