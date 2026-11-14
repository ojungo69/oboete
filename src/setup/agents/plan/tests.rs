    use super::{
        Agent, Code, Component, Effect, FileId, HookCommand, Input, Inputs, PI_MARKER, Plan,
        plan_agent,
    };
    use serde_json::{Value, json};
    const CANARY: &str = "PRIVATE_CANARY_NEVER_PUBLIC";
    fn command() -> HookCommand {
        HookCommand {
            exe: "/tools/oboete".into(),
            home: Some("/memory".into()),
        }
    }
    fn fixture(agent: Agent) -> Inputs {
        let mut inputs: Inputs = [
            FileId::Hooks,
            FileId::Mcp,
            FileId::Plugin,
            FileId::Extension,
            FileId::Jsonc,
        ]
        .into_iter()
        .map(|file| {
            (
                file,
                Input {
                    path: format!("/{CANARY}/{file:?}").into(),
                    text: None,
                    witness: CANARY.into(),
                    available: true,
                },
            )
        })
        .collect();
        let hooks = match agent {
            Agent::Agy => json!({"foreign":{"keep":CANARY},"oboete":{"old":true}}),
            Agent::Cursor => {
                json!({"keep":CANARY,"version":7,"hooks":{"sessionStart":[{"command":"/old/oboete hook cursor SessionStart"},{"command":"/tools/foreign"}]}})
            }
            _ => {
                json!({"keep":CANARY,"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"/old/oboete hook codex SessionStart"}]},{"hooks":[{"type":"command","command":"/tools/foreign","timeout":5}]}]}})
            }
        };
        inputs.get_mut(&FileId::Hooks).unwrap().text = Some(hooks.to_string());
        let foreign = json!({"command":"/tools/foreign","env":{"KEEP":CANARY}});
        let ours = json!({"command":"/old/oboete","args":["mcp"],"disabled":true,"timeout":42,"env":{"KEEP":CANARY}});
        let mcp = if matches!(agent, Agent::Codex | Agent::Grok) {
            let mut text = format!(
                "keep = {CANARY:?}\n[mcp_servers.foreign]\ncommand = '/tools/foreign'\n[mcp_servers.oboete]\ncommand = '/old/oboete'\nenabled = false\nstartup_timeout_sec = 42\n[mcp_servers.oboete.env]\nKEEP = {CANARY:?}\n"
            );
            if agent == Agent::Codex {
                text.push_str(&format!("[hooks.state.\"/{CANARY}/Hooks:session_start:1:0\"]\ntrusted_hash = 'foreign-hash'\nkeep = {CANARY:?}\n"));
            }
            text
        } else if agent == Agent::Opencode {
            json!({"keep":CANARY,"mcp":{"servers":{"foreign":foreign,"oboete":ours}}}).to_string()
        } else if agent == Agent::Claude {
            json!({"keep":CANARY,"mcpServers":{"foreign":foreign}}).to_string()
        } else {
            json!({"keep":CANARY,"mcpServers":{"foreign":foreign,"oboete":ours}}).to_string()
        };
        inputs.get_mut(&FileId::Mcp).unwrap().text = Some(mcp);
        inputs
    }
    fn apply(inputs: &mut Inputs, plan: &Plan) {
        for edit in &plan.edits {
            let input = inputs.get_mut(&edit.file).unwrap();
            assert_eq!((&input.path, &input.witness), (&edit.path, &edit.witness));
            input.text = edit.after.clone();
        }
        if let Some(native) = &plan.claude_mcp {
            let input = inputs.get_mut(&FileId::Mcp).unwrap();
            assert_eq!(
                (&input.path, &input.witness),
                (&native.path, &native.witness)
            );
            let mut root: Value = serde_json::from_str(input.text.as_deref().unwrap()).unwrap();
            let servers = root["mcpServers"].as_object_mut().unwrap();
            assert_eq!(servers.get("oboete"), native.old_entry.as_ref());
            match &native.wanted_entry {
                Some(entry) => {
                    servers.insert("oboete".into(), entry.clone());
                }
                None => {
                    servers.remove("oboete");
                }
            }
            input.text = Some(root.to_string());
        }
    }
    fn text(inputs: &Inputs, file: FileId) -> &str {
        inputs[&file].text.as_deref().unwrap()
    }
    fn projection(plan: &Plan) {
        let public = serde_json::to_string(&plan.steps).unwrap();
        for private in [CANARY, "/tools/", "/memory", "foreign-hash"] {
            assert!(!public.contains(private));
        }
    }
    #[test]
    fn w6a_saved_claude_payload_is_never_forwarded_to_native_argv() {
        for entry in [
            json!({"command":"/old/oboete","args":["mcp"],"env":{"TOKEN":CANARY}}),
            json!({"command":"/old/oboete","args":["--token",CANARY],"env":{}}),
            json!({"command":CANARY,"args":[],"env":{}}),
            json!({"command":"/old/oboete","args":["mcp"],"unknown":CANARY}),
        ] {
            let mut inputs = fixture(Agent::Claude);
            inputs.get_mut(&FileId::Mcp).unwrap().text =
                Some(json!({"mcpServers":{"oboete":entry}}).to_string());
            let wire = plan_agent(Agent::Claude, &command(), false, &inputs).unwrap();
            assert!(
                wire.claude_mcp.is_none(),
                "saved payload would reach native add/rollback"
            );
            assert!(
                wire.steps
                    .iter()
                    .any(|step| step.component == Component::Mcp && step.effect == Effect::Manual)
            );
            assert!(
                !wire.edits.is_empty(),
                "independent hooks must remain available"
            );
            projection(&wire);
            let unwire = plan_agent(Agent::Claude, &command(), true, &inputs).unwrap();
            assert!(unwire.claude_mcp.unwrap().wanted_entry.is_none());
        }
        let mut inputs = fixture(Agent::Claude);
        let same = super::claude_mcp_entry(
            Some(&json!({"env":{"TOKEN":CANARY},"unknown":CANARY})),
            &command(),
        );
        inputs.get_mut(&FileId::Mcp).unwrap().text =
            Some(json!({"mcpServers":{"oboete":same}}).to_string());
        let unchanged = plan_agent(Agent::Claude, &command(), false, &inputs).unwrap();
        assert!(unchanged.claude_mcp.is_none());
        assert!(
            unchanged
                .steps
                .iter()
                .any(|step| step.component == Component::Mcp && step.effect == Effect::Unchanged)
        );
    }

    #[test]
    fn seven_agents_wire_noop_unwire_preserve_foreign_settings() {
        for agent in [
            Agent::Claude,
            Agent::Codex,
            Agent::Grok,
            Agent::Agy,
            Agent::Opencode,
            Agent::Pi,
            Agent::Cursor,
        ] {
            let mut inputs = fixture(agent);
            let wire = plan_agent(agent, &command(), false, &inputs).unwrap();
            assert!(!wire.edits.is_empty() || wire.claude_mcp.is_some());
            projection(&wire);
            apply(&mut inputs, &wire);
            if matches!(agent, Agent::Codex | Agent::Grok) {
                let doc: toml_edit::DocumentMut = text(&inputs, FileId::Mcp).parse().unwrap();
                assert_eq!(
                    doc["mcp_servers"]["oboete"]["enabled"].as_bool(),
                    Some(false)
                );
                assert_eq!(
                    doc["mcp_servers"]["oboete"]["startup_timeout_sec"].as_integer(),
                    Some(42)
                );
                assert_eq!(
                    doc["mcp_servers"]["oboete"]["env"]["KEEP"].as_str(),
                    Some(CANARY)
                );
                if agent == Agent::Codex {
                    assert_eq!(
                        wire.edits
                            .iter()
                            .filter(|edit| edit.file == FileId::Mcp)
                            .count(),
                        1
                    );
                    let key = format!("/{CANARY}/Hooks:session_start:0:0");
                    assert_eq!(
                        doc["hooks"]["state"][key.as_str()]["trusted_hash"].as_str(),
                        Some("foreign-hash")
                    );
                    assert!(doc["hooks"]["state"].as_table().unwrap().len() > 1);
                }
            } else if agent == Agent::Pi {
                assert!(text(&inputs, FileId::Extension).starts_with(PI_MARKER));
            } else if agent == Agent::Claude {
                let root: Value = serde_json::from_str(text(&inputs, FileId::Mcp)).unwrap();
                assert_eq!(root["mcpServers"]["oboete"]["command"], command().exe);
                assert_eq!(root["mcpServers"]["oboete"]["env"], json!({}));
                assert_eq!(root["mcpServers"]["foreign"]["env"]["KEEP"], CANARY);
            } else {
                let root: Value = serde_json::from_str(text(&inputs, FileId::Mcp)).unwrap();
                let servers = if agent == Agent::Opencode {
                    &root["mcp"]["servers"]
                } else {
                    &root["mcpServers"]
                };
                assert_eq!(servers["oboete"]["disabled"], true);
                assert_eq!(servers["oboete"]["timeout"], 42);
                assert_eq!(servers["oboete"]["env"]["KEEP"], CANARY);
            }
            let again = plan_agent(agent, &command(), false, &inputs).unwrap();
            projection(&again);
            assert!(
                again.edits.is_empty() && again.claude_mcp.is_none(),
                "{agent:?}"
            );
            assert!(
                again
                    .steps
                    .iter()
                    .all(|step| step.effect == Effect::Unchanged)
            );
            let unwire = plan_agent(agent, &command(), true, &inputs).unwrap();
            projection(&unwire);
            apply(&mut inputs, &unwire);
            if matches!(agent, Agent::Codex | Agent::Grok) {
                let doc: toml_edit::DocumentMut = text(&inputs, FileId::Mcp).parse().unwrap();
                assert!(doc["mcp_servers"].get("oboete").is_none());
                assert_eq!(doc["keep"].as_str(), Some(CANARY));
                assert_eq!(
                    doc["mcp_servers"]["foreign"]["command"].as_str(),
                    Some("/tools/foreign")
                );
                if agent == Agent::Codex {
                    let rows = doc["hooks"]["state"].as_table().unwrap();
                    assert_eq!(rows.len(), 1);
                    assert_eq!(
                        rows[format!("/{CANARY}/Hooks:session_start:0:0").as_str()]["keep"]
                            .as_str(),
                        Some(CANARY)
                    );
                }
            } else if agent != Agent::Pi {
                let root: Value = serde_json::from_str(text(&inputs, FileId::Mcp)).unwrap();
                let servers = if agent == Agent::Opencode {
                    &root["mcp"]["servers"]
                } else {
                    &root["mcpServers"]
                };
                assert!(servers.get("oboete").is_none());
                assert_eq!(servers["foreign"]["env"]["KEEP"], CANARY);
                assert_eq!(root["keep"], CANARY);
            }
            if matches!(agent, Agent::Pi | Agent::Opencode) {
                let file = if agent == Agent::Pi {
                    FileId::Extension
                } else {
                    FileId::Plugin
                };
                assert!(inputs[&file].text.is_none());
            } else {
                let root: Value = serde_json::from_str(text(&inputs, FileId::Hooks)).unwrap();
                match agent {
                    Agent::Agy => {
                        assert!(root.get("oboete").is_none());
                        assert_eq!(root["foreign"]["keep"], CANARY);
                    }
                    Agent::Cursor => {
                        assert_eq!(root["version"], 7);
                        assert_eq!(
                            root["hooks"],
                            json!({"sessionStart":[{"command":"/tools/foreign"}]})
                        );
                    }
                    _ => assert_eq!(
                        root["hooks"],
                        json!({"SessionStart":[{"hooks":[{"type":"command","command":"/tools/foreign","timeout":5}]}]})
                    ),
                }
            }
        }
    }
    #[test]
    fn jsonc_allows_only_plugin_edit_and_keeps_private_projection() {
        let mut inputs = fixture(Agent::Opencode);
        let before = text(&inputs, FileId::Mcp).to_owned();
        let jsonc = format!("// {CANARY}\n{{}}");
        inputs.get_mut(&FileId::Jsonc).unwrap().text = Some(jsonc.clone());
        let plan = plan_agent(Agent::Opencode, &command(), false, &inputs).unwrap();
        projection(&plan);
        assert_eq!(plan.edits.len(), 1);
        assert_eq!(plan.edits[0].file, FileId::Plugin);
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step.code, Some(Code::ManualJsonc)))
        );
        apply(&mut inputs, &plan);
        assert_eq!(text(&inputs, FileId::Mcp), before);
        assert_eq!(text(&inputs, FileId::Jsonc), jsonc);
    }
    #[test]
    fn cursor_bad_or_unavailable_mcp_keeps_valid_hooks_independent() {
        for available in [true, false] {
            let mut inputs = fixture(Agent::Cursor);
            let bad = format!("{{ {CANARY}");
            let mcp = inputs.get_mut(&FileId::Mcp).unwrap();
            mcp.text = Some(bad.clone());
            mcp.available = available;
            let plan = plan_agent(Agent::Cursor, &command(), false, &inputs).unwrap();
            projection(&plan);
            assert_eq!(plan.edits.len(), 1);
            assert_eq!(plan.edits[0].file, FileId::Hooks);
            let expected = if available {
                Effect::Manual
            } else {
                Effect::Unavailable
            };
            assert!(
                plan.steps
                    .iter()
                    .any(|step| step.component == Component::Mcp && step.effect == expected)
            );
            apply(&mut inputs, &plan);
            assert_eq!(text(&inputs, FileId::Mcp), bad);
        }
    }
    #[test]
    fn pi_foreign_marker_is_kept_and_grok_owned_empty_file_is_deleted() {
        let mut inputs = fixture(Agent::Pi);
        inputs.get_mut(&FileId::Extension).unwrap().text = Some(CANARY.into());
        let plan = plan_agent(Agent::Pi, &command(), true, &inputs).unwrap();
        projection(&plan);
        assert!(plan.edits.is_empty());
        assert_eq!(
            serde_json::to_value(&plan.steps[0]).unwrap(),
            json!({"component":"extension","effect":"skipped","code":"foreign_extension_kept"})
        );
        apply(&mut inputs, &plan);
        assert_eq!(text(&inputs, FileId::Extension), CANARY);
        let mut inputs = fixture(Agent::Grok);
        inputs.get_mut(&FileId::Hooks).unwrap().text = Some("{}".into());
        let plan = plan_agent(Agent::Grok, &command(), false, &inputs).unwrap();
        apply(&mut inputs, &plan);
        let plan = plan_agent(Agent::Grok, &command(), true, &inputs).unwrap();
        assert!(
            plan.edits
                .iter()
                .any(|edit| edit.file == FileId::Hooks && edit.after.is_none())
        );
        apply(&mut inputs, &plan);
        assert!(inputs[&FileId::Hooks].text.is_none());
    }
