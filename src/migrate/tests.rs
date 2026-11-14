use super::*;
use crate::raw::{self, Event, Item, OpKind};
use rusqlite::params;
use std::path::PathBuf;

#[test]
fn w5c_confirmed_finish_creates_raw_without_invalidating_its_own_consent() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let v1 = V1::new(h);
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, "finish into fresh raw");
    drop(v1); // Close v1 before preview/deletion, including on Windows.
    std::fs::write(
        h.join("config.toml"),
        "providers = []\n[summary]\ncurate = false\n",
    )
    .unwrap();
    assert!(!h.join("raw.db").exists());
    let shown = preview_finish(h).unwrap();
    assert_eq!(shown.deletion.targets, ["oboete.db"]);
    assert_eq!(
        (
            shown.candidates.events,
            shown.candidates.records,
            shown.candidates.repos,
            shown.candidates.documents,
        ),
        (1, 1, 1, 0)
    );
    assert!(!h.join("raw.db").exists());

    let outcome = finish_report(
        h,
        &shown.key,
        crate::executable::CommandCaller::Worker,
        &mut |_| {},
    )
    .unwrap();

    assert!(outcome.committed());
    assert!(outcome.effects.stores_changed);
    assert_eq!(
        (
            outcome.stats.events,
            outcome.stats.records,
            outcome.stats.repos,
            outcome.stats.documents,
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        (
            outcome.deletion.selected,
            outcome.deletion.attempted,
            outcome.deletion.removed,
            outcome.deletion.failed,
            outcome.deletion.uncertain,
        ),
        (1, 1, 1, 0, 0)
    );
    assert!(matches!(
        outcome.deletion.targets[0].state,
        FinishTargetState::Removed
    ));
    let raw = raw::open(h).unwrap();
    assert_eq!(
        of_kind(&raw, "prompt"),
        [json!({"prompt": "finish into fresh raw"})]
    );
    assert_eq!(of_kind(&raw, "touch").len(), 1);
    assert_eq!(records(&raw).len(), 2);
    assert!(!h.join("oboete.db").exists());
}

#[test]
fn w5c_confirmed_finish_keeps_imports_when_inputs_change_after_event_commit() {
    for changed in ["source", "cache", "config"] {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let v1 = V1::new(h);
        v1.session("s1", "r", 100);
        v1.prompt("s1", 110, "committed before input changed");
        // This case deliberately retains an old writer; all three paths must refuse deletion.
        let config = h.join("config.toml");
        std::fs::write(&config, "providers = []\n[summary]\ncurate = false\n").unwrap();
        std::fs::create_dir(h.join("cache")).unwrap();
        let child = h.join("cache/selected.txt");
        std::fs::write(&child, b"before").unwrap();
        let shown = preview_finish(h).unwrap();
        assert!(
            shown
                .deletion
                .targets
                .iter()
                .any(|target| target == "cache")
        );
        let mut mutated = false;
        let failure = finish_report(
            h,
            &shown.key,
            crate::executable::CommandCaller::Worker,
            &mut |event| {
                if mutated
                    || !matches!(
                        event,
                        FinishCommit::Import(Committed {
                            stage: Stage::Events,
                            ..
                        })
                    )
                {
                    return;
                }
                mutated = true;
                match changed {
                    "source" => {
                        v1.prompt("s1", 120, "late source record");
                    }
                    "cache" => std::fs::write(&child, b"after!").unwrap(),
                    "config" => std::fs::write(
                        &config,
                        "providers = []\n[summary]\ncurate = false\n# changed after import\n",
                    )
                    .unwrap(),
                    _ => unreachable!(),
                }
            },
        )
        .unwrap_err();

        assert_eq!(failure.code, FailureCode::Stale, "{changed}");
        let outcome = &failure.outcome;
        assert!(outcome.committed(), "{changed}");
        assert_eq!(
            (
                outcome.stats.events,
                outcome.stats.records,
                outcome.stats.repos,
                outcome.stats.documents,
            ),
            (1, 1, 1, 0),
            "{changed}"
        );
        assert_eq!(
            (
                outcome.deletion.attempted,
                outcome.deletion.removed,
                outcome.deletion.failed,
                outcome.deletion.uncertain,
            ),
            (0, 0, 0, 0),
            "{changed}"
        );
        let raw = raw::open(h).unwrap();
        assert_eq!(
            of_kind(&raw, "prompt"),
            [json!({"prompt": "committed before input changed"})],
            "{changed}"
        );
        assert_eq!(of_kind(&raw, "touch").len(), 1, "{changed}");
        assert_eq!(records(&raw).len(), 2, "{changed}");
        assert!(
            mutated,
            "the admitted Events callback was not reached: {changed}"
        );
        assert!(h.join("oboete.db").is_file(), "{changed}");
        assert!(child.is_file(), "{changed}");
        assert!(config.is_file(), "{changed}");
    }
}

#[test]
fn w5c_confirmed_finish_checks_remaining_content_after_deleting_the_source() {
    use std::io::Write;

    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let v1 = V1::new(h);
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, "committed before partial deletion");
    drop(v1); // No original v1 handle may prevent the first deletion on Windows.
    std::fs::write(
        h.join("config.toml"),
        "providers = []\n[summary]\ncurate = false\n",
    )
    .unwrap();
    let memory = h.join("memory.db");
    std::fs::write(&memory, b"before").unwrap();
    let modified = std::fs::metadata(&memory).unwrap().modified().unwrap();
    let shown = preview_finish(h).unwrap();
    assert_eq!(shown.deletion.targets, ["memory.db", "oboete.db"]);
    let mut mutated = false;
    let mut source_was_absent = false;
    let failure = finish_report(
        h,
        &shown.key,
        crate::executable::CommandCaller::Worker,
        &mut |event| {
            if mutated
                || !matches!(
                    event,
                    FinishCommit::Deletion {
                        attempted: 1,
                        removed: 1,
                        ..
                    }
                )
            {
                return;
            }
            source_was_absent = !h.join("oboete.db").exists();
            let mut file = std::fs::File::options().write(true).open(&memory).unwrap();
            file.write_all(b"after!").unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
            mutated = true;
        },
    )
    .unwrap_err();

    assert_eq!(failure.code, FailureCode::Stale);
    let outcome = &failure.outcome;
    assert!(outcome.committed());
    assert_eq!(
        (
            outcome.stats.events,
            outcome.stats.records,
            outcome.stats.repos,
            outcome.stats.documents,
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        (
            outcome.deletion.selected,
            outcome.deletion.attempted,
            outcome.deletion.removed,
            outcome.deletion.failed,
            outcome.deletion.uncertain,
        ),
        (2, 1, 1, 0, 0)
    );
    let source_target = outcome
        .deletion
        .targets
        .iter()
        .find(|target| target.label == "oboete.db")
        .unwrap();
    let memory_target = outcome
        .deletion
        .targets
        .iter()
        .find(|target| target.label == "memory.db")
        .unwrap();
    assert!(matches!(source_target.state, FinishTargetState::Removed));
    assert!(matches!(memory_target.state, FinishTargetState::Changed));
    let raw = raw::open(h).unwrap();
    assert_eq!(
        of_kind(&raw, "prompt"),
        [json!({"prompt": "committed before partial deletion"})]
    );
    assert_eq!(of_kind(&raw, "touch").len(), 1);
    assert_eq!(records(&raw).len(), 2);
    assert!(mutated && source_was_absent);
    // Changed, rather than NotAttempted, proves the remaining-target check ran after source removal.
    assert!(!h.join("oboete.db").exists());
    assert_eq!(std::fs::read(&memory).unwrap(), b"after!");
    let kept = std::fs::metadata(&memory).unwrap();
    assert_eq!(kept.len(), 6);
    assert_eq!(kept.modified().unwrap(), modified);
}

