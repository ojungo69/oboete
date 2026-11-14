fn imported_home() -> (tempfile::TempDir, Maintenance, PathBuf) {
    let (root, maintenance, home, _) = fixture();
    let operation = json!({"kind":"transcripts","agent":"codex"});
    let preview = maintenance
        .preview(
            None,
            &home,
            &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
        )
        .unwrap();
    let body = serde_json::to_vec(&json!({"operation":operation,"preview_key":preview["preview_key"],"operation_id":"1".repeat(64),"confirmed":true})).unwrap();
    assert_eq!(
        maintenance
            .start(executable::CommandCaller::Worker, &home, &body)
            .unwrap()["last"]["phase"],
        "complete"
    );
    crate::worker::run_once(&home).unwrap();
    (root, maintenance, home)
}

#[test]
fn rebuild_and_restore_require_fixed_previewed_operations_and_replay_receipts() {
    let (_root, maintenance, home) = imported_home();
    for (index, kind) in ["rebuild", "restore"].into_iter().enumerate() {
        let operation = json!({"kind":kind});
        let preview = maintenance
            .preview(
                None,
                &home,
                &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
            )
            .unwrap();
        assert_eq!(preview["kind"], kind);
        assert_eq!(preview["no_model_request"], true);
        assert_eq!(preview["hybrid_ready"], false);
        assert_eq!(preview["raw"]["records"], 8);
        let body = serde_json::to_vec(&json!({"operation":operation,"preview_key":preview["preview_key"],"operation_id":if index==0 {"2".repeat(64)} else {"3".repeat(64)},"confirmed":true})).unwrap();
        let receipt = maintenance
            .start(executable::CommandCaller::Worker, &home, &body)
            .unwrap();
        assert_eq!(receipt["last"]["kind"], kind);
        assert_eq!(receipt["last"]["phase"], "complete");
        assert_eq!(
            receipt["last"]["result"]["outcome"]["index"]["state"],
            "complete"
        );
        assert_eq!(receipt["last"]["result"]["outcome"]["hybrid_ready"], false);
        assert_eq!(
            maintenance
                .start(executable::CommandCaller::Worker, &home, &body)
                .unwrap(),
            receipt
        );
    }
}

#[test]
fn rebuild_and_restore_reject_extra_sources_and_stale_saved_configuration() {
    let (_root, maintenance, home) = imported_home();
    for kind in ["rebuild", "restore"] {
        let bad = serde_json::to_vec(&json!({"operation":{"kind":kind,"from":"other-store.db"}}))
            .unwrap();
        assert_eq!(
            maintenance.preview(None, &home, &bad).unwrap_err().status,
            400
        );
        let operation = json!({"kind":kind});
        let preview = maintenance
            .preview(
                None,
                &home,
                &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
            )
            .unwrap();
        let raw_before = std::fs::read(home.join("raw.db")).unwrap();
        std::fs::write(
            home.join("config.toml"),
            format!("# changed saved scope {kind}\n[summary]\ncurate = false\n"),
        )
        .unwrap();
        let request = serde_json::to_vec(&json!({"operation":operation,"preview_key":preview["preview_key"],"operation_id":if kind=="rebuild" {"4".repeat(64)} else {"5".repeat(64)},"confirmed":true})).unwrap();
        let receipt = maintenance
            .start(executable::CommandCaller::Worker, &home, &request)
            .unwrap();
        assert_eq!(receipt["last"]["phase"], "failed");
        assert_eq!(receipt["last"]["result"]["code"], "maintenance_stale");
        assert_eq!(receipt["last"]["committed"], false);
        assert_eq!(std::fs::read(home.join("raw.db")).unwrap(), raw_before);
    }
}

use super::*;

