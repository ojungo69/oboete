use super::*;

#[test]
fn w6d_inventory_uses_the_same_admitted_config_even_during_an_unobserved_aba() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, b"").unwrap();
    let modified = std::fs::metadata(&config).unwrap().modified().unwrap();
    let during = config.clone();
    BEFORE_INVENTORY.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(during, b"[broken").unwrap();
        }))
    });
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(&config, b"").unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(config)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(
        report["checks"]["source_stability"], "known",
        "fixture must restore the before/after witness"
    );
    assert_eq!(
        report["inventory"]["config"], "valid",
        "inventory independently consumed the transient config"
    );
    assert_eq!(report["complete"], true);
}

#[test]
fn w6d_negative_native_position_is_unknown_instead_of_a_known_count() {
    let root = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(root.path()).unwrap();
    raw.append(&crate::raw::test_event("invented count boundary"))
        .unwrap();
    drop(raw);
    assert!(
        doctor_report(root.path()).complete,
        "native positions start established"
    );
    Connection::open(root.path().join("raw.db"))
        .unwrap()
        .execute("UPDATE records SET seq=-1", [])
        .unwrap();
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(
        report["checks"]["raw"]["max_seq"],
        serde_json::json!({"state":"unavailable","value":null})
    );
    assert_eq!(report["complete"], false);
}

#[test]
fn w6d_nullable_call_duration_is_incomplete_but_an_empty_history_is_complete() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("config.toml"), b"providers=[]").unwrap();
    let db = crate::providers_db::open(root.path()).unwrap();
    assert!(
        doctor_report(root.path()).complete,
        "empty native ledger should establish its facts"
    );
    db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms) VALUES(1,'invented','curator','ok',-1)",[]).unwrap();
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(report["checks"]["providers"]["recent"]["state"], "known");
    assert_eq!(
        report["checks"]["providers"]["recent"]["calls"][0]["ms"],
        serde_json::Value::Null
    );
    assert_eq!(report["complete"], false);
    assert_eq!(report["unhealthy"], serde_json::json!([]));
}

#[test]
fn w6d_nullable_rewind_time_is_incomplete_but_no_rewinds_is_complete() {
    let root = tempfile::tempdir().unwrap();
    let db = Connection::open(root.path().join("knowledge.db")).unwrap();
    db.execute_batch("CREATE TABLE rewinds(ts); CREATE TABLE gaps(agent TEXT,transcript_turns INTEGER,raw_turns INTEGER);").unwrap();
    assert!(
        doctor_report(root.path()).complete,
        "empty rewind history is an established fact"
    );
    db.execute("INSERT INTO rewinds VALUES(NULL)", []).unwrap();
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(
        report["checks"]["knowledge"]["rewinds"]["count"]["value"],
        1
    );
    assert_eq!(
        report["checks"]["knowledge"]["rewinds"]["last_at_ms"],
        serde_json::Value::Null
    );
    assert_eq!(report["complete"], false);
}

#[test]
fn w6d_corrected_config_drops_the_original_invalid_warning() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    std::fs::write(&path, b"[broken").unwrap();
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(path, b"").unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(report["checks"]["source_stability"], "changed");
    assert_eq!(report["complete"], false);
    assert!(
        !report["unhealthy"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("config_invalid"))
    );
    assert_eq!(report["checks"]["disk"]["state"], "known");
}

#[test]
fn w6d_complete_reports_established_absence_and_fixed_known_problems() {
    let root = tempfile::tempdir().unwrap();
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(report["complete"], true);
    assert_eq!(report["checks"]["remaining"], "none");
    assert_eq!(report["unhealthy"], serde_json::json!([]));
    std::fs::write(root.path().join("raw.db"), b"invented damaged store").unwrap();
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(report["complete"], false);
    assert_eq!(report["unhealthy"], serde_json::json!(["raw_damaged"]));
    std::fs::write(root.path().join("config.toml"), b"[broken").unwrap();
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(report["complete"], false);
    assert!(
        report["unhealthy"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("config_invalid"))
    );
    assert_eq!(report["checks"]["remaining"], "none");
}

#[test]
fn w6d_unknown_runtime_leaf_is_incomplete_without_inventing_a_failure() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(state.join("worker-outcome"), b"not a native outcome").unwrap();
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    assert_eq!(report["checks"]["runtime"]["state"], "known");
    assert_eq!(
        report["checks"]["runtime"]["worker"]["last"]["state"],
        "invalid"
    );
    assert_eq!(report["complete"], false);
    assert_eq!(report["unhealthy"], serde_json::json!([]));
}

#[test]
fn w6d_retained_metadata_counts_evaluation_copies_and_nothing_else() {
    let root = tempfile::tempdir().unwrap();
    let empty = serde_json::to_value(doctor_report(root.path())).unwrap();
    let retained = &empty["checks"]["retained"];
    assert_eq!(retained["state"], "known");
    assert_eq!(retained["evaluation_copies"]["present"]["value"], false);
    assert_eq!(retained["evaluation_copies"]["bytes"]["value"], 0);
    // v1's old files are no longer a category of their own (decision 99, 2026-11-12).
    for (name, size) in [("memory.db", 3), ("pre-snapshot.db", 6)] {
        std::fs::write(root.path().join(name), vec![b'x'; size]).unwrap();
    }
    for (name, size) in [("cache", 8), ("eval", 10)] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("private-child-never-public"),
            vec![b'y'; size],
        )
        .unwrap();
    }
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    let retained = &report["checks"]["retained"];
    assert_eq!(retained["state"], "known");
    assert!(retained.get("categories").is_none(), "{retained}");
    assert_eq!(retained["evaluation_copies"]["present"]["value"], true);
    assert_eq!(retained["evaluation_copies"]["bytes"]["value"], 10);
    assert!(!report.to_string().contains("private-child-never-public"));
}