#[cfg(target_os = "linux")]
#[test]
fn w5c_finish_preview_treats_links_as_links_and_refuses_fifo_and_backup_overlap() {
    use std::io::Write;
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let v1 = V1::new(h);
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, "synthetic finish preview");
    assert!(!h.join("config.toml").exists());
    let db_before = std::fs::read(h.join("oboete.db")).unwrap();
    let wal_before = std::fs::read(h.join("oboete.db-wal")).unwrap();
    assert!(!wal_before.is_empty());
    let stable_source = || {
        assert_eq!(std::fs::read(h.join("oboete.db")).unwrap(), db_before);
        assert_eq!(std::fs::read(h.join("oboete.db-wal")).unwrap(), wal_before);
        assert!(!h.join("raw.db").exists() && !h.join("state").exists());
    };
    let bare = preview_finish(h).unwrap();
    assert_eq!(
        (
            bare.candidates.events,
            bare.candidates.records,
            bare.candidates.repos,
            bare.candidates.documents
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        bare.deletion.targets,
        ["oboete.db", "oboete.db-shm", "oboete.db-wal"]
    );
    stable_source();
    assert!(!h.join("config.toml").exists());

    // The old-name selector includes memory.db-wal, but finish must bind this
    // selected link itself and must never hash/open its outside target.
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("not-an-old-store");
    std::fs::write(&outside_file, b"outside-one").unwrap();
    let link = h.join("memory.db-wal");
    std::os::unix::fs::symlink(&outside_file, &link).unwrap();
    let shown = preview_finish(h).unwrap();
    assert_ne!(shown.key, bare.key);
    assert_eq!(
        (
            shown.candidates.events,
            shown.candidates.records,
            shown.candidates.repos,
            shown.candidates.documents
        ),
        (1, 1, 1, 0)
    );
    assert!(
        shown
            .deletion
            .targets
            .iter()
            .any(|name| name == "memory.db-wal")
    );
    assert_eq!(std::fs::read_link(&link).unwrap(), outside_file);
    assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside-one");
    stable_source();
    assert!(!h.join("config.toml").exists());

    // Same-length outside edit changes only the link target's contents. The
    // target is retained; the link entry, its label and its literal target do
    // not change, so this edit must not change finish consent.
    std::fs::write(&outside_file, b"outside-two").unwrap();
    let changed_outside = preview_finish(h).unwrap();
    assert_eq!(changed_outside.key, shown.key);
    assert_eq!(changed_outside.deletion, shown.deletion);
    assert_eq!(std::fs::read_link(&link).unwrap(), outside_file);
    assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside-two");
    stable_source();
    assert!(!h.join("config.toml").exists());

    // A special descendant beneath a selected directory is refused before
    // open/read. Bound the call: a regression to blocking FIFO reads must fail
    // rather than leave a stuck test thread.
    std::fs::create_dir(h.join("cache")).unwrap();
    let pipe = h.join("cache/selected-pipe");
    let name = std::ffi::CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: the fixture path is owned, NUL-terminated and alive for mkfifo.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (send, receive) = std::sync::mpsc::channel();
    let owned_home = h.to_path_buf();
    let call = std::thread::spawn(move || {
        send.send(preview_finish(&owned_home)).unwrap();
    });
    let first = receive.recv_timeout(std::time::Duration::from_secs(5));
    let did_not_block = first.is_ok();
    let result = match first {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Rescue only this owned FIFO if a faulty reader is waiting for a
            // writer. Close it before collecting the thread's terminal result.
            let mut rescue = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&pipe)
                .unwrap();
            rescue.write_all(b"x").unwrap();
            drop(rescue);
            receive
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        }
        Err(error) => panic!("owned finish preview thread disconnected: {error}"),
    };
    call.join().unwrap();
    assert!(did_not_block, "finish preview blocked on selected FIFO");
    assert!(
        format!("{:#}", result.unwrap_err())
            .contains("finish target is not a file, directory or link")
    );
    assert!(
        std::fs::symlink_metadata(&pipe)
            .unwrap()
            .file_type()
            .is_fifo()
    );
    assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside-two");
    stable_source();
    assert!(!h.join("config.toml").exists());

    // Remove the special fixture entry before the next independent refusal.
    std::fs::remove_file(&pipe).unwrap();
    std::fs::create_dir_all(h.join("cache/native")).unwrap();
    let kept_backup = h.join("cache/native/retained-segment");
    std::fs::write(&kept_backup, b"synthetic native backup marker").unwrap();
    std::fs::write(h.join("config.toml"), "[backup]\ndir = 'cache/native'\n").unwrap();
    let config_before = std::fs::read(h.join("config.toml")).unwrap();
    let backup_before = std::fs::read(&kept_backup).unwrap();
    let refused = preview_finish(h).unwrap_err();
    assert!(format!("{refused:#}").contains("finish target overlaps native backups"));
    assert_eq!(std::fs::read(h.join("config.toml")).unwrap(), config_before);
    assert_eq!(std::fs::read(&kept_backup).unwrap(), backup_before);
    assert_eq!(std::fs::read_link(&link).unwrap(), outside_file);
    assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside-two");
    stable_source();
}

