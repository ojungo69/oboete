//! Confirmed fixed-agent integrations and one bounded active/last receipt.
mod files;
mod plan;

use crate::executable::{CommandCaller, CommandHome};
use crate::settings::Refusal;
use plan::{Action, Agent, Code, Component, Effect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewRequest {
    action: Action,
    agents: Vec<Agent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    action: Action,
    agents: Vec<Agent>,
    preview_key: String,
    operation_id: String,
    confirmed: bool,
}

fn refusal(status: u16, code: &'static str) -> Refusal {
    Refusal {
        status,
        code,
        field: String::new(),
    }
}

fn bad_request() -> Refusal {
    refusal(400, "bad_request")
}

fn selected(mut agents: Vec<Agent>) -> Result<Vec<Agent>, Refusal> {
    if agents.is_empty() || agents.len() > 7 {
        return Err(bad_request());
    }
    agents.sort();
    if agents.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(bad_request());
    }
    Ok(agents)
}

fn id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Backup {
    Create,
    Kept,
    None,
}

#[derive(Clone, Serialize)]
struct PublicStep {
    component: Component,
    effect: Effect,
    backup: Backup,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<Code>,
}

#[derive(Serialize)]
struct AgentPreview {
    agent: Agent,
    steps: Vec<PublicStep>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Running,
    Complete,
    Partial,
    Failed,
    Stale,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Stage,
    Backup,
    Write,
    Delete,
    NativeRemove,
    NativeAdd,
    Readback,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Prepared,
    Committed,
    Noop,
    Manual,
    Skipped,
    Failed,
    Stale,
    Unknown,
}

#[derive(Clone, Serialize)]
struct ReceiptStep {
    #[serde(flatten)]
    step: PublicStep,
    kind: Kind,
    outcome: Outcome,
}

#[derive(Clone, Serialize)]
struct AgentReceipt {
    agent: Agent,
    steps: Vec<ReceiptStep>,
}

#[derive(Clone, Serialize)]
struct Receipt {
    operation_id: String,
    action: Action,
    phase: Phase,
    agents: Vec<AgentReceipt>,
    live_verified: bool,
    activation: &'static str,
}

#[derive(Clone)]
struct Run {
    fingerprint: String,
    receipt: Receipt,
}

#[derive(Default)]
struct State {
    active: Option<Run>,
    last: Option<Run>,
}

#[derive(Default)]
pub(crate) struct Agents {
    state: Mutex<State>,
}

struct Active<'a> {
    agents: &'a Agents,
    operation_id: &'a str,
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut state = self
            .agents
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state
            .active
            .as_ref()
            .is_some_and(|run| run.receipt.operation_id == self.operation_id)
            && let Some(mut run) = state.active.take()
        {
            run.receipt.phase = Phase::Unknown;
            state.last = Some(run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn w6a_expected_alias_unlink_keeps_referent_and_fresh_resource_state() {
        let root = tempfile::tempdir().unwrap();
        let referent = root.path().join("owned-plugin");
        let alias = root.path().join("plugin");
        let intermediate = root.path().join("intermediate");
        std::fs::write(&referent, "owned plugin bytes").unwrap();
        std::os::unix::fs::symlink(&referent, &intermediate).unwrap();
        std::os::unix::fs::symlink(&intermediate, &alias).unwrap();
        let original = files::input(&alias).unwrap();
        let mut resources = Resources::default();
        resources.observe(&alias);
        std::fs::remove_file(&alias).unwrap();
        resources.refresh(&alias, true).unwrap();
        assert!(resources.unchanged());
        assert!(!alias.exists());
        assert_eq!(
            std::fs::read_to_string(referent).unwrap(),
            "owned plugin bytes"
        );
        std::fs::remove_file(&intermediate).unwrap();
        std::os::unix::fs::symlink(root.path().join("unexpected"), &intermediate).unwrap();
        assert!(
            !original.after_unlink(&files::input(&alias).unwrap()),
            "expected leaf deletion must not admit another alias changing"
        );
    }

    #[test]
    #[cfg(unix)]
    fn w6a_native_readback_keeps_the_original_physical_route() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let other = root.path().join("other");
        let alias = root.path().join("alias");
        std::fs::write(&first, "{}").unwrap();
        std::fs::write(&other, "{}").unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let mut resources = Resources::default();
        resources.observe(&alias);
        // An agent may replace the contents/inode at the agreed target.
        let staged = root.path().join("new");
        std::fs::write(&staged, "{\"native\":true}").unwrap();
        std::fs::rename(staged, &first).unwrap();
        resources.refresh(&alias, false).unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&other, &alias).unwrap();
        assert!(
            resources.refresh(&alias, false).is_err(),
            "readback accepted a different physical target"
        );
        assert_eq!(
            resources.get(&alias).unwrap().text.as_deref(),
            Some("{\"native\":true}")
        );
        assert!(!resources.unchanged());
        assert_eq!(std::fs::read_to_string(other).unwrap(), "{}");
    }

    #[test]
    fn w6a_same_id_replays_an_established_result_while_saving_is_locked() {
        let agents = Agents::default();
        let saving = Mutex::new(());
        let _guard = saving.lock().unwrap();
        let operation_id = "a".repeat(64);
        let key = "b".repeat(64);
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!({
                    "action":"wire","agents":["codex"],"preview_key":key
                }))
                .unwrap()
            )
        );
        assert!(
            agents
                .replay(&operation_id, &fingerprint)
                .unwrap()
                .is_none()
        );
        let receipt = Receipt {
            operation_id: operation_id.clone(),
            action: Action::Wire,
            phase: Phase::Complete,
            agents: vec![],
            live_verified: false,
            activation: "next_session",
        };
        // A has completed since B's first lookup. B must use the receipt before
        // collecting resources again, even if the supplied caller/home is unusable.
        agents.state.lock().unwrap().last = Some(Run {
            fingerprint,
            receipt: receipt.clone(),
        });
        let body = serde_json::to_vec(&json!({"action":"wire","agents":["codex"],
            "preview_key":key,"operation_id":operation_id,"confirmed":true}))
        .unwrap();
        assert_eq!(
            agents
                .start(None, Path::new("unused"), &saving, &body)
                .unwrap(),
            json!(receipt)
        );
    }
}

