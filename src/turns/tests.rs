use super::*;
use crate::provider::ChainResult;
use crate::raw::{Event, Target, test_event};
use std::cell::RefCell;
use std::path::Path;

/// G09 of the 13.34.2 comparison: two turns' prompts are the same bytes up to the record, but
/// for the sentence that names its fence, so a provider's prefix cache can serve them.
#[test]
fn two_turns_prompts_share_their_instructions() {
    let turn = |prompts: &str| Turn {
        agent: "claude".into(),
        session: "s".into(),
        repo: None,
        ts: 0,
        from: 1,
        through: 2,
        prompts: prompts.into(),
        reply: "Done.".into(),
        cards: Vec::new(),
        read: Vec::new(),
        stretches: Vec::new(),
        goals: Vec::new(),
        removed: Vec::new(),
    };
    let a = prompt("English", &turn("Use tabs."));
    let b = prompt("English", &turn("Use spaces."));
    let same = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let language = a.find("Write every field").unwrap();
    assert!(
        same > language,
        "the prompts part at byte {same}: {}",
        &a[..same]
    );
    // What a Messages entry caches is that shared part.
    let cached = a.find(crate::curate::FENCE_SENTENCE).unwrap();
    assert!(language < cached && cached <= same, "{cached} {same}");
}

/// A record of `session` in repository `r`: a prompt, a tool output or a reply saying `text`.
fn said(session: &str, kind: &str, text: &str) -> Event {
    let body = match kind {
        "prompt" => json!({"prompt": text}),
        "reply" => json!({"assistant": text}),
        _ => json!({"tool": "Bash", "input": "make", "output": text, "failed": false}),
    };
    Event {
        kind: kind.into(),
        session: session.into(),
        repo: Some("r".into()),
        ts: 1_000,
        ..test_event(&body.to_string())
    }
}

/// A curated window op over `from` to `to`, whose card is `summary`, `goals` carried in.
fn window(from: i64, to: i64, summary: &str, goals: &[i64]) -> (OpKind, Value) {
    let op = json!({"outcome": "curated", "summary": summary, "from_seq": from,
            "from_offset": null, "to_seq": to, "to_offset": null, "elided": [], "goals": goals,
            "removed": []});
    (OpKind::Window, op)
}

/// A home with `records` appended and `windows` curated over them; the consumers run.
/// Codex on #410: two openers of a home whose notes index predates them rebuild it once. The
/// second waits for the first's write lock past its own busy timeout, through the stores'
/// initialization window, then finds the index built (a deferred transaction, or one that
/// waited only its busy timeout, failed there with "database is locked").
#[test]
fn a_second_opener_waits_for_the_notes_index_and_finds_it_built() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("knowledge.db");
    let first = Connection::open(&path).unwrap();
    first.execute_batch("PRAGMA journal_mode = WAL").unwrap();
    schema(&first).unwrap();
    first
        .execute_batch(
            "PRAGMA user_version = 0;
                 BEGIN IMMEDIATE;
                 INSERT INTO turns_fts(rowid, text) VALUES (999, 'built by the first');",
        )
        .unwrap();
    let second = Connection::open(&path).unwrap();
    second
        .busy_timeout(std::time::Duration::from_millis(100))
        .unwrap();
    let done = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        first
            .execute_batch("PRAGMA user_version = 1; COMMIT;")
            .unwrap();
    });
    schema(&second).unwrap();
    done.join().unwrap();
    let kept: i64 = second
        .query_row(
            "SELECT count(*) FROM turns_fts WHERE rowid = 999",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
}

fn home(records: &[Event], windows: &[(OpKind, Value)]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    for e in records {
        raw.append(e).unwrap();
    }
    raw.append_ops(windows).unwrap();
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    home
}

/// An answer that completed something.
fn completed(text: &str) -> Value {
    json!({"skip": false, "request": "Build the parser", "investigated": "", "learned": "",
            "completed": text, "next_steps": "", "notes": ""})
}

/// The phase once, each call answered with `answer`, then the consumers: what it did, and
/// the prompts it sent.
fn run(home: &Path, answer: &Value) -> (Phase, Vec<String>) {
    run_with(home, answer, &Rules::default())
}

/// A rule the home's config does not hold: the rescan never removes what it masks.
fn otp() -> Rules {
    rule("otp=([0-9]{6})")
}

/// Rules of one `regex`, its first group the secret, that the home's config does not hold.
fn rule(regex: &str) -> Rules {
    Rules::new(
        &crate::config::parse_capture(Some(&format!(
            "[redaction]\nextra_rules = [{{ id = \"r\", regex = '{regex}', secret_group = 1 }}]\n"
        )))
        .unwrap()
        .redaction,
    )
    .unwrap()
}

/// A curated window op over `from` to `to` whose curator wrote `card`.
fn observed(from: i64, to: i64, card: Value) -> (OpKind, Value) {
    let (kind, mut op) = window(from, to, "", &[]);
    op["observations"] = json!([card]);
    (kind, op)
}

/// `run` under `rules`.
fn run_with(home: &Path, answer: &Value, rules: &Rules) -> (Phase, Vec<String>) {
    chain_with(home, std::slice::from_ref(answer), rules)
}

/// `run_with` for a chain whose entries answer in order: the first answer the turn's check
/// takes is the chain's, as `provider` takes only a caller-approved answer.
fn chain_with(home: &Path, answers: &[Value], rules: &Rules) -> (Phase, Vec<String>) {
    let mut raw = crate::raw::open(home).unwrap();
    let k = crate::knowledge::open(home).unwrap();
    let db = crate::providers_db::open(home).unwrap();
    let sent = RefCell::new(Vec::new());
    let mut summarizer = |_: &str,
                          p: &str,
                          check: &crate::provider::AnswerCheck,
                          _: &crate::provider::Gate|
     -> Result<ChainResult> {
        sent.borrow_mut().push(p.to_owned());
        let answer = (answers.iter().find(|a| check(a).is_none()))
            .unwrap_or_else(|| panic!("no entry's answer passes the check: {answers:?}"));
        Ok(ChainResult {
            provider: "fake".into(),
            output: answer.clone(),
            tier: 1,
        })
    };
    let summary = Summary::default();
    let phase = phase(&mut raw, &k, &db, rules, &summary, "chain", &mut summarizer).unwrap();
    drop((raw, k, db));
    crate::worker::run_once(home).unwrap();
    (phase, sent.into_inner())
}

fn turn_ops(home: &Path) -> Vec<TurnOp> {
    let raw = crate::raw::open(home).unwrap();
    raw.ops_after(raw.device(), 0, 100)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == OpKind::Turn)
        .map(|o| serde_json::from_value(o.body).unwrap())
        .collect()
}

fn shown(home: &Path, rules: &Rules) -> Vec<TurnSummary> {
    let raw = crate::raw::open(home).unwrap();
    let k = crate::knowledge::open(home).unwrap();
    recent(&k, &raw, "r", 10, rules).unwrap()
}

