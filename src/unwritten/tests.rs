use super::*;
use serde_json::json;

/// A write-back's count, the write-back having gone through.
fn written((n, back): (usize, Result<()>)) -> usize {
    back.unwrap();
    n
}

/// U3: the files a write-back commits before a later file's store error still count beside
/// the error (Greptile and Codex on #440).
#[test]
fn a_later_files_error_keeps_the_count_of_those_written() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    keep(h, &[prompt("first", 1)], "r").unwrap();
    keep(h, &[prompt("second", 2)], "r").unwrap();
    let mut raw = crate::raw::open(h).unwrap();
    FAILS.set(Some((1, rusqlite::ffi::SQLITE_FULL)));
    let (n, back) = write_back(h, &mut raw, &Default::default());
    assert_eq!(n, 1);
    assert!(back.is_err());
    assert_eq!(counts(h).unwrap(), (1, 0));
}

fn prompt(text: &str, ts: i64) -> Captured {
    let payload = json!({"session_id": "s", "cwd": "/w", "prompt": text});
    let settings = crate::capture::Settings::default();
    crate::capture::events("claude", "UserPromptSubmit", &payload, ts, &settings)
        .pop()
        .unwrap()
}

fn events(raw: &crate::raw::Raw) -> Vec<crate::raw::Event> {
    raw.after(raw.device(), 0, 100)
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.item {
            crate::raw::Item::Event(e) => Some(*e),
            _ => None,
        })
        .collect()
}

fn large_prompt(ts: i64) -> Captured {
    let mut c = prompt("kept", ts);
    // The body has two fields within capture's per-field cap, plus the prompt.
    c.event.body = json!({
        "prompt": "kept",
        "input": "x".repeat(64 << 10),
        "output": "y".repeat(64 << 10),
    })
    .to_string();
    c
}

/// #443, U3: a call commits one bounded part, and keeps its own events behind the rest.
#[test]
fn a_large_kept_file_is_written_in_parts_before_the_calls_own_events() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let settings = crate::capture::Settings::default();
    // Six events of just over half the 256 KiB budget: only one fits in each part.
    let large: Vec<_> = (1..=6).map(large_prompt).collect();
    keep(h, &large, settings.rules.version()).unwrap();
    let mut raw = crate::raw::open(h).unwrap();
    for call in 1..=6 {
        let mut failed = 0;
        let mut back = 0;
        let recorded = crate::hook::record(
            h,
            &mut raw,
            "claude",
            "UserPromptSubmit",
            &json!({"session_id": "s", "cwd": "/w", "prompt": "own"}),
            100 + call,
            &settings,
            &mut failed,
            &mut back,
        )
        .unwrap();
        assert_eq!(failed, 0);
        if call < 6 {
            assert_eq!(back, 1, "one part committed by call {call}");
            assert!(recorded.events.is_empty(), "own events overtook the rest");
            let times: Vec<_> = events(&raw).iter().map(|e| e.ts).collect();
            assert_eq!(times, (1..=call).collect::<Vec<_>>());
            assert_eq!(counts(h).unwrap(), (call as usize + 1, 0));
        }
    }
    assert_eq!(counts(h).unwrap(), (0, 0));
    let records = raw.after(raw.device(), 0, 100).unwrap();
    let seqs: Vec<_> = records.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, (1..=12).collect::<Vec<_>>());
    let times: Vec<_> = events(&raw).iter().map(|e| e.ts).collect();
    assert_eq!(times, [1, 2, 3, 4, 5, 6, 101, 102, 103, 104, 105, 106]);
}