impl Agents {
    pub(crate) fn show(&self) -> Value {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        json!({"active":state.active.as_ref().map(|run| &run.receipt),
            "last":state.last.as_ref().map(|run| &run.receipt)})
    }

    fn replay(&self, operation_id: &str, fingerprint: &str) -> Result<Option<Value>, Refusal> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = [state.active.as_ref(), state.last.as_ref()]
            .into_iter()
            .flatten()
            .find(|run| run.receipt.operation_id == operation_id)
        {
            return if run.fingerprint == fingerprint {
                Ok(Some(json!(run.receipt)))
            } else {
                Err(refusal(409, "setup_replay_conflict"))
            };
        }
        if state.active.is_some() {
            return Err(refusal(503, "setup_busy"));
        }
        Ok(None)
    }

    pub(crate) fn preview(
        &self,
        caller: Option<CommandCaller>,
        home: &Path,
        body: &[u8],
    ) -> Result<Value, Refusal> {
        let request: PreviewRequest = serde_json::from_slice(body).map_err(|_| bad_request())?;
        let agents = selected(request.agents)?;
        let caller = caller.ok_or_else(|| refusal(422, "setup_unavailable"))?;
        let bundle = Bundle::build(home, caller, request.action, &agents)?;
        Ok(
            json!({"preview_key":bundle.key,"action":request.action,"agents":bundle.preview,
            "live_verified":false,"activation":"next_session"}),
        )
    }

    pub(crate) fn start(
        &self,
        caller: Option<CommandCaller>,
        home: &Path,
        saving: &Mutex<()>,
        body: &[u8],
    ) -> Result<Value, Refusal> {
        let request: StartRequest = serde_json::from_slice(body).map_err(|_| bad_request())?;
        let agents = selected(request.agents)?;
        if !request.confirmed || !id(&request.preview_key) || !id(&request.operation_id) {
            return Err(bad_request());
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!({
                    "action":request.action,"agents":agents,"preview_key":request.preview_key
                }))
                .map_err(|_| bad_request())?
            )
        );
        if let Some(receipt) = self.replay(&request.operation_id, &fingerprint)? {
            return Ok(receipt);
        }
        let _saving = saving.try_lock().map_err(|_| refusal(503, "setup_busy"))?;
        if let Some(receipt) = self.replay(&request.operation_id, &fingerprint)? {
            return Ok(receipt);
        }
        let caller = caller.ok_or_else(|| refusal(422, "setup_unavailable"))?;
        let _directory = super::DiagnosticDirectory::open(home)
            .map_err(|_| refusal(422, "setup_unavailable"))?;
        let admitted = CommandHome::new(home, caller).map_err(|_| refusal(409, "setup_stale"))?;
        let _integration =
            super::integration_lock(home).map_err(|_| refusal(422, "setup_unavailable"))?;
        // This is existing coordination state, not an agent-file edit or a first-run preset.
        let _configuration =
            crate::settings::config_lock(home).map_err(|_| refusal(422, "setup_unavailable"))?;
        admitted
            .check(home)
            .map_err(|_| refusal(409, "setup_stale"))?;
        let mut bundle = Bundle::build(home, caller, request.action, &agents)?;
        if bundle.key != request.preview_key || !bundle.unchanged() {
            return Err(refusal(409, "setup_stale"));
        }
        let receipt = Receipt {
            operation_id: request.operation_id.clone(),
            action: request.action,
            phase: Phase::Running,
            agents: agents
                .iter()
                .copied()
                .map(|agent| AgentReceipt {
                    agent,
                    steps: Vec::new(),
                })
                .collect(),
            live_verified: false,
            activation: "next_session",
        };
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active = Some(Run {
            fingerprint,
            receipt,
        });
        let _active = Active {
            agents: self,
            operation_id: &request.operation_id,
        };
        let phase = bundle.apply(home, &admitted, |agent, step| {
            self.record(&request.operation_id, agent, step)
        });
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut run = state
            .active
            .take()
            .expect("this operation holds the saving guard");
        run.receipt.phase = phase;
        let answer = json!(run.receipt);
        state.last = Some(run);
        Ok(answer)
    }

    fn record(&self, id: &str, agent: Agent, step: ReceiptStep) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = state
            .active
            .as_mut()
            .filter(|run| run.receipt.operation_id == id)
            && let Some(row) = run.receipt.agents.iter_mut().find(|row| row.agent == agent)
        {
            row.steps.push(step);
        }
    }
}

