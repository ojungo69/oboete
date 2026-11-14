    use super::*;
    use crate::consumer::subagents::Subagents;
    use crate::raw::{self, Event, OpKind, Target, test_event};
    use crate::worker;

    const MIN: i64 = 60_000;

    fn ev(session: &str, kind: &str, ts: i64, body: Value) -> Event {
        Event {
            session: session.into(),
            kind: kind.into(),
            ts,
            ..test_event(&body.to_string())
        }
    }

    fn step(session: &str, id: &str, ts: i64) -> Event {
        ev(session, "tool", ts, json!({"tool": "Read", "agent_id": id}))
    }

    fn stop(session: &str, id: &str, ts: i64) -> Event {
        ev(session, SUBAGENT_STOP, ts, json!({"agent_id": id}))
    }

    /// A launch record as capture keeps it: what it launched, and its input with the task given
    /// here as `task`.
    fn launch(session: &str, ts: i64, mut launched: Value) -> Event {
        let task = launched.as_object_mut().unwrap().remove("task");
        let input = json!({ "description": task }).to_string();
        ev(
            session,
            "tool",
            ts,
            json!({"tool": "Agent", "input": input, "launched": launched}),
        )
    }

    fn drain(home: &Path, store: &Raw) -> Connection {
        let mut consumers: Vec<Box<dyn worker::Consumer>> = vec![Box::new(Subagents)];
        let mut k = crate::knowledge::open(home).unwrap();
        worker::drain(store, &mut k, &mut consumers).unwrap();
        k
    }

    fn key(session: &str) -> Key {
        ("claude".to_owned(), session.to_owned())
    }

    fn states(found: &[Subagent], now: i64) -> Vec<(&str, &'static str, Option<i64>)> {
        (found.iter())
            .map(|s| (s.agent_id.as_str(), s.state(now), s.finished))
            .collect()
    }

    /// Slice 2 (docs/subagents.md): Finished with the time when a stop came at or after the last
    /// step (in the same millisecond too), a foreground launch that returned among the stops; Working again after a later step,
    /// within 15 minutes of it, then No recent step. A launch or a stop with no step makes no
    /// subagent; another conversation's and a Codex one's are not read. The type is the newest
    /// step's or stop's, none when no record names one; the task is the launch's.
    #[test]
    fn a_subagents_state_follows_its_steps_stops_and_launch() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let typed = ev(
            "s",
            "tool",
            MIN,
            json!({"tool": "Read", "agent_id": "a1", "agent_type": "Explore"}),
        );
        for e in [
            typed,
            stop("s", "a1", 2 * MIN),
            stop("s", "b2", MIN),
            step("s", "b2", 3 * MIN),
            step("s", "c3", 4 * MIN),
            launch(
                "s",
                5 * MIN,
                json!({"agent_id": "c3", "status": "completed", "async": false,
                    "task": "Find the parser"}),
            ),
            step("s", "d4", 4 * MIN),
            launch(
                "s",
                5 * MIN,
                json!({"agent_id": "d4", "status": "async_launched", "async": true}),
            ),
            launch(
                "s",
                5 * MIN,
                json!({"agent_id": "e5", "status": "completed", "async": false}),
            ),
            stop("s", "f6", 5 * MIN),
            step("s", "i9", 6 * MIN),
            stop("s", "i9", 6 * MIN),
            step("t", "g7", 5 * MIN),
            Event {
                agent: "codex".into(),
                ..step("s", "h8", 5 * MIN)
            },
        ] {
            store.append(&e).unwrap();
        }
        let k = drain(home.path(), &store);
        let codex = ("codex".to_owned(), "s".to_owned());
        let found = read(&store, &k, &[key("s"), codex], &Rules::default()).unwrap();
        assert_eq!(
            states(&found, 5 * MIN),
            [
                ("i9", "finished", Some(6 * MIN)),
                ("c3", "finished", Some(5 * MIN)),
                ("d4", "working", None),
                ("b2", "working", None),
                ("a1", "finished", Some(2 * MIN)),
            ]
        );
        let of = |id: &str| found.iter().find(|s| s.agent_id == id).unwrap();
        assert_eq!(of("a1").agent_type.as_deref(), Some("Explore"));
        assert_eq!(of("b2").agent_type, None);
        assert_eq!(of("c3").task.as_deref(), Some("Find the parser"));
        assert_eq!(of("d4").task, None);
        let b2 = of("b2");
        assert_eq!(b2.state(3 * MIN + RECENT_MS), "working");
        assert_eq!(b2.state(3 * MIN + RECENT_MS + 1), "quiet");
        assert!(
            read(&store, &k, &[key("u")], &Rules::default())
                .unwrap()
                .is_empty()
        );
    }

    /// Slice 2: at most `MAX_LISTED` subagents, those with the newest last steps.
    #[test]
    fn the_subagents_with_the_newest_steps_are_listed() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        for n in 0..=MAX_LISTED as i64 {
            let session = if n % 2 == 0 { "s" } else { "t" };
            store
                .append(&step(session, &format!("a{n}"), n * MIN))
                .unwrap();
        }
        let k = drain(home.path(), &store);
        let found = read(&store, &k, &[key("s"), key("t")], &Rules::default()).unwrap();
        assert_eq!(found.len(), MAX_LISTED);
        assert!(found.iter().all(|s| s.agent_id != "a0"), "{found:?}");
        assert_eq!(
            (found[0].conversation.1.as_str(), found[0].agent_id.as_str()),
            ("s", "a300")
        );
    }

    /// Slice 2, with the consumer held behind: a stop appended since its pass counts at once, a
    /// forget or a mask registered since hides what it took, a record expired since (its rebuild
    /// not run) is passed over, and a stop both among the rows and past the checkpoint counts once.
    /// With no checkpoint, there is nothing to show.
    #[test]
    fn the_records_past_the_checkpoint_count_at_once() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let empty = crate::knowledge::open(home.path()).unwrap();
        store.append(&step("s", "a1", MIN)).unwrap();
        assert!(
            read(&store, &empty, &[key("s")], &Rules::default())
                .unwrap()
                .is_empty()
        );
        drop(empty);
        let b = store.append(&step("s", "b2", MIN)).unwrap();
        let c = store.append(&step("s", "c3", MIN)).unwrap();
        let d = store.append(&step("s", "d4", MIN)).unwrap();
        let task = launch(
            "s",
            MIN,
            json!({"agent_id": "a1", "status": "async_launched", "async": true,
                "task": "Read the tests"}),
        );
        let l = store.append(&task).unwrap();
        let k = drain(home.path(), &store);
        let ids = |found: &[Subagent]| -> Vec<String> {
            found.iter().map(|s| s.agent_id.clone()).collect()
        };
        assert_eq!(
            ids(&read(&store, &k, &[key("s")], &Rules::default()).unwrap()),
            ["a1", "b2", "c3", "d4"]
        );
        let device = store.device().to_owned();
        let stopped = store.append(&stop("s", "a1", 2 * MIN)).unwrap();
        (store.append_tombstone(Target::Record {
            device: device.clone(),
            seq: b,
        }))
        .unwrap();
        let Item::Event(e) = store.after(&device, c - 1, 1).unwrap()[0].item.clone() else {
            panic!("an event")
        };
        (store.append_tombstone(Target::Range {
            device: device.clone(),
            seq: c,
            offset: e.body.find("c3").unwrap() as i64,
            length: 2,
        }))
        .unwrap();
        // The launch's task goes with its record.
        (store.append_tombstone(Target::Record {
            device: device.clone(),
            seq: l,
        }))
        .unwrap();
        store.expire_for_test(&[d]);
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        assert_eq!(states(&found, 2 * MIN), [("a1", "finished", Some(2 * MIN))]);
        assert_eq!(found[0].task, None);
        drop(k);
        // The consumer's rows hold the stop now; its checkpoint set back before it, the stop is
        // read from raw too.
        let k = drain(home.path(), &store);
        k.execute(
            "UPDATE checkpoints SET seq = ?1 WHERE consumer = ?2",
            params![stopped - 1, subagents::NAME],
        )
        .unwrap();
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        assert_eq!(states(&found, 2 * MIN), [("a1", "finished", Some(2 * MIN))]);
        k.execute(
            "DELETE FROM checkpoints WHERE consumer = ?1",
            [subagents::NAME],
        )
        .unwrap();
        assert!(
            read(&store, &k, &[key("s")], &Rules::default())
                .unwrap()
                .is_empty()
        );
    }

    /// Slice 2 (Codex on slice 2): a launch's task is read from its record's input as capture kept it
    /// whole (a closed block and a run of base64 taken out there), through the rules of now before
    /// its cut: a value a rule added since masks is masked where the cut splits it.
    #[test]
    fn a_launch_task_is_read_whole_through_the_rules_of_now_before_its_cut() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let captured = |id: &str, description: String| {
            let payload = json!({"session_id": "s", "tool_name": "Agent",
                "tool_input": {"description": description, "prompt": "p"},
                "tool_response": {"agentId": id, "status": "async_launched"}});
            (crate::capture::events("claude", "PostToolUse", &payload, MIN, &settings))
                .remove(0)
                .event
        };
        let secret = format!("{} teal-{}{}", "d".repeat(150), "1234", "5678");
        let encoded = format!("{} {}", "e".repeat(10), "aA0z".repeat(400));
        let private = format!("{} <private>zqx hidden</private> after", "f".repeat(150));
        for (id, description) in [("a1", secret), ("b2", encoded), ("c3", private)] {
            store.append(&captured(id, description)).unwrap();
            store.append(&step("s", id, 2 * MIN)).unwrap();
        }
        let k = drain(home.path(), &store);
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = 'teal', regex = 'teal-[0-9]{8}' }]\n",
        )
        .unwrap();
        let rules = Rules::load(home.path()).unwrap();
        let found = read(&store, &k, &[key("s")], &rules).unwrap();
        let task = |id: &str| -> String {
            let s = found.iter().find(|s| s.agent_id == id).unwrap();
            s.task.clone().unwrap_or_else(|| panic!("no task of {id}"))
        };
        assert!(!task("a1").contains("1234"), "{}", task("a1"));
        assert!(task("a1").starts_with(&"d".repeat(150)), "{}", task("a1"));
        assert!(!task("b2").contains("aA0zaA0z"), "{}", task("b2"));
        assert!(!task("c3").contains("zqx"), "{}", task("c3"));
        assert_eq!(task("c3").chars().count(), 156, "{}", task("c3"));
    }

    /// Slice 2 (Codex on slice 2): a launch's task falls back as claude-mem reads one, to the input's
    /// prompt (cut at 500) and to the response's description when the input does not parse; a task
    /// a build before this one kept in the launch is never read, so with neither text parsing there
    /// is none.
    #[test]
    fn a_launch_task_falls_back_as_claude_mem_reads_one() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let record = |id: &str, input: String, output: String| {
            ev(
                "s",
                "tool",
                MIN,
                json!({"tool": "Agent", "input": input, "output": output, "launched":
                    {"agent_id": id, "status": "async_launched", "async": true,
                     "task": "Kept task"}}),
            )
        };
        let prompt = json!({ "prompt": "p ".repeat(400) }).to_string();
        let described = json!({"description": "From the response"}).to_string();
        for (id, input, output) in [
            ("a1", prompt, String::new()),
            ("b2", "{cut".to_owned(), described),
            ("c3", "{cut".to_owned(), "{cut".to_owned()),
        ] {
            store.append(&record(id, input, output)).unwrap();
            store.append(&step("s", id, 2 * MIN)).unwrap();
        }
        let k = drain(home.path(), &store);
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        let task = |id: &str| (found.iter().find(|s| s.agent_id == id).unwrap().task).clone();
        assert_eq!(task("a1").map(|t| t.chars().count()), Some(500));
        assert_eq!(task("b2").as_deref(), Some("From the response"));
        assert_eq!(task("c3"), None);
    }

    /// Slice 2 (cubic on #850): Mark finished's folder is the one the subagent's newest step was
    /// recorded in, whichever device recorded it: a copied home's first device's too. One a
    /// tombstone took, or one no directory now, gives none.
    #[test]
    fn the_folder_is_the_newest_steps_of_any_device() {
        let from = tempfile::tempdir().unwrap();
        let place = tempfile::tempdir().unwrap();
        let folder = |name: &str| {
            let p = place.path().join(name);
            std::fs::create_dir_all(&p).unwrap();
            p.to_string_lossy().into_owned()
        };
        let (old, new) = (folder("old"), folder("new"));
        let mut store = raw::open(from.path()).unwrap();
        let at = |cwd: &str, ts: i64| Event {
            cwd: Some(cwd.to_owned()),
            ..step("s", "a1", ts)
        };
        store.append(&at(&old, MIN)).unwrap();
        store.append(&at(&new, 2 * MIN)).unwrap();
        let first = store.device().to_owned();
        drop(drain(from.path(), &store));
        drop(store);
        let to = tempfile::tempdir().unwrap();
        for f in ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"] {
            if from.path().join(f).exists() {
                std::fs::copy(from.path().join(f), to.path().join(f)).unwrap();
            }
        }
        let mut store = raw::open(to.path()).unwrap();
        let k = crate::knowledge::open(to.path()).unwrap();
        let rules = Rules::default();
        let found = read(&store, &k, &[key("s")], &rules).unwrap();
        assert_eq!(
            recorded_folder(&store, &found[0]).unwrap(),
            Some(new.clone())
        );
        std::fs::remove_dir(&new).unwrap();
        assert_eq!(recorded_folder(&store, &found[0]).unwrap(), None);
        std::fs::create_dir(&new).unwrap();
        (store.append_tombstone(Target::Record {
            device: first,
            seq: found[0].step_record.1,
        }))
        .unwrap();
        assert_eq!(recorded_folder(&store, &found[0]).unwrap(), None);
    }

    /// Slice 2 (Codex on slice 2): in a home copied from another device's, whose own device the
    /// consumer has no checkpoint of yet, a forget of the first device's record and a stop recorded
    /// there count at once.
    #[test]
    fn a_copied_homes_own_records_count_before_its_first_checkpoint() {
        let from = tempfile::tempdir().unwrap();
        let mut store = raw::open(from.path()).unwrap();
        let a = store.append(&step("s", "a1", MIN)).unwrap();
        store.append(&step("s", "b2", MIN)).unwrap();
        let first = store.device().to_owned();
        drop(drain(from.path(), &store));
        drop(store);
        let to = tempfile::tempdir().unwrap();
        for f in ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"] {
            if from.path().join(f).exists() {
                std::fs::copy(from.path().join(f), to.path().join(f)).unwrap();
            }
        }
        let mut store = raw::open(to.path()).unwrap();
        assert_ne!(store.device(), first);
        let k = crate::knowledge::open(to.path()).unwrap();
        (store.append_tombstone(Target::Record {
            device: first,
            seq: a,
        }))
        .unwrap();
        store.append(&stop("s", "b2", 2 * MIN)).unwrap();
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        assert_eq!(states(&found, 2 * MIN), [("b2", "finished", Some(2 * MIN))]);
    }

    /// Slice 2: a claude-mem `agent-lifecycle` row `oboete import claude-mem` brought in is its
    /// subagent's stop at its `stopped_at_epoch` (the platform `claude` when none); one before the
    /// last step does not finish it, and an agent's own write to a list of that name is no stop.
    #[test]
    fn an_imported_agent_lifecycle_row_is_its_subagents_stop() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        for id in ["a1", "b2", "c3"] {
            store.append(&step("s", id, 2 * MIN)).unwrap();
        }
        let row = |n: &str, fields: Value| {
            json!({"repo": "claude-mem:p", "list": crate::work_state::AGENT_LIFECYCLE,
                "fields": fields, "clock": 1, "at": 1, "source": "claude-mem:p",
                "source_id": n})
        };
        let stopped = |id: &str, at: i64| {
            json!({"task": format!("Agent {id} in claude session s"), "status": "done",
                "content_session_id": "s", "agent_id": id, "stopped_at_epoch": at,
                "stop_source": "subagent-stop-hook"})
        };
        let mut b2 = stopped("b2", MIN);
        b2["platform_source"] = json!("claude");
        // Another conversation's row of an id alike is no stop of this one's.
        let mut other = stopped("c3", 4 * MIN);
        other["content_session_id"] = json!("u");
        store
            .append_ops(&[
                (OpKind::WorkState, row("w1", stopped("a1", 3 * MIN))),
                (OpKind::WorkState, row("w2", b2)),
                (OpKind::WorkState, row("w3", other)),
            ])
            .unwrap();
        let fields = stopped("c3", 3 * MIN);
        store
            .work_state(
                "github.com/o/r",
                crate::work_state::AGENT_LIFECYCLE,
                fields.as_object().unwrap(),
                None,
            )
            .unwrap();
        let k = drain(home.path(), &store);
        assert_eq!(
            states(
                &read(&store, &k, &[key("s")], &Rules::default()).unwrap(),
                3 * MIN
            ),
            [
                ("a1", "finished", Some(3 * MIN)),
                ("b2", "working", None),
                ("c3", "working", None),
            ]
        );
    }
