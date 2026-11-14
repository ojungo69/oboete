use super::*;

fn lines(home: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(home.join("state").join(FILE)).unwrap_or_default();
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn what_a_hook_injects_takes_what_remains_of_its_agents_timeout() {
    // 5 s less the margin, from a start 1 s ago: 3 s remain. The bounds leave a slow machine
    // 2 s between this line and the last.
    let clock = Clock::started(Duration::from_secs(1), SLOW);
    assert_eq!(clock.by(), None);
    assert_eq!(clock.left(Duration::from_secs(60)), Duration::from_secs(60));
    assert!(!clock.over());
    clock.inject_within(Some(Duration::from_secs(5)));
    let left = clock.left(Duration::from_secs(60));
    assert!(left <= Duration::from_secs(3), "{left:?}");
    assert!(left > Duration::from_secs(1), "{left:?}");
    assert!(!clock.over());
    // A wait shorter than what remains is kept.
    assert_eq!(
        clock.left(Duration::from_millis(1)),
        Duration::from_millis(1)
    );
    let far = Instant::now() + Duration::from_secs(60);
    assert_eq!(clock.no_later(far), clock.by().unwrap());
    let near = Instant::now();
    assert_eq!(clock.no_later(near), near);
    // Past it, nothing remains.
    let clock = Clock::started(Duration::from_millis(4_100), SLOW);
    clock.inject_within(Some(Duration::from_secs(5)));
    assert!(clock.over());
    assert_eq!(clock.left(Duration::from_secs(1)), Duration::ZERO);
    // An agent with no timeout of oboete's keeps its waits.
    clock.inject_within(None);
    assert!(!clock.over());
    assert_eq!(clock.no_later(far), far);
}

/// Each phase is written under its own name: `NAMES` is in `Phase`'s order.
#[test]
fn each_phase_is_written_under_its_name() {
    let named = [
        (Phase::Parse, "parse"),
        (Phase::Lookup, "lookup"),
        (Phase::Note, "note"),
        (Phase::Settings, "settings"),
        (Phase::Open, "open"),
        (Phase::Record, "record"),
        (Phase::Fence, "fence"),
        (Phase::Manifest, "manifest"),
        (Phase::Prompt, "prompt"),
        (Phase::After, "after"),
        (Phase::Output, "output"),
    ];
    assert_eq!(named.len(), NAMES.len());
    for (phase, name) in named {
        let clock = Clock::starting_now();
        clock.begin(phase);
        let line = clock.line("claude", "Stop", "end");
        assert_eq!(line["phase"], name);
        assert!(line["began"][name].is_u64(), "{line}");
    }
}

#[test]
fn a_hook_inside_its_time_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let clock = Clock::starting_now();
    clock.begin(Phase::Record);
    clock.finish(home.path(), "claude", "Stop");
    assert!(!home.path().join("state").exists());
}

#[test]
fn a_slow_hook_is_written_while_it_runs_and_at_its_end() {
    let home = tempfile::tempdir().unwrap();
    let clock = Clock::started(Duration::ZERO, Duration::from_millis(60));
    clock.inject_within(Some(MARGIN + Duration::from_millis(100)));
    clock.begin(Phase::Open);
    clock.watch(home.path(), "claude", "UserPromptSubmit");
    // The thread's two lines come while the hook is still in `open`: one at `slow`, one half
    // a margin past the deadline.
    let until = Instant::now() + Duration::from_secs(10);
    while lines(home.path()).len() < 2 {
        assert!(Instant::now() < until, "{:?}", lines(home.path()));
        std::thread::sleep(Duration::from_millis(10));
    }
    clock.begin(Phase::Record);
    clock.finish(home.path(), "claude", "UserPromptSubmit");
    let got = lines(home.path());
    let at: Vec<&str> = got.iter().map(|l| l["at"].as_str().unwrap()).collect();
    assert_eq!(at, ["slow", "late", "end"]);
    for l in &got {
        assert_eq!(l["agent"], "claude");
        assert_eq!(l["event"], "UserPromptSubmit");
        assert_eq!(l["pid"], std::process::id());
        assert_eq!(l["ts"], got[0]["ts"]);
        assert_eq!(l["began"]["parse"], 0);
        assert!(l["began"]["open"].is_u64(), "{l}");
        assert_eq!(l["before_ms"], Value::Null);
    }
    assert_eq!(got[0]["phase"], "open");
    assert!(got[0]["ms"].as_u64().unwrap() >= 60);
    assert!(got[1]["ms"].as_u64().unwrap() >= 100 + 500);
    assert_eq!(got[0]["began"].get("record"), None);
    assert_eq!(got[2]["phase"], "record");
    assert!(got[2]["began"]["record"].as_u64().unwrap() >= 600);
}

