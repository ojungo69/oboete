use super::*;
use serde_json::json;

fn ev(session: &str, kind: &str, body: Value, cwd: &Path) -> Event {
    Event {
        agent: "claude".into(),
        session: session.into(),
        kind: kind.into(),
        ts: crate::db::now_ms(),
        repo: Some("github.com/example/shop".into()),
        branch: None,
        head: None,
        gitdir: None,
        cwd: Some(cwd.to_string_lossy().into_owned()),
        source: "hook".into(),
        body: body.to_string(),
        original_bytes: None,
    }
}

/// A call as the hooks record one in a public prompt's turn (`mark_public`).
fn tool(session: &str, name: &str, input: Value, cwd: &Path) -> Event {
    let body = json!({"tool": name, "input": input.to_string(), "output": "", "failed": false});
    let mut e = ev(session, "tool", body, cwd);
    mark_public(&mut e);
    e
}

/// A turn of `session`: its prompt, its calls and its reply.
fn turn(raw: &mut Raw, session: &str, cwd: &Path, calls: &[(&str, Value)]) {
    raw.append(&ev(session, "prompt", json!({"prompt": "Make it."}), cwd))
        .unwrap();
    for (name, input) in calls {
        raw.append(&tool(session, name, input.clone(), cwd))
            .unwrap();
    }
    raw.append(&ev(session, "reply", json!({"assistant": "Done."}), cwd))
        .unwrap();
}

/// A store whose phase has had its first run: its checkpoint at the newest record.
fn ready() -> (tempfile::TempDir, Raw) {
    let home = tempfile::tempdir().unwrap();
    let raw = crate::raw::open(home.path()).unwrap();
    assert!(!step(home.path(), &raw));
    (home, raw)
}

/// knowledge.db with the `subagents` consumer drained, as the worker's round has it.
fn drained(home: &Path, raw: &Raw) -> Connection {
    let mut consumers: Vec<Box<dyn crate::worker::Consumer>> =
        vec![Box::new(crate::consumer::subagents::Subagents)];
    let mut k = crate::knowledge::open(home).unwrap();
    crate::worker::drain(raw, &mut k, &mut consumers).unwrap();
    k
}

fn step(home: &Path, raw: &Raw) -> bool {
    run(home, raw, &drained(home, raw), &mut None, &|| false).unwrap()
}

/// Each row's title, its copy's path and whether it was declared.
fn copies(home: &Path) -> Vec<(String, String, bool)> {
    let db = open(home).unwrap();
    db.prepare("SELECT title, copy, declared FROM deliverables ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// claude-mem's names (tests/shared/deliverables.test.ts): unsafe characters made dashes, a
/// long key cut with a hash of the whole, a stem giving way before the hash and extension.
#[test]
fn names_are_claude_mems() {
    assert_eq!(
        folder_name("thedotmack/claude-mem"),
        "thedotmack-claude-mem"
    );
    assert_eq!(folder_name(r"C:\work\repo"), "C-work-repo");
    assert_eq!(folder_name(".."), "unknown-project");
    let long = "k".repeat(300);
    let folder = folder_name(&long);
    let hash = &crate::curate::sha256_hex(&long)[..8];
    assert!(
        folder.len() <= NAME_MAX && folder.ends_with(&format!("-{hash}")),
        "{folder}"
    );
    let sha = format!("3f9a1c2b{}", "0".repeat(56));
    let name = |path: &str, hash: &str| copy_name(Path::new(path), hash);
    assert_eq!(
        name("/w/report.md", &sha[..8]).unwrap(),
        "report-3f9a1c2b.md"
    );
    assert_eq!(name("/w/.hidden", &sha[..8]).unwrap(), "hidden-3f9a1c2b");
    assert_eq!(name("/w/a:b?.pdf", &sha[..8]).unwrap(), "a-b-3f9a1c2b.pdf");
    let whole = name(&format!("/w/{}.pdf", "s".repeat(300)), &sha).unwrap();
    assert!(
        whole.len() <= NAME_MAX && whole.ends_with(&format!("-{sha}.pdf")),
        "{whole}"
    );
    assert!(name(&format!("/w/a.{}", "x".repeat(260)), &sha[..8]).is_err());
    // An extension is made safe too, so every copy's name is one the sweep removes.
    assert_eq!(
        name("/w/report.p*df", &sha[..8]).unwrap(),
        "report-3f9a1c2b.p-df"
    );
    // A backslash is in a name only where it separates no folders.
    #[cfg(unix)]
    {
        assert_eq!(
            name(r"/w/report.p\df", &sha[..8]).unwrap(),
            "report-3f9a1c2b.p-df"
        );
    }
}

/// X23: a turn's declared file (relative to its folder) and the files it wrote outside the
/// source tree (in no repository, or one git ignores) are copied, declared first; a source
/// file it edited is not; the same bytes another turn makes are kept once.
#[test]
fn a_turn_copies_what_it_declared_and_wrote_outside_the_source_tree() {
    let (home, mut raw) = ready();
    let (work, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (w, r) = (work.path(), repo.path());
    git(r, &["init", "-q"]);
    std::fs::write(r.join(".gitignore"), "out/\n").unwrap();
    std::fs::create_dir(r.join("out")).unwrap();
    std::fs::write(w.join("report.pdf"), b"%PDF made").unwrap();
    std::fs::write(w.join("notes.md"), b"# Notes").unwrap();
    std::fs::write(r.join("out/chart.png"), b"chart").unwrap();
    std::fs::write(r.join("logo.png"), b"logo").unwrap();
    let fields =
        json!({"list": "report", "fields": {"task": "report", "work_product": "notes.md"}});
    turn(
        &mut raw,
        "s1",
        w,
        &[
            (
                "Write",
                json!({"file_path": w.join("report.pdf"), "content": "%PDF made"}),
            ),
            ("Write", json!({"file_path": r.join("out/chart.png")})),
            ("Edit", json!({"file_path": r.join("logo.png")})),
            ("mcp__oboete__work_state_write", fields),
        ],
    );
    assert!(step(home.path(), &raw));
    let got = copies(home.path());
    let titles: Vec<&str> = got.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(titles, ["notes.md", "report.pdf", "chart.png"]);
    assert!(got[0].1.starts_with("shop/notes-") && got[0].2 && !got[1].2);
    let read = |copy: &str| std::fs::read(home.path().join(FOLDER).join(copy)).unwrap();
    assert_eq!(read(&got[1].1), b"%PDF made");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let copy = home.path().join(FOLDER).join(&got[1].1);
        let mode = std::fs::metadata(copy).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "a copy is the owner's alone");
    }
    turn(
        &mut raw,
        "s2",
        w,
        &[("Write", json!({"file_path": w.join("report.pdf")}))],
    );
    assert!(step(home.path(), &raw));
    let again = copies(home.path());
    assert_eq!(
        (again.len(), &again[3].1),
        (4, &got[1].1),
        "the same bytes, one file"
    );
    assert!(!step(home.path(), &raw), "nothing new");
}

/// X23: a failed call names nothing, and nor does a call of a replayed session or an agent
/// whose prompts the hooks do not all see.
#[test]
fn a_failed_call_a_replay_and_another_agent_copy_nothing() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
    let input = json!({"file_path": w.join("report.pdf")}).to_string();
    let failed = json!({"tool": "Write", "input": input, "output": "", "failed": true,
            "public_turn": true});
    raw.append(&ev("s1", "prompt", json!({"prompt": "Go."}), w))
        .unwrap();
    raw.append(&ev("s1", "tool", failed, w)).unwrap();
    raw.append(&ev("s1", "reply", json!({"assistant": "No."}), w))
        .unwrap();
    for (agent, source) in [("claude", "replay"), ("cursor", "hook"), ("agy", "hook")] {
        let at = |e: Event| Event {
            agent: agent.into(),
            source: source.into(),
            ..e
        };
        raw.append(&at(ev("s2", "prompt", json!({"prompt": "Go."}), w)))
            .unwrap();
        let call = json!({"file_path": w.join("report.pdf")});
        raw.append(&at(tool("s2", "Write", call, w))).unwrap();
        raw.append(&at(ev("s2", "reply", json!({"assistant": "Done."}), w)))
            .unwrap();
    }
    step(home.path(), &raw);
    assert!(copies(home.path()).is_empty());
}

/// Grok writes with `write` and `search_replace`, and its hooks give an MCP call's input as
/// `{"tool_name", "tool_input"}`: its turn's files are copied as Claude Code's are.
#[test]
fn a_grok_turns_writes_and_wrapped_declaration_are_read() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    for (name, bytes) in [
        ("notes.md", "# Notes"),
        ("deck.pdf", "deck"),
        ("chart.png", "c"),
    ] {
        std::fs::write(w.join(name), bytes).unwrap();
    }
    let grok = |e: Event| Event {
        agent: "grok".into(),
        ..e
    };
    raw.append(&grok(ev("g1", "prompt", json!({"prompt": "Go."}), w)))
        .unwrap();
    let declare = json!({"tool_name": "oboete__work_state_write", "tool_input":
            {"list": "deck", "fields": {"work_product": w.join("notes.md")}}});
    let calls = [
        (
            "write",
            json!({"file_path": w.join("deck.pdf"), "content": "deck"}),
        ),
        ("search_replace", json!({"file_path": w.join("chart.png")})),
        ("oboete__work_state_write", declare),
    ];
    for (name, input) in calls {
        raw.append(&grok(tool("g1", name, input, w))).unwrap();
    }
    raw.append(&grok(ev("g1", "reply", json!({"assistant": "Done."}), w)))
        .unwrap();
    assert!(step(home.path(), &raw));
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["notes.md", "deck.pdf", "chart.png"]);
}

/// A call recorded with no folder, an empty one or one not absolute has no root to hold its
/// paths to, so it names nothing: claude-mem's roots start at the call's folder, which keeps
/// out `work_product: ~/.ssh/id_rsa`.
#[test]
fn a_call_with_no_folder_names_nothing() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("notes.md"), b"# Notes").unwrap();
    std::fs::write(w.join("deck.pdf"), b"deck").unwrap();
    for (i, cwd) in [None, Some(""), Some("work")].into_iter().enumerate() {
        let session = format!("s{i}");
        let at = |e: Event| Event {
            cwd: cwd.map(str::to_owned),
            ..e
        };
        raw.append(&at(ev(&session, "prompt", json!({"prompt": "Go."}), w)))
            .unwrap();
        let declare = json!({"list": "l", "fields": {"work_product": w.join("notes.md")}});
        let write = json!({"file_path": w.join("deck.pdf")});
        for (name, input) in [("mcp__oboete__work_state_write", declare), ("Write", write)] {
            raw.append(&at(tool(&session, name, input, w))).unwrap();
        }
        raw.append(&at(ev(&session, "reply", json!({"assistant": "Done."}), w)))
            .unwrap();
    }
    step(home.path(), &raw);
    assert!(copies(home.path()).is_empty());
}

/// A turn the harness's message starts (a task's notification, recorded as an `envelope`) is a
/// turn as claude-mem reads one, whose transcript line starts a turn: its files are copied as a
/// prompt's turn's are.
#[test]
fn a_turn_a_notification_starts_copies_its_files() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("deck.pdf"), b"deck").unwrap();
    let note = json!({"prompt": "<task-notification>\n<status>completed</status>"});
    raw.append(&ev("s1", "envelope", note, w)).unwrap();
    let write = json!({"file_path": w.join("deck.pdf")});
    raw.append(&tool("s1", "Write", write, w)).unwrap();
    raw.append(&ev("s1", "reply", json!({"assistant": "Done."}), w))
        .unwrap();
    assert!(step(home.path(), &raw));
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["deck.pdf"]);
}