#[test]
fn w6d_retained_growth_invalidates_only_affected_bytes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("eval")).unwrap();
    let path = root.path().join("eval/copy");
    std::fs::write(&path, b"one").unwrap();
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(path, b"longer content").unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    let retained = &report["checks"]["retained"];
    assert_eq!(retained["state"], "changed");
    let row = &retained["evaluation_copies"];
    assert_eq!(row["present"]["value"], true);
    assert_eq!(
        row["bytes"],
        serde_json::json!({"state":"changed","value":null})
    );
    assert_eq!(report["checks"]["raw"]["integrity"], "absent");
    assert_eq!(report["checks"]["source_stability"], "changed");
}

#[cfg(unix)]
#[test]
fn w6d_retained_symlinks_count_the_link_without_walking_its_target() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let target = root.path().join("outside");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("never-count-this"), [0u8; 1000]).unwrap();
    symlink(&target, home.join("eval")).unwrap();
    let own = std::fs::symlink_metadata(home.join("eval")).unwrap().len();
    let report = serde_json::to_value(doctor_report(&home)).unwrap();
    let retained = &report["checks"]["retained"];
    assert_eq!(retained["state"], "known");
    assert_eq!(retained["evaluation_copies"]["present"]["value"], true);
    assert_eq!(retained["evaluation_copies"]["bytes"]["value"], own);
}

#[cfg(target_os = "linux")]
#[test]
fn w6d_retained_fifo_named_eval_is_metadata_only_without_a_data_open() {
    use std::{
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("eval");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: owned synthetic path, no existing file.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    // SAFETY: fresh descriptor transferred to File exactly once.
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(fd >= 0);
    let mut watcher = unsafe { std::fs::File::from_raw_fd(fd) };
    // SAFETY: watcher and terminated path live through registration.
    assert!(
        unsafe { libc::inotify_add_watch(watcher.as_raw_fd(), name.as_ptr(), libc::IN_OPEN) } >= 0
    );
    let report = serde_json::to_value(doctor_report(root.path())).unwrap();
    let row = &report["checks"]["retained"]["evaluation_copies"];
    assert_eq!(row["present"]["value"], false);
    assert_eq!(row["bytes"]["value"], 0);
    assert!(
        matches!(watcher.read(&mut [0u8;64]),Err(error) if error.kind()==ErrorKind::WouldBlock)
    );
}

#[test]
fn w6d_disk_admission_change_remains_observed_at_report_finish() {
    let observation = DiskObservation(Err(CheckState::Changed));
    let mut checks = observation.checks();
    assert!(matches!(checks.state, CheckState::Changed));
    assert!(matches!(
        observation.finish(Path::new("unused"), &mut checks),
        Some(CheckState::Changed)
    ));
    assert!(checks.free_bytes.is_none());
}

#[test]
fn w6d_initially_missing_home_is_absent_instead_of_changed() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("not-initialized");
    let report = serde_json::to_value(doctor_report(&home)).unwrap();
    assert_eq!(report["checks"]["disk"]["state"], "absent");
    assert_eq!(report["checks"]["source_stability"], "known");
    assert!(!home.exists());
}

#[test]
fn w6d_disk_binding_change_is_not_hidden_by_absent_stores() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let kept = root.path().join("kept");
    std::fs::create_dir(&home).unwrap();
    let path = home.clone();
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&path, kept).unwrap();
            std::fs::create_dir(path).unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(&home)).unwrap();
    assert_eq!(report["checks"]["disk"]["state"], "changed");
    assert_eq!(
        report["checks"]["disk"]["free_bytes"],
        serde_json::Value::Null
    );
    assert_eq!(report["checks"]["source_stability"], "changed");
    assert_eq!(report["checks"]["raw"]["integrity"], "absent");
}

#[test]
fn w6d_disk_unknown_and_zero_keep_the_native_low_space_boundary() {
    let unknown = serde_json::to_value(DiskChecks::from_bytes(None)).unwrap();
    assert_eq!(
        unknown,
        serde_json::json!({"state":"unavailable","free_bytes":null,"low_space":null})
    );
    for (value, low) in [
        (0, true),
        (super::super::LOW_FREE_BYTES - 1, true),
        (super::super::LOW_FREE_BYTES, false),
    ] {
        let report = serde_json::to_value(DiskChecks::from_bytes(Some(value))).unwrap();
        assert_eq!(report["state"], "known");
        assert_eq!(report["free_bytes"], value);
        assert_eq!(report["low_space"], low);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn w6d_disk_observation_is_explicit_and_does_not_create_state() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let before = crate::backup::tests::w5b_files(p);
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["disk"]["state"], "known");
    assert!(report["checks"]["disk"]["free_bytes"].as_u64().is_some());
    assert!(report["checks"]["disk"]["low_space"].is_boolean());
    assert!(
        before == crate::backup::tests::w5b_files(p),
        "disk observation changed source files"
    );
}

#[test]
fn w6d_runtime_lock_status_does_not_depend_on_lock_file_contents() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let state = p.join("state");
    std::fs::create_dir(&state).unwrap();
    for length in [3, super::super::readiness::TEXT_LIMIT + 1] {
        let worker = std::fs::File::create(state.join("worker.lock")).unwrap();
        worker.set_len(length).unwrap();
        worker.lock().unwrap();
        let viewer = std::fs::File::create(state.join("view.lock")).unwrap();
        viewer.set_len(length).unwrap();
        viewer.lock().unwrap();
        assert!(crate::worker::lock_held(&state.join("worker.lock")));
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["runtime"]["worker"]["running"],
            serde_json::json!({"state":"known","value":true})
        );
        assert_eq!(
            report["checks"]["runtime"]["resident_viewer"]["running"],
            serde_json::json!({"state":"known","value":true})
        );
    }
}

