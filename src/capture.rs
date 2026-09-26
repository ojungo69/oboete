//! Design B capture (docs/milestone-2-plan.md Tasks 2 and 3; spec 2.1-2.4): one hook payload
//! becomes the events appended to `raw.db`, each with the ledger of what redaction masked. Text is
//! kept whole up to `MAX_FIELD_BYTES`, head and tail above it; every stored string loses its
//! `<private>`-style blocks and is scanned in full; images and other base64 content become a
//! marker; git fields are read from files, never from `git`.
//! Agents move here one by one: `PORTED` lists those done, and the others still go through
//! `hook::handle` into v1's store until their port lands.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::hook::{compact, field, is_envelope, str_field, strip_blocks, without_blocks};
use crate::raw::Event;
use crate::{redact, repo};

pub const PORTED: &[&str] = &["claude", "codex"];

/// Bytes a stored string keeps before only its head and tail are kept (spec 2.4, plan D4).
/// Provisional: Task 12 sets it from M14 on the slowest machine.
pub const MAX_FIELD_BYTES: usize = 256 * 1024;

/// One event as captured, with the ledger rows of what its redaction masked: (field, finding).
#[derive(Debug)]
pub struct Captured {
    pub event: Event,
    pub ledger: Vec<(String, redact::Finding)>,
}

/// The events one hook call of a ported agent records: none for events that carry nothing.
pub fn events(agent: &str, event: &str, payload: &Value, ts: i64) -> Vec<Captured> {
    let (kind, body) = match event {
        "SessionStart" => ("start", json!({"source": payload.get("source").map(clean)})),
        "UserPromptSubmit" => {
            let prompt = strip_blocks(str_field(payload, &["prompt"]).unwrap_or(""), true);
            if prompt.is_empty() {
                return Vec::new();
            }
            // Harness traffic is recorded, but never as something the developer typed.
            let kind = if is_envelope(&prompt) {
                "envelope"
            } else {
                "prompt"
            };
            (kind, json!({"prompt": base64_runs(&prompt)}))
        }
        "PostToolUse" | "PostToolUseFailure" => {
            let mut body = json!({
                "tool": without_blocks(str_field(payload, &["tool_name", "toolName", "name"]).unwrap_or("?"), false),
                "input": text(field(payload, &["tool_input", "toolInput", "args"])),
                "output": text(field(payload, &["tool_response", "toolResult", "tool_output", "error"])),
                "failed": event == "PostToolUseFailure",
            });
            // Provenance and status when the payload has them: a subagent's call, and a call
            // that never got its answer (both from `oboete transcript`, src/transcript.rs).
            for key in ["agent_id", "interrupted"] {
                if let Some(v) = payload.get(key) {
                    body[key] = clean(v);
                }
            }
            ("tool", body)
        }
        "Stop" => {
            // Codex's Stop carries no message; its rollout transcript does.
            let reply = match str_field(payload, &["last_assistant_message"]) {
                Some(t) => t.to_string(),
                None => str_field(payload, &["transcript_path"])
                    .map(|p| crate::hook::last_assistant_in_transcript(Path::new(p)))
                    .unwrap_or_default(),
            };
            let reply = without_blocks(&reply, false);
            if reply.trim().is_empty() {
                return Vec::new();
            }
            ("reply", json!({"assistant": base64_runs(&reply)}))
        }
        "PreCompact" => (
            "compaction",
            json!({"trigger": payload.get("trigger").map(clean)}),
        ),
        "PostCompact" => {
            match str_field(payload, &["compact_summary"]).map(|s| without_blocks(s, false)) {
                Some(s) if !s.trim().is_empty() => {
                    ("compaction", json!({"summary": base64_runs(&s)}))
                }
                _ => return Vec::new(),
            }
        }
        "SessionEnd" => ("end", json!({"reason": payload.get("reason").map(clean)})),
        _ => return Vec::new(), // PreToolUse and the rest carry nothing to keep
    };
    let cwd = str_field(payload, &["cwd"]).unwrap_or(".");
    let git = git(Path::new(cwd));
    // Labels are stored text too (spec 2.2): a path or branch can carry a token.
    let mut gate = Gate::default();
    let mut label = |field: &str, s: &str| gate.text(field, &without_blocks(s, false));
    let session = label(
        "session",
        str_field(payload, &["session_id", "sessionId"]).unwrap_or("unknown"),
    );
    let repo = label("repo", &repo::key(Path::new(cwd)));
    let branch = git.branch.as_deref().map(|b| label("branch", b));
    let head = git.head.as_deref().map(|h| label("head", h));
    let gitdir = git.gitdir.as_deref().map(|g| label("gitdir", g));
    let cwd_label = label("cwd", cwd);
    // One gate for the body: every string and key in it, whatever field it is.
    let body = gate.value("", body).to_string();
    vec![Captured {
        event: Event {
            agent: agent.into(),
            // A label only: an event without one is still this device's next seq.
            session,
            kind: kind.into(),
            ts,
            repo: Some(repo),
            branch,
            head,
            gitdir,
            cwd: Some(cwd_label),
            source: "hook".into(),
            body,
            original_bytes: gate.cut,
        },
        ledger: gate.ledger,
    }]
}

