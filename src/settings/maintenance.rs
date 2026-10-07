//! Bounded viewer import previews and one active/last receipt; no scheduler or persisted jobs.
use super::{Refusal, refused};
use crate::{backup, curate, executable, migrate, transcript, worker};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Agent {
    Claude,
    Codex,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Transcripts { agent: Option<Agent> },
    V1 { from: Option<String> },
    Rebuild {},
    Restore {},
    Finish {},
    Recurate { scope: curate::RecurationScope },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewRequest {
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    operation: Operation,
    preview_key: String,
    operation_id: String,
    confirmed: bool,
}

#[derive(Clone, Default, Serialize)]
struct AgentProgress {
    events: u64,
    selected_bytes: u64,
}
#[derive(Clone, Default, Serialize)]
struct Progress {
    v1_records: u64,
    v1_repositories: u64,
    v1_documents: u64,
    claude: AgentProgress,
    codex: AgentProgress,
    #[serde(skip_serializing_if = "Option::is_none")]
    native: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    providers: Option<curate::ProviderReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    windows: Option<curate::WindowReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_effects: Option<migrate::FinishEffects>,
}
#[derive(Clone, Serialize)]
struct Run {
    operation_id: String,
    kind: &'static str,
    agent: Option<&'static str>,
    phase: &'static str,
    stage: &'static str,
    progress: Progress,
    /// At least one native commit boundary completed, including a zero-payload checkpoint.
    committed: bool,
    result: Option<Value>,
    #[serde(skip)]
    fingerprint: String,
    #[serde(skip)]
    consent: Option<curate::Consent>,
}
#[derive(Default)]
struct State {
    active: Option<Run>,
    last: Option<Run>,
}
pub(crate) struct Maintenance {
    state: Mutex<State>,
    claude: PathBuf,
    codex: PathBuf,
}
struct ActiveRun<'a> {
    maintenance: &'a Maintenance,
    id: &'a str,
}
impl Drop for ActiveRun<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut state = self
            .maintenance
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state
            .active
            .as_ref()
            .is_none_or(|run| run.operation_id != self.id)
        {
            return;
        }
        if let Some(mut run) = state.active.take() {
            run.phase = "unknown";
            run.stage = "unknown";
            run.result = Some(json!({"code":"maintenance_unknown","outcome":null}));
            state.last = Some(run);
        }
    }
}
impl Default for Maintenance {
    fn default() -> Self {
        Self::with_roots(
            crate::setup::claude_dir().join("projects"),
            crate::setup::codex_home().join("sessions"),
        )
    }
}
impl Maintenance {
    fn with_roots(claude: PathBuf, codex: PathBuf) -> Self {
        Self {
            state: Mutex::new(State::default()),
            claude,
            codex,
        }
    }
    fn snapshot(home: &Path, state: &State) -> Value {
        let mut value = json!({"available": state.active.is_some() || state.last.is_some(),
               "active": state.active, "last": state.last});
        let rules = crate::redact::Rules::load(home).ok();
        let gate = |label: &mut Value| {
            if let Some(text) = label.as_str() {
                *label = json!(
                    rules
                        .as_ref()
                        .map(|rules| crate::redact::outbound_with(text, rules))
                        .unwrap_or_default()
                );
            }
        };
        for run in ["active", "last"] {
            let Some(outcome) = value
                .get_mut(run)
                .and_then(|run| run.pointer_mut("/result/outcome"))
            else {
                continue;
            };
            // Cached receipts keep their counts and protocol identities. Only display text
            // crosses the current gate again, including replay and unreadable configuration.
            for pointer in ["/preview/plan/long_sessions", "/deletion/targets"] {
                if let Some(items) = outcome.pointer_mut(pointer).and_then(Value::as_array_mut) {
                    for item in items {
                        if let Some(label) = item.get_mut("label") {
                            gate(label);
                        }
                    }
                }
            }
            for pointer in [
                "/import/deleted_session_labels",
                "/import/uncertain_identifier_labels",
            ] {
                if let Some(items) = outcome.pointer_mut(pointer).and_then(Value::as_array_mut) {
                    for label in items {
                        gate(label);
                    }
                }
            }
        }
        value
    }
    /// Reads display rules only; never opens stores or sources held by a running import.
    pub(crate) fn show(&self, home: &Path) -> Value {
        Self::snapshot(
            home,
            &self.state.lock().unwrap_or_else(PoisonError::into_inner),
        )
    }
    fn roots(&self, agent: Option<Agent>) -> Vec<(&str, &Path)> {
        [
            (Agent::Claude, "claude", self.claude.as_path()),
            (Agent::Codex, "codex", self.codex.as_path()),
        ]
        .into_iter()
        .filter(|(which, _, _)| agent.is_none_or(|a| a == *which))
        .map(|(_, name, path)| (name, path))
        .collect()
    }
    pub(crate) fn preview(
        &self,
        caller: Option<executable::CommandCaller>,
        home: &Path,
        body: &[u8],
    ) -> Result<Value, Refusal> {
        let request: PreviewRequest =
            serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
        request.operation.validate()?;
        if self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .is_some()
        {
            return Err(refused(409, "maintenance_busy", ""));
        }
        let failed = || refused(422, "maintenance_preview_failed", "");
        match request.operation {
            Operation::Recurate { scope } => self.prepare_recuration(
                caller.ok_or_else(|| refused(422, "maintenance_busy", ""))?,
                home,
                scope,
            ),
            Operation::Transcripts { agent } => {
                let preview =
                    transcript::preview(home, &self.roots(agent)).map_err(|_| failed())?;
                Ok(json!({"kind":"transcripts", "agent":agent_name(agent),
                    "preview_key":preview.key, "candidates":transcript_counts(&preview.candidates),
                    "v1":preview.v1.map(|p| json!({"candidates":p.candidates,"settings":p.settings})),
                    "no_model_request":true}))
            }
            Operation::Rebuild { .. } | Operation::Restore { .. } => {
                let preview = match request.operation {
                    Operation::Rebuild { .. } => worker::preview_rebuild(home),
                    _ => backup::preview_restore(home),
                }
                .map_err(|error| refused(422, native_preview_code(&error), ""))?;
                let rules = crate::redact::Rules::load(home).map_err(|_| failed())?;
                let label =
                    crate::redact::outbound_with(&preview.backup_dir.to_string_lossy(), &rules);
                let mut value = serde_json::to_value(&preview).expect("typed preview serializes");
                let fields = value.as_object_mut().expect("typed preview object");
                let key = fields.remove("key").expect("preview key");
                let kind = fields.remove("operation").expect("preview kind");
                fields.insert("preview_key".into(), key);
                fields.insert("kind".into(), kind);
                fields.insert("no_model_request".into(), json!(true));
                if preview.backup.is_some() {
                    fields.insert("backup_label".into(), json!(label));
                }
                Ok(value)
            }
            Operation::Finish { .. } => {
                let preview = migrate::preview_finish(home).map_err(|_| failed())?;
                let rules = crate::redact::Rules::load(home).map_err(|_| failed())?;
                let targets: Vec<_> = preview
                    .deletion
                    .targets
                    .iter()
                    .map(|label| crate::redact::outbound_with(label, &rules))
                    .collect();
                let candidates = &preview.candidates;
                Ok(json!({"kind":"finish", "preview_key":preview.key,
                    "no_model_request":true, "final_import_before_deletion":true,
                    "candidates":{"events":candidates.events,"records":candidates.records,
                        "repositories":candidates.repos,"documents":candidates.documents,
                        "bytes":candidates.bytes},
                    "deletion":{"targets":targets,"nodes":preview.deletion.nodes,
                        "bytes":preview.deletion.bytes}}))
            }
            Operation::V1 { from } => {
                let default_source = from.as_deref().is_none_or(str::is_empty);
                let from = from_path(home, from.as_deref());
                if default_source && !from.try_exists().map_err(|_| failed())? {
                    return Err(refused(422, "maintenance_source_missing", "operation.from"));
                }
                let preview = migrate::preview(home, &from).map_err(|_| failed())?;
                let rules = crate::redact::Rules::load(home).map_err(|_| failed())?;
                let label = crate::redact::outbound_with(&from.to_string_lossy(), &rules);
                Ok(json!({"kind":"v1", "preview_key":preview.key,
                    "source":label, "candidates":preview.candidates,"settings":preview.settings,
                    "no_model_request":true}))
            }
        }
    }