/// Decision 73: a public prompt the agent never answered, then a wholly private prompt whose
/// turn declares a file: the private turn's records are marked by the hooks, and the file is
/// not copied, nor is one a turn the harness starts after it declares (its reply leaves the
/// flag); the next public prompt's is.
#[test]
fn a_private_turns_files_are_not_copied() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    for name in ["plan.pdf", "note.pdf", "public.pdf"] {
        std::fs::write(w.join(name), name).unwrap();
    }
    let settings = crate::capture::Settings::default();
    let hook = |raw: &mut Raw, event: &str, mut payload: Value| {
        payload["session_id"] = json!("s1");
        payload["cwd"] = json!(w);
        let (mut failed, mut back) = (0, 0);
        let ts = crate::db::now_ms();
        crate::hook::record(
            home.path(),
            raw,
            "claude",
            event,
            &payload,
            ts,
            &settings,
            &mut failed,
            &mut back,
        )
        .unwrap();
    };
    let declare = |file: &str| {
        json!({"tool_name": "mcp__oboete__work_state_write", "tool_response": "ok",
                "tool_input": {"list": "plan", "fields": {"work_product": w.join(file)}}})
    };
    hook(
        &mut raw,
        "UserPromptSubmit",
        json!({"prompt": "Start the plan."}),
    );
    hook(
        &mut raw,
        "UserPromptSubmit",
        json!({"prompt": "<private>the real plan</private>"}),
    );
    hook(&mut raw, "PostToolUse", declare("plan.pdf"));
    hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
    // A turn a task's notification starts after it goes on with the private work.
    let note = "<task-notification>\n<status>completed</status>";
    hook(&mut raw, "UserPromptSubmit", json!({ "prompt": note }));
    hook(&mut raw, "PostToolUse", declare("note.pdf"));
    hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
    hook(
        &mut raw,
        "UserPromptSubmit",
        json!({"prompt": "Now the public one."}),
    );
    hook(&mut raw, "PostToolUse", declare("public.pdf"));
    hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
    step(home.path(), &raw);
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["public.pdf"]);
    assert!(
        !PrivateTurn::new(home.path(), "claude", "s1").is_set(),
        "cleared by the public prompt"
    );
}

/// A call before any prompt record of its turn (hooks set up during a turn, a wholly private
/// prompt), and a call whose mark a rule masked, copy nothing; past the cap of a turn, only the
/// files that open count, so fifty missing ones leave a real one copied.
#[test]
fn a_call_with_no_prompt_or_a_masked_mark_copies_nothing() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
    let write = json!({"file_path": w.join("report.pdf")});
    raw.append(&tool("s1", "Write", write.clone(), w)).unwrap();
    raw.append(&ev("s1", "reply", json!({"assistant": "Done."}), w))
        .unwrap();
    let masked = json!({"tool": "Write", "input": write.to_string(), "output": "",
            "failed": false, "[REDACTED:word]": true});
    raw.append(&ev("s2", "prompt", json!({"prompt": "Go."}), w))
        .unwrap();
    raw.append(&ev("s2", "tool", masked, w)).unwrap();
    raw.append(&ev("s2", "reply", json!({"assistant": "Done."}), w))
        .unwrap();
    step(home.path(), &raw);
    assert!(copies(home.path()).is_empty());
    let mut many: Vec<Value> = (0..MAX_FILES)
        .map(|i| json!(w.join(format!("gone-{i}.pdf"))))
        .collect();
    many.push(json!(w.join("report.pdf")));
    let declare = json!({"list": "l", "fields": {"work_products": Value::Array(many).to_string()}});
    turn(
        &mut raw,
        "s3",
        w,
        &[("mcp__oboete__work_state_write", declare)],
    );
    step(home.path(), &raw);
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["report.pdf"]);
}

/// A seq a restore gave again is another turn: its row and the old one are both kept, so
/// forgetting one leaves the copy the other names.
#[test]
fn a_turn_end_is_known_by_its_seq_and_time() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let row = |turn_ts: i64| {
        db.execute(
                "INSERT OR IGNORE INTO deliverables(device, agent, session, turn_seq, turn_ts,
                   call_seq, call_ts, title, kind, source_path, copy, bytes, sha256, declared,
                   copied)
                 VALUES('d', 'claude', 's', 7, ?1, 6, 1, 't', 'file', '/w/a', 'shop/a-1', 1, 'h', 0, 1)",
                [turn_ts],
            )
            .unwrap()
    };
    assert_eq!((row(100), row(200), row(200)), (1, 1, 0));
}

/// A forgotten copy the sweep cannot remove yet keeps its row and fails the sweep, so forget's
/// last step stays owed; once it can, the sweep and `forget --continue`'s purge remove it.
#[cfg(unix)]
#[test]
fn a_forgotten_copy_not_yet_removed_keeps_forget_owed() {
    use std::os::unix::fs::PermissionsExt;
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("own.pdf"), b"own").unwrap();
    turn(
        &mut raw,
        "s1",
        w,
        &[("Write", json!({"file_path": w.join("own.pdf")}))],
    );
    let first = raw.max_seq().unwrap();
    step(home.path(), &raw);
    let copy = copies(home.path()).remove(0).1;
    let folder = home
        .path()
        .join(FOLDER)
        .join(copy.split('/').next().unwrap());
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
    let handle = format!("{}:{first}", raw.device());
    let target = crate::forget::session_target(home.path(), &handle).unwrap();
    let preview = crate::forget::preview(home.path(), target).unwrap();
    crate::forget::start(home.path(), &preview).unwrap();
    drop(raw);
    let raw = crate::raw::open(home.path()).unwrap();
    let blocked = std::fs::remove_file(folder.join("probe"))
        .err()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
        || std::fs::write(folder.join("probe"), b"").is_err();
    let swept = sweep(home.path(), &raw, false);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    if blocked {
        assert!(swept.is_err());
        assert_eq!(copies(home.path()).len(), 1, "its row stays");
    }
    drop(raw);
    crate::forget::purge(home.path()).unwrap();
    assert!(copies(home.path()).is_empty());
    assert!(!home.path().join(FOLDER).join(&copy).exists());
}

/// The sweep never follows a link at `deliverables/` itself: files outside no row names stay,
/// and a folder there that cannot be read is not read; a copy removed through a folder link is
/// refused too.
#[cfg(unix)]
#[test]
fn the_sweep_follows_no_link_at_its_folder() {
    use std::os::unix::fs::PermissionsExt;
    let (home, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir(outside.path().join("shop")).unwrap();
    std::fs::write(outside.path().join("shop").join("keep.txt"), b"keep").unwrap();
    let closed = outside.path().join("closed");
    std::fs::create_dir(&closed).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::os::unix::fs::symlink(outside.path(), home.path().join(FOLDER)).unwrap();
    drop(open(home.path()).unwrap());
    let raw = crate::raw::open(home.path()).unwrap();
    let swept = sweep(home.path(), &raw, true);
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(swept.unwrap(), 0);
    assert!(!remove_copy(home.path(), "shop/keep.txt").unwrap());
    assert!(outside.path().join("shop").join("keep.txt").is_file());
}

/// No Stop clears the flag, an empty one (its turn may go on) or one with a reply (a turn the
/// harness starts next may go on with the private work), nor does the harness's message: the
/// next public prompt does.
#[test]
fn a_private_turn_lasts_until_a_public_prompt() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let settings = crate::capture::Settings::default();
    let (mut failed, mut back) = (0, 0);
    let mut hook = |raw: &mut Raw, event: &str, mut payload: Value| {
        payload["session_id"] = json!("s1");
        payload["cwd"] = json!(home.path());
        crate::hook::record(
            home.path(),
            raw,
            "claude",
            event,
            &payload,
            1,
            &settings,
            &mut failed,
            &mut back,
        )
        .unwrap();
    };
    hook(
        &mut raw,
        "UserPromptSubmit",
        json!({"prompt": "<private>plan</private>"}),
    );
    let set = || PrivateTurn::new(home.path(), "claude", "s1").is_set();
    hook(&mut raw, "Stop", json!({"last_assistant_message": ""}));
    assert!(set());
    hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
    assert!(set());
    let note = "<task-notification>\n<status>completed</status>";
    hook(&mut raw, "UserPromptSubmit", json!({ "prompt": note }));
    assert!(set());
    hook(&mut raw, "UserPromptSubmit", json!({"prompt": "Go on."}));
    assert!(!set());
}

/// A replayed reply in a live session is no boundary of its live turn: the turn's file is
/// copied at the live reply.
#[test]
fn a_replayed_reply_is_no_boundary_of_a_live_turn() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
    raw.append(&ev("s1", "prompt", json!({"prompt": "Go."}), w))
        .unwrap();
    raw.append(&tool(
        "s1",
        "Write",
        json!({"file_path": w.join("report.pdf")}),
        w,
    ))
    .unwrap();
    let replayed = Event {
        source: "replay".into(),
        ..ev("s1", "reply", json!({"assistant": "Replayed."}), w)
    };
    raw.append(&replayed).unwrap();
    raw.append(&ev("s1", "reply", json!({"assistant": "Done."}), w))
        .unwrap();
    step(home.path(), &raw);
    assert_eq!(copies(home.path()).len(), 1);
}

/// The checkpoint's record: the empty store's (seq 0) names none; a record with a time where
/// the checkpoint had none is another one; one a forget made a tombstone since is the same
/// record in the same raw.db file, or with no file kept, and not in another (#836 item 3).
#[test]
fn a_checkpoint_knows_its_record() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let at = |seq, ts, file: Option<&str>| Checkpoint {
        seq,
        ts,
        file: file.map(str::to_owned),
    };
    let file = raw.file_identity().map(str::to_owned);
    assert!(file.is_some());
    assert!(still_there(&raw, &at(0, None, None)).unwrap());
    let seq = raw
        .append(&ev("s1", "prompt", json!({"prompt": "Go."}), home.path()))
        .unwrap();
    let ts = time_at(&raw, seq).unwrap();
    assert!(still_there(&raw, &at(seq, ts, None)).unwrap());
    assert!(!still_there(&raw, &at(seq, None, None)).unwrap());
    assert!(!still_there(&raw, &at(seq, Some(1), None)).unwrap());
    assert!(!still_there(&raw, &at(seq + 1, ts, None)).unwrap());
    let device = raw.device().to_owned();
    raw.append_tombstone(crate::raw::Target::Record { device, seq })
        .unwrap();
    assert_eq!(time_at(&raw, seq).unwrap(), None, "removed now");
    for kept in [ts, None] {
        assert!(still_there(&raw, &at(seq, kept, file.as_deref())).unwrap());
        assert!(still_there(&raw, &at(seq, kept, None)).unwrap());
        assert!(!still_there(&raw, &at(seq, kept, Some("0:0"))).unwrap());
    }
}

/// A replay's hooks for the same session neither clear nor read a live private turn's flag.
#[test]
fn a_replay_leaves_a_live_private_turn_alone() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let replay = crate::capture::Settings {
        source: "replay",
        ..crate::capture::Settings::default()
    };
    let flag = PrivateTurn::new(home.path(), "claude", "s1");
    flag.set().unwrap();
    let (mut failed, mut back) = (0, 0);
    let prompt = json!({"session_id": "s1", "cwd": home.path(), "prompt": "A public one."});
    crate::hook::record(
        home.path(),
        &mut raw,
        "claude",
        "UserPromptSubmit",
        &prompt,
        1,
        &replay,
        &mut failed,
        &mut back,
    )
    .unwrap();
    assert!(flag.is_set());
}

