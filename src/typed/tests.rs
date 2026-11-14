use super::*;
use serde_json::json;

const DAY: i64 = 24 * 60 * 60 * 1000;

fn long(head: &str) -> String {
    format!(
        "{head} {}",
        "and the parser keeps its tables between runs ".repeat(6)
    )
}

fn prompt(extra: Value) -> Value {
    let mut body = json!({"prompt": "Fix the parser."});
    (body.as_object_mut().unwrap()).extend(extra.as_object().unwrap().clone());
    body
}

/// Codex's five review templates (0.166.0) are a program's; the owner's own review request,
/// and text that only opens or only closes as a template does, are not.
#[test]
fn codex_review_templates_are_a_programs() {
    let findings = "Provide prioritized, actionable findings.";
    for p in [
        "Review the current code changes (staged, unstaged, and untracked files) and provide \
             prioritized findings."
            .to_owned(),
        format!("Review the code changes introduced by commit 1a2b3c4d. {findings}"),
        format!(
            "Review the code changes introduced by commit 1a2b3c4d (\"Fix the parser\"). \
                 {findings}"
        ),
        format!(
            "Review the code changes introduced by commit 1a2b3c4d (\"Fix the parser\nAlways \
                 keep its tables\"). {findings}"
        ),
        format!(
            "Review the code changes against the base branch 'main'. The merge base commit \
                 for this comparison is 1a2b3c4d. Run `git diff 1a2b3c4d` to inspect the changes \
                 relative to main. {findings}"
        ),
        format!(
            "Review the code changes against the base branch 'main'. Start by finding the \
                 merge diff between the current branch and main's upstream e.g. (`git merge-base \
                 HEAD \"$(git rev-parse --abbrev-ref \"main@{{upstream}}\")\"`), then run `git \
                 diff` against that SHA to see what changes we would merge into the main branch. \
                 {findings}\n"
        ),
    ] {
        assert!(codex_review_preset(&p), "{p}");
        assert!(AgentText::new([]).agent_written(0, &p), "{p}");
    }
    for p in [
        "Review the code changes against the base branch 'main' and say whether the parser \
             keeps its tables."
            .to_owned(),
        format!("Look at the parser first. {findings}"),
        "Review the current code changes and provide prioritized findings.".to_owned(),
        "レビューして: Review the code changes introduced by commit 1a2b3c4d.".to_owned(),
        format!(
            "Review the code changes against the base branch 'main'. Focus only on parser \
                 compatibility. {findings}"
        ),
        format!(
            "Review the code changes against the base branch 'main'. The merge base commit \
                 for this comparison is 1a2b3c4d. Run `git diff 1a2b3c4d` to inspect the changes \
                 relative to main. {findings} Then check the tests."
        ),
        format!(
            "Review the code changes against the base branch 'main'. The merge base commit \
                 for this comparison is 1a2b3c4d. Run `git diff 9f8e7d6c` to inspect the changes \
                 relative to main. {findings}"
        ),
        format!("Review the code changes introduced by commit 1a2b3c4d and the tests. {findings}"),
    ] {
        assert!(!codex_review_preset(&p), "{p}");
    }
}

/// C3: a prompt's sender is the conversation, other than its own, whose tool input held its
/// opening last before it in the lookback; a reply names none.
#[test]
fn a_prompts_sender_is_the_last_other_tool_input_that_held_its_opening() {
    let text = long("Port the lexer to the new parser");
    let key = |s: &str| ("claude".to_owned(), s.to_owned());
    let mut index = AgentText::new([(10 * DAY, text.as_str())]);
    index.add(10 * DAY - 4_000, &format!("I will send it: {text}"));
    assert_eq!(index.sender(10 * DAY, &text, &key("own")), None);
    assert!(index.agent_written(10 * DAY, &text));
    index.add_from(10 * DAY - 5_000, &format!("echo '{text}'"), &key("a"), 5);
    index.add_from(
        10 * DAY - 2_000,
        &format!("codex exec '{text}'"),
        &key("b"),
        7,
    );
    index.add_from(
        10 * DAY - 1_000,
        &format!("cat <<EOF\n{text}\nEOF"),
        &key("own"),
        9,
    );
    assert_eq!(
        index.sender(10 * DAY, &text, &key("own")),
        Some((&key("b"), 7))
    );
    // Past the lookback, or after the prompt, names none.
    let mut late = AgentText::new([(10 * DAY, text.as_str())]);
    late.add_from(10 * DAY + 1, &text, &key("a"), 1);
    late.add_from(10 * DAY - LOOKBACK_MS - 1, &text, &key("b"), 2);
    assert_eq!(late.sender(10 * DAY, &text, &key("own")), None);
}

