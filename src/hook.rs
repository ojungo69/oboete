//! Hook path: one agent event in on stdin, one row out. Must be fast and must never fail the agent.
//! Claude Code, Codex, Grok Build, and Pi share one JSON dialect; Grok also sends camelCase copies
//! and runs Claude Code's hooks as a compatibility layer, which is handled in `resolve_agent`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{config, db, redact};

/// Largest text kept per field. Tool outputs beyond this are clipped with a marker.
const MAX_FIELD: usize = 8_000;
/// Redaction looks this far past the clip point, so a secret straddling it is masked whole when
/// it fits; a longer block (an RSA-8192 PEM is ~6,400 chars) is cut at its BEGIN line instead.
const REDACT_OVERLAP: usize = 4_000;
/// Set on the curator CLIs oboete runs, so the curator's own session is never captured.
pub const SKIP_ENV: &str = "OBOETE_SKIP";
/// Blocks that are not part of what was asked or read, removed before anything is stored, in this
/// order: context an IDE or another memory tool puts in front of the text (it may quote
/// `<private>`), claude-mem's copy of the past that it writes into instruction files an agent
/// reads, then `<private>`, the developer's opt-out (claude-mem's tag; unclosed, it hides the
/// rest, where claude-mem would keep it all).
pub(crate) const STRIP_BLOCKS: &[&str] = &[
    "ide_opened_file",
    "hook_context",
    "claude-mem-context",
    "private",
];
/// Prompts that are harness traffic, not typed: background task and teammate notifications and
/// the /loop sentinel (13.5% of the 15,218 prompts claude-mem stored on this machine).
/// ponytail: fixed prefix list; add one when a new envelope shows up as a prompt card.
const ENVELOPES: &[&str] = &[
    "<task-notification",
    "<agent-message",
    "<system_notification",
    "<bash-notification",
    "<<autonomous-loop",
    // Agent teams: a teammate's message arrives as a prompt (docs/milestone-1.md, dev drafts).
    "Another Claude session sent a message:",
];

pub fn run_stdin(home: &Path, agent: &str, event: &str) -> Result<()> {
    run_io(
        home,
        agent,
        event,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
}

fn run_io(
    home: &Path,
    agent: &str,
    event: &str,
    mut input: impl Read,
    mut output: impl Write,
) -> Result<()> {
    // MUST-M16, for raw.db: whether this call tried to write there (skips and filtered sessions
    // never do), and whether a row was written. Only a written row shows that recording works
    // again: a hook with nothing to capture (PreToolUse, an empty Stop) proves nothing.
    let mut tried = false;
    let mut wrote = false;
    // When the store operation ended (0 until one did): overlapping hooks change the marker in
    // this order, so it is taken before anything that runs after the write.
    let mut ended = 0;
    // Task 9: the manifest this call injects for the checkout its payload names, and whether it
    // is an injection point at all (Task 2b: each agent has its own, see `injects`).
    let mut manifest = None;
    let mut injecting = false;
    // Cursor's compaction flag this call took, put back if its write fails: the manifest is then
    // shown at the next prompt, once recording works again.
    let mut took_compaction: Option<String> = None;
    let result: Result<()> = (|| {
        if std::env::var_os(SKIP_ENV).is_some() {
            return Ok(());
        }
        let mut raw = String::new();
        input.read_to_string(&mut raw)?;
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        let mut payload: Value = if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw)?
        };
        // The agent mark is oboete's own, from a rollout or the transcript parser (#273): a live
        // hook's payload is the agent's, so any it carries is dropped.
        if let Some(fields) = payload.as_object_mut() {
            fields.remove(crate::capture::AGENT_SENT);
        }
        let Some(agent) = resolve_agent(agent, &payload, &grok_hooks_file()) else {
            return Ok(());
        };
        if !crate::setup::AGENTS.contains(&agent)
            || (matches!(agent, "agy" | "cursor") && agent_workspace(agent, &payload).is_none())
            || is_agent_internal(agent, &payload)
        {
            return Ok(());
        }
        // Creating the home is part of the attempt: a home that cannot be made is a failure too.
        tried = true;
        std::fs::create_dir_all(home)?;
        let labels = agent_labels(agent, &payload);
        if (agent, event) == ("cursor", "PreCompact") {
            // Before the write: a compaction whose record fails still reinjects at the next
            // prompt. A flag that cannot be written costs that reinjection, not the record.
            if let Err(e) = crate::hookstate::set(home, agent, session_label(&labels), "compacted")
            {
                eprintln!("oboete: compaction not noted: {e}");
            }
        }
        // Before the write too: when this call is the agent's injection point and its own
        // write fails, the point still carries the recording-failure line. Grok's and agy's
        // points stay taken then (the line is shown once per session, not at every call).
        injecting = injects(home, agent, event, &labels);
        if injecting && (agent, event) == ("cursor", "UserPromptSubmit") {
            took_compaction = Some(session_label(&labels).to_owned());
        }
        let settings = crate::capture::Settings::load(home)?;
        let mut store = crate::raw::open_within(home, Duration::from_secs(2))?;
        let events = record(
            home,
            &mut store,
            agent,
            event,
            &payload,
            db::now_ms(),
            &settings,
        )?;
        wrote = !events.is_empty();
        ended = crate::failure::now();
        // A manifest that cannot be read is no recording failure: the row is written.
        if injecting {
            manifest = checkout_manifest(home, &store, &labels, &settings);
        }
        Ok(())
    })();
    if ended == 0 {
        ended = crate::failure::now(); // a failure: the operation ended when it returned
    }
    let mut out = None;
    if tried {
        // The marker lives outside the stores, so it is written when they cannot be.
        match &result {
            Ok(_) if wrote => {
                crate::failure::prepare(home);
                crate::failure::clear(home, ended);
                // After the marker: nothing that can take long runs between the write and its
                // marker update, which overlapping hooks order by the write's end.
                if let Err(e) = start_worker(home) {
                    // The row is written, and the next hook starts a worker for it: MUST-M16's
                    // marker is about the store, so this is no recording failure.
                    eprintln!("oboete: worker not started: {e:#}");
                }
            }
            Ok(_) => {}
            Err(e) => {
                crate::failure::mark(home, crate::failure::classify(e), ended);
                if let Some(session) = &took_compaction
                    && let Err(e) = crate::hookstate::set(home, "cursor", session, "compacted")
                {
                    eprintln!("oboete: compaction not noted again: {e}");
                }
                // Task 8: the worker restores a damaged raw.db, and no hook starts one otherwise
                // (they start it after a written row). A worker already running opened raw.db
                // before the damage: the request makes it open the stores again.
                if crate::backup::corrupt(e) {
                    crate::backup::request_restore(home);
                    if let Err(e) = start_worker(home) {
                        eprintln!("oboete: worker not started: {e:#}");
                    }
                }
            }
        }
        // The recording-failure line at every SessionStart the agent reads (also when this call
        // failed before it knew whether it injects), then the manifest in its fence.
        let failed = crate::failure::since(home).or_else(|| {
            // No marker when even the marker could not be written: this call's error, then.
            result
                .as_ref()
                .err()
                .map(|e| (crate::failure::classify(e), ended))
        });
        // Grok, agy and Cursor show it at their injection points only (other calls return nothing
        // or {}); the others at every SessionStart, resumes too.
        let reads_start = event == "SessionStart" && !matches!(agent, "grok" | "agy" | "cursor");
        let parts: Vec<String> = [
            failed
                .filter(|_| injecting || reads_start)
                .map(crate::failure::line),
            manifest.as_deref().map(crate::manifest::fenced),
        ]
        .into_iter()
        .flatten()
        .collect();
        // Cursor gets its field even when empty: a reinjection is consumed either way.
        if !parts.is_empty() || (injecting && agent == "cursor") {
            out = Some(injection(agent, event, &parts.join("\n")).to_string());
        }
    }
    if let Some(out) = &out {
        writeln!(output, "{out}")?;
    } else if matches!(agent, "agy" | "cursor") {
        // Each adapter returns its own JSON shape, including skip and error paths.
        writeln!(output, "{{}}")?;
    }
    result
}

/// Task 2b: whether this call is the agent's point to inject context. Claude Code, Codex, Pi and
/// OpenCode read SessionStart (not on a resume: its context has it already; after a compaction it
/// does not, so it is shown again). Grok ignores SessionStart's output, so the first tool call of
/// a session injects, and agy reads PreInvocation, once per session too. Cursor reads SessionStart
/// and, after its compaction marker, the next prompt. A point is claimed before the manifest is
/// read, so a session whose checkout has none yet gets none later either, as at SessionStart
/// (Claude; overrulable).
fn injects(home: &Path, agent: &str, event: &str, payload: &Value) -> bool {
    let session = session_label(payload);
    match (agent, event) {
        ("grok", "PreToolUse") | ("agy", "PreInvocation") => {
            crate::hookstate::claim(home, agent, session, "injected")
        }
        ("cursor", "SessionStart") => true,
        ("cursor", "UserPromptSubmit") => crate::hookstate::take(home, agent, session, "compacted"),
        ("grok" | "agy" | "cursor", _) => false,
        (_, "SessionStart") => str_field(payload, &["source"]) != Some("resume"),
        _ => false,
    }
}

/// The session a hook's state is kept under (`run_io` sets Cursor's compaction marker with it).
fn session_label(payload: &Value) -> &str {
    str_field(
        payload,
        &[
            "session_id",
            "sessionId",
            "conversation_id",
            "conversationId",
        ],
    )
    .unwrap_or("")
}

/// The injected text in the shape the agent reads.
fn injection(agent: &str, event: &str, text: &str) -> Value {
    match agent {
        "agy" => json!({"injectSteps": [{"ephemeralMessage": text}]}),
        "cursor" => cursor_injection(text),
        _ => json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}),
    }
}

/// Design B (milestone 2 Task 2): the events of one hook call, appended to `raw.db` with the
/// event's time (`now` in a hook; the fixture's in a replay). Returns the appended events.
pub fn record(
    home: &Path,
    raw: &mut crate::raw::Raw,
    agent: &str,
    event: &str,
    payload: &Value,
    ts: i64,
    settings: &crate::capture::Settings,
) -> Result<Vec<crate::raw::Event>> {
    // Count and append together so overlapping SessionEnd hooks cannot recover the same turn.
    // ponytail: one recovery lock per home; use per-session locks if end hooks contend.
    let _recovery = if agent == "cursor" && event == "SessionEnd" {
        let state = home.join("state");
        std::fs::create_dir_all(&state)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state.join("cursor-recovery.lock"))?;
        file.lock()?;
        Some(file)
    } else {
        None
    };
    let mut appended = Vec::new();
    for (event, payload) in adapt(home, raw, agent, event, payload, settings)? {
        for mut c in crate::capture::events(agent, &event, &payload, ts, settings) {
            c.event.session = own_session(std::mem::take(&mut c.event.session), raw);
            if let Err(e) = raw.append_with_ledger(&c.event, &c.ledger, settings.rules.version()) {
                // The claim precedes capture; a failed append must let a later hook retry this step.
                if agent == "agy"
                    && event == "UserPromptSubmit"
                    && let Some(step) = payload["step_index"].as_i64()
                {
                    crate::hookstate::take(
                        home,
                        agent,
                        payload["session_id"].as_str().unwrap_or("unknown"),
                        &format!("step-{step}"),
                    );
                }
                return Err(e);
            }
            appended.push(c.event);
        }
    }
    Ok(appended)
}

/// What SessionStart shows for the checkout `labels` names (Claude Code's fields), for the agent's
/// model provider: its manifest with the delivered claims (`consumer::manifest::text`), which a
/// checkout with no manifest to show gets too; gated with the rules as they are now, so a rule
/// added after the text was built already hides its value (spec 6.4), and cut to its cap from the
/// end, where the index is. Text that cannot be read is none, never a failed hook.
fn checkout_manifest(
    home: &Path,
    store: &crate::raw::Raw,
    labels: &Value,
    settings: &crate::capture::Settings,
) -> Option<String> {
    let (session, repo, branch) = crate::capture::checkout(labels, settings);
    let session = own_session(session, store);
    start_text(
        home,
        store,
        &repo,
        branch.as_deref().unwrap_or(""),
        &session,
        settings,
    )
}

/// SessionStart's manifest for the checkout (`repo`, `branch`) shown to `session`, the labels as
/// `checkout_manifest` reads them from a hook's fields: gated with the rules as they are now and
/// cut to `[inject]`'s size. It writes nothing (milestone 4 D11: the viewer's Context page shows it
/// for any checkout).
pub fn start_text(
    home: &Path,
    store: &crate::raw::Raw,
    repo: &str,
    branch: &str,
    session: &str,
    settings: &crate::capture::Settings,
) -> Option<String> {
    start_text_read(home, store, repo, branch, session, settings).unwrap_or_else(|e| {
        eprintln!("oboete: manifest not read: {e:#}");
        None
    })
}

/// `start_text` with the manifest's read error returned, which a hook only logs: the viewer's
/// Context page answers it (D11: 503 for a store a restore or a rebuild holds).
pub fn start_text_read(
    home: &Path,
    store: &crate::raw::Raw,
    repo: &str,
    branch: &str,
    session: &str,
    settings: &crate::capture::Settings,
) -> anyhow::Result<Option<String>> {
    // `[inject]` (#94): off, or a smaller size than the stored manifest's. Settings that do not
    // read inject nothing, as capture settings that do not read record nothing.
    let Some(inject) = crate::config::inject(home)
        .inspect_err(|e| eprintln!("oboete: nothing injected: {e:#}"))
        .ok()
        .filter(|i| i.session_start)
    else {
        return Ok(None);
    };
    let text = crate::consumer::manifest::text(
        home,
        store,
        repo,
        branch,
        session,
        settings.rules.version(),
    )?;
    Ok(text.map(|t| {
        let gated = crate::redact::outbound_with(&t, &settings.rules);
        crate::manifest::cut(&gated, inject.session_start_chars)
    }))
}

/// `oboete inject`: what a SessionStart hook shows for the checkout at `cwd` (the recording-failure
/// line, then the manifest in its fence). OpenCode's plugin reads its context here, since
/// OpenCode drops a hook's output.
pub fn inject_text(home: &Path, cwd: &Path, session: Option<&str>) -> String {
    // The failure line does not wait on the settings or raw.db: one that cannot be read may be
    // the failure it reports.
    let manifest = (|| -> Result<Option<String>> {
        let settings = crate::capture::Settings::load(home)?;
        let store = crate::raw::open_within(home, Duration::from_secs(2))?;
        let labels = json!({"session_id": session.unwrap_or("unknown"), "cwd": cwd});
        Ok(checkout_manifest(home, &store, &labels, &settings))
    })()
    .unwrap_or_else(|e| {
        eprintln!("oboete: manifest not read: {e:#}");
        None
    });
    joined(home, manifest.as_deref())
}