/// Records read past a whole round with no turn end are progress: the worker takes the next
/// round, where a turn end may wait.
#[test]
fn records_read_past_are_progress() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    raw.append(&ev("s1", "prompt", json!({"prompt": "Go."}), w))
        .unwrap();
    for _ in 0..BATCH + 5 {
        raw.append(&tool("s1", "Read", json!({"file_path": "/w/a"}), w))
            .unwrap();
    }
    assert!(step(home.path(), &raw), "a full round");
    assert!(step(home.path(), &raw), "the rest");
    assert!(!step(home.path(), &raw), "nothing left");
}

/// The flag the hooks keep: a flag that cannot be read is set (fail closed).
#[test]
fn a_private_turn_flag_that_cannot_be_read_is_set() {
    let home = tempfile::tempdir().unwrap();
    let flag = PrivateTurn::new(home.path(), "claude", "s1");
    assert!(!flag.is_set());
    flag.set().unwrap();
    assert!(flag.is_set());
    flag.clear().unwrap();
    flag.clear().unwrap();
    assert!(!flag.is_set());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = home.path().join("state").join("private-turns");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = std::fs::read_dir(&dir).is_err();
        let set = flag.is_set();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(set || !unreadable, "an unreadable flag is set");
    }
}

/// A source is checked on its handle: outside the roots, through a link (to a file or a folder)
/// leading out, or a FIFO, it is refused, the FIFO without blocking.
#[test]
fn a_source_outside_the_roots_a_link_out_or_a_fifo_is_refused() {
    let base = tempfile::tempdir().unwrap();
    let (a, b) = (base.path().join("a"), base.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("key"), b"secret").unwrap();
    std::fs::write(a.join("ok.pdf"), b"ok").unwrap();
    let roots = vec![std::fs::canonicalize(&a).unwrap()];
    assert!(open_source(&a.join("ok.pdf"), &roots).is_ok());
    assert!(open_source(&b.join("key"), &roots).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(b.join("key"), a.join("link.pdf")).unwrap();
        assert!(open_source(&a.join("link.pdf"), &roots).is_err());
        std::os::unix::fs::symlink(&b, a.join("dir")).unwrap();
        assert!(open_source(&a.join("dir").join("key"), &roots).is_err());
        let pipe = std::ffi::CString::new(a.join("pipe.pdf").to_str().unwrap()).unwrap();
        // SAFETY: a NUL-terminated path, a fresh name in a folder of this test.
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(open_source(&a.join("pipe.pdf"), &roots).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}

/// A name another file holds (two contents whose hashes start alike) leaves the copy its whole
/// hash's name; the same bytes again are found under it.
#[test]
fn a_name_other_bytes_hold_leaves_the_copy_its_whole_hash() {
    let (home, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = work.path().join("report.pdf");
    std::fs::write(&src, b"new").unwrap();
    let sha = hex(&Sha256::digest(b"new"));
    let dir = held_folder(home.path(), "shop").unwrap().path;
    let short = dir.join(format!("report-{}.pdf", &sha[..8]));
    std::fs::write(&short, b"other").unwrap();
    let roots = roots(work.path());
    let copy = |src: &Path| {
        copy_into(home.path(), "shop", src, open_source(src, &roots).unwrap()).unwrap()
    };
    let copied = copy(&src);
    assert_eq!(copied.copy, format!("shop/report-{sha}.pdf"));
    assert_eq!(std::fs::read(&short).unwrap(), b"other");
    assert_eq!(copy(&src), copied);
    #[cfg(unix)]
    {
        std::fs::remove_file(&short).unwrap();
        std::os::unix::fs::symlink(&src, &short).unwrap();
        assert_eq!(copy(&src), copied, "a link under the name is not read");
        // A larger file under the name (sparse here) is read no further than the copy's size.
        std::fs::remove_file(&short).unwrap();
        let big = std::fs::File::create(&short).unwrap();
        big.set_len(MAX_BYTES + 1).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(copy(&src), copied);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}

/// A cut input names its path by its head.
#[test]
fn a_cut_write_input_names_its_path_by_its_head() {
    let cut = format!(
        "{{\"file_path\":\"/w/report.pdf\",\"content\":\"{}\n…[cut: 99 bytes in full]…\ntail\"}}",
        "x".repeat(10)
    );
    assert_eq!(written("Write", &cut).as_deref(), Some("/w/report.pdf"));
    let quoted = r#"{"file_path":"/w/a\"b.pdf","content":"c"#;
    assert_eq!(written("Write", quoted).as_deref(), Some("/w/a\"b.pdf"));
    let notebook = r#"{"notebook_path":"/w/n.ipynb"}"#;
    assert_eq!(
        written("NotebookEdit", notebook).as_deref(),
        Some("/w/n.ipynb")
    );
    assert_eq!(written("Read", r#"{"file_path":"/w/r.pdf"}"#), None);
}

/// Runs `f` with `act` called at each stage of a copy's or a removal's hold, on this thread.
fn with_seam<T>(act: impl FnMut(Stage) + 'static, f: impl FnOnce() -> T) -> T {
    SEAM.with(|s| *s.borrow_mut() = Some(Box::new(act)));
    let out = f();
    SEAM.with(|s| *s.borrow_mut() = None);
    out
}

/// An act at `at` that moves `dir` to `moved` and puts a link to `outside` in its place, once.
#[cfg(unix)]
fn swap_at(at: Stage, dir: &Path, moved: &Path, outside: &Path) -> impl FnMut(Stage) + 'static {
    let (dir, moved, outside) = (dir.to_owned(), moved.to_owned(), outside.to_owned());
    let mut done = false;
    move |stage| {
        if stage == at && !done {
            done = true;
            std::fs::rename(&dir, &moved).unwrap();
            std::os::unix::fs::symlink(&outside, &dir).unwrap();
        }
    }
}

/// #836 item 2: a copy holds `deliverables/` and its folder by their descriptors. The folder
/// swapped for a link to a folder outside once it is open receives the copy where it was moved:
/// the folder outside, holding a file of the copy's short name with other bytes, is neither
/// read for that name nor written; the same bytes already under the short name in the folder
/// held are reused there.
#[cfg(unix)]
#[test]
fn a_copy_holds_its_folder_when_it_is_swapped_for_a_link() {
    for held_same in [false, true] {
        let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("deck.pdf");
        std::fs::write(&src, b"deck").unwrap();
        let short = copy_name(&src, &hex(&Sha256::digest(b"deck"))[..8]).unwrap();
        let folder = home.path().join(FOLDER).join("shop");
        std::fs::create_dir_all(&folder).unwrap();
        if held_same {
            std::fs::write(folder.join(&short), b"deck").unwrap();
        }
        let outside = out.path().join("shop");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join(&short), b"other").unwrap();
        let moved = out.path().join("moved");
        let act = swap_at(Stage::Folder, &folder, &moved, &outside);
        let copied = with_seam(act, || {
            copy_into(
                home.path(),
                "shop",
                &src,
                std::fs::File::open(&src).unwrap(),
            )
        })
        .unwrap();
        assert_eq!(
            copied.copy,
            format!("shop/{short}"),
            "held_same {held_same}"
        );
        assert_eq!(std::fs::read(moved.join(&short)).unwrap(), b"deck");
        assert_eq!(std::fs::read(outside.join(&short)).unwrap(), b"other");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
    }
}

/// #836 item 2: a copy whose read fails after its partial file is made removes the partial
/// from the folder it holds, also when that folder was swapped for a link meanwhile.
#[cfg(unix)]
#[test]
fn a_failed_copy_removes_its_partial_from_the_held_folder() {
    let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let folder = home.path().join(FOLDER).join("shop");
    std::fs::create_dir_all(&folder).unwrap();
    let outside = out.path().join("shop");
    std::fs::create_dir(&outside).unwrap();
    let moved = out.path().join("moved");
    // A folder's handle read as the source: its read fails, every time.
    let source = std::fs::File::open(out.path()).unwrap();
    let act = swap_at(Stage::Partial, &folder, &moved, &outside);
    let copied = with_seam(act, || {
        copy_into(home.path(), "shop", Path::new("/w/deck.pdf"), source)
    });
    assert!(copied.is_err());
    assert_eq!(
        std::fs::read_dir(&moved).unwrap().count(),
        0,
        "no partial left"
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

/// #836 item 2: `deliverables/` swapped for a link once it is open, before a folder of it is
/// made: the folder is made in the one held, and nothing outside.
#[cfg(unix)]
#[test]
fn a_missing_folder_is_made_in_the_held_top() {
    let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let top = home.path().join(FOLDER);
    std::fs::create_dir(&top).unwrap();
    let outside = out.path().join("top");
    std::fs::create_dir(&outside).unwrap();
    let moved = out.path().join("moved");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("deck.pdf");
    std::fs::write(&src, b"deck").unwrap();
    let act = swap_at(Stage::Top, &top, &moved, &outside);
    let copied = with_seam(act, || {
        copy_into(
            home.path(),
            "shop",
            &src,
            std::fs::File::open(&src).unwrap(),
        )
    })
    .unwrap();
    assert_eq!(std::fs::read(moved.join(&copied.copy)).unwrap(), b"deck");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

/// #836 item 2: a removal holds its folder: swapped for a link once it is open, the folder where
/// it was moved loses the file and the folder outside keeps its file of the same name; the
/// sweep's walk of files no row names does the same, reading the names of the folder it holds
/// (one outside holds none of them).
#[cfg(unix)]
#[test]
fn a_removal_holds_its_folder_when_it_is_swapped_for_a_link() {
    for orphans in [false, true] {
        let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let folder = home.path().join(FOLDER).join("shop");
        std::fs::create_dir_all(&folder).unwrap();
        let held = if orphans { "gone.pdf" } else { "old.pdf" };
        std::fs::write(folder.join(held), b"old").unwrap();
        let outside = out.path().join("shop");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("old.pdf"), b"keep").unwrap();
        let moved = out.path().join("moved");
        drop(open(home.path()).unwrap());
        let raw = crate::raw::open(home.path()).unwrap();
        let act = swap_at(Stage::Folder, &folder, &moved, &outside);
        let removed = with_seam(act, || {
            if orphans {
                sweep(home.path(), &raw, true).unwrap() == 1
            } else {
                remove_copy(home.path(), "shop/old.pdf").unwrap()
            }
        });
        assert!(removed, "orphans {orphans}");
        assert!(!moved.join(held).exists(), "orphans {orphans}");
        assert_eq!(std::fs::read(outside.join("old.pdf")).unwrap(), b"keep");
    }
}

/// The sweep's walk of files no row names passes over a folder it cannot read, said, and sweeps
/// the others, in whatever order the folders are read (CodeRabbit on #845).
#[cfg(unix)]
#[test]
fn the_orphan_walk_passes_over_a_folder_it_cannot_read() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let top = home.path().join(FOLDER);
    for folder in ["closed", "open"] {
        std::fs::create_dir_all(top.join(folder)).unwrap();
        std::fs::write(top.join(folder).join("orphan.pdf"), b"x").unwrap();
    }
    let closed = top.join("closed");
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Where permissions bind (not as root).
    let binds = std::fs::read_dir(&closed).is_err();
    drop(open(home.path()).unwrap());
    let raw = crate::raw::open(home.path()).unwrap();
    let swept = sweep(home.path(), &raw, true);
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
    if binds {
        assert_eq!(swept.unwrap(), 1);
        assert!(closed.join("orphan.pdf").exists());
    }
    assert!(!top.join("open").join("orphan.pdf").exists());
}

/// A folder of `deliverables/` that is a link already is neither written through nor emptied
/// through (as before #836 item 2: its directory opens never follow a link).
#[cfg(unix)]
#[test]
fn a_folder_that_is_a_link_is_not_followed() {
    let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let top = home.path().join(FOLDER);
    std::fs::create_dir(&top).unwrap();
    let outside = out.path().join("shop");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("old.pdf"), b"keep").unwrap();
    std::os::unix::fs::symlink(&outside, top.join("shop")).unwrap();
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("deck.pdf");
    std::fs::write(&src, b"deck").unwrap();
    let file = std::fs::File::open(&src).unwrap();
    assert!(copy_into(home.path(), "shop", &src, file).is_err());
    assert!(!remove_copy(home.path(), "shop/old.pdf").unwrap());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    assert_eq!(std::fs::read(outside.join("old.pdf")).unwrap(), b"keep");
}

/// raw.db rebuilt from this store's records below `cut` and its ops under the same device, and
/// put in place as a restore puts one (`turns.rs`'s #646 test does the same): another file.
fn rebuilt_below(home: &Path, raw: Raw, cut: i64) -> Raw {
    let (device, lines) = (
        raw.device().to_owned(),
        raw.export_lines(0, usize::MAX).unwrap(),
    );
    let ops = raw.export_op_lines(0, usize::MAX).unwrap();
    let before = raw.file_identity().map(str::to_owned);
    drop(raw);
    let rebuilt = home.join("raw.db.rebuilt");
    let mut rebuild = crate::raw::Rebuild::new(&rebuilt, &device).unwrap();
    for (_, line) in lines.iter().filter(|(seq, _)| *seq < cut) {
        rebuild.add(line).unwrap();
    }
    for (_, line) in &ops {
        rebuild.add_op(line).unwrap();
    }
    rebuild.finish().unwrap();
    for file in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(home.join(file));
    }
    std::fs::rename(&rebuilt, home.join("raw.db")).unwrap();
    let raw = crate::raw::open(home).unwrap();
    assert_eq!(raw.device(), device);
    assert_ne!(raw.file_identity().map(str::to_owned), before);
    raw
}

/// #836 item 3: a checkpoint at a record with no time (a tombstone) is known by raw.db's file
/// too. A restore below it that gives its seq to another tombstone is seen, and the phase starts
/// again at the newest record as at any restore, copying nothing older; in the same file the
/// records after it are read.
#[test]
fn a_restore_that_gives_a_timeless_checkpoints_seq_to_another_record_is_seen() {
    for restored in [true, false] {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("plan.pdf"), b"plan").unwrap();
        turn(&mut raw, "s1", w, &[]);
        let target = raw.max_seq().unwrap();
        let device = raw.device().to_owned();
        let tombstone = |raw: &mut Raw| {
            raw.append_tombstone(crate::raw::Target::Record {
                device: device.clone(),
                seq: target,
            })
            .unwrap()
        };
        let cut = tombstone(&mut raw);
        assert!(step(home.path(), &raw));
        let db = open(home.path()).unwrap();
        let at = checkpoint(&db, &device).unwrap().map(|c| (c.seq, c.ts));
        assert_eq!(at, Some((cut, None)));
        if restored {
            raw = rebuilt_below(home.path(), raw, cut);
            assert_eq!(tombstone(&mut raw), cut, "the seq given again");
        } else {
            tombstone(&mut raw);
        }
        let write = [("Write", json!({"file_path": w.join("plan.pdf")}))];
        turn(&mut raw, "s2", w, &write);
        step(home.path(), &raw);
        assert_eq!(
            copies(home.path()).len(),
            usize::from(!restored),
            "restored: {restored}"
        );
    }
}

/// #836 item 3: a checkpoint kept before raw.db's file was (`file` NULL) is taken as it is, and
/// the file is written to it then, also when no record follows it, so a restore after that is
/// seen; with a turn after it, the turn is copied.
#[test]
fn a_checkpoint_kept_before_the_file_takes_it_once() {
    for pending in [false, true] {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("plan.pdf"), b"plan").unwrap();
        turn(&mut raw, "s1", w, &[]);
        let target = raw.max_seq().unwrap();
        let device = raw.device().to_owned();
        let tombstone = |raw: &mut Raw| {
            raw.append_tombstone(crate::raw::Target::Record {
                device: device.clone(),
                seq: target,
            })
            .unwrap()
        };
        let cut = tombstone(&mut raw);
        assert!(step(home.path(), &raw));
        let db = open(home.path()).unwrap();
        db.execute("UPDATE deliverable_scan SET file = NULL", [])
            .unwrap();
        let write = [("Write", json!({"file_path": w.join("plan.pdf")}))];
        if pending {
            turn(&mut raw, "s2", w, &write);
            assert!(step(home.path(), &raw));
            assert_eq!(copies(home.path()).len(), 1);
            continue;
        }
        assert!(!step(home.path(), &raw));
        let c = checkpoint(&db, &device).unwrap().unwrap();
        assert_eq!(
            (c.seq, c.ts, c.file.as_deref()),
            (cut, None, raw.file_identity())
        );
        raw = rebuilt_below(home.path(), raw, cut);
        assert_eq!(tombstone(&mut raw), cut);
        turn(&mut raw, "s2", w, &write);
        step(home.path(), &raw);
        assert!(copies(home.path()).is_empty());
    }
}

/// The first run copies none of the records already there; a restore that ends below the
/// checkpoint moves it to the newest record, copying nothing older.
#[test]
fn the_first_run_and_a_restore_copy_nothing_older() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
    let write = || [("Write", json!({"file_path": w.join("report.pdf")}))];
    turn(&mut raw, "s1", w, &write());
    assert!(!step(home.path(), &raw));
    assert!(copies(home.path()).is_empty());
    let db = open(home.path()).unwrap();
    // raw.db ends below the checkpoint, or another record's time is at its seq.
    for ahead in [10, 0] {
        let seq = raw.max_seq().unwrap() + ahead;
        set_checkpoint(&db, &raw, seq, Some(1)).unwrap();
        turn(&mut raw, "s1", w, &write());
        assert!(!step(home.path(), &raw), "{ahead}");
        assert!(copies(home.path()).is_empty(), "{ahead}");
        let at = checkpoint(&db, raw.device()).unwrap().map(|c| c.seq);
        assert_eq!(at, Some(raw.max_seq().unwrap()), "{ahead}");
    }
    turn(&mut raw, "s1", w, &write());
    assert!(step(home.path(), &raw));
    assert_eq!(copies(home.path()).len(), 1);
}