/// O2: a Claude Code prompt counts by its entry point: the terminal, the desktop app and the
/// VS Code extension; `claude -p`, the Agent SDK and Claude Code's other programs never (O5:
/// one recorded before entry points were kept counts when a hook wrote it).
#[test]
fn a_headless_claude_prompt_is_not_typed() {
    for source in ["hook", "transcript"] {
        for e in ["cli", "claude-desktop", "claude-vscode"] {
            let body = prompt(json!({"entrypoint": e}));
            assert!(typed("prompt", "claude", source, &body), "{e}");
        }
        for e in [
            "sdk-cli",
            "sdk-ts",
            "sdk-py",
            "mcp",
            "remote_trigger",
            "claude-code-github-action",
            "CLI",
            "",
        ] {
            let body = prompt(json!({"entrypoint": e}));
            assert!(!typed("prompt", "claude", source, &body), "{e}");
        }
    }
    assert_eq!(entrypoint(&json!("remote_trigger")), Some("remote_trigger"));
    for source in ["hook", "oboete-v1"] {
        assert!(
            typed("prompt", "claude", source, &prompt(json!({}))),
            "{source}"
        );
    }
    assert!(
        !typed("prompt", "claude", "transcript", &prompt(json!({}))),
        "a session only the transcript import found ran no hooks"
    );
}

/// O1: an envelope, a prompt kept private whole and an omitted one are no typed prompt.
#[test]
fn an_envelope_a_private_prompt_and_an_omitted_one_are_not_typed() {
    assert!(!typed(
        "envelope",
        "claude",
        "hook",
        &prompt(json!({"entrypoint": "cli"}))
    ));
    assert!(!typed(
        "prompt",
        "claude",
        "hook",
        &json!({"omitted": true, "entrypoint": "cli"})
    ));
    let private = json!({"session_id": "s", "cwd": "/w", "prompt": "<private>a secret</private>"});
    let captured = crate::capture::events(
        "claude",
        "UserPromptSubmit",
        &private,
        0,
        &Default::default(),
    );
    assert!(captured.is_empty(), "nothing is recorded of it");
}

/// O2: a Codex prompt Claude Code's Codex plugin sent is marked another program's, as an exec
/// run's is.
#[test]
fn a_codex_prompt_the_codex_plugin_sent_is_not_typed() {
    let plugin = json!({"source": "vscode", "originator": "Claude Code"});
    assert!(crate::transcript::codex_agent_sent(&plugin));
    assert!(!typed(
        "prompt",
        "codex",
        "hook",
        &prompt(json!({"agent_sent": true}))
    ));
    assert!(typed("prompt", "codex", "hook", &prompt(json!({}))));
}

/// O5: a Grok prompt carries no mark, so it counts unless a text test flags it.
#[test]
fn a_grok_prompt_counts_unless_a_text_test_flags_it() {
    let text = long("Review the parser change");
    assert!(typed("prompt", "grok", "hook", &json!({"prompt": text})));
    let mut index = AgentText::new([(10 * DAY, text.as_str())]);
    assert!(!index.agent_written(10 * DAY, &text));
    index.add(
        9 * DAY,
        &format!("grok --prompt-file p.txt <<EOF\n{text}\nEOF"),
    );
    assert!(index.agent_written(10 * DAY, &text));
}

/// O3: an opening an agent wrote in the nine days before the prompt flags it, in English and
/// in Japanese.
#[test]
fn a_prompt_whose_opening_an_agent_wrote_earlier_is_not_typed() {
    let english = long("Review the parser change");
    let japanese = format!(
        "{}{}",
        "パーサーの変更を確認して、テーブルの保存と読み込みが正しく動くかを調べてください。",
        "読み込みの時間も測って結果を表にまとめてください。".repeat(5)
    );
    assert!(fingerprint(&japanese).is_some(), "Japanese is tested too");
    let t = 100 * DAY;
    let mut index = AgentText::new([(t, english.as_str()), (t, japanese.as_str())]);
    index.add(t - DAY, &format!("prompt.txt: {english}"));
    index.add(t - 2 * DAY, &format!("「{japanese}」"));
    assert!(index.agent_written(t, &english));
    assert!(index.agent_written(t, &japanese));
}

/// O3: agent text written after the prompt, or more than nine days before it, does not flag it.
#[test]
fn agent_text_after_the_prompt_or_from_10_days_before_does_not_flag_it() {
    let text = long("Review the parser change");
    let t = 100 * DAY;
    let mut index = AgentText::new([(t, text.as_str())]);
    index.add(t + 1, &text);
    index.add(t - 10 * DAY, &text);
    assert!(!index.agent_written(t, &text));
    index.add(t - 9 * DAY, &text);
    assert!(index.agent_written(t, &text));
}

/// O3: a prompt that opens in lower case or is short is never looked for.
#[test]
fn a_prompt_that_opens_in_lowercase_or_is_short_is_not_fingerprinted() {
    assert_eq!(fingerprint(&long("review the parser change")), None);
    assert_eq!(fingerprint("Review the parser change."), None);
    let f = fingerprint(&long("Review the parser change")).unwrap();
    assert_eq!(f.chars().count(), FINGERPRINT);
    assert!(f.starts_with("review the parser change and"));
}