/// #443: the byte budget is shared by files, and an indivisible event above it still advances.
#[test]
fn the_byte_budget_covers_all_files_and_admits_an_oversized_first_event() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let settings = crate::capture::Settings::default();
    for ts in 1..=3 {
        keep(h, &[large_prompt(ts)], "r").unwrap();
    }
    let mut raw = crate::raw::open(h).unwrap();
    for call in 1..=3 {
        assert_eq!(written(write_back(h, &mut raw, &settings)), 1);
        assert_eq!(counts(h).unwrap(), (3 - call, 0));
        assert_eq!(
            events(&raw).iter().map(|e| e.ts).collect::<Vec<_>>(),
            (1..=call as i64).collect::<Vec<_>>()
        );
    }
    let mut oversized = large_prompt(4);
    let mut body: serde_json::Value = serde_json::from_str(&oversized.event.body).unwrap();
    body["extra"] = json!("z".repeat(256 << 10));
    oversized.event.body = body.to_string();
    keep(h, &[oversized], "r").unwrap();
    keep(h, &[prompt("later", 5)], "r").unwrap();
    assert_eq!(written(write_back(h, &mut raw, &settings)), 1);
    assert_eq!(counts(h).unwrap(), (1, 0));
    assert_eq!(
        events(&raw).iter().map(|e| e.ts).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert_eq!(written(write_back(h, &mut raw, &settings)), 1);
    assert_eq!(
        events(&raw).iter().map(|e| e.ts).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
}

/// #443: rows and progress survive or roll back together, on either side of commit.
#[test]
fn a_killed_part_neither_duplicates_nor_loses_events() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let settings = crate::capture::Settings::default();
    keep(h, &(1..=3).map(large_prompt).collect::<Vec<_>>(), "r").unwrap();
    let file = files(&h.join(DIR)).unwrap().pop().unwrap();
    let original = std::fs::read(&file).unwrap();
    let mut raw = crate::raw::open(h).unwrap();
    // A stop between inserting the rows and recording progress must roll both back.
    // This also catches moving the rows' commit ahead of the progress statement.
    let conn = rusqlite::Connection::open(h.join("raw.db")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER stop_part BEFORE INSERT ON unwritten_progress
                 BEGIN SELECT RAISE(ABORT, 'stopped before progress'); END;",
    )
    .unwrap();
    let (n, back) = write_back(h, &mut raw, &settings);
    assert_eq!(n, 0);
    assert!(back.is_err());
    assert!(
        events(&raw).is_empty(),
        "part committed without its progress"
    );
    conn.execute_batch("DROP TRIGGER stop_part;").unwrap();
    drop(conn);
    // The existing commit seam kills the transaction with both rows and progress
    // staged. Neither may land.
    crate::crash::at(1);
    let (n, back) = write_back(h, &mut raw, &settings);
    crate::crash::off();
    assert_eq!(n, 0);
    assert!(back.is_err());
    drop(raw);
    let mut raw = crate::raw::open(h).unwrap();
    assert!(events(&raw).is_empty());
    // Stop on the other side of the same commit, with the whole file still present.
    STOP_AFTER_PART.set(true);
    let (n, back) = write_back(h, &mut raw, &settings);
    assert_eq!(n, 1);
    assert!(back.is_err());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    drop(raw);
    let mut raw = crate::raw::open(h).unwrap();
    assert_eq!(events(&raw).iter().map(|e| e.ts).collect::<Vec<_>>(), [1]);
    for (n, last) in [(2, false), (3, true)] {
        assert_eq!(written(write_back(h, &mut raw, &settings)), 1);
        assert_eq!(
            events(&raw).iter().map(|e| e.ts).collect::<Vec<_>>(),
            (1..=n).collect::<Vec<_>>()
        );
        assert_eq!(counts(h).unwrap(), (usize::from(!last), 0));
    }
    let records = raw.after(raw.device(), 0, 100).unwrap();
    assert_eq!(records.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(
        events(&raw).iter().map(|e| e.ts).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

/// Codex on #440: a spool that cannot be listed is an error, not an empty one: it may hide
/// kept files.
#[test]
fn a_spool_that_cannot_be_listed_is_an_error() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(DIR), "not a directory").unwrap();
    assert!(counts(home.path()).is_err());
}

/// U1: each file is named one past the newest, under the keep lock, so the files sort as they
/// were kept, past ten too (CI on #440) and across processes (Codex on #440); one kept after a
/// write-back sorts after what is left.
#[test]
fn kept_files_sort_in_the_order_they_were_kept() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    for n in 0..12 {
        keep(h, &[prompt(&format!("prompt {n:02}"), 1_000 + n)], "r").unwrap();
    }
    let held = |h: &Path| -> Vec<String> {
        files(&h.join(DIR))
            .unwrap()
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect()
    };
    for (n, body) in held(h).iter().enumerate() {
        assert!(body.contains(&format!("prompt {n:02}")), "{n}: {body}");
    }
    for p in files(&h.join(DIR)).unwrap().iter().take(11) {
        std::fs::remove_file(p).unwrap();
    }
    keep(h, &[prompt("later", 2_000)], "r").unwrap();
    let left = held(h);
    assert_eq!(left.len(), 2);
    assert!(left[0].contains("prompt 11") && left[1].contains("later"));
}

/// U5: a keep waits `KEEP_WAIT` at most for one that holds the lock, so the hook still sets its
/// marker before the agent's deadline (Codex on #440).
#[test]
fn a_keep_gives_up_on_a_keep_that_holds_the_lock() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("keep.lock"))
        .unwrap();
    held.lock().unwrap();
    let start = std::time::Instant::now();
    assert!(keep(home.path(), &[prompt("waits", 1_000)], "r").is_err());
    assert!(start.elapsed() >= KEEP_WAIT);
    assert_eq!(counts(home.path()).unwrap().0, 0);
}