/// The recording-failure line, then `manifest` in its fence: what SessionStart shows, as `oboete
/// inject` prints it and the viewer's Context page shows it.
pub fn joined(home: &Path, manifest: Option<&str>) -> String {
    let parts: Vec<String> = [
        crate::failure::since(home).map(crate::failure::line),
        manifest.map(crate::manifest::fenced),
    ]
    .into_iter()
    .flatten()
    .collect();
    parts.join("\n")
}

/// An idless event's session is this device's own: a bare "unknown" would be one session on
/// every device once they sync (v1 did the same). Recording and the manifest's lookup
/// both use it.
pub(crate) fn own_session(session: String, raw: &crate::raw::Raw) -> String {
    if session == "unknown" {
        format!("unknown-{}", raw.device())
    } else {
        session
    }
}

/// Recording and injection use the same labels, including IDE workspaces that differ from
/// the hook process's cwd. Agents already sending Claude Code's fields keep them unchanged.
fn agent_labels(agent: &str, payload: &Value) -> Value {
    if !matches!(agent, "grok" | "agy" | "cursor") {
        return payload.clone();
    }
    let mut p = payload.as_object().cloned().unwrap_or_default();
    let session = str_field(
        payload,
        if agent == "cursor" {
            &["session_id", "conversation_id"]
        } else {
            &[
                "session_id",
                "sessionId",
                "conversation_id",
                "conversationId",
            ]
        },
    )
    .unwrap_or("unknown");
    p.insert("session_id".into(), json!(session));
    let cwd = agent_workspace(agent, payload)
        .or_else(|| str_field(payload, &["cwd", "workspaceRoot"]).map(str::to_owned))
        .unwrap_or_else(|| ".".into());
    p.insert("cwd".into(), json!(cwd));
    Value::Object(p)
}

/// Translate agent fields before capture's one privacy and redaction gate.
fn adapt(
    home: &Path,
    raw: &crate::raw::Raw,
    agent: &str,
    event: &str,
    payload: &Value,
    settings: &crate::capture::Settings,
) -> Result<Vec<(String, Value)>> {
    if matches!(agent, "agy" | "cursor") && agent_workspace(agent, payload).is_none() {
        return Ok(Vec::new());
    }
    let mut p = agent_labels(agent, payload);
    if agent == "grok"
        && let Some(text) = str_field(payload, &["last_assistant_message", "lastAssistantMessage"])
    {
        p["last_assistant_message"] = json!(text);
    }
    if agent == "cursor" {
        match event {
            // An empty Cursor response must not fall back to another transcript dialect.
            "Stop" => {
                p["last_assistant_message"] = json!(str_field(payload, &["text"]).unwrap_or(""))
            }
            "PostToolUseFailure" => p["tool_response"] = payload["error_message"].clone(),
            _ => {}
        }
    }
    if agent == "agy" && matches!(event, "PreInvocation" | "PostToolUse" | "Stop") {
        // One bounded read per hook. Step indices, not file order, define the current turn.
        let steps: Vec<Value> = str_field(payload, &["transcriptPath"])
            .map(|p| transcript_tail(Path::new(p), TAIL))
            .unwrap_or_default()
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let prompt = steps
            .iter()
            .filter(|s| s["type"] == "USER_INPUT" && s["source"] == "USER_EXPLICIT")
            .max_by_key(|s| s["step_index"].as_i64())
            .and_then(|s| {
                let step = s["step_index"].as_i64().filter(|i| *i >= 0)?;
                let (_, text) = s["content"].as_str()?.split_once("<USER_REQUEST>")?;
                let (text, _) = text.split_once("</USER_REQUEST>")?;
                Some((step, text))
            });
        let mut events = Vec::new();
        if matches!(event, "PreInvocation" | "Stop")
            && let Some((step, text)) = prompt
            && crate::hookstate::claim(
                home,
                agent,
                p["session_id"].as_str().unwrap(),
                &format!("step-{step}"),
            )
        {
            let mut turn = p.clone();
            turn["prompt"] = json!(text);
            turn["step_index"] = json!(step);
            events.push(("UserPromptSubmit".into(), turn));
        }
        if event == "PostToolUse" {
            let step = payload["stepIdx"].as_i64().and_then(|index| {
                steps
                    .iter()
                    .find(|s| s["step_index"].as_i64() == Some(index))
            });
            p["tool_name"] = json!(
                str_field(&payload["toolCall"], &["tool_name", "toolName", "name"]).unwrap_or("?")
            );
            p["tool_input"] =
                field(&payload["toolCall"], &["tool_input", "toolInput", "args"]).clone();
            p["tool_response"] = step
                .map(|s| {
                    s.get("content")
                        .filter(|v| v.as_str().is_some_and(|t| !t.is_empty()))
                        .unwrap_or(&s["error"])
                        .clone()
                })
                .unwrap_or(Value::Null);
            let failed = payload["error"].as_str().is_some_and(|s| !s.is_empty())
                || step.is_some_and(|s| s["status"] == "ERROR");
            events.push((if failed { "PostToolUseFailure" } else { event }.into(), p));
        } else if event == "Stop" {
            let turn_start = prompt.map_or(-1, |(step, _)| step);
            if let Some(text) = steps
                .iter()
                .filter(|s| {
                    s["type"] == "PLANNER_RESPONSE"
                        && s["step_index"].as_i64().is_some_and(|i| i > turn_start)
                        && s["content"].as_str().is_some_and(|t| !t.trim().is_empty())
                })
                .max_by_key(|s| s["step_index"].as_i64())
                .and_then(|s| s["content"].as_str())
            {
                p["last_assistant_message"] = json!(text);
                events.push((event.into(), p));
            }
        }
        return Ok(events);
    }
    if agent == "cursor"
        && event == "SessionEnd"
        && let Some(path) = str_field(payload, &["transcript_path"])
    {
        let (session, _, _) = crate::capture::checkout(&p, settings);
        let session = own_session(session, raw);
        let counts = raw.prompt_counts(agent, &session)?;
        let mut nth = std::collections::HashMap::new();
        let mut events = Vec::new();
        for (prompt, answer) in cursor_turns(Path::new(path)) {
            let mut turn = p.clone();
            turn["prompt"] = json!(prompt);
            // The gate may strip private blocks, redact, cap, or omit the prompt. Compare the
            // body it stores, not transcript bytes; harness envelopes are not recovered turns.
            let captured = crate::capture::events(agent, "UserPromptSubmit", &turn, 0, settings);
            let Some(c) = captured.first().filter(|c| c.event.kind == "prompt") else {
                continue;
            };
            let n = nth.entry(c.event.body.clone()).or_insert(0);
            *n += 1;
            if *n <= counts.get(&c.event.body).copied().unwrap_or(0) {
                continue;
            }
            events.push(("UserPromptSubmit".into(), turn.clone()));
            if !answer.trim().is_empty() {
                turn["last_assistant_message"] = json!(answer);
                events.push(("Stop".into(), turn));
            }
        }
        events.push((event.into(), p));
        return Ok(events);
    }
    if agent == "codex" && event == "PostToolUse" && codex_call_failed(payload) {
        return Ok(vec![("PostToolUseFailure".into(), p)]);
    }
    // A transcript import says who sent the prompt; a live hook reads the rollout (#273).
    if agent == "codex"
        && event == "UserPromptSubmit"
        && p.get(crate::capture::AGENT_SENT).is_none()
        && str_field(payload, &["transcript_path"])
            .is_some_and(|t| rollout_agent_sent(Path::new(t)))
    {
        p[crate::capture::AGENT_SENT] = json!(true);
    }
    Ok(vec![(event.into(), p)])
}

/// Whether another agent started the Codex session whose rollout is `path`, from its first line,
/// the `session_meta` (about 23 KB with Codex's instructions, 0.158). One bounded read per
/// prompt; a line over the cap or a file that cannot be read leaves the prompt the user's.
fn rollout_agent_sent(path: &Path) -> bool {
    use std::io::BufRead;
    const HEAD: u64 = 1 << 20;
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let mut line = Vec::new();
    if std::io::BufReader::new(f.take(HEAD))
        .read_until(b'\n', &mut line)
        .is_err()
    {
        return false;
    }
    serde_json::from_slice::<Value>(&line).is_ok_and(|v| {
        v["type"] == "session_meta" && crate::transcript::codex_agent_sent(&v["payload"])
    })
}

/// Whether the Codex call a PostToolUse reports failed. Codex's hook input carries only the
/// output (0.155.1), but its rollout records the call's `item_completed`, with `exit_code` and
/// `status`, under the hook's `tool_use_id` before the hook runs. One bounded read per call.
fn codex_call_failed(payload: &Value) -> bool {
    let (Some(id), Some(path)) = (
        str_field(payload, &["tool_use_id"]),
        str_field(payload, &["transcript_path"]),
    ) else {
        return false;
    };
    let needle = format!("\"id\":{}", Value::from(id));
    transcript_tail(Path::new(path), TAIL)
        .lines()
        .rev()
        .filter(|line| line.contains(&needle))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|v| {
            let item = &v["payload"]["item"];
            (v["payload"]["type"] == "item_completed" && item["id"] == id).then(|| {
                item["status"] == "failed" || item["exit_code"].as_i64().is_some_and(|c| c != 0)
            })
        })
        .unwrap_or(false)
}

/// The hook file `oboete setup grok` writes. While it exists, Grok delivers its own events.
pub fn grok_hooks_file() -> PathBuf {
    grok_home().join("hooks").join("oboete.json")
}

/// Grok Build's config directory: `$GROK_HOME`, else `~/.grok`.
pub fn grok_home() -> PathBuf {
    std::env::var_os("GROK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::home_dir().join(".grok"))
}

/// Grok Build runs Claude Code's hooks too, with its own camelCase payload: such an event is
/// Grok's, and a duplicate when Grok's own hook file carries our handler (`None` = drop it).
/// Cursor imports them as well; drop those copies so only its dedicated adapter captures and
/// injects context (docs/research/agent-adapters-2026-09-23.md).
pub fn resolve_agent<'a>(agent: &'a str, payload: &Value, grok_hooks: &Path) -> Option<&'a str> {
    if agent == "claude" && payload.get("cursor_version").is_some() {
        return None;
    }
    if agent == "claude" && payload.get("hookEventName").is_some() {
        return if grok_delivers(grok_hooks) {
            None
        } else {
            Some("grok")
        };
    }
    Some(agent)
}

/// Grok delivers our events itself while its hook file holds our handler. The file may also
/// hold the developer's own entries (setup keeps them), which prove nothing.
fn grok_delivers(grok_hooks: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(grok_hooks) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    root["hooks"].as_object().is_some_and(|events| {
        events
            .values()
            .flat_map(|groups| groups.as_array().into_iter().flatten())
            .any(crate::setup::has_ours)
    })
}

/// Directories (under the home) where agents run housekeeping sessions of their own: Codex's
/// memory consolidation works in `~/.codex/memories`, and claude-mem asks Claude through the
/// Agent SDK in `~/.claude-mem/observer-sessions` (its prompts, not the owner's, #273). Only
/// these are skipped; a repository the developer keeps elsewhere under `~/.codex` or `~/.claude`
/// (a plugin, say) is real work.
const HOUSEKEEPING_DIRS: &[&str] = &[".codex/memories", ".claude-mem/observer-sessions"];

fn is_agent_internal(agent: &str, payload: &Value) -> bool {
    let Some(cwd) = agent_workspace(agent, payload)
        .or_else(|| str_field(payload, &["cwd", "workspaceRoot"]).map(str::to_owned))
    else {
        return false;
    };
    let home = config::home_dir();
    HOUSEKEEPING_DIRS
        .iter()
        .any(|d| Path::new(&cwd).starts_with(home.join(d)))
}

fn agent_workspace(agent: &str, payload: &Value) -> Option<String> {
    match agent {
        "agy" => agy_workspace(payload),
        "cursor" => cursor_workspace(payload, std::env::var_os("CURSOR_PROJECT_DIR").as_deref()),
        _ => None,
    }
}

/// Cursor's process cwd is the config directory, never a fallback for a missing workspace.
fn cursor_workspace(payload: &Value, project_dir: Option<&std::ffi::OsStr>) -> Option<String> {
    [
        payload["cwd"].as_str().map(Path::new),
        payload["workspace_roots"]
            .as_array()
            .and_then(|roots| roots.first())
            .and_then(Value::as_str)
            .map(Path::new),
        project_dir.map(Path::new),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_absolute())
    .map(|path| path.to_string_lossy().into_owned())
}

/// agy's hook cwd is its config directory, so only its explicit workspace can identify a repo.
fn agy_workspace(payload: &Value) -> Option<String> {
    let workspace = payload["workspacePaths"].as_array()?.first()?.as_str()?;
    let path = if let Some(uri) = workspace.strip_prefix("file://") {
        let uri = uri
            .strip_prefix("localhost/")
            .map_or_else(|| uri.to_string(), |path| format!("/{path}"));
        let decoded = percent_encoding::percent_decode_str(&uri)
            .decode_utf8()
            .ok()?;
        #[cfg(windows)]
        let decoded = decoded
            .strip_prefix('/')
            .filter(|p| p.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(&decoded);
        decoded.to_string()
    } else {
        workspace.to_string()
    };
    Path::new(&path).is_absolute().then_some(path)
}

fn cursor_injection(text: &str) -> Value {
    // Cursor measures JS string length and drops the whole field above 10,000 units.
    let mut units = 0;
    let text: String = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 9_500
        })
        .collect();
    json!({"additional_context": text})
}

/// The text without the blocks in `STRIP_BLOCKS` (`<tag>` or `<tag attr…>` up to its own
/// `</tag>`, nesting counted), trimmed. An unclosed `<private>` hides the rest only when
/// `unclosed_private_hides_rest` (a typed prompt); anywhere else the tag is just text an agent
/// read or wrote, and cutting there would drop the rest of the session.
pub fn strip_blocks(s: &str, unclosed_private_hides_rest: bool) -> String {
    without_blocks(s, unclosed_private_hides_rest)
        .trim()
        .to_string()
}

