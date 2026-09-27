//! The model-driven half of codex's isolation gate (docs/spike/curator-isolation.md): the
//! curator's own `codex exec`, pointed at a scripted model on 127.0.0.1 instead of OpenAI's. The
//! script, not a model's choice, tries each action: a read, a write and a fetch through codex's
//! exec tool in its sandbox, the same with `require_escalated`, then a sub-agent that tries both
//! again. No model is called and nothing leaves the machine. Written against codex 0.155.1's
//! Responses stream; a codex that no longer runs the script fails the gate.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// What the scripted model was sent back.
#[derive(Debug, Default, Clone)]
pub(crate) struct Seen {
    /// Each tool output by the id of the script's call it answers (`call_root_0`, `call_sub_1`):
    /// a thread's results are those of the calls made to it, whatever else its requests carry.
    pub outputs: BTreeMap<String, String>,
    /// Every tool codex offered the model, as `namespace.name` (a hosted tool by its type).
    pub tools: BTreeSet<String>,
    /// A request carried a credential other than the probe's own key: codex sent a login.
    pub login: bool,
}

/// The tools codex 0.155.1 and 0.157.0 offer the model under the curator's flags (measured in the
/// dogfood user, 2026-09-27): the exec tool the gate drives, whose commands the profile governs,
/// and its wait; the sub-agent tools, whose agents run under the same profile (the gate drives one);
/// a question to a user no headless run has; and a sleep. A tool codex adds has not been reviewed
/// and fails the gate.
pub(crate) const OFFERED: [&str; 11] = [
    "clock.sleep",
    "collaboration.followup_task",
    "collaboration.interrupt_agent",
    "collaboration.list_agents",
    "collaboration.send_message",
    "collaboration.spawn_agent",
    "collaboration.wait_agent",
    "functions.exec",
    "functions.request_user_input",
    "functions.request_user_input_async",
    "functions.wait",
];

/// The commands the script runs, one line of output each (`<name>:<exit>:<output>`).
pub(crate) struct Actions {
    pub cat: String,
    pub touch: String,
    pub curl: String,
}

impl Actions {
    fn js(&self, thread: &str, escalated: bool) -> String {
        let extra = if escalated {
            r#", sandbox_permissions: "require_escalated", justification: "probe""#
        } else {
            ""
        };
        [
            ("cat", self.cat.clone()),
            ("touch", format!("{}-{thread}", self.touch)),
            ("curl", self.curl.clone()),
        ]
        .iter()
        .map(|(name, cmd)| {
            format!(
                "try {{ const r = await tools.exec_command({{cmd: {cmd}{extra}}}); \
                 text({name} + \":\" + (r.exit_code ?? \"?\") + \":\" + r.output.slice(0, 200) + \"\\n\"); }} \
                 catch (e) {{ text({name} + \":threw:\" + String(e).slice(0, 200) + \"\\n\"); }}\n",
                cmd = Value::from(cmd.as_str()),
                name = Value::from(*name),
            )
        })
        .collect()
    }
}

/// The scripted model's next output for a request from `agent` after `done` tool outputs: the
/// root runs the actions, escalates them, spawns a sub-agent and waits for it; the sub-agent runs
/// the actions and escalates them; then each ends its turn.
pub(crate) fn next_item(agent: &str, done: usize, a: &Actions) -> Value {
    let thread = if agent == "/root" { "root" } else { "sub" };
    let exec = |n: usize, escalated: bool| {
        json!({"type": "custom_tool_call", "id": format!("ct_{thread}_{n}"),
            "call_id": format!("call_{thread}_{n}"), "namespace": "functions", "name": "exec",
            "input": a.js(thread, escalated)})
    };
    let collab = |n: usize, name: &str, args: Value| {
        json!({"type": "function_call", "id": format!("fc_{n}"), "call_id": format!("call_{thread}_{n}"),
            "namespace": "collaboration", "name": name, "arguments": args.to_string()})
    };
    match (thread, done) {
        (_, 0) => exec(0, false),
        (_, 1) => exec(1, true),
        ("root", 2) => collab(
            2,
            "spawn_agent",
            json!({"task_name": "probe", "message": "probe", "fork_turns": "none"}),
        ),
        ("root", 3) => collab(3, "wait_agent", json!({"timeout_ms": 15000})),
        _ => json!({"type": "message", "role": "assistant", "id": format!("m_{thread}"),
            "content": [{"type": "output_text", "text": "done"}]}),
    }
}

