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
    let mut unlisted = ok.clone();
    unlisted.tools.clear();
    assert_eq!(
        verdict(&unlisted, "SECRET-x", false),
        Err("codex did not list its tools where the gate reads them".into())
    );
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
                "name": "functions", "tools": [{"type": "custom", "name": "exec"}]},
                {"type": "namespace", "name": "collaboration", "tools": [{"name": "spawn_agent"}]}]}),
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
        // A refused request may be reset rather than closed: its body was never read.
        let mut answer = String::new();
        c.read_to_string(&mut answer).ok()?;
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
    // Another agent codex runs gets no script, and its results are not the sub-agent's.
    assert_eq!(
        post("/root/guardian", &[a], &key).unwrap()["type"],
        "message"
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
        ["collaboration.spawn_agent", "functions.exec", "web_search"]
    );
    assert!(seen.login);
}

/// Another local process can find the port: what it opens is bounded in handlers, in header
/// bytes and in time, and codex is answered once it lets go.
#[test]
fn connections_that_are_not_codex_are_bounded() {
    let model = Model::start(actions()).unwrap();
    let connect = || {
        let c = TcpStream::connect(("127.0.0.1", model.port)).unwrap();
        c.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        c
    };
    // A header that never ends is cut at the cap and the connection closed.
    let mut endless = connect();
    let _ = endless.write_all(&vec![b'a'; 2 * MAX_HEADERS as usize]);
    let mut rest = Vec::new();
    assert!(
        endless
            .read_to_end(&mut rest)
            .map_or(true, |_| rest.is_empty())
    );
    // Idle connections take every handler; one more is closed unread. Which one is not fixed:
    // the handler of the connection above is counted until its thread ends, a moment after it
    // closed the connection (on the Windows runner, after the next accept).
    let idle: Vec<TcpStream> = (0..=HANDLERS).map(|_| connect()).collect();
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Closed (Linux, macOS) or reset (Windows), not left waiting.
    let closed = idle
        .iter()
        .filter(|&c| {
            let mut c: &TcpStream = c;
            c.set_read_timeout(Some(std::time::Duration::from_millis(100)))
                .unwrap();
            match c.read(&mut [0u8; 16]) {
                Ok(0) => true,
                Ok(_) => panic!("a connection past the cap was answered"),
                Err(e) => !matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ),
            }
        })
        .count();
    assert!(closed >= 1, "every connection past the cap was kept");
    drop(idle);
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Codex is answered again.
    let mut c = connect();
    let body = r#"{"input":[]}"#;
    write!(
            c,
            "POST /v1/responses HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{body}",
            model.key,
            body.len()
        )
        .unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}