#[test]
fn w5c_finish_preview_keeps_live_wal_and_binds_empty_directory_identity() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let v1 = V1::new(h);
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, "one");
    v1.observation("s1", 120, "Tabs", "Use tabs.");
    std::fs::write(
        h.join("config.toml"),
        "providers = []\n[summary]\ncurate = false\n",
    )
    .unwrap();
    std::fs::write(h.join("pre-1.db"), "x").unwrap();
    std::fs::create_dir_all(h.join("cache/empty")).unwrap();
    std::fs::create_dir_all(h.join("cache/deep")).unwrap();
    std::fs::write(h.join("cache/deep/f"), "xyz").unwrap();
    std::fs::create_dir(h.join("eval")).unwrap();
    std::fs::write(h.join("eval/kept"), "evaluation kept").unwrap();
    let before = crate::backup::tests::w5b_files(h);
    let shown = preview_finish(h).unwrap();
    assert_eq!(
        (
            shown.candidates.events,
            shown.candidates.records,
            shown.candidates.repos,
            shown.candidates.documents
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(
        shown.deletion.targets,
        [
            "cache",
            "oboete.db",
            "oboete.db-shm",
            "oboete.db-wal",
            "pre-1.db"
        ]
    );
    assert_eq!(shown.deletion.nodes, 8);
    let source_bytes: u64 = ["oboete.db", "oboete.db-wal", "oboete.db-shm"]
        .iter()
        .map(|name| std::fs::metadata(h.join(name)).unwrap().len())
        .sum();
    assert_eq!(shown.deletion.bytes, source_bytes + 4);
    assert_eq!(preview_finish(h).unwrap().key, shown.key);
    assert_eq!(crate::backup::tests::w5b_files(h), before);
    assert!(!h.join("raw.db").exists() && !h.join("state").exists());

    // Keep the old directory alive so a recreated empty one cannot reuse its inode.
    let retired = tempfile::tempdir().unwrap();
    std::fs::rename(h.join("cache/empty"), retired.path().join("empty")).unwrap();
    std::fs::create_dir(h.join("cache/empty")).unwrap();
    let replaced = preview_finish(h).unwrap();
    assert_ne!(
        replaced.key, shown.key,
        "a replaced empty directory inherited consent"
    );
    assert_eq!(replaced.deletion.targets, shown.deletion.targets);
    assert_eq!(replaced.deletion.nodes, shown.deletion.nodes);
    assert_eq!(replaced.deletion.bytes, shown.deletion.bytes);
}

#[test]
fn readonly_preview_counts_wal_candidates_and_config_without_creating_home() {
    let source = tempfile::tempdir().unwrap();
    let v1 = V1::new(source.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 200, "one");
    v1.observation("s1", 210, "Tabs", "Use tabs.");
    let config = b"# preserve these bytes\n[capture]\nstore_prompts = false\n";
    std::fs::write(source.path().join("config.toml"), config).unwrap();
    let bytes = |p: &Path| std::fs::read(p).unwrap();
    let wal = v1.path.with_file_name("oboete.db-wal");
    let before = (bytes(&v1.path), bytes(&wal));
    let destination = tempfile::tempdir().unwrap();
    let home = destination.path().join("absent");
    let shown = preview(&home, &v1.path).unwrap();
    assert_eq!(
        (
            shown.candidates.events,
            shown.candidates.records,
            shown.candidates.repos,
            shown.candidates.documents
        ),
        (1, 1, 1, 1)
    );
    assert!(shown.candidates.bytes > 0);
    assert_eq!(shown.settings.effect, SettingsEffect::Copy);
    assert_eq!(shown.key.len(), 64);
    assert_eq!(
        shown.settings.missing,
        [
            MissingSetting::Curate,
            MissingSetting::Inject,
            MissingSetting::Redaction,
            MissingSetting::Chain
        ]
    );
    assert!(!home.exists());
    assert_eq!((bytes(&v1.path), bytes(&wal)), before);
    assert_eq!(bytes(&source.path().join("config.toml")), config);
}

#[test]
fn confirmed_migration_refuses_changed_wal_and_config_before_effects() {
    for changed_config in [false, true] {
        let source = tempfile::tempdir().unwrap();
        let v1 = V1::new(source.path());
        v1.session("s1", "r", 100);
        v1.prompt("s1", 200, "one");
        std::fs::write(source.path().join("config.toml"), "# original\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let shown = preview(home.path(), &v1.path).unwrap();
        if changed_config {
            std::fs::write(source.path().join("config.toml"), "# changed\n").unwrap();
        } else {
            v1.conn
                .execute("UPDATE events SET payload = '{\"prompt\":\"two\"}'", [])
                .unwrap();
        }
        let mut progress = Vec::new();
        let failed = run(home.path(), &v1.path, Some(&shown.key), &mut |c| {
            progress.push(c.clone())
        })
        .unwrap_err();
        assert_eq!(failed.code, FailureCode::Stale);
        assert_eq!(failed.outcome.stats.records, 0);
        assert!(failed.outcome.settings.is_none() && progress.is_empty());
        assert!(!crate::raw::path(home.path()).exists());
        assert!(!home.path().join("config.toml").exists());
    }
}

#[test]
fn composite_preview_discloses_conditional_v1_candidates_without_writes() {
    let home = tempfile::tempdir().unwrap();
    let v1 = V1::new(home.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 200, "one");
    v1.observation("s1", 210, "Tabs", "Use tabs.");
    let source = tempfile::tempdir().unwrap();
    std::fs::copy(
        "src/testdata/transcripts/codex-basic.jsonl",
        source.path().join("rollout-basic.jsonl"),
    )
    .unwrap();
    let shown = crate::transcript::preview_plan(home.path(), &[("codex", source.path())]).unwrap();
    assert_eq!(shown.preview.candidates.agents["codex"].events, 8);
    let migration = (shown.preview.v1.as_ref()).expect("transcript consent must include v1");
    assert_eq!(
        (migration.candidates.records, migration.candidates.documents),
        (1, 1)
    );
    assert_eq!(migration.settings.effect, SettingsEffect::Defaults);
    assert!(!crate::raw::path(home.path()).exists());
    assert!(!home.path().join("state").exists());
    let mut stages = Vec::new();
    let imported = crate::transcript::run(
        home.path(),
        &[("codex", source.path())],
        Some(shown),
        &mut |c| match c {
            crate::transcript::Committed::V1(c) => stages.push(format!("v1:{:?}", c.stage)),
            crate::transcript::Committed::Transcripts {
                agent,
                events,
                bytes,
            } => {
                assert_eq!(agent, "codex");
                assert_eq!(*events, 8);
                assert!(*bytes > 0);
                stages.push("transcripts".to_owned());
            }
        },
    )
    .unwrap();
    assert_eq!(imported.stats.agents["codex"].events, 8);
    assert_eq!(imported.v1.unwrap().stats.documents, 1);
    assert_eq!(
        stages,
        ["v1:Events", "v1:Repos", "v1:Documents", "transcripts"]
    );
}

#[test]
fn typed_migration_keeps_settings_and_committed_event_or_document_batches() {
    for stop in [Stage::Events, Stage::Documents] {
        let source = tempfile::tempdir().unwrap();
        let v1 = V1::new(source.path());
        v1.session("s1", "r", 100);
        for i in 0..600 {
            v1.prompt("s1", 200 + i, "prompt");
        }
        for i in 0..100 {
            v1.observation("s1", 1000 + i, "Title", &"x".repeat(60_000));
        }
        std::fs::write(source.path().join("config.toml"), "# original\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let shown = preview(home.path(), &v1.path).unwrap();
        let mut progress = Vec::new();
        let failed = run(home.path(), &v1.path, Some(&shown.key), &mut |c| {
            progress.push(c.clone());
            if c.stage == stop {
                crate::crash::at(1);
            }
        })
        .unwrap_err();
        crate::crash::off();
        assert_eq!(
            failed.outcome.settings.unwrap().effect,
            SettingsEffect::Copy
        );
        let raw = raw::open(home.path()).unwrap();
        let last = progress.last().unwrap();
        let stats = failed.outcome.stats;
        assert_eq!(
            (last.records, last.repos, last.documents),
            (stats.records, stats.repos, stats.documents)
        );
        assert_eq!(records(&raw).len() as u64, stats.records + stats.repos);
        assert_eq!(
            raw.import_keys("oboete-v1:d1e5").unwrap().len() as u64,
            stats.documents
        );
        if stop == Stage::Events {
            assert_eq!(stats.records, 500);
        } else {
            assert!(stats.documents > 0 && stats.documents < 100);
        }
        drop(raw);
        let current = preview(home.path(), &v1.path).unwrap();
        let resumed = run(home.path(), &v1.path, Some(&current.key), &mut |_| {}).unwrap();
        assert_eq!(stats.records + resumed.stats.records, 600);
        assert_eq!(stats.documents + resumed.stats.documents, 100);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn confirmed_migration_rechecks_config_after_waiting_for_its_writer() {
    let source = tempfile::tempdir().unwrap();
    let v1 = V1::new(source.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 200, "one");
    std::fs::write(source.path().join("config.toml"), "# source\n").unwrap();
    let home = tempfile::tempdir().unwrap();
    let shown = preview(home.path(), &v1.path).unwrap();
    let fence = crate::settings::config_lock(home.path()).unwrap();
    let (dest, from) = (home.path().to_owned(), v1.path.clone());
    let thread = std::thread::Builder::new()
        .name("w5-migrate".into())
        .spawn(move || run(&dest, &from, Some(&shown.key), &mut |_| {}))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let waiting = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .flatten()
            .any(|task| {
                std::fs::read_to_string(task.path().join("comm"))
                    .is_ok_and(|s| s.trim() == "w5-migrate")
                    && std::fs::read_to_string(task.path().join("wchan"))
                        .is_ok_and(|s| s.contains("locks_") || s.contains("flock_"))
            });
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "migration never waited for config.lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!raw::path(home.path()).exists());
    std::fs::write(home.path().join("config.toml"), "# chosen by writer\n").unwrap();
    drop(fence);
    let failed = thread.join().unwrap().unwrap_err();
    assert_eq!(failed.code, FailureCode::Stale);
    assert!(failed.outcome.settings.is_none());
    assert!(!raw::path(home.path()).exists());
}

#[test]
fn composite_failure_retains_v1_commits_before_transcripts_begin() {
    let home = tempfile::tempdir().unwrap();
    let v1 = V1::new(home.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 200, "one");
    let shown = crate::transcript::preview_plan(home.path(), &[]).unwrap();
    let failed = crate::transcript::run(home.path(), &[], Some(shown), &mut |c| {
        if matches!(
            c,
            crate::transcript::Committed::V1(Committed {
                stage: Stage::Events,
                ..
            })
        ) {
            crate::crash::at(1);
        }
    })
    .unwrap_err();
    crate::crash::off();
    let imported = failed.outcome.v1.unwrap();
    assert_eq!((imported.stats.records, imported.stats.repos), (1, 0));
    assert!(imported.settings.is_some());
    assert!(failed.outcome.stats.agents.is_empty());
}

/// A transcript confirm is of what its preview showed of the older store too. It makes no
/// preview of its own (#947), so each of these three looks stands alone. An older store that
/// appeared since the preview is not migrated.
#[test]
fn a_transcript_confirm_stops_at_an_older_store_its_preview_did_not_show() {
    let home = tempfile::tempdir().unwrap();
    let shown = crate::transcript::preview_plan(home.path(), &[]).unwrap();
    assert!(shown.preview.v1.is_none());
    let v1 = V1::new(home.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 200, "one");
    let failed =
        crate::transcript::run(home.path(), &[], Some(shown), &mut |_| panic!("no commits"))
            .unwrap_err();
    assert_eq!(failed.code, FailureCode::Stale);
    assert!(failed.outcome.v1.is_none());
    assert!(!crate::raw::path(home.path()).exists());
}

/// One that changed since the preview is not migrated either: the migration is held to the
/// preview of it that the transcript preview showed.
#[test]
fn a_transcript_confirm_stops_at_an_older_store_that_changed_since_its_preview() {
    let home = tempfile::tempdir().unwrap();
    let v1 = V1::new(home.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 200, "one");
    let shown = crate::transcript::preview_plan(home.path(), &[]).unwrap();
    assert!(shown.preview.v1.is_some());
    v1.conn
        .execute("UPDATE events SET payload = '{\"prompt\":\"two\"}'", [])
        .unwrap();
    let failed =
        crate::transcript::run(home.path(), &[], Some(shown), &mut |_| panic!("no commits"))
            .unwrap_err();
    assert_eq!(failed.code, FailureCode::Stale);
    assert_eq!(failed.outcome.v1.map(|v1| v1.stats.records), Some(0));
    assert!(!crate::raw::path(home.path()).exists());
}

/// And one the preview showed that is gone stops the confirm: its transcripts are not
/// imported without the migration that was shown with them.
#[test]
fn a_transcript_confirm_stops_when_the_older_store_its_preview_showed_is_gone() {
    let home = tempfile::tempdir().unwrap();
    let v1 = V1::new(home.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 200, "one");
    let shown = crate::transcript::preview_plan(home.path(), &[]).unwrap();
    assert!(shown.preview.v1.is_some());
    let path = v1.path.clone();
    drop(v1);
    std::fs::remove_file(&path).unwrap();
    for rest in ["oboete.db-wal", "oboete.db-shm"] {
        let _ = std::fs::remove_file(home.path().join(rest));
    }
    let failed =
        crate::transcript::run(home.path(), &[], Some(shown), &mut |_| panic!("no commits"))
            .unwrap_err();
    assert_eq!(failed.code, FailureCode::Stale);
    assert!(failed.outcome.v1.is_none());
    assert!(!crate::raw::path(home.path()).exists());
}

/// v1's tables as its `db.rs` left them, `uid` columns included, in plain SQL: a fixture no
/// Design B code writes.
const V1_SCHEMA: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT NOT NULL, repo TEXT NOT NULL, cwd TEXT,
  started_at INTEGER NOT NULL, ended_at INTEGER, last_event_at INTEGER NOT NULL,
  injected_at INTEGER, last_prompt_step INTEGER, reinject_pending INTEGER NOT NULL DEFAULT 0,
  observed_event_id INTEGER NOT NULL DEFAULT 0);
CREATE TABLE session_repos(session_id TEXT NOT NULL, repo TEXT NOT NULL,
  PRIMARY KEY(session_id, repo)) WITHOUT ROWID;
CREATE TABLE events(id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, event TEXT NOT NULL,
  ts INTEGER NOT NULL, payload TEXT NOT NULL);
CREATE INDEX events_session ON events(session_id, id);
CREATE TABLE observations(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
  repo TEXT NOT NULL, ts INTEGER NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL,
  body TEXT NOT NULL, provider TEXT NOT NULL, uid TEXT);
CREATE TABLE summaries(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
  repo TEXT NOT NULL, ts INTEGER NOT NULL, body TEXT NOT NULL, provider TEXT NOT NULL, uid TEXT);
CREATE TABLE prompts(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
  repo TEXT NOT NULL, ts INTEGER NOT NULL, body TEXT NOT NULL, uid TEXT);
INSERT INTO meta VALUES('device_id', 'd1e5');
";

/// A v1 store at `<dir>/oboete.db`, in WAL mode as v1 ran.
struct V1 {
    path: PathBuf,
    conn: Connection,
}

impl V1 {
    fn new(dir: &Path) -> Self {
        let path = dir.join("oboete.db");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        Self { path, conn }
    }

    /// A Claude Code session in `repo`, with the `session_repos` row v1 wrote for it.
    fn session(&self, id: &str, repo: &str, started: i64) {
        self.conn
            .execute(
                "INSERT INTO sessions(id, agent, repo, cwd, started_at, last_event_at)
                     VALUES(?1, 'claude', ?2, ?3, ?4, ?4)",
                params![id, repo, format!("/w/{id}"), started],
            )
            .unwrap();
        self.touched(id, repo);
    }

    fn touched(&self, session: &str, repo: &str) {
        self.conn
            .execute(
                "INSERT INTO session_repos VALUES(?1, ?2)",
                params![session, repo],
            )
            .unwrap();
    }

    fn event(&self, session: &str, event: &str, ts: i64, stored: Value) -> i64 {
        self.conn
            .execute(
                "INSERT INTO events(session_id, event, ts, payload) VALUES(?1, ?2, ?3, ?4)",
                params![session, event, ts, stored.to_string()],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }

    fn prompt(&self, session: &str, ts: i64, text: &str) -> i64 {
        self.event(session, "UserPromptSubmit", ts, json!({"prompt": text}))
    }

    fn observation(&self, session: &str, ts: i64, title: &str, body: &str) {
        self.conn
            .execute(
                "INSERT INTO observations(session_id, repo, ts, kind, title, body, provider)
                     VALUES(?1, 'github.com/o/r', ?2, 'decision', ?3, ?4, 'groq')",
                params![session, ts, title, body],
            )
            .unwrap();
    }

    /// What the old viewer's `db::delete_session` takes.
    fn delete_session(&self, id: &str) {
        for sql in [
            "DELETE FROM events WHERE session_id = ?1",
            "DELETE FROM session_repos WHERE session_id = ?1",
            "DELETE FROM sessions WHERE id = ?1",
        ] {
            self.conn.execute(sql, [id]).unwrap();
        }
    }
}

/// Design B's events, in seq order.
fn records(raw: &Raw) -> Vec<Event> {
    let (mut out, mut after) = (Vec::new(), 0);
    loop {
        let page = raw.after(raw.device(), after, 1_000).unwrap();
        let Some(last) = page.last() else {
            return out;
        };
        after = last.seq;
        out.extend(page.into_iter().filter_map(|r| match r.item {
            Item::Event(e) => Some(*e),
            _ => None,
        }));
    }
}

fn body(e: &Event) -> Value {
    serde_json::from_str(&e.body).unwrap()
}

fn of_kind(raw: &Raw, kind: &str) -> Vec<Value> {
    records(raw)
        .iter()
        .filter(|e| e.kind == kind)
        .map(body)
        .collect()
}

/// D6: v1's events become `oboete-v1` records of B's kinds with v1's session, agent, repo and
/// cwd, in (ts, id) order within a batch, redacted with the rules as they are now; with prompts
/// off, a prompt is a turn without its text.
#[test]
fn v1_events_become_oboete_v1_records() {
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "github.com/o/r", 100);
    v1.event("s1", "SessionStart", 150, json!({"source": "resume"}));
    v1.prompt("s1", 300, "use tabs");
    let tool = json!({"tool": "Bash", "input": "{\"command\":\"ls\"}", "output": "ok",
            "failed": false});
    v1.event("s1", "PostToolUse", 200, tool);
    let reply = json!({"assistant": format!("done with {token}")});
    v1.event("s1", "Stop", 400, reply);
    let failed = json!({"tool": "Bash", "input": "x", "output": "boom", "failed": true});
    v1.event("s1", "PostToolUseFailure", 500, failed);
    v1.event("s1", "PostCompact", 550, json!({"summary": "compacted"}));
    v1.event("s1", "SessionEnd", 600, json!({"reason": "exit"}));
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((stats.events, stats.records, stats.repos), (7, 7, 1));
    let got = records(&raw);
    let kinds: Vec<(&str, i64)> = got.iter().map(|e| (e.kind.as_str(), e.ts)).collect();
    assert_eq!(
        kinds,
        [
            ("start", 150),
            ("tool", 200),
            ("prompt", 300),
            ("reply", 400),
            ("tool", 500),
            ("compaction", 550),
            ("end", 600),
            ("touch", 100)
        ]
    );
    for e in &got {
        let labels = (
            e.agent.as_str(),
            e.session.as_str(),
            e.repo.as_deref(),
            e.cwd.as_deref(),
            e.source.as_str(),
        );
        let want = (
            "claude",
            "s1",
            Some("github.com/o/r"),
            Some("/w/s1"),
            "oboete-v1",
        );
        assert_eq!(labels, want);
        assert_eq!((&e.branch, &e.head), (&None, &None));
    }
    assert_eq!(body(&got[0]), json!({"source": "resume"}));
    let tool = body(&got[1]);
    let tool = (tool["tool"].as_str(), tool["output"].as_str());
    assert_eq!(tool, (Some("Bash"), Some("ok")));
    assert_eq!(body(&got[2]), json!({"prompt": "use tabs"}));
    assert!(!got[3].body.contains(&token) && got[3].body.contains("[REDACTED]"));
    assert_eq!(body(&got[4])["failed"], true);
    assert_eq!(body(&got[5]), json!({"summary": "compacted"}));
    assert_eq!(body(&got[6]), json!({"reason": "exit"}));
    assert_eq!(got[7].body, "{}");
    // A store without v1's device id is not v1's: refused before anything is read.
    v1.conn.execute("DELETE FROM meta", []).unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut raw = raw::open(other.path()).unwrap();
    let refused = pass(other.path(), &mut raw, &v1.path).unwrap_err();
    assert!(
        format!("{refused:#}").contains("not a v1 store"),
        "{refused:#}"
    );
    assert_eq!(raw.max_seq().unwrap(), 0);
    v1.conn
        .execute("INSERT INTO meta VALUES('device_id', 'd1e5')", [])
        .unwrap();
    // With prompts off, the turn stays and its text does not.
    let off = tempfile::tempdir().unwrap();
    std::fs::write(
        off.path().join("config.toml"),
        "[capture]\nstore_prompts = false\n",
    )
    .unwrap();
    let mut raw = raw::open(off.path()).unwrap();
    pass(off.path(), &mut raw, &v1.path).unwrap();
    assert_eq!(of_kind(&raw, "prompt"), [json!({"omitted": true})]);
}

/// Spec 7.4: a rerun imports nothing twice, and then only what v1 wrote since: events,
/// `session_repos` rows and documents.
#[test]
fn a_rerun_appends_only_new_v1_events() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "github.com/o/r", 100);
    v1.prompt("s1", 200, "one");
    v1.observation("s1", 210, "Tabs", "We use tabs.");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let first = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((first.records, first.repos, first.documents), (1, 1, 1));
    let again = pass(home.path(), &mut raw, &v1.path).unwrap();
    let nothing = Stats {
        seen: 1,
        ..Stats::default()
    };
    assert_eq!(again, nothing);
    v1.prompt("s1", 300, "two");
    v1.touched("s1", "github.com/o/lib");
    v1.observation("s1", 310, "Spaces", "Not spaces.");
    let next = pass(home.path(), &mut raw, &v1.path).unwrap();
    let counts = (
        next.events,
        next.records,
        next.repos,
        next.documents,
        next.seen,
    );
    assert_eq!(counts, (1, 1, 1, 1, 1));
    let prompts = of_kind(&raw, "prompt");
    assert_eq!(
        prompts,
        [json!({"prompt": "one"}), json!({"prompt": "two"})]
    );
}

/// Each listed id reads as the rules read now: a rule added after the import hides it.
#[test]
fn a_deleted_session_is_listed_as_the_rules_read_now() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("acme-123456", "r", 100);
    v1.prompt("acme-123456", 101, "one");
    v1.session("b", "r", 200);
    v1.prompt("b", 201, "two");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    pass(home.path(), &mut raw, &v1.path).unwrap();
    v1.delete_session("acme-123456");
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
    std::fs::write(home.path().join("config.toml"), rule).unwrap();
    let listed = pass(home.path(), &mut raw, &v1.path).unwrap().deleted;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(!listed[0].contains("acme-123456"), "{listed:?}");
}

#[test]
fn a_stricter_rule_does_not_list_a_held_v1_session_as_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("acme-123456", "r", 100);
    v1.prompt("acme-123456", 101, "one");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().records, 1);
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
    std::fs::write(home.path().join("config.toml"), rule).unwrap();
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert!(stats.deleted.is_empty(), "{:?}", stats.deleted);
}