/// The scripted model, serving until dropped with the listener's thread.
pub(crate) struct Model {
    pub port: u16,
    /// The key codex sends as its bearer token (from its environment, which only this user can
    /// read): a request without it is not codex's, and is neither answered nor counted.
    pub key: String,
    pub seen: Arc<Mutex<Seen>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Model {
    pub fn start(actions: Actions) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw).map_err(std::io::Error::other)?;
        let key: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s, halt, actions) = (seen.clone(), stop.clone(), Arc::new(actions));
        let bearer = Arc::new(format!("bearer {key}"));
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                if halt.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let (s, actions, bearer) = (s.clone(), actions.clone(), bearer.clone());
                std::thread::spawn(move || {
                    let _ = answer(conn, &s, &actions, &bearer);
                });
            }
        });
        Ok(Self {
            port,
            key,
            seen,
            stop,
        })
    }
}

/// The worker probes before every codex call: the listener's thread ends with the probe.
impl Drop for Model {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // wakes its accept
    }
}

/// Most one request may take: codex 0.155.1 sends about 50 KB.
const MAX_REQUEST: usize = 4 << 20;

fn answer(
    conn: TcpStream,
    seen: &Mutex<Seen>,
    actions: &Actions,
    bearer: &str,
) -> std::io::Result<()> {
    conn.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let (mut length, mut line, mut ours) = (0, String::new(), false);
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = lower.strip_prefix("authorization:") {
            ours = v.trim() == bearer;
            if !ours && v.contains("bearer") {
                seen.lock().unwrap().login = true;
            }
        }
        if lower.starts_with("chatgpt-account-id:") {
            seen.lock().unwrap().login = true;
        }
    }
    if !ours {
        let mut out = conn;
        return out.write_all(b"HTTP/1.1 401 Unauthorized\r\nconnection: close\r\n\r\n");
    }
    let mut body = vec![0; length.min(MAX_REQUEST)];
    reader.read_exact(&mut body)?;
    let request: Value = serde_json::from_slice(&body).unwrap_or_default();
    let meta: Value = request["client_metadata"]["x-codex-turn-metadata"]
        .as_str()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or_default();
    let agent = meta["agent_name"].as_str().unwrap_or("?");
    let input = request["input"].as_array().map_or(&[][..], Vec::as_slice);
    let outputs: Vec<(String, String)> = input
        .iter()
        .filter(|i| {
            matches!(
                i["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            )
        })
        .map(|i| {
            (
                i["call_id"].as_str().unwrap_or("").to_owned(),
                output_text(i),
            )
        })
        .collect();
    // This thread's own results: a request may also carry another thread's.
    let mine = format!("call_{}_", if agent == "/root" { "root" } else { "sub" });
    let done = outputs
        .iter()
        .filter(|(id, _)| id.starts_with(&mine))
        .count();
    {
        let mut seen = seen.lock().unwrap();
        tool_names(&request["tools"], "", &mut seen.tools);
        for extra in input.iter().filter(|i| i["type"] == "additional_tools") {
            tool_names(&extra["tools"], "", &mut seen.tools);
        }
        seen.outputs.extend(outputs);
    }
    let id = format!("resp_{}_{done}", agent.len());
    let events = [
        json!({"type": "response.created", "response": {"id": id}}),
        json!({"type": "response.output_item.done", "item": next_item(agent, done, actions)}),
        json!({"type": "response.completed", "response": {"id": id,
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}}),
    ];
    let mut out = conn;
    out.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    )?;
    for e in events {
        write!(
            out,
            "event: {}\ndata: {e}\n\n",
            e["type"].as_str().unwrap_or("")
        )?;
    }
    out.flush()
}