#[test]
fn w5c_cached_status_and_replay_apply_current_display_rules_only_to_labels() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let canary = "synthetic-private-session";
    for operation in [
        Operation::Finish {},
        Operation::Recurate {
            scope: curate::RecurationScope::Queued {},
        },
    ] {
        std::fs::write(home.join("config.toml"), "providers = []\n").unwrap();
        let maintenance = Maintenance::with_roots(home.join("claude"), home.join("codex"));
        let id = "a".repeat(64);
        let key = "b".repeat(64);
        let mut hash = Sha256::new();
        hash.update(b"oboete:maintenance-request:v1\0");
        hash.update(serde_json::to_vec(&operation).unwrap());
        hash.update([0]);
        hash.update(key.as_bytes());
        let outcome = if matches!(operation, Operation::Finish { .. }) {
            json!({"operation":"finish","import":{"records":7,
                    "deleted_session_labels":[canary],"uncertain_identifier_labels":[canary]},
                    "deletion":{"removed":1,"targets":[{"label":canary,"state":"removed","bytes":42}]}})
        } else {
            json!({"preview":{"preview_key":key,"scope":{"kind":"queued"},
                    "plan":{"windows":1,"long_sessions":[{"label":canary,"characters":513}]}}})
        };
        let cached = json!({"code":null,"outcome":outcome});
        maintenance.state.lock().unwrap().last = Some(Run {
            operation_id: id.clone(),
            kind: if matches!(operation, Operation::Finish { .. }) {
                "finish"
            } else {
                "recurate"
            },
            agent: None,
            phase: "complete",
            stage: "complete",
            progress: Progress::default(),
            committed: true,
            result: Some(cached.clone()),
            fingerprint: format!("{:x}", hash.finalize()),
            consent: None,
        });
        let request = serde_json::to_vec(&json!({"operation":operation,
                "preview_key":key,"operation_id":id,"confirmed":true}))
        .unwrap();
        assert!(maintenance.show(home).to_string().contains(canary));
        for config in [
            "[redaction]\nextra_rules = [{id = 'display', regex = 'synthetic-private-session|^[0-9a-f]{64}$'}]\n",
            "invalid = [toml",
        ] {
            std::fs::write(home.join("config.toml"), config).unwrap();
            let shown = maintenance.show(home);
            assert!(
                !shown.to_string().contains(canary),
                "cached maintenance labels bypass current display rules"
            );
            let replay = maintenance
                .start(executable::CommandCaller::Worker, home, &request)
                .unwrap();
            assert_eq!(shown, replay);
            assert_eq!(shown["last"]["operation_id"], id);
            assert_eq!(shown["last"]["phase"], "complete");
            assert_eq!(shown["last"]["committed"], true);
            if matches!(operation, Operation::Finish { .. }) {
                assert_eq!(shown["last"]["result"]["outcome"]["import"]["records"], 7);
                assert_eq!(
                    shown["last"]["result"]["outcome"]["deletion"]["targets"][0]["bytes"],
                    42
                );
            } else {
                assert_eq!(
                    shown["last"]["result"]["outcome"]["preview"]["preview_key"],
                    key
                );
                assert_eq!(
                    shown["last"]["result"]["outcome"]["preview"]["plan"]["long_sessions"][0]["characters"],
                    513
                );
            }
            assert_eq!(
                maintenance
                    .state
                    .lock()
                    .unwrap()
                    .last
                    .as_ref()
                    .unwrap()
                    .result
                    .as_ref(),
                Some(&cached)
            );
        }
        assert_eq!(std::fs::read_dir(home).unwrap().count(), 1);
    }
}

fn fixture() -> (tempfile::TempDir, Maintenance, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let codex = root.path().join("codex/sessions");
    std::fs::create_dir_all(&codex).unwrap();
    let source = codex.join("rollout-native.jsonl");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/transcripts/codex-basic.jsonl"),
        &source,
    )
    .unwrap();
    let maintenance = Maintenance::with_roots(root.path().join("claude/projects"), codex);
    let home = root.path().join("memory");
    (root, maintenance, home, source)
}