#[test]
fn prior_redaction_reports_uncertainty_without_recommending_forget() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("acme-123456", "r", 100);
    v1.prompt("acme-123456", 101, "one");
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    std::fs::write(
        &config,
        "[redaction]\nextra_rules = [{ id = 'part', regex = '[0-9]{6}' }]\n",
    )
    .unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().records, 1);
    std::fs::write(
        &config,
        "[redaction]\nextra_rules = [{ id = 'whole', regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert!(stats.deleted.is_empty(), "{:?}", stats.deleted);
    let report = serde_json::to_value(&stats).unwrap();
    assert_eq!(report["uncertain"], json!(["acme-[REDACTED]"]));
    drop(raw);
    v1.conn
        .execute(
            "VACUUM INTO ?1",
            [home.path().join("oboete.db").to_str().unwrap()],
        )
        .unwrap();
    let mut out = Vec::new();
    finish(home.path(), "no\n".as_bytes(), &mut out).unwrap();
    let said = String::from_utf8(out).unwrap();
    assert!(
        said.contains("cannot be determined") && said.contains("acme-[REDACTED]"),
        "{said}"
    );
    assert!(!said.contains("forget them"), "{said}");
}

/// D6: v1 gives the ids of deleted newest events to new ones, so a pass refuses when the
/// event at its checkpoint is gone or another, before it imports anything.
#[test]
fn a_reused_id_past_a_deleted_newest_session_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("a", "r", 100);
    v1.prompt("a", 110, "a one");
    v1.session("b", "r", 200);
    v1.prompt("b", 210, "b one");
    v1.prompt("b", 220, "b two");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().records, 3);
    let top = (raw.max_seq().unwrap(), raw.max_op_seq().unwrap());
    v1.delete_session("b");
    let refused = pass(home.path(), &mut raw, &v1.path).unwrap_err();
    assert!(
        format!("{refused:#}").contains("deleted the newest session"),
        "{refused:#}"
    );
    // Hooks write more events than it held: ids 2 and 3 again, and 4.
    v1.session("c", "r", 300);
    for ts in 310..313 {
        v1.prompt("c", ts, "c");
    }
    assert!(pass(home.path(), &mut raw, &v1.path).is_err());
    assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), top);
}