fn frame(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

struct BinaryPin {
    path: PathBuf,
    metadata: String,
    witness: String,
}
impl BinaryPin {
    fn metadata(path: &Path, file: &std::fs::File) -> anyhow::Result<String> {
        let value = file.metadata()?;
        anyhow::ensure!(value.is_file(), "launcher is not a regular file");
        Ok(format!(
            "{}:{}:{}:{:?}:{:?}",
            crate::db::store_file_from(file)?,
            files::mode(&value),
            value.len(),
            value.modified()?,
            super::diagnostic_canonicalize(path)?
        ))
    }
    fn read(path: &Path) -> anyhow::Result<Self> {
        const LIMIT: u64 = 1024 * 1024 * 1024;
        let path = std::path::absolute(path)?;
        let mut file = super::diagnostic_read_file(&path)?;
        anyhow::ensure!(file.metadata()?.len() <= LIMIT, "launcher is too large");
        let before = Self::metadata(&path, &file)?;
        let mut hash = Sha256::new();
        frame(&mut hash, path.as_os_str().as_encoded_bytes());
        frame(&mut hash, before.as_bytes());
        let expected = file.metadata()?.len();
        hash.update(expected.to_le_bytes());
        let mut total = 0u64;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            anyhow::ensure!(total <= LIMIT, "launcher grew while reading");
            hash.update(&chunk[..count]);
        }
        anyhow::ensure!(
            total == expected && before == Self::metadata(&path, &file)?,
            "launcher changed"
        );
        let named = super::diagnostic_read_file(&path)?;
        anyhow::ensure!(
            before == Self::metadata(&path, &named)?,
            "launcher binding changed"
        );
        Ok(Self {
            path,
            metadata: before,
            witness: format!("{:x}", hash.finalize()),
        })
    }
    fn unchanged(&self) -> bool {
        super::diagnostic_read_file(&self.path).is_ok_and(|file| {
            Self::metadata(&self.path, &file).is_ok_and(|value| value == self.metadata)
        })
    }
    fn content_unchanged(&self) -> bool {
        Self::read(&self.path).is_ok_and(|now| now.witness == self.witness)
    }
}

#[derive(Default)]
struct Resources {
    known: BTreeMap<PathBuf, files::Input>,
    unavailable: BTreeSet<PathBuf>,
}
impl Resources {
    fn observe(&mut self, path: &Path) -> plan::Input {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        if !self.known.contains_key(&absolute) && !self.unavailable.contains(&absolute) {
            match files::input(&absolute) {
                Ok(input) => {
                    self.known.insert(absolute.clone(), input);
                }
                Err(_) => {
                    self.unavailable.insert(absolute.clone());
                }
            }
        }
        match self.known.get(&absolute) {
            Some(input) => plan::Input {
                path: path.to_owned(),
                text: input.text.clone(),
                witness: input.witness.clone(),
                available: true,
            },
            None => plan::Input {
                path: path.to_owned(),
                text: None,
                witness: format!(
                    "{:x}",
                    Sha256::digest(absolute.as_os_str().as_encoded_bytes())
                ),
                available: false,
            },
        }
    }
    fn get(&self, path: &Path) -> Option<&files::Input> {
        self.known.get(&std::path::absolute(path).ok()?)
    }
    fn refresh(&mut self, path: &Path, unlinked: bool) -> anyhow::Result<()> {
        let absolute = std::path::absolute(path)?;
        let input = files::input(&absolute)?;
        anyhow::ensure!(
            self.known.get(&absolute).is_some_and(|before| if unlinked {
                before.after_unlink(&input)
            } else {
                before.same_route(&input)
            }),
            "setup input route changed"
        );
        self.known.insert(absolute.clone(), input);
        self.unavailable.remove(&absolute);
        Ok(())
    }
    fn unchanged(&self) -> bool {
        self.known.values().all(files::Input::unchanged)
            && self
                .unavailable
                .iter()
                .all(|path| files::input(path).is_err())
    }
}

