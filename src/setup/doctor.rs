//! Explicit readonly diagnostics, without store initialization or repair.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use super::readiness::Readiness;
use crate::preview::{OpenedSource, PreviewCopyError, SourceStamp};

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
        let embedding = matches!(self.embeddings.state, CheckState::Off | CheckState::Absent)
            || [
                self.embeddings.state,
                self.embeddings.generation.state,
                self.embeddings.waiting.claims.state,
                self.embeddings.waiting.cards.state,
                self.embeddings.waiting.summaries.state,
                self.embeddings.waiting.imports.state,
                self.embeddings.waiting.records.state,
                self.embeddings.skipped.state,
            ]
            .into_iter()
            .all(|state| matches!(state, CheckState::Known));
        raw && knowledge
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
        let checks = crate::preview::with_preview_files(db, wal, |conn| {
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

#[derive(Serialize)]
struct Stores {
    raw: StorePresence,
    raw_restored: StorePresence,
    knowledge: StorePresence,
    /// `oboete.db`, a store of oboete v1, by its metadata only: this build no longer opens or
    /// migrates it (decision 99, 2026-11-12).
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
    let mut raw = copy_checks(home, Database::Raw, &raw_before, raw_queries)
        .unwrap_or_else(RawChecks::unknown);
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
mod tests;
