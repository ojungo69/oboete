//! Shared native builders for the fixed agent integration plan.
//! No filesystem/environment/process access. Only Step/Agent/Action are public projections.

use super::super::{
    HookCommand, MCP_NAME, PI_MARKER, agy_merge_mcp, agy_spec, claude_groups, claude_mcp_entry,
    codex_groups, codex_trust_delta, codex_trust_keys, codex_write_trust, cursor_hook_spec,
    cursor_merge_hooks, cursor_merge_mcp, grok_empty, grok_groups, json_text, merge_groups,
    opencode_mcp_text, opencode_plugin, pi_extension, toml_mcp_text,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Agent {
    Claude,
    Codex,
    Grok,
    Agy,
    Opencode,
    Pi,
    Cursor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Wire,
    Unwire,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Component {
    Hooks,
    Mcp,
    Trust,
    Plugin,
    Extension,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Effect {
    Create,
    Replace,
    Delete,
    Unchanged,
    Manual,
    Skipped,
    Unavailable,
    NativeCommand,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Code {
    ManualJsonc,
    ManualJson,
    ForeignExtensionKept,
    AgentMissing,
    LauncherMissing,
    UnavailableFile,
    UnsupportedCommand,
    PriorUnknown,
    McpArgumentsTooLarge,
    ManualMcp,
}
#[derive(Serialize)]
pub(super) struct Step {
    pub(super) component: Component,
    pub(super) effect: Effect,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) code: Option<Code>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FileId {
    Hooks,
    Mcp,
    Plugin,
    Extension,
    Jsonc,
}
pub(super) struct Input {
    pub(super) path: PathBuf,
    pub(super) text: Option<String>, // None means confirmed absent, never a read error.
    pub(super) witness: String,
    pub(super) available: bool,
}
pub(super) type Inputs = BTreeMap<FileId, Input>;
pub(super) struct Edit {
    pub(super) file: FileId,
    pub(super) path: PathBuf,
    pub(super) witness: String,
    pub(super) after: Option<String>, // None means delete this fixed target.
    pub(super) components: &'static [Component],
}
pub(super) struct ClaudeMcp {
    pub(super) path: PathBuf,
    pub(super) witness: String,
    pub(super) old_entry: Option<Value>,
    pub(super) wanted_entry: Option<Value>,
}
pub(super) struct Plan {
    pub(super) agent: Agent,
    pub(super) edits: Vec<Edit>,
    pub(super) claude_mcp: Option<ClaudeMcp>,
    pub(super) steps: Vec<Step>,
}
#[derive(Debug)]
pub(super) enum PlanError {
    MissingInput(FileId),
    InvalidInput(FileId),
    UnsupportedCommand,
}
type Result<T> = std::result::Result<T, PlanError>;

fn input(inputs: &Inputs, id: FileId) -> Result<&Input> {
    inputs.get(&id).ok_or(PlanError::MissingInput(id))
}
fn object(input: &Input, id: FileId, empty_is_object: bool) -> Result<Value> {
    if !input.available {
        return Err(PlanError::InvalidInput(id));
    }
    let root = match input.text.as_deref() {
        None => json!({}),
        Some(text) if empty_is_object && text.trim().is_empty() => json!({}),
        Some(text) => serde_json::from_str(text).map_err(|_| PlanError::InvalidInput(id))?,
    };
    if !root.is_object() {
        return Err(PlanError::InvalidInput(id));
    }
    Ok(root)
}
fn render(value: &Value, id: FileId) -> Result<String> {
    json_text(value).map_err(|_| PlanError::InvalidInput(id))
}
fn effect(before: Option<&str>, after: Option<&str>) -> Effect {
    if before == after {
        Effect::Unchanged
    } else {
        match (before, after) {
            (None, Some(_)) => Effect::Create,
            (Some(_), None) => Effect::Delete,
            _ => Effect::Replace,
        }
    }
}
fn step(plan: &mut Plan, component: Component, effect: Effect, code: Option<Code>) {
    plan.steps.push(Step {
        component,
        effect,
        code,
    });
}
fn edit(
    plan: &mut Plan,
    id: FileId,
    input: &Input,
    after: Option<String>,
    components: &'static [Component],
) -> Effect {
    let change = effect(input.text.as_deref(), after.as_deref());
    if change != Effect::Unchanged {
        plan.edits.push(Edit {
            file: id,
            path: input.path.clone(),
            witness: input.witness.clone(),
            after,
            components,
        });
    }
    change
}
fn component(plan: &mut Plan, id: FileId, input: &Input, after: Option<String>, part: Component) {
    let parts: &'static [Component] = match part {
        Component::Hooks => &[Component::Hooks],
        Component::Mcp => &[Component::Mcp],
        Component::Trust => &[Component::Trust],
        Component::Plugin => &[Component::Plugin],
        Component::Extension => &[Component::Extension],
    };
    let change = edit(plan, id, input, after, parts);
    step(plan, part, change, None);
}

/// Eligibility, fixed path selection and read/write admission happen in the root reader.
/// Effects describe fixed-target bytes/native actions, never successful registration or live use.
pub(super) fn plan_agent(
    agent: Agent,
    cmd: &HookCommand,
    remove: bool,
    inputs: &Inputs,
) -> Result<Plan> {
    let ids: &[FileId] = match agent {
        Agent::Opencode => &[FileId::Plugin, FileId::Mcp, FileId::Jsonc],
        Agent::Pi => &[FileId::Extension],
        _ => &[FileId::Hooks, FileId::Mcp],
    };
    for &id in ids {
        input(inputs, id)?;
    }
    let required = match agent {
        Agent::Cursor => &[][..],
        Agent::Opencode => &[FileId::Plugin][..],
        _ => ids,
    };
    for &id in required {
        if !input(inputs, id)?.available {
            return Err(PlanError::InvalidInput(id));
        }
    }
    let mut plan = Plan {
        agent,
        edits: Vec::new(),
        claude_mcp: None,
        steps: Vec::new(),
    };
    match agent {
        Agent::Claude | Agent::Grok => {
            let hooks = input(inputs, FileId::Hooks)?;
            if remove && hooks.text.is_none() {
                step(&mut plan, Component::Hooks, Effect::Unchanged, None);
            } else {
                let mut root = object(hooks, FileId::Hooks, true)?;
                let wanted = if remove {
                    vec![]
                } else {
                    match agent {
                        Agent::Claude => claude_groups(cmd),
                        _ => grok_groups(cmd),
                    }
                };
                merge_groups(&mut root, wanted);
                let after = if matches!(agent, Agent::Grok) && grok_empty(&root) {
                    None
                } else {
                    Some(render(&root, FileId::Hooks)?)
                };
                component(&mut plan, FileId::Hooks, hooks, after, Component::Hooks);
            }
            let mcp = input(inputs, FileId::Mcp)?;
            if matches!(agent, Agent::Claude) {
                let root = object(mcp, FileId::Mcp, true)?;
                let old_entry = root["mcpServers"].get(MCP_NAME).cloned();
                let wanted_entry = (!remove).then(|| claude_mcp_entry(old_entry.as_ref(), cmd));
                if old_entry == wanted_entry {
                    step(&mut plan, Component::Mcp, Effect::Unchanged, None);
                } else if !remove && old_entry.is_some() {
                    // Saved fields (including old command/args) may contain credentials.
                    // Preserve them rather than forwarding them to add-json or rollback argv.
                    step(
                        &mut plan,
                        Component::Mcp,
                        Effect::Manual,
                        Some(Code::ManualMcp),
                    );
                } else {
                    plan.claude_mcp = Some(ClaudeMcp {
                        path: mcp.path.clone(),
                        witness: mcp.witness.clone(),
                        old_entry,
                        wanted_entry,
                    });
                    step(&mut plan, Component::Mcp, Effect::NativeCommand, None);
                }
            } else if remove && mcp.text.is_none() {
                step(&mut plan, Component::Mcp, Effect::Unchanged, None);
            } else {
                let text = toml_mcp_text(&mcp.path, mcp.text.as_deref().unwrap_or(""), cmd, remove)
                    .map_err(|_| PlanError::InvalidInput(FileId::Mcp))?;
                component(&mut plan, FileId::Mcp, mcp, Some(text), Component::Mcp);
            }
        }
        Agent::Codex => {
            let hooks = input(inputs, FileId::Hooks)?;
            let mcp = input(inputs, FileId::Mcp)?;
            let coupled = !(remove && hooks.text.is_none());
            let mut trusted = mcp.text.clone().unwrap_or_default();
            let trust_change = if coupled {
                let mut root = object(hooks, FileId::Hooks, true)?;
                let mut doc: toml_edit::DocumentMut = trusted
                    .parse()
                    .map_err(|_| PlanError::InvalidInput(FileId::Mcp))?;
                let before = codex_trust_keys(&hooks.path, &root);
                merge_groups(&mut root, if remove { vec![] } else { codex_groups(cmd) });
                let delta = codex_trust_delta(&before, &codex_trust_keys(&hooks.path, &root));
                codex_write_trust(&mut doc, &delta);
                trusted = doc.to_string();
                component(
                    &mut plan,
                    FileId::Hooks,
                    hooks,
                    Some(render(&root, FileId::Hooks)?),
                    Component::Hooks,
                );
                effect(mcp.text.as_deref(), Some(&trusted))
            } else {
                step(&mut plan, Component::Hooks, Effect::Unchanged, None);
                Effect::Unchanged
            };
            let final_text = if coupled || !remove || mcp.text.is_some() {
                Some(
                    toml_mcp_text(&mcp.path, &trusted, cmd, remove)
                        .map_err(|_| PlanError::InvalidInput(FileId::Mcp))?,
                )
            } else {
                None
            };
            let mcp_change = match final_text.as_deref() {
                Some(text) if text != trusted => {
                    if mcp.text.is_some() {
                        Effect::Replace
                    } else {
                        Effect::Create
                    }
                }
                _ => Effect::Unchanged,
            };
            edit(
                &mut plan,
                FileId::Mcp,
                mcp,
                final_text,
                &[Component::Trust, Component::Mcp],
            );
            step(&mut plan, Component::Trust, trust_change, None);
            step(&mut plan, Component::Mcp, mcp_change, None);
        }
        Agent::Agy => {
            let hooks = input(inputs, FileId::Hooks)?;
            let mcp = input(inputs, FileId::Mcp)?;
            let mut root = object(hooks, FileId::Hooks, true)?;
            let old = root.clone();
            let named = root
                .as_object_mut()
                .ok_or(PlanError::InvalidInput(FileId::Hooks))?;
            if remove {
                named.remove(MCP_NAME);
            } else {
                named.insert(
                    MCP_NAME.into(),
                    agy_spec(cmd, cfg!(windows)).map_err(|_| PlanError::UnsupportedCommand)?,
                );
            }
            if root == old {
                step(&mut plan, Component::Hooks, Effect::Unchanged, None);
            } else {
                component(
                    &mut plan,
                    FileId::Hooks,
                    hooks,
                    Some(render(&root, FileId::Hooks)?),
                    Component::Hooks,
                );
            }
            let mut root = object(mcp, FileId::Mcp, true)?;
            let old = root.clone();
            agy_merge_mcp(&mcp.path, &mut root, cmd, remove)
                .map_err(|_| PlanError::InvalidInput(FileId::Mcp))?;
            if root == old {
                step(&mut plan, Component::Mcp, Effect::Unchanged, None);
            } else {
                component(
                    &mut plan,
                    FileId::Mcp,
                    mcp,
                    Some(render(&root, FileId::Mcp)?),
                    Component::Mcp,
                );
            }
        }
        Agent::Opencode => {
            let plugin = input(inputs, FileId::Plugin)?;
            let mcp = input(inputs, FileId::Mcp)?;
            let after = if remove {
                None
            } else {
                Some(opencode_plugin(cmd).map_err(|_| PlanError::UnsupportedCommand)?)
            };
            component(&mut plan, FileId::Plugin, plugin, after, Component::Plugin);
            if !mcp.available || !input(inputs, FileId::Jsonc)?.available {
                step(
                    &mut plan,
                    Component::Mcp,
                    Effect::Unavailable,
                    Some(Code::UnavailableFile),
                );
            } else if input(inputs, FileId::Jsonc)?.text.is_some() {
                step(
                    &mut plan,
                    Component::Mcp,
                    Effect::Manual,
                    Some(Code::ManualJsonc),
                );
            } else {
                let update = object(mcp, FileId::Mcp, false).and_then(|root| {
                    opencode_mcp_text(&mcp.path, root, cmd, remove)
                        .map_err(|_| PlanError::InvalidInput(FileId::Mcp))
                });
                match update {
                    Ok(Some(text)) => {
                        component(&mut plan, FileId::Mcp, mcp, Some(text), Component::Mcp)
                    }
                    Ok(None) => step(&mut plan, Component::Mcp, Effect::Unchanged, None),
                    Err(_) => step(
                        &mut plan,
                        Component::Mcp,
                        Effect::Manual,
                        Some(Code::ManualJson),
                    ),
                }
            }
        }
        Agent::Pi => {
            let extension = input(inputs, FileId::Extension)?;
            if remove
                && extension
                    .text
                    .as_ref()
                    .is_some_and(|text| !text.starts_with(PI_MARKER))
            {
                step(
                    &mut plan,
                    Component::Extension,
                    Effect::Skipped,
                    Some(Code::ForeignExtensionKept),
                );
            } else {
                let after = if remove {
                    None
                } else {
                    Some(pi_extension(cmd).map_err(|_| PlanError::UnsupportedCommand)?)
                };
                component(
                    &mut plan,
                    FileId::Extension,
                    extension,
                    after,
                    Component::Extension,
                );
            }
        }
        Agent::Cursor => {
            let spec = if remove {
                json!({})
            } else {
                cursor_hook_spec(cmd, cfg!(windows)).map_err(|_| PlanError::UnsupportedCommand)?
            };
            for (id, part) in [
                (FileId::Hooks, Component::Hooks),
                (FileId::Mcp, Component::Mcp),
            ] {
                let value = input(inputs, id)?;
                if !value.available {
                    step(
                        &mut plan,
                        part,
                        Effect::Unavailable,
                        Some(Code::UnavailableFile),
                    );
                    continue;
                }
                if remove && value.text.is_none() {
                    step(&mut plan, part, Effect::Unchanged, None);
                    continue;
                }
                let merged = object(value, id, false).and_then(|mut root| {
                    let before = root.clone();
                    let result = match id {
                        FileId::Hooks => cursor_merge_hooks(&mut root, &spec, remove),
                        _ => cursor_merge_mcp(&mut root, cmd, remove),
                    };
                    result.map_err(|_| PlanError::InvalidInput(id))?;
                    if root == before {
                        Ok(None)
                    } else {
                        Ok(Some(render(&root, id)?))
                    }
                });
                match merged {
                    Ok(Some(text)) => component(&mut plan, id, value, Some(text), part),
                    Ok(None) => step(&mut plan, part, Effect::Unchanged, None),
                    Err(_) => step(&mut plan, part, Effect::Manual, Some(Code::ManualJson)),
                }
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests;