impl Agent {
    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Agy => "agy",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Cursor => "cursor",
        }
    }
    fn components(self) -> &'static [Component] {
        match self {
            Self::Codex => &[Component::Hooks, Component::Trust, Component::Mcp],
            Self::Opencode => &[Component::Plugin, Component::Mcp],
            Self::Pi => &[Component::Extension],
            _ => &[Component::Hooks, Component::Mcp],
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        super::BACKUP_SUFFIX
    ))
}

fn inputs(agent: Agent, resources: &mut Resources) -> plan::Inputs {
    use plan::FileId;
    let paths = match agent {
        Agent::Claude => {
            let [legacy, current] = super::claude_mcp_paths();
            let old = resources.observe(&legacy);
            let chosen = if !old.available || old.text.is_some() {
                legacy
            } else {
                current
            };
            vec![
                (FileId::Hooks, super::claude_settings_file()),
                (FileId::Mcp, chosen),
            ]
        }
        Agent::Codex => vec![
            (FileId::Hooks, super::codex_home().join("hooks.json")),
            (FileId::Mcp, super::codex_home().join("config.toml")),
        ],
        Agent::Grok => vec![
            (FileId::Hooks, crate::hook::grok_hooks_file()),
            (FileId::Mcp, super::grok_config_file()),
        ],
        Agent::Agy => vec![
            (FileId::Hooks, super::agy_dir().join("config/hooks.json")),
            (FileId::Mcp, super::agy_dir().join("config/mcp_config.json")),
        ],
        Agent::Opencode => vec![
            (
                FileId::Plugin,
                super::opencode_dir().join("plugins/oboete.js"),
            ),
            (FileId::Mcp, super::opencode_dir().join("opencode.json")),
            (FileId::Jsonc, super::opencode_dir().join("opencode.jsonc")),
        ],
        Agent::Pi => vec![(
            FileId::Extension,
            super::pi_dir().join("extensions/oboete.ts"),
        )],
        Agent::Cursor => vec![
            (FileId::Hooks, super::cursor_dir().join("hooks.json")),
            (FileId::Mcp, super::cursor_dir().join("mcp.json")),
        ],
    };
    paths
        .into_iter()
        .map(|(id, path)| (id, resources.observe(&path)))
        .collect()
}

fn notice(agent: Agent, effect: Effect, code: Code) -> AgentPreview {
    AgentPreview {
        agent,
        steps: agent
            .components()
            .iter()
            .copied()
            .map(|component| PublicStep {
                component,
                effect,
                backup: Backup::None,
                code: Some(code),
            })
            .collect(),
    }
}