/// O3: a role-assignment opening over 400 characters is an agent's, with no agent text.
#[test]
fn a_role_assignment_opening_over_400_characters_is_an_agents() {
    let index = AgentText::new([]);
    let role = format!(
        "You are the reviewer. {}",
        "Check every change. ".repeat(25)
    );
    assert!(index.agent_written(0, &role));
    assert!(index.agent_written(0, &format!("Task: {}", "x".repeat(400))));
    assert!(!index.agent_written(0, "You are the reviewer. Check it."));
    assert!(!index.agent_written(0, &format!("You were {}", "x".repeat(400))));
}

/// O3: a prompt an agent wrote into a tool call, a command or a file, is found across its
/// lines; a reply is read as written; other records hold no agent text.
#[test]
fn a_prompt_written_into_a_tool_call_is_found_across_its_lines() {
    let text = format!("Review the parser change.\nKeep {}", long("its tables"));
    let input = json!({"command": format!("cat > p.txt <<'EOF'\n{text}\nEOF")}).to_string();
    let t = 100 * DAY;
    let mut index = AgentText::new([(t, text.as_str())]);
    index.add(t - DAY, &input);
    assert!(
        !index.agent_written(t, &text),
        "escaped, the line break reads as an n"
    );
    index.add(
        t - DAY,
        &agent_text("tool", &json!({"input": input})).unwrap(),
    );
    assert!(index.agent_written(t, &text));
    let reply = agent_text("reply", &json!({"assistant": "Done."}));
    assert_eq!(reply.as_deref(), Some("Done."));
    assert_eq!(agent_text("prompt", &json!({"prompt": text})), None);
}

/// Every event of `raw` from `from` on, device by device, in pages.
fn each_event(raw: &crate::raw::Raw, from: i64, mut each: impl FnMut(&crate::raw::Event)) {
    for device in raw.devices().unwrap() {
        let mut seq = 0;
        loop {
            let page = raw.after(&device, seq, 2_000).unwrap();
            let Some(last) = page.last() else {
                break;
            };
            seq = last.seq;
            for r in &page {
                if let crate::raw::Item::Event(e) = &r.item
                    && e.ts >= from
                {
                    each(e);
                }
            }
        }
    }
}

/// Slice 1's measurement (docs/my-work-model.md): on a copy of a store, never the live one,
/// how many of the last 31 days' prompts each test flags, by agent, and how long the 40 days
/// of agent text before them take to stream. By hand: `OBOETE_MEASURE_HOME=<copy> cargo test
/// --release --bin oboete -- --ignored --nocapture measure_the_marks`.
#[test]
#[ignore]
fn measure_the_marks_on_a_copy_of_a_store() {
    let home = std::env::var("OBOETE_MEASURE_HOME").expect("a copy of a store");
    let raw = crate::raw::open(std::path::Path::new(&home)).unwrap();
    let now = crate::db::now_ms();
    let started = std::time::Instant::now();
    let mut prompts = Vec::new();
    each_event(&raw, now - 31 * DAY, |e| {
        if e.kind == "prompt" {
            let body: Value = serde_json::from_str(&e.body).unwrap_or_default();
            let text = crate::hook::compact(&body["prompt"]);
            prompts.push((
                e.agent.clone(),
                e.ts,
                typed(&e.kind, &e.agent, &e.source, &body),
                text,
            ));
        }
    });
    let read = started.elapsed();
    let marked = prompts.iter().filter(|p| p.2).map(|p| (p.1, p.3.as_str()));
    let mut index = AgentText::new(marked);
    let started = std::time::Instant::now();
    let (mut testing, mut pieces, mut chars) = (std::time::Duration::ZERO, 0usize, 0usize);
    each_event(&raw, now - 40 * DAY, |e| {
        let body: Value = serde_json::from_str(&e.body).unwrap_or_default();
        if let Some(text) = agent_text(&e.kind, &body) {
            let at = std::time::Instant::now();
            index.add(e.ts, &text);
            testing += at.elapsed();
            pieces += 1;
            chars += text.chars().count();
        }
    });
    let streamed = started.elapsed();
    let mut by_agent = std::collections::BTreeMap::<&str, [usize; 7]>::new();
    for (agent, ts, typed, text) in &prompts {
        let row = by_agent.entry(agent).or_default();
        row[0] += 1;
        if !typed {
            row[1] += 1;
            continue;
        }
        row[2] += 1;
        let opening = fingerprint(text);
        row[3] += usize::from(opening.is_some());
        let found = opening.is_some_and(|f| index.found.contains(&(f.chars().collect(), *ts)));
        row[4] += usize::from(found);
        row[5] += usize::from(assigns_a_role(text));
        row[6] += usize::from(!index.agent_written(*ts, text));
    }
    println!(
        "31 days of prompts by agent: all, counted out by the marks, typed by the marks, \
             checkable, opening in agent text, role assignment, typed after the tests"
    );
    for (agent, row) in &by_agent {
        println!("{agent}: {row:?}");
    }
    let peak = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let peak = peak.lines().find(|l| l.starts_with("VmHWM:")).unwrap_or("");
    println!(
        "prompts read in {read:?}; {pieces} pieces of agent text ({chars} characters) of 40 \
             days streamed in {streamed:?}, {testing:?} of it in the tests; {peak}"
    );
}