/// The one gate every stored string passes (spec 2.2): `redact::scan_capped` over its whole
/// length (head and tail above the cap), each finding kept with the field it is in.
#[derive(Default)]
struct Gate {
    ledger: Vec<(String, redact::Finding)>,
    /// The full size of the strings that were cut, when any was.
    cut: Option<i64>,
}

impl Gate {
    fn text(&mut self, field: &str, s: &str) -> String {
        let (stored, found, full) = redact::scan_capped(s, MAX_FIELD_BYTES);
        if let Some(n) = full {
            *self.cut.get_or_insert(0) += n as i64;
        }
        self.ledger
            .extend(found.into_iter().map(|f| (field.to_owned(), f)));
        stored
    }

    /// Every string and key of `v`, at its JSON pointer. Blocks are gone by now: stripping here
    /// again would pair an opener left in one flattened tool field with a closer in another.
    fn value(&mut self, path: &str, v: Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(path, &s)),
            Value::Array(a) => Value::Array(
                a.into_iter()
                    .enumerate()
                    .map(|(i, x)| self.value(&format!("{path}/{i}"), x))
                    .collect(),
            ),
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, x)| {
                        // The pointer is built from the stored key, so it never holds a secret.
                        let key = self.text(&format!("{path}#key"), &k);
                        let child = format!("{path}/{}", segment(&key));
                        let x = self.value(&child, x);
                        (key, x)
                    })
                    .collect(),
            ),
            other => other,
        }
    }
}