/// A command asking the worker to step aside between two files leaves the turn to the next
/// round, which copies it once.
#[test]
fn a_step_aside_leaves_the_turn_to_the_next_round() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
    // Declared: no git check comes first, so the copy's own wait is what gives way.
    let declare = json!({"list": "l", "fields": {"work_product": w.join("report.pdf")}});
    turn(
        &mut raw,
        "s1",
        w,
        &[("mcp__oboete__work_state_write", declare)],
    );
    assert!(
        run(
            home.path(),
            &raw,
            &drained(home.path(), &raw),
            &mut None,
            &|| true
        )
        .unwrap()
    );
    assert!(copies(home.path()).is_empty());
    assert!(step(home.path(), &raw));
    assert_eq!(copies(home.path()).len(), 1);
    assert!(!step(home.path(), &raw));
}

/// Forgetting a session removes its rows and the files no other row names; a copy whose
/// records are only not found keeps them.
#[test]
fn forgetting_the_session_removes_its_copies() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("shared.pdf"), b"shared").unwrap();
    std::fs::write(w.join("own.pdf"), b"own").unwrap();
    let write = |f: &str| ("Write", json!({"file_path": w.join(f)}));
    turn(&mut raw, "s1", w, &[write("shared.pdf"), write("own.pdf")]);
    let first = raw.max_seq().unwrap();
    turn(&mut raw, "s2", w, &[write("shared.pdf")]);
    let second = raw.max_seq().unwrap();
    step(home.path(), &raw);
    let before = copies(home.path());
    assert_eq!(before.len(), 3);
    let handle = format!("{}:{first}", raw.device());
    let target = crate::forget::session_target(home.path(), &handle).unwrap();
    let preview = crate::forget::preview(home.path(), target).unwrap();
    crate::forget::start(home.path(), &preview).unwrap();
    let raw = crate::raw::open(home.path()).unwrap();
    sweep(home.path(), &raw, false).unwrap();
    let after = copies(home.path());
    assert_eq!(after.len(), 1);
    let file = |c: &str| home.path().join(FOLDER).join(c);
    assert!(file(&after[0].1).is_file(), "s2's copy stays");
    assert!(!file(&before[1].1).exists(), "s1's own copy goes");
    // A row whose records are not found (a restored raw.db) is no forget.
    let db = open(home.path()).unwrap();
    db.execute(
        "UPDATE deliverables SET turn_seq = turn_seq + 1000, call_seq = call_seq + 1000",
        [],
    )
    .unwrap();
    sweep(home.path(), &raw, true).unwrap();
    assert_eq!(copies(home.path()).len(), 1);
    assert!(file(&after[0].1).is_file());
    // Its session forgotten later removes it still, by the forget's scope.
    let handle = format!("{}:{second}", raw.device());
    let target = crate::forget::session_target(home.path(), &handle).unwrap();
    let preview = crate::forget::preview(home.path(), target).unwrap();
    crate::forget::start(home.path(), &preview).unwrap();
    let raw = crate::raw::open(home.path()).unwrap();
    sweep(home.path(), &raw, false).unwrap();
    assert!(copies(home.path()).is_empty());
    assert!(!file(&after[0].1).exists());
}

/// Slice 2: a copy is opened to serve only at its place, of its size and bytes, within its
/// time: a same-size file put in its place, another size, a link there, and `deliverables/`
/// swapped for a link to a folder holding the same name and bytes are refused. A home reached
/// through a link reads its copies' store; a link in the store's place is refused.
#[test]
fn a_copy_opens_only_as_its_row_says() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("deck.pdf"), b"deck one").unwrap();
    let write = json!({"file_path": w.join("deck.pdf")});
    turn(&mut raw, "s1", w, &[("Write", write)]);
    assert!(step(home.path(), &raw));
    let db = open_read(home.path()).unwrap().unwrap();
    let row = rows(&db).unwrap().remove(0);
    let later = || std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut got = Vec::new();
    (open_copy(home.path(), &row, later()).unwrap().unwrap())
        .read_to_end(&mut got)
        .unwrap();
    assert_eq!(got, b"deck one");
    let copy = home.path().join(FOLDER).join(&row.copy);
    std::fs::write(&copy, b"deck two").unwrap();
    let none = |deadline| matches!(open_copy(home.path(), &row, deadline), Ok(None));
    assert!(none(later()), "same size");
    std::fs::write(&copy, b"deck").unwrap();
    assert!(none(later()), "another size");
    std::fs::write(&copy, b"deck one").unwrap();
    let past = std::time::Instant::now();
    assert!(open_copy(home.path(), &row, past).is_err(), "out of time");
    assert!(open_copy(home.path(), &row, later()).unwrap().is_some());
    #[cfg(unix)]
    {
        std::fs::remove_file(&copy).unwrap();
        std::fs::write(w.join("other"), b"deck one").unwrap();
        std::os::unix::fs::symlink(w.join("other"), &copy).unwrap();
        assert!(none(later()), "a link");
        std::fs::remove_file(&copy).unwrap();
        std::fs::write(&copy, b"deck one").unwrap();
        let top = home.path().join(FOLDER);
        let moved = w.join("moved");
        std::fs::rename(&top, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &top).unwrap();
        assert!(none(later()), "a linked folder");
        let alias = w.join("home");
        std::os::unix::fs::symlink(home.path(), &alias).unwrap();
        let db = open_read(&alias)
            .unwrap()
            .expect("a home reached through a link");
        assert_eq!(rows(&db).unwrap(), std::slice::from_ref(&row));
        let leaf = w.join("leaf");
        std::fs::create_dir(&leaf).unwrap();
        std::os::unix::fs::symlink(home.path().join(DB), leaf.join(DB)).unwrap();
        assert!(open_read(&leaf).is_err(), "the store a link");
    }
}