/// Owner decision 43 (Codex's security review of the switch): a turn read while redaction was
/// off is not sent once it is on again before the call: the gate finds other rules, nothing
/// goes out, and no summary is kept.
#[test]
fn a_turn_read_under_other_rules_is_not_sent() {
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    // As `home` makes it, recorded and read while redaction is off.
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    std::fs::write(&config, "[redaction]\nenabled = false\n").unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    raw.append(&said("s1", "prompt", &format!("Deploy with {key}.")))
        .unwrap();
    raw.append(&said("s1", "reply", "Deployed.")).unwrap();
    raw.append_ops(&[window(1, 2, "Deployed.", &[])]).unwrap();
    crate::worker::run_once(home.path()).unwrap();
    let off = Rules::load(home.path()).unwrap();
    let k = crate::knowledge::open(home.path()).unwrap();
    let db = crate::providers_db::open(home.path()).unwrap();
    let sent = std::cell::Cell::new(false);
    let mut summarizer = |_: &str,
                          p: &str,
                          _: &crate::provider::AnswerCheck,
                          gate: &crate::provider::Gate|
     -> Result<ChainResult> {
        assert!(p.contains(&key), "read under the rules off: {p}");
        std::fs::write(&config, "[redaction]\nenabled = true\n").unwrap();
        gate()?;
        sent.set(true);
        Ok(ChainResult {
            provider: "fake".into(),
            output: completed("Deployed."),
            tier: 1,
        })
    };
    let phase = phase(
        &mut raw,
        &k,
        &db,
        &off,
        &Summary::default(),
        "chain",
        &mut summarizer,
    )
    .unwrap();
    assert!(matches!(phase, Phase::Waiting { .. }));
    assert!(!sent.get());
    drop((raw, k, db));
    assert!(turn_ops(home.path()).is_empty());
}

/// docs/summaries.md test 1: a turn end curation has passed gets one summary, once; one it
/// has not passed gets none.
#[test]
fn a_turn_end_past_curation_gets_one_summary() {
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "tool", "ok"),
            said("s1", "reply", "The parser is built."),
            said("s1", "reply", "Not curated yet."),
        ],
        &[window(1, 3, "Built the parser.", &[])],
    );
    let (phase, sent) = run(home.path(), &completed("The parser reads a line."));
    assert_eq!((phase, sent.len()), (Phase::Covered, 1));
    let ops = turn_ops(home.path());
    assert_eq!(ops.len(), 1);
    assert_eq!((ops[0].from, ops[0].through, ops[0].skipped), (1, 3, false));
    assert_eq!(ops[0].fields["completed"], "The parser reads a line.");
    assert_eq!(run(home.path(), &completed("again")).1.len(), 0);
    let shown = shown(home.path(), &Rules::default());
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].fields["request"], "Build the parser");
}

/// docs/my-work-model.md A3: after a summary due, a My Work job due an hour or more goes
/// before a window, and any other job only when no window was sent.
#[test]
fn a_round_asks_for_a_my_work_job_due_an_hour_before_a_window_and_any_other_after() {
    // Records no window covers and no turn end: a window is due, no summary.
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "tool", "one"),
            said("s1", "tool", "two"),
        ],
        &[],
    );
    let asked = RefCell::new(Vec::new());
    // My Work's pass answers `covered` for the passes it covers: `true` the overdue one.
    let round_with = |covered: &[bool]| {
        let mut raw = crate::raw::open(home.path()).unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let db = crate::providers_db::open(home.path()).unwrap();
        let summary = Summary {
            curate: true,
            ..Summary::default()
        };
        let mut never = |_: &str,
                         _: &str,
                         _: &crate::provider::AnswerCheck,
                         _: &crate::provider::Gate|
         -> Result<ChainResult> { panic!("no summary is due") };
        let mut curator = |_: &str,
                           _: &str,
                           _: &crate::provider::AnswerCheck,
                           _: &crate::provider::Gate|
         -> Result<ChainResult> {
            asked.borrow_mut().push("window".to_owned());
            Ok(ChainResult {
                provider: "fake".into(),
                output: json!({"claims": [], "summary": "Stepped."}),
                tier: 1,
            })
        };
        let mut my_work = |_: &mut Raw, _: &Connection, _: &Connection, overdue: bool| {
            asked
                .borrow_mut()
                .push(format!("my work, overdue {overdue}"));
            Ok(if covered.contains(&overdue) {
                Phase::Covered
            } else {
                Phase::Idle
            })
        };
        let rules = Rules::default();
        let (s, c) = (&mut never, &mut curator);
        let phase = round(&mut raw, &k, &db, &rules, &summary, "", s, c, &mut my_work);
        drop((raw, k, db));
        crate::worker::run_once(home.path()).unwrap();
        (phase.unwrap(), asked.take())
    };
    let overdue = round_with(&[true]);
    assert_eq!(
        overdue,
        (Phase::Covered, vec!["my work, overdue true".to_owned()])
    );
    let window = round_with(&[false]);
    assert_eq!(
        window,
        (
            Phase::Covered,
            vec!["my work, overdue true".to_owned(), "window".to_owned()]
        )
    );
    // The window is curated: the job not due an hour goes now.
    let after = round_with(&[false]);
    assert_eq!(
        after,
        (
            Phase::Covered,
            vec![
                "my work, overdue true".to_owned(),
                "my work, overdue false".to_owned()
            ]
        )
    );
}

/// Decision 56 (T1): a run asks for the oldest summary due first, and for a window only when
/// it sent none, so windows that go a step at a time while the work goes on hold no summary
/// back.
#[test]
fn a_run_asks_for_a_summary_due_before_the_next_window() {
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "The parser is built."),
            said("s1", "tool", "The next step."),
        ],
        &[window(1, 2, "Built the parser.", &[])],
    );
    let asked = RefCell::new(Vec::new());
    let once = || {
        let mut raw = crate::raw::open(home.path()).unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let db = crate::providers_db::open(home.path()).unwrap();
        let summary = Summary {
            curate: true,
            ..Summary::default()
        };
        let answer = |role: &'static str, output: Value| {
            let asked = &asked;
            move |_: &str,
                  _: &str,
                  _: &crate::provider::AnswerCheck,
                  _: &crate::provider::Gate|
                  -> Result<ChainResult> {
                asked.borrow_mut().push(role);
                Ok(ChainResult {
                    provider: "fake".into(),
                    output: output.clone(),
                    tier: 1,
                })
            }
        };
        let mut summarizer = answer("summary", completed("The parser reads a line."));
        let mut curator = answer("window", json!({"claims": [], "summary": "Stepped."}));
        let rules = Rules::default();
        let (s, c) = (&mut summarizer, &mut curator);
        let mut idle = |_: &mut Raw, _: &Connection, _: &Connection, _: bool| Ok(Phase::Idle);
        let phase = round(&mut raw, &k, &db, &rules, &summary, "", s, c, &mut idle).unwrap();
        drop((raw, k, db));
        crate::worker::run_once(home.path()).unwrap();
        phase
    };
    assert_eq!(once(), Phase::Covered);
    assert_eq!(*asked.borrow(), ["summary"]);
    assert_eq!(once(), Phase::Covered);
    assert_eq!(*asked.borrow(), ["summary", "window"]);
    assert_eq!(once(), Phase::Idle);
}