#[test]
fn confirmed_import_and_exact_id_replay_keep_one_committed_result() {
    let (_root, maintenance, home, source) = fixture();
    let operation = json!({"kind":"transcripts","agent":"codex"});
    let preview = maintenance
        .preview(
            None,
            &home,
            &serde_json::to_vec(&json!({
                "operation":operation
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(preview["candidates"]["codex"]["events"], 8);
    assert!(!home.exists());
    let request = serde_json::to_vec(&json!({"operation":operation,
            "preview_key":preview["preview_key"],"operation_id":"a".repeat(64),
            "confirmed":true}))
    .unwrap();
    let result = maintenance
        .start(executable::CommandCaller::Worker, &home, &request)
        .unwrap();
    assert_eq!(result["last"]["phase"], "complete");
    assert_eq!(
        result["last"]["result"]["outcome"]["transcripts"]["codex"]["events"],
        8
    );
    assert_eq!(result["last"]["committed"], true);
    let raw = crate::raw::read_only(&home).unwrap().unwrap();
    let before: i64 = raw
        .conn
        .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
        .unwrap();
    drop(raw);
    // A replay must return its receipt without reparsing a now-unreadable source.
    std::fs::write(source, "not a transcript").unwrap();
    assert_eq!(
        maintenance
            .start(executable::CommandCaller::Worker, &home, &request)
            .unwrap(),
        result
    );
    let raw = crate::raw::read_only(&home).unwrap().unwrap();
    let after: i64 = raw
        .conn
        .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    assert!(!home.join("providers.db").exists());
}

/// A source that is no longer what its preview bound waits (#947): the confirm completes
/// with none of it stored, and none of its new contents in the receipt.
#[test]
fn a_changed_source_waits_and_none_of_it_is_stored_or_shown() {
    let (_root, maintenance, home, source) = fixture();
    let operation = json!({"kind":"transcripts","agent":"codex"});
    let preview = maintenance
        .preview(
            None,
            &home,
            &serde_json::to_vec(&json!({
                "operation":operation
            }))
            .unwrap(),
        )
        .unwrap();
    std::fs::write(source, "unconfirmed source contents").unwrap();
    let request = serde_json::to_vec(&json!({"operation":operation,
            "preview_key":preview["preview_key"],"operation_id":"b".repeat(64),
            "confirmed":true}))
    .unwrap();
    let result = maintenance
        .start(executable::CommandCaller::Worker, &home, &request)
        .unwrap();
    assert_eq!(result["last"]["phase"], "complete", "{result}");
    let codex = &result["last"]["result"]["outcome"]["transcripts"]["codex"];
    assert_eq!(
        (&codex["files"], &codex["events"], &codex["waiting"]),
        (&json!(1), &json!(0), &json!(1)),
        "{result}"
    );
    assert_eq!(result["last"]["committed"], false);
    assert_eq!(records(&home), 0);
    assert!(!home.join("config.toml").exists());
    assert!(!result.to_string().contains("unconfirmed source contents"));
}

/// The records in the home's store; none where there is no store.
fn records(home: &Path) -> i64 {
    crate::raw::read_only(home).unwrap().map_or(0, |raw| {
        raw.conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .unwrap()
    })
}

/// A confirm imports of its preview's plan, which the viewer keeps and gives once (#947):
/// with none under the key the confirm is stale and stores nothing. An older preview's key
/// after a newer preview, a key another viewer process gave, and the same key a second time.
#[test]
fn a_confirm_without_its_previews_plan_is_stale() {
    let (root, maintenance, home, source) = fixture();
    let operation = json!({"kind":"transcripts","agent":"codex"});
    let preview_request = serde_json::to_vec(&json!({"operation":operation})).unwrap();
    let confirm = |maintenance: &Maintenance, key: &Value, id: char| {
        let request = serde_json::to_vec(&json!({"operation":operation,
                "preview_key":key,"operation_id":id.to_string().repeat(64),"confirmed":true}))
        .unwrap();
        maintenance
            .start(executable::CommandCaller::Worker, &home, &request)
            .unwrap()
    };
    let stale = |result: &Value| {
        assert_eq!(result["last"]["phase"], "failed", "{result}");
        assert_eq!(result["last"]["result"]["code"], "maintenance_stale");
        assert_eq!(result["last"]["committed"], false);
    };
    let older = maintenance.preview(None, &home, &preview_request).unwrap();
    another_session(&source);
    let newer = maintenance.preview(None, &home, &preview_request).unwrap();
    assert_ne!(older["preview_key"], newer["preview_key"]);
    stale(&confirm(&maintenance, &older["preview_key"], 'a'));
    assert_eq!(records(&home), 0);
    // A viewer started since never made the plan.
    let restarted = Maintenance::with_roots(
        root.path().join("claude/projects"),
        root.path().join("codex/sessions"),
    );
    stale(&confirm(&restarted, &newer["preview_key"], 'b'));
    assert_eq!(records(&home), 0);
    // A stale confirm leaves the newer plan where it is: its own confirm imports.
    let done = confirm(&maintenance, &newer["preview_key"], 'c');
    assert_eq!(done["last"]["phase"], "complete", "{done}");
    assert_eq!(records(&home), 16);
    // Given once: the same key again is stale, and stores nothing more.
    stale(&confirm(&maintenance, &newer["preview_key"], 'd'));
    assert_eq!(records(&home), 16);
}

const BASIC_SESSION: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_SESSION: &str = "44444444-4444-4444-8444-444444444444";

/// The fixture's transcript again as another session's, beside it.
fn another_session(source: &Path) -> PathBuf {
    let other = source.with_file_name("rollout-other.jsonl");
    let basic = std::fs::read_to_string(source).unwrap();
    std::fs::write(&other, basic.replace(BASIC_SESSION, OTHER_SESSION)).unwrap();
    other
}

/// A session at work writes one more prompt to its transcript.
fn grows(transcript: &Path) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(transcript)
        .unwrap();
    writeln!(
            file,
            r#"{{"timestamp":"2026-09-02T00:00:20.000Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"appended after the preview"}}]}}}}"#
        )
        .unwrap();
}

fn records_of(home: &Path, session: &str) -> i64 {
    let raw = crate::raw::read_only(home).unwrap().unwrap();
    raw.conn
        .query_row(
            "SELECT COUNT(*) FROM records WHERE session = ?1",
            [session],
            |r| r.get(0),
        )
        .unwrap()
}

/// #947, decision 100: a transcript a session wrote to since the preview is left for a later
/// import, and the confirm imports the rest. Nothing of the one that grew is stored.
#[test]
fn a_confirm_imports_the_rest_when_a_transcript_grew_since_its_preview() {
    let (_root, maintenance, home, source) = fixture();
    another_session(&source);
    let operation = json!({"kind":"transcripts","agent":"codex"});
    let preview = maintenance
        .preview(
            None,
            &home,
            &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
        )
        .unwrap();
    assert_eq!(preview["candidates"]["codex"]["events"], 16);
    grows(&source);
    let request = serde_json::to_vec(&json!({"operation":operation,
            "preview_key":preview["preview_key"],"operation_id":"d".repeat(64),
            "confirmed":true}))
    .unwrap();
    let result = maintenance
        .start(executable::CommandCaller::Worker, &home, &request)
        .unwrap();
    assert_eq!(result["last"]["phase"], "complete", "{result}");
    let codex = &result["last"]["result"]["outcome"]["transcripts"]["codex"];
    assert_eq!(
        (&codex["files"], &codex["events"], &codex["waiting"]),
        (&json!(2), &json!(8), &json!(1)),
        "{result}"
    );
    assert!(records_of(&home, OTHER_SESSION) > 0);
    assert_eq!(
        records_of(&home, BASIC_SESSION),
        0,
        "the one that grew waits"
    );
}

fn panic_after_parse(remaining: usize) {
    transcript::AFTER_PARSE.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            if remaining == 0 {
                panic!("controlled native unwind");
            }
            panic_after_parse(remaining - 1);
        }));
    });
}