#[test]
fn w6d_runtime_distinguishes_missing_invalid_and_stale_status() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["runtime"]["recording"]["state"], "absent");
    let state = p.join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(p.join("config.toml"), "[view]\nport=23123\n").unwrap();
    std::fs::write(state.join("recording-failed"), "invalid marker").unwrap();
    std::fs::write(state.join("worker-outcome"), "private-invalid-generation").unwrap();
    std::fs::write(state.join("view-outcome"), "listening 23124").unwrap();
    let viewer = std::fs::File::create(state.join("view.lock")).unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["runtime"]["recording"]["state"], "invalid");
    assert_eq!(
        report["checks"]["runtime"]["worker"]["last"]["state"],
        "invalid"
    );
    assert_eq!(
        report["checks"]["runtime"]["resident_viewer"]["last"]["kind"],
        "stopped"
    );
    viewer.lock().unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["runtime"]["resident_viewer"]["last"]["kind"],
        "other_port"
    );
    assert_eq!(
        report["checks"]["runtime"]["resident_viewer"]["actual_port"],
        23124
    );
    #[cfg(unix)]
    {
        let target = p.join("private-target");
        std::fs::write(&target, "failed io 1 2").unwrap();
        std::fs::remove_file(state.join("recording-failed")).unwrap();
        std::os::unix::fs::symlink(target, state.join("recording-failed")).unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(
            report["checks"]["runtime"]["recording"]["state"],
            "unavailable"
        );
        assert_eq!(
            report["checks"]["runtime"]["recording"]["failed"],
            serde_json::Value::Null
        );
    }
}

/// #855: a worker that stepped aside for a command is no failure: its own kind, healthy; one
/// whose run stopped with no outcome still reads interrupted.
#[test]
fn a_worker_that_stepped_aside_is_healthy() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let state = p.join("state");
    std::fs::create_dir(&state).unwrap();
    // Its lock free: no worker runs.
    std::fs::File::create(state.join("worker.lock")).unwrap();
    for (why, kind, unhealthy) in [
        (crate::worker::ASIDE, "stepped_aside", serde_json::json!([])),
        (
            crate::worker::STOPPED,
            "interrupted",
            serde_json::json!(["worker_failed"]),
        ),
    ] {
        std::fs::write(state.join("worker-outcome"), format!("3\n{why}")).unwrap();
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        assert_eq!(report["checks"]["runtime"]["worker"]["last"]["kind"], kind);
        assert_eq!(
            report["unhealthy"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| *c == "worker_failed")
                .count(),
            unhealthy.as_array().unwrap().len(),
            "{why}"
        );
    }
}

#[test]
fn w6d_runtime_invalidates_changed_lock_and_config_dependencies() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let state = p.join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(
        state.join("worker-outcome"),
        format!("3\n{}", crate::worker::STOPPED),
    )
    .unwrap();
    let worker = std::fs::File::create(state.join("worker.lock")).unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["runtime"]["worker"]["last"]["kind"],
        "interrupted"
    );
    worker.lock().unwrap();
    AFTER_COLLECTION.with(|hook| *hook.borrow_mut() = Some(Box::new(move || drop(worker))));
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["runtime"]["worker"]["running"]["state"],
        "changed"
    );
    assert_eq!(
        report["checks"]["runtime"]["worker"]["last"]["state"],
        "changed"
    );
    assert_eq!(report["checks"]["source_stability"], "changed");
    std::fs::write(state.join("recording-failed"), "ok 1\n").unwrap();
    let path = p.join("config.toml");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(path, "[view]\nport=23125\n").unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["runtime"]["resident_viewer"]["configured_port"]["state"],
        "changed"
    );
    assert_eq!(report["checks"]["runtime"]["recording"]["failed"], false);
}

#[test]
fn w6d_runtime_reads_fixed_status_without_starting_or_renumbering() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let state = p.join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(
        p.join("config.toml"),
        "[worker]\nresident=true\n[view]\nport=23123\n[embedding]\nprovider='unsupported'\n",
    )
    .unwrap();
    std::fs::write(state.join("recording-failed"), "failed disk-full 17 22\n").unwrap();
    std::fs::write(state.join("worker-outcome"), "7\nprivate-worker-error").unwrap();
    std::fs::write(state.join("view-outcome"), "listening 23123").unwrap();
    std::fs::write(state.join("restored"), "private-restored-note").unwrap();
    let worker = std::fs::File::create(state.join("worker.lock")).unwrap();
    let viewer = std::fs::File::create(state.join("view.lock")).unwrap();
    // Snapshot the bytes before taking Windows' exclusive byte-range locks.
    let before = crate::backup::tests::w5b_files(p);
    worker.lock().unwrap();
    viewer.lock().unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    let runtime = &report["checks"]["runtime"];
    assert_eq!(
        runtime["recording"],
        serde_json::json!({"state":"known","failed":true,"class":"disk_full","since_ns":17})
    );
    assert_eq!(
        runtime["worker"]["configured_resident"],
        serde_json::json!({"state":"known","value":true})
    );
    assert_eq!(
        runtime["worker"]["running"],
        serde_json::json!({"state":"known","value":true})
    );
    assert_eq!(runtime["worker"]["last"]["kind"], "failed");
    assert_eq!(
        runtime["resident_viewer"]["configured_port"]["value"],
        23123
    );
    assert_eq!(runtime["resident_viewer"]["last"]["kind"], "up");
    assert_eq!(runtime["resident_viewer"]["actual_port"], 23123);
    assert_eq!(runtime["restore_note"]["value"], true);
    assert!(!report.to_string().contains("private-"));
    worker.unlock().unwrap();
    viewer.unlock().unwrap();
    assert!(
        before == crate::backup::tests::w5b_files(p),
        "runtime diagnosis changed files"
    );
    worker.lock().unwrap();
    viewer.lock().unwrap();
    let path = state.join("worker-outcome");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(path, "8\n").unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["runtime"]["worker"]["last"]["state"],
        "changed"
    );
    assert_eq!(
        report["checks"]["runtime"]["resident_viewer"]["last"]["kind"],
        "up"
    );
}