/// Codex on #637 (#640): a stretch with no turn end is read once by this process: the next look
/// starts where the last stopped, and reads only what came after.
#[test]
fn a_stretch_with_no_turn_end_is_read_once() {
    let home = home(
        &[
            said("s1", "prompt", "Build."),
            said("s1", "tool", "one"),
            said("s1", "tool", "two"),
        ],
        &[window(1, 3, "Stepping.", &[])],
    );
    let look = |home: &Path| {
        let mut raw = crate::raw::open(home).unwrap();
        let k = crate::knowledge::open(home).unwrap();
        let db = crate::providers_db::open(home).unwrap();
        let summary = Summary {
            curate: true,
            ..Summary::default()
        };
        let mut never = |_: &str,
                         _: &str,
                         _: &crate::provider::AnswerCheck,
                         _: &crate::provider::Gate|
         -> Result<ChainResult> { panic!("no turn ended") };
        SCANNED.take();
        let phase = phase(
            &mut raw,
            &k,
            &db,
            &Rules::default(),
            &summary,
            "",
            &mut never,
        );
        assert_eq!(phase.unwrap(), Phase::Idle);
        SCANNED.take()
    };
    assert_eq!(look(home.path()), [(0, 3)]);
    assert_eq!(look(home.path()), [(3, 3)]);
    let mut raw = crate::raw::open(home.path()).unwrap();
    for text in ["three", "four"] {
        raw.append(&said("s1", "tool", text)).unwrap();
    }
    raw.append_ops(&[window(4, 5, "Stepping on.", &[])])
        .unwrap();
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    assert_eq!(look(home.path()), [(3, 5)]);
    // #646: a restore puts another raw.db in place with the same device and records; this
    // process reads that stretch again rather than trust the old file's mark.
    let raw = crate::raw::open(home.path()).unwrap();
    let (device, lines) = (
        raw.device().to_owned(),
        raw.export_lines(0, usize::MAX).unwrap(),
    );
    let ops = raw.export_op_lines(0, usize::MAX).unwrap();
    drop(raw);
    let rebuilt = home.path().join("raw.db.rebuilt");
    let mut rebuild = crate::raw::Rebuild::new(&rebuilt, &device).unwrap();
    for (_, line) in &lines {
        rebuild.add(line).unwrap();
    }
    for (_, line) in &ops {
        rebuild.add_op(line).unwrap();
    }
    rebuild.finish().unwrap();
    for file in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(home.path().join(file));
    }
    std::fs::rename(&rebuilt, home.path().join("raw.db")).unwrap();
    assert_eq!(crate::raw::open(home.path()).unwrap().device(), device);
    assert_eq!(look(home.path()), [(0, 5)]);
    assert_eq!(look(home.path()), [(5, 5)]);
    // A mark holds for its device, the same last turn end asked, and no further than the
    // checkpoint.
    read_to("elsewhere", 10, 20);
    assert_eq!(read_through("elsewhere", 10, 30), Some(20));
    assert_eq!(read_through("elsewhere", 5, 30), None);
    assert_eq!(read_through("elsewhere", 10, 15), None);
    assert_eq!(read_through("nowhere", 10, 30), None);
}

/// Decision 56 (T1): a summary its providers hold keeps the run's wait when no window is due,
/// so the worker is up when it may be asked again.
#[test]
fn a_held_summary_keeps_the_runs_wait_when_no_window_is_due() {
    let home = home(
        &[
            said("s1", "prompt", "Build."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 2, "Built.", &[])],
    );
    let mut raw = crate::raw::open(home.path()).unwrap();
    let k = crate::knowledge::open(home.path()).unwrap();
    let db = crate::providers_db::open(home.path()).unwrap();
    let until = crate::db::now_ms() + 60_000;
    let mut failing = |_: &str,
                       _: &str,
                       _: &crate::provider::AnswerCheck,
                       _: &crate::provider::Gate|
     -> Result<ChainResult> {
        Err(
            crate::provider::ChainFailed(vec![crate::provider::Fallback {
                provider: "fake".into(),
                reason: "rate limited".into(),
                skip: crate::provider::Skip::Wait(until),
                answer: None,
            }])
            .into(),
        )
    };
    let mut curator = |_: &str,
                       _: &str,
                       _: &crate::provider::AnswerCheck,
                       _: &crate::provider::Gate|
     -> Result<ChainResult> { panic!("no window is due") };
    let (rules, summary) = (Rules::default(), Summary::default());
    let (s, c) = (&mut failing, &mut curator);
    let mut idle = |_: &mut Raw, _: &Connection, _: &Connection, _: bool| Ok(Phase::Idle);
    let phase = round(
        &mut raw, &k, &db, &rules, &summary, "chain", s, c, &mut idle,
    )
    .unwrap();
    assert_eq!(phase, Phase::Waiting { until, up: true });
}

/// Codex on C2 (T1): every turn end is reached, however many records follow it.
#[test]
fn a_turn_end_far_behind_the_newest_records_still_gets_its_summary() {
    let mut records = vec![
        said("s1", "prompt", "Build."),
        said("s1", "reply", "Built."),
    ];
    records.extend((0..2_001).map(|i| said("s1", "tool", &format!("step {i}"))));
    let home = home(
        &records,
        &[
            window(1, 2, "Built.", &[]),
            window(3, 2_003, "Stepped.", &[]),
        ],
    );
    assert_eq!(run(home.path(), &completed("Built.")).1.len(), 1);
    assert_eq!(turn_ops(home.path())[0].through, 2);
}

/// Codex on C2 (T5): a turn whose op could not be kept even as a skip (its labels pass the op
/// cap) is not asked for: nothing is paid for that cannot be kept.
#[test]
fn a_turn_whose_op_cannot_be_kept_is_not_asked_for() {
    let session = "s".repeat(70_000);
    let home = home(
        &[
            said(&session, "prompt", "Build."),
            said(&session, "reply", "Built."),
        ],
        &[window(1, 2, "Built.", &[])],
    );
    assert_eq!(run(home.path(), &completed("Built.")).1.len(), 0);
    assert!(turn_ops(home.path()).is_empty());
}