#[test]
fn a_native_unwind_keeps_unknown_progress_and_releases_admission() {
    for earlier_commit in [false, true] {
        let (_root, maintenance, home, source) = fixture();
        if earlier_commit {
            let text = std::fs::read_to_string(&source).unwrap();
            std::fs::write(
                source.with_file_name("rollout-z-new.jsonl"),
                text.replace(
                    "22222222-2222-4222-8222-222222222222",
                    "44444444-4444-4444-8444-444444444444",
                )
                .replace(
                    "33333333-3333-4333-8333-333333333333",
                    "55555555-5555-4555-8555-555555555555",
                ),
            )
            .unwrap();
        }
        let preview_request = serde_json::to_vec(&json!({
            "operation":{"kind":"transcripts","agent":"codex"}
        }))
        .unwrap();
        let preview = maintenance.preview(None, &home, &preview_request).unwrap();
        let request = |key: &Value, id: char| {
            serde_json::to_vec(&json!({"operation":{"kind":"transcripts","agent":"codex"},
                    "preview_key":key,"operation_id":id.to_string().repeat(64),"confirmed":true}))
            .unwrap()
        };
        let original = request(&preview["preview_key"], 'c');
        // The first execution parse precedes the real first commit: a confirm imports of
        // its preview's plan, and parses nothing for a preview of its own (#947).
        panic_after_parse(usize::from(earlier_commit));
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            maintenance.start(executable::CommandCaller::Worker, &home, &original)
        }));
        assert!(unwind.is_err());
        let shown = maintenance.show(&home);
        assert!(shown["active"].is_null());
        assert_eq!(shown["last"]["phase"], "unknown");
        assert_eq!(shown["last"]["result"]["code"], "maintenance_unknown");
        assert!(shown["last"]["result"]["outcome"].is_null());
        assert_eq!(shown["last"]["committed"], earlier_commit);
        assert_eq!(
            shown["last"]["progress"]["codex"]["events"],
            if earlier_commit { 8 } else { 0 }
        );
        assert_eq!(
            maintenance
                .start(executable::CommandCaller::Worker, &home, &original)
                .unwrap(),
            shown
        );
        let fresh = maintenance.preview(None, &home, &preview_request).unwrap();
        let finished = maintenance
            .start(
                executable::CommandCaller::Worker,
                &home,
                &request(&fresh["preview_key"], 'd'),
            )
            .unwrap();
        assert_eq!(finished["last"]["phase"], "complete");
        assert_eq!(finished["last"]["progress"]["codex"]["events"], 8);
        let raw = crate::raw::read_only(&home).unwrap().unwrap();
        let records: i64 = raw
            .conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(records, if earlier_commit { 16 } else { 8 });
    }
}