#[test]
fn a_hook_past_slow_writes_its_end_with_no_thread_beside_it() {
    let home = tempfile::tempdir().unwrap();
    let clock = Clock::started(Duration::from_millis(80), Duration::from_millis(60));
    clock.finish(home.path(), "codex", "Stop");
    let got = lines(home.path());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["at"], "end");
    assert!(got[0]["ms"].as_u64().unwrap() >= 80);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = home.path().join("state").join(FILE);
        let mode = || std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(), 0o600);
        // A file others could read, from before: the next line makes it the person's alone.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        clock.finish(home.path(), "codex", "Stop");
        assert_eq!(mode(), 0o600);
    }
}

#[test]
fn the_record_takes_names_only_and_makes_no_home() {
    let home = tempfile::tempdir().unwrap();
    let clock = Clock::started(Duration::from_secs(3), SLOW);
    let line = clock.line("my agent/../x", &"E".repeat(40), "end");
    assert_eq!(line["agent"], "myagentx");
    assert_eq!(line["event"], "E".repeat(32));
    let keys: Vec<&str> = line
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "ts",
            "pid",
            "agent",
            "event",
            "at",
            "ms",
            "before_ms",
            "phase",
            "began"
        ]
    );
    // A home that is not there is not made.
    let gone = home.path().join("gone");
    clock.finish(&gone, "claude", "Stop");
    assert!(!gone.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn the_time_before_main_is_read_on_linux() {
    // A process made 123.45 s after boot (12,345 ticks at 100 a second), whose `main` began
    // 0.25 s ago, on a machine up for 200 s: 200 - 123.45 - 0.25 s passed before `main`. Its
    // name holds spaces and parentheses.
    let stat = "4242 (a (b) c) S 1 4242 4242 0 -1 4194304 100 0 0 0 1 2 0 0 20 0 1 0 12345 \
                    1000000 100 18446744073709551615";
    assert_eq!(
        before_from(stat, "200.00 150.00\n", 100.0, 0.25),
        Some(76_300)
    );
    // A start the two clocks' hundredths put after now is no time, not a negative one.
    assert_eq!(before_from(stat, "123.44 1.00\n", 100.0, 0.0), Some(0));
    // Text that is not theirs gives none.
    assert_eq!(before_from("4242 (a) S 1", "200.00", 100.0, 0.0), None);
    assert_eq!(before_from(stat, "", 100.0, 0.0), None);
    assert_eq!(before_from(stat, "200.00", 0.0, 0.0), None);
    // This process's own: some time.
    assert!(before_ms(Instant::now()).is_some());
    let clock = Clock::of_process();
    assert!(clock.line("claude", "Stop", "end")["before_ms"].is_u64());
}

#[test]
fn the_record_starts_again_at_its_cap() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let full = "x".repeat(usize::try_from(CAP).unwrap());
    std::fs::write(state.join(FILE), &full).unwrap();
    append(home.path(), &json!({"a": 1})).unwrap();
    assert_eq!(
        std::fs::read_to_string(state.join(FILE)).unwrap(),
        "\n{\"a\":1}\n"
    );
    assert_eq!(
        std::fs::read_to_string(state.join(format!("{FILE}.1"))).unwrap(),
        full
    );
    // Under the cap it is added to.
    append(home.path(), &json!({"a": 2})).unwrap();
    assert_eq!(
        std::fs::read_to_string(state.join(FILE)).unwrap(),
        "\n{\"a\":1}\n\n{\"a\":2}\n"
    );
}