/// Codex on C2 (T2): a turn starts at its session's first event after the previous reply, so
/// a window of the earlier turn that reaches past that reply only over other records (here
/// two tombstones) shows none of its cards.
#[test]
fn a_turn_starts_at_its_sessions_first_event_after_the_previous_reply() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let device = raw.device().to_owned();
    raw.append(&said("s1", "prompt", "Old.")).unwrap();
    raw.append(&said("s1", "reply", "Old done.")).unwrap();
    for seq in [998, 999] {
        let device = device.clone();
        raw.append_tombstone(Target::Record { device, seq })
            .unwrap();
    }
    raw.append(&said("s1", "prompt", "New.")).unwrap();
    raw.append(&said("s1", "reply", "New done.")).unwrap();
    raw.append_ops(&[
        window(1, 4, "Old work.", &[]),
        window(5, 6, "New work.", &[]),
    ])
    .unwrap();
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    run(home.path(), &completed("Old."));
    let (_, sent) = run(home.path(), &completed("New."));
    assert!(sent[0].contains("New work."), "{}", sent[0]);
    assert!(!sent[0].contains("Old work."), "{}", sent[0]);
    assert_eq!(turn_ops(home.path())[1].from, 5);
}

/// Codex on #371: an import of the same session between a live prompt and its reply is no
/// turn boundary and no part of the turn: it starts at the live prompt, and its prompts and
/// its repository are the live records'.
#[test]
fn an_imported_reply_of_the_session_is_no_turn_boundary() {
    let imported = |kind: &str, text: &str| Event {
        source: "transcript".into(),
        repo: Some("q".into()),
        ..said("s1", kind, text)
    };
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            imported("prompt", "An old request."),
            imported("reply", "An old answer."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 4, "Built the parser.", &[])],
    );
    let (_, sent) = run(home.path(), &completed("Built."));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Build the parser."), "{}", sent[0]);
    assert!(!sent[0].contains("An old request."), "{}", sent[0]);
    let op = &turn_ops(home.path())[0];
    assert_eq!((op.from, op.repo.as_deref()), (1, Some("r")));
}

/// Codex on #371: a card of a window of imported records alone, labelled as the live session,
/// is no part of the live turn.
#[test]
fn an_imported_windows_card_is_no_part_of_a_live_turn() {
    let imported = |kind: &str, text: &str| Event {
        source: "transcript".into(),
        ..said("s1", kind, text)
    };
    let card = |title: &str| {
        json!({"type": "change", "title": title, "narrative": "", "facts": [],
                "concepts": [], "files_read": [], "files_modified": []})
    };
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            imported("prompt", "An old request."),
            imported("reply", "An old answer."),
            said("s1", "reply", "Built."),
        ],
        &[
            observed(1, 1, card("Live work")),
            observed(2, 3, card("Imported work")),
            window(4, 4, "Replied.", &[]),
        ],
    );
    let (_, sent) = run(home.path(), &completed("Built."));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Live work"), "{}", sent[0]);
    assert!(!sent[0].contains("Imported work"), "{}", sent[0]);
}

/// Codex on #371: a removal from an imported record of the same labels inside a live turn's
/// span does not hide the turn's summary.
#[test]
fn a_removal_from_an_imported_record_in_the_span_does_not_hide_it() {
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            Event {
                source: "transcript".into(),
                ..said("s1", "prompt", "An old request.")
            },
            said("s1", "reply", "Built."),
        ],
        &[
            window(1, 1, "Asked.", &[]),
            (
                OpKind::Window,
                json!({"outcome": "skipped", "reason": "imported:transcript",
                        "from_seq": 2, "from_offset": null, "to_seq": 2, "to_offset": null,
                        "elided": []}),
            ),
            window(3, 3, "Built the parser.", &[]),
        ],
    );
    run(home.path(), &completed("Built."));
    assert_eq!(shown(home.path(), &Rules::default()).len(), 1);
    let mut raw = crate::raw::open(home.path()).unwrap();
    let device = raw.device().to_owned();
    raw.append_tombstone(Target::Record { device, seq: 2 })
        .unwrap();
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    assert_eq!(shown(home.path(), &Rules::default()).len(), 1);
}

/// T1: a summary every provider fails waits as a window does, and the turns after it wait
/// with it, so none is passed over for good.
#[test]
fn a_held_turn_holds_the_turns_after_it() {
    let home = home(
        &[
            said("s1", "prompt", "One."),
            said("s1", "reply", "One done."),
            said("s1", "prompt", "Two."),
            said("s1", "reply", "Two done."),
        ],
        &[window(1, 2, "One.", &[]), window(3, 4, "Two.", &[])],
    );
    let mut raw = crate::raw::open(home.path()).unwrap();
    let k = crate::knowledge::open(home.path()).unwrap();
    let db = crate::providers_db::open(home.path()).unwrap();
    let until = crate::db::now_ms() + 60_000;
    let mut calls = 0;
    let mut failing = |_: &str,
                       _: &str,
                       _: &crate::provider::AnswerCheck,
                       _: &crate::provider::Gate|
     -> Result<ChainResult> {
        calls += 1;
        Err(
            crate::provider::ChainFailed(vec![crate::provider::Fallback {
                provider: "fake".into(),
                reason: "rate limited".into(),
                skip: crate::provider::Skip::Wait(until),
                answer: None,
            }])
            .into(),
        )
    };
    let (rules, summary) = (Rules::default(), Summary::default());
    let phase = phase(&mut raw, &k, &db, &rules, &summary, "chain", &mut failing).unwrap();
    assert_eq!(calls, 1);
    assert!(matches!(phase, Phase::Waiting { .. }), "{phase:?}");
    drop((raw, k, db));
    assert_eq!(run(home.path(), &completed("Two.")).1.len(), 0);
    assert!(turn_ops(home.path()).is_empty());
}

/// Test 2 (D13): a session in an excluded repository gets no call.
#[test]
fn an_excluded_sessions_turn_gets_no_call() {
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 2, "Built the parser.", &[])],
    );
    crate::raw::open(home.path())
        .unwrap()
        .exclude("r", false)
        .unwrap();
    assert_eq!(run(home.path(), &completed("x")).1.len(), 0);
    // Kept back as a skip of its labels alone, marked so an undo lets it out (Codex on #371).
    let ops = turn_ops(home.path());
    assert_eq!(ops.len(), 1);
    let op = &ops[0];
    assert!(op.excluded && op.skipped && op.fields.is_empty() && op.read.is_empty());
    assert_eq!((op.from, op.through, op.session.as_str()), (1, 2, "s1"));
}