/// Spec 7.4, A105: a session the old viewer deleted after a pass is listed by the next, so the
/// owner can forget it in Design B.
#[test]
fn deleted_v1_sessions_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    for (i, id) in ["a", "b", "c"].into_iter().enumerate() {
        v1.session(id, "r", 100 * i as i64);
        v1.prompt(id, 100 * i as i64 + 1, id);
    }
    // An id the gate masks: its original identifier cannot be recovered from its label.
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let masked = format!("s-{token}");
    v1.session(&masked, "r", 400);
    v1.prompt(&masked, 401, "four");
    // Not the newest: deleting the newest session is refused (D6).
    v1.session("d", "r", 500);
    v1.prompt("d", 501, "five");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert!(stats.deleted.is_empty());
    assert!(stats.uncertain.is_empty(), "{:?}", stats.uncertain);
    v1.delete_session("a");
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!(stats.deleted, ["a"]);
    assert!(stats.uncertain.is_empty(), "{:?}", stats.uncertain);
    v1.delete_session(&masked);
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!(stats.deleted, ["a"]);
    assert_eq!(stats.uncertain.len(), 1, "{:?}", stats.uncertain);
    assert!(stats.deleted.iter().all(|s| !s.contains(&token)));
    assert!(stats.uncertain.iter().all(|s| !s.contains(&token)));
    // And by `--finish`, from the home's own store.
    drop(raw);
    let h = home.path();
    let own = h.join("oboete.db");
    v1.conn
        .execute("VACUUM INTO ?1", [own.to_str().unwrap()])
        .unwrap();
    let mut out = Vec::new();
    finish(h, "no\n".as_bytes(), &mut out).unwrap();
    let said = String::from_utf8(out).unwrap();
    assert!(
        said.contains("here too): a\n")
            && said.contains("cannot be determined")
            && !said.contains(&token),
        "{said}"
    );
}

/// Spec 7.4: each batch commits with its checkpoint, so a pass killed between two resumes after
/// the first, and every event lands once, in order.
#[test]
fn a_killed_migrate_resumes_without_duplicates_or_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "r", 100);
    // 100 tool calls of 60 KB: past `MAX_BATCH_BYTES`, so two appends.
    for i in 0..100 {
        let output = format!("{i:03}{}", "x".repeat(60_000));
        let stored = json!({"tool": "Read", "input": format!("file {i}"), "output": output});
        v1.event("s1", "PostToolUse", 1_000 + i, stored);
    }
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    crate::crash::at(2);
    let killed = pass(home.path(), &mut raw, &v1.path);
    crate::crash::off();
    assert!(killed.is_err());
    let landed = of_kind(&raw, "tool").len();
    assert!(landed > 0 && landed < 100, "{landed}");
    pass(home.path(), &mut raw, &v1.path).unwrap();
    let inputs: Vec<String> = of_kind(&raw, "tool")
        .iter()
        .map(|b| b["input"].as_str().unwrap().to_owned())
        .collect();
    let want: Vec<String> = (0..100).map(|i| format!("file {i}")).collect();
    assert_eq!(inputs, want);
}

/// D5: `append_imports` commits `IMPORT_BATCH` documents at a time, so a pass stopped between
/// two keeps the first and the next adds the rest, once.
#[test]
fn a_killed_pass_keeps_the_documents_it_appended() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "r", 100);
    for i in 0..600 {
        v1.observation("s1", 110 + i, "Note", &format!("Note {i}."));
    }
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    // The touch label commits first, then the documents' first batch.
    crate::crash::at(3);
    let killed = pass(home.path(), &mut raw, &v1.path);
    crate::crash::off();
    assert!(killed.is_err());
    assert_eq!(
        raw.import_keys("oboete-v1:d1e5").unwrap().len(),
        IMPORT_BATCH
    );
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((stats.documents, stats.seen), (100, 500));
}

#[test]
fn a_bad_late_document_is_readonly_in_preview_and_appends_no_document_batch() {
    let source = tempfile::tempdir().unwrap();
    let v1 = V1::new(source.path());
    v1.session("s1", "r", 100);
    for i in 0..600 {
        v1.observation("s1", 200 + i, "Note", "Native document body.");
    }
    // SQLite permits a BLOB in a TEXT column; the native typed reader must reject it.
    v1.conn
        .execute("UPDATE observations SET body = x'00' WHERE id = 600", [])
        .unwrap();
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("absent");
    assert!(preview(&home, &v1.path).is_err());
    assert!(!home.exists());
    let failed = run(&home, &v1.path, None, &mut |_| {}).unwrap_err();
    assert_eq!(
        (failed.outcome.stats.repos, failed.outcome.stats.documents),
        (1, 0)
    );
    let raw = raw::open(&home).unwrap();
    assert!(raw.import_keys("oboete-v1:d1e5").unwrap().is_empty());
    drop(raw);
    v1.conn
        .execute(
            "UPDATE observations SET body = 'Repaired.' WHERE id = 600",
            [],
        )
        .unwrap();
    let resumed = run(&home, &v1.path, None, &mut |_| {}).unwrap();
    assert_eq!((resumed.stats.documents, resumed.stats.seen), (600, 0));
}

/// D6: events that record nothing still move the checkpoint, so the next pass does not read
/// them again.
#[test]
fn a_batch_with_nothing_to_record_moves_the_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "r", 100);
    v1.event("s1", "Stop", 110, json!({"assistant": ""}));
    v1.event("s1", "PreToolUse", 120, json!({}));
    v1.prompt("s1", 130, "<private>not for anyone</private>");
    let home = tempfile::tempdir().unwrap();
    let shown = preview(home.path(), &v1.path).unwrap();
    let mut commits = Vec::new();
    let failed = run(home.path(), &v1.path, Some(&shown.key), &mut |c| {
        commits.push(c.clone());
        if c.stage == Stage::Events {
            crate::crash::at(1);
        }
    })
    .unwrap_err();
    crate::crash::off();
    let stats = failed.outcome.stats;
    assert_eq!(
        (stats.events, stats.records, stats.repos, stats.documents),
        (3, 0, 0, 0)
    );
    // The zero-record event checkpoint is durable: count totals cannot classify this
    // later repository-batch failure. The callback is the committed-boundary witness.
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].stage, Stage::Events);
    assert_eq!(
        (commits[0].records, commits[0].repos, commits[0].documents),
        (0, 0, 0)
    );
    let raw = raw::open(home.path()).unwrap();
    assert!(records(&raw).is_empty());
    let checkpoints = raw.migration_checkpoints("oboete-v1:").unwrap();
    assert_eq!(checkpoints["oboete-v1:d1e5"].through, 3);
    drop(raw);
    let resumed = run(home.path(), &v1.path, None, &mut |_| {}).unwrap();
    assert_eq!((resumed.stats.events, resumed.stats.repos), (0, 1));
}