#[test]
fn w6d_embedding_quota_uses_native_shared_pool_and_query_reserve() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let db = crate::providers_db::open(p).unwrap();
    let now = crate::db::now_ms();
    db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms,detail,usd) VALUES(?1,?2,'embed','error',1,'private-first-error',0.1),(?1,?2,'query','ok',1,'private-query',0.2),(1,?2,'private-role','error',1,'private-last-error',0)",rusqlite::params![now,crate::embed::CALLS]).unwrap();
    crate::providers_db::set_state(
        &db,
        crate::embed::CALLS,
        crate::providers_db::State {
            down_until: now + 600_000,
            ..Default::default()
        },
    )
    .unwrap();
    for cap in [39u32, 40, 41] {
        std::fs::write(p.join("config.toml"),format!("[embedding]\nprovider='workers-ai'\naccount_id='private-account'\ndaily_requests={cap}\nmonthly_usd=2.5\n")).unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let report = serde_json::to_value(doctor_report(p)).unwrap();
        let quota = &report["checks"]["providers"]["embedding_quota"];
        assert_eq!(quota["document_daily_requests"], cap.saturating_sub(40));
        assert_eq!(quota["query_daily_requests"], cap);
        assert_eq!(quota["monthly_usd_cap"], 2.5);
        assert_eq!(
            quota["calls_last_day"],
            serde_json::json!({"state":"known","value":2})
        );
        assert_eq!(quota["rest"]["kind"], "resting");
        assert_eq!(quota["last_error"]["role"], "other");
        assert_eq!(quota["last_error"]["at_ms"], 1);
        assert!(!report.to_string().contains("private-"));
        assert_eq!(report["checks"]["embeddings"]["state"], "absent");
        assert_eq!(before, crate::backup::tests::w5b_files(p));
    }
    let path = p.join("providers.db");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            Connection::open(path)
                .unwrap()
                .execute_batch("UPDATE provider_calls SET usd=usd+1;")
                .unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["providers"]["embedding_quota"]["query_daily_requests"],
        41
    );
    assert_eq!(
        report["checks"]["providers"]["embedding_quota"]["calls_last_day"]["state"],
        "changed"
    );
    std::fs::write(p.join("config.toml"), "").unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["providers"]["embedding_quota"]["state"],
        "off"
    );
}

#[test]
fn w6d_failed_final_backup_validation_invalidates_dependent_facts() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = crate::raw::open(p).unwrap();
    raw.append(&crate::raw::test_event("invented final-check record"))
        .unwrap();
    crate::backup::export(p).unwrap();
    let directory = p.join("backups");
    let checksum = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".sha256"))
        .unwrap();
    let original = std::fs::read(&checksum).unwrap();
    let path = checksum.clone();
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(path, [0xff]).unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["backups"]["state"], "unreadable");
    assert_eq!(
        report["checks"]["backups"]["verification"]["checksum_mismatch"],
        serde_json::json!({"state":"unreadable","value":null})
    );
    assert_eq!(report["checks"]["backups"]["record_segments"]["value"], 1);
    assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 1);
    std::fs::write(checksum, original).unwrap();
    let kept = p.join("kept");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&directory, kept).unwrap();
            std::fs::write(directory, b"not a directory").unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["backups"]["record_segments"]["value"],
        serde_json::Value::Null
    );
    assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 1);
}