/// Codex on #371: a turn every provider fails `ATTEMPTS` times is kept as a skip, as a window
/// given up is, and not asked again, though the request changes (a recuration's business).
#[test]
fn a_turn_given_up_is_kept_as_a_skip_and_not_asked_again() {
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 2, "Built the parser.", &[])],
    );
    let refusing = |chain: &str| {
        let mut raw = crate::raw::open(home.path()).unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let db = crate::providers_db::open(home.path()).unwrap();
        let mut calls = 0;
        let mut refuse = |_: &str,
                          _: &str,
                          _: &crate::provider::AnswerCheck,
                          _: &crate::provider::Gate|
         -> Result<ChainResult> {
            calls += 1;
            Err(
                crate::provider::ChainFailed(vec![crate::provider::Fallback {
                    provider: "fake".into(),
                    reason: "unanchored".into(),
                    skip: crate::provider::Skip::Refused,
                    answer: None,
                }])
                .into(),
            )
        };
        let (rules, summary) = (Rules::default(), Summary::default());
        phase(&mut raw, &k, &db, &rules, &summary, chain, &mut refuse).unwrap();
        drop((raw, k, db));
        crate::worker::run_once(home.path()).unwrap();
        calls
    };
    for _ in 0..crate::curate::ATTEMPTS {
        assert_eq!(refusing("chain"), 1);
    }
    let ops = turn_ops(home.path());
    assert_eq!(ops.len(), 1);
    assert!(ops[0].skipped && !ops[0].excluded && ops[0].through == 2);
    assert_eq!(refusing("another chain"), 0);
}

/// Codex on #371: a waiting row past what the store holds, as a raw.db restored from backups
/// that lack its newest records leaves in providers.db, does not hide the turns before it.
#[test]
fn a_waiting_row_past_the_store_does_not_hide_the_turns_before_it() {
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 2, "Built the parser.", &[])],
    );
    let device = crate::raw::open(home.path()).unwrap().device().to_owned();
    let db = crate::providers_db::open(home.path()).unwrap();
    for (seq, attempts) in [(90, 1), (99, crate::curate::ATTEMPTS)] {
        crate::providers_db::set_turn_pending(
            &db,
            &crate::providers_db::TurnPending {
                device: device.clone(),
                seq,
                agent: "claude".into(),
                session: "lost".into(),
                prompt: "p".into(),
                reason: "r".into(),
                hold: "time".into(),
                attempts,
                next_attempt_at: 0,
            },
        )
        .unwrap();
    }
    drop(db);
    let (_, sent) = run(home.path(), &completed("Built."));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Build the parser."), "{}", sent[0]);
}

/// Codex on #371: the turns an undo lets out, the oldest of each session the list no longer
/// names, oldest first: not one of a session kept back now, one asked for since it was kept
/// back, or another device's.
#[test]
fn the_turns_released_are_the_oldest_of_each_session_let_out() {
    let k = Connection::open_in_memory().unwrap();
    schema(&k).unwrap();
    for (op_seq, (device, through, agent, session, excluded)) in [
        ("a", 5, "claude", "kept", true),
        ("a", 3, "claude", "free", true),
        ("a", 9, "claude", "free", true),
        ("a", 1, "codex", "free", true),
        ("b", 2, "claude", "free", true),
        ("a", 3, "claude", "free", false),
    ]
    .into_iter()
    .enumerate()
    {
        k.execute(
            "INSERT INTO turns(device, op_seq, ts, agent, session, from_seq, through,
                   skipped, excluded) VALUES(?1, ?2, 0, ?3, ?4, ?5, ?5, 1, ?6)",
            params![device, op_seq as i64, agent, session, through, excluded],
        )
        .unwrap();
    }
    let got = released(&k, "a", |_, session| session == "kept").unwrap();
    assert_eq!(got, [1, 9]);
}

/// Codex on #371: a turn the exclusion list kept back is summarized after an undo, though a
/// later turn of another repository's session was summarized while it waited.
#[test]
fn a_turn_an_exclusion_kept_back_is_summarized_after_the_undo() {
    let other = |kind: &str, text: &str| Event {
        repo: Some("q".into()),
        ..said("s2", kind, text)
    };
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
            other("prompt", "Fix the lexer."),
            other("reply", "Fixed."),
        ],
        &[
            window(1, 2, "Built the parser.", &[]),
            window(3, 4, "Fixed the lexer.", &[]),
        ],
    );
    let exclude = |undo: bool| {
        crate::raw::open(home.path())
            .unwrap()
            .exclude("r", undo)
            .unwrap()
    };
    exclude(false);
    let (_, sent) = run(home.path(), &completed("Fixed."));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Fix the lexer."), "{}", sent[0]);
    assert_eq!(run(home.path(), &completed("x")).1.len(), 0);
    exclude(true);
    let (_, sent) = run(home.path(), &completed("Built."));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Build the parser."), "{}", sent[0]);
    assert_eq!(run(home.path(), &completed("x")).1.len(), 0);
}

/// Codex on #371: an upgraded home's digest rows, whose repository may read like a turn, do
/// not stand for a turn's hold.
#[test]
fn an_old_digest_row_named_like_a_turn_does_not_stand_for_its_hold() {
    let other = |kind: &str, text: &str| Event {
        repo: Some("q".into()),
        ..said("s2", kind, text)
    };
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
            other("prompt", "Fix the lexer."),
            other("reply", "Fixed."),
        ],
        &[
            window(1, 2, "Built the parser.", &[]),
            window(3, 4, "Fixed the lexer.", &[]),
        ],
    );
    let device = crate::raw::open(home.path()).unwrap().device().to_owned();
    let db = crate::providers_db::open(home.path()).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS digest_pending(device TEXT NOT NULL, agent TEXT NOT NULL,
               session TEXT NOT NULL, repo TEXT NOT NULL, prompt TEXT NOT NULL,
               reason TEXT NOT NULL, hold TEXT NOT NULL, attempts INTEGER NOT NULL,
               next_attempt_at INTEGER NOT NULL, PRIMARY KEY(device, agent, session, repo))",
    )
    .unwrap();
    db.execute(
        "INSERT INTO digest_pending VALUES(?1, 'claude', 'old', 'turn 2', 'p', 'r', 'r', 3, 0)",
        [&device],
    )
    .unwrap();
    let exclude = |undo: bool| {
        crate::raw::open(home.path())
            .unwrap()
            .exclude("r", undo)
            .unwrap()
    };
    exclude(false);
    assert_eq!(run(home.path(), &completed("Fixed.")).1.len(), 1);
    exclude(true);
    let (_, sent) = run(home.path(), &completed("Built."));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Build the parser."), "{}", sent[0]);
}

/// Codex on #371: the turn ends are read a page at a time, and a turn after a page of
/// turns the list keeps back is still reached.
#[test]
fn a_turn_after_a_page_of_kept_back_turns_is_reached() {
    let mut records = Vec::new();
    for _ in 0..PAGE + 2 {
        records.push(said("s1", "prompt", "Kept back."));
        records.push(said("s1", "reply", "Kept."));
    }
    records.push(Event {
        repo: Some("q".into()),
        ..said("s2", "prompt", "Fix the lexer.")
    });
    records.push(Event {
        repo: Some("q".into()),
        ..said("s2", "reply", "Fixed.")
    });
    let top = records.len() as i64;
    let home = home(&records, &[window(1, top, "All of it.", &[])]);
    crate::raw::open(home.path())
        .unwrap()
        .exclude("r", false)
        .unwrap();
    let (_, sent) = run(home.path(), &completed("Fixed."));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Fix the lexer."), "{}", sent[0]);
}