/// Slice 2b: a list takes a copy as on disk only as `open_copy` would open it: of its row's
/// size, with no link in its place nor for its folder, its home reached through a link too, by
/// one open and the handle's own path, so a folder swapped for a link between two checks is
/// not followed (Codex's local review of slice 2b).
#[test]
fn a_copy_is_on_disk_only_where_it_would_open() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("deck.pdf"), b"deck one").unwrap();
    let write = json!({"file_path": w.join("deck.pdf")});
    turn(&mut raw, "s1", w, &[("Write", write)]);
    assert!(step(home.path(), &raw));
    let db = open_read(home.path()).unwrap().unwrap();
    let row = rows(&db).unwrap().remove(0);
    assert!(on_disk(home.path(), &row));
    let copy = home.path().join(FOLDER).join(&row.copy);
    std::fs::write(&copy, b"deck").unwrap();
    assert!(!on_disk(home.path(), &row), "another size");
    std::fs::write(&copy, b"deck one").unwrap();
    #[cfg(unix)]
    {
        let alias = w.join("home");
        std::os::unix::fs::symlink(home.path(), &alias).unwrap();
        assert!(on_disk(&alias, &row), "a home reached through a link");
        // Its folder swapped for a link to one outside that holds the same name and size.
        let folder = copy.parent().unwrap();
        let outside = w.join("outside");
        std::fs::rename(folder, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, folder).unwrap();
        assert!(!on_disk(home.path(), &row), "a linked folder");
        std::fs::remove_file(folder).unwrap();
        std::fs::rename(&outside, folder).unwrap();
        assert!(on_disk(home.path(), &row));
        std::fs::remove_file(&copy).unwrap();
        std::os::unix::fs::symlink(w.join("deck.pdf"), &copy).unwrap();
        assert!(!on_disk(home.path(), &row), "a link in its place");
    }
    let _ = std::fs::remove_file(&copy);
    assert!(!on_disk(home.path(), &row), "gone");
}

/// A row's provenance names its turn, call and name too: a row id a sweep freed, taken by a
/// copy of the same bytes at the same time in another turn, has another provenance and
/// fingerprint (and so another link's tag).
#[test]
fn a_reused_row_id_is_another_copy() {
    let row = Row {
        id: 7,
        device: "d".into(),
        agent: "claude".into(),
        session: "s".into(),
        repo: None,
        turn_seq: 10,
        turn_ts: 100,
        call_seq: 9,
        call_ts: 99,
        title: "a.pdf".into(),
        kind: "document".into(),
        source_path: "/w/a.pdf".into(),
        copy: "r/a-1.pdf".into(),
        bytes: 3,
        sha256: "ab".into(),
        copied: 1_000,
    };
    let other = Row {
        turn_seq: 20,
        turn_ts: 200,
        ..row.clone()
    };
    assert_ne!(provenance(&row), provenance(&other));
    assert_ne!(fingerprint([&row]), fingerprint([&other]));
    assert_eq!(fingerprint([&row]), fingerprint([&row.clone()]));
    assert_ne!(fingerprint([&row]), fingerprint(std::iter::empty::<&Row>()));
}

/// A long turn's records are read one at a time (#836): a turn of hundreds of records copies
/// its file all the same.
#[test]
fn a_long_turn_copies_its_file() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("deck.pdf"), b"deck").unwrap();
    let mut calls: Vec<(&str, Value)> = (0..450)
        .map(|i| ("Read", json!({"file_path": format!("/w/{i}.rs")})))
        .collect();
    calls.push(("Write", json!({"file_path": w.join("deck.pdf")})));
    turn(&mut raw, "s1", w, &calls);
    // The phase reads `BATCH` records a round, as the worker calls it while it goes on.
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["deck.pdf"]);
}

/// git's verdict is read in English whatever the user's language (#836): a git that answers
/// in another language unless `LC_ALL=C` still leaves a folder in no repository outside.
#[cfg(unix)]
#[test]
fn a_folder_in_no_repository_is_outside_in_any_language() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let git = bin.path().join("git");
    let script = "#!/bin/sh\nif [ \"$LC_ALL\" = C ]; then\n  \
            echo 'fatal: not a git repository (or any of the parent directories): .git' >&2\n\
            else\n  echo 'fatal: git リポジトリではありません' >&2\nfi\nexit 128\n";
    std::fs::write(&git, script).unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("deck.pdf");
    std::fs::write(&file, b"deck").unwrap();
    assert!(outside_source_by(
        git.as_os_str(),
        &file,
        &mut HashMap::new()
    ));
}

/// Slice 3 (docs/subagents.md): subagent `id`'s call, marked public as the hooks mark one.
fn sub_call(session: &str, id: &str, file: &Path, cwd: &Path) -> Event {
    let input = json!({"file_path": file}).to_string();
    let body = json!({"tool": "Write", "input": input, "output": "", "failed": false,
            "agent_id": id, "agent_type": "general-purpose"});
    let mut e = ev(session, "tool", body, cwd);
    mark_public(&mut e);
    e
}

/// The parent's call that launched subagent `id`, in a public turn: in the background, or
/// returned with it.
fn launch_of(session: &str, id: &str, background: bool, cwd: &Path) -> Event {
    let input = json!({"description": "Draw the chart", "prompt": "Draw it."}).to_string();
    let status = if background {
        "async_launched"
    } else {
        "completed"
    };
    let body = json!({"tool": "Agent", "input": input, "output": "", "failed": false,
            "launched": {"agent_id": id, "async": background, "status": status}});
    let mut e = ev(session, "tool", body, cwd);
    mark_public(&mut e);
    e
}

/// Subagent `id`'s stop, as the hook keeps one.
fn stop_of(session: &str, id: &str, cwd: &Path) -> Event {
    let body = json!({"agent_id": id, "agent_type": "general-purpose"});
    ev(session, crate::raw::SUBAGENT_STOP, body, cwd)
}

fn said(raw: &mut Raw, session: &str, kind: &str, cwd: &Path) -> i64 {
    raw.append(&ev(session, kind, json!({"text": "Go on."}), cwd))
        .unwrap()
}