/// U5: what `bad/` holds counts against the bound: oboete never deletes it (Codex on #440).
#[test]
fn what_bad_holds_counts_against_the_bound() {
    let home = tempfile::tempdir().unwrap();
    let bad = home.path().join(DIR).join("bad");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::File::create(bad.join("set-aside.json"))
        .unwrap()
        .set_len(BOUND)
        .unwrap();
    assert!(keep(home.path(), &[prompt("one more", 1_000)], "r").is_err());
    assert_eq!(counts(home.path()).unwrap().0, 0);
}

/// U5: a keep waits for the one in progress, so overlapping failed calls cannot each pass the
/// bound (Codex on #440).
#[test]
fn a_keep_waits_for_the_keep_in_progress() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_path_buf();
    let dir = h.join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("keep.lock"))
        .unwrap();
    held.lock().unwrap();
    let (done, kept) = std::sync::mpsc::channel();
    let p = h.clone();
    let t = std::thread::spawn(move || {
        keep(&p, &[prompt("waits", 1_000)], "r").unwrap();
        done.send(()).unwrap();
    });
    assert!(
        kept.recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "kept beside a keep in progress"
    );
    drop(held);
    kept.recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    t.join().unwrap();
    assert_eq!(counts(&h).unwrap().0, 1);
}

/// U1: a temporary file that a keep killed before its rename left is removed by the next keep,
/// which then takes its name (Codex on #440).
#[test]
fn a_temporary_file_a_killed_keep_left_gives_up_its_name() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    let left = dir.join(format!(".{:020}.tmp", 0));
    std::fs::write(&left, "half").unwrap();
    keep(home.path(), &[prompt("after the kill", 1_000)], "r").unwrap();
    assert!(!left.exists());
    assert_eq!(counts(home.path()).unwrap().0, 1);
}

/// U1, U4: names are unique across the files kept and those set aside, so a file set aside
/// after the first one left never replaces it (Codex on #440).
#[test]
fn a_file_set_aside_never_replaces_one_set_aside_before() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let mut raw = crate::raw::open(h).unwrap();
    for n in 0..2 {
        keep(h, &[prompt(&format!("kept {n}"), 1_000 + n)], "r").unwrap();
        let file = files(&h.join(DIR)).unwrap().pop().unwrap();
        std::fs::write(&file, format!("damaged {n}")).unwrap();
        assert_eq!(written(write_back(h, &mut raw, &Default::default())), 0);
    }
    let bad: Vec<String> = files(&h.join(DIR).join("bad"))
        .unwrap()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    assert_eq!(bad, ["damaged 0", "damaged 1"]);
}

/// U5: a move to `bad/` takes the keep lock, so a keep's look at the files cannot miss a file
/// on its way there (Codex on #440): while a keep holds the lock, the file stays.
#[test]
fn a_move_to_bad_waits_for_the_keep_lock() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let dir = h.join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{:020}.json", 0)), "not json").unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("keep.lock"))
        .unwrap();
    held.lock().unwrap();
    let mut raw = crate::raw::open(h).unwrap();
    assert!(write_back(h, &mut raw, &Default::default()).1.is_err());
    assert_eq!(counts(h).unwrap(), (1, 0));
    drop(held);
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 0);
    assert_eq!(counts(h).unwrap(), (0, 1));
}

/// U1, U3: a kept file holds the masked text, never the secret, and is the owner's alone; it
/// is written back in one go, before what comes after, and is gone then.
#[test]
fn kept_events_are_masked_and_written_back_in_order() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let secret = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
    let ruleset = crate::capture::Settings::default()
        .rules
        .version()
        .to_owned();
    // One built without a store names no session yet (U2).
    let mut nameless = prompt("second", 1_001);
    nameless.event.session = "unknown".into();
    let kept = [prompt(&format!("use {secret} here"), 1_000), nameless];
    keep(h, &kept, &ruleset).unwrap();
    let file = files(&h.join(DIR)).unwrap().pop().unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains(&secret));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
    let mut raw = crate::raw::open(h).unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 1);
    assert_eq!(counts(h).unwrap(), (0, 0));
    let later = prompt("third", 2_000);
    raw.append_with_ledger(&later.event, &later.ledger, &ruleset)
        .unwrap()
        .unwrap();
    let events = events(&raw);
    assert_eq!(events.len(), 3);
    assert!(!events[0].body.contains(&secret) && events[0].body.contains("use "));
    assert!(events[1].body.contains("second") && events[2].body.contains("third"));
    assert_eq!(events[1].session, format!("unknown-{}", raw.device()));
}