/// The names of `tools` into `out`, a namespace's tools as `namespace.name` and a hosted tool,
/// which has no name, by its type.
fn tool_names(tools: &Value, prefix: &str, out: &mut BTreeSet<String>) {
    for t in tools.as_array().into_iter().flatten() {
        let name = t["name"].as_str().or(t["type"].as_str()).unwrap_or("?");
        if t["tools"].is_array() {
            tool_names(&t["tools"], &format!("{prefix}{name}."), out);
        } else {
            out.insert(format!("{prefix}{name}"));
        }
    }
}

/// A tool output as text: a string, or the texts of an array of content items.
fn output_text(item: &Value) -> String {
    match &item["output"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .concat(),
        other => other.to_string(),
    }
}

/// Whether what the script saw proves codex could not act: every action ran and was refused, in
/// the sandbox and escalated, in the root and in any sub-agent that ran.
pub(crate) fn verdict(seen: &Seen, secret: &str, touched: bool) -> Result<(), String> {
    if seen.login {
        return Err("codex sent credentials to the probe's model".into());
    }
    if touched {
        return Err("a command wrote outside the sandbox".into());
    }
    let unreviewed: Vec<&str> = seen
        .tools
        .iter()
        .map(String::as_str)
        .filter(|t| !OFFERED.contains(t))
        .collect();
    if !unreviewed.is_empty() {
        return Err(format!(
            "codex offers the model tools the gate has not reviewed: {}",
            unreviewed.join(", ")
        ));
    }
    // A sub-agent that did not run proves nothing about one that would: its results are required
    // too, as the root's are.
    if !seen.outputs.contains_key("call_sub_0") {
        return Err("no sub-agent ran the probe's tool calls".into());
    }
    for (who, thread) in [("", "root"), ("a sub-agent: ", "sub")] {
        for (step, escalated) in [(0, false), (1, true)] {
            let Some(out) = seen.outputs.get(&format!("call_{thread}_{step}")) else {
                return Err(format!("{who}codex did not run the probe's tool calls"));
            };
            if out.contains(secret) {
                return Err(format!("{who}a command read a file under HOME"));
            }
            for name in ["cat", "touch", "curl"] {
                let Some(rest) = out
                    .lines()
                    .find_map(|l| l.strip_prefix(&format!("{name}:")))
                else {
                    return Err(format!("{who}the {name} probe did not run"));
                };
                let code = rest.split(':').next().unwrap_or("");
                // In the sandbox each ends in its own refusal (curl's is exit 7: a fetch that got
                // out waits out its time instead); escalated, codex refuses to run it at all.
                let refused = match (name, escalated) {
                    ("curl", false) => code == "7",
                    (_, false) => code.parse::<i32>().is_ok_and(|c| c != 0),
                    (_, true) => code == "threw",
                };
                if !refused {
                    let what = if escalated { " when escalated" } else { "" };
                    return Err(format!(
                        "{who}the {name} probe was not refused{what} ({code})"
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Actions {
        Actions {
            cat: "cat /h/secret".into(),
            touch: "touch /h/touched".into(),
            curl: "curl http://127.0.0.1:9/".into(),
        }
    }

    const SANDBOXED: &str = "Script completed\ncat:1:cat: /h/secret: No such file or directory\n\
        touch:1:touch: cannot touch '/h/touched-root': No such file or directory\n\
        curl:7:curl: (7) Failed to connect\n";
    const ESCALATED: &str = "cat:threw:approval policy is Never; reject command\n\
        touch:threw:approval policy is Never; reject command\n\
        curl:threw:approval policy is Never; reject command\n";

    const SUB: &[&str] = &[SANDBOXED, ESCALATED];

    fn seen(root: &[&str], sub: &[&str]) -> Seen {
        let calls = |thread: &str, outs: &[&str]| -> Vec<(String, String)> {
            outs.iter()
                .enumerate()
                .map(|(n, o)| (format!("call_{thread}_{n}"), o.to_string()))
                .collect()
        };
        Seen {
            outputs: calls("root", root)
                .into_iter()
                .chain(calls("sub", sub))
                .collect(),
            tools: OFFERED.iter().map(|t| t.to_string()).collect(),
            login: false,
        }
    }

    #[test]
    fn every_action_refused_in_the_root_and_the_sub_agent_passes() {
        let spawned = r#"{"task_name":"/root/probe"}"#;
        let ok = seen(&[SANDBOXED, ESCALATED, spawned], &[SANDBOXED, ESCALATED]);
        assert_eq!(verdict(&ok, "SECRET-x", false), Ok(()));
        // A sub-agent that did not run shows nothing about one that would.
        let refused = seen(&[SANDBOXED, ESCALATED, "collab spawn failed"], &[]);
        assert_eq!(
            verdict(&refused, "SECRET-x", false),
            Err("no sub-agent ran the probe's tool calls".into())
        );
    }

    #[test]
    fn an_action_that_was_not_refused_fails() {
        let spawned = r#"{"task_name":"/root/probe"}"#;
        let read = SANDBOXED.replace(
            "cat:1:cat: /h/secret: No such file or directory",
            "cat:0:SECRET-x",
        );
        let fetched = SANDBOXED.replace("curl:7:curl: (7) Failed to connect", "curl:28:timed out");
        let ran = ESCALATED.replace(
            "touch:threw:approval policy is Never; reject command",
            "touch:0:",
        );
        for (s, why) in [
            (seen(&[&read, ESCALATED, spawned], SUB), "read a file"),
            (
                seen(&[&fetched, ESCALATED, spawned], SUB),
                "curl probe was not refused (28)",
            ),
            (
                seen(&[SANDBOXED, &ran, spawned], SUB),
                "touch probe was not refused when escalated",
            ),
            (
                seen(&[SANDBOXED, ESCALATED, spawned], &[&read, ESCALATED]),
                "a sub-agent: a command read",
            ),
            (
                seen(&[SANDBOXED, ESCALATED, spawned], &[SANDBOXED]),
                "a sub-agent: codex did not run",
            ),
            (
                seen(&[SANDBOXED], SUB),
                "did not run the probe's tool calls",
            ),
            (
                seen(&["Script completed\n", ESCALATED, spawned], SUB),
                "the cat probe did not run",
            ),
        ] {
            let got = verdict(&s, "SECRET-x", false);
            assert!(matches!(&got, Err(w) if w.contains(why)), "{why}: {got:?}");
        }
        let ok = seen(&[SANDBOXED, ESCALATED, spawned], SUB);
        assert!(verdict(&ok, "SECRET-x", true).is_err());
        let mut more = ok.clone();
        more.tools.insert("web_search".into());
        assert_eq!(
            verdict(&more, "SECRET-x", false),
            Err("codex offers the model tools the gate has not reviewed: web_search".into())
        );
        let login = Seen { login: true, ..ok };
        assert!(verdict(&login, "SECRET-x", false).is_err());
    }

    /// The server end to end, with this test as codex: each request with the key gets the
    /// script's next call, and each thread's outputs are kept; any other request is refused.
    #[test]
    fn the_scripted_model_answers_each_thread_in_turn_and_only_codex() {
        let model = Model::start(actions()).unwrap();
        let key = format!("Bearer {}", model.key);
        let post = |agent: &str, outputs: &[(&str, &str)], auth: &str| -> Option<Value> {
            let meta = json!({"agent_name": agent}).to_string();
            let mut input: Vec<Value> = outputs
                .iter()
                .map(|(id, o)| {
                    json!({"type": "custom_tool_call_output", "call_id": id,
                    "output": [{"type": "input_text", "text": o}]})
                })
                .collect();
            input.push(
                json!({"type": "additional_tools", "tools": [{"type": "namespace",
                "name": "functions", "tools": [{"type": "custom", "name": "exec"}]}]}),
            );
            let body = json!({"client_metadata": {"x-codex-turn-metadata": meta}, "input": input,
                "tools": [{"type": "web_search"}]})
            .to_string();
            let mut c = TcpStream::connect(("127.0.0.1", model.port)).unwrap();
            let auth = if auth.is_empty() {
                String::new()
            } else {
                format!("Authorization: {auth}\r\n")
            };
            write!(
                c,
                "POST /v1/responses HTTP/1.1\r\nContent-Length: {}\r\n{auth}\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut answer = String::new();
            c.read_to_string(&mut answer).unwrap();
            let item = answer
                .lines()
                .filter_map(|l| l.strip_prefix("data: "))
                .filter_map(|d| serde_json::from_str::<Value>(d).ok())
                .find(|e| e["type"] == "response.output_item.done")?;
            Some(item["item"].clone())
        };
        let first = post("/root", &[], &key).unwrap();
        assert_eq!(
            (first["type"].as_str(), first["name"].as_str()),
            (Some("custom_tool_call"), Some("exec"))
        );
        assert!(first["input"].as_str().unwrap().contains("cat /h/secret"));
        let (a, b, c, d) = (
            ("call_root_0", "a"),
            ("call_root_1", "b"),
            ("call_root_2", "c"),
            ("call_root_3", "d"),
        );
        let escalate = post("/root", &[a], &key).unwrap();
        assert!(
            escalate["input"]
                .as_str()
                .unwrap()
                .contains("require_escalated")
        );
        assert_eq!(post("/root", &[a, b], &key).unwrap()["name"], "spawn_agent");
        assert_eq!(
            post("/root", &[a, b, c], &key).unwrap()["call_id"],
            "call_root_3"
        );
        // A sub-agent whose requests carry the root's results starts its own calls all the same,
        // and those results are not taken for its own.
        let first = post("/root/probe", &[a, b, c, d], &key).unwrap();
        assert_eq!(
            (&first["name"], &first["call_id"]),
            (&json!("exec"), &json!("call_sub_0"))
        );
        let mut only_root = model.seen.lock().unwrap().clone();
        only_root.tools.remove("web_search");
        assert_eq!(
            verdict(&only_root, "SECRET-x", false),
            Err("no sub-agent ran the probe's tool calls".into())
        );
        assert_eq!(
            post("/root/probe", &[a, ("call_sub_0", "s")], &key).unwrap()["call_id"],
            "call_sub_1"
        );
        assert_eq!(
            post("/root", &[a, b, c, d], &key).unwrap()["type"],
            "message"
        );
        // Another local process knows the port, not the key: refused, and not counted.
        let forged = [("call_sub_1", "cat:1:x"), ("call_root_9", "x")];
        assert!(post("/root", &forged, "").is_none());
        assert!(!model.seen.lock().unwrap().login);
        // A credential that is not the key is a login codex sent: refused, and it fails the gate.
        assert!(post("/root", &forged, "Bearer sk-other").is_none());
        let seen = model.seen.lock().unwrap().clone();
        assert_eq!(
            seen.outputs.keys().collect::<Vec<_>>(),
            [
                "call_root_0",
                "call_root_1",
                "call_root_2",
                "call_root_3",
                "call_sub_0"
            ]
        );
        assert_eq!(
            seen.tools.iter().collect::<Vec<_>>(),
            ["functions.exec", "web_search"]
        );
        assert!(seen.login);
    }
}