#[test]
fn w6d_backup_directory_enumeration_and_children_stay_on_the_admitted_object() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("selected");
    std::fs::create_dir(&selected).unwrap();
    let mut expected = Vec::new();
    for index in 0..400 {
        let name = format!("entry-{index:04}-{}.data", "あ".repeat(70));
        std::fs::write(selected.join(&name), b"owned original").unwrap();
        expected.push(std::ffi::OsString::from(name));
    }
    expected.sort();
    let mut directory = super::super::DiagnosticDirectory::open(&selected).unwrap();
    for _ in 0..2 {
        let mut names = directory.names().unwrap();
        names.sort();
        assert_eq!(names, expected);
    }
    std::fs::rename(&selected, root.path().join("kept")).unwrap();
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(selected.join(&expected[0]), b"replacement").unwrap();
    let mut value = String::new();
    directory
        .open_file(&expected[0])
        .unwrap()
        .read_to_string(&mut value)
        .unwrap();
    assert_eq!(value, "owned original");
    let replacement = super::super::DiagnosticDirectory::open(&selected).unwrap();
    assert_ne!(
        directory.identity().unwrap(),
        replacement.identity().unwrap()
    );
    assert_eq!(
        directory
            .open_file(std::ffi::OsStr::new("../outside"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
}

#[cfg(target_os = "linux")]
#[test]
fn w6d_backup_fifo_is_refused_before_a_data_open() {
    use std::{
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let root = tempfile::tempdir().unwrap();
    let name = "private-fifo-000000000001-000000000002.seg.zst";
    let path = root.path().join(name);
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: all native calls use this owned synthetic path and newly owned descriptors.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    // SAFETY: these flags return a nonblocking close-on-exec descriptor.
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(fd >= 0);
    // SAFETY: successful init returned a newly owned descriptor exactly once.
    let mut watcher = unsafe { std::fs::File::from_raw_fd(fd) };
    // SAFETY: the watcher and terminated path live through registration.
    assert!(
        unsafe { libc::inotify_add_watch(watcher.as_raw_fd(), path.as_ptr(), libc::IN_OPEN) } >= 0
    );
    let directory = super::super::DiagnosticDirectory::open(root.path()).unwrap();
    assert_eq!(
        directory
            .open_file(std::ffi::OsStr::new(name))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
    let mut events = [0u8; 64];
    assert!(
        matches!(watcher.read(&mut events),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
    );
}

#[cfg(windows)]
#[test]
fn w6d_backup_windows_junctions_do_not_redirect_directory_or_child_reads() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("selected");
    let target = root.path().join("target");
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(selected.join("owned"), b"original").unwrap();
    std::fs::write(target.join("owned"), b"target").unwrap();
    let mut directory = super::super::DiagnosticDirectory::open(&selected).unwrap();
    std::fs::rename(&selected, root.path().join("kept")).unwrap();
    let system = std::env::var_os("SystemRoot").unwrap();
    let junction = |link: &Path| {
        let output =
            std::process::Command::new(PathBuf::from(&system).join("System32").join("cmd.exe"))
                .env_clear()
                .env("SystemRoot", &system)
                .env("TEMP", root.path())
                .env("TMP", root.path())
                .current_dir(root.path())
                .args(["/d", "/c", "mklink", "/J"])
                .arg(link)
                .arg(&target)
                .output()
                .unwrap();
        assert!(output.status.success(), "private junction fixture failed");
    };
    junction(&selected);
    assert!(super::super::DiagnosticDirectory::open(&selected).is_err());
    assert_eq!(
        directory.names().unwrap(),
        vec![std::ffi::OsString::from("owned")]
    );
    let mut text = String::new();
    directory
        .open_file(std::ffi::OsStr::new("owned"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "original");
    junction(&root.path().join("kept").join("child"));
    // A real directory would open here; a junction must not be followed.
    assert!(
        directory
            .open_directory(std::ffi::OsStr::new("child"))
            .is_err()
    );
    assert!(directory.open_file(std::ffi::OsStr::new("child")).is_err());
    assert_eq!(std::fs::read(target.join("owned")).unwrap(), b"target");
}

#[test]
fn w6d_non_null_malformed_covers_is_not_silently_ignored() {
    for (covers, invalid) in [
        (None, false),
        (Some(serde_json::Value::Null), false),
        (Some(serde_json::json!({"from_seq":1,"to_seq":1})), false),
        (
            Some(serde_json::json!({"from_seq":1,"to_seq":1,"from_offset":null,"to_offset":null})),
            false,
        ),
        (
            Some(serde_json::json!({"from_seq":1,"to_seq":1,"from_offset":"bad"})),
            true,
        ),
        (
            Some(serde_json::json!({"from_seq":1,"to_seq":1,"to_offset":1.5})),
            true,
        ),
        (
            Some(serde_json::json!({"from_seq":1,"to_seq":1,"from_offset":u64::MAX})),
            true,
        ),
        (Some(serde_json::json!({"from_seq":1})), true),
        (Some(serde_json::json!({})), true),
        (Some(serde_json::json!("invalid")), true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let mut body = serde_json::json!({"from_seq":1,"to_seq":1,"outcome":"skipped","reason":"imported:oboete-v1"});
        if let Some(covers) = covers {
            body["covers"] = covers;
        }
        raw.append_ops(&[(crate::raw::OpKind::Window, body)])
            .unwrap();
        let report = serde_json::to_value(doctor_report(home.path())).unwrap();
        assert_eq!(
            report["checks"]["raw"]["parking"]["state"],
            if invalid { "unavailable" } else { "known" }
        );
        assert_eq!(
            crate::curate::parked_spans(&raw, "oboete-v1").is_err(),
            invalid,
            "native recuration must reject corrupt ranges before arithmetic or sends"
        );
    }
}

#[test]
fn w6d_non_null_window_offsets_require_integers() {
    for field in ["from_offset", "to_offset"] {
        for (offset, invalid) in [
            (serde_json::Value::Null, false),
            (serde_json::json!("bad"), true),
            (serde_json::json!(1.5), true),
            (serde_json::json!(u64::MAX), true),
        ] {
            let home = tempfile::tempdir().unwrap();
            let mut raw = crate::raw::open(home.path()).unwrap();
            let mut body = serde_json::json!({"from_seq":1,"to_seq":1,"outcome":"skipped","reason":"imported:oboete-v1"});
            body[field] = offset;
            raw.append_ops(&[(crate::raw::OpKind::Window, body)])
                .unwrap();
            let report = serde_json::to_value(doctor_report(home.path())).unwrap();
            assert_eq!(
                report["checks"]["raw"]["parking"]["state"],
                if invalid { "unavailable" } else { "known" }
            );
            assert_eq!(
                crate::curate::parked_spans(&raw, "oboete-v1").is_err(),
                invalid
            );
        }
    }
}

#[test]
fn w6d_logically_damaged_window_ranges_leave_parking_unknown() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    raw.append_ops(&[
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":i64::MIN,"from_offset":-1,"to_seq":1,"outcome":"skipped","reason":"imported:oboete-v1"})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":i64::MIN,"to_seq":0,"recurate":true})),
        ]).unwrap();
    let report = serde_json::to_value(doctor_report(home.path())).unwrap();
    assert_eq!(report["checks"]["raw"]["integrity"], "known");
    assert_eq!(report["checks"]["raw"]["max_op_seq"]["value"], 2);
    assert_eq!(report["checks"]["raw"]["parking"]["state"], "unavailable");
    assert_eq!(
        report["checks"]["raw"]["parking"]["rows"],
        serde_json::Value::Null
    );
}

#[test]
fn w6d_backup_verification_uses_selected_membership_and_native_checksums() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = crate::raw::open(p).unwrap();
    raw.append(&crate::raw::test_event("invented backup record"))
        .unwrap();
    raw.append(&crate::raw::test_event("another invented backup record"))
        .unwrap();
    raw.append_ops(&[(
        crate::raw::OpKind::Migration,
        serde_json::json!({"key":"invented","through":1}),
    )])
    .unwrap();
    crate::backup::export(p).unwrap();
    let selected = p.join("selected");
    std::fs::rename(p.join("backups"), &selected).unwrap();
    std::fs::write(p.join("config.toml"), "[backup]\ndir='selected'\n").unwrap();
    let record = std::fs::read_dir(&selected)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".seg.zst"))
        .unwrap();
    let foreign = selected.join("private-foreign-000000000001-000000000090.seg.zst");
    std::fs::copy(&record, &foreign).unwrap();
    let sum = foreign.with_file_name(format!(
        "{}.sha256",
        foreign.file_name().unwrap().to_str().unwrap()
    ));
    std::fs::write(&sum, crate::forget::hash(&std::fs::read(&foreign).unwrap())).unwrap();
    let before = crate::backup::tests::w5b_files(p);
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    let backup = &report["checks"]["backups"];
    assert_eq!(
        backup["record_segments"],
        serde_json::json!({"state":"known","value":2})
    );
    assert_eq!(backup["op_segments"]["value"], 1);
    assert_eq!(backup["through_seq"]["value"], 2);
    assert_eq!(backup["through_op_seq"]["value"], 1);
    assert_eq!(backup["verification"]["missing_checksum"]["value"], 0);
    assert_eq!(backup["verification"]["checksum_mismatch"]["value"], 0);
    assert!(!report.to_string().contains("private-"));
    assert_eq!(before, crate::backup::tests::w5b_files(p));
    std::fs::write(&sum, "wrong synthetic checksum\n").unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["backups"]["verification"]["checksum_mismatch"]["value"],
        1
    );
    std::fs::remove_file(&sum).unwrap();
    std::fs::write(
        p.join("config.toml"),
        "[backup]\ndir='selected'\n[embedding]\nprovider='unsupported'\n",
    )
    .unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["backups"]["verification"]["missing_checksum"]["value"],
        1
    );
    assert_eq!(report["checks"]["backups"]["record_segments"]["value"], 2);
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(foreign).unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["backups"]["state"], "changed");
    assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 2);
}