/// A57, D6: v1's `session_repos` rows become `touch` records at their session's start, which a
/// restore keeps, and a pass after it adds none again.
#[test]
fn session_repos_become_label_records_that_survive_a_restore() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "github.com/o/r", 100);
    v1.touched("s1", "github.com/o/lib");
    v1.prompt("s1", 110, "one");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().repos, 2);
    let touches = raw.touches("oboete-v1").unwrap();
    let label = |repo: &str| ("s1".to_owned(), Some(repo.to_owned()));
    let want = [label("github.com/o/lib"), label("github.com/o/r")];
    assert_eq!(touches, want.into_iter().collect());
    assert!(
        records(&raw)
            .iter()
            .all(|e| e.kind != "touch" || e.ts == 100)
    );
    drop(raw);
    crate::backup::export(home.path()).unwrap();
    std::fs::write(home.path().join("raw.db"), b"not a database at all").unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(home.path().join(f));
    }
    crate::worker::run_once(home.path()).unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    assert_eq!(raw.touches("oboete-v1").unwrap(), touches);
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((stats.records, stats.repos), (0, 0));
}

/// Row 30-1: a v1 session that touched an excluded repository, which only its `session_repos`
/// row tells, reaches no curator, as a live one does not; before the exclusion it would.
#[test]
fn an_excluded_repo_through_session_repos_reaches_no_curator() {
    use crate::curate::{Reading, Reads, Span, WINDOW_TOKENS, span_windows};
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("x", "github.com/o/open", 100);
    v1.touched("x", "github.com/o/secret");
    v1.prompt("x", 110, "Secret plan.");
    v1.session("y", "github.com/o/open", 200);
    v1.prompt("y", 210, "Open work.");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    pass(home.path(), &mut raw, &v1.path).unwrap();
    let rules = crate::redact::Rules::default();
    let sent = |raw: &Raw| {
        let reading = Reading::now(raw, Reads::Source("oboete-v1".into())).unwrap();
        let span = Span::records(1, raw.max_seq().unwrap());
        span_windows(raw, &span, WINDOW_TOKENS, &rules, &reading)
            .unwrap()
            .into_iter()
            .map(|w| w.text)
            .collect::<String>()
    };
    let before = sent(&raw);
    assert!(before.contains("Secret plan.") && before.contains("Open work."));
    raw.exclude("github.com/o/secret", false).unwrap();
    let after = sent(&raw);
    assert!(!after.contains("Secret plan.") && after.contains("Open work."));
}

/// Slice 4a (Codex on #489): a repository excluded before a rule that masks part of its key is
/// still never recorded by a pass, its events and its `session_repos` rows alike: the pass
/// matches the list as its labels are made now.
#[test]
fn a_rule_added_after_a_repository_was_excluded_keeps_its_v1_records_out() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "github.com/o/acme-482915", 100);
    v1.prompt("s1", 110, "canary-4a");
    v1.session("s2", "github.com/o/open", 200);
    v1.touched("s2", "github.com/o/acme-482915");
    v1.prompt("s2", 210, "Open work.");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    raw.exclude_capture(("repo", "github.com/o/acme-482915"), false)
        .unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    pass(home.path(), &mut raw, &v1.path).unwrap();
    let repos: std::collections::BTreeSet<_> = records(&raw).into_iter().map(|e| e.repo).collect();
    assert_eq!(repos, [Some("github.com/o/open".to_owned())].into());
}

/// D5: v1's observations, summaries and prompts become import ops under their uids (v1's own,
/// a claude-mem one it imported kept), which the worker resolves, never injected.
#[test]
fn v1_documents_become_import_ops_under_their_uids() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "github.com/o/r", 100);
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    v1.observation("s1", 110, "Tabs", &format!("We use tabs. {token}"));
    v1.observation("s1", 120, "Imported", "From claude-mem.");
    v1.conn
        .execute_batch(
            "UPDATE observations SET uid = 'claude-mem:abc:o5' WHERE id = 2;
                 INSERT INTO summaries(session_id, repo, ts, body, provider)
                   VALUES('s1', 'github.com/o/r', 130, 'Request: tabs', 'groq');
                 INSERT INTO prompts(session_id, repo, ts, body)
                   VALUES('s1', 'github.com/o/r', 140, 'use tabs');",
        )
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().documents, 4);
    let device = raw.device().to_owned();
    let docs: Vec<ImportDoc> = raw
        .ops_after(&device, 0, 100)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == raw::OpKind::Import)
        .map(|o| serde_json::from_value(o.body).unwrap())
        .collect();
    let keys: Vec<(&str, &str, &str, &str)> = docs
        .iter()
        .map(|d| {
            (
                d.uid.as_str(),
                d.source.as_str(),
                d.source_id.as_str(),
                d.kind.as_str(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("d1e5:o1", "oboete-v1:d1e5", "o1", "decision"),
            ("claude-mem:abc:o5", "oboete-v1:d1e5", "o2", "decision"),
            ("d1e5:s1", "oboete-v1:d1e5", "s1", "summary"),
            ("d1e5:p1", "oboete-v1:d1e5", "p1", "prompt"),
        ]
    );
    assert_eq!(docs[0].body, "We use tabs. [REDACTED]");
    drop(raw);
    crate::worker::run_once(home.path()).unwrap();
    let k = crate::knowledge::open(home.path()).unwrap();
    let resolved: i64 = k
        .query_row("SELECT count(*) FROM imported", [], |r| r.get(0))
        .unwrap();
    assert_eq!(resolved, 4);
    let raw = raw::open(home.path()).unwrap();
    let settings = Settings::default();
    let shown = crate::hook::start_text_read(
        home.path(),
        Some(&raw),
        "github.com/o/r",
        "",
        "s1",
        &settings,
        true,
        false,
    )
    .unwrap()
    .map(|s| s.text)
    .unwrap_or_default();
    assert!(!shown.contains("We use tabs."), "{shown}");
}

/// MUST-M9: migrated records move neither the manifest's facts nor its count of records not
/// yet curated.
#[test]
fn migrated_records_never_move_the_manifest_or_backlog() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let live = |text: &str, ts: i64| Event {
        session: "L".into(),
        kind: "prompt".into(),
        repo: Some("github.com/o/r".into()),
        ts,
        body: json!({"prompt": text}).to_string(),
        ..raw::test_event("")
    };
    raw.append(&live("Live prompt.", 1_000)).unwrap();
    crate::worker::run_once(home.path()).unwrap();
    let settings = Settings::default();
    // The backlog's age in minutes moves with the clock between the two reads.
    let age = regex::Regex::new(r"/ \d+ min").unwrap();
    let manifest = |raw: &Raw| {
        crate::hook::start_text_read(
            home.path(),
            Some(raw),
            "github.com/o/r",
            "",
            "L",
            &settings,
            true,
            false,
        )
        .unwrap()
        .map(|m| age.replace_all(&m.text, "/ _ min").into_owned())
    };
    let before = manifest(&raw);
    assert!(
        before
            .as_deref()
            .is_some_and(|m| m.contains("Live prompt.")),
        "{before:?}"
    );
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "github.com/o/r", 2_000);
    v1.prompt("s1", 2_100, "Old prompt.");
    let failed = json!({"tool": "Bash", "input": "make", "output": "boom", "failed": true});
    v1.event("s1", "PostToolUseFailure", 2_200, failed);
    pass(home.path(), &mut raw, &v1.path).unwrap();
    crate::worker::run_once(home.path()).unwrap();
    assert_eq!(manifest(&raw), before);
}

/// MUST-M16: a known commit gap proves interleaving without relying on SQLite's busy
/// handler or the scheduler to give a waiting hook that gap.
#[test]
fn a_hook_appends_between_migrated_batches() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "r", 100);
    for i in 0..1_200 {
        v1.prompt("s1", 1_000 + i, &format!("old {i}"));
    }
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let live = std::rc::Rc::new(std::cell::Cell::new(None));
    let appended = live.clone();
    let hook_home = home.path().to_owned();
    BETWEEN_BATCHES.with(|seam| {
        *seam.borrow_mut() = Some(Box::new(move || {
            let mut hook = raw::open(&hook_home).unwrap();
            appended.set(Some(
                hook.append(&raw::test_event("live between batches"))
                    .unwrap(),
            ));
        }));
    });
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((stats.events, stats.records), (1_200, 1_200));
    let migrated: Vec<i64> = raw
        .after(raw.device(), 0, 2_000)
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.item {
            Item::Event(e) if e.source == SOURCE && e.kind == "prompt" => Some(r.seq),
            _ => None,
        })
        .collect();
    assert_eq!(migrated.len(), 1_200);
    let live = live.get().expect("the between-batch seam did not run");
    assert!(migrated[0] < live && live < migrated[1_199], "{live}");
    assert_eq!(
        raw.migration_checkpoints("oboete-v1:").unwrap()["oboete-v1:d1e5"].through,
        1_200
    );
}