/// Codex on #371: a run does not read again the turns it kept back, so a repository kept back
/// long costs a run nothing: a run after the one that kept them back writes nothing.
#[test]
fn a_run_that_finds_only_held_turns_writes_nothing() {
    let home = home(
        &[
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
            said("s1", "prompt", "Test it."),
            said("s1", "reply", "Tested."),
        ],
        &[window(1, 4, "Built and tested the parser.", &[])],
    );
    crate::raw::open(home.path())
        .unwrap()
        .exclude("r", false)
        .unwrap();
    assert_eq!(run(home.path(), &completed("x")).1.len(), 0);
    let mut raw = crate::raw::open(home.path()).unwrap();
    let k = crate::knowledge::open(home.path()).unwrap();
    let db = crate::providers_db::open(home.path()).unwrap();
    let mut summarizer = |_: &str,
                          _: &str,
                          _: &crate::provider::AnswerCheck,
                          _: &crate::provider::Gate|
     -> Result<ChainResult> { unreachable!() };
    crate::crash::off();
    let summary = Summary::default();
    let rules = Rules::default();
    phase(
        &mut raw,
        &k,
        &db,
        &rules,
        &summary,
        "chain",
        &mut summarizer,
    )
    .unwrap();
    assert_eq!(crate::crash::count(), 0);
}

/// Test 3 (T2): the turn's prompts, its session's cards of the windows that hold the turn,
/// and the reply, gated; not another session's cards, nor an earlier turn's.
#[test]
fn the_prompt_holds_the_turn_and_its_sessions_cards_gated() {
    let home = home(
        &[
            said("s1", "prompt", "Old request."),
            said("s1", "reply", "Old reply."),
            said("s1", "prompt", "Build the parser with otp=654321."),
            said("s1", "tool", "ok"),
            said("s2", "prompt", "Other session."),
            said("s1", "reply", "The parser is built."),
        ],
        &[
            window(1, 2, "Old work.", &[]),
            window(3, 4, "Built the parser.", &[]),
            window(5, 5, "Other session's work.", &[]),
            window(6, 6, "Replied about the parser.", &[]),
        ],
    );
    // The earlier turn first, then this one.
    assert_eq!(run(home.path(), &completed("Old.")).1.len(), 1);
    let (_, sent) = run_with(home.path(), &completed("Built."), &otp());
    let p = &sent[0];
    assert!(p.contains("Build the parser with "), "{p}");
    assert!(!p.contains("654321"), "{p}");
    assert!(p.contains("Built the parser.") && p.contains("Replied about the parser."));
    assert!(p.contains("The parser is built."), "{p}");
    assert!(!p.contains("Old"), "{p}");
    assert!(!p.contains("Other session"), "{p}");
    assert_eq!(turn_ops(home.path())[1].read, vec![(6, 6), (3, 4)]);
}

/// Codex on C2: each prompt is gated alone, as a window gates each record: a rule anchored to
/// a whole prompt still holds when the turn has two.
#[test]
fn each_prompt_is_gated_alone() {
    let home = home(
        &[
            said("s1", "prompt", "otp=654321"),
            said("s1", "prompt", "Continue."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 3, "Built.", &[])],
    );
    let (_, sent) = run_with(home.path(), &completed("Built."), &rule("^otp=([0-9]{6})$"));
    assert!(sent[0].contains("Continue."), "{}", sent[0]);
    assert!(!sent[0].contains("654321"), "{}", sent[0]);
}

/// Codex on C2: a card's title, subtitle, narrative and facts are shown as they were written
/// and gated (K6), never joined into one line: a rule that matches only the joined text, as
/// `otp= ([0-9]{6})` matches `otp=` and a line break before the digits once joined, finds
/// nothing to show it.
#[test]
fn a_cards_text_is_shown_as_written_never_joined_into_one_line() {
    let card = json!({"type": "change", "title": "T otp=\n111111",
            "subtitle": "S otp=\n222222", "narrative": "N otp=\n333333",
            "facts": ["F otp=\n444444", "otp=555555"], "concepts": [], "files_read": [],
            "files_modified": []});
    let home = home(
        &[
            said("s1", "prompt", "Build."),
            said("s1", "reply", "Built."),
        ],
        &[observed(1, 2, card)],
    );
    let (_, sent) = run_with(home.path(), &completed("Built."), &rule("otp= ?([0-9]{6})"));
    for (written, joined) in [
        ("T otp=\n111111", "otp= 111111"),
        ("N otp=\n333333", "otp= 333333"),
    ] {
        assert!(
            sent[0].contains(written) && !sent[0].contains(joined),
            "{}",
            sent[0]
        );
    }
    assert!(!sent[0].contains("555555"), "{}", sent[0]);
}

/// Codex on C2: an answer whose op would pass the op log's cap is never lost to it, which
/// would ask again at every run: the removal list goes first, as a window op's past its bound,
/// and the summary is then hidden by any removal (K4).
#[test]
fn an_op_over_the_cap_is_kept_without_its_removal_list() {
    let mut records = vec![said("s1", "prompt", "Build.")];
    records.extend((0..400).map(|i| said("s1", "tool", &format!("step {i}"))));
    records.push(said("s1", "reply", "Built."));
    let home = home(&records, &[window(1, 402, "Built.", &[])]);
    let mut raw = crate::raw::open(home.path()).unwrap();
    let device = raw.device().to_owned();
    for seq in 2..=401 {
        let device = device.clone();
        raw.append_tombstone(Target::Record { device, seq })
            .unwrap();
    }
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    // Each control character is six bytes in JSON: about 62,000 of fields.
    let wide = |n: usize| "\u{1}".repeat(n);
    let answer = json!({"skip": false, "request": wide(300), "investigated": wide(2_000),
            "learned": wide(2_000), "completed": wide(2_000), "next_steps": wide(2_000),
            "notes": wide(2_000)});
    assert_eq!(run(home.path(), &answer).1.len(), 1);
    let ops = turn_ops(home.path());
    assert_eq!(ops.len(), 1);
    assert!(
        ops[0].removed.is_empty() && !ops[0].skipped,
        "{:?}",
        ops[0].removed.len()
    );
    assert_eq!(run(home.path(), &answer).1.len(), 0);
    assert!(shown(home.path(), &Rules::default()).is_empty());
}

