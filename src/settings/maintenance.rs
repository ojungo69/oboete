//! Bounded viewer import previews and one active/last receipt; no scheduler or persisted jobs.
use super::{Refusal, refused};
use crate::{migrate, transcript};
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
    fn snapshot(state: &State) -> Value {
        json!({"available": state.active.is_some() || state.last.is_some(),
               "active": state.active, "last": state.last})
    }
    /// No config, store or source read: safe while an import holds native database locks.
    pub(crate) fn show(&self) -> Value {
        Self::snapshot(&self.state.lock().unwrap_or_else(PoisonError::into_inner))
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
    pub(crate) fn preview(&self, home: &Path, body: &[u8]) -> Result<Value, Refusal> {
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
            Operation::Transcripts { agent } => {
                let preview =
                    transcript::preview(home, &self.roots(agent)).map_err(|_| failed())?;
                Ok(json!({"kind":"transcripts", "agent":agent_name(agent),
                    "preview_key":preview.key, "candidates":transcript_counts(&preview.candidates),
                    "v1":preview.v1.map(|p| json!({"candidates":p.candidates,"settings":p.settings})),
                    "no_model_request":true}))
            }
            Operation::V1 { from } => {
                let from = from_path(home, from.as_deref());
                let preview = migrate::preview(home, &from).map_err(|_| failed())?;
                let rules = crate::redact::Rules::load(home).map_err(|_| failed())?;
                let label = crate::redact::outbound_with(&from.to_string_lossy(), &rules);
                Ok(json!({"kind":"v1", "preview_key":preview.key,
                    "source":label, "candidates":preview.candidates,"settings":preview.settings,
                    "no_model_request":true}))
            }
        }
    }
    pub(crate) fn start(&self, home: &Path, body: &[u8]) -> Result<Value, Refusal> {
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
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for run in [&state.active, &state.last].into_iter().flatten() {
                if run.operation_id == request.operation_id {
                    return if run.fingerprint == fingerprint {
                        Ok(Self::snapshot(&state))
                    } else {
                        Err(refused(409, "maintenance_id_changed", "operation_id"))
                    };
                }
            }
            if state.active.is_some() {
                return Err(refused(409, "maintenance_busy", ""));
            }
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
            });
        }
        let id = &request.operation_id;
        let (result, code, partial) = match request.operation {
            Operation::Transcripts { agent } => {
                let mut committed = |event: &transcript::Committed| match event {
                    transcript::Committed::V1(event) => self.migration_progress(id, event),
                    transcript::Committed::Transcripts {
                        agent,
                        events,
                        bytes,
                    } => {
                        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                        if let Some(run) =
                            state.active.as_mut().filter(|run| run.operation_id == *id)
                        {
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
                };
                match transcript::run(
                    home,
                    &self.roots(agent),
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
        };
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
        Ok(Self::snapshot(&state))
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
        }
    }
    fn agent(&self) -> Option<&'static str> {
        match self {
            Self::Transcripts { agent } => Some(agent_name(*agent)),
            Self::V1 { .. } => None,
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
fn transcript_counts(stats: &transcript::ImportStats) -> Value {
    json!({"claude":stats.agents.get("claude"),"codex":stats.agents.get("codex")})
}
fn migration_outcome(outcome: &migrate::Outcome) -> Value {
    let s = &outcome.stats;
    json!({"events":s.events,"records":s.records,"repositories":s.repos,
        "documents":s.documents,"seen":s.seen,"deleted_sessions":s.deleted.len(),
        "uncertain_identifiers":s.uncertain.len(),"settings":outcome.settings})
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
    use super::*;

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
        let result = maintenance.start(&home, &request).unwrap();
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
        assert_eq!(maintenance.start(&home, &request).unwrap(), result);
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
        let result = maintenance.start(&home, &request).unwrap();
        assert_eq!(result["last"]["phase"], "failed");
        assert_eq!(result["last"]["result"]["code"], "maintenance_stale");
        assert_eq!(result["last"]["committed"], false);
        assert!(!home.join("raw.db").exists());
        assert!(!home.join("config.toml").exists());
        assert!(!result.to_string().contains("unconfirmed source contents"));
    }
}