/// A key as one JSON pointer segment (`~0`, `~1` escaped). A key over 128 bytes is named by
/// `~sha:` and the first 16 hex digits of its sha256 instead: thousands of findings under a huge
/// key would otherwise each copy it into the ledger. `~s` is no valid escape, so the name
/// cannot be mistaken for a key.
fn segment(key: &str) -> String {
    if key.len() > 128 {
        let sha: String = Sha256::digest(key.as_bytes())[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        return format!("~sha:{sha}");
    }
    key.replace('~', "~0").replace('/', "~1")
}

/// A tool field as text: `clean`, then flattened. `Gate` scans it with the rest of the body.
fn text(v: &Value) -> String {
    compact(&clean(v))
}

/// A payload value as stored, string by string, so a block never pairs across two fields: closed
/// `<private>`-style blocks removed without a trim (stored text keeps its whitespace, as v1's
/// `clip` did), and base64 replaced by `{kind, mime, bytes, sha256}` (spec 2.3).
fn clean(v: &Value) -> Value {
    match v {
        Value::Object(m) => binary(m).unwrap_or_else(|| {
            Value::Object(
                m.iter()
                    .map(|(k, x)| (without_blocks(k, false), clean(x)))
                    .collect(),
            )
        }),
        Value::Array(a) => Value::Array(a.iter().map(clean).collect()),
        Value::String(s) => base64_runs(&without_blocks(s, false)),
        other => other.clone(),
    }
}

/// The object shapes seen in agent payloads: a content block `{type, source: {type: base64,
/// media_type, data}}`, Claude Code's image read result `{type: <mime>, base64}`, and MCP's
/// `{type, mimeType, data}`. OpenAI's `image_url` is a data URI string (`base64_runs`).
fn binary(m: &Map<String, Value>) -> Option<Value> {
    let kind = m.get("type").and_then(Value::as_str);
    let (kind, mime, data) = if let Some(src) = m.get("source").filter(|s| s["type"] == "base64") {
        (kind?, src["media_type"].as_str()?, src["data"].as_str()?)
    } else if let Some(data) = m.get("base64").and_then(Value::as_str) {
        let mime = kind?;
        (mime.split('/').next()?, mime, data)
    } else {
        (
            kind?,
            m.get("mimeType")?.as_str()?,
            m.get("data")?.as_str()?,
        )
    };
    Some(marker(kind, mime, data))
}

/// Base64 in a string: a `data:<mime>;base64,` URI (OpenAI's `image_url`, an inline `<img>`), or
/// any run of 1,024 or more base64 characters, whatever shape carried it (the design's length check,
/// docs/research/redesign-2026-09-24/constraints-synthesis.md S2-25). A whole-string match becomes
/// the marker object; one inside other text becomes the marker's JSON.
/// ponytail: line-wrapped base64 (76 characters a line) is not caught; add when one shows up.
fn base64_runs(s: &str) -> Value {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"data:([\w.+-]+/[\w.+-]+);base64,([A-Za-z0-9+/]+=*)|[A-Za-z0-9+/_-]{1024,}={0,2}",
        )
        .expect("base64 pattern")
    });
    // A bare run counts only if it looks like encoded bytes: all three character classes, and a
    // length base64 can have. A long lowercase or hex run, or an identifier, stays text.
    let found = |c: &regex::Captures| -> Option<Value> {
        if let (Some(mime), Some(data)) = (c.get(1), c.get(2)) {
            let mime = mime.as_str();
            return Some(marker(
                mime.split('/').next().unwrap_or(mime),
                mime,
                data.as_str(),
            ));
        }
        let run = c[0].trim_end_matches('=');
        let has = |class: fn(&char) -> bool| run.chars().any(|ch| class(&ch));
        (has(char::is_ascii_uppercase)
            && has(char::is_ascii_lowercase)
            && has(char::is_ascii_digit)
            && run.len() % 4 != 1)
            .then(|| marker("binary", "application/octet-stream", &c[0]))
    };
    match RE.captures(s) {
        Some(c) if c[0].len() == s.len() => {
            found(&c).unwrap_or_else(|| Value::String(s.to_owned()))
        }
        None => Value::String(s.to_owned()),
        Some(_) => Value::String(
            RE.replace_all(s, |c: &regex::Captures| {
                found(c).map_or_else(|| c[0].to_string(), |m| m.to_string())
            })
            .into_owned(),
        ),
    }
}