/// The cap's last resort: fields that pass it alone go, and the turn is kept as a skip.
#[test]
fn an_op_whose_fields_pass_the_cap_is_kept_as_a_skip() {
    let op = TurnOp {
        agent: "claude".into(),
        session: "s".into(),
        repo: None,
        ts: 0,
        from: 1,
        through: 2,
        read: Vec::new(),
        stretches: Vec::new(),
        goals: Vec::new(),
        removed: Vec::new(),
        fields: BTreeMap::from([("notes".to_owned(), "x".repeat(70_000))]),
        skipped: false,
        excluded: false,
    };
    let v = fitted(op).unwrap().unwrap();
    assert_eq!((&v["skipped"], &v["fields"]), (&json!(true), &json!({})));
}

/// Codex on C2 (T2): the cards fit `window_tokens` beside the prompts and the reply, the
/// oldest going first, and the op lists only the windows whose cards were shown.
#[test]
fn the_oldest_cards_go_first_to_fit_the_window() {
    // About 1,760 estimated tokens a card: two fit the default 5,000, three do not.
    let long = |c: char| {
        json!({"type": "change", "title": "Work", "narrative": c.to_string().repeat(2_200),
                "facts": [], "concepts": [], "files_read": [], "files_modified": []})
    };
    let home = home(
        &[
            said("s1", "prompt", "Build."),
            said("s1", "tool", "a"),
            said("s1", "tool", "b"),
            said("s1", "reply", "Built."),
        ],
        &[
            observed(1, 1, long('一')),
            observed(2, 2, long('二')),
            observed(3, 4, long('三')),
        ],
    );
    let (_, sent) = run(home.path(), &completed("Built."));
    assert!(!sent[0].contains('一'), "the oldest card was shown");
    assert!(sent[0].contains('二') && sent[0].contains('三'));
    assert_eq!(turn_ops(home.path())[0].read, vec![(3, 4), (2, 2)]);
}

/// Test 4 (T4): an answer with none of its fields is refused, one with notes alone is kept
/// (#403); a skip is kept as an op with no fields, and the turn is not asked again.
#[test]
fn an_answer_with_none_of_its_fields_is_refused_and_a_skip_is_kept() {
    let rules = Rules::default();
    let none = json!({"skip": false, "request": " ", "investigated": "", "learned": "",
            "completed": "", "next_steps": "", "notes": ""});
    assert_eq!(check(&none, &rules), Some("empty"));
    let notes = json!({"skip": false, "request": " ", "investigated": "", "learned": "",
            "completed": "", "next_steps": "", "notes": "Only a note."});
    assert_eq!(check(&notes, &rules), None);
    assert_eq!(check(&json!("text"), &rules), Some("shape"));
    // An answer to another question, which a CLI whose output is the answer hands on whole.
    assert_eq!(check(&json!({"ok": true}), &rules), Some("empty"));
    let home = home(
        &[said("s1", "prompt", "ok?"), said("s1", "reply", "Yes.")],
        &[window(1, 2, "Asked.", &[])],
    );
    let skip = json!({"skip": true, "request": "", "investigated": "", "learned": "",
            "completed": "", "next_steps": "", "notes": ""});
    assert_eq!(run(home.path(), &skip).1.len(), 1);
    let ops = turn_ops(home.path());
    assert!(ops[0].skipped && ops[0].fields.is_empty());
    assert_eq!(run(home.path(), &skip).1.len(), 0);
    assert!(shown(home.path(), &rules).is_empty());
}

/// #402 (T4): a field the outbound gate empties (a closed private block) counts as empty: an
/// answer left with none is refused, and a public field beside it, a note too (#403), is kept
/// alone.
#[test]
fn a_field_the_gate_empties_counts_as_none() {
    let rules = Rules::default();
    let private = "<private>synthetic</private>";
    assert_eq!(crate::redact::outbound_with(private, &rules), "");
    let answer = |more: Value| {
        let mut v = json!({"skip": false, "request": private, "investigated": "",
                "learned": "", "completed": "", "next_steps": "", "notes": ""});
        v.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        v
    };
    assert!(kept(&answer(json!({})), &rules).is_empty());
    assert_eq!(check(&answer(json!({})), &rules), Some("empty"));
    let note = answer(json!({"notes": "A note."}));
    assert_eq!(check(&note, &rules), None);
    assert_eq!(kept(&note, &rules).keys().collect::<Vec<_>>(), ["notes"]);
    let public = answer(json!({"completed": "Built the parser."}));
    assert_eq!(check(&public, &rules), None);
    assert_eq!(
        kept(&public, &rules).keys().collect::<Vec<_>>(),
        ["completed"]
    );
}

/// #402: the chain's next entry answers a turn whose first entry's answer was private only, and
/// the turn's op holds its fields: no blank summary is kept.
#[test]
fn a_private_only_answer_leaves_the_turn_to_the_next_entry() {
    let home = home(
        &[said("s1", "prompt", "ok?"), said("s1", "reply", "Yes.")],
        &[window(1, 2, "Asked.", &[])],
    );
    let private = json!({"skip": false, "request": "<private>synthetic</private>",
            "investigated": "", "learned": "", "completed": "", "next_steps": "", "notes": ""});
    chain_with(
        home.path(),
        &[private, completed("Built.")],
        &Rules::default(),
    );
    let ops = turn_ops(home.path());
    assert_eq!(ops.len(), 1);
    assert!(!ops[0].skipped);
    assert_eq!(ops[0].fields["completed"], "Built.");
    assert!(ops[0].fields.values().all(|v| !v.trim().is_empty()));
}

/// Test 5 (T4): a field over its cap is dropped, never cut.
#[test]
fn a_field_over_its_cap_is_dropped_never_cut() {
    let answer = json!({"skip": false, "request": "x".repeat(301),
            "investigated": "y".repeat(2_001), "learned": "", "completed": "z".repeat(2_000),
            "next_steps": "", "notes": ""});
    let fields = kept(&answer, &Rules::default());
    assert_eq!(fields.keys().collect::<Vec<_>>(), ["completed"]);
    assert_eq!(fields["completed"].chars().count(), 2_000);
}

/// T7, decision 107 (docs/curator-conversation.md): a card was built on the stretches its
/// window read too (its op's `read`), so a summary shown the card read them: its op lists
/// their spans with the window's, names them as stretches (it read no card of theirs), and a
/// removal from one of their records hides it.
#[test]
fn a_summary_rests_on_the_stretches_its_cards_window_read() {
    let (kind, mut later) = window(3, 5, "Built it.", &[]);
    later["read"] = json!([[1, 2]]);
    let home = home(
        &[
            said("s0", "prompt", "Another session's."),
            said("s0", "reply", "Done."),
            said("s1", "prompt", "Earlier."),
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
        ],
        &[(kind, later)],
    );
    run(home.path(), &completed("Done."));
    run(home.path(), &completed("Built."));
    let ops = turn_ops(home.path());
    let op = ops.iter().find(|op| op.through == 5).unwrap();
    assert_eq!((op.from, &op.read), (3, &vec![(3, 5), (1, 2)]));
    assert_eq!(op.stretches, [(1, 2)]);
    // The other session's summary and this one's.
    assert_eq!(shown(home.path(), &Rules::default()).len(), 2);
    let mut raw = crate::raw::open(home.path()).unwrap();
    let device = raw.device().to_owned();
    raw.append_tombstone(Target::Record { device, seq: 2 })
        .unwrap();
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    assert_eq!(shown(home.path(), &Rules::default()).len(), 0);
}