/// Each row's title, its turn's seq and its call's seq, by id.
fn origins(home: &Path) -> Vec<(String, i64, i64)> {
    let db = open(home).unwrap();
    db.prepare("SELECT title, turn_seq, call_seq FROM deliverables ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// The owner's forget of one record of this device: registered, and when `purge`, purged as
/// the worker finishes one (its row rewritten without its labels, its copies swept).
fn forget_record(home: &Path, raw: Raw, seq: i64, purge: bool) -> Raw {
    let device = raw.device().to_owned();
    drop(raw);
    let target = crate::forget::Target::Record { device, seq };
    let preview = crate::forget::preview(home, target).unwrap();
    crate::forget::start(home, &preview).unwrap();
    if purge {
        crate::forget::purge(home).unwrap();
    }
    crate::raw::open(home).unwrap()
}

fn files(dir: &Path, names: &[&str]) {
    for name in names {
        if let Some(parent) = dir.join(name).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(dir.join(name), name.as_bytes()).unwrap();
    }
}

/// Slice 3: a file a background subagent wrote after its parent's reply, with no prompt
/// between, is copied at its stop, once, its row the reply's turn and the writing call's; one
/// it wrote inside its parent's turn is the turn's, a foreground subagent's too, and not taken
/// again at its stop; a stop after the parent's next prompt still takes the gap's call; a
/// session's end with no reply copies the prompt-backed file, and the stop nothing again.
#[test]
fn a_background_subagents_file_is_copied_at_its_stop_once() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(
        w,
        &[
            "in.pdf", "out.pdf", "fore.pdf", "mid.pdf", "late.pdf", "end.pdf",
        ],
    );
    // Straddling its parent's reply.
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    let inside = raw
        .append(&sub_call("s1", "a1", &w.join("in.pdf"), w))
        .unwrap();
    let reply = said(&mut raw, "s1", "reply", w);
    let gap = raw
        .append(&sub_call("s1", "a1", &w.join("out.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    // A foreground one: it stops before the call that started it returns.
    said(&mut raw, "s2", "prompt", w);
    let fore = raw
        .append(&sub_call("s2", "f1", &w.join("fore.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s2", "f1", w)).unwrap();
    raw.append(&launch_of("s2", "f1", false, w)).unwrap();
    let reply2 = said(&mut raw, "s2", "reply", w);
    // One that writes in its parent's next turn too, and stops after it.
    said(&mut raw, "s3", "prompt", w);
    raw.append(&launch_of("s3", "l1", true, w)).unwrap();
    let reply3 = said(&mut raw, "s3", "reply", w);
    let late = raw
        .append(&sub_call("s3", "l1", &w.join("late.pdf"), w))
        .unwrap();
    said(&mut raw, "s3", "prompt", w);
    let mid = raw
        .append(&sub_call("s3", "l1", &w.join("mid.pdf"), w))
        .unwrap();
    let next = said(&mut raw, "s3", "reply", w);
    raw.append(&stop_of("s3", "l1", w)).unwrap();
    // A session that ends with no reply.
    said(&mut raw, "s4", "prompt", w);
    raw.append(&launch_of("s4", "e1", true, w)).unwrap();
    let ended = raw
        .append(&sub_call("s4", "e1", &w.join("end.pdf"), w))
        .unwrap();
    let end = said(&mut raw, "s4", "end", w);
    raw.append(&stop_of("s4", "e1", w)).unwrap();
    while step(home.path(), &raw) {}
    let expected: Vec<(String, i64, i64)> = vec![
        ("in.pdf".into(), reply, inside),
        ("out.pdf".into(), reply, gap),
        ("fore.pdf".into(), reply2, fore),
        ("mid.pdf".into(), next, mid),
        ("late.pdf".into(), reply3, late),
        ("end.pdf".into(), end, ended),
    ];
    assert_eq!(origins(home.path()), expected);
    let db = open(home.path()).unwrap();
    set_checkpoint(&db, &raw, 0, None).unwrap();
    while step(home.path(), &raw) {}
    assert_eq!(origins(home.path()), expected, "nothing twice");
}

/// Slice 3: one stop over two gaps keeps each gap's copies apart under the unique key:
/// identical content at two paths is two rows, each its gap's reply's and its call's; one path
/// written in both is one row, the last write's (the bytes on disk are its). A forget of either
/// call leaves the other's row and the file; of both, neither.
#[test]
fn one_stop_over_two_gaps_keeps_each_gaps_copies_apart() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["notes.pdf"]);
    for d in ["one", "two"] {
        std::fs::create_dir(w.join(d)).unwrap();
        std::fs::write(w.join(d).join("report.pdf"), b"the same bytes").unwrap();
    }
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    let r1 = said(&mut raw, "s1", "reply", w);
    let c1 = (raw.append(&sub_call("s1", "a1", &w.join("one/report.pdf"), w))).unwrap();
    (raw.append(&sub_call("s1", "a1", &w.join("notes.pdf"), w))).unwrap();
    said(&mut raw, "s1", "prompt", w);
    let r2 = said(&mut raw, "s1", "reply", w);
    let c3 = (raw.append(&sub_call("s1", "a1", &w.join("two/report.pdf"), w))).unwrap();
    let c4 = (raw.append(&sub_call("s1", "a1", &w.join("notes.pdf"), w))).unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    step(home.path(), &raw);
    let expected: Vec<(String, i64, i64)> = vec![
        ("report.pdf".into(), r1, c1),
        ("notes.pdf".into(), r2, c4),
        ("report.pdf".into(), r2, c3),
    ];
    assert_eq!(origins(home.path()), expected);
    let copy = copies(home.path())[0].1.clone();
    assert_eq!(copies(home.path())[2].1, copy, "one file for one content");
    let file = home.path().join(FOLDER).join(&copy);
    let raw = forget_record(home.path(), raw, c1, true);
    assert_eq!(origins(home.path()), expected[1..]);
    assert!(file.is_file(), "the other call's row names it");
    drop(forget_record(home.path(), raw, c3, true));
    assert_eq!(origins(home.path()), expected[1..2]);
    assert!(!file.exists());
}

/// Slice 3: a later stop takes only the calls after the one the phase took, though that stop
/// is forgotten since (its copies stay: their origins are the calls and the reply) and a
/// source changed since; so a resumed subagent's new file is copied though its earlier calls
/// named `MAX_FILES` files. A stop with no call since copies nothing; a replay's and a
/// transcript's stop are no boundary; a stop taken again after an interruption, before its
/// place was kept or after, makes no row twice; a restore empties the places kept.
#[test]
fn a_later_stop_takes_only_the_calls_after_the_one_taken() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    let names: Vec<String> = (0..MAX_FILES).map(|i| format!("f{i}.pdf")).collect();
    files(w, &names.iter().map(String::as_str).collect::<Vec<_>>());
    files(w, &["new.pdf", "after.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    let reply = said(&mut raw, "s1", "reply", w);
    for name in &names {
        raw.append(&sub_call("s1", "a1", &w.join(name), w)).unwrap();
    }
    let first = raw.append(&stop_of("s1", "a1", w)).unwrap();
    step(home.path(), &raw);
    assert_eq!(origins(home.path()).len(), MAX_FILES);
    let mut raw = forget_record(home.path(), raw, first, true);
    assert_eq!(origins(home.path()).len(), MAX_FILES, "the copies stay");
    std::fs::write(w.join("f0.pdf"), b"changed since").unwrap();
    let resumed = raw
        .append(&sub_call("s1", "a1", &w.join("new.pdf"), w))
        .unwrap();
    let second = raw.append(&stop_of("s1", "a1", w)).unwrap();
    step(home.path(), &raw);
    let rows = origins(home.path());
    assert_eq!(rows.len(), MAX_FILES + 1);
    assert_eq!(rows[MAX_FILES], ("new.pdf".into(), reply, resumed));
    // No call since: nothing, the stop read past.
    let empty = raw.append(&stop_of("s1", "a1", w)).unwrap();
    step(home.path(), &raw);
    assert_eq!(origins(home.path()).len(), MAX_FILES + 1);
    let db = open(home.path()).unwrap();
    assert_eq!(checkpoint(&db, raw.device()).unwrap().unwrap().seq, empty);
    // Taken again: before its place was kept, and after.
    for kept in [first, second] {
        (db.execute("UPDATE subagent_scan SET seq = ?1", [kept])).unwrap();
        set_checkpoint(&db, &raw, resumed, time_at(&raw, resumed).unwrap()).unwrap();
        while step(home.path(), &raw) {}
        assert_eq!(origins(home.path()).len(), MAX_FILES + 1);
    }
    // A replay's and a transcript's stop between a call and the hook's stop.
    let after = raw
        .append(&sub_call("s1", "a1", &w.join("after.pdf"), w))
        .unwrap();
    for source in ["replay", "transcript"] {
        let mut other = stop_of("s1", "a1", w);
        other.source = source.into();
        raw.append(&other).unwrap();
    }
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    while step(home.path(), &raw) {}
    let rows = origins(home.path());
    assert_eq!(rows.last().unwrap(), &("after.pdf".into(), reply, after));
    let kept = |db: &Connection| -> i64 {
        (db.query_row("SELECT count(*) FROM subagent_scan", [], |r| r.get(0))).unwrap()
    };
    assert_eq!(kept(&db), 1);
    drop(db);
    let raw = rebuilt_below(home.path(), raw, resumed);
    step(home.path(), &raw);
    assert_eq!(kept(&open(home.path()).unwrap()), 0);
}

/// Slice 3: a stop the phase did not take, forgotten before it read it, leaves its calls to
/// the next stop.
#[test]
fn a_stop_forgotten_before_the_phase_read_it_leaves_its_calls_to_the_next() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["one.pdf", "two.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    let reply = said(&mut raw, "s1", "reply", w);
    let one = raw
        .append(&sub_call("s1", "a1", &w.join("one.pdf"), w))
        .unwrap();
    let first = raw.append(&stop_of("s1", "a1", w)).unwrap();
    let mut raw = forget_record(home.path(), raw, first, true);
    let two = raw
        .append(&sub_call("s1", "a1", &w.join("two.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    while step(home.path(), &raw) {}
    let expected: Vec<(String, i64, i64)> = vec![
        ("one.pdf".into(), reply, one),
        ("two.pdf".into(), reply, two),
    ];
    assert_eq!(origins(home.path()), expected);
}

/// Slice 3, through the hooks: a subagent launched from a wholly private prompt's turn, or from
/// the turn a task's notification starts after it, copies nothing, though a later public
/// prompt cleared the session's flag before it wrote; one launched from a public turn copies
/// its file.
#[test]
fn a_subagent_launched_from_a_private_turn_copies_nothing() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["secret.pdf", "notified.pdf", "public.pdf"]);
    let settings = crate::capture::Settings::default();
    let hook = |raw: &mut Raw, session: &str, event: &str, mut payload: Value| {
        payload["session_id"] = json!(session);
        payload["cwd"] = json!(w);
        let (mut failed, mut back) = (0, 0);
        let ts = crate::db::now_ms();
        crate::hook::record(
            home.path(),
            raw,
            "claude",
            event,
            &payload,
            ts,
            &settings,
            &mut failed,
            &mut back,
        )
        .unwrap();
    };
    // The prompts before the launch: a wholly private one; the same, then the turn a task's
    // notification starts after it, private still though its record is there; a public one.
    let private = "<private>the real plan</private>";
    let note = "<task-notification>\n<status>completed</status>";
    let cases: [(&str, &[&str], &str); 3] = [
        ("s1", &[private], "secret.pdf"),
        ("s2", &[private, note], "notified.pdf"),
        ("s3", &["Draw the chart."], "public.pdf"),
    ];
    for (session, prompts, file) in cases {
        let id = format!("{session}a");
        for (i, prompt) in prompts.iter().enumerate() {
            if i > 0 {
                let done = json!({"last_assistant_message": "Done."});
                hook(&mut raw, session, "Stop", done);
            }
            hook(
                &mut raw,
                session,
                "UserPromptSubmit",
                json!({ "prompt": prompt }),
            );
        }
        let launched = json!({"tool_name": "Agent",
                "tool_input": {"description": "Draw", "prompt": "Draw it."},
                "tool_response": {"agentId": id, "status": "async_launched", "isAsync": true}});
        hook(&mut raw, session, "PostToolUse", launched);
        hook(
            &mut raw,
            session,
            "Stop",
            json!({"last_assistant_message": "On it."}),
        );
        hook(
            &mut raw,
            session,
            "UserPromptSubmit",
            json!({"prompt": "And a legend?"}),
        );
        hook(
            &mut raw,
            session,
            "Stop",
            json!({"last_assistant_message": "Soon."}),
        );
        let wrote = json!({"tool_name": "Write", "agent_id": id,
                "tool_input": {"file_path": w.join(file), "content": "x"},
                "tool_response": {"type": "create"}});
        hook(&mut raw, session, "PostToolUse", wrote);
        hook(&mut raw, session, "SubagentStop", json!({ "agent_id": id }));
    }
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["public.pdf"]);
}

/// Slice 3: with no launch record, its launch forgotten (purged or registered alone) or its id
/// masked, launched from a turn with no prompt observed, a replay's or a transcript's launch, a
/// call that failed or is not marked public, or a Codex session's, a stop copies nothing;
/// beside them a subagent launched as the hooks see a public turn copies its file.
#[test]
fn a_subagent_with_no_public_launch_or_call_copies_nothing() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    let cases = [
        "nolaunch",
        "forgotten",
        "registered",
        "masked",
        "unseen",
        "replayed",
        "imported",
        "failed",
        "unmarked",
        "codex",
        "notified",
        "control",
    ];
    let names: Vec<String> = cases.iter().map(|c| format!("{c}.pdf")).collect();
    files(w, &names.iter().map(String::as_str).collect::<Vec<_>>());
    let mut launches = HashMap::new();
    for case in cases {
        let agent = |mut e: Event| {
            if case == "codex" {
                e.agent = "codex".into();
            }
            e
        };
        match case {
            "unseen" => {}
            // A turn a task's notification starts.
            "notified" => {
                said(&mut raw, case, "envelope", w);
            }
            _ => {
                let prompt = ev(case, "prompt", json!({"prompt": "Go."}), w);
                raw.append(&agent(prompt)).unwrap();
            }
        }
        if case != "nolaunch" {
            let mut launch = agent(launch_of(case, case, true, w));
            launch.source = match case {
                "replayed" => "replay".into(),
                "imported" => "transcript".into(),
                _ => launch.source,
            };
            let seq = raw.append(&launch).unwrap();
            launches.insert(case, seq);
        }
        raw.append(&agent(ev(case, "reply", json!({"assistant": "On it."}), w)))
            .unwrap();
        let mut call = sub_call(case, case, &w.join(format!("{case}.pdf")), w);
        if case == "failed" {
            call.body = call.body.replace("\"failed\":false", "\"failed\":true");
        }
        if case == "unmarked" {
            let body = json!({"tool": "Write",
                    "input": json!({"file_path": w.join("unmarked.pdf")}).to_string(),
                    "output": "", "failed": false, "agent_id": case});
            call = ev(case, "tool", body, w);
        }
        raw.append(&agent(call)).unwrap();
        raw.append(&agent(stop_of(case, case, w))).unwrap();
    }
    let device = raw.device().to_owned();
    let masked = launches["masked"];
    let body = match &raw.after(&device, masked - 1, 1).unwrap()[0].item {
        Item::Event(e) => e.body.clone(),
        _ => panic!("an event"),
    };
    let at = body.find("\"agent_id\":\"masked\"").unwrap() + "\"agent_id\":\"".len();
    let target = crate::raw::Target::Range {
        device,
        seq: masked,
        offset: at as i64,
        length: "masked".len() as i64,
    };
    raw.append_tombstone(target).unwrap();
    let raw = forget_record(home.path(), raw, launches["forgotten"], true);
    // Registered, not yet purged.
    let raw = forget_record(home.path(), raw, launches["registered"], false);
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["notified.pdf", "control.pdf"]);
}

/// Slice 3: the phase's first run copies nothing older; a stop after it takes no call before
/// the subagent's earlier stop, which the facts hold though the phase never took it. A
/// subagent that ran across the first run and stops after it takes its calls since its launch,
/// as a turn that ends after the first run takes its records from before it, and as
/// claude-mem's SubagentStop reads the subagent's transcript.
#[test]
fn a_stop_after_the_first_run_takes_no_call_before_an_earlier_stop() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["old.pdf", "new.pdf", "across.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    let reply = said(&mut raw, "s1", "reply", w);
    raw.append(&sub_call("s1", "a1", &w.join("old.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    said(&mut raw, "s2", "prompt", w);
    raw.append(&launch_of("s2", "b1", true, w)).unwrap();
    let reply2 = said(&mut raw, "s2", "reply", w);
    let across = raw
        .append(&sub_call("s2", "b1", &w.join("across.pdf"), w))
        .unwrap();
    assert!(!step(home.path(), &raw));
    let new = raw
        .append(&sub_call("s1", "a1", &w.join("new.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    raw.append(&stop_of("s2", "b1", w)).unwrap();
    while step(home.path(), &raw) {}
    let expected: Vec<(String, i64, i64)> = vec![
        ("new.pdf".into(), reply, new),
        ("across.pdf".into(), reply2, across),
    ];
    assert_eq!(origins(home.path()), expected);
}

/// Slice 3: a gap whose reply a forget denies takes nothing, registered (its labels still
/// there) or purged; a prompt forgotten and purged between the reply and a call makes the call
/// no gap's, as it is no copy of its turn's, also when an earlier call of the same stop lay in
/// the gap before it; beside them a gap copies.
#[test]
fn a_gap_whose_reply_or_prompt_was_forgotten_takes_nothing() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(
        w,
        &[
            "denied.pdf",
            "purged.pdf",
            "prompt.pdf",
            "between-gap.pdf",
            "between.pdf",
            "control.pdf",
        ],
    );
    let (mut purged, mut denied) = (Vec::new(), 0);
    for case in ["denied", "purged", "prompt", "between", "control"] {
        said(&mut raw, case, "prompt", w);
        raw.append(&launch_of(case, case, true, w)).unwrap();
        let reply = said(&mut raw, case, "reply", w);
        // A call in the gap, then the purged prompt's turn: the second call is no gap's.
        if case == "between" {
            let file = w.join("between-gap.pdf");
            raw.append(&sub_call(case, case, &file, w)).unwrap();
        }
        if matches!(case, "prompt" | "between") {
            purged.push(said(&mut raw, case, "prompt", w));
        }
        let file = w.join(format!("{case}.pdf"));
        raw.append(&sub_call(case, case, &file, w)).unwrap();
        if matches!(case, "prompt" | "between") {
            said(&mut raw, case, "reply", w);
        }
        raw.append(&stop_of(case, case, w)).unwrap();
        match case {
            "denied" => denied = reply,
            "purged" => purged.push(reply),
            _ => {}
        }
    }
    for seq in purged {
        raw = forget_record(home.path(), raw, seq, true);
    }
    // Registered, not yet purged: the reply's labels are still read.
    let raw = forget_record(home.path(), raw, denied, false);
    let db = Connection::open(home.path().join("raw.db")).unwrap();
    let labels: Option<String> = (db.query_row(
        "SELECT session FROM records WHERE device = ?1 AND seq = ?2",
        params![raw.device(), denied],
        |r| r.get(0),
    ))
    .unwrap();
    assert_eq!(labels.as_deref(), Some("denied"));
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["between-gap.pdf", "control.pdf"]);
}

/// Slice 3: with the `subagents` consumer behind, reply A is taken and the stop waits with
/// reply B behind it, the phase's checkpoint below the stop; with no checkpoint, or a rebuild
/// owed, it waits too; once the consumer has reached it, both are taken, no row twice.
#[test]
fn a_stop_waits_for_the_subagents_facts() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["a.pdf", "gap.pdf", "b.pdf"]);
    let parent = |file: &str| tool("s1", "Write", json!({"file_path": w.join(file)}), w);
    said(&mut raw, "s1", "prompt", w);
    raw.append(&parent("a.pdf")).unwrap();
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    let a = said(&mut raw, "s1", "reply", w);
    raw.append(&sub_call("s1", "a1", &w.join("gap.pdf"), w))
        .unwrap();
    let behind = drained(home.path(), &raw);
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    said(&mut raw, "s1", "prompt", w);
    raw.append(&parent("b.pdf")).unwrap();
    said(&mut raw, "s1", "reply", w);
    let titles = || -> Vec<String> { copies(home.path()).into_iter().map(|c| c.0).collect() };
    let at = || {
        checkpoint(&open(home.path()).unwrap(), raw.device())
            .unwrap()
            .unwrap()
            .seq
    };
    assert!(run(home.path(), &raw, &behind, &mut None, &|| false).unwrap());
    assert_eq!(titles(), ["a.pdf"]);
    assert_eq!(at(), a);
    assert!(!run(home.path(), &raw, &behind, &mut None, &|| false).unwrap());
    let empty = tempfile::tempdir().unwrap();
    let none = crate::knowledge::open(empty.path()).unwrap();
    assert!(!run(home.path(), &raw, &none, &mut None, &|| false).unwrap());
    let owed = drained(home.path(), &raw);
    let mut consumer = crate::consumer::subagents::Subagents;
    crate::worker::Consumer::rewind(&mut consumer, &owed, raw.device(), 0).unwrap();
    assert!(!run(home.path(), &raw, &owed, &mut None, &|| false).unwrap());
    assert_eq!((titles(), at()), (vec!["a.pdf".to_owned()], a));
    while step(home.path(), &raw) {}
    assert_eq!(titles(), ["a.pdf", "gap.pdf", "b.pdf"]);
    let db = open(home.path()).unwrap();
    set_checkpoint(&db, &raw, a, time_at(&raw, a).unwrap()).unwrap();
    while step(home.path(), &raw) {}
    assert_eq!(titles(), ["a.pdf", "gap.pdf", "b.pdf"]);
}

/// Slice 3: a stop's copy goes with a forget of its call or of its gap's reply, not with one
/// of the stop alone, and stays through retention that expires its call, its reply, its
/// launch and its stop.
#[test]
fn a_stops_copy_goes_with_its_call_or_reply_and_stays_through_retention() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    let cases = ["call", "reply", "stop", "expired"];
    let names: Vec<String> = cases.iter().map(|c| format!("{c}.pdf")).collect();
    files(w, &names.iter().map(String::as_str).collect::<Vec<_>>());
    let mut seqs = HashMap::new();
    for case in cases {
        said(&mut raw, case, "prompt", w);
        let launch = raw.append(&launch_of(case, case, true, w)).unwrap();
        let reply = said(&mut raw, case, "reply", w);
        let file = w.join(format!("{case}.pdf"));
        let call = raw.append(&sub_call(case, case, &file, w)).unwrap();
        let stop = raw.append(&stop_of(case, case, w)).unwrap();
        seqs.insert(case, (launch, reply, call, stop));
    }
    step(home.path(), &raw);
    assert_eq!(copies(home.path()).len(), 4);
    let (launch, reply, call, stop) = seqs["expired"];
    raw.expire_for_test(&[launch, reply, call, stop]);
    sweep(home.path(), &raw, true).unwrap();
    assert_eq!(copies(home.path()).len(), 4);
    let raw = forget_record(home.path(), raw, seqs["call"].2, true);
    let raw = forget_record(home.path(), raw, seqs["reply"].1, true);
    let raw = forget_record(home.path(), raw, seqs["stop"].3, true);
    sweep(home.path(), &raw, true).unwrap();
    let left = copies(home.path());
    let titles: Vec<&str> = left.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(titles, ["stop.pdf", "expired.pdf"]);
    for (_, copy, _) in &left {
        assert!(home.path().join(FOLDER).join(copy).is_file());
    }
}

/// Slice 3: a stop's kept seq and the phase's checkpoint are written together: when the
/// checkpoint's write fails neither is kept, so a restore that keeps the older checkpoint and
/// gives the stop's seqs to new records copies the new file.
#[test]
fn a_stops_seq_is_kept_with_the_checkpoint() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["old.pdf", "new.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    said(&mut raw, "s1", "reply", w);
    step(home.path(), &raw);
    let write = raw
        .append(&sub_call("s1", "a1", &w.join("old.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    let db = open(home.path()).unwrap();
    db.execute_batch(
        "CREATE TRIGGER interrupted BEFORE UPDATE ON deliverable_scan
             BEGIN SELECT RAISE(ABORT, 'interrupted'); END;",
    )
    .unwrap();
    let k = drained(home.path(), &raw);
    assert!(run(home.path(), &raw, &k, &mut None, &|| false).is_err());
    db.execute_batch("DROP TRIGGER interrupted").unwrap();
    let kept: i64 = (db.query_row("SELECT count(*) FROM subagent_scan", [], |r| r.get(0))).unwrap();
    assert_eq!(kept, 0);
    let mut raw = rebuilt_below(home.path(), raw, write);
    drop(drained(home.path(), &raw));
    let new = raw
        .append(&sub_call("s1", "a1", &w.join("new.pdf"), w))
        .unwrap();
    assert_eq!(new, write, "the seq is given again");
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert!(titles.contains(&"new.pdf".to_owned()), "{titles:?}");
}

/// Slice 3: a stop stepped aside between its files is taken whole next round, no row twice;
/// a long run of calls is read with an eye on a step aside too, the stop then waiting for the
/// next round below its checkpoint.
#[test]
fn a_stop_stepped_aside_is_taken_again_whole() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["one.pdf", "two.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    said(&mut raw, "s1", "reply", w);
    for f in ["one.pdf", "two.pdf"] {
        raw.append(&sub_call("s1", "a1", &w.join(f), w)).unwrap();
    }
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    // Two git checks, then a look before each file: the second file is not reached.
    let asked = std::cell::Cell::new(0);
    let aside = || {
        asked.set(asked.get() + 1);
        asked.get() > 3
    };
    let k = drained(home.path(), &raw);
    assert!(run(home.path(), &raw, &k, &mut None, &aside).unwrap());
    assert_eq!(copies(home.path()).len(), 1);
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["one.pdf", "two.pdf"]);
    // Three hundred reads in a gap: asked to step aside, the stop is not taken.
    said(&mut raw, "s2", "prompt", w);
    raw.append(&launch_of("s2", "b1", true, w)).unwrap();
    said(&mut raw, "s2", "reply", w);
    for _ in 0..300 {
        let mut read = sub_call("s2", "b1", &w.join("one.pdf"), w);
        read.body = read.body.replace("\"Write\"", "\"Read\"");
        raw.append(&read).unwrap();
    }
    let stop = raw.append(&stop_of("s2", "b1", w)).unwrap();
    let k = drained(home.path(), &raw);
    // The phase reads `BATCH` records a round: the stop is in the second. Asked at once, the
    // labels' pass steps aside; asked from the reading of the calls on, that pass does.
    for asked_from in [1, 2] {
        let asked = std::cell::Cell::new(0);
        let aside = || {
            asked.set(asked.get() + 1);
            asked.get() >= asked_from
        };
        for _ in 0..3 {
            assert!(run(home.path(), &raw, &k, &mut None, &aside).unwrap());
        }
    }
    let at = || {
        checkpoint(&open(home.path()).unwrap(), raw.device())
            .unwrap()
            .unwrap()
            .seq
    };
    assert!(at() < stop);
    while step(home.path(), &raw) {}
    assert_eq!(at(), stop);
}

/// Slice 3: a stop's row is denied from the registration of a forget of its gap's reply, as of
/// its call, before any purge; a forget of the stop or of the launch alone denies none, the
/// copy having been made from a public launch's calls.
#[test]
fn a_stops_row_is_denied_by_its_reply_not_by_its_stop_or_launch() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    let cases = ["reply", "stop", "launch"];
    files(w, &["reply.pdf", "stop.pdf", "launch.pdf"]);
    let mut seqs = HashMap::new();
    for case in cases {
        said(&mut raw, case, "prompt", w);
        let launch = raw.append(&launch_of(case, case, true, w)).unwrap();
        let reply = said(&mut raw, case, "reply", w);
        let file = w.join(format!("{case}.pdf"));
        raw.append(&sub_call(case, case, &file, w)).unwrap();
        let stop = raw.append(&stop_of(case, case, w)).unwrap();
        seqs.insert(case, [reply, stop, launch]);
    }
    step(home.path(), &raw);
    let mut raw = raw;
    for (i, case) in cases.iter().enumerate() {
        raw = forget_record(home.path(), raw, seqs[case][i], false);
    }
    let db = open(home.path()).unwrap();
    let denied: Vec<(String, bool)> = (rows(&db).unwrap().into_iter())
        .map(|r| (r.title.clone(), raw.denies(&r).unwrap()))
        .collect();
    let expected: Vec<(String, bool)> = vec![
        ("reply.pdf".into(), true),
        ("stop.pdf".into(), false),
        ("launch.pdf".into(), false),
    ];
    assert_eq!(denied, expected);
}

/// Slice 3: a taken stop whose id a mask takes since still bounds the next stop, though a
/// source changed since.
#[test]
fn a_taken_stop_masked_since_still_bounds_the_next() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["one.pdf", "two.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    let reply = said(&mut raw, "s1", "reply", w);
    let one = raw
        .append(&sub_call("s1", "a1", &w.join("one.pdf"), w))
        .unwrap();
    let first = raw.append(&stop_of("s1", "a1", w)).unwrap();
    step(home.path(), &raw);
    let device = raw.device().to_owned();
    let body = match &raw.after(&device, first - 1, 1).unwrap()[0].item {
        Item::Event(e) => e.body.clone(),
        _ => panic!("an event"),
    };
    let at = body.find("\"a1\"").unwrap() + 1;
    let target = crate::raw::Target::Range {
        device,
        seq: first,
        offset: at as i64,
        length: 2,
    };
    raw.append_tombstone(target).unwrap();
    std::fs::write(w.join("one.pdf"), b"changed since").unwrap();
    let two = raw
        .append(&sub_call("s1", "a1", &w.join("two.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    while step(home.path(), &raw) {}
    let expected: Vec<(String, i64, i64)> = vec![
        ("one.pdf".into(), reply, one),
        ("two.pdf".into(), reply, two),
    ];
    assert_eq!(origins(home.path()), expected);
}

/// Slice 3: a forget of the launch registered between a stop's two copies leaves the first its
/// row and the second none: each row is written under the read fence, held from the launch's
/// second look to the row (a forget's registration cannot take the fence then).
#[test]
fn a_launch_forgotten_while_its_stops_files_are_copied_leaves_no_row_after() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["first.pdf", "second.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    let launch = raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    said(&mut raw, "s1", "reply", w);
    for f in ["first.pdf", "second.pdf"] {
        raw.append(&sub_call("s1", "a1", &w.join(f), w)).unwrap();
    }
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    let k = drained(home.path(), &raw);
    let (dir, device) = (home.path().to_owned(), raw.device().to_owned());
    let held = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = held.clone();
    let mut rows = 0;
    let act = move |stage| match stage {
        Stage::Fenced => {
            let taken =
                crate::dispatch::exclusive_within(&dir, std::time::Duration::from_millis(50));
            if taken.is_err_and(|e| crate::dispatch::busy(&e)) {
                seen.set(seen.get() + 1);
            }
        }
        Stage::Row => {
            rows += 1;
            if rows == 2 {
                let target = crate::forget::Target::Record {
                    device: device.clone(),
                    seq: launch,
                };
                let preview = crate::forget::preview(&dir, target).unwrap();
                crate::forget::start(&dir, &preview).unwrap();
            }
        }
        _ => {}
    };
    with_seam(act, || {
        run(home.path(), &raw, &k, &mut None, &|| false).unwrap();
    });
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["first.pdf"]);
    assert_eq!(held.get(), 1, "the fence was held at the first row");
}

/// Slice 3: a launch whose id a mask takes between a stop's copy and its row names the
/// subagent no more: the copy gets no row.
#[test]
fn a_launch_masked_while_its_stops_file_is_copied_leaves_no_row() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["late.pdf"]);
    said(&mut raw, "s1", "prompt", w);
    let launch = raw.append(&launch_of("s1", "a1", true, w)).unwrap();
    said(&mut raw, "s1", "reply", w);
    raw.append(&sub_call("s1", "a1", &w.join("late.pdf"), w))
        .unwrap();
    raw.append(&stop_of("s1", "a1", w)).unwrap();
    let device = raw.device().to_owned();
    let body = match &raw.after(&device, launch - 1, 1).unwrap()[0].item {
        Item::Event(e) => e.body.clone(),
        _ => panic!("an event"),
    };
    let at = body.find("\"agent_id\":\"a1\"").unwrap() + "\"agent_id\":\"".len();
    let k = drained(home.path(), &raw);
    let dir = home.path().to_owned();
    let mut once = true;
    let act = move |stage| {
        if stage == Stage::Row && std::mem::take(&mut once) {
            let target = crate::raw::Target::Range {
                device: device.clone(),
                seq: launch,
                offset: at as i64,
                length: 2,
            };
            let mut raw = crate::raw::open(&dir).unwrap();
            raw.append_tombstone(target).unwrap();
        }
    };
    with_seam(act, || {
        run(home.path(), &raw, &k, &mut None, &|| false).unwrap();
    });
    assert!(copies(home.path()).is_empty());
}

/// Slice 3: a record a restore brings back with no kind (a backup line that carried none)
/// ends a turn and a gap, as it may have been a reply: a turn after it copies none of the
/// turn before it again, and a call after it is no gap of an older reply. Beside them a gap
/// copies.
#[test]
fn a_restored_record_of_no_kind_ends_a_turn_and_a_gap() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(w, &["first.pdf", "second.pdf", "older.pdf", "control.pdf"]);
    let write = |s: &str, f: &str| tool(s, "Write", json!({"file_path": w.join(f)}), w);
    let stub = |raw: &Raw, seq: i64| {
        let db = Connection::open(home.path().join("raw.db")).unwrap();
        db.execute(
            "UPDATE records SET type = 'removed', source = 'restore', kind = NULL,
                   agent = NULL, session = NULL, repo = NULL, cwd = NULL, body = NULL,
                   enc = 'plain', original_bytes = NULL
                 WHERE device = ?1 AND seq = ?2",
            params![raw.device(), seq],
        )
        .unwrap();
    };
    said(&mut raw, "turns", "prompt", w);
    raw.append(&write("turns", "first.pdf")).unwrap();
    let first = said(&mut raw, "turns", "reply", w);
    step(home.path(), &raw);
    stub(&raw, first);
    said(&mut raw, "turns", "prompt", w);
    raw.append(&write("turns", "second.pdf")).unwrap();
    said(&mut raw, "turns", "reply", w);
    said(&mut raw, "older", "prompt", w);
    raw.append(&launch_of("older", "o1", true, w)).unwrap();
    said(&mut raw, "older", "reply", w);
    let newer = said(&mut raw, "older", "reply", w);
    let file = w.join("older.pdf");
    raw.append(&sub_call("older", "o1", &file, w)).unwrap();
    raw.append(&stop_of("older", "o1", w)).unwrap();
    said(&mut raw, "control", "prompt", w);
    raw.append(&launch_of("control", "c1", true, w)).unwrap();
    said(&mut raw, "control", "reply", w);
    let file = w.join("control.pdf");
    raw.append(&sub_call("control", "c1", &file, w)).unwrap();
    raw.append(&stop_of("control", "c1", w)).unwrap();
    stub(&raw, newer);
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["first.pdf", "second.pdf", "control.pdf"]);
}

/// Slice 3: a purged reply ends its turn and its gap, its session gone: a turn after it copies
/// none of the forgotten turn's files again; a call after two replies whose newer one is
/// purged is no gap of the older one; a subagent launched after a reply since purged is
/// launched from no prompt's turn still. Beside them a gap copies.
#[test]
fn a_purged_reply_still_ends_its_turn_and_gap() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    files(
        w,
        &[
            "first.pdf",
            "second.pdf",
            "older.pdf",
            "nested.pdf",
            "control.pdf",
        ],
    );
    let write = |s: &str, f: &str| tool(s, "Write", json!({"file_path": w.join(f)}), w);
    // A turn whose reply is forgotten, then the next turn.
    said(&mut raw, "turns", "prompt", w);
    raw.append(&write("turns", "first.pdf")).unwrap();
    let first = said(&mut raw, "turns", "reply", w);
    step(home.path(), &raw);
    let mut raw = forget_record(home.path(), raw, first, true);
    assert!(copies(home.path()).is_empty());
    said(&mut raw, "turns", "prompt", w);
    raw.append(&write("turns", "second.pdf")).unwrap();
    said(&mut raw, "turns", "reply", w);
    // Two replies, the newer purged.
    said(&mut raw, "older", "prompt", w);
    raw.append(&launch_of("older", "o1", true, w)).unwrap();
    said(&mut raw, "older", "reply", w);
    let newer = said(&mut raw, "older", "reply", w);
    let file = w.join("older.pdf");
    raw.append(&sub_call("older", "o1", &file, w)).unwrap();
    raw.append(&stop_of("older", "o1", w)).unwrap();
    // Launched after a reply, which is purged since.
    said(&mut raw, "nested", "prompt", w);
    let reply = said(&mut raw, "nested", "reply", w);
    raw.append(&launch_of("nested", "n1", true, w)).unwrap();
    said(&mut raw, "nested", "prompt", w);
    said(&mut raw, "nested", "reply", w);
    let file = w.join("nested.pdf");
    raw.append(&sub_call("nested", "n1", &file, w)).unwrap();
    raw.append(&stop_of("nested", "n1", w)).unwrap();
    said(&mut raw, "control", "prompt", w);
    raw.append(&launch_of("control", "c1", true, w)).unwrap();
    said(&mut raw, "control", "reply", w);
    let file = w.join("control.pdf");
    raw.append(&sub_call("control", "c1", &file, w)).unwrap();
    raw.append(&stop_of("control", "c1", w)).unwrap();
    let raw = forget_record(home.path(), raw, newer, true);
    let raw = forget_record(home.path(), raw, reply, true);
    while step(home.path(), &raw) {}
    let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
    assert_eq!(titles, ["second.pdf", "control.pdf"]);
}

/// What is recorded while `[capture] deliverables` is off is not copied once it is on again, as
/// no history is copied when it is first turned on: the checkpoint follows the newest record while
/// it is off. It stayed where it was, and the next round copied the files of every turn made while
/// it was off.
#[test]
fn a_turn_made_while_deliverables_is_off_is_not_copied_once_it_is_on() {
    let (home, mut raw) = ready();
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::write(w.join("report.pdf"), b"%PDF made").unwrap();
    let write = [("Write", json!({"file_path": w.join("report.pdf")}))];
    let config = home.path().join("config.toml");
    std::fs::write(&config, "[capture]\ndeliverables = false\n").unwrap();
    turn(&mut raw, "s1", w, &write);
    assert!(!step(home.path(), &raw));
    std::fs::remove_file(&config).unwrap();
    assert!(
        !step(home.path(), &raw),
        "nothing of the turn made while it was off"
    );
    assert!(copies(home.path()).is_empty());
    turn(&mut raw, "s2", w, &write);
    assert!(step(home.path(), &raw));
    assert_eq!(copies(home.path()).len(), 1);
}