/// `sha256` is of the base64 text, `bytes` the decoded size. `kind` and `mime` come from the
/// payload, so they are cleaned like any other string.
fn marker(kind: &str, mime: &str, data: &str) -> Value {
    let (kind, mime) = (without_blocks(kind, false), without_blocks(mime, false));
    let sha: String = Sha256::digest(data.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({
        "kind": kind,
        "mime": mime,
        "bytes": data.trim_end_matches('=').len() * 3 / 4,
        "sha256": sha,
    })
}

#[derive(Debug, Default, PartialEq)]
pub struct Git {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub gitdir: Option<String>,
}

/// Branch, HEAD SHA and the checkout's own git directory, read from files (plan D9): `.git` may
/// be a directory or, in a linked worktree, a `gitdir:` file. Refs live in the common directory.
pub fn git(cwd: &Path) -> Git {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let Some(dot_git) = start
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|g| g.exists())
    else {
        return Git::default();
    };
    let gitdir = if dot_git.is_file() {
        match repo::gitdir(&dot_git) {
            Some(g) => g.canonicalize().unwrap_or(g),
            None => return Git::default(),
        }
    } else {
        dot_git
    };
    let common = repo::common_dir(&gitdir).unwrap_or_else(|| gitdir.clone());
    let head = std::fs::read_to_string(gitdir.join("HEAD")).unwrap_or_default();
    let head = head.trim();
    let (branch, sha) = match head.strip_prefix("ref: ") {
        Some(name) => (
            name.strip_prefix("refs/heads/").map(str::to_owned),
            resolve(&common, name),
        ),
        None => (None, (!head.is_empty()).then(|| head.to_owned())), // detached
    };
    Git {
        branch,
        head: sha,
        gitdir: Some(gitdir.to_string_lossy().into_owned()),
    }
}