pub(crate) fn without_blocks(s: &str, unclosed_private_hides_rest: bool) -> String {
    let mut out = s.to_string();
    for tag in STRIP_BLOCKS {
        out = if *tag == "claude-mem-context" {
            let mut kept = String::with_capacity(out.len());
            let mut pos = 0;
            for (start, end) in memory_context_blocks(&out) {
                kept.push_str(&out[pos..start]);
                pos = end;
            }
            kept.push_str(&out[pos..]);
            kept
        } else {
            strip_tag(&out, tag, unclosed_private_hides_rest && *tag == "private")
        };
    }
    out
}

/// The byte ranges of claude-mem's `<claude-mem-context>` blocks in `s`. claude-mem writes both
/// tags on lines of their own, so only such a line is a tag, with or without the line number a
/// Read puts in front (`1\t`, `1→`, `1|`, agy's `1: `); a tag inside a line (claude-mem's own
/// source quoting it, a pair of them too) stays text. A read cut inside a block leaves one tag
/// unpaired: the text before a lone closer (a read that began inside) or after a lone opener
/// (one that ended inside) is the block's too.
pub(crate) fn memory_context_blocks(s: &str) -> Vec<(usize, usize)> {
    static TAG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?m)^[ \t]*(?:\d+(?:\t|\x{2192}|\||:)[ \t]?)?<(/?)claude-mem-context>[ \t]*\r?$",
        )
        .expect("tag pattern")
    });
    let (mut blocks, mut open, mut from) = (Vec::new(), None, 0);
    for m in TAG.captures_iter(s) {
        let whole = m.get(0).expect("match");
        let end = whole.end() + usize::from(s[whole.end()..].starts_with('\n'));
        match (m[1].is_empty(), open) {
            (true, None) => open = Some(whole.start()),
            (false, Some(start)) => {
                blocks.push((start, end));
                (open, from) = (None, end);
            }
            // A lone closer: the read began inside the block.
            (false, None) => {
                blocks.push((from, end));
                from = end;
            }
            (true, Some(_)) => {}
        }
    }
    if let Some(start) = open {
        blocks.push((start, s.len()));
    }
    blocks
}

/// One pass over `<tag` openers and `</tag>` closers: each closer pairs with the nearest open
/// opener, and every paired block goes (nested ones inside their outer block). An opener left
/// without a closer is kept as text, except with `hide_unclosed`, where the text stops at the
/// first one. Linear in the input, so a prompt full of stray openers cannot stall the hook.
fn strip_tag(s: &str, tag: &str, hide_unclosed: bool) -> String {
    let (blocks, end) = tag_blocks(s, tag, hide_unclosed);
    let (mut out, mut pos) = (String::with_capacity(s.len()), 0);
    for (start, stop) in blocks {
        if start >= end {
            break;
        }
        if start >= pos {
            out.push_str(&s[pos..start]);
            pos = stop;
        }
    }
    out.push_str(&s[pos.min(end)..end]);
    out
}

/// `tag`'s paired blocks in `s`, sorted, and where its text ends (see `strip_tag`).
pub(crate) fn tag_blocks(s: &str, tag: &str, hide_unclosed: bool) -> (Vec<(usize, usize)>, usize) {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut marks: Vec<(usize, bool)> = s
        .match_indices(&open)
        .filter(|(i, _)| opens(&s[i + open.len()..]))
        .map(|(i, _)| (i, true))
        .chain(s.match_indices(&close).map(|(i, _)| (i, false)))
        .collect();
    marks.sort_unstable();
    let (mut stack, mut blocks) = (Vec::new(), Vec::new());
    for (i, is_open) in marks {
        if is_open {
            stack.push(i);
        } else if let Some(start) = stack.pop() {
            blocks.push((start, i + close.len()));
        }
    }
    // No paired block spans a leftover opener: the closer would have paired with it instead.
    let end = match stack.first() {
        Some(&first) if hide_unclosed => first,
        _ => s.len(),
    };
    blocks.sort_unstable();
    (blocks, end)
}

/// What follows `<tag` makes it the tag (`<privateer>` is not `<private`).
fn opens(after: &str) -> bool {
    after.starts_with(|c: char| c == '>' || c.is_whitespace())
}

/// A prompt the harness sent rather than the developer typed (see `ENVELOPES`).
pub fn is_envelope(prompt: &str) -> bool {
    ENVELOPES.iter().any(|e| prompt.starts_with(e))
}

pub(crate) fn field<'a>(v: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter().find_map(|k| v.get(*k)).unwrap_or(&Value::Null)
}

pub(crate) fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str))
}

pub(crate) fn compact(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Redact the head (plus the overlap) and keep MAX_FIELD characters of it: scanning the
/// discarded tail of a megabyte tool output would only cost hook time.
pub fn clip(s: &str) -> String {
    // Closed `<private>`-style blocks go before the cut: a block cut in half would leave an
    // opener that the outbound gate keeps as text, with the private content right after it.
    let s = &without_blocks(s, false);
    let total = s.chars().count();
    if total <= MAX_FIELD {
        return redact::redact(s);
    }
    let window: String = s.chars().take(MAX_FIELD + REDACT_OVERLAP).collect();
    let head: String = redact::redact(&window).chars().take(MAX_FIELD).collect();
    // Second pass over what is kept: gitleaks' line-scoped allowlists must judge the stored
    // line, not the wider window. Then drop a key block the rules could not match (no END).
    let mut head = redact::redact(&head);
    if let Some(begin) = head.rfind("-----BEGIN")
        && !head[begin..].contains("-----END")
    {
        head.truncate(begin);
    }
    format!("{head}\n…[clipped, {total} chars in full]")
}

/// Tail read by the hooks that run during a turn.
const TAIL: u64 = 256 * 1024;

/// At most `tail` bytes from the end, even if the agent appends while we read. Discard the first
/// partial line.
fn transcript_tail(path: &Path, tail: u64) -> String {
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > tail {
        use std::io::Seek;
        if f.seek(std::io::SeekFrom::Start(len - tail)).is_err() {
            return String::new();
        }
    }
    let mut bytes = Vec::new();
    if f.take(tail).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    if len > tail {
        let Some(newline) = bytes.iter().position(|b| *b == b'\n') else {
            return String::new();
        };
        bytes.drain(..=newline);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// (prompt, last answer) per turn in a Cursor agent transcript. Lines are
/// `{"role", "message": {"content": [{"type": "text", "text"}, {"type": "tool_use"}…]}}`
/// (agent-transcript in the cursor-agent bundle). The user text may wrap the typed prompt in
/// `<user_query>` (unverified); metadata / `turn_ended` lines have no role.
fn cursor_turns(path: &Path) -> Vec<(String, String)> {
    let mut turns: Vec<(String, String)> = Vec::new();
    let Ok(f) = std::fs::File::open(path) else {
        return turns;
    };
    // The whole conversation, one line at a time (SessionEnd is off the hot path): a turn before
    // large tool calls is still found, and the n-th repeat of a prompt is counted from the start.
    let mut reader = std::io::BufReader::new(f);
    let mut line = Vec::new();
    loop {
        line.clear();
        match std::io::BufRead::read_until(&mut reader, b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(v) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line)) else {
            continue;
        };
        let text = v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        match v["role"].as_str() {
            Some("user") => {
                let query = text
                    .split_once("<user_query>")
                    .and_then(|(_, rest)| rest.split_once("</user_query>"))
                    .map_or(text.as_str(), |(query, _)| query);
                turns.push((query.to_string(), String::new()));
            }
            Some("assistant") if !text.trim().is_empty() => {
                if let Some(turn) = turns.last_mut() {
                    turn.1 = text;
                }
            }
            _ => {}
        }
    }
    turns
}

/// Last assistant `output_text` in a Codex rollout JSONL, reading only the file's tail.
pub(crate) fn last_assistant_in_transcript(path: &Path) -> String {
    let text = transcript_tail(path, TAIL);
    let mut last = String::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] != "response_item"
            || v["payload"]["type"] != "message"
            || v["payload"]["role"] != "assistant"
        {
            continue;
        }
        if let Some(t) = v["payload"]["content"]
            .as_array()
            .and_then(|c| c.iter().rev().find_map(|x| x["text"].as_str()))
        {
            last = t.to_string();
        }
    }
    last
}

/// After every append (D6): start a worker when none holds the lock. The lock is dropped before
/// the spawn; while a worker runs, this costs one open and one failed `flock`.
fn start_worker(home: &Path) -> Result<()> {
    if std::env::var_os("OBOETE_NO_SPAWN").is_some() {
        return Ok(());
    }
    if crate::worker::lock(home)?.is_some() {
        spawn_detached(home, &["worker"]);
    }
    Ok(())
}