    fn prepare_recuration(
        &self,
        caller: executable::CommandCaller,
        home: &Path,
        scope: curate::RecurationScope,
    ) -> Result<Value, Refusal> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| refused(422, "maintenance_preview_failed", ""))?;
        let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.active.is_some() {
                return Err(refused(409, "maintenance_busy", ""));
            }
            state.active = Some(Run {
                operation_id: id.clone(),
                kind: "recurate",
                agent: None,
                phase: "running",
                stage: "preparing",
                progress: Progress::default(),
                committed: false,
                result: None,
                fingerprint: format!("prepare:{id}"),
                consent: None,
            });
        }
        let _active = ActiveRun {
            maintenance: self,
            id: &id,
        };
        let (index, result) = curate::prepare_report(home, &scope, caller, &mut |event| {
            self.native_progress(&id, event);
        });
        let (mut preview, code, consent) = match result {
            Ok((preview, consent)) => {
                let mut value = serde_json::to_value(preview).expect("typed preview serializes");
                let fields = value.as_object_mut().expect("typed preview object");
                let key = fields.remove("key").expect("preview key");
                fields.insert("preview_key".into(), key);
                (value, None, Some(consent))
            }
            Err(error) => (
                json!({"scope":scope,"preview_key":null}),
                Some(native_preview_code(&error)),
                None,
            ),
        };
        let fields = preview.as_object_mut().expect("typed preview object");
        fields.insert("kind".into(), json!("recurate"));
        fields.insert("no_model_request".into(), json!(true));
        fields.insert("local_preparation".into(), json!(true));
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(mut run) = state.active.take() {
            run.phase = if code.is_none() {
                "prepared"
            } else if run.committed {
                "partial"
            } else {
                "failed"
            };
            run.stage = run.phase;
            run.consent = consent;
            run.result = Some(json!({"code":code,"outcome":{"index":index,"preview":preview}}));
            state.last = Some(run);
        }
        let preparation = Self::snapshot(home, &state);
        if let Some(plan) = preparation.pointer("/last/result/outcome/preview/plan") {
            preview["plan"] = plan.clone();
        }
        preview["preparation"] = preparation;
        Ok(preview)
    }
    pub(crate) fn start(
        &self,
        caller: executable::CommandCaller,
        home: &Path,
        body: &[u8],
    ) -> Result<Value, Refusal> {
        let request: StartRequest =
            serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
        request.operation.validate()?;
        if !request.confirmed {
            return Err(refused(422, "maintenance_confirmation", "confirmed"));
        }
        if !canonical(&request.preview_key) {
            return Err(refused(422, "maintenance_key", "preview_key"));
        }
        if !canonical(&request.operation_id) {
            return Err(refused(422, "maintenance_id", "operation_id"));
        }
        let mut hash = Sha256::new();
        hash.update(b"oboete:maintenance-request:v1\0");
        hash.update(serde_json::to_vec(&request.operation).expect("typed operation serializes"));
        hash.update([0]);
        hash.update(request.preview_key.as_bytes());
        let fingerprint = format!("{:x}", hash.finalize());
        let consent;
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for run in [&state.active, &state.last].into_iter().flatten() {
                if run.operation_id == request.operation_id {
                    return if run.fingerprint == fingerprint {
                        Ok(Self::snapshot(home, &state))
                    } else {
                        Err(refused(409, "maintenance_id_changed", "operation_id"))
                    };
                }
            }
            if state.active.is_some() {
                return Err(refused(409, "maintenance_busy", ""));
            }
            // Replay was checked first. Consume only this last prepared scope/key once.
            consent = if let Operation::Recurate { scope } = &request.operation {
                let prepared = state.last.as_mut().filter(|run| run.phase == "prepared");
                match prepared.and_then(|run| {
                    if run
                        .consent
                        .as_ref()
                        .is_some_and(|c| c.matches(scope, &request.preview_key))
                    {
                        run.consent.take()
                    } else {
                        None
                    }
                }) {
                    Some(consent) => Some(consent),
                    None => return Err(refused(409, "maintenance_stale", "preview_key")),
                }
            } else {
                None
            };
            state.active = Some(Run {
                operation_id: request.operation_id.clone(),
                kind: request.operation.kind(),
                agent: request.operation.agent(),
                phase: "running",
                stage: "checking",
                progress: Progress::default(),
                committed: false,
                result: None,
                fingerprint,
                consent: None,
            });
        }
        let id = &request.operation_id;
        let _active = ActiveRun {
            maintenance: self,
            id,
        };
        let (result, code, partial) = self.execute(caller, home, &request, consent);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(mut run) = state.active.take() {
            run.phase = if code.is_none() {
                "complete"
            } else if partial || run.committed {
                "partial"
            } else {
                "failed"
            };
            run.stage = run.phase;
            run.result = Some(json!({"code":code,"outcome":result}));
            state.last = Some(run);
        }
        Ok(Self::snapshot(home, &state))
    }
    fn execute(
        &self,
        caller: executable::CommandCaller,
        home: &Path,
        request: &StartRequest,
        consent: Option<curate::Consent>,
    ) -> (Value, Option<&'static str>, bool) {
        let id = &request.operation_id;
        match &request.operation {
            Operation::Finish { .. } => {
                match migrate::finish_report(home, &request.preview_key, caller, &mut |event| {
                    self.finish_progress(id, event)
                }) {
                    Ok(outcome) => (finish_outcome(home, &outcome), None, false),
                    Err(failure) => (
                        finish_outcome(home, &failure.outcome),
                        Some(failure_code(failure.code)),
                        failure.outcome.committed() || failure.outcome.uncertain(),
                    ),
                }
            }
            Operation::Recurate { scope } => {
                let (outcome, result) = curate::recurate_report(
                    home,
                    scope,
                    &consent.expect("recuration admission retained its consent"),
                    caller,
                    &mut |event| self.recuration_progress(id, event),
                );
                let code = match result {
                    Ok(()) if outcome.complete() => None,
                    Ok(()) => Some("maintenance_incomplete"),
                    Err(error) => Some(
                        error
                            .downcast_ref::<backup::MaintenanceCode>()
                            .map_or("maintenance_failed", native_code),
                    ),
                };
                let partial = outcome.committed();
                (
                    serde_json::to_value(outcome).expect("typed recuration outcome serializes"),
                    code,
                    partial,
                )
            }
            Operation::Rebuild { .. } | Operation::Restore { .. } => {
                let mut committed =
                    |event: &worker::MaintenanceCommit| self.native_progress(id, event);
                let result = match &request.operation {
                    Operation::Rebuild { .. } => worker::rebuild_report(
                        home,
                        Some(&request.preview_key),
                        caller,
                        &mut committed,
                    ),
                    _ => worker::restore_report(
                        home,
                        Some(&request.preview_key),
                        caller,
                        &mut committed,
                    ),
                };
                match result {
                    Ok(outcome) => (native_outcome(&outcome), None, false),
                    Err(failure) => (
                        native_outcome(&failure.outcome),
                        Some(native_code(&failure.code)),
                        failure.outcome.committed(),
                    ),
                }
            }
            Operation::Transcripts { agent } => {
                let mut committed =
                    |event: &transcript::Committed| self.transcript_progress(id, event);
                match transcript::run(
                    home,
                    &self.roots(*agent),
                    Some(&request.preview_key),
                    &mut committed,
                ) {
                    Ok(outcome) => (transcript_outcome(&outcome), None, false),
                    Err(failure) => (
                        transcript_outcome(&failure.outcome),
                        Some(failure_code(failure.code)),
                        transcript_effects(&failure.outcome),
                    ),
                }
            }
            Operation::V1 { from } => {
                let mut committed = |event: &migrate::Committed| self.migration_progress(id, event);
                match migrate::run(
                    home,
                    &from_path(home, from.as_deref()),
                    Some(&request.preview_key),
                    &mut committed,
                ) {
                    Ok(outcome) => (migration_outcome(&outcome), None, false),
                    Err(failure) => (
                        migration_outcome(&failure.outcome),
                        Some(failure_code(failure.code)),
                        migration_effects(&failure.outcome),
                    ),
                }
            }
        }
    }
    fn finish_progress(&self, id: &str, event: &migrate::FinishCommit) {
        if let migrate::FinishCommit::Import(import) = event {
            self.migration_progress(id, import);
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = state.active.as_mut().filter(|run| run.operation_id == id) {
            run.committed |= event.committed();
            if let migrate::FinishCommit::Effects(effects) = event {
                run.progress.finish_effects = Some(effects.clone());
            }
            run.stage = match event {
                migrate::FinishCommit::Effects(_) => "final_import",
                migrate::FinishCommit::Deletion { .. } => "deleting_old_files",
                migrate::FinishCommit::Import(_) => unreachable!(),
            };
            run.progress.native =
                Some(serde_json::to_value(event).expect("typed finish progress serializes"));
        }
    }
    fn recuration_progress(&self, id: &str, event: &curate::RecurationCommit) {
        if let Some(index) = event.native_index() {
            self.native_progress(id, index);
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = state.active.as_mut().filter(|run| run.operation_id == id) {
            match event {
                curate::RecurationCommit::Provider { providers } => {
                    run.committed |= providers.reserved != 0;
                    run.progress.providers = Some(providers.clone());
                }
                curate::RecurationCommit::Window { windows } => {
                    run.committed |= windows.committed != 0;
                    run.progress.windows = Some(windows.clone());
                }
                curate::RecurationCommit::Index { .. } => unreachable!(),
            }
            run.stage = "curating";
            run.progress.native =
                Some(serde_json::to_value(event).expect("typed progress serializes"));
        }
    }
    fn native_progress(&self, id: &str, event: &worker::MaintenanceCommit) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = state.active.as_mut().filter(|run| run.operation_id == id) {
            run.committed = true;
            run.stage = match event {
                worker::MaintenanceCommit::Effect { stage } => *stage,
                worker::MaintenanceCommit::Index { .. } => "indexing",
            };
            run.progress.native =
                Some(serde_json::to_value(event).expect("typed progress serializes"));
        }
    }
    fn transcript_progress(&self, id: &str, event: &transcript::Committed) {
        match event {
            transcript::Committed::V1(event) => self.migration_progress(id, event),
            transcript::Committed::Transcripts {
                agent,
                events,
                bytes,
            } => {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(run) = state.active.as_mut().filter(|run| run.operation_id == id) {
                    run.committed = true;
                    run.stage = "transcripts";
                    let progress = match agent.as_str() {
                        "claude" => &mut run.progress.claude,
                        "codex" => &mut run.progress.codex,
                        _ => return,
                    };
                    progress.events = *events;
                    progress.selected_bytes = *bytes;
                }
            }
        }
    }
    fn migration_progress(&self, id: &str, event: &migrate::Committed) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = state.active.as_mut().filter(|run| run.operation_id == id) {
            run.committed = true;
            run.stage = match event.stage {
                migrate::Stage::Settings => "settings",
                migrate::Stage::Events => "v1_events",
                migrate::Stage::Repos => "v1_repositories",
                migrate::Stage::Documents => "v1_documents",
            };
            run.progress.v1_records = event.records;
            run.progress.v1_repositories = event.repos;
            run.progress.v1_documents = event.documents;
        }
    }
}
impl Operation {
    fn validate(&self) -> Result<(), Refusal> {
        if let Self::Recurate { scope } = self {
            scope
                .validate()
                .map_err(|_| refused(422, "maintenance_scope", "operation.scope"))?;
        }
        if let Self::V1 { from: Some(from) } = self
            && (from.len() > 4096 || from.contains('\0'))
        {
            return Err(refused(422, "maintenance_source", "operation.from"));
        }
        Ok(())
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Transcripts { .. } => "transcripts",
            Self::V1 { .. } => "v1",
            Self::Rebuild { .. } => "rebuild",
            Self::Restore { .. } => "restore",
            Self::Finish { .. } => "finish",
            Self::Recurate { .. } => "recurate",
        }
    }
    fn agent(&self) -> Option<&'static str> {
        match self {
            Self::Transcripts { agent } => Some(agent_name(*agent)),
            Self::V1 { .. }
            | Self::Rebuild { .. }
            | Self::Restore { .. }
            | Self::Finish { .. }
            | Self::Recurate { .. } => None,
        }
    }
}
fn canonical(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn agent_name(agent: Option<Agent>) -> &'static str {
    match agent {
        None => "all",
        Some(Agent::Claude) => "claude",
        Some(Agent::Codex) => "codex",
    }
}
fn from_path(home: &Path, from: Option<&str>) -> PathBuf {
    from.filter(|from| !from.is_empty())
        .map_or_else(|| home.join("oboete.db"), PathBuf::from)
}
fn failure_code(code: migrate::FailureCode) -> &'static str {
    match code {
        migrate::FailureCode::Stale => "maintenance_stale",
        migrate::FailureCode::InvalidSource => "maintenance_source",
        migrate::FailureCode::InvalidConfig => "maintenance_config",
        migrate::FailureCode::Busy => "maintenance_busy",
        migrate::FailureCode::Refused => "maintenance_refused",
        migrate::FailureCode::Failed => "maintenance_failed",
    }
}
fn native_code(code: &backup::MaintenanceCode) -> &'static str {
    match code {
        backup::MaintenanceCode::Stale => "maintenance_stale",
        backup::MaintenanceCode::InvalidSource => "maintenance_source",
        backup::MaintenanceCode::InvalidConfig => "maintenance_config",
        backup::MaintenanceCode::Busy => "maintenance_busy",
        backup::MaintenanceCode::RecoveryRequired => "maintenance_recovery_required",
        backup::MaintenanceCode::Failed => "maintenance_failed",
    }
}
fn native_preview_code(error: &anyhow::Error) -> &'static str {
    error
        .downcast_ref::<backup::MaintenanceCode>()
        .map_or("maintenance_preview_failed", native_code)
}
fn native_outcome(outcome: &worker::MaintenanceOutcome) -> Value {
    serde_json::to_value(outcome).expect("typed native outcome serializes")
}
fn transcript_counts(stats: &transcript::ImportStats) -> Value {
    json!({"claude":stats.agents.get("claude"),"codex":stats.agents.get("codex")})
}
fn migration_stats(s: &migrate::Stats) -> Value {
    json!({"events":s.events,"records":s.records,"repositories":s.repos,
        "documents":s.documents,"seen":s.seen,"deleted_sessions":s.deleted.len(),
        "uncertain_identifiers":s.uncertain.len()})
}
fn migration_outcome(outcome: &migrate::Outcome) -> Value {
    let mut value = migration_stats(&outcome.stats);
    value["settings"] = json!(outcome.settings);
    value
}
fn finish_outcome(home: &Path, outcome: &migrate::FinishOutcome) -> Value {
    // Delivery can race a manual config edit after native completion. Hide labels if the
    // display rules cannot load; preserve all observed effects and counts for inspection.
    let rules = crate::redact::Rules::load(home).ok();
    let labels = |items: &[String]| {
        items
            .iter()
            .take(10)
            .map(|label| {
                rules
                    .as_ref()
                    .map(|rules| crate::redact::outbound_with(label, rules))
                    .unwrap_or_default()
                    .chars()
                    .take(256)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    let mut imported = migration_stats(&outcome.stats);
    imported["deleted_session_labels"] = json!(labels(&outcome.stats.deleted));
    imported["uncertain_identifier_labels"] = json!(labels(&outcome.stats.uncertain));
    let deletion = &outcome.deletion;
    let targets: Vec<_> = deletion
        .targets
        .iter()
        .map(|target| {
            json!({
        "label":rules.as_ref().map(|rules| crate::redact::outbound_with(&target.label, rules))
            .unwrap_or_default(),
        "state":target.state,"bytes":target.bytes})
        })
        .collect();
    json!({"operation":"finish", "import":imported,
        "effects":outcome.effects,
        "deletion":{"selected":deletion.selected,"attempted":deletion.attempted,
            "removed":deletion.removed,"failed":deletion.failed,"uncertain":deletion.uncertain,
            "authorized_bytes":deletion.authorized_bytes,"removed_bytes":deletion.removed_bytes,
            "targets":targets}})
}
fn transcript_outcome(outcome: &transcript::Outcome) -> Value {
    json!({"transcripts":transcript_counts(&outcome.stats),
           "v1":outcome.v1.as_ref().map(migration_outcome)})
}
fn migration_effects(outcome: &migrate::Outcome) -> bool {
    outcome.stats.records != 0
        || outcome.stats.repos != 0
        || outcome.stats.documents != 0
        || outcome
            .settings
            .as_ref()
            .is_some_and(|s| s.effect == migrate::SettingsEffect::Copy)
}
fn transcript_effects(outcome: &transcript::Outcome) -> bool {
    outcome.stats.agents.values().any(|s| s.events != 0)
        || outcome.v1.as_ref().is_some_and(migration_effects)
}

#[cfg(test)]
mod tests {
    fn imported_home() -> (tempfile::TempDir, Maintenance, PathBuf) {
        let (root, maintenance, home, _) = fixture();
        let operation = json!({"kind":"transcripts","agent":"codex"});
        let preview = maintenance
            .preview(
                None,
                &home,
                &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
            )
            .unwrap();
        let body = serde_json::to_vec(&json!({"operation":operation,"preview_key":preview["preview_key"],"operation_id":"1".repeat(64),"confirmed":true})).unwrap();
        assert_eq!(
            maintenance
                .start(executable::CommandCaller::Worker, &home, &body)
                .unwrap()["last"]["phase"],
            "complete"
        );
        crate::worker::run_once(&home).unwrap();
        (root, maintenance, home)
    }

    #[test]
    fn rebuild_and_restore_require_fixed_previewed_operations_and_replay_receipts() {
        let (_root, maintenance, home) = imported_home();
        for (index, kind) in ["rebuild", "restore"].into_iter().enumerate() {
            let operation = json!({"kind":kind});
            let preview = maintenance
                .preview(
                    None,
                    &home,
                    &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
                )
                .unwrap();
            assert_eq!(preview["kind"], kind);
            assert_eq!(preview["no_model_request"], true);
            assert_eq!(preview["hybrid_ready"], false);
            assert_eq!(preview["raw"]["records"], 8);
            let body = serde_json::to_vec(&json!({"operation":operation,"preview_key":preview["preview_key"],"operation_id":if index==0 {"2".repeat(64)} else {"3".repeat(64)},"confirmed":true})).unwrap();
            let receipt = maintenance
                .start(executable::CommandCaller::Worker, &home, &body)
                .unwrap();
            assert_eq!(receipt["last"]["kind"], kind);
            assert_eq!(receipt["last"]["phase"], "complete");
            assert_eq!(
                receipt["last"]["result"]["outcome"]["index"]["state"],
                "complete"
            );
            assert_eq!(receipt["last"]["result"]["outcome"]["hybrid_ready"], false);
            assert_eq!(
                maintenance
                    .start(executable::CommandCaller::Worker, &home, &body)
                    .unwrap(),
                receipt
            );
        }
    }

    #[test]
    fn rebuild_and_restore_reject_extra_sources_and_stale_saved_configuration() {
        let (_root, maintenance, home) = imported_home();
        for kind in ["rebuild", "restore"] {
            let bad =
                serde_json::to_vec(&json!({"operation":{"kind":kind,"from":"other-store.db"}}))
                    .unwrap();
            assert_eq!(
                maintenance.preview(None, &home, &bad).unwrap_err().status,
                400
            );
            let operation = json!({"kind":kind});
            let preview = maintenance
                .preview(
                    None,
                    &home,
                    &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
                )
                .unwrap();
            let raw_before = std::fs::read(home.join("raw.db")).unwrap();
            std::fs::write(
                home.join("config.toml"),
                format!("# changed saved scope {kind}\n[summary]\ncurate = false\n"),
            )
            .unwrap();
            let request = serde_json::to_vec(&json!({"operation":operation,"preview_key":preview["preview_key"],"operation_id":if kind=="rebuild" {"4".repeat(64)} else {"5".repeat(64)},"confirmed":true})).unwrap();
            let receipt = maintenance
                .start(executable::CommandCaller::Worker, &home, &request)
                .unwrap();
            assert_eq!(receipt["last"]["phase"], "failed");
            assert_eq!(receipt["last"]["result"]["code"], "maintenance_stale");
            assert_eq!(receipt["last"]["committed"], false);
            assert_eq!(std::fs::read(home.join("raw.db")).unwrap(), raw_before);
        }
    }

    use super::*;

    #[test]
    fn w5c_cached_status_and_replay_apply_current_display_rules_only_to_labels() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let canary = "synthetic-private-session";
        for operation in [
            Operation::Finish {},
            Operation::Recurate {
                scope: curate::RecurationScope::Queued {},
            },
        ] {
            std::fs::write(home.join("config.toml"), "providers = []\n").unwrap();
            let maintenance = Maintenance::with_roots(home.join("claude"), home.join("codex"));
            let id = "a".repeat(64);
            let key = "b".repeat(64);
            let mut hash = Sha256::new();
            hash.update(b"oboete:maintenance-request:v1\0");
            hash.update(serde_json::to_vec(&operation).unwrap());
            hash.update([0]);
            hash.update(key.as_bytes());
            let outcome = if matches!(operation, Operation::Finish { .. }) {
                json!({"operation":"finish","import":{"records":7,
                    "deleted_session_labels":[canary],"uncertain_identifier_labels":[canary]},
                    "deletion":{"removed":1,"targets":[{"label":canary,"state":"removed","bytes":42}]}})
            } else {
                json!({"preview":{"preview_key":key,"scope":{"kind":"queued"},
                    "plan":{"windows":1,"long_sessions":[{"label":canary,"characters":513}]}}})
            };
            let cached = json!({"code":null,"outcome":outcome});
            maintenance.state.lock().unwrap().last = Some(Run {
                operation_id: id.clone(),
                kind: if matches!(operation, Operation::Finish { .. }) {
                    "finish"
                } else {
                    "recurate"
                },
                agent: None,
                phase: "complete",
                stage: "complete",
                progress: Progress::default(),
                committed: true,
                result: Some(cached.clone()),
                fingerprint: format!("{:x}", hash.finalize()),
                consent: None,
            });
            let request = serde_json::to_vec(&json!({"operation":operation,
                "preview_key":key,"operation_id":id,"confirmed":true}))
            .unwrap();
            assert!(maintenance.show(home).to_string().contains(canary));
            for config in [
                "[redaction]\nextra_rules = [{id = 'display', regex = 'synthetic-private-session|^[0-9a-f]{64}$'}]\n",
                "invalid = [toml",
            ] {
                std::fs::write(home.join("config.toml"), config).unwrap();
                let shown = maintenance.show(home);
                assert!(
                    !shown.to_string().contains(canary),
                    "cached maintenance labels bypass current display rules"
                );
                let replay = maintenance
                    .start(executable::CommandCaller::Worker, home, &request)
                    .unwrap();
                assert_eq!(shown, replay);
                assert_eq!(shown["last"]["operation_id"], id);
                assert_eq!(shown["last"]["phase"], "complete");
                assert_eq!(shown["last"]["committed"], true);
                if matches!(operation, Operation::Finish { .. }) {
                    assert_eq!(shown["last"]["result"]["outcome"]["import"]["records"], 7);
                    assert_eq!(
                        shown["last"]["result"]["outcome"]["deletion"]["targets"][0]["bytes"],
                        42
                    );
                } else {
                    assert_eq!(
                        shown["last"]["result"]["outcome"]["preview"]["preview_key"],
                        key
                    );
                    assert_eq!(
                        shown["last"]["result"]["outcome"]["preview"]["plan"]["long_sessions"][0]["characters"],
                        513
                    );
                }
                assert_eq!(
                    maintenance
                        .state
                        .lock()
                        .unwrap()
                        .last
                        .as_ref()
                        .unwrap()
                        .result
                        .as_ref(),
                    Some(&cached)
                );
            }
            assert_eq!(std::fs::read_dir(home).unwrap().count(), 1);
        }
    }

    fn fixture() -> (tempfile::TempDir, Maintenance, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let codex = root.path().join("codex/sessions");
        std::fs::create_dir_all(&codex).unwrap();
        let source = codex.join("rollout-native.jsonl");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/testdata/transcripts/codex-basic.jsonl"),
            &source,
        )
        .unwrap();
        let maintenance = Maintenance::with_roots(root.path().join("claude/projects"), codex);
        let home = root.path().join("memory");
        (root, maintenance, home, source)
    }

    #[test]
    fn confirmed_import_and_exact_id_replay_keep_one_committed_result() {
        let (_root, maintenance, home, source) = fixture();
        let operation = json!({"kind":"transcripts","agent":"codex"});
        let preview = maintenance
            .preview(
                None,
                &home,
                &serde_json::to_vec(&json!({
                    "operation":operation
                }))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(preview["candidates"]["codex"]["events"], 8);
        assert!(!home.exists());
        let request = serde_json::to_vec(&json!({"operation":operation,
            "preview_key":preview["preview_key"],"operation_id":"a".repeat(64),
            "confirmed":true}))
        .unwrap();
        let result = maintenance
            .start(executable::CommandCaller::Worker, &home, &request)
            .unwrap();
        assert_eq!(result["last"]["phase"], "complete");
        assert_eq!(
            result["last"]["result"]["outcome"]["transcripts"]["codex"]["events"],
            8
        );
        assert_eq!(result["last"]["committed"], true);
        let raw = crate::raw::read_only(&home).unwrap().unwrap();
        let before: i64 = raw
            .conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .unwrap();
        drop(raw);
        // A replay must return its receipt without reparsing a now-unreadable source.
        std::fs::write(source, "not a transcript").unwrap();
        assert_eq!(
            maintenance
                .start(executable::CommandCaller::Worker, &home, &request)
                .unwrap(),
            result
        );
        let raw = crate::raw::read_only(&home).unwrap().unwrap();
        let after: i64 = raw
            .conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
        assert!(!home.join("providers.db").exists());
    }

    #[test]
    fn a_changed_source_returns_a_bounded_stale_receipt_before_import() {
        let (_root, maintenance, home, source) = fixture();
        let operation = json!({"kind":"transcripts","agent":"codex"});
        let preview = maintenance
            .preview(
                None,
                &home,
                &serde_json::to_vec(&json!({
                    "operation":operation
                }))
                .unwrap(),
            )
            .unwrap();
        std::fs::write(source, "unconfirmed source contents").unwrap();
        let request = serde_json::to_vec(&json!({"operation":operation,
            "preview_key":preview["preview_key"],"operation_id":"b".repeat(64),
            "confirmed":true}))
        .unwrap();
        let result = maintenance
            .start(executable::CommandCaller::Worker, &home, &request)
            .unwrap();
        assert_eq!(result["last"]["phase"], "failed");
        assert_eq!(result["last"]["result"]["code"], "maintenance_stale");
        assert_eq!(result["last"]["committed"], false);
        assert!(!home.join("raw.db").exists());
        assert!(!home.join("config.toml").exists());
        assert!(!result.to_string().contains("unconfirmed source contents"));
    }

    fn panic_after_parse(remaining: usize) {
        transcript::AFTER_PARSE.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                if remaining == 0 {
                    panic!("controlled native unwind");
                }
                panic_after_parse(remaining - 1);
            }));
        });
    }

    #[test]
    fn a_native_unwind_keeps_unknown_progress_and_releases_admission() {
        for earlier_commit in [false, true] {
            let (_root, maintenance, home, source) = fixture();
            if earlier_commit {
                let text = std::fs::read_to_string(&source).unwrap();
                std::fs::write(
                    source.with_file_name("rollout-z-new.jsonl"),
                    text.replace(
                        "22222222-2222-4222-8222-222222222222",
                        "44444444-4444-4444-8444-444444444444",
                    )
                    .replace(
                        "33333333-3333-4333-8333-333333333333",
                        "55555555-5555-4555-8555-555555555555",
                    ),
                )
                .unwrap();
            }
            let preview_request = serde_json::to_vec(&json!({
                "operation":{"kind":"transcripts","agent":"codex"}
            }))
            .unwrap();
            let preview = maintenance.preview(None, &home, &preview_request).unwrap();
            let request = |key: &Value, id: char| {
                serde_json::to_vec(&json!({"operation":{"kind":"transcripts","agent":"codex"},
                    "preview_key":key,"operation_id":id.to_string().repeat(64),"confirmed":true}))
                .unwrap()
            };
            let original = request(&preview["preview_key"], 'c');
            // The two preview parses and first execution parse precede the real first commit.
            panic_after_parse(if earlier_commit { 3 } else { 0 });
            let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                maintenance.start(executable::CommandCaller::Worker, &home, &original)
            }));
            assert!(unwind.is_err());
            let shown = maintenance.show(&home);
            assert!(shown["active"].is_null());
            assert_eq!(shown["last"]["phase"], "unknown");
            assert_eq!(shown["last"]["result"]["code"], "maintenance_unknown");
            assert!(shown["last"]["result"]["outcome"].is_null());
            assert_eq!(shown["last"]["committed"], earlier_commit);
            assert_eq!(
                shown["last"]["progress"]["codex"]["events"],
                if earlier_commit { 8 } else { 0 }
            );
            assert_eq!(
                maintenance
                    .start(executable::CommandCaller::Worker, &home, &original)
                    .unwrap(),
                shown
            );
            let fresh = maintenance.preview(None, &home, &preview_request).unwrap();
            let finished = maintenance
                .start(
                    executable::CommandCaller::Worker,
                    &home,
                    &request(&fresh["preview_key"], 'd'),
                )
                .unwrap();
            assert_eq!(finished["last"]["phase"], "complete");
            assert_eq!(finished["last"]["progress"]["codex"]["events"], 8);
            let raw = crate::raw::read_only(&home).unwrap().unwrap();
            let records: i64 = raw
                .conn
                .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
                .unwrap();
            assert_eq!(records, if earlier_commit { 16 } else { 8 });
        }
    }

    #[test]
    fn a_normal_old_guard_cannot_clear_a_reused_id() {
        let (_root, maintenance, home, _source) = fixture();
        let maintenance = std::sync::Arc::new(maintenance);
        let prepare = |id: char| {
            let operation = json!({"kind":"transcripts","agent":"codex"});
            let preview = maintenance
                .preview(
                    None,
                    &home,
                    &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
                )
                .unwrap();
            serde_json::to_vec(
                &json!({"operation":operation,"preview_key":preview["preview_key"],
                "operation_id":id.to_string().repeat(64),"confirmed":true}),
            )
            .unwrap()
        };
        maintenance
            .start(executable::CommandCaller::Worker, &home, &prepare('c'))
            .unwrap();
        let old_id = "c".repeat(64);
        // Delay the first normal guard drop until another receipt evicts its ID.
        let old = ActiveRun {
            maintenance: &maintenance,
            id: &old_id,
        };
        maintenance
            .start(executable::CommandCaller::Worker, &home, &prepare('d'))
            .unwrap();
        let reused = prepare('c');
        let (ready, observed) = std::sync::mpsc::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let running = maintenance.clone();
        let running_home = home.clone();
        let thread = std::thread::spawn(move || {
            transcript::AFTER_PARSE.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    ready.send(()).unwrap();
                    waiting.recv().unwrap();
                }));
            });
            running.start(executable::CommandCaller::Worker, &running_home, &reused)
        });
        observed
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap();
        drop(old);
        let while_running = maintenance.show(&home);
        release.send(()).unwrap();
        let finished = thread.join().unwrap().unwrap();
        assert_eq!(while_running["active"]["operation_id"], old_id);
        assert_eq!(while_running["active"]["phase"], "running");
        assert_eq!(finished["last"]["phase"], "complete");
    }

    #[test]
    fn a_missing_default_v1_source_is_distinct_and_creates_no_store() {
        let (_root, maintenance, home, _source) = fixture();
        for from in [Value::Null, json!("")] {
            let body = serde_json::to_vec(&json!({"operation":{"kind":"v1","from":from}})).unwrap();
            let refused = maintenance.preview(None, &home, &body).unwrap_err();
            assert_eq!(refused.code, "maintenance_source_missing");
            assert!(!home.exists());
            assert_eq!(
                maintenance.show(&home),
                json!({"available":false,"active":null,"last":null})
            );
        }
        let body = serde_json::to_vec(
            &json!({"operation":{"kind":"v1","from":home.join("explicit-missing.db")}}),
        )
        .unwrap();
        assert_eq!(
            maintenance.preview(None, &home, &body).unwrap_err().code,
            "maintenance_preview_failed"
        );
        assert!(!home.exists());
    }
}