/// MUST-M16: real contention must not lose hook appends or stop a pass. Scheduling cannot
/// promise interleaving; `a_hook_appends_between_migrated_batches` covers it deterministically.
#[test]
fn migrate_and_a_hook_append_together() {
    migrate_with_hooks(std::time::Duration::from_millis(1));
}

/// Without a pause, contention must still preserve every append and finish the pass.
/// `a_hook_appends_between_migrated_batches` owns interleaving, which scheduling cannot promise.
#[test]
fn migrate_progresses_while_hooks_append_without_a_pause() {
    migrate_with_hooks(std::time::Duration::ZERO);
}

fn migrate_with_hooks(pause: std::time::Duration) {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "r", 100);
    for i in 0..1_200 {
        v1.prompt("s1", 1_000 + i, &format!("old {i}"));
    }
    v1.observation("s1", 2_200, "Migrated document", "Kept once.");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let done = std::sync::atomic::AtomicBool::new(false);
    let (live, stats) = std::thread::scope(|s| {
        let (ready, started) = std::sync::mpsc::channel();
        let done = &done;
        let hook_home = home.path();
        let hook = s.spawn(move || {
            let mut store = raw::open(hook_home).unwrap();
            let mut seqs = vec![store.append(&raw::test_event("live")).unwrap()];
            ready.send(()).unwrap();
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                seqs.push(store.append(&raw::test_event("live")).unwrap());
                std::thread::sleep(pause);
            }
            seqs
        });
        // A delayed hook thread must not make the append-count check vacuous.
        started.recv().unwrap();
        let passed = pass(home.path(), &mut raw, &v1.path);
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        let live = hook.join().unwrap();
        (live, passed.unwrap())
    });
    assert_eq!(
        (stats.events, stats.records, stats.documents),
        (1_200, 1_200, 1)
    );
    let all = records(&raw);
    assert_eq!(
        all.iter()
            .filter(|e| e.source == SOURCE && e.kind == "prompt")
            .count(),
        1_200
    );
    assert_eq!(
        all.iter().filter(|e| e.source == "hook").count(),
        live.len()
    );
    assert_eq!(
        raw.migration_checkpoints("oboete-v1:").unwrap()["oboete-v1:d1e5"].through,
        1_200
    );
    let rerun = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((rerun.events, rerun.records, rerun.documents), (0, 0, 0));
    assert_eq!(rerun.seen, 1);
}

#[cfg(target_os = "linux")]
#[test]
fn migrated_settings_wait_for_the_config_writer_and_keep_its_choice() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let ours = home.path().join("config.toml");
    std::fs::write(
        source.path().join("config.toml"),
        "providers = []\n[worker]\nresident = true\n",
    )
    .unwrap();
    std::fs::create_dir(home.path().join("state")).unwrap();
    let h = home.path();
    std::thread::scope(|scope| {
        // Kept inside the scope so a failed assertion drops the fence before the join.
        let fence = std::fs::File::create(home.path().join("state/config.lock")).unwrap();
        fence.lock().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let from = source.path().join("oboete.db");
        let migration = std::thread::Builder::new()
            .name("r5-migrate".into())
            .spawn_scoped(scope, move || {
                send.send(settings(h, &from)).unwrap();
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                matches!(
                    receive.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                ),
                "migration completed while another config writer held the lock"
            );
            assert!(
                !ours.exists(),
                "migration wrote through another config writer"
            );
            let waiting = std::fs::read_dir("/proc/self/task")
                .unwrap()
                .flatten()
                .any(|task| {
                    std::fs::read_to_string(task.path().join("comm"))
                        .is_ok_and(|name| name.trim() == "r5-migrate")
                        && std::fs::read_to_string(task.path().join("wchan"))
                            .is_ok_and(|wait| wait.contains("locks_") || wait.contains("flock_"))
                });
            if waiting {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "migration never waited on config.lock"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let chosen = "# The first writer's choice\nproviders = []\n[worker]\nresident = false\n";
        std::fs::write(&ours, chosen).unwrap();
        drop(fence);
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        migration.join().unwrap();
        assert_eq!(std::fs::read_to_string(&ours).unwrap(), chosen);
    });
}

/// R05, A106: v1's config.toml goes unchanged into a home that has none, and loads under
/// Design B with its providers in order, their models, and the key files by path; a home's
/// own file is never edited; what the file leaves unanswered is printed.
#[test]
fn the_v1_config_loads_under_b() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    let text = r#"gemini = "after-subscriptions"

[[providers]]
kind = "openai"
name = "groq"
base_url = "https://api.groq.com/openai/v1"
key_file = "/k/GROQ_KEY.md"
model = "llama-3.3-70b-versatile"

[[providers]]
kind = "cli"
name = "codex"
cli = "codex"
model = "gpt-5.5"

[embedding]
provider = "workers-ai"
account_id = "acct"
key_file = "/k/CF_WORKERS_AI_KEY.md"
"#;
    std::fs::write(dir.path().join("config.toml"), text).unwrap();
    let home = tempfile::tempdir().unwrap();
    let lines = settings(home.path(), &v1.path).unwrap();
    let copied = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert_eq!(copied, text);
    assert!(
        lines[0].starts_with("[summary] curate is not set"),
        "{lines:?}"
    );
    let cfg = crate::config::load(home.path()).unwrap();
    let chain: Vec<(&str, Option<&str>, Option<&Path>)> = cfg
        .providers
        .iter()
        .map(|p| match p {
            crate::config::Provider::Openai {
                name,
                model,
                key_file,
                ..
            } => (name.as_str(), Some(model.as_str()), key_file.as_deref()),
            crate::config::Provider::Cli { name, model, .. } => {
                (name.as_str(), model.as_deref(), None)
            }
        })
        .collect();
    assert_eq!(
        chain[..2],
        [
            (
                "groq",
                Some("llama-3.3-70b-versatile"),
                Some(Path::new("/k/GROQ_KEY.md"))
            ),
            ("codex", Some("gpt-5.5"), None),
        ]
    );
    assert_eq!(chain.last().map(|c| c.0), Some("gemini"));
    let key = cfg.embedding.key_file.clone();
    assert_eq!(key, Path::new("/k/CF_WORKERS_AI_KEY.md"));
    assert!(!cfg.summary.curate);
    // A home with its own file keeps it as it is.
    let own = "[summary]\ncurate = true\n";
    std::fs::write(home.path().join("config.toml"), own).unwrap();
    let lines = settings(home.path(), &v1.path).unwrap();
    let kept = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert_eq!(kept, own);
    assert!(lines.iter().all(|l| !l.contains("curate")), "{lines:?}");
    // A copied file that does not parse: its line, never its text, which can hold a value
    // `[redaction]` hides.
    let secret = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let broken = format!("[redaction]\nallowlist = [{secret}]\n");
    std::fs::write(dir.path().join("config.toml"), broken).unwrap();
    let fresh = tempfile::tempdir().unwrap();
    // A copy an earlier run left cut short is not the file: it is copied again, whole.
    std::fs::write(fresh.path().join("config.toml.part"), "[redaction]\n").unwrap();
    let refused = settings(fresh.path(), &v1.path).unwrap_err();
    assert!(!fresh.path().join("config.toml.part").exists());
    let said = format!("{refused:#}");
    assert!(said.contains("line 2") && !said.contains(&secret), "{said}");
    let fresh = tempfile::tempdir().unwrap();
    let refused = run(fresh.path(), &v1.path, None, &mut |_| {}).unwrap_err();
    assert_eq!(refused.code, FailureCode::InvalidConfig);
    assert_eq!(
        refused.outcome.settings.unwrap().effect,
        SettingsEffect::Copy
    );
    assert!(!format!("{:#}", refused.cause).contains(&secret));
}