#[test]
fn w6d_curation_uses_native_leftover_spans_and_partial_checkpoint() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = crate::raw::open(p).unwrap();
    for seq in 1..=9 {
        let mut event = crate::raw::test_event("invented imported turn");
        event.source = if seq <= 6 {
            "oboete-v1"
        } else if seq == 7 {
            "private-source"
        } else {
            "hook"
        }
        .into();
        if seq == 9 {
            event.kind = "touch".into();
        }
        raw.append(&event).unwrap();
    }
    raw.append_ops(&[
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":1,"to_seq":5,"outcome":"skipped","reason":"imported:oboete-v1"})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":2,"to_seq":4,"recurate":true})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":6,"to_seq":7,"to_offset":1,"outcome":"ok"})),
            (crate::raw::OpKind::Window,serde_json::json!({"from_seq":1,"to_seq":99,"outcome":"ok"})),
        ]).unwrap();
    let writer = Connection::open(p.join("raw.db")).unwrap();
    writer
        .execute_batch("UPDATE ops SET device='private-foreign-device' WHERE op_seq=4;")
        .unwrap();
    let before = crate::backup::tests::w5b_files(p);
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["raw"]["curated_through"],
        serde_json::json!({"state":"known","value":6})
    );
    assert_eq!(
        report["checks"]["raw"]["parking"]["rows"],
        serde_json::json!([
            {"source":"oboete_v1","parked":2,"waiting":0},
            {"source":"transcript","parked":0,"waiting":0},
            {"source":"other","parked":0,"waiting":1}
        ])
    );
    assert!(!report.to_string().contains("private-"));
    assert_eq!(before, crate::backup::tests::w5b_files(p));
    writer
        .execute_batch("ALTER TABLE ops DROP COLUMN batch;")
        .unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 9);
    assert_eq!(report["checks"]["raw"]["curated_through"]["value"], 6);
    assert_eq!(
        report["checks"]["raw"]["parking"]["state"],
        "schema_missing"
    );
    writer.execute_batch("UPDATE ops SET body=json_set(body,'$.to_seq',-9223372036854775808,'$.to_offset',1) WHERE op_seq=3;").unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["raw"]["curated_through"]["state"],
        "unavailable"
    );
    assert_eq!(report["checks"]["raw"]["max_seq"]["value"], 9);
    writer.execute_batch("ALTER TABLE ops ADD COLUMN batch INTEGER NOT NULL DEFAULT 0; UPDATE ops SET body=json_set(body,'$.to_seq',7,'$.to_offset',1) WHERE op_seq=3;").unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["raw"]["parking"]["state"], "known");
    writer.execute_batch("UPDATE ops SET body=json_set(body,'$.to_seq',9223372036854775807,'$.to_offset',NULL) WHERE op_seq=3;").unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["raw"]["parking"]["state"], "unavailable");
    assert_eq!(
        report["checks"]["raw"]["curated_through"]["value"],
        i64::MAX
    );
}

#[test]
fn w6d_config_used_for_facts_must_match_its_admitted_bytes() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let kept = home.path().join("original.toml");
    std::fs::write(&path, "providers=[]\n").unwrap();
    let _ledger = crate::providers_db::open(home.path()).unwrap();
    let (from, to) = (path.clone(), kept.clone());
    BEFORE_CONFIG.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&from, &to).unwrap();
            std::fs::write(
                from,
                "providers=[]\n[embedding]\nprovider='workers-ai'\naccount_id='synthetic'\n",
            )
            .unwrap();
        }))
    });
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(kept, path).unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(home.path())).unwrap();
    assert_eq!(report["checks"]["source_stability"], "changed");
    assert_eq!(report["checks"]["embeddings"]["state"], "changed");
    assert_eq!(
        report["checks"]["providers"]["usage"]["daily_calls"]["state"],
        "changed"
    );
}