/// Decision 107 (Codex on #974): a span is a stretch only when the turn was shown no card
/// of its window. A turn shown the cards of two windows, the later of which read the
/// earlier's stretch, names no stretch: forget's rule for a card read of a forgotten window
/// holds for both.
#[test]
fn a_span_whose_windows_card_was_shown_is_no_stretch() {
    let (kind, mut later) = window(5, 5, "Built it.", &[]);
    later["read"] = json!([[3, 4]]);
    let home = home(
        &[
            said("s0", "prompt", "Another session's."),
            said("s0", "reply", "Done."),
            said("s1", "prompt", "Plan the parser."),
            said("s1", "prompt", "Build the parser."),
            said("s1", "reply", "Built."),
        ],
        &[window(3, 4, "Planned it.", &[]), (kind, later)],
    );
    run(home.path(), &completed("Done."));
    run(home.path(), &completed("Built."));
    let ops = turn_ops(home.path());
    let op = ops.iter().find(|op| op.through == 5).unwrap();
    let mut read = op.read.clone();
    read.sort();
    assert_eq!((read, &op.stretches), (vec![(3, 4), (5, 5)], &vec![]));
}

/// Test 6 (T7): a removal from the turn's records, from a record of a window whose card it
/// read outside the turn, or from that window's goal hides the summary; one made before it
/// was asked, which its op lists, does not. The earlier turn (1 to 3) read both windows and
/// is hidden by each of them too.
#[test]
fn a_removal_from_what_a_summary_rests_on_hides_it() {
    for (target, before) in [(4, false), (3, false), (1, false), (4, true)] {
        // The window over the turn starts at the earlier turn's reply, and carried in the
        // session's goal, record 1.
        let home = home(
            &[
                said("s1", "prompt", "Goal."),
                said("s1", "tool", "ok"),
                said("s1", "reply", "Earlier."),
                said("s1", "prompt", "Build the parser."),
                said("s1", "reply", "Built."),
            ],
            &[
                window(1, 2, "Started.", &[]),
                window(3, 5, "Built it.", &[1]),
            ],
        );
        let tombstone = || {
            let mut raw = crate::raw::open(home.path()).unwrap();
            let device = raw.device().to_owned();
            raw.append_tombstone(Target::Record {
                device,
                seq: target,
            })
            .unwrap();
            drop(raw);
            crate::worker::run_once(home.path()).unwrap();
        };
        if before {
            tombstone();
        }
        run(home.path(), &completed("Started."));
        run(home.path(), &completed("Built."));
        let ops = turn_ops(home.path());
        assert_eq!((ops[1].from, ops[1].through), (4, 5));
        if !before {
            assert_eq!((&ops[1].read, &ops[1].goals), (&vec![(3, 5)], &vec![1]));
            assert_eq!(shown(home.path(), &Rules::default()).len(), 2);
            tombstone();
        }
        assert_eq!(
            shown(home.path(), &Rules::default()).len(),
            if before { 2 } else { 0 },
            "a removal of record {target}, before: {before}"
        );
    }
}

/// Codex on C2 (T7): the turn is its own session's records, so a removal from another
/// session's record inside its span does not hide its summary.
#[test]
fn a_removal_from_another_sessions_record_in_the_span_does_not_hide_it() {
    let home = home(
        &[
            said("a", "prompt", "Build."),
            said("b", "prompt", "Other."),
            said("b", "reply", "Other done."),
            said("a", "reply", "Built."),
        ],
        &[
            window(1, 1, "Asked.", &[]),
            window(2, 3, "Other.", &[]),
            window(4, 4, "Built.", &[]),
        ],
    );
    run(home.path(), &completed("Other."));
    run(home.path(), &completed("Built."));
    assert_eq!(shown(home.path(), &Rules::default()).len(), 2);
    let mut raw = crate::raw::open(home.path()).unwrap();
    let device = raw.device().to_owned();
    raw.append_tombstone(Target::Record { device, seq: 2 })
        .unwrap();
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    let shown = shown(home.path(), &Rules::default());
    assert_eq!(
        shown.iter().map(|s| s.session.as_str()).collect::<Vec<_>>(),
        ["a"]
    );
}

/// Codex on C2 (T5, K2): a turn's repository is the one its own records are in, none when
/// they are in two; another session's record between them does not count.
#[test]
fn a_turn_has_its_records_one_repository_or_none() {
    let in_q = |e: Event| Event {
        repo: Some("q".into()),
        ..e
    };
    let home = home(
        &[
            said("a", "prompt", "Build."),
            in_q(said("b", "prompt", "Other.")),
            said("a", "reply", "Built."),
            said("c", "prompt", "Move."),
            in_q(said("c", "tool", "ok")),
            said("c", "reply", "Moved."),
        ],
        &[window(1, 3, "Built.", &[]), window(4, 6, "Moved.", &[])],
    );
    run(home.path(), &completed("Built."));
    run(home.path(), &completed("Moved."));
    let ops = turn_ops(home.path());
    assert_eq!(
        ops.iter()
            .map(|o| (o.session.as_str(), o.repo.as_deref()))
            .collect::<Vec<_>>(),
        [("a", Some("r")), ("c", None)]
    );
}

/// Test 7 (T6): a rebuild makes the same rows from the op log.
#[test]
fn a_rebuild_gives_the_same_rows() {
    let home = home(
        &[
            said("s1", "prompt", "Build."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 2, "Built.", &[])],
    );
    run(home.path(), &completed("Built."));
    let rows = || {
        let k = crate::knowledge::open(home.path()).unwrap();
        let mut st = k
            .prepare("SELECT device, op_seq, through, fields, skipped FROM turns")
            .unwrap();
        st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
    };
    let before = rows();
    assert_eq!(before.len(), 1);
    crate::worker::rebuild(home.path()).unwrap();
    assert_eq!(rows(), before);
}

/// Test 8 (T8): every field is gated with the rules as they are when it is read.
#[test]
fn recent_gates_every_field_with_the_rules_now() {
    let home = home(
        &[
            said("s1", "prompt", "Build."),
            said("s1", "reply", "Built."),
        ],
        &[window(1, 2, "Built.", &[])],
    );
    run(home.path(), &completed("The code is otp=654321 now."));
    let shown = shown(home.path(), &otp());
    assert!(
        !shown[0].fields["completed"].contains("654321"),
        "{shown:?}"
    );
}