/// Two hooks turn the full record over at once: the second finds no file, which is no error,
/// and its line is written. A turn-over that fails otherwise writes nothing past the cap.
#[test]
fn a_record_another_hook_turned_over_still_takes_the_line() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let (file, kept) = (state.join(FILE), state.join(format!("{FILE}.1")));
    // The file went between this hook's look at its size and its rename.
    turn_over(&file, &kept).unwrap();
    assert!(!kept.exists());
    // What is kept cannot be replaced: a folder that is not empty stands where it goes.
    std::fs::create_dir_all(kept.join("held")).unwrap();
    let full = "x".repeat(usize::try_from(CAP).unwrap());
    std::fs::write(&file, &full).unwrap();
    assert!(append(home.path(), &json!({"a": 1})).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), full);
}

/// A line cut short takes only itself with it: the next begins on a line of its own.
#[test]
fn a_line_cut_short_does_not_take_the_next_with_it() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join(FILE), "\n{\"ts\":1,\"pid\":2,\"at\":\"sl").unwrap();
    let now = 1_000_000;
    append(
        home.path(),
        &json!({"ts": now, "pid": 3, "at": "end", "ms": 2_000, "phase": "output"}),
    )
    .unwrap();
    let line = doctor_line(home.path(), now).unwrap();
    assert!(line.starts_with("slow hooks: 1 on record"), "{line}");
    assert!(line.contains(" 0 of them never ended"), "{line}");
}

#[test]
fn doctors_line_counts_the_slow_hooks_and_those_that_never_ended() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let day = 86_400_000_i64;
    let now = 100 * day;
    assert_eq!(doctor_line(home.path(), now), None);
    let line = |ts: i64, pid: u64, at: &str, phase: &str| {
        json!({"ts": ts, "pid": pid, "at": at, "ms": 2_000, "phase": phase,
                   "began": {"parse": 0, "open": 10, "record": 1_900}})
        .to_string()
    };
    // The turned-over file is read too. 8 days ago: not counted.
    std::fs::write(
        state.join(format!("{FILE}.1")),
        [
            line(now - 8 * day, 1, "slow", "open"),
            line(now - day, 2, "slow", "prompt"),
        ]
        .join("\n"),
    )
    .unwrap();
    std::fs::write(
        state.join(FILE),
        [
            // Hook 2 went on and ended; hook 3 was cancelled in the prompt point.
            line(now - day, 2, "end", "output"),
            line(now - 1_000, 3, "slow", "open"),
            line(now - 1_000, 3, "late", "prompt"),
            // Hooks 4 and 5 ended with no line before them: `open` took 1,890 of their
            // 2,000 ms. Two in `open` and two in `prompt`: the earlier phase is named.
            line(now - 500, 4, "end", "output"),
            line(now - 400, 5, "end", "output"),
            "not json".to_owned(),
        ]
        .join("\n"),
    )
    .unwrap();
    assert_eq!(
        doctor_line(home.path(), now).as_deref(),
        Some(
            "slow hooks: 4 on record in the last 7 days still ran at 2 s, 1 of them never ended (cancelled or killed); most often in: open"
        )
    );
}

#[test]
fn an_ends_longest_phase_is_named() {
    let v = json!({"ms": 3_000, "began": {"parse": 0, "settings": 5, "open": 10, "record": 2_500}});
    assert_eq!(longest(&v).as_deref(), Some("open"));
    // The last phase runs to the line's end.
    let v = json!({"ms": 9_000, "began": {"parse": 0, "open": 10, "record": 2_500}});
    assert_eq!(longest(&v).as_deref(), Some("record"));
    assert_eq!(longest(&json!({"ms": 1})), None);
}