#[test]
fn w6d_cached_ledger_counts_reservations_without_double_spend_or_key_reads() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    std::fs::write(p.join("config.toml"), "[[providers]]\nkind='cli'\nname='private-shared'\ncli='claude'\n[[providers]]\nkind='cli'\nname='private-shared'\ncli='claude'\n").unwrap();
    let db = crate::providers_db::open(p).unwrap();
    let now = crate::db::now_ms();
    for (provider, role, outcome, usd, detail, prompt, completion, bytes) in [
        (
            "private-shared",
            "curator",
            "error",
            2.0,
            "transport",
            Some(3),
            Some(2),
            10,
        ),
        (
            "private-shared",
            "curator",
            "reserved",
            1.0,
            "[10,20]",
            None,
            None,
            0,
        ),
        (
            crate::embed::CALLS,
            "query",
            "ok",
            0.25,
            "private-detail",
            None,
            None,
            10,
        ),
        (
            crate::embed::CALLS,
            "embed",
            "ok",
            0.5,
            "private-detail",
            None,
            None,
            10,
        ),
        (
            "private-removed",
            "private-role",
            "ok",
            0.5,
            "private-detail",
            None,
            None,
            10,
        ),
    ] {
        db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms,detail,usd,prompt_tokens,completion_tokens,bytes_out) VALUES(?1,?2,?3,?4,12,?5,?6,?7,?8,?9)",rusqlite::params![now,provider,role,outcome,detail,usd,prompt,completion,bytes]).unwrap();
    }
    crate::providers_db::set_state(
        &db,
        "private-shared",
        crate::providers_db::State {
            down_until: crate::providers_db::OWNER_HOLD,
            ..Default::default()
        },
    )
    .unwrap();
    crate::providers_db::set_state(
        &db,
        "private-resting",
        crate::providers_db::State {
            down_until: now + 600_000,
            ..Default::default()
        },
    )
    .unwrap();
    crate::providers_db::set_key_limit(&db, "private-shared", Some(100), now, "private-key-sha")
        .unwrap();
    db.execute_batch("INSERT INTO isolation VALUES('private-cli','private-version',1,'private-detail',1),('private-cli','private-version2',0,'private-detail',2);
         INSERT INTO pending VALUES('private-device',1,NULL,2,NULL,'private-reason','private-hold',1,0,0,'private-prompt');").unwrap();
    let before = crate::backup::tests::w5b_files(p);
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    let ledger = &report["checks"]["providers"];
    assert_eq!(
        ledger["spend"]["curation_usd"],
        serde_json::json!({"state":"known","value":3.5})
    );
    assert_eq!(ledger["spend"]["embed_query_usd"]["value"], 0.75);
    assert_eq!(ledger["spend"]["reserved_usd"]["value"], 1.0);
    assert_eq!(ledger["usage"]["daily_calls"]["value"], 2);
    assert_eq!(ledger["usage"]["tokens"]["value"], 5);
    assert_eq!(ledger["usage"]["reserved_calls"]["value"], 1);
    assert_eq!(ledger["usage"]["reserved_tokens"]["value"], 30.0);
    assert_eq!(ledger["usage"]["key_binding"], "unchecked");
    assert_eq!(ledger["stopped"]["owner"]["value"], 1);
    assert_eq!(ledger["stopped"]["resting"]["value"], 1);
    assert_eq!(ledger["isolation"]["passed"]["value"], 0);
    assert_eq!(ledger["isolation"]["failed"]["value"], 1);
    assert_eq!(ledger["pending"]["value"], 1);
    assert_eq!(ledger["recent"]["calls"].as_array().unwrap().len(), 5);
    assert!(!report.to_string().contains("private-"));
    assert_eq!(before, crate::backup::tests::w5b_files(p));
    assert!(!p.join("raw.db").exists());
    db.execute_batch("DROP TABLE pending;").unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["providers"]["pending"],
        serde_json::json!({"state":"schema_missing","value":null})
    );
    assert_eq!(
        report["checks"]["providers"]["spend"]["curation_usd"]["value"],
        3.5
    );
    let path = p.join("config.toml");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(path, "providers=[]\n").unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["providers"]["usage"]["daily_calls"],
        serde_json::json!({"state":"changed","value":null})
    );
    assert_eq!(
        report["checks"]["providers"]["spend"]["curation_usd"]["value"],
        3.5
    );
    let path = p.join("providers.db");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            Connection::open(path)
                .unwrap()
                .execute_batch("UPDATE provider_calls SET usd=usd+1;")
                .unwrap();
        }))
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["providers"]["integrity"], "changed");
    assert_eq!(
        report["checks"]["providers"]["spend"]["curation_usd"]["value"],
        serde_json::Value::Null
    );
    assert_eq!(report["checks"]["raw"]["integrity"], "absent");
}

#[test]
fn w6d_embeddings_off_and_native_waiting_counts_need_no_writes() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let k = Connection::open(p.join("knowledge.db")).unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(report["checks"]["embeddings"]["state"], "off");
    std::fs::write(
        p.join("config.toml"),
        "[embedding]\nprovider='workers-ai'\naccount_id='synthetic-account'\n",
    )
    .unwrap();
    k.execute_batch(
        "CREATE TABLE claims(uid TEXT);
             INSERT INTO claims VALUES('a'),('b');
             CREATE VIEW active AS SELECT uid FROM claims;
             CREATE TABLE imported(uid TEXT);
             INSERT INTO imported VALUES('i'),('i'),('j');
             CREATE TABLE raw_docs(device TEXT,seq INTEGER,kind TEXT);
             INSERT INTO raw_docs VALUES('private-device',1,'prompt'),('private-device',2,'reply'),
               ('private-device',3,'tool');
             CREATE TABLE cards(device TEXT,op_seq INTEGER,n INTEGER,replaced_by INTEGER);
             INSERT INTO cards VALUES('private-device',5,0,NULL),('private-device',5,1,NULL),
               ('private-device',4,0,5);
             CREATE TABLE turns(device TEXT,op_seq INTEGER,skipped INTEGER);
             INSERT INTO turns VALUES('private-device',6,0),('private-device',7,0),
               ('private-device',8,1);
             CREATE TABLE vec_generation(embedder TEXT,state TEXT);
             CREATE TABLE vector_keys(embedder TEXT,kind TEXT,key TEXT,skipped TEXT);",
    )
    .unwrap();
    let embedder = crate::embed::EMBEDDER;
    k.execute("INSERT INTO vec_generation VALUES(?1,'active')", [embedder])
        .unwrap();
    for (kind, key, skipped) in [
        ("c", "a", None),
        ("o", "private-device.5.0", None),
        ("s", "Sprivate-device.6", None),
        ("k", "i", Some("excluded")),
        ("r", "private-device:1", Some("private-reason")),
    ] {
        k.execute(
            "INSERT INTO vector_keys VALUES(?1,?2,?3,?4)",
            rusqlite::params![embedder, kind, key, skipped],
        )
        .unwrap();
    }
    let before = crate::backup::tests::w5b_files(p);
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    let checks = &report["checks"]["embeddings"];
    assert_eq!(checks["generation"]["value"], "active");
    for name in ["claims", "cards", "summaries", "imports", "records"] {
        assert_eq!(
            checks["waiting"][name],
            serde_json::json!({"state":"known","value":1})
        );
    }
    assert_eq!(
        checks["skipped"]["rows"],
        serde_json::json!([{"reason":"excluded","count":1},{"reason":"held","count":0},{"reason":"refused","count":0},{"reason":"empty","count":0},{"reason":"other","count":1}])
    );
    let public = report.to_string();
    for secret in ["private-reason", "private-device", "synthetic-account"] {
        assert!(!public.contains(secret));
    }
    assert_eq!(before, crate::backup::tests::w5b_files(p));
    assert!(!p.join("providers.db").exists());
    k.execute_batch("DROP TABLE raw_docs;").unwrap();
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["embeddings"]["waiting"]["claims"]["value"],
        1
    );
    assert_eq!(
        report["checks"]["embeddings"]["waiting"]["records"],
        serde_json::json!({"state":"schema_missing","value":null})
    );
}