/// A ref's SHA from its loose file, else from `packed-refs`; `None` before the first commit.
fn resolve(common: &Path, name: &str) -> Option<String> {
    if let Ok(s) = std::fs::read_to_string(common.join(name)) {
        return Some(s.trim().to_owned());
    }
    std::fs::read_to_string(common.join("packed-refs"))
        .ok()?
        .lines()
        .find_map(|l| {
            let (sha, r) = l.split_once(' ')?;
            (r == name).then(|| sha.to_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(event: &str, payload: Value) -> Event {
        let mut v = events("claude", event, &payload, 7);
        assert_eq!(v.len(), 1, "{event}: {v:?}");
        v.remove(0).event
    }

    fn body(e: &Event) -> Value {
        serde_json::from_str(&e.body).unwrap()
    }

    #[test]
    fn a_long_tool_output_is_kept_whole_and_redacted_past_the_old_window() {
        let key = &format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // split, as in import.rs, so secret scanners pass it
        let out = "word ".repeat(20_000) + " Authorization: Bearer " + key;
        let e = one(
            "PostToolUse",
            json!({"session_id": "s", "cwd": "/", "tool_name": "Bash",
                   "tool_input": {"command": "ls"}, "tool_response": {"stdout": out}}),
        );
        assert_eq!(e.kind, "tool");
        let b = body(&e);
        assert!(
            b["output"]
                .as_str()
                .unwrap()
                .contains(&"word ".repeat(20_000))
        );
        assert!(
            !e.body.contains(key),
            "a secret past today's 12,000-character window"
        );
        assert_eq!(b["input"], r#"{"command":"ls"}"#);
        assert_eq!(b["failed"], false);
        assert_eq!(
            (e.session.as_str(), e.ts, e.source.as_str()),
            ("s", 7, "hook")
        );
    }

    #[test]
    fn prompts_lose_private_blocks_and_envelopes_are_not_typed_prompts() {
        let e = one(
            "UserPromptSubmit",
            json!({"prompt": "keep <private>secret plan</private> this"}),
        );
        assert_eq!(
            (e.kind.as_str(), body(&e)["prompt"].as_str()),
            ("prompt", Some("keep  this"))
        );
        let e = one(
            "UserPromptSubmit",
            json!({"prompt": "shown <private>and the rest is hidden"}),
        );
        assert_eq!(body(&e)["prompt"], "shown");
        assert!(
            events(
                "claude",
                "UserPromptSubmit",
                &json!({"prompt": "<private>x"}),
                0
            )
            .is_empty()
        );
        for envelope in [
            "<task-notification>done</task-notification>",
            "Another Claude session sent a message: <teammate-message from=\"a\">hi</teammate-message>",
        ] {
            let e = one("UserPromptSubmit", json!({"prompt": envelope}));
            assert_eq!(e.kind, "envelope", "{envelope}");
        }
        assert_eq!(
            one("UserPromptSubmit", json!({"prompt": "hi"})).session,
            "unknown"
        );
    }

    #[test]
    fn a_block_never_pairs_across_two_fields() {
        let out = json!({"a": "<private>x", "b": "y</private>", "keep": "z", "c": "<private>gone</private>!"});
        let e = one(
            "PostToolUse",
            json!({"tool_name": "t", "tool_input": {}, "tool_response": out}),
        );
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert_eq!(
            stored,
            json!({"a": "<private>x", "b": "y</private>", "keep": "z", "c": "!"})
        );
    }

    #[test]
    fn the_ledger_names_the_field_and_the_mask_never_the_value() {
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let payload = json!({"session_id": "s", "tool_name": "Bash",
            "tool_input": {"command": format!("curl -H 'Authorization: Bearer {key}'")},
            "tool_response": {"stdout": "ok"}, "reason": {format!("Bearer {key}"): 1}});
        let v = events("claude", "PostToolUse", &payload, 0);
        let c = &v[0];
        assert!(!format!("{c:?}").contains(&key), "{c:?}");
        let (field, f) = &c.ledger[0];
        assert_eq!((field.as_str(), f.length), ("/input", key.len()));
        let input = body(&c.event)["input"].as_str().unwrap().to_owned();
        assert_eq!(
            &input[f.offset..f.offset + "[REDACTED]".len()],
            "[REDACTED]"
        );
        // A key names its object, from the stored key: the pointer never holds the secret.
        let end = events(
            "claude",
            "SessionEnd",
            &json!({"reason": {format!("Bearer {key}"): 1}}),
            0,
        );
        assert!(
            end[0]
                .ledger
                .iter()
                .any(|(field, _)| field == "/reason#key"),
            "{:?}",
            end[0].ledger
        );
        assert!(!format!("{:?}", end[0]).contains(&key));
    }

    #[test]
    fn secrets_behind_json_escapes_in_tool_fields_are_masked() {
        // A tool field is stored as flattened JSON: its quotes and line breaks become `\"`, `\n`.
        let b64 = format!("dXNyOnE5Wng4{}", "bUwydkI0blI3dFl3");
        let weak = format!("MTExMTExMTE6{}", "MTExMTExMTE="); // 11111111:11111111
        let pass = format!("q9Zx8mL2{}", "vB4nR7tY1wK3");
        let api = format!("k7Fq2Lp9{}", "Xw3Rt8Vn5Bz1Mc6D");
        let cases = [
            // Bash stdout holding JSON (an API's answer, a config file)
            (
                json!({"command": "cat cfg.json"}),
                json!({"stdout": format!("{{\"Authorization\":\"Basic {b64}\"}}"), "stderr": ""}),
                &b64,
            ),
            // A weak credential after "Authorization: Basic" is still one
            (
                json!({"command": "cat headers.txt"}),
                json!({"stdout": format!("Authorization: Basic {weak}\n")}),
                &weak,
            ),
            // curl's -u and -H in double quotes
            (
                json!({"command": format!("curl -u \"admin:{pass}\" https://x.invalid/")}),
                json!({"stdout": "ok"}),
                &pass,
            ),
            (
                json!({"command": format!("curl -H \"X-Api-Key: {api}\" https://x.invalid/")}),
                json!({"stdout": "ok"}),
                &api,
            ),
            // A key and its value side by side: why tool fields stay flattened
            (
                json!({"api_key": api.clone(), "q": "x"}),
                json!({"stdout": "ok"}),
                &api,
            ),
        ];
        for (input, output, secret) in cases {
            let payload = json!({"session_id": "s", "tool_name": "Bash",
                "tool_input": input, "tool_response": output});
            let c = &events("claude", "PostToolUse", &payload, 0)[0];
            assert!(
                !format!("{c:?}").contains(secret.as_str()),
                "{}",
                c.event.body
            );
            assert!(!c.ledger.is_empty());
            let b = body(&c.event);
            for (field, f) in &c.ledger {
                let text = b[&field[1..]].as_str().unwrap();
                assert_eq!(&text[f.offset..f.offset + "[REDACTED]".len()], "[REDACTED]");
            }
        }
    }

    #[test]
    fn a_huge_key_is_named_by_its_hash_in_the_ledger() {
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let big = "k".repeat(250_000);
        let inner: Map<String, Value> = (0..50).map(|i| (format!("n{i}"), json!(key))).collect();
        let e = &events(
            "claude",
            "SessionEnd",
            &json!({"reason": {big.clone(): inner}}),
            0,
        )[0];
        assert_eq!(e.ledger.len(), 50);
        for (field, _) in &e.ledger {
            assert!(
                field.starts_with("/reason/~sha:") && field.len() < 64,
                "{}",
                &field[..60.min(field.len())]
            );
        }
        assert_eq!(segment("a/b~c"), "a~1b~0c");
    }

    #[test]
    fn an_oversized_field_keeps_head_and_tail_and_its_full_size() {
        let out = "a".repeat(MAX_FIELD_BYTES) + &"b".repeat(MAX_FIELD_BYTES);
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_response": out}),
        );
        let stored = body(&e)["output"].as_str().unwrap().to_owned();
        assert!(stored.len() < MAX_FIELD_BYTES + 100, "{}", stored.len());
        assert!(stored.starts_with('a') && stored.ends_with('b'));
        assert!(stored.contains(&format!("[cut: {} bytes in full]", out.len())));
        assert_eq!(e.original_bytes, Some(out.len() as i64));
        let small = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_response": "x"}),
        );
        assert_eq!(small.original_bytes, None);
    }

    #[test]
    fn a_tool_call_keeps_its_subagent_and_interruption() {
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_response": null,
                   "agent_id": "a1", "interrupted": true}),
        );
        assert_eq!(
            (&body(&e)["agent_id"], &body(&e)["interrupted"]),
            (&json!("a1"), &json!(true))
        );
        let plain = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}}),
        );
        assert!(
            body(&plain).get("agent_id").is_none() && body(&plain).get("interrupted").is_none()
        );
    }

    #[test]
    fn stored_text_keeps_its_whitespace() {
        let out = "\n    indented line\n\t";
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Read", "tool_input": " a ", "tool_response": out}),
        );
        assert_eq!(
            (body(&e)["output"].as_str(), body(&e)["input"].as_str()),
            (Some(out), Some(" a "))
        );
        let e = one(
            "Stop",
            json!({"last_assistant_message": "  code:\n    x = 1\n"}),
        );
        assert_eq!(body(&e)["assistant"], "  code:\n    x = 1\n");
    }

    #[test]
    fn private_blocks_leave_every_text_field() {
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {"command": "echo <private>zqx-in</private>"},
                   "tool_response": {"stdout": "a <private>zqx-out</private> b"}}),
        );
        assert!(
            !e.body.contains("zqx") && !e.body.contains("private"),
            "{}",
            e.body
        );
        let e = one(
            "Stop",
            json!({"last_assistant_message": "ok <private>reply</private>"}),
        );
        assert_eq!(body(&e)["assistant"], "ok "); // whitespace is kept, the block is not
        let e = one(
            "PostCompact",
            json!({"compact_summary": "sum <private>mary</private>"}),
        );
        assert_eq!(body(&e)["summary"], "sum ");
        let only_private = json!({"last_assistant_message": "<private>all</private>"});
        assert!(events("claude", "Stop", &only_private, 0).is_empty());
    }

    #[test]
    fn every_string_in_the_body_passes_the_gate() {
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let bearer = format!("Authorization: Bearer {token} <private>zqx-opt-out</private>");
        for (event, payload) in [
            (
                "PostToolUse",
                json!({"tool_name": bearer, "tool_input": {}, "tool_response": ""}),
            ),
            ("SessionStart", json!({"source": bearer})),
            ("PreCompact", json!({"trigger": {"nested": [bearer]}})),
            ("SessionEnd", json!({"reason": bearer})),
            ("UserPromptSubmit", json!({"prompt": bearer})),
            ("Stop", json!({"last_assistant_message": bearer})),
            ("PostCompact", json!({"compact_summary": bearer})),
        ] {
            let e = one(event, payload);
            assert!(
                !e.body.contains(&token)
                    && e.body.contains("[REDACTED]")
                    && !e.body.contains("zqx"),
                "{event}: {}",
                e.body
            );
        }
    }

    #[test]
    fn labels_are_redacted_too() {
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join(format!("Bearer {token}"));
        std::fs::create_dir(&cwd).unwrap();
        let e = one(
            "SessionStart",
            json!({"session_id": format!("Authorization: Bearer {token}"), "cwd": cwd}),
        );
        for field in [Some(&e.session), e.cwd.as_ref(), e.repo.as_ref()] {
            let field = field.unwrap();
            assert!(
                !field.contains(&token) && field.contains("[REDACTED]"),
                "{field}"
            );
        }
    }

    #[test]
    fn images_become_markers_in_each_shape_seen() {
        let data = "iVBORw0KGgo=";
        let out = json!([
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}},
            {"type": "image/png", "base64": data, "dimensions": {}},
            {"type": "image", "mimeType": "image/jpeg", "data": data},
            {"type": "text", "text": "caption"},
        ]);
        let e = one(
            "PostToolUse",
            json!({"tool_name": "Read", "tool_input": {}, "tool_response": out}),
        );
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert!(!e.body.contains(data));
        for (i, mime) in ["image/png", "image/png", "image/jpeg"].iter().enumerate() {
            assert_eq!(stored[i]["kind"], "image");
            assert_eq!(stored[i]["mime"], *mime);
            assert_eq!(stored[i]["bytes"], 8);
            assert_eq!(stored[i]["sha256"].as_str().unwrap().len(), 64);
        }
        assert_eq!(stored[3]["text"], "caption");
        let tagged =
            json!({"type": "image", "mimeType": "image/<private>zqx</private>png", "data": data});
        let e = one(
            "PostToolUse",
            json!({"tool_name": "t", "tool_input": {}, "tool_response": tagged}),
        );
        assert!(!e.body.contains("zqx"), "{}", e.body);
    }

    #[test]
    fn long_base64_in_any_shape_becomes_a_marker() {
        let blob = "iVBORw0KGgoAAAANSUhEUg".repeat(50); // 1,100 characters
        let e = one(
            "PostToolUse",
            json!({"tool_name": "mcp__x", "tool_input": {},
                   "tool_response": {"encoding": "base64", "content": blob, "note": format!("saved {blob}.")}}),
        );
        assert!(!e.body.contains("SUhEUgiVBOR"), "{}", e.body);
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert_eq!(stored["content"]["kind"], "binary");
        assert_eq!(stored["content"]["bytes"], 825);
        assert!(
            stored["note"]
                .as_str()
                .unwrap()
                .starts_with("saved {\"kind\":\"binary\"")
        );
        // Text that only looks like it stays: short runs (a hash, a key id), runs missing a character
        // class (a long lowercase or hex run), and a length base64 cannot have (1,025).
        for kept in [
            "a1B2".repeat(64),
            "x".repeat(2_000),
            "0f".repeat(600),
            "a1B".repeat(341) + "cd",
        ] {
            let e = one(
                "PostToolUse",
                json!({"tool_name": "x", "tool_input": {}, "tool_response": kept}),
            );
            assert!(e.body.contains(&kept), "{}", kept.len());
        }
        // Prompts, replies and summaries get the same pass as tool fields.
        let uri = "data:image/png;base64,iVBORw0KGgo=";
        for (event, payload) in [
            ("UserPromptSubmit", json!({"prompt": format!("see {uri}")})),
            ("Stop", json!({"last_assistant_message": blob})),
            (
                "PostCompact",
                json!({"compact_summary": format!("{uri} and {blob}")}),
            ),
        ] {
            let e = one(event, payload);
            assert!(!e.body.contains("iVBORw0KGgo"), "{event}: {}", e.body);
        }
    }

    #[test]
    fn data_uris_become_markers_whole_or_inside_text() {
        let uri = "data:image/png;base64,iVBORw0KGgo=";
        let out = json!([
            {"type": "input_image", "image_url": uri},
            {"type": "image_url", "image_url": {"url": uri}},
            format!("<img src=\"{uri}\"> after"),
        ]);
        let e = one(
            "PostToolUse",
            json!({"tool_name": "view_image", "tool_input": {}, "tool_response": out}),
        );
        assert!(!e.body.contains("iVBORw0KGgo"), "{}", e.body);
        let stored: Value = serde_json::from_str(body(&e)["output"].as_str().unwrap()).unwrap();
        assert_eq!(stored[0]["image_url"]["mime"], "image/png");
        assert_eq!(stored[1]["image_url"]["url"]["bytes"], 8);
        let inline = stored[2].as_str().unwrap();
        assert!(
            inline.starts_with("<img src=\"{\"kind\":\"image\"") && inline.ends_with("\"> after"),
            "{inline}"
        );
    }

    #[test]
    fn replies_come_from_the_payload_or_the_codex_transcript() {
        let e = one("Stop", json!({"last_assistant_message": "done"}));
        assert_eq!(
            (e.kind.as_str(), body(&e)["assistant"].as_str()),
            ("reply", Some("done"))
        );
        let dir = tempfile::tempdir().unwrap();
        let rollout = dir.path().join("rollout.jsonl");
        std::fs::write(
            &rollout,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"from codex"}]}}"#,
        )
        .unwrap();
        let v = events("codex", "Stop", &json!({"transcript_path": rollout}), 0);
        assert_eq!(body(&v[0].event)["assistant"], "from codex");
        assert!(events("claude", "Stop", &json!({"last_assistant_message": " "}), 0).is_empty());
        assert!(events("claude", "PreToolUse", &json!({"tool_name": "Bash"}), 0).is_empty());
    }

    #[test]
    fn git_fields_come_from_files_in_a_worktree_too() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let git = |args: &[&str], cwd: &Path| {
            let ok = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=main",
                ])
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(ok.status.success(), "{args:?}: {ok:?}");
            String::from_utf8(ok.stdout).unwrap().trim().to_owned()
        };
        std::fs::create_dir(&main).unwrap();
        assert_eq!(super::git(&main), Git::default());
        git(&["init", "-q"], &main);
        let unborn = super::git(&main);
        assert_eq!(
            (unborn.branch.as_deref(), unborn.head),
            (Some("main"), None)
        );
        git(&["commit", "-q", "--allow-empty", "-m", "one"], &main);
        let sha = git(&["rev-parse", "HEAD"], &main);
        let wt = dir.path().join("wt");
        git(
            &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()],
            &main,
        );
        git(&["pack-refs", "--all"], &main); // refs now only in packed-refs
        let m = super::git(&main.join("sub-dir-that-does-not-exist"));
        assert_eq!(
            (m.branch.as_deref(), m.head.as_deref()),
            (Some("main"), Some(sha.as_str()))
        );
        let w = super::git(&wt);
        assert_eq!(
            (w.branch.as_deref(), w.head.as_deref()),
            (Some("side"), Some(sha.as_str()))
        );
        assert_ne!(w.gitdir, m.gitdir);
        assert!(w.gitdir.unwrap().contains("worktrees"));
        git(&["checkout", "-q", "--detach"], &wt);
        let d = super::git(&wt);
        assert_eq!((d.branch, d.head.as_deref()), (None, Some(sha.as_str())));
    }
}