/// #454: a file a call left behind after its write-back committed (stopped before the
/// removal) is removed by the next write-back, not written again, so a record forgotten in
/// between does not come back; a new file under the same name, once the directory emptied, is
/// still written, and so is another file's.
#[test]
fn a_file_left_after_its_commit_is_removed_not_written_again() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    keep(h, &[prompt("kept once", 1)], "r").unwrap();
    let file = files(&h.join(DIR)).unwrap().pop().unwrap();
    let copy = std::fs::read(&file).unwrap();
    let mut raw = crate::raw::open(h).unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 1);
    // Forgotten between the commit and the file coming back.
    let target = crate::forget::Target::Record {
        device: raw.device().into(),
        seq: 1,
    };
    crate::forget::start(h, &crate::forget::preview(h, target).unwrap()).unwrap();
    std::fs::write(&file, &copy).unwrap();
    keep(h, &[prompt("another", 2)], "r").unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 1);
    assert_eq!(counts(h).unwrap(), (0, 0));
    let bodies: Vec<String> = events(&raw).into_iter().map(|e| e.body).collect();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("another"));
    // The names start again once the directory is empty: the same name, other events.
    keep(h, &[prompt("third", 3)], "r").unwrap();
    assert_eq!(files(&h.join(DIR)).unwrap().pop().unwrap(), file);
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 1);
    assert_eq!(events(&raw).len(), 2);
    // Two calls in one millisecond with one payload keep the same events: both are written.
    keep(h, &[prompt("twice", 4)], "r").unwrap();
    keep(h, &[prompt("twice", 4)], "r").unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 2);
    assert_eq!(events(&raw).len(), 4);
}

/// Greptile, Codex, CodeRabbit on #460: two files kept before the keep's id, of one payload in
/// one millisecond, hold the same bytes: both are written, neither taken for the other.
#[test]
fn two_files_kept_before_the_id_are_both_written() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    keep(h, &[prompt("twice", 4)], "r").unwrap();
    keep(h, &[prompt("twice", 4)], "r").unwrap();
    let kept = files(&h.join(DIR)).unwrap();
    assert_eq!(kept.len(), 2);
    for file in &kept {
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        assert!(v.as_object_mut().unwrap().remove("id").is_some());
        std::fs::write(file, v.to_string()).unwrap();
    }
    assert_eq!(
        std::fs::read(&kept[0]).unwrap(),
        std::fs::read(&kept[1]).unwrap()
    );
    let mut raw = crate::raw::open(h).unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 2);
    assert_eq!(events(&raw).len(), 2);
    assert_eq!(counts(h).unwrap(), (0, 0));
}

/// U3: one write-back at a time: while another call holds it, a call writes nothing back, and
/// the file waits for the next.
#[test]
fn one_write_back_at_a_time() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    keep(h, &[prompt("kept", 1)], "r").unwrap();
    let mut raw = crate::raw::open(h).unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(h.join(DIR).join("write-back.lock"))
        .unwrap();
    held.lock().unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 0);
    assert_eq!(counts(h).unwrap(), (1, 0));
    drop(held);
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 1);
    assert_eq!(counts(h).unwrap(), (0, 0));
}

/// U4: a file that cannot be read back goes to `bad/` and stays; U5: nothing is kept past the
/// bound.
#[test]
fn a_bad_file_goes_aside_and_nothing_is_kept_past_the_bound() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let dir = h.join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("1-1.json"), "not json").unwrap();
    let mut raw = crate::raw::open(h).unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 0);
    assert_eq!(counts(h).unwrap(), (0, 1));
    assert!(events(&raw).is_empty());
    // A store that fails the write keeps the file for a later call: it is no bad file.
    keep(h, &[prompt("kept", 5)], "r").unwrap();
    let conn = rusqlite::Connection::open(h.join("raw.db")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER refuse BEFORE INSERT ON records BEGIN SELECT RAISE(ABORT, 'test'); END;",
    )
    .unwrap();
    assert!(write_back(h, &mut raw, &Default::default()).1.is_err());
    assert_eq!(counts(h).unwrap(), (1, 1));
    conn.execute_batch("DROP TRIGGER refuse;").unwrap();
    assert_eq!(written(write_back(h, &mut raw, &Default::default())), 1);
    // A file that would take it past the bound is not kept either.
    std::fs::File::create(dir.join("2-1.json"))
        .unwrap()
        .set_len(BOUND - 10)
        .unwrap();
    let e = keep(h, &[prompt("one", 1)], "r").unwrap_err();
    assert!(e.to_string().contains("holds"), "{e}");
    assert_eq!(counts(h).unwrap(), (1, 1));
}