/// CodeRabbit on #429: the cards' and the summaries' waiting counts are among what a complete
/// report has established.
#[test]
fn w6d_unknown_card_or_summary_waiting_counts_leave_the_report_incomplete() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    std::fs::write(
        p.join("config.toml"),
        "[embedding]\nprovider='workers-ai'\naccount_id='synthetic-account'\n",
    )
    .unwrap();
    let k = Connection::open(p.join("knowledge.db")).unwrap();
    let cards = "CREATE TABLE cards(device TEXT,op_seq INTEGER,n INTEGER,replaced_by INTEGER);";
    let turns = "CREATE TABLE turns(device TEXT,op_seq INTEGER,skipped INTEGER);";
    k.execute_batch(&format!(
        "CREATE TABLE rewinds(ts);
             CREATE TABLE gaps(agent TEXT,transcript_turns INTEGER,raw_turns INTEGER);
             CREATE TABLE claims(uid TEXT);
             CREATE VIEW active AS SELECT uid FROM claims;
             CREATE TABLE imported(uid TEXT);
             CREATE TABLE raw_docs(device TEXT,seq INTEGER,kind TEXT);
             CREATE TABLE vec_generation(embedder TEXT,state TEXT);
             CREATE TABLE vector_keys(embedder TEXT,kind TEXT,key TEXT,skipped TEXT);
             {cards} {turns}"
    ))
    .unwrap();
    let complete = || serde_json::to_value(doctor_report(p)).unwrap()["complete"].clone();
    assert_eq!(complete(), true);
    for (table, create) in [("cards", cards), ("turns", turns)] {
        k.execute_batch(&format!("DROP TABLE {table};")).unwrap();
        assert_eq!(complete(), false, "{table}");
        k.execute_batch(create).unwrap();
    }
    assert_eq!(complete(), true);
}

/// Decision 99 (2026-11-12): a store of oboete v1 is named by its presence only. Doctor neither
/// opens nor counts it, and the report is neither unhealthy nor incomplete for it.
#[test]
fn w6d_a_v1_store_is_named_by_its_presence_only() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("oboete.db");
    std::fs::write(&store, b"not a database: never opened").unwrap();
    let report = serde_json::to_value(doctor_report(home.path())).unwrap();
    assert_eq!(
        report["stores"]["legacy"],
        serde_json::json!({"file":"present","wal":"absent"})
    );
    assert!(report["checks"].get("legacy").is_none(), "{report}");
    assert_eq!(report["unhealthy"], serde_json::json!([]));
    assert_eq!(report["checks"]["source_stability"], "known");
    assert_eq!(
        std::fs::read(&store).unwrap(),
        b"not a database: never opened"
    );
    let names: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["oboete.db"]);
}

#[test]
fn w6d_an_observed_change_stays_changed_after_the_original_file_returns() {
    let home = tempfile::tempdir().unwrap();
    let original = home.path().join("raw.db");
    let retired = home.path().join("retired.db");
    let conn = Connection::open(&original).unwrap();
    conn.execute_batch("CREATE TABLE fixture(value INTEGER);")
        .unwrap();
    drop(conn);
    let (from, kept) = (original.clone(), retired.clone());
    BEFORE_COPY.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&from, &kept).unwrap();
            std::fs::copy(&kept, &from).unwrap();
        }));
    });
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(&original).unwrap();
            std::fs::rename(&retired, &original).unwrap();
        }));
    });
    let report = serde_json::to_value(doctor_report(home.path())).unwrap();
    assert_eq!(report["checks"]["raw"]["integrity"], "changed");
    assert_eq!(
        report["checks"]["source_stability"], "changed",
        "Doctor forgot a change already observed during collection"
    );
}

#[test]
fn w6d_config_change_does_not_discard_independent_raw_facts() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = crate::raw::open(home.path()).unwrap();
    let seq = raw
        .append(&crate::raw::test_event("owned config consistency fixture"))
        .unwrap();
    let config = home.path().join("config.toml");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(config, "[summary]\ncurate=false\n").unwrap();
        }));
    });
    let report = serde_json::to_value(doctor_report(home.path())).unwrap();
    assert_eq!(report["checks"]["source_stability"], "changed");
    assert_eq!(report["checks"]["embeddings"]["state"], "changed");
    assert_eq!(report["checks"]["raw"]["max_seq"]["value"], seq);
    assert_eq!(report["checks"]["raw"]["max_seq"]["state"], "known");
}

#[test]
fn w6d_changed_raw_after_collection_keeps_independent_knowledge() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = crate::raw::open(p).unwrap();
    raw.append(&crate::raw::test_event("owned consistency fixture"))
        .unwrap();
    let k = crate::knowledge::open(p).unwrap();
    k.execute_batch("INSERT INTO rewinds VALUES(1000,'claims','private-device',2,1);")
        .unwrap();
    let source = p.join("raw.db");
    AFTER_COLLECTION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let writer = Connection::open(source).unwrap();
            writer
                .execute_batch("UPDATE records SET seq=seq+1;")
                .unwrap();
        }));
    });
    let report = serde_json::to_value(doctor_report(p)).unwrap();
    assert_eq!(
        report["checks"]["raw"]["max_seq"]["state"], "changed",
        "Doctor kept a source-derived value after its source changed"
    );
    assert_eq!(
        report["checks"]["raw"]["max_seq"]["value"],
        serde_json::Value::Null
    );
    assert_eq!(
        report["checks"]["knowledge"]["rewinds"]["count"]["value"],
        1
    );
}