/// `oboete --home <home> <args>`, detached in its own process group.
fn spawn_detached(home: &Path, args: &[&str]) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--home").arg(home).args(args);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // Windows passes every inheritable handle on, this process's own stdio among them: the
    // child would hold the agent's pipes open until it exits (60 s of a worker). The handles
    // stay usable here; `Stdio::inherit` in a later spawn duplicates its own.
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        for handle in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            // SAFETY: it changes one flag of a handle of this process and fails on a null or
            // closed one, which leaves the handle as it was.
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
        }
    }
    let _ = cmd.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo;
    use rusqlite::Connection;

    fn hook(home: &Path, agent: &str, event: &str, payload: &Value) -> String {
        // Hooks run at once on several threads here: one holds the lock, the others go on.
        let _contending = crate::worker::contending();
        let _worker = crate::worker::lock(home).unwrap();
        let mut output = Vec::new();
        run_io(
            home,
            agent,
            event,
            payload.to_string().as_bytes(),
            &mut output,
        )
        .unwrap();
        String::from_utf8(output).unwrap().trim().to_owned()
    }

    fn recorded(home: &Path, agent: &str, session: &str) -> Vec<crate::raw::Event> {
        let raw = crate::raw::open(home).unwrap();
        assert!(!home.join("oboete.db").exists());
        raw.after(raw.device(), 0, 1000)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.item {
                crate::raw::Item::Event(e) if e.agent == agent && e.session == session => Some(*e),
                _ => None,
            })
            .collect()
    }

    fn built_manifest(home: &Path, cwd: &Path, prompt: &str) -> String {
        hook(
            home,
            "claude",
            "UserPromptSubmit",
            &json!({
                "session_id": "prior", "cwd": cwd, "prompt": prompt,
            }),
        );
        crate::worker::run_once(home).unwrap();
        let raw = crate::raw::open(home).unwrap();
        let settings = crate::capture::Settings::load(home).unwrap();
        let (session, repo, branch) =
            crate::capture::checkout(&json!({"session_id": "next", "cwd": cwd}), &settings);
        let text = crate::consumer::manifest::text(
            home,
            &raw,
            &repo,
            branch.as_deref().unwrap_or(""),
            &session,
            settings.rules.version(),
        )
        .unwrap()
        .unwrap();
        crate::manifest::fenced(text.trim())
    }

    /// A test's folder, removed when the test ends, passed or failed: the tests that did not remove
    /// theirs left one each in the system's temporary folder on every run (3,449 by 2026-09-30).
    struct Tmp(tempfile::TempDir);

    impl std::ops::Deref for Tmp {
        type Target = Path;
        fn deref(&self) -> &Path {
            self.0.path()
        }
    }

    impl AsRef<Path> for Tmp {
        fn as_ref(&self) -> &Path {
            self.0.path()
        }
    }

    fn tmp(name: &str) -> Tmp {
        let prefix = format!("oboete-hook-{name}-");
        Tmp(tempfile::Builder::new().prefix(&prefix).tempdir().unwrap())
    }

    #[test]
    fn a_failed_write_is_marked_and_named_at_the_next_session_start() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        // A directory where raw.db should be: every write to the store fails.
        std::fs::create_dir(home.join("raw.db")).unwrap();
        let prompt = br#"{"session_id":"s","prompt":"first"}"#;
        let mut out = Vec::new();
        assert!(run_io(home, "claude", "UserPromptSubmit", &prompt[..], &mut out).is_err());
        assert!(out.is_empty());
        let (_, first) = crate::failure::since(home).expect("marked");
        let start = br#"{"session_id":"t","source":"startup"}"#;
        let mut out = Vec::new();
        assert!(run_io(home, "claude", "SessionStart", &start[..], &mut out).is_err());
        let v: Value = serde_json::from_slice(&out).unwrap();
        let text = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(text.contains("recording has failed since"), "{text}");
        assert_eq!(crate::failure::since(home).map(|f| f.1), Some(first));
    }

    #[test]
    fn a_hook_open_timeout_marks_the_failure_before_its_agent_timeout() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        drop(crate::raw::open(home).unwrap());
        let writer = Connection::open(home.join("raw.db")).unwrap();
        writer
            .execute_batch("DROP INDEX ops_exclusions; BEGIN IMMEDIATE;")
            .unwrap();
        let _contending = crate::worker::contending();
        let _worker = crate::worker::lock(home).unwrap();
        let (started, ready) = std::sync::mpsc::channel();
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let prompt = br#"{"session_id":"s","prompt":"first"}"#;
                started.send(()).unwrap();
                let result = run_io(home, "claude", "UserPromptSubmit", &prompt[..], Vec::new());
                sent.send(result).unwrap();
            });
            ready.recv().unwrap();
            let result = received.recv_timeout(std::time::Duration::from_millis(2_500));
            writer.execute_batch("ROLLBACK").unwrap();
            assert!(
                result
                    .expect("a hook must return before its agent can kill it")
                    .is_err()
            );
            assert_eq!(
                crate::failure::since(home).map(|failure| failure.0),
                Some(crate::failure::Class::Busy)
            );
        });
    }

    #[test]
    fn a_write_to_v1_store_neither_marks_nor_clears_raw_failures() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        // raw.db cannot be written: Claude Code's hook fails and marks it.
        std::fs::create_dir(home.join("raw.db")).unwrap();
        let prompt = br#"{"session_id":"s","prompt":"first"}"#;
        assert!(run_io(home, "claude", "UserPromptSubmit", &prompt[..], Vec::new()).is_err());
        let failed = crate::failure::since(home).expect("marked");
        // An unported caller still writes v1's store: that says nothing about raw.db.
        let grok = br#"{"session_id":"g","prompt":"hello","hook_event_name":"UserPromptSubmit"}"#;
        run_io(home, "legacy", "UserPromptSubmit", &grok[..], Vec::new()).unwrap();
        assert_eq!(crate::failure::since(home), Some(failed));
    }

    #[test]
    fn a_hook_with_nothing_to_record_leaves_the_marker() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        crate::failure::mark(home, crate::failure::Class::Busy, 0);
        // A Stop with no reply captures nothing: it does not show that writes work again.
        let stop = br#"{"session_id":"s"}"#;
        let mut out = Vec::new();
        run_io(home, "claude", "Stop", &stop[..], &mut out).unwrap();
        assert!(crate::failure::since(home).is_some());
    }

    #[test]
    fn a_successful_write_clears_the_marker_and_says_nothing() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        crate::failure::mark(home, crate::failure::Class::Busy, 0);
        let start = br#"{"session_id":"t","source":"startup"}"#;
        let mut out = Vec::new();
        run_io(home, "claude", "SessionStart", &start[..], &mut out).unwrap();
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
        assert_eq!(crate::failure::since(home), None);
        let marker = home.join("state").join("recording-failed");
        assert_eq!(std::fs::metadata(marker).unwrap().len(), 64);
    }

    #[test]
    fn a_worker_that_cannot_start_is_no_recording_failure() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        // The lock cannot be opened: the worker does not start, but the row is written.
        std::fs::create_dir_all(home.join("state").join("worker.lock")).unwrap();
        let prompt = br#"{"session_id":"s","prompt":"hello"}"#;
        let mut out = Vec::new();
        run_io(home, "claude", "UserPromptSubmit", &prompt[..], &mut out).unwrap();
        assert_eq!(crate::failure::since(home), None);
        let raw = crate::raw::open(home).unwrap();
        assert_eq!(raw.max_seq().unwrap(), 1);
    }

    #[test]
    fn session_start_shows_the_manifest_with_a_directive_line_taken_back() {
        // MUST-M5 through the hook: a directive of two lines, one taken back in a later session;
        // a new session sees the other in the fence, and a resume sees nothing again.
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".git")).unwrap();
        std::fs::write(cwd.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let c = cwd.path().to_string_lossy().into_owned();
        let hook = |event: &str, payload: Value| {
            let mut out = Vec::new();
            let input = payload.to_string();
            run_io(home.path(), "claude", event, input.as_bytes(), &mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        hook(
            "UserPromptSubmit",
            json!({"session_id": "s1", "cwd": c,
                "prompt": "今後はテストを先に書いて\nコミットの前に必ず cargo fmt を通して"}),
        );
        hook(
            "UserPromptSubmit",
            json!({"session_id": "s2", "cwd": c, "prompt": "テストを先に書くのはやめて"}),
        );
        crate::worker::run_once(home.path()).unwrap();
        let out = hook(
            "SessionStart",
            json!({"session_id": "s3", "cwd": c, "source": "startup"}),
        );
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        let text = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            text.starts_with("<oboete-memory>")
                && text.contains("必ず cargo fmt を通して")
                && !text.contains("テストを先に書いて"),
            "{text}"
        );
        let resumed = hook(
            "SessionStart",
            json!({"session_id": "s3", "cwd": c, "source": "resume"}),
        );
        assert_eq!(resumed, "");
    }

    /// #94: `[inject]` cuts the manifest the hook shows at its size, or leaves it out.
    #[test]
    fn session_start_injects_the_manifest_at_the_configured_size_or_not_at_all() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".git")).unwrap();
        std::fs::write(cwd.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let c = cwd.path().to_string_lossy().into_owned();
        let hook = |event: &str, payload: Value| {
            let mut out = Vec::new();
            let input = payload.to_string();
            run_io(home.path(), "claude", event, input.as_bytes(), &mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        for i in 0..12 {
            let prompt = format!(
                "今後は手順 {i} の前に必ず{}を確かめて",
                "キャッシュと索引".repeat(8)
            );
            hook(
                "UserPromptSubmit",
                json!({"session_id": format!("s{i}"), "cwd": c, "prompt": prompt}),
            );
        }
        crate::worker::run_once(home.path()).unwrap();
        let shown = |session: &str| {
            let out = hook(
                "SessionStart",
                json!({"session_id": session, "cwd": c, "source": "startup"}),
            );
            if out.is_empty() {
                return None;
            }
            let v: Value = serde_json::from_str(out.trim()).unwrap();
            let text = v["hookSpecificOutput"]["additionalContext"].as_str();
            Some(text.unwrap().to_owned())
        };
        let fence = crate::manifest::fenced("").chars().count();
        let whole = shown("t1").unwrap();
        assert!(whole.chars().count() > fence + 1_000, "{whole}");
        // The cut ends at a line's end: it drops at most one line of the text below the size.
        let line = whole.lines().map(|l| l.chars().count() + 1).max().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[inject]\nsession_start_chars = 1000\n",
        )
        .unwrap();
        let cut = shown("t2").unwrap().chars().count();
        let low = (fence + 1_000).saturating_sub(line);
        assert!((low..=fence + 1_000).contains(&cut), "{cut}");
        std::fs::write(
            home.path().join("config.toml"),
            "[inject]\nsession_start = false\n",
        )
        .unwrap();
        assert_eq!(shown("t3"), None);
        assert_eq!(inject_text(home.path(), cwd.path(), Some("t4")), "");
        // A wrong `[inject]` injects nothing, not the defaults: its owner may have turned it off.
        std::fs::write(
            home.path().join("config.toml"),
            "[inject]\nsession_start = \"false\"\n",
        )
        .unwrap();
        assert_eq!(shown("t5"), None);
        assert_eq!(inject_text(home.path(), cwd.path(), Some("t6")), "");
    }

    #[test]
    fn pi_and_opencode_record_to_raw_and_get_the_manifest_at_session_start() {
        for agent in ["pi", "opencode"] {
            let home = tempfile::tempdir().unwrap();
            let cwd = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(cwd.path().join(".git")).unwrap();
            std::fs::write(cwd.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            let c = cwd.path().to_string_lossy().into_owned();
            let hook = |event: &str, payload: Value| {
                let mut out = Vec::new();
                let input = payload.to_string();
                run_io(home.path(), agent, event, input.as_bytes(), &mut out).unwrap();
                String::from_utf8(out).unwrap()
            };
            hook(
                "UserPromptSubmit",
                json!({"session_id": "a", "cwd": c, "prompt": "look at the <private>x</private>cache"}),
            );
            hook(
                "PostToolUse",
                json!({"session_id": "a", "cwd": c, "tool_name": "read",
                       "tool_input": {"filePath": "a.rs"}, "tool_response": "file content"}),
            );
            hook(
                "Stop",
                json!({"session_id": "a", "cwd": c, "last_assistant_message": "Done"}),
            );
            let r = crate::raw::open(home.path()).unwrap();
            let kinds: Vec<String> = r
                .after(r.device(), 0, 100)
                .unwrap()
                .into_iter()
                .filter_map(|x| match x.item {
                    crate::raw::Item::Event(e) if e.agent == agent => Some(e.kind),
                    _ => None,
                })
                .collect();
            assert_eq!(kinds, ["prompt", "tool", "reply"], "{agent}");
            crate::worker::run_once(home.path()).unwrap();
            let out = hook(
                "SessionStart",
                json!({"session_id": "b", "cwd": c, "source": "startup"}),
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            let text = v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            assert!(text.contains("look at the cache"), "{agent}: {text}");
            // OpenCode drops the hook's output and asks `oboete inject` for the same text.
            let asked = inject_text(home.path(), cwd.path(), Some("b"));
            assert_eq!(asked, text, "{agent}");
            let resumed = json!({"session_id": "b", "cwd": c, "source": "resume"});
            assert_eq!(hook("SessionStart", resumed), "", "{agent}");
            // After a compaction the context no longer has it: shown again.
            let compacted = json!({"session_id": "b", "cwd": c, "source": "compact"});
            assert!(
                hook("SessionStart", compacted).contains("look at the cache"),
                "{agent}"
            );
        }
    }

    #[test]
    fn an_idless_session_start_is_not_shown_its_own_session_as_another() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".git")).unwrap();
        std::fs::write(cwd.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let c = cwd.path().to_string_lossy().into_owned();
        let hook = |event: &str, payload: Value| {
            let mut out = Vec::new();
            let input = payload.to_string();
            run_io(home.path(), "claude", event, input.as_bytes(), &mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        hook(
            "UserPromptSubmit",
            json!({"cwd": c, "prompt": "look at it"}),
        );
        crate::worker::run_once(home.path()).unwrap();
        let out = hook("SessionStart", json!({"cwd": c, "source": "startup"}));
        assert!(out.contains("look at it"), "{out}");
        assert!(!out.contains("Other active sessions"), "{out}");
    }

    #[test]
    fn each_agent_injects_at_its_own_point_once_per_session() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let s = |id: &str| json!({"session_id": id, "source": "startup"});
        for agent in ["claude", "codex", "pi", "opencode", "cursor"] {
            assert!(injects(h, agent, "SessionStart", &s("x")), "{agent}");
        }
        let resumed = json!({"session_id": "x", "source": "resume"});
        assert!(!injects(h, "claude", "SessionStart", &resumed));
        assert!(!injects(h, "grok", "SessionStart", &s("g")));
        assert!(injects(h, "grok", "PreToolUse", &json!({"sessionId": "g"})));
        assert!(!injects(
            h,
            "grok",
            "PreToolUse",
            &json!({"sessionId": "g"})
        ));
        assert!(!injects(h, "agy", "SessionStart", &s("a")));
        let agy = json!({"conversationId": "a"});
        assert!(injects(h, "agy", "PreInvocation", &agy));
        assert!(!injects(h, "agy", "PreInvocation", &agy));
        // Cursor: the first prompt after its compaction marker, once.
        let c = json!({"session_id": "c"});
        assert!(!injects(h, "cursor", "UserPromptSubmit", &c));
        crate::hookstate::set(h, "cursor", "c", "compacted").unwrap(); // run_io, at PreCompact
        assert!(!injects(h, "cursor", "PreCompact", &c));
        assert!(injects(h, "cursor", "UserPromptSubmit", &c));
        assert!(!injects(h, "cursor", "UserPromptSubmit", &c));
    }

    #[test]
    fn session_start_shows_no_manifest_built_under_other_rules() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".git")).unwrap();
        std::fs::write(cwd.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let c = cwd.path().to_string_lossy().into_owned();
        let hook = |event: &str, payload: Value| {
            let mut out = Vec::new();
            let input = payload.to_string();
            run_io(home.path(), "claude", event, input.as_bytes(), &mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        hook(
            "UserPromptSubmit",
            json!({"session_id": "s1", "cwd": c, "prompt": "deploy acme  123456 today"}),
        );
        crate::worker::run_once(home.path()).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme {2}[0-9]{6}' }]\n",
        )
        .unwrap();
        let start = || {
            hook(
                "SessionStart",
                json!({"session_id": "s2", "cwd": c, "source": "startup"}),
            )
        };
        // Built under the old rules, and the text has its spaces flattened: the new rule cannot
        // be applied to it, so none is shown until the worker builds it again.
        let out = start();
        assert!(!out.contains("deploy") && !out.contains("123456"), "{out}");
        crate::worker::run_once(home.path()).unwrap();
        let out = start();
        assert!(out.contains("deploy") && !out.contains("123456"), "{out}");
    }

    #[test]
    fn a_home_that_cannot_be_made_is_reported_at_session_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), "").unwrap();
        let home = dir.path().join("file").join("home"); // under a file: no directory, no marker
        let start = br#"{"session_id":"s"}"#;
        let mut out = Vec::new();
        assert!(run_io(&home, "claude", "SessionStart", &start[..], &mut out).is_err());
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("recording has failed since"), "{out}");
    }

    /// Payload shapes from the Cursor event table, with all paths kept inside the test repo.
    fn cursor_fixture(dir: &Path) -> Value {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let mut payloads: Value =
            serde_json::from_str(include_str!("testdata/cursor/payloads.json")).unwrap();
        for payload in payloads.as_object_mut().unwrap().values_mut() {
            payload["workspace_roots"] = json!([dir]);
            if payload.get("cwd").is_some() {
                payload["cwd"] = json!(dir);
            }
        }
        payloads
    }

    #[test]
    fn cursor_session_start_uses_workspace_and_prints_flat_context() {
        let dir = tmp("cursor-start");
        let payloads = cursor_fixture(&dir);
        let manifest = built_manifest(&dir, &dir, "earlier Cursor work");
        let output = hook(&dir, "cursor", "SessionStart", &payloads["SessionStart"]);
        assert_eq!(
            serde_json::from_str::<Value>(&output).unwrap(),
            json!({
                "additional_context": manifest,
            })
        );
        let events = recorded(&dir, "cursor", "cursor-session");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "start");
        assert_eq!(events[0].repo, Some(repo::key(&dir)));
        assert_eq!(events[0].cwd.as_deref(), dir.to_str());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_events_capture_prompt_tool_text_and_session_end() {
        let dir = tmp("cursor-events");
        let mut payloads = cursor_fixture(&dir);
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolUseFailure",
            "Stop",
            "SessionEnd",
        ] {
            let payload = &mut payloads[event];
            // session_id wins; a conversation-only event still belongs to the same session.
            if event == "SessionEnd" {
                payload.as_object_mut().unwrap().remove("session_id");
                payload["sessionId"] = json!("not-a-cursor-field");
            } else {
                payload["conversation_id"] = json!("not-the-session");
            }
            let out = hook(&dir, "cursor", event, payload);
            assert_eq!(
                serde_json::from_str::<Value>(&out).unwrap(),
                if event == "SessionStart" {
                    json!({"additional_context":""})
                } else {
                    json!({})
                }
            );
        }
        let stored = recorded(&dir, "cursor", "cursor-session");
        assert_eq!(
            stored.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["start", "prompt", "tool", "tool", "reply", "end"]
        );
        assert!(
            stored
                .iter()
                .all(|e| e.cwd.as_deref() == dir.to_str() && e.repo == Some(repo::key(&dir)))
        );
        let bodies: Vec<Value> = stored
            .iter()
            .map(|e| serde_json::from_str(&e.body).unwrap())
            .collect();
        assert_eq!(
            bodies[1],
            json!({"prompt":"Read hello.txt and explain the result."})
        );
        assert_eq!(
            bodies[2],
            json!({"tool":"Shell", "input":"{\"command\":\"cat hello.txt\"}", "output":"{\"exitCode\":0,\"stdout\":\"hello\\n\"}", "failed":false})
        );
        assert_eq!(
            bodies[3],
            json!({"tool":"Read", "input":"{\"path\":\"missing.txt\"}", "output":"File not found: missing.txt", "failed":true})
        );
        assert_eq!(
            bodies[4],
            json!({"assistant":"hello.txt contains hello; missing.txt does not exist."})
        );
        assert_eq!(bodies[5], json!({"reason":"user_close"}));
        assert_eq!(stored.iter().filter(|e| e.kind == "prompt").count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_print_mode_session_end_recovers_turns_from_the_transcript() {
        let dir = tmp("cursor-print");
        let payloads = cursor_fixture(&dir);
        let transcript = dir.join("transcript.jsonl");
        let lines = [
            json!({"type":"metadata","metadata":{"overview":"x"}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nSay hi.\n</user_query>"}]}}),
            // A large tool call pushes the first turn out of the hot-path tail window.
            json!({"role":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"text":"x".repeat(300_000)}}]}}),
            json!({"role":"assistant","message":{"content":[{"type":"text","text":"Hi."}]}}),
            json!({"type":"turn_ended","status":"success"}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"Read hello.txt and explain the result."}]}}),
            json!({"role":"assistant","message":{"content":[{"type":"text","text":"Seen."}]}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"Keep <private>sk-secret</private> out"}]}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"Say hi."}]}}),
            json!({"role":"assistant","message":{"content":[{"type":"text","text":"Hi again."}]}}),
        ];
        let body: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&transcript, body.join("\n") + "\n").unwrap();
        // The middle turn came through the prompt hook (the TUI), so only the others are new.
        assert_eq!(
            hook(
                &dir,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            ),
            "{}"
        );
        let mut end = payloads["SessionEnd"].clone();
        end["transcript_path"] = json!(transcript);
        assert_eq!(hook(&dir, "cursor", "SessionEnd", &end), "{}");
        // A resumed `-p` run ends again with the same transcript: nothing is added twice.
        assert_eq!(hook(&dir, "cursor", "SessionEnd", &end), "{}");
        let stored: Vec<(String, Value)> = recorded(&dir, "cursor", "cursor-session")
            .into_iter()
            .map(|e| (e.kind, serde_json::from_str(&e.body).unwrap()))
            .collect();
        let prompt = |p: &str| ("prompt".to_string(), json!({"prompt": p}));
        // Task 11 keeps the transcript path in the end record.
        let end_event = (
            "end".to_string(),
            json!({"reason": "user_close", "transcript": transcript}),
        );
        assert_eq!(
            stored,
            [
                prompt("Read hello.txt and explain the result."),
                prompt("Say hi."),
                ("reply".into(), json!({"assistant":"Hi."})),
                prompt("Keep  out"),
                prompt("Say hi."),
                ("reply".into(), json!({"assistant":"Hi again."})),
                end_event.clone(),
                end_event,
            ]
        );
        assert_eq!(
            stored.iter().filter(|(kind, _)| kind == "prompt").count(),
            4
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_recovery_counts_gated_bodies_with_labels_compression_and_tombstones() {
        for store_prompts in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path();
            let payloads = cursor_fixture(home);
            std::fs::write(home.join("config.toml"), format!(
                "[capture]\nstore_prompts = {store_prompts}\n[redaction]\nextra_rules = [{{ id = 'acme', regex = 'acme-[0-9]{{6}}' }}]\n"
            )).unwrap();
            let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
            let secret = format!("acme-{}", "123456");
            let prompt = format!(
                "use {token} {secret} <private>private text</private> {} tail",
                "x".repeat(crate::capture::MAX_FIELD_BYTES)
            );
            let mut turn = payloads["UserPromptSubmit"].clone();
            turn["prompt"] = json!(prompt);
            // Same body in another agent/session must not count. One of this session's two
            // stored prompts was removed; compression must not change the remaining count.
            hook(home, "cursor", "UserPromptSubmit", &turn);
            hook(home, "cursor", "UserPromptSubmit", &turn);
            let mut raw = crate::raw::open(home).unwrap();
            raw.append_tombstone(crate::raw::Target::Record {
                device: raw.device().into(),
                seq: 2,
            })
            .unwrap();
            let compressed = raw
                .compress_through(raw.device(), 0, raw.max_seq().unwrap())
                .unwrap();
            assert!(!store_prompts || compressed > 0);
            hook(home, "grok", "UserPromptSubmit", &turn);
            let mut other = turn.clone();
            other["session_id"] = json!("another-session");
            hook(home, "cursor", "UserPromptSubmit", &other);
            let transcript = home.join("cursor.jsonl");
            let lines = [
                json!({"role":"user", "message":{"content":[{"type":"text", "text":prompt}]}}),
                json!({"role":"assistant", "message":{"content":[{"type":"text", "text":"already recorded"}]}}),
                json!({"role":"user", "message":{"content":[{"type":"text", "text":prompt.replace(&secret, &format!("acme-{}", "654321"))}]}}),
                json!({"role":"assistant", "message":{"content":[{"type":"text", "text":format!("new answer {token} <private>hidden reply</private>")}]}}),
                json!({"role":"user", "message":{"content":[{"type":"text", "text":"<task-notification>not typed</task-notification>"}]}}),
                json!({"role":"assistant", "message":{"content":[{"type":"text", "text":"envelope answer"}]}}),
            ];
            std::fs::write(
                &transcript,
                lines
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
            let mut end = payloads["SessionEnd"].clone();
            end["transcript_path"] = json!(transcript);
            for _ in 0..2 {
                assert_eq!(hook(home, "cursor", "SessionEnd", &end), "{}");
            }
            let events = recorded(home, "cursor", "cursor-session");
            assert_eq!(
                events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
                ["prompt", "prompt", "reply", "end", "end"]
            );
            assert_eq!(events[0].body, events[1].body);
            assert!(events.iter().all(|e| !e.body.contains(&token)
                && !e.body.contains("acme-")
                && !e.body.contains("private text")
                && !e.body.contains("hidden reply")
                && !e.body.contains("envelope answer")));
            assert_eq!(
                serde_json::from_str::<Value>(&events[2].body).unwrap(),
                json!({"assistant":"new answer [REDACTED] "})
            );
            if store_prompts {
                assert!(events[0].original_bytes.is_some());
                assert!(events[0].body.contains("[REDACTED]") && events[0].body.contains("tail"));
            } else {
                assert_eq!(
                    serde_json::from_str::<Value>(&events[0].body).unwrap(),
                    json!({"omitted":true})
                );
            }
        }
    }

    #[test]
    fn cursor_reinjection_uses_its_conversation_alias_not_other_agents_fields() {
        let dir = tempfile::tempdir().unwrap();
        let payloads = cursor_fixture(dir.path());
        let mut compact = payloads["PreCompact"].clone();
        compact.as_object_mut().unwrap().remove("session_id");
        compact["sessionId"] = json!("not-a-cursor-field");
        assert_eq!(hook(dir.path(), "cursor", "PreCompact", &compact), "{}");
        let output = hook(
            dir.path(),
            "cursor",
            "UserPromptSubmit",
            &payloads["UserPromptSubmit"],
        );
        assert_eq!(
            serde_json::from_str::<Value>(&output).unwrap(),
            json!({"additional_context":""})
        );
        assert_eq!(recorded(dir.path(), "cursor", "cursor-session").len(), 2);
    }

    #[test]
    fn cursor_concurrent_session_ends_recover_each_turn_once() {
        let dir = tempfile::tempdir().unwrap();
        let payloads = cursor_fixture(dir.path());
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(&path, concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"question\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"answer\"}]}}\n",
        )).unwrap();
        let mut end = payloads["SessionEnd"].clone();
        end["transcript_path"] = json!(path);
        let ready = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        let mut raw = crate::raw::open(dir.path()).unwrap();
                        ready.wait();
                        ready.wait();
                        record(
                            dir.path(),
                            &mut raw,
                            "cursor",
                            "SessionEnd",
                            &end,
                            0,
                            &Default::default(),
                        )
                        .unwrap();
                    })
                })
                .collect();
            ready.wait();
            let conn = Connection::open(dir.path().join("raw.db")).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            ready.wait();
            // Both hooks can read while the writer is busy; neither may recover that same turn.
            std::thread::sleep(std::time::Duration::from_millis(150));
            conn.execute_batch("COMMIT").unwrap();
            for thread in threads {
                thread.join().unwrap();
            }
        });
        let events = recorded(dir.path(), "cursor", "cursor-session");
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["prompt", "reply", "end", "end"]
        );
    }

    #[test]
    fn inject_shows_the_failure_line_when_raw_cannot_be_opened() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("raw.db")).unwrap(); // not a database file
        let start = json!({"session_id": "s", "cwd": cwd.path(), "source": "startup"}).to_string();
        let mut out = Vec::new();
        assert!(
            run_io(
                home.path(),
                "opencode",
                "SessionStart",
                start.as_bytes(),
                &mut out
            )
            .is_err()
        );
        let failed = crate::failure::since(home.path()).expect("marked");
        let text = inject_text(home.path(), cwd.path(), Some("s"));
        assert_eq!(text, crate::failure::line(failed));
    }

    #[test]
    fn cursor_recovery_reads_turns_before_a_large_tail() {
        // A turn followed by more than the old 16 MiB window of tool data is still recovered.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let line = |role: &str, content: Value| {
            json!({"role": role, "message": {"content": content}}).to_string() + "\n"
        };
        let big = "x".repeat(17 << 20);
        let text = line(
            "user",
            json!([{"type": "text", "text": "<user_query>early</user_query>"}]),
        ) + &line("assistant", json!([{"type": "tool_use", "input": big}]))
            + &line("assistant", json!([{"type": "text", "text": "done"}]));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            cursor_turns(&path),
            [("early".to_string(), "done".to_string())]
        );
    }

    #[test]
    fn adapter_injection_points_warn_when_their_own_write_fails() {
        for (agent, event) in [
            ("grok", "PreToolUse"),
            ("agy", "PreInvocation"),
            ("cursor", "SessionStart"),
        ] {
            let dir = tmp(&format!("fail-{agent}"));
            let payload = match agent {
                "agy" => agy_fixture(&dir)["PreInvocation"].clone(),
                "cursor" => cursor_fixture(&dir)["SessionStart"].clone(),
                _ => json!({"sessionId": "g", "workspaceRoot": &*dir,
                            "hookEventName": "PreToolUse", "toolName": "Read"}),
            };
            std::fs::create_dir_all(dir.join("raw.db")).unwrap(); // cannot be opened
            let mut out = Vec::new();
            let input = payload.to_string();
            assert!(
                run_io(&dir, agent, event, input.as_bytes(), &mut out).is_err(),
                "{agent}"
            );
            let out = String::from_utf8(out).unwrap();
            assert!(out.contains("recording has failed since"), "{agent}: {out}");
        }
    }

    #[test]
    fn a_codex_call_that_exited_non_zero_is_recorded_as_failed() {
        let dir = tmp("codex-exit");
        let rollout = dir.join("rollout.jsonl");
        let item = |id: &str, status: &str, code: i64| {
            json!({"type": "event_msg", "payload": {"type": "item_completed",
                   "item": {"type": "CommandExecution", "id": id, "status": status, "exit_code": code}}})
            .to_string()
        };
        let lines = [item("exec-1", "failed", 1), item("exec-2", "completed", 0)];
        std::fs::write(&rollout, lines.join("\n") + "\n").unwrap();
        // exec-3 is not in the rollout: nothing says it failed.
        for (id, failed) in [("exec-1", true), ("exec-2", false), ("exec-3", false)] {
            let payload = json!({"session_id": id, "cwd": &*dir, "transcript_path": rollout,
                                 "tool_name": "Bash", "tool_input": {"command": "cat /x"},
                                 "tool_response": "out", "tool_use_id": id});
            hook(&dir, "codex", "PostToolUse", &payload);
            let ev = recorded(&dir, "codex", id);
            let body: Value = serde_json::from_str(&ev[0].body).unwrap();
            assert_eq!(body["failed"], failed, "{id}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_event_in_a_nested_repository_is_filed_under_that_repository() {
        // Spec acceptance 30-5: a session that moves into a nested repository touches it too.
        // Each raw event carries the repository of its own cwd.
        let dir = tmp("nested");
        std::fs::create_dir_all(dir.join("outer/.git")).unwrap();
        std::fs::create_dir_all(dir.join("outer/inner/.git")).unwrap();
        for cwd in ["outer", "outer/inner", "outer"] {
            let payload = json!({"session_id": "s", "cwd": dir.join(cwd), "prompt": "go on"});
            hook(&dir, "claude", "UserPromptSubmit", &payload);
        }
        let repos: std::collections::BTreeSet<String> = recorded(&dir, "claude", "s")
            .into_iter()
            .filter_map(|e| e.repo)
            .collect();
        let want = [
            repo::key(&dir.join("outer")),
            repo::key(&dir.join("outer/inner")),
        ];
        assert_eq!(repos, want.into_iter().collect());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cursor_reinjects_after_a_compaction_whose_record_failed() {
        let dir = tmp("cursor-compact-failed");
        let payloads = cursor_fixture(&dir);
        hook(&dir, "cursor", "SessionStart", &payloads["SessionStart"]);
        built_manifest(&dir, &dir, "context after compaction");
        let conn = Connection::open(dir.join("raw.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_compaction BEFORE INSERT ON records WHEN NEW.kind='compaction'
            BEGIN SELECT RAISE(ABORT, 'test insert failure'); END;",
        )
        .unwrap();
        let mut output = Vec::new();
        let compact = payloads["PreCompact"].to_string();
        assert!(
            run_io(
                &dir,
                "cursor",
                "PreCompact",
                compact.as_bytes(),
                &mut output
            )
            .is_err()
        );
        assert_eq!(output, b"{}\n");
        conn.execute_batch("DROP TRIGGER refuse_compaction")
            .unwrap();
        let out = hook(
            &dir,
            "cursor",
            "UserPromptSubmit",
            &payloads["UserPromptSubmit"],
        );
        assert!(out.contains("context after compaction"), "{out}");
    }

    #[test]
    fn cursor_compaction_reinjects_once_after_cleanup_even_with_concurrent_prompts() {
        let dir = tmp("cursor-compact");
        let payloads = cursor_fixture(&dir);
        assert_eq!(
            hook(&dir, "cursor", "SessionStart", &payloads["SessionStart"]),
            r#"{"additional_context":""}"#
        );
        assert_eq!(
            hook(
                &dir,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            ),
            "{}"
        );
        assert_eq!(
            hook(&dir, "cursor", "PreCompact", &payloads["PreCompact"]),
            "{}"
        );
        let events = recorded(&dir, "cursor", "cursor-session");
        let marker = events
            .iter()
            .find(|e| e.kind == "compaction")
            .expect("compaction marker");
        assert_eq!(
            serde_json::from_str::<Value>(&marker.body).unwrap(),
            json!({"trigger":"auto"})
        );
        built_manifest(&dir, &dir, "context after compaction");
        // Processing raw records must not consume the reinjection flag.
        let conn = Connection::open(dir.join("raw.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_cursor_prompt BEFORE INSERT ON records WHEN NEW.kind='prompt'
            BEGIN SELECT RAISE(ABORT, 'test insert failure'); END;",
        )
        .unwrap();
        let mut output = Vec::new();
        assert!(
            run_io(
                &dir,
                "cursor",
                "UserPromptSubmit",
                payloads["UserPromptSubmit"].to_string().as_bytes(),
                &mut output
            )
            .is_err()
        );
        // The failure is shown at this prompt; the flag is put back for the next one.
        let shown: Value = serde_json::from_slice(&output).unwrap();
        assert!(
            shown["additional_context"]
                .as_str()
                .unwrap()
                .contains("recording has failed since")
        );
        conn.execute_batch("DROP TRIGGER refuse_cursor_prompt")
            .unwrap();
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.to_path_buf();
                let payload = payloads["UserPromptSubmit"].clone();
                std::thread::spawn(move || hook(&dir, "cursor", "UserPromptSubmit", &payload))
            })
            .collect();
        let responses: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(responses.iter().filter(|s| s.as_str() != "{}").count(), 1);
        let injected: Value =
            serde_json::from_str(responses.iter().find(|s| s.as_str() != "{}").unwrap()).unwrap();
        assert!(
            injected["additional_context"]
                .as_str()
                .unwrap()
                .contains("context after compaction")
        );
        assert_eq!(injected.as_object().unwrap().len(), 1);
        assert_eq!(
            hook(
                &dir,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            ),
            "{}"
        );
        assert_eq!(
            recorded(&dir, "cursor", "cursor-session")
                .iter()
                .filter(|e| e.kind == "prompt")
                .count(),
            6
        );
        // A second compaction permits exactly one more injection, scoped to its session.
        assert_eq!(
            hook(&dir, "cursor", "PreCompact", &payloads["PreCompact"]),
            "{}"
        );
        let mut other = payloads["UserPromptSubmit"].clone();
        other["session_id"] = json!("other-session");
        assert_eq!(hook(&dir, "cursor", "UserPromptSubmit", &other), "{}");
        assert_ne!(
            hook(
                &dir,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            ),
            "{}"
        );
        assert_eq!(
            hook(
                &dir,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            ),
            "{}"
        );
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_injection_limit_counts_utf16_without_splitting_non_bmp_characters() {
        for (input, expected) in [
            ("界".repeat(9_501), "界".repeat(9_500)),
            ("🚀".repeat(4_751), "🚀".repeat(4_750)),
            (
                format!("x{}", "🚀".repeat(4_750)),
                format!("x{}", "🚀".repeat(4_749)),
            ),
            ("context\n\"quoted\"".into(), "context\n\"quoted\"".into()),
        ] {
            let out = cursor_injection(&input);
            assert_eq!(out["additional_context"], expected);
            assert!(
                out["additional_context"]
                    .as_str()
                    .unwrap()
                    .encode_utf16()
                    .count()
                    <= 9_500
            );
            assert_eq!(out.as_object().unwrap().len(), 1);
        }
    }

    #[test]
    fn a_user_rule_masks_at_capture_and_its_version_goes_in_the_ledger() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        std::fs::write(
            home.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let prompt = br#"{"session_id":"s","prompt":"deploy with acme-123456"}"#;
        run_io(home, "claude", "UserPromptSubmit", &prompt[..], Vec::new()).unwrap();
        let version = crate::capture::Settings::load(home)
            .unwrap()
            .rules
            .version()
            .to_owned();
        assert_ne!(version, crate::redact::Rules::default().version());
        let c = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        let body: Vec<u8> = c
            .query_row("SELECT body FROM records", [], |r| r.get(0))
            .unwrap();
        assert!(!String::from_utf8(body).unwrap().contains("123456"));
        let row: (String, String) = c
            .query_row("SELECT rule, ruleset FROM ledger", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(row, ("user:acme".into(), version));
    }

    #[test]
    fn a_wrong_redaction_table_records_nothing() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        std::fs::write(
            home.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"x\", regex = '(' }]\n",
        )
        .unwrap();
        let prompt = br#"{"session_id":"s","prompt":"hello"}"#;
        let e = run_io(home, "claude", "UserPromptSubmit", &prompt[..], Vec::new()).unwrap_err();
        assert!(format!("{e:#}").contains("regex"), "{e:#}");
        assert!(!home.join("raw.db").exists());
    }

    #[test]
    fn an_idless_design_b_event_is_this_devices_own_session() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        record(
            home.path(),
            &mut raw,
            "claude",
            "UserPromptSubmit",
            &json!({"prompt": "hi"}),
            0,
            &Default::default(),
        )
        .unwrap();
        let recs = raw.after(raw.device(), 0, 10).unwrap();
        let crate::raw::Item::Event(e) = &recs[0].item else {
            panic!("{recs:?}")
        };
        assert_eq!(e.session, format!("unknown-{}", raw.device()));
    }

    #[test]
    fn leading_bom_is_accepted_for_every_agent() {
        let dir = tmp("bom");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let _worker = crate::worker::lock(&dir).unwrap();
        for agent in crate::setup::AGENTS {
            let payload = json!({"session_id":agent, "conversationId":agent, "cwd":&*dir, "workspacePaths":[&*dir], "workspace_roots":[&*dir]});
            let input = format!("\u{feff}{payload}\r\n");
            let mut output = Vec::new();
            run_io(&dir, agent, "SessionStart", input.as_bytes(), &mut output).unwrap();
            let events = recorded(&dir, agent, agent);
            assert_eq!(events.len(), 1, "{agent}");
            assert_eq!(events[0].kind, "start");
            assert_eq!(events[0].cwd.as_deref(), dir.to_str());
            match agent {
                "cursor" => assert_eq!(
                    serde_json::from_slice::<Value>(&output).unwrap(),
                    json!({"additional_context":""})
                ),
                "agy" => assert_eq!(output, b"{}\n"),
                _ => assert!(output.is_empty(), "{agent}: {output:?}"),
            }
        }
        drop(_worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_workspace_precedence_and_missing_paths_never_use_process_cwd() {
        let dir = tmp("cursor-paths");
        let cwd = dir.join("tool-repo");
        let root = dir.join("workspace");
        let env = dir.join("environment");
        let expected = |p: &Path| Some(p.to_string_lossy().into_owned());
        assert_eq!(
            cursor_workspace(
                &json!({"cwd":cwd, "workspace_roots":[root]}),
                Some(env.as_os_str())
            ),
            expected(&cwd)
        );
        assert_eq!(
            cursor_workspace(&json!({"workspace_roots":[root]}), Some(env.as_os_str())),
            expected(&root)
        );
        assert_eq!(
            cursor_workspace(&json!({"workspace_roots":[]}), Some(env.as_os_str())),
            expected(&env)
        );
        assert_eq!(cursor_workspace(&json!({}), None), None);
        for invalid in ["", ".", "relative/repo"] {
            assert_eq!(cursor_workspace(&json!({"cwd":invalid}), None), None);
            assert_eq!(
                cursor_workspace(
                    &json!({"cwd":invalid, "workspace_roots":[root]}),
                    Some(env.as_os_str())
                ),
                expected(&root)
            );
            assert_eq!(
                cursor_workspace(
                    &json!({"cwd":invalid, "workspace_roots":[invalid]}),
                    Some(env.as_os_str())
                ),
                expected(&env)
            );
            assert_eq!(
                cursor_workspace(&json!({"workspace_roots":[invalid]}), None),
                None
            );
            assert_eq!(
                cursor_workspace(&json!({}), Some(std::ffi::OsStr::new(invalid))),
                None
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_missing_workspace_and_environment_fallback_use_isolated_processes() {
        // Child test processes isolate environment changes from the parallel Rust test runner.
        const CASE_DIR: &str = "OBOETE_CURSOR_WORKSPACE_TEST_DIR";
        if let Some(dir) = std::env::var_os(CASE_DIR) {
            let dir = PathBuf::from(dir);
            let home = dir.join("store");
            let mut output = Vec::new();
            run_io(
                &home,
                "cursor",
                "SessionStart",
                &b"{\"conversation_id\":\"env-session\"}"[..],
                &mut output,
            )
            .unwrap();
            if let Some(workspace) = std::env::var_os("CURSOR_PROJECT_DIR") {
                let events = recorded(&home, "cursor", "env-session");
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].cwd.as_deref(), workspace.to_str());
                assert_eq!(events[0].repo, Some(repo::key(Path::new(&workspace))));
                assert_eq!(
                    serde_json::from_slice::<Value>(&output).unwrap(),
                    json!({"additional_context":""})
                );
            } else {
                assert_eq!(output, b"{}\n");
                assert!(
                    !home.exists(),
                    "missing workspace must not even create storage"
                );
            }
            return;
        }
        let dir = tmp("cursor-env");
        for fallback in [false, true] {
            let case = dir.join(if fallback { "fallback" } else { "missing" });
            std::fs::create_dir_all(case.join("repo/.git")).unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "hook::tests::cursor_missing_workspace_and_environment_fallback_use_isolated_processes"])
                .env(CASE_DIR, &case).env_remove(SKIP_ENV).env_remove("CURSOR_PROJECT_DIR");
            if fallback {
                command.env("CURSOR_PROJECT_DIR", case.join("repo"));
            }
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_stdout_is_empty_object_when_parsing_or_storage_fails() {
        let dir = tmp("cursor-fail-open");
        let payloads = cursor_fixture(&dir);
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, "x").unwrap();
        for input in ["invalid JSON".into(), payloads["SessionStart"].to_string()] {
            let mut output = Vec::new();
            assert!(
                run_io(
                    &blocked,
                    "cursor",
                    "SessionStart",
                    input.as_bytes(),
                    &mut output
                )
                .is_err()
            );
            assert_eq!(output, b"{}\n");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_empty_reinjection_is_consumed_and_prompts_keep_privacy_rules() {
        let dir = tmp("cursor-empty");
        let mut payloads = cursor_fixture(&dir);
        assert_eq!(
            hook(&dir, "cursor", "PreCompact", &payloads["PreCompact"]),
            "{}"
        );
        payloads["UserPromptSubmit"]["prompt"] = json!("<private>private only</private>");
        let out = hook(
            &dir,
            "cursor",
            "UserPromptSubmit",
            &payloads["UserPromptSubmit"],
        );
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({"additional_context":""})
        );
        payloads["UserPromptSubmit"]["prompt"] = json!(
            "<hook_context>not asked</hook_context>save this <private>private text</private>"
        );
        assert_eq!(
            hook(
                &dir,
                "cursor",
                "UserPromptSubmit",
                &payloads["UserPromptSubmit"]
            ),
            "{}"
        );
        let events = recorded(&dir, "cursor", "cursor-session");
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["compaction", "prompt"]
        );
        assert_eq!(
            serde_json::from_str::<Value>(&events[1].body).unwrap(),
            json!({"prompt":"save this"})
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn agy_fixture(dir: &Path) -> Value {
        let transcript = dir.join("transcript_full.jsonl");
        std::fs::write(
            &transcript,
            include_str!("testdata/agy/transcript_full.jsonl"),
        )
        .unwrap();
        let mut payloads: Value =
            serde_json::from_str(include_str!("testdata/agy/payloads.json")).unwrap();
        for payload in payloads.as_object_mut().unwrap().values_mut() {
            payload["transcriptPath"] = json!(transcript);
            payload["workspacePaths"] = json!([dir]);
        }
        payloads
    }

    #[test]
    fn agy_session_start_uses_conversation_and_workspace_without_injection() {
        let dir = tmp("agy-start");
        let payloads = agy_fixture(&dir);
        let payload = &payloads["SessionStart"];
        assert_eq!(hook(&dir, "agy", "SessionStart", payload), "{}");
        let id = payload["conversationId"].as_str().unwrap();
        let events = recorded(&dir, "agy", id);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "start");
        assert_eq!(events[0].cwd.as_deref(), dir.to_str());
        assert_eq!(events[0].repo, Some(repo::key(&dir)));
        assert!(!dir.join("state/hooks/agy").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_prompt_is_captured_once_even_after_worker_processes_raw_events() {
        let dir = tmp("agy-prompt");
        let mut payloads = agy_fixture(&dir);
        let id = payloads["PreInvocation"]["conversationId"]
            .as_str()
            .unwrap()
            .to_owned();
        drop(crate::raw::open(&dir).unwrap());
        let conn = Connection::open(dir.join("raw.db")).unwrap();
        // A failed append must release the step claim so the next hook can capture the prompt.
        conn.execute_batch(
            "CREATE TRIGGER refuse_prompt BEFORE INSERT ON records WHEN NEW.kind='prompt'
            BEGIN SELECT RAISE(ABORT, 'test insert failure'); END;",
        )
        .unwrap();
        let mut output = Vec::new();
        assert!(
            run_io(
                &dir,
                "agy",
                "PreInvocation",
                payloads["PreInvocation"].to_string().as_bytes(),
                &mut output
            )
            .is_err()
        );
        // Its injection point: the failure is shown there, in agy's shape.
        let shown: Value = serde_json::from_slice(&output).unwrap();
        assert!(
            shown["injectSteps"][0]["ephemeralMessage"]
                .as_str()
                .unwrap()
                .contains("recording has failed since")
        );
        assert!(recorded(&dir, "agy", &id).is_empty());
        conn.execute_batch("DROP TRIGGER refuse_prompt;").unwrap();
        for invocation in 0..3 {
            payloads["PreInvocation"]["invocationNum"] = json!(invocation);
            assert_eq!(
                hook(&dir, "agy", "PreInvocation", &payloads["PreInvocation"]),
                "{}"
            );
        }
        assert_eq!(hook(&dir, "agy", "Stop", &payloads["Stop"]), "{}");
        let expected = "Read the file hello.txt with your file viewing tool, then run the shell command 'ls /nonexistent-dir' and tell me the secret word and the error.";
        let events = recorded(&dir, "agy", &id);
        let prompts: Vec<_> = events.iter().filter(|e| e.kind == "prompt").collect();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&prompts[0].body).unwrap(),
            json!({"prompt":expected})
        );
        crate::worker::run_once(&dir).unwrap();
        drop(conn);
        assert_eq!(
            hook(&dir, "agy", "PreInvocation", &payloads["PreInvocation"]),
            "{}"
        );
        assert_eq!(hook(&dir, "agy", "Stop", &payloads["Stop"]), "{}");
        let count = recorded(&dir, "agy", &id)
            .iter()
            .filter(|e| e.kind == "prompt")
            .count();
        assert_eq!(count, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_stop_without_an_answer_does_not_reuse_the_previous_turns() {
        let dir = tmp("agy-noanswer");
        let payloads = agy_fixture(&dir);
        let transcript = dir.join("transcript_full.jsonl");
        let mut text = std::fs::read_to_string(&transcript).unwrap();
        text.push_str(&format!(
            "{}\n",
            json!({"step_index": 11, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE",
                   "content": "<USER_REQUEST>\nsecond question\n</USER_REQUEST>"})
        ));
        std::fs::write(&transcript, text).unwrap();
        assert_eq!(hook(&dir, "agy", "Stop", &payloads["Stop"]), "{}");
        let id = payloads["Stop"]["conversationId"].as_str().unwrap();
        let events = recorded(&dir, "agy", id);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "prompt");
        assert_eq!(
            serde_json::from_str::<Value>(&events[0].body).unwrap(),
            json!({"prompt":"second question"})
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_tool_outputs_match_step_indices_and_failures_have_both_signals() {
        let dir = tmp("agy-tool");
        let payloads = agy_fixture(&dir);
        let mut payload = payloads["PostToolUse"].clone();
        let id = payload["conversationId"].as_str().unwrap().to_owned();
        assert_eq!(hook(&dir, "agy", "PostToolUse", &payload), "{}");
        // Step 2 is before step 1 in the real fixture; the hook itself reports no error.
        payload["stepIdx"] = json!(2);
        assert_eq!(hook(&dir, "agy", "PostToolUse", &payload), "{}");
        // A hook error also fails a step that the transcript calls DONE.
        payload["stepIdx"] = json!(3);
        payload["error"] = json!("tool hook failed");
        assert_eq!(hook(&dir, "agy", "PostToolUse", &payload), "{}");
        let events = recorded(&dir, "agy", &id);
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["tool", "tool", "tool"]
        );
        let value: Value = serde_json::from_str(&events[0].body).unwrap();
        assert_eq!(value["tool"], "run_command");
        assert_eq!(value["input"], payload["toolCall"]["args"].to_string());
        assert_eq!(
            value["output"],
            "Created At: 2026-09-24T06:55:35+09:00\nCompleted At: 2026-09-24T06:55:35+09:00\n\nThe command exited with code 2.\nOutput:\nls: cannot access '/nonexistent-dir': No such file or directory\r\n\n"
        );
        assert_eq!(value["failed"], false);
        let failure: Value = serde_json::from_str(&events[1].body).unwrap();
        assert_eq!(failure["failed"], true);
        assert!(
            failure["output"]
                .as_str()
                .unwrap()
                .contains("Encountered error in step execution")
        );

        assert_eq!(
            serde_json::from_str::<Value>(&events[2].body).unwrap()["failed"],
            true
        );

        // Some error steps have no content at all.
        std::fs::write(
            dir.join("transcript_full.jsonl"),
            "{\"step_index\":3,\"status\":\"ERROR\",\"error\":\"permission denied\"}\n",
        )
        .unwrap();
        payload["error"] = json!("");
        assert_eq!(hook(&dir, "agy", "PostToolUse", &payload), "{}");
        let events = recorded(&dir, "agy", &id);
        assert_eq!(events[3].kind, "tool");
        let failure: Value = serde_json::from_str(&events[3].body).unwrap();
        assert_eq!(failure["output"], "permission denied");
        assert_eq!(failure["failed"], true);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_stop_recovers_unflushed_prompt_before_the_assistant_answer() {
        let dir = tmp("agy-stop");
        let payloads = agy_fixture(&dir);
        let transcript = dir.join("transcript_full.jsonl");
        std::fs::remove_file(&transcript).unwrap();
        assert_eq!(
            hook(&dir, "agy", "PreInvocation", &payloads["PreInvocation"]),
            "{}"
        );
        std::fs::write(
            &transcript,
            include_str!("testdata/agy/transcript_full.jsonl"),
        )
        .unwrap();
        assert_eq!(hook(&dir, "agy", "Stop", &payloads["Stop"]), "{}");
        let id = payloads["Stop"]["conversationId"].as_str().unwrap();
        let events = recorded(&dir, "agy", id);
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["prompt", "reply"]
        );
        let answer: Value = serde_json::from_str(&events[1].body).unwrap();
        assert_eq!(
            answer["assistant"],
            "[hello.txt](file:///home/dev/proj/hello.txt) の確認およびコマンド実行結果は以下のとおりです。\n\n- **秘密の言葉（secret word）**: `pineapple`\n- **コマンド実行時のエラー**:\n  ```text\n  ls: cannot access '/nonexistent-dir': No such file or directory\n  ```"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_injects_context_once_at_preinvocation_in_its_own_json_shape() {
        let dir = tmp("agy-inject");
        let payloads = agy_fixture(&dir);
        // An empty first injection point is consumed too (milestone 2 Task 2b).
        assert_eq!(
            hook(&dir, "agy", "PreInvocation", &payloads["PreInvocation"]),
            "{}"
        );
        let manifest = built_manifest(&dir, &dir, "earlier work");
        for event in ["PreInvocation", "Stop", "SessionStart", "PreInvocation"] {
            assert_eq!(hook(&dir, "agy", event, &payloads[event]), "{}");
        }
        let mut fresh = payloads["PreInvocation"].clone();
        fresh["conversationId"] = json!("fresh");
        assert_eq!(hook(&dir, "agy", "SessionStart", &fresh), "{}");
        let output = hook(&dir, "agy", "PreInvocation", &fresh);
        assert_eq!(
            serde_json::from_str::<Value>(&output).unwrap(),
            json!({
                "injectSteps": [{"ephemeralMessage": manifest}]
            })
        );
        assert_eq!(hook(&dir, "agy", "PreInvocation", &fresh), "{}");
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.to_path_buf();
                let mut payload = payloads["PreInvocation"].clone();
                payload["conversationId"] = json!("simultaneous");
                std::thread::spawn(move || hook(&dir, "agy", "PreInvocation", &payload))
            })
            .collect();
        let responses: Vec<_> = workers.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(responses.iter().filter(|s| s.as_str() != "{}").count(), 1);
        let events = recorded(&dir, "agy", "simultaneous");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "prompt");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_stdout_is_an_object_even_when_input_or_storage_fails() {
        let dir = tmp("agy-fail-open");
        let payloads = agy_fixture(&dir);
        let mut output = Vec::new();
        assert!(
            run_io(
                &dir,
                "agy",
                "SessionStart",
                &b"invalid JSON"[..],
                &mut output
            )
            .is_err()
        );
        assert_eq!(output, b"{}\n");
        let blocked_home = dir.join("not-a-directory");
        std::fs::write(&blocked_home, "x").unwrap();
        output.clear();
        assert!(
            run_io(
                &blocked_home,
                "agy",
                "SessionStart",
                payloads["SessionStart"].to_string().as_bytes(),
                &mut output
            )
            .is_err()
        );
        assert_eq!(output, b"{}\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_empty_workspaces_do_not_create_storage_and_file_uris_are_accepted() {
        let dir = tmp("agy-workspaces");
        let mut payloads = agy_fixture(&dir);
        let unused_home = dir.join("unused");
        for (event, payload) in payloads.as_object_mut().unwrap() {
            payload["workspacePaths"] = json!([]);
            // Neither a Claude-shaped field nor the process cwd can stand in for a workspace.
            payload["cwd"] = json!(&*dir);
            let mut output = Vec::new();
            run_io(
                &unused_home,
                "agy",
                event,
                payload.to_string().as_bytes(),
                &mut output,
            )
            .unwrap();
            assert_eq!(output, b"{}\n");
        }
        assert!(!unused_home.exists());
        assert!(!dir.join("raw.db").exists() && !dir.join("oboete.db").exists());
        let workspace = dir.join("workspace with spaces");
        std::fs::create_dir_all(&workspace).unwrap();
        let payload = &mut payloads["SessionStart"];
        let uri_path = workspace
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20");
        payload["workspacePaths"] = json!([format!(
            "file://{}{uri_path}",
            if cfg!(windows) { "/" } else { "" }
        )]);
        assert_eq!(hook(&dir, "agy", "SessionStart", payload), "{}");
        let events = recorded(&dir, "agy", payload["conversationId"].as_str().unwrap());
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].cwd.as_deref().map(Path::new),
            Some(workspace.as_path())
        );
        assert_eq!(events[0].repo, Some(repo::key(&workspace)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_later_steps_use_the_shared_prompt_privacy_and_envelope_rules() {
        use std::io::Write;
        let dir = tmp("agy-prompt-rules");
        let payloads = agy_fixture(&dir);
        let payload = &payloads["PreInvocation"];
        let transcript = dir.join("transcript_full.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        let secret = format!(
            "<hook_context>not asked</hook_context>key gsk_{}",
            "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8gI4kM7oQ1sV3xZ6bD"
        );
        for (index, request) in [
            (11, "same request"),
            (12, "same request"),
            (13, "<private>private only</private>"),
            (14, "<task-notification>done</task-notification>"),
            (15, secret.as_str()),
        ] {
            writeln!(file, "{}", json!({"step_index":index, "type":"USER_INPUT", "source":"USER_EXPLICIT",
                "content":format!("<USER_REQUEST>{request}</USER_REQUEST><ADDITIONAL_METADATA>not asked</ADDITIONAL_METADATA>")})).unwrap();
            // Later file lines can have lower indices; implicit input is never the user's turn.
            writeln!(
                file,
                "{}",
                include_str!("testdata/agy/transcript_full.jsonl")
                    .lines()
                    .next()
                    .unwrap()
            )
            .unwrap();
            writeln!(file, "{}", json!({"step_index":99, "type":"USER_INPUT", "source":"MODEL", "content":"<USER_REQUEST>not asked</USER_REQUEST>"})).unwrap();
            for _ in 0..2 {
                assert_eq!(hook(&dir, "agy", "PreInvocation", payload), "{}");
            }
        }
        let events = recorded(&dir, "agy", payload["conversationId"].as_str().unwrap());
        let prompts: Vec<Value> = events
            .iter()
            .filter(|e| e.kind == "prompt")
            .map(|e| serde_json::from_str::<Value>(&e.body).unwrap()["prompt"].clone())
            .collect();
        assert_eq!(prompts, ["same request", "same request", "key [REDACTED]"]);
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|e| !e.body.contains("private only")
            && !e.body.contains("not asked")
            && !e.body.contains("gsk_")));
        assert!(events[2].body.contains("task-notification"));
        assert_eq!(events[2].kind, "envelope");
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_reads_only_a_bounded_tail_and_uses_step_order_for_the_last_answer() {
        use std::io::Write;
        let dir = tmp("agy-tail");
        let payloads = agy_fixture(&dir);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("transcript_full.jsonl"))
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({"step_index":11, "type":"GENERIC", "content":"x".repeat(2 * 1024 * 1024)})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({"step_index":14, "type":"PLANNER_RESPONSE", "content":"latest answer"})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({"step_index":12, "type":"PLANNER_RESPONSE", "content":"earlier answer"})
        )
        .unwrap();
        writeln!(file, "{}", json!({"step_index":15, "type":"PLANNER_RESPONSE", "content":"  ", "thinking":"never captured"})).unwrap();
        // A partial final write must not hide the last complete response.
        write!(file, "{{\"step_index\":16").unwrap();
        for event in ["PreInvocation", "PostToolUse", "Stop"] {
            assert_eq!(hook(&dir, "agy", event, &payloads[event]), "{}");
        }
        let events = recorded(
            &dir,
            "agy",
            payloads["Stop"]["conversationId"].as_str().unwrap(),
        );
        assert_eq!(events.len(), 2); // The prompt and old tool output are outside the tail.
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["tool", "reply"]
        );
        let tool: Value = serde_json::from_str(&events[0].body).unwrap();
        assert_eq!(tool["output"], "");
        let answer: Value = serde_json::from_str(&events[1].body).unwrap();
        assert_eq!(answer, json!({"assistant":"latest answer"}));
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agent_housekeeping_sessions_are_ignored() {
        let home = config::home_dir();
        let inside = json!({"cwd": home.join(".codex").join("memories").to_string_lossy()});
        assert!(is_agent_internal("codex", &inside));
        let outside = json!({"cwd": home.join("projects").join("x").to_string_lossy()});
        assert!(!is_agent_internal("codex", &outside));
        let plugin =
            json!({"cwd": home.join(".claude").join("plugins").join("p").to_string_lossy()});
        assert!(!is_agent_internal("claude", &plugin));
        // claude-mem asks Claude through the Agent SDK there: its prompts, not the owner's (#273).
        let observer = home.join(".claude-mem").join("observer-sessions");
        assert!(is_agent_internal(
            "claude",
            &json!({"cwd": observer.to_string_lossy()})
        ));
        assert!(!is_agent_internal("claude", &json!({})));
        let stores = tempfile::tempdir().unwrap();
        for agent in crate::setup::AGENTS {
            let store = stores.path().join(agent);
            let mut payload = json!({
                "session_id": "housekeeping", "cwd": inside["cwd"],
                "workspaceRoot": inside["cwd"], "workspacePaths": [inside["cwd"]],
                "workspace_roots": [inside["cwd"]],
            });
            let mut output = Vec::new();
            run_io(
                &store,
                agent,
                "SessionStart",
                payload.to_string().as_bytes(),
                &mut output,
            )
            .unwrap();
            assert!(!store.exists(), "{agent}");
            assert_eq!(
                output.as_slice(),
                if matches!(agent, "agy" | "cursor") {
                    &b"{}\n"[..]
                } else {
                    &b""[..]
                }
            );
            for cwd in [&outside["cwd"], &plugin["cwd"]] {
                payload["cwd"] = cwd.clone();
                payload["workspaceRoot"] = cwd.clone();
                payload["workspacePaths"] = json!([cwd]);
                payload["workspace_roots"] = json!([cwd]);
                hook(&store, agent, "SessionStart", &payload);
            }
            let events = recorded(&store, agent, "housekeeping");
            assert_eq!(events.len(), 2, "{agent}");
            assert_eq!(events[0].cwd.as_deref(), outside["cwd"].as_str());
            assert_eq!(events[1].cwd.as_deref(), plugin["cwd"].as_str());
        }
    }

    #[test]
    fn secret_straddling_the_clip_boundary_is_masked() {
        let key = "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";
        let s = format!("{} {key} {}", "a".repeat(MAX_FIELD - 20), "b".repeat(3_000));
        let out = clip(&s);
        assert!(!out.contains("gsk_") && out.contains("[REDACTED]"));
        assert!(
            out.contains("…[clipped, 11038 chars in full]"),
            "{}",
            out.len()
        );
        // A key block too long for the overlap has no END in the window: cut at its BEGIN.
        let s = format!(
            "{} -----BEGIN RSA PRIVATE KEY-----\n{}",
            "a".repeat(MAX_FIELD - 40),
            "Q".repeat(9_000)
        );
        let out = clip(&s);
        assert!(!out.contains("BEGIN") && !out.contains("QQ") && out.contains("aaa"));
        assert_eq!(
            clip("x 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 y"),
            "x [REDACTED] y"
        );
    }

    #[test]
    fn a_private_block_across_the_cap_is_not_stored() {
        // Blocks go before the cap: a block cut in two would otherwise leave half of it stored.
        let dir = tmp("cap-private");
        let half = crate::capture::MAX_FIELD_BYTES / 2;
        let output = format!(
            "{} <private>{}</private> tail",
            "h".repeat(half - 100),
            "S".repeat(half)
        );
        let payload = json!({"session_id": "c1", "cwd": &*dir, "tool_name": "Read",
                             "tool_input": {}, "tool_response": output});
        hook(&dir, "claude", "PostToolUse", &payload);
        let stored = &recorded(&dir, "claude", "c1")[0].body;
        assert!(!stored.contains("SSS"), "private content stored");
        assert!(!stored.contains("<private>"), "{stored}");
        assert!(stored.contains("tail"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn stray_openers_do_not_shield_later_blocks() {
        let text = "mentions <private>, then <private>CUSTOMER DATA</private> end";
        assert_eq!(strip_blocks(text, false), "mentions <private>, then  end");
        assert_eq!(strip_blocks(text, true), "mentions");
        assert_eq!(
            strip_blocks("a <private>x <private>y</private> z</private> b", false),
            "a  b"
        );
        assert_eq!(
            strip_blocks("a </private> b <private>c</private>", false),
            "a </private> b"
        );
        // Linear: a prompt of stray openers is cheap.
        let many = "<hook_context ".repeat(50_000) + "</hook_context>";
        let start = std::time::Instant::now();
        strip_blocks(&many, true);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    /// claude-mem writes its context block into instruction files (AGENTS.md, CLAUDE.md): a file an
    /// agent reads keeps its own text, and the other memory's copy of the past is not stored.
    #[test]
    fn another_memorys_context_block_is_not_stored() {
        // Built, so this file does not hold the pair it strips when an agent reads it.
        let tag = "claude-mem-context";
        let file = format!(
            "<{tag}>\n# Memory Context\n\n### Sep 27\n| #1 | decided to use tabs |\n</{tag}>\n\n# Rules\nUse spaces."
        );
        assert_eq!(strip_blocks(&file, false), "# Rules\nUse spaces.");
        // A read cut before the block ends (`head`, a Read with a limit), with or without line
        // numbers: from the opener's line on, the text is claude-mem's.
        let cut = format!("# Title\n<{tag}>\n# Memory Context\n| #1 | decided to use tabs |");
        assert_eq!(strip_blocks(&cut, false), "# Title");
        let numbered = format!("     1\u{2192}<{tag}>\n     2\u{2192}# Memory Context");
        assert_eq!(strip_blocks(&numbered, false), "");
        let agy = format!("1: <{tag}>\n2: # Memory Context");
        assert_eq!(strip_blocks(&agy, false), "");
        // A read that starts inside the block: up to its closing line, the text is claude-mem's.
        let inside = format!("| #1 | decided to use tabs |\n</{tag}>\n\n# Rules\nUse spaces.");
        assert_eq!(strip_blocks(&inside, false), "# Rules\nUse spaces.");
        // A mention inside a line (claude-mem's own source) is text, a pair of them too.
        let source = format!("  const startTag = '<{tag}>';\n  write(startTag);");
        assert_eq!(strip_blocks(&source, false), source.trim());
        let pair =
            format!("const open = '<{tag}>';\nconst body = render();\nconst close = '</{tag}>';");
        assert_eq!(strip_blocks(&pair, false), pair);
    }

    #[test]
    fn typed_prompts_are_kept_without_private_blocks_and_harness_traffic() {
        let dir = tmp("prompts");
        let cwd = dir.to_string_lossy().to_string();
        let submit = |prompt: &str| {
            let p = json!({"session_id": "c1", "cwd": cwd, "prompt": prompt});
            assert_eq!(hook(&dir, "claude", "UserPromptSubmit", &p), "");
        };
        submit("検索を直して <private>pw hunter2</private> お願い");
        submit(
            "<task-notification>\n<summary>Background command done</summary>\n</task-notification>\nRead the output file to retrieve the result: /tmp/x.output",
        );
        submit("<private reason=\"mine\">only this</private>");
        submit("<ide_opened_file>The user opened a.rs</ide_opened_file>\n再開して");
        submit("before <private>unclosed hunter3");
        submit("<privateer> is not the tag");
        // Nested: an inner close does not end the outer block.
        submit("x <private>a <private>b</private> hunter5</private> y");
        submit("keep <private>outer <private>inner</private> hunter6");
        // Another tool's context may quote the tag; it goes first, whole.
        submit(
            "<hook_context>- bugfix: text after an unclosed <private> tag\n</hook_context>\n\nテストを足して",
        );
        let ev = recorded(&dir, "claude", "c1");
        let bodies: Vec<String> = ev
            .iter()
            .filter(|e| e.kind == "prompt")
            .map(|e| {
                serde_json::from_str::<Value>(&e.body).unwrap()["prompt"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            bodies,
            [
                "検索を直して  お願い",
                "再開して",
                "before",
                "<privateer> is not the tag",
                "x  y",
                "keep",
                "テストを足して"
            ]
        );
        // The notification is kept as an envelope; nothing private is stored, and a prompt that
        // was all private leaves no event.
        assert_eq!(ev.len(), 8);
        assert_eq!(ev[1].kind, "envelope");
        assert!(ev[1].body.contains("task-notification"));
        assert!(ev.iter().all(|e| !e.body.contains("hunter")
            && !e.body.contains("only this")
            && !e.body.contains("opened a.rs")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn grok_compat_events_are_grok_or_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let grok = json!({"hookEventName": "Stop", "sessionId": "g1", "workspaceRoot":dir.path(), "lastAssistantMessage":"compat reply"});
        let mut raw = crate::raw::open(dir.path()).unwrap();
        let mut capture = |path: &Path| {
            if let Some(agent) = resolve_agent("claude", &grok, path) {
                record(
                    dir.path(),
                    &mut raw,
                    agent,
                    "Stop",
                    &grok,
                    0,
                    &Default::default(),
                )
                .unwrap();
            }
        };
        let missing = Path::new("/nonexistent/oboete.json");
        assert_eq!(resolve_agent("claude", &grok, missing), Some("grok"));
        capture(missing);
        let installed = dir.path().join("oboete.json");
        // The developer's own entries in our file do not mean Grok delivers our events.
        std::fs::write(
            &installed,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]}]}}"#,
        )
        .unwrap();
        assert_eq!(resolve_agent("claude", &grok, &installed), Some("grok"));
        capture(&installed);
        std::fs::write(&installed, r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]},{"hooks":[{"type":"command","command":"/x/oboete hook grok Stop"}]}]}}"#).unwrap();
        assert_eq!(resolve_agent("claude", &grok, &installed), None);
        capture(&installed);
        let events = recorded(dir.path(), "grok", "g1");
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .all(|e| e.kind == "reply" && e.body == r#"{"assistant":"compat reply"}"#)
        );
        assert_eq!(raw.max_seq().unwrap(), 2);
        assert_eq!(
            resolve_agent("claude", &json!({"session_id": "c1"}), &installed),
            Some("claude")
        );
        assert_eq!(
            resolve_agent("codex", &json!({}), &installed),
            Some("codex")
        );
        let cursor = json!({"conversation_id": "k1", "cursor_version": "2026.09.15", "workspace_roots": ["/r"]});
        assert_eq!(resolve_agent("claude", &cursor, missing), None);
        let opencode = json!({"session_id": "oc1", "cwd": "/tmp/project"});
        assert_eq!(
            resolve_agent("opencode", &opencode, &installed),
            Some("opencode")
        );
    }

    /// #273: a prompt in a Codex session another agent started (`codex exec`, Claude Code's
    /// Codex plugin, a sub-agent) is stored as the agent's, from the rollout's `session_meta`.
    /// One the owner types in the TUI or VS Code, or with no rollout to read, stays the user's.
    #[test]
    fn a_codex_prompt_another_agent_sent_is_stored_as_the_agents() {
        let dir = tmp("codexsender");
        let sender = |id: &str, meta: Option<Value>| {
            let mut payload = json!({"session_id": id, "cwd": dir.to_string_lossy(), "prompt": "Review the diff."});
            // An agent's payload that marks itself the user's: the rollout decides (cubic on #275).
            payload[crate::capture::AGENT_SENT] = json!(false);
            if let Some(meta) = meta {
                let rollout = dir.join(format!("{id}.jsonl"));
                let line = json!({"type": "session_meta", "payload": meta});
                std::fs::write(&rollout, format!("{line}\n")).unwrap();
                payload["transcript_path"] = json!(rollout);
            }
            hook(&dir, "codex", "UserPromptSubmit", &payload);
            let ev = recorded(&dir, "codex", id);
            assert_eq!(ev.len(), 1, "{id}");
            assert_eq!(ev[0].kind, "prompt", "{id}");
            let body: Value = serde_json::from_str(&ev[0].body).unwrap();
            // A marker, not text: search finds the prompt by what it says only.
            assert_eq!(
                crate::consumer::fts::text(&ev[0].body),
                "Review the diff.",
                "{id}"
            );
            body["agent_sent"].clone()
        };
        let exec = json!({"originator": "codex_exec", "source": "exec"});
        assert_eq!(sender("exec", Some(exec.clone())), true);
        let plugin = json!({"originator": "Claude Code", "source": "vscode"});
        assert_eq!(sender("plugin", Some(plugin)), true);
        let sub = json!({"originator": "codex-tui", "source": {"subagent": {"thread_spawn": {}}}});
        assert_eq!(sender("sub", Some(sub)), true);
        let typed = json!({"originator": "codex-tui", "source": "vscode"});
        assert_eq!(sender("tui", Some(typed)), Value::Null);
        // Only a sub-agent's shape: another object form of the owner's session stays theirs.
        let other = json!({"originator": "codex-tui", "source": {"ide": "vscode"}});
        assert_eq!(sender("object", Some(other)), Value::Null);
        assert_eq!(sender("none", None), Value::Null);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn codex_stop_reads_last_assistant_from_transcript_tail() {
        let dir = tmp("codex");
        let transcript = dir.join("rollout.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"first\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"q\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"done: 完了 \\\"quoted\\\"\"}]}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"}}\n",
            ),
        )
        .unwrap();
        assert_eq!(
            last_assistant_in_transcript(&transcript),
            "done: 完了 \"quoted\""
        );
        assert_eq!(last_assistant_in_transcript(Path::new("/nonexistent")), "");

        let payload = json!({"session_id": "cx1", "cwd": dir.to_string_lossy(), "transcript_path": transcript.to_string_lossy()});
        hook(&dir, "codex", "Stop", &payload);
        let ev = recorded(&dir, "codex", "cx1");
        assert_eq!(ev.len(), 1);
        assert!(ev[0].body.contains("done: 完了"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grok_camelcase_fields_and_resume_without_injection() {
        let dir = tmp("grokfields");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let stop = json!({"sessionId": "g1", "workspaceRoot": cwd, "hookEventName": "Stop", "lastAssistantMessage": "grok said hi"});
        assert_eq!(hook(&dir, "grok", "Stop", &stop), "");
        let tool = json!({"sessionId": "g1", "workspaceRoot": cwd, "toolName": "Bash", "toolInput": {"cmd": "ls"}, "toolResult": "a b"});
        assert_eq!(hook(&dir, "grok", "PostToolUse", &tool), "");
        let ev = recorded(&dir, "grok", "g1");
        assert_eq!(
            ev.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["reply", "tool"]
        );
        assert_eq!(
            serde_json::from_str::<Value>(&ev[0].body).unwrap(),
            json!({"assistant":"grok said hi"})
        );
        assert_eq!(
            serde_json::from_str::<Value>(&ev[1].body).unwrap(),
            json!({
                "tool":"Bash", "input":"{\"cmd\":\"ls\"}", "output":"a b", "failed":false,
            })
        );
        assert!(
            ev.iter()
                .all(|e| e.cwd.as_deref() == Some(&cwd) && e.repo == Some(repo::key(&dir)))
        );

        // Something to inject exists for this repo …
        let manifest = built_manifest(&dir, &dir, "earlier work");
        let fresh = json!({"session_id": "g2", "cwd": cwd, "source": "startup"});
        assert!(hook(&dir, "claude", "SessionStart", &fresh).contains("earlier work"));
        // … but a resumed session already carries it.
        let resumed = json!({"session_id": "g3", "cwd": cwd, "source": "resume"});
        assert_eq!(hook(&dir, "claude", "SessionStart", &resumed), "");
        // Grok: nothing at SessionStart, once at the first tool call, never again.
        let g_start = json!({"sessionId": "g4", "workspaceRoot": cwd, "hookEventName": "SessionStart", "source": "startup"});
        assert_eq!(hook(&dir, "grok", "SessionStart", &g_start), "");
        let g_tool = json!({"sessionId": "g4", "workspaceRoot": cwd, "hookEventName": "PreToolUse", "toolName": "Read"});
        let out = hook(&dir, "grok", "PreToolUse", &g_tool);
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({
                "hookSpecificOutput": {"hookEventName":"PreToolUse", "additionalContext":manifest},
            })
        );
        assert_eq!(hook(&dir, "grok", "PreToolUse", &g_tool), "");
        std::fs::remove_dir_all(&dir).ok();
    }
}