/// A58: `--finish` imports once more, lists v1's old files with their sizes, and deletes them
/// only on `yes`, and only while v1 has written nothing since its pass; Design B's files and
/// `eval/` stay; the store is deleted first.
#[test]
fn finish_deletes_only_on_yes() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let v1 = V1::new(h);
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, "one");
    for f in ["pre-1.db", "pre-1.db-wal", "memory.db", "notes.txt"] {
        std::fs::write(h.join(f), "x").unwrap();
    }
    for d in ["spool", "cache", "logs", "pre-rollout-1", "eval"] {
        std::fs::create_dir(h.join(d)).unwrap();
        std::fs::write(h.join(d).join("f"), "xyz").unwrap();
    }
    let listed = |out: &[u8]| String::from_utf8(out.to_vec()).unwrap();
    let mut out = Vec::new();
    finish(h, "no\n".as_bytes(), &mut out).unwrap();
    let said = listed(&out);
    assert!(said.contains("spool (3 bytes)") && said.ends_with("Nothing was deleted.\n"));
    assert!(
        !said.contains("eval") && !said.contains("notes.txt"),
        "{said}"
    );
    assert!(h.join("oboete.db").exists() && h.join("spool").exists());
    let raw = raw::open(h).unwrap();
    assert_eq!(of_kind(&raw, "prompt"), [json!({"prompt": "one"})]);
    drop(raw);
    // While the answer is read, raw.db stays open: a restore cannot swap it under the pass.
    struct Swapping<'a>(&'a Path, Option<bool>);
    impl std::io::Read for Swapping<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_some() {
                return Ok(0);
            }
            let lock = std::fs::File::open(self.0.join("raw.lock"))?;
            self.1 = Some(lock.try_lock().is_err());
            buf[..3].copy_from_slice(b"no\n");
            Ok(3)
        }
    }
    let mut answer = std::io::BufReader::new(Swapping(h, None));
    finish(h, &mut answer, &mut Vec::new()).unwrap();
    assert_eq!(
        answer.into_inner().1,
        Some(true),
        "a restore could swap raw.db"
    );
    // An old hook writes while the answer is read: nothing is deleted.
    struct Writing<'a>(&'a V1, bool);
    impl std::io::Read for Writing<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::replace(&mut self.1, true) {
                return Ok(0);
            }
            self.0.prompt("s1", 120, "written meanwhile");
            buf[..4].copy_from_slice(b"yes\n");
            Ok(4)
        }
    }
    let answer = std::io::BufReader::new(Writing(&v1, false));
    let refused = finish(h, answer, &mut Vec::new()).unwrap_err();
    assert!(format!("{refused:#}").contains("changed after the import pass"));
    assert!(h.join("oboete.db").exists() && h.join("spool").exists());
    drop(v1);
    // A link named as an old directory goes, never what it points to.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("kept"), "x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), h.join("memory.db-wal")).unwrap();
    let mut out = Vec::new();
    finish(h, "yes\n".as_bytes(), &mut out).unwrap();
    assert!(outside.path().join("kept").exists());
    // The store is deleted first, right after the recheck.
    // The answer is read on the question's line, which the first deletion's line ends.
    let said = listed(&out);
    let first = said
        .split_once("delete them: ")
        .and_then(|(_, a)| a.lines().next());
    let store = format!("deleted {}", h.join("oboete.db").display());
    assert_eq!(first, Some(store.as_str()));
    for gone in [
        "oboete.db",
        "pre-1.db",
        "pre-1.db-wal",
        "memory.db",
        "spool",
        "cache",
    ] {
        assert!(!h.join(gone).exists(), "{gone}: {}", listed(&out));
    }
    for kept in ["raw.db", "eval", "notes.txt"] {
        assert!(h.join(kept).exists(), "{kept}");
    }
    let raw = raw::open(h).unwrap();
    let prompts = of_kind(&raw, "prompt");
    assert_eq!(prompts.len(), 2, "the write during the answer was imported");
    // The old viewer deletes the newest session while the answer is read, and a hook writes
    // as many events again: the same highest ids, another newest event.
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let v1 = V1::new(h);
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, "one");
    v1.session("s2", "r", 200);
    v1.prompt("s2", 210, "two");
    struct Reusing<'a>(&'a V1, bool);
    impl std::io::Read for Reusing<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::replace(&mut self.1, true) {
                return Ok(0);
            }
            self.0.delete_session("s2");
            self.0.session("s3", "r", 300);
            self.0.prompt("s3", 310, "three");
            buf[..4].copy_from_slice(b"yes\n");
            Ok(4)
        }
    }
    let answer = std::io::BufReader::new(Reusing(&v1, false));
    let refused = finish(h, answer, &mut Vec::new()).unwrap_err();
    assert!(format!("{refused:#}").contains("changed after the import pass"));
    assert!(h.join("oboete.db").exists());
}

/// Spec 7.4: doctor counts v1's events not migrated yet, and lists v1's old files and the
/// evaluation copies.
#[test]
fn doctor_names_what_the_cut_over_leaves() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let v1 = V1::new(h);
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, "one");
    v1.prompt("s1", 120, "two");
    let lines = doctor(h).unwrap();
    assert!(
        lines[0].starts_with("v1 events not migrated yet: 2 "),
        "{lines:?}"
    );
    let mut raw = raw::open(h).unwrap();
    pass(h, &mut raw, &v1.path).unwrap();
    v1.prompt("s1", 130, "three");
    std::fs::create_dir(h.join("eval")).unwrap();
    std::fs::write(h.join("eval").join("queries.jsonl"), "{}\n").unwrap();
    let lines = doctor(h).unwrap();
    assert!(
        lines[0].starts_with("v1 events not migrated yet: 1 "),
        "{lines:?}"
    );
    assert!(lines[1].contains("oboete.db ("), "{lines:?}");
    assert!(lines[2].ends_with("eval (3 bytes)"), "{lines:?}");
}

/// Labels are stored text (spec 2.2): what the gate masks in a v1 session id or a document's
/// repository is kept nowhere, not in a record's labels, a document's or the checkpoint.
#[test]
fn no_label_or_checkpoint_keeps_what_the_gate_masks() {
    let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    let session = format!("s-{token}");
    v1.session(&session, "github.com/o/r", 100);
    v1.prompt(&session, 110, "one");
    v1.conn
        .execute(
            "INSERT INTO observations(session_id, repo, ts, kind, title, body, provider)
                 VALUES(?1, ?2, 120, 'decision', 'Tabs', 'We use tabs.', 'groq')",
            params![session, format!("github.com/o/{token}")],
        )
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((stats.records, stats.documents), (1, 1));
    let ops = raw.ops_after(raw.device(), 0, 100).unwrap();
    let kinds: Vec<OpKind> = ops.iter().map(|o| o.kind).collect();
    assert!(
        kinds.contains(&OpKind::Migration) && kinds.contains(&OpKind::Import),
        "{kinds:?}"
    );
    for op in &ops {
        assert!(!op.body.to_string().contains(&token), "{:?}", op.kind);
    }
    for e in records(&raw) {
        assert!(!format!("{e:?}").contains(&token), "{}", e.kind);
    }
    // The fingerprint still matches: a rerun is not refused.
    assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().seen, 1);
}

/// D6, A104: a v1 event and a document `denied` refuses are left out; the others land.
#[test]
fn migrate_leaves_a_denied_event_and_document_out() {
    let forgotten = format!("x {} y", raw::DENIED_IN_TESTS);
    let dir = tempfile::tempdir().unwrap();
    let v1 = V1::new(dir.path());
    v1.session("s1", "r", 100);
    v1.prompt("s1", 110, &forgotten);
    v1.prompt("s1", 120, "kept");
    v1.observation("s1", 130, "Forgotten", &forgotten);
    v1.observation("s1", 140, "Kept", "Kept.");
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
    assert_eq!((stats.records, stats.documents), (1, 1));
    assert_eq!(of_kind(&raw, "prompt"), [json!({"prompt": "kept"})]);
    let keys = raw.import_keys("oboete-v1:d1e5").unwrap();
    assert_eq!(keys, ["o2".to_owned()].into_iter().collect());
}

fn sha(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

/// A v1 store with its newest event only in the WAL: the hashes of the store and its WAL,
/// and its device id.
fn v1_store(home: &Path) -> (String, String, String) {
    let conn = crate::db::open(home).unwrap();
    conn.execute_batch(
        "INSERT INTO sessions(id, agent, repo, started_at, last_event_at)
               VALUES ('s', 'claude', 'r', 1, 1);
             INSERT INTO events(session_id, event, ts, payload)
               VALUES ('s', 'Stop', 1, '{\"assistant\": \"one\"}');",
    )
    .unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint = 0")
        .unwrap();
    conn.execute(
        "INSERT INTO events(session_id, event, ts, payload)
             VALUES ('s', 'Stop', 2, '{\"assistant\": \"two\"}')",
        [],
    )
    .unwrap();
    let device = crate::db::device_id(&conn).unwrap();
    // Left open: closing the last connection would move the WAL into the store.
    std::mem::forget(conn);
    let wal = home.join("oboete.db-wal");
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);
    (sha(&home.join("oboete.db")), sha(&wal), device)
}

/// Spec 7.4: doctor and a pass read v1's store without a write, the events only in its WAL
/// imported, also on a copy, where v1's open would have given the store a new device id.
#[test]
fn the_old_store_is_left_byte_for_byte() {
    let home = tempfile::tempdir().unwrap();
    let before = v1_store(home.path());
    let copy = tempfile::tempdir().unwrap();
    for f in ["oboete.db", "oboete.db-wal"] {
        std::fs::copy(home.path().join(f), copy.path().join(f)).unwrap();
    }
    for h in [home.path(), copy.path()] {
        crate::setup::doctor(h).ok();
        let mut raw = raw::open(h).unwrap();
        pass(h, &mut raw, &h.join("oboete.db")).unwrap();
        let replies = of_kind(&raw, "reply");
        assert_eq!(
            replies,
            [json!({"assistant": "one"}), json!({"assistant": "two"})]
        );
        let conn = open_v1(&h.join("oboete.db")).unwrap();
        let events: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 2);
        let device = crate::db::device_id(&conn).unwrap();
        drop(conn);
        let after = (
            sha(&h.join("oboete.db")),
            sha(&h.join("oboete.db-wal")),
            device,
        );
        assert_eq!(after, before, "{}", h.display());
    }
}
