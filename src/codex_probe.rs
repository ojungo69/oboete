//! The model-driven half of codex's isolation gate (docs/spike/curator-isolation.md): the
//! curator's own `codex exec`, pointed at a scripted model on 127.0.0.1 instead of OpenAI's. The
//! script, not a model's choice, tries each action: a read, a write and a fetch through codex's
//! exec tool in its sandbox, the same with `require_escalated`, then a sub-agent that tries both
//! again. No model is called and nothing leaves the machine. Written against codex 0.155.1's
//! Responses stream; a codex that no longer runs the script fails the gate.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// What the scripted model was sent back: each thread's tool outputs, in order.
#[derive(Debug, Default, Clone)]
pub(crate) struct Seen {
    pub root: Vec<String>,
    pub sub: Vec<String>,
    /// A request carried credentials: codex sent a login to the probe's endpoint.
    pub authorized: bool,
}

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
        json!({"type": "function_call", "id": format!("fc_{n}"), "call_id": format!("call_{name}"),
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
    pub seen: Arc<Mutex<Seen>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Model {
    pub fn start(actions: Actions) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s, halt, actions) = (seen.clone(), stop.clone(), Arc::new(actions));
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                if halt.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let (s, actions) = (s.clone(), actions.clone());
                std::thread::spawn(move || {
                    let _ = answer(conn, &s, &actions);
                });
            }
        });
        Ok(Self { port, seen, stop })
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

fn answer(conn: TcpStream, seen: &Mutex<Seen>, actions: &Actions) -> std::io::Result<()> {
    conn.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let (mut length, mut line) = (0, String::new());
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("authorization:") || lower.starts_with("chatgpt-account-id:") {
            seen.lock().unwrap().authorized = true;
        }
    }
    let mut body = vec![0; length.min(MAX_REQUEST)];
    reader.read_exact(&mut body)?;
    let request: Value = serde_json::from_slice(&body).unwrap_or_default();
    let meta: Value = request["client_metadata"]["x-codex-turn-metadata"]
        .as_str()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or_default();
    let agent = meta["agent_name"].as_str().unwrap_or("?");
    let outputs: Vec<String> = request["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| {
            matches!(
                i["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            )
        })
        .map(output_text)
        .collect();
    {
        let mut seen = seen.lock().unwrap();
        let mine = if agent == "/root" {
            &mut seen.root
        } else {
            &mut seen.sub
        };
        if outputs.len() > mine.len() {
            *mine = outputs.clone();
        }
    }
    let id = format!("resp_{}_{}", agent.len(), outputs.len());
    let events = [
        json!({"type": "response.created", "response": {"id": id}}),
        json!({"type": "response.output_item.done", "item": next_item(agent, outputs.len(), actions)}),
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
    if seen.authorized {
        return Err("codex sent credentials to the probe's model".into());
    }
    if touched {
        return Err("a command wrote outside the sandbox".into());
    }
    if seen.root.len() < 3 {
        return Err("codex did not run the probe's tool calls".into());
    }
    // A sub-agent that did not run proves nothing about one that would: its results are required
    // too, as the root's are.
    if seen.sub.is_empty() {
        return Err("no sub-agent ran the probe's tool calls".into());
    }
    let threads = [("", &seen.root), ("a sub-agent: ", &seen.sub)];
    for (who, outputs) in threads {
        for (step, escalated) in [(0, false), (1, true)] {
            let Some(out) = outputs.get(step) else {
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
        Seen {
            root: root.iter().map(|s| s.to_string()).collect(),
            sub: sub.iter().map(|s| s.to_string()).collect(),
            authorized: false,
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
                seen(&[SANDBOXED, ESCALATED], SUB),
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
        let login = Seen {
            authorized: true,
            ..ok
        };
        assert!(verdict(&login, "SECRET-x", false).is_err());
    }

    /// The server end to end, with this test as codex: each request gets the script's next call,
    /// and each thread's outputs are kept.
    #[test]
    fn the_scripted_model_answers_each_thread_in_turn() {
        let model = Model::start(actions()).unwrap();
        let post = |agent: &str, outputs: &[&str], auth: bool| -> Value {
            let meta = json!({"agent_name": agent}).to_string();
            let input: Vec<Value> = outputs
                .iter()
                .map(|o| json!({"type": "custom_tool_call_output", "output": [{"type": "input_text", "text": o}]}))
                .collect();
            let body = json!({"client_metadata": {"x-codex-turn-metadata": meta}, "input": input})
                .to_string();
            let mut c = TcpStream::connect(("127.0.0.1", model.port)).unwrap();
            let auth = if auth {
                "Authorization: Bearer x\r\n"
            } else {
                ""
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
                .find(|e| e["type"] == "response.output_item.done")
                .unwrap();
            item["item"].clone()
        };
        let first = post("/root", &[], false);
        assert_eq!(
            (first["type"].as_str(), first["name"].as_str()),
            (Some("custom_tool_call"), Some("exec"))
        );
        assert!(first["input"].as_str().unwrap().contains("cat /h/secret"));
        let escalate = post("/root", &["a"], false);
        assert!(
            escalate["input"]
                .as_str()
                .unwrap()
                .contains("require_escalated")
        );
        assert_eq!(post("/root", &["a", "b"], false)["name"], "spawn_agent");
        assert_eq!(post("/root", &["a", "b", "c"], false)["name"], "wait_agent");
        assert_eq!(post("/root/probe", &["s"], false)["name"], "exec");
        assert_eq!(
            post("/root", &["a", "b", "c", "d"], true)["type"],
            "message"
        );
        let seen = model.seen.lock().unwrap().clone();
        assert_eq!(
            (seen.root.len(), seen.sub, seen.authorized),
            (4, vec!["s".to_string()], true)
        );
    }
}
