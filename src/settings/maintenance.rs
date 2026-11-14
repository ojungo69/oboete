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
    /// The last transcript preview's plan, kept for its confirm and taken by it once (#947).
    transcripts: Option<transcript::PreviewPlan>,
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
                let plan =
                    transcript::preview_plan(home, &self.roots(agent)).map_err(|_| failed())?;
                let preview = &plan.preview;
                let shown = json!({"kind":"transcripts", "agent":agent_name(agent),
                    "preview_key":preview.key, "candidates":transcript_counts(&preview.candidates),
                    "v1":preview.v1.as_ref().map(|p| json!({"candidates":p.candidates,"settings":p.settings})),
                    "no_model_request":true});
                // Kept for its confirm, which imports of what this bound and does not read it
                // all again. A newer preview's plan takes its place.
                self.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .transcripts = Some(plan);
                Ok(shown)
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
        let plan;
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
            // The same once for a transcript preview's plan: the confirm of its key takes it.
            // With none under the key the confirm is stale, and its receipt says so.
            plan = if matches!(request.operation, Operation::Transcripts { .. })
                && (state.transcripts.as_ref())
                    .is_some_and(|plan| plan.preview.key == request.preview_key)
            {
                state.transcripts.take()
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
        let (result, code, partial) = self.execute(caller, home, &request, consent, plan);
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
        plan: Option<transcript::PreviewPlan>,
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
                // No plan under the key: another preview since, or the viewer started again.
                let Some(plan) = plan else {
                    let nothing = transcript::Outcome::default();
                    let stale = failure_code(crate::preview::FailureCode::Stale);
                    return (transcript_outcome(&nothing), Some(stale), false);
                };
                let mut committed =
                    |event: &transcript::Committed| self.transcript_progress(id, event);
                match transcript::run(home, &self.roots(*agent), Some(plan), &mut committed) {
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
fn failure_code(code: crate::preview::FailureCode) -> &'static str {
    match code {
        crate::preview::FailureCode::Stale => "maintenance_stale",
        crate::preview::FailureCode::InvalidSource => "maintenance_source",
        crate::preview::FailureCode::InvalidConfig => "maintenance_config",
        crate::preview::FailureCode::Busy => "maintenance_busy",
        crate::preview::FailureCode::Refused => "maintenance_refused",
        crate::preview::FailureCode::Failed => "maintenance_failed",
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
mod tests;