struct Bundle {
    key: String,
    preview: Vec<AgentPreview>,
    plans: Vec<Option<plan::Plan>>,
    resources: Resources,
    home: PathBuf,
    home_identity: String,
    executable: BinaryPin,
    claude: Option<BinaryPin>,
    path: Option<std::ffi::OsString>,
}
impl Bundle {
    fn build(
        home: &Path,
        caller: CommandCaller,
        action: Action,
        agents: &[Agent],
    ) -> Result<Self, Refusal> {
        let directory = super::DiagnosticDirectory::open(home)
            .map_err(|_| refusal(422, "setup_unavailable"))?;
        CommandHome::new(home, caller).map_err(|_| refusal(409, "setup_stale"))?;
        let command = super::readiness::current_command(home, super::diagnostic_canonicalize)
            .ok_or_else(|| refusal(422, "setup_unavailable"))?;
        let executable = BinaryPin::read(Path::new(&command.exe))
            .map_err(|_| refusal(422, "setup_unavailable"))?;
        let mut resources = Resources::default();
        if !resources.observe(&home.join("config.toml")).available {
            return Err(refusal(422, "setup_unavailable"));
        }
        let inventory = json!(super::readiness(home));
        let mut claude = None;
        if agents.contains(&Agent::Claude)
            && let Some(path) = std::env::var_os("PATH")
                .into_iter()
                .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                .flat_map(|dir| super::launcher_files(&dir, "claude"))
                .find(|path| {
                    super::diagnostic_metadata(path).is_ok_and(|metadata| metadata.is_file())
                })
        {
            claude = BinaryPin::read(&path).ok();
        }
        let mut preview = Vec::new();
        let mut plans = Vec::new();
        let mut failures = Vec::new();
        for &agent in agents {
            let row = inventory["agents"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["agent"] == agent.id()));
            if !matches!(agent, Agent::Claude | Agent::Codex | Agent::Grok) {
                let found = row.is_some_and(|row| {
                    row["directory_found"] == true || row["launch_file_found"] == true
                });
                if !found {
                    let missing = row.is_some_and(|row| {
                        row["directory_found"] == false && row["launch_file_found"] == false
                    });
                    preview.push(notice(
                        agent,
                        if missing {
                            Effect::Skipped
                        } else {
                            Effect::Unavailable
                        },
                        if missing {
                            Code::AgentMissing
                        } else {
                            Code::UnavailableFile
                        },
                    ));
                    plans.push(None);
                    continue;
                }
            }
            let read = inputs(agent, &mut resources);
            let mut plan = match plan::plan_agent(agent, &command, action == Action::Unwire, &read)
            {
                Ok(plan) => plan,
                Err(error) => {
                    let code = match error {
                        plan::PlanError::UnsupportedCommand => Code::UnsupportedCommand,
                        plan::PlanError::MissingInput(file) => {
                            failures.push(format!("missing:{file:?}"));
                            Code::UnavailableFile
                        }
                        plan::PlanError::InvalidInput(file) => {
                            failures.push(format!("invalid:{file:?}"));
                            Code::UnavailableFile
                        }
                    };
                    preview.push(notice(agent, Effect::Unavailable, code));
                    plans.push(None);
                    continue;
                }
            };
            if plan.claude_mcp.is_some() && claude.is_none() {
                plan.claude_mcp = None;
                for step in &mut plan.steps {
                    if step.component == Component::Mcp && step.effect == Effect::NativeCommand {
                        step.effect = Effect::Skipped;
                        step.code = Some(Code::LauncherMissing);
                    }
                }
            }
            if plan
                .claude_mcp
                .as_ref()
                .zip(claude.as_ref())
                .is_some_and(|(native, pin)| {
                    native
                        .wanted_entry
                        .as_ref()
                        .into_iter()
                        .any(|entry| !super::claude_mcp_arguments_fit(&pin.path, entry))
                })
            {
                plan.claude_mcp = None;
                for step in &mut plan.steps {
                    if step.component == Component::Mcp && step.effect == Effect::NativeCommand {
                        step.effect = Effect::Unavailable;
                        step.code = Some(Code::McpArgumentsTooLarge);
                    }
                }
            }
            let mut backups = BTreeMap::new();
            for path in plan
                .edits
                .iter()
                .map(|edit| &edit.path)
                .chain(plan.claude_mcp.iter().map(|native| &native.path))
            {
                let before = resources.observe(path);
                let backup = resources.observe(&backup_path(path));
                if !backup.available {
                    backups.insert(path.clone(), None);
                } else {
                    backups.insert(
                        path.clone(),
                        Some(
                            if resources
                                .get(&backup.path)
                                .is_some_and(files::Input::present_entry)
                            {
                                Backup::Kept
                            } else if before.text.is_some() {
                                Backup::Create
                            } else {
                                Backup::None
                            },
                        ),
                    );
                }
            }
            if backups.values().any(Option::is_none) {
                preview.push(notice(agent, Effect::Unavailable, Code::UnavailableFile));
                plans.push(None);
                continue;
            }
            let steps = plan
                .steps
                .iter()
                .map(|step| {
                    let path = plan
                        .edits
                        .iter()
                        .find(|edit| edit.components.contains(&step.component))
                        .map(|edit| &edit.path)
                        .or_else(|| {
                            plan.claude_mcp
                                .as_ref()
                                .filter(|_| step.component == Component::Mcp)
                                .map(|native| &native.path)
                        });
                    PublicStep {
                        component: step.component,
                        effect: step.effect,
                        code: step.code,
                        backup: path
                            .and_then(|path| backups.get(path).copied().flatten())
                            .unwrap_or(Backup::None),
                    }
                })
                .collect();
            preview.push(AgentPreview { agent, steps });
            plans.push(Some(plan));
        }
        let mut targets = BTreeMap::new();
        for plan in plans.iter().flatten() {
            for edit in &plan.edits {
                let input = resources
                    .get(&edit.path)
                    .ok_or_else(|| refusal(422, "setup_unavailable"))?;
                if let Some(prior) =
                    targets.insert(input.target_path().to_owned(), edit.after.clone())
                    && prior != edit.after
                {
                    return Err(refusal(409, "setup_conflict"));
                }
            }
        }
        let mut bundle = Self {
            key: String::new(),
            preview,
            plans,
            resources,
            home: home.to_owned(),
            home_identity: directory
                .identity()
                .map_err(|_| refusal(422, "setup_unavailable"))?,
            executable,
            claude,
            path: std::env::var_os("PATH"),
        };
        let mut hash = Sha256::new();
        frame(&mut hash, b"oboete-agent-integration-v1");
        frame(&mut hash, bundle.home_identity.as_bytes());
        frame(&mut hash, command.exe.as_bytes());
        frame(&mut hash, command.home.as_deref().unwrap_or("").as_bytes());
        frame(&mut hash, bundle.executable.witness.as_bytes());
        frame(
            &mut hash,
            bundle
                .claude
                .as_ref()
                .map(|pin| pin.witness.as_bytes())
                .unwrap_or(b"no-launcher"),
        );
        frame(
            &mut hash,
            bundle
                .path
                .as_deref()
                .unwrap_or_default()
                .as_encoded_bytes(),
        );
        frame(
            &mut hash,
            &serde_json::to_vec(&json!({"action":action,"agents":bundle.preview}))
                .map_err(|_| bad_request())?,
        );
        for input in bundle.resources.known.values() {
            frame(&mut hash, input.witness.as_bytes());
        }
        for path in &bundle.resources.unavailable {
            frame(&mut hash, path.as_os_str().as_encoded_bytes());
            frame(&mut hash, b"unavailable");
        }
        for failure in failures {
            frame(&mut hash, failure.as_bytes());
        }
        for plan in bundle.plans.iter().flatten() {
            for edit in &plan.edits {
                frame(&mut hash, format!("{:?}", edit.file).as_bytes());
                frame(&mut hash, edit.witness.as_bytes());
                frame(&mut hash, edit.after.as_deref().unwrap_or("").as_bytes());
            }
            if let Some(native) = &plan.claude_mcp {
                frame(
                    &mut hash,
                    &serde_json::to_vec(&native.wanted_entry).map_err(|_| bad_request())?,
                );
            }
        }
        bundle.key = format!("{:x}", hash.finalize());
        if !bundle.unchanged() {
            return Err(refusal(409, "setup_stale"));
        }
        Ok(bundle)
    }
    fn unchanged(&self) -> bool {
        self.path == std::env::var_os("PATH")
            && self.executable.unchanged()
            && self.claude.as_ref().is_none_or(BinaryPin::unchanged)
            && self.resources.unchanged()
            && super::DiagnosticDirectory::open(&self.home).is_ok_and(|directory| {
                directory
                    .identity()
                    .is_ok_and(|identity| identity == self.home_identity)
            })
    }

    fn step(&self, index: usize, component: Component) -> PublicStep {
        self.preview[index]
            .steps
            .iter()
            .find(|step| step.component == component)
            .cloned()
            .expect("the fixed plan includes this component")
    }

    fn safe(&self, home: &Path, admitted: &CommandHome) -> bool {
        admitted.check(home).is_ok() && self.unchanged()
    }

    fn backup(
        &mut self,
        index: usize,
        path: &Path,
        component: Component,
        emit: &mut impl FnMut(Agent, ReceiptStep),
    ) -> Result<(), Phase> {
        let step = self.step(index, component);
        let before = self.resources.get(path).ok_or(Phase::Stale)?.text.clone();
        let backup = backup_path(path);
        let prior = self
            .resources
            .get(&backup)
            .ok_or(Phase::Stale)?
            .text
            .clone();
        if !self.resources.unchanged() {
            return Err(Phase::Stale);
        }
        if super::backup_once(path).is_err() {
            emit(
                self.preview[index].agent,
                ReceiptStep {
                    step,
                    kind: Kind::Backup,
                    outcome: Outcome::Failed,
                },
            );
            return Err(Phase::Failed);
        }
        let observed = files::input(&std::path::absolute(&backup).map_err(|_| Phase::Unknown)?)
            .map_err(|_| Phase::Unknown)?;
        let expected = if matches!(step.backup, Backup::Kept) {
            &prior
        } else {
            &before
        };
        if &observed.text != expected {
            emit(
                self.preview[index].agent,
                ReceiptStep {
                    step,
                    kind: Kind::Backup,
                    outcome: Outcome::Unknown,
                },
            );
            return Err(Phase::Unknown);
        }
        let changed = prior.is_none() && observed.text.is_some();
        self.resources
            .refresh(&backup, false)
            .map_err(|_| Phase::Unknown)?;
        emit(
            self.preview[index].agent,
            ReceiptStep {
                step,
                kind: Kind::Backup,
                outcome: if changed {
                    Outcome::Committed
                } else {
                    Outcome::Noop
                },
            },
        );
        Ok(())
    }

    fn apply_files(
        &mut self,
        index: usize,
        plan: &plan::Plan,
        home: &Path,
        admitted: &CommandHome,
        emit: &mut impl FnMut(Agent, ReceiptStep),
    ) -> Result<(), Phase> {
        let mut staged = Vec::new();
        for edit in &plan.edits {
            let component = edit.components[0];
            let step = self.step(index, component);
            if !self.safe(home, admitted) {
                emit(
                    plan.agent,
                    ReceiptStep {
                        step,
                        kind: Kind::Stage,
                        outcome: Outcome::Stale,
                    },
                );
                return Err(Phase::Stale);
            }
            let source = self.resources.get(&edit.path).ok_or(Phase::Stale)?;
            if source.text == edit.after {
                for &component in edit.components {
                    emit(
                        plan.agent,
                        ReceiptStep {
                            step: self.step(index, component),
                            kind: Kind::Readback,
                            outcome: Outcome::Noop,
                        },
                    );
                }
                continue;
            }
            if source.witness != edit.witness {
                emit(
                    plan.agent,
                    ReceiptStep {
                        step,
                        kind: Kind::Stage,
                        outcome: Outcome::Stale,
                    },
                );
                return Err(Phase::Stale);
            }
            let prepared = match &edit.after {
                Some(text) => super::stage(source.target_path(), text).map(Some),
                None => super::refuse_read_only(source.target_path()).map(|_| None),
            };
            match prepared {
                Ok(prepared) => {
                    emit(
                        plan.agent,
                        ReceiptStep {
                            step,
                            kind: Kind::Stage,
                            outcome: Outcome::Prepared,
                        },
                    );
                    staged.push((edit, prepared));
                }
                Err(_) => {
                    emit(
                        plan.agent,
                        ReceiptStep {
                            step,
                            kind: Kind::Stage,
                            outcome: Outcome::Failed,
                        },
                    );
                    return Err(Phase::Failed);
                }
            }
        }
        for (edit, _) in &staged {
            if !self.safe(home, admitted) {
                return Err(Phase::Stale);
            }
            self.backup(index, &edit.path, edit.components[0], emit)?;
        }
        for (edit, prepared) in staged {
            let kind = if prepared.is_some() {
                Kind::Write
            } else {
                Kind::Delete
            };
            if !self.safe(home, admitted) {
                emit(
                    plan.agent,
                    ReceiptStep {
                        step: self.step(index, edit.components[0]),
                        kind,
                        outcome: Outcome::Stale,
                    },
                );
                return Err(Phase::Stale);
            }
            let result = match prepared {
                Some(prepared) => prepared.commit(),
                None => std::fs::remove_file(&edit.path).map_err(anyhow::Error::from),
            };
            if result.is_err() {
                emit(
                    plan.agent,
                    ReceiptStep {
                        step: self.step(index, edit.components[0]),
                        kind,
                        outcome: Outcome::Failed,
                    },
                );
                return Err(Phase::Failed);
            }
            let refreshed = self.resources.refresh(&edit.path, edit.after.is_none());
            let landed = refreshed.is_ok()
                && self
                    .resources
                    .get(&edit.path)
                    .is_some_and(|input| input.text == edit.after);
            for &component in edit.components {
                emit(
                    plan.agent,
                    ReceiptStep {
                        step: self.step(index, component),
                        kind,
                        outcome: if landed {
                            Outcome::Committed
                        } else {
                            Outcome::Unknown
                        },
                    },
                );
            }
            if !landed {
                return Err(Phase::Unknown);
            }
        }
        Ok(())
    }

    fn entry(&mut self, path: &Path) -> anyhow::Result<Option<Value>> {
        self.resources.refresh(path, false)?;
        let text = self
            .resources
            .get(path)
            .and_then(|input| input.text.as_deref())
            .unwrap_or("");
        let root: Value = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(text)?
        };
        anyhow::ensure!(root.is_object(), "native state is not an object");
        Ok(root["mcpServers"].get(super::MCP_NAME).cloned())
    }

    fn native_command(
        &mut self,
        index: usize,
        home: &Path,
        admitted: &CommandHome,
        args: &[&str],
        kind: Kind,
        emit: &mut impl FnMut(Agent, ReceiptStep),
    ) -> Result<(), Phase> {
        let step = self.step(index, Component::Mcp);
        let Some(pin) = self.claude.as_ref() else {
            return Err(Phase::Stale);
        };
        if !self.safe(home, admitted) || !pin.content_unchanged() {
            emit(
                Agent::Claude,
                ReceiptStep {
                    step,
                    kind,
                    outcome: Outcome::Stale,
                },
            );
            return Err(Phase::Stale);
        }
        let exit = super::claude_cli_at(&pin.path, args, std::time::Duration::from_secs(30));
        let outcome = match exit {
            super::ClaudeCliExit::Success => Outcome::Committed,
            super::ClaudeCliExit::Nonzero | super::ClaudeCliExit::SpawnFailed => Outcome::Failed,
            super::ClaudeCliExit::Unknown => Outcome::Unknown,
        };
        emit(
            Agent::Claude,
            ReceiptStep {
                step,
                kind,
                outcome,
            },
        );
        match exit {
            super::ClaudeCliExit::Success => Ok(()),
            super::ClaudeCliExit::Unknown => Err(Phase::Unknown),
            _ => Err(Phase::Failed),
        }
    }

    fn apply_claude(
        &mut self,
        index: usize,
        native: &plan::ClaudeMcp,
        home: &Path,
        admitted: &CommandHome,
        emit: &mut impl FnMut(Agent, ReceiptStep),
    ) -> Result<(), Phase> {
        if !self.safe(home, admitted) {
            return Err(Phase::Stale);
        }
        if self
            .resources
            .get(&native.path)
            .is_none_or(|input| input.witness != native.witness)
        {
            return Err(Phase::Stale);
        }
        self.backup(index, &native.path, Component::Mcp, emit)?;
        if native.old_entry.is_some() {
            let result = self.native_command(
                index,
                home,
                admitted,
                &["mcp", "remove", "--scope", "user", super::MCP_NAME],
                Kind::NativeRemove,
                emit,
            );
            let removed = self.entry(&native.path).is_ok_and(|entry| entry.is_none());
            if result == Err(Phase::Unknown) {
                return Err(Phase::Unknown);
            }
            if removed {
                let mut step = self.step(index, Component::Mcp);
                step.effect = Effect::Delete;
                emit(
                    Agent::Claude,
                    ReceiptStep {
                        step,
                        kind: Kind::Readback,
                        outcome: Outcome::Committed,
                    },
                );
            }
            result?;
            if !removed {
                return Err(Phase::Unknown);
            }
        }
        let Some(wanted) = &native.wanted_entry else {
            let absent = self.entry(&native.path).is_ok_and(|entry| entry.is_none());
            emit(
                Agent::Claude,
                ReceiptStep {
                    step: self.step(index, Component::Mcp),
                    kind: Kind::Readback,
                    outcome: if absent {
                        Outcome::Committed
                    } else {
                        Outcome::Unknown
                    },
                },
            );
            return if absent { Ok(()) } else { Err(Phase::Unknown) };
        };
        let encoded = wanted.to_string();
        let added = self.native_command(
            index,
            home,
            admitted,
            &[
                "mcp",
                "add-json",
                "--scope",
                "user",
                super::MCP_NAME,
                &encoded,
            ],
            Kind::NativeAdd,
            emit,
        );
        let landed = self
            .entry(&native.path)
            .is_ok_and(|entry| entry.as_ref() == Some(wanted));
        if added == Err(Phase::Unknown) {
            return Err(Phase::Unknown);
        }
        if landed {
            emit(
                Agent::Claude,
                ReceiptStep {
                    step: self.step(index, Component::Mcp),
                    kind: Kind::Readback,
                    outcome: Outcome::Committed,
                },
            );
            return added;
        }
        Err(if added.is_err() {
            Phase::Failed
        } else {
            Phase::Unknown
        })
    }

    fn apply(
        &mut self,
        home: &Path,
        admitted: &CommandHome,
        mut emit: impl FnMut(Agent, ReceiptStep),
    ) -> Phase {
        #[derive(Clone, Copy, Default)]
        struct Progress {
            established: bool,
            effects: bool,
            problem: bool,
            stale: bool,
            unknown: bool,
        }
        let progress = std::cell::Cell::new(Progress::default());
        let mut record = |agent, step: ReceiptStep| {
            let mut value = progress.get();
            value.established |= matches!(step.outcome, Outcome::Committed | Outcome::Noop);
            value.effects |= matches!(step.outcome, Outcome::Committed | Outcome::Prepared);
            value.problem |= matches!(
                step.outcome,
                Outcome::Manual
                    | Outcome::Skipped
                    | Outcome::Failed
                    | Outcome::Stale
                    | Outcome::Unknown
            );
            value.unknown |= step.outcome == Outcome::Unknown;
            progress.set(value);
            emit(agent, step);
        };
        for index in 0..self.plans.len() {
            if progress.get().unknown {
                for mut step in self.preview[index].steps.clone() {
                    step.effect = Effect::Skipped;
                    step.code = Some(Code::PriorUnknown);
                    record(
                        self.preview[index].agent,
                        ReceiptStep {
                            step,
                            kind: Kind::Readback,
                            outcome: Outcome::Skipped,
                        },
                    );
                }
                continue;
            }
            let Some(plan) = self.plans[index].take() else {
                for step in self.preview[index].steps.clone() {
                    record(
                        self.preview[index].agent,
                        ReceiptStep {
                            step,
                            kind: Kind::Readback,
                            outcome: Outcome::Skipped,
                        },
                    );
                }
                continue;
            };
            for step in self.preview[index]
                .steps
                .clone()
                .into_iter()
                .filter(|step| {
                    matches!(
                        step.effect,
                        Effect::Unchanged | Effect::Manual | Effect::Skipped | Effect::Unavailable
                    )
                })
            {
                let outcome = match step.effect {
                    Effect::Unchanged => Outcome::Noop,
                    Effect::Manual => Outcome::Manual,
                    _ => Outcome::Skipped,
                };
                record(
                    plan.agent,
                    ReceiptStep {
                        step,
                        kind: Kind::Readback,
                        outcome,
                    },
                );
            }
            let result = self
                .apply_files(index, &plan, home, admitted, &mut record)
                .and_then(|_| {
                    if let Some(native) = &plan.claude_mcp {
                        self.apply_claude(index, native, home, admitted, &mut record)
                    } else {
                        Ok(())
                    }
                });
            if let Err(phase) = result {
                let mut value = progress.get();
                value.problem = true;
                value.stale |= phase == Phase::Stale;
                value.unknown |= phase == Phase::Unknown;
                progress.set(value);
            }
        }
        let value = progress.get();
        if value.unknown {
            Phase::Unknown
        } else if value.problem && (value.effects || value.established) {
            Phase::Partial
        } else if value.stale {
            Phase::Stale
        } else if value.problem {
            Phase::Failed
        } else {
            Phase::Complete
        }
    }
}