#[test]
fn a_normal_old_guard_cannot_clear_a_reused_id() {
    let (_root, maintenance, home, _source) = fixture();
    let maintenance = std::sync::Arc::new(maintenance);
    let prepare = |id: char| {
        let operation = json!({"kind":"transcripts","agent":"codex"});
        let preview = maintenance
            .preview(
                None,
                &home,
                &serde_json::to_vec(&json!({"operation":operation})).unwrap(),
            )
            .unwrap();
        serde_json::to_vec(
            &json!({"operation":operation,"preview_key":preview["preview_key"],
                "operation_id":id.to_string().repeat(64),"confirmed":true}),
        )
        .unwrap()
    };
    maintenance
        .start(executable::CommandCaller::Worker, &home, &prepare('c'))
        .unwrap();
    let old_id = "c".repeat(64);
    // Delay the first normal guard drop until another receipt evicts its ID.
    let old = ActiveRun {
        maintenance: &maintenance,
        id: &old_id,
    };
    maintenance
        .start(executable::CommandCaller::Worker, &home, &prepare('d'))
        .unwrap();
    let reused = prepare('c');
    let (ready, observed) = std::sync::mpsc::channel();
    let (release, waiting) = std::sync::mpsc::channel();
    let running = maintenance.clone();
    let running_home = home.clone();
    let thread = std::thread::spawn(move || {
        transcript::AFTER_PARSE.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                ready.send(()).unwrap();
                waiting.recv().unwrap();
            }));
        });
        running.start(executable::CommandCaller::Worker, &running_home, &reused)
    });
    observed
        .recv_timeout(std::time::Duration::from_secs(20))
        .unwrap();
    drop(old);
    let while_running = maintenance.show(&home);
    release.send(()).unwrap();
    let finished = thread.join().unwrap().unwrap();
    assert_eq!(while_running["active"]["operation_id"], old_id);
    assert_eq!(while_running["active"]["phase"], "running");
    assert_eq!(finished["last"]["phase"], "complete");
}

#[test]
fn a_missing_default_v1_source_is_distinct_and_creates_no_store() {
    let (_root, maintenance, home, _source) = fixture();
    for from in [Value::Null, json!("")] {
        let body = serde_json::to_vec(&json!({"operation":{"kind":"v1","from":from}})).unwrap();
        let refused = maintenance.preview(None, &home, &body).unwrap_err();
        assert_eq!(refused.code, "maintenance_source_missing");
        assert!(!home.exists());
        assert_eq!(
            maintenance.show(&home),
            json!({"available":false,"active":null,"last":null})
        );
    }
    let body = serde_json::to_vec(
        &json!({"operation":{"kind":"v1","from":home.join("explicit-missing.db")}}),
    )
    .unwrap();
    assert_eq!(
        maintenance.preview(None, &home, &body).unwrap_err().code,
        "maintenance_preview_failed"
    );
    assert!(!home.exists());
}
