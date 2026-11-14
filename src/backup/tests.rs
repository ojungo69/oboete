use super::*;
use crate::raw::{Item, Target};

/// D5 (2c, Codex and Greptile on #453): a backup line is purged when its op holds the text of
/// a uid a job forgets, kept when it holds none (a body a purge wrote, a kind whose ops hold no
/// uid's text, a blank line) and unknown when that cannot be told; a purge takes for oboete's
/// only the names an export or a purge gives.
#[test]
fn a_purge_judges_each_backup_line_and_name() {
    let line = |kind: &str, body: serde_json::Value| {
        serde_json::json!({"device": "d0", "op_seq": 7, "type": kind, "ts": 1,
                "body": body.to_string(), "batch": 3})
        .to_string()
    };
    let jobs = std::collections::HashMap::from([("u1".to_owned(), "j1".to_owned())]);
    let secret = line(
        "correction",
        serde_json::json!({"uid": "u1", "body": "secret"}),
    );
    let Line::Purged(new) = purged_line(&secret, &jobs, &raw::Judge::default()) else {
        panic!("{secret}");
    };
    assert_eq!(
        new,
        line("correction", serde_json::json!({"forgotten": "j1"}))
    );
    for kept in [
        line(
            "correction",
            serde_json::json!({"uid": "u2", "body": "kept"}),
        ),
        line("claim", serde_json::json!({"forgotten": "j0"})),
        line("window", serde_json::json!({"note": "u1"})),
        " ".to_owned(),
    ] {
        assert!(
            matches!(
                purged_line(&kept, &jobs, &raw::Judge::default()),
                Line::Kept
            ),
            "{kept}"
        );
    }
    for unknown in [
        line("claim", serde_json::json!({"quote": "secret"})),
        line(
            "claim",
            serde_json::json!({"forgotten": "j0", "quote": "secret"}),
        ),
        line(
            "correction",
            serde_json::json!({"forgotten": null, "body": "secret"}),
        ),
        line("future", serde_json::json!({"uid": "u1"})),
        secret[..40].to_owned(),
    ] {
        assert!(
            matches!(
                purged_line(&unknown, &jobs, &raw::Judge::default()),
                Line::Unknown(None)
            ),
            "{unknown}"
        );
    }

    let seg = "d0-000000000001-000000000002.ops.zst";
    assert!(segment(seg, Kind::Ops) && !segment(&format!(".{seg}"), Kind::Ops));
    for name in [
        "d-1-2.ops.zst",
        "-000000000001-000000000002.ops.zst",
        "d0-0000000000001-000000000002.ops.zst",
        "d0-000000000001-000000000002.records.zst",
        "d.0-000000000001-000000000002.ops.zst",
    ] {
        assert!(!segment(name, Kind::Ops), "{name}");
    }
    assert!(set_aside(
        &format!("{seg}.quarantined-1700000000000"),
        Kind::Ops
    ));
    for name in [
        "notes.ops.zst.quarantined-old".to_owned(),
        format!("{seg}.quarantined-"),
        format!("{seg}.quarantined-1.sha256"),
        format!("{seg}.sha256.quarantined-1"),
        "d0-000000000001-000000000002.records.zst.quarantined-1".to_owned(),
    ] {
        assert!(!set_aside(&name, Kind::Ops), "{name}");
    }
    for name in [
        format!(".{seg}.tmp"),
        format!(".{seg}.purge"),
        format!("..{seg}.purge.tmp"),
        format!(".{seg}.quarantined-1.tmp"),
    ] {
        assert!(leftover(&name), "{name}");
    }
    for name in [
        format!("{seg}.tmp"),
        format!(".{seg}.sha256.tmp"),
        format!("...{seg}.purge.tmp"),
        format!(".{seg}.purge.purge"),
        ".notes.tmp".to_owned(),
    ] {
        assert!(!leftover(&name), "{name}");
    }
    // Slice 3c-2: a record segment's names, likewise.
    let rec = "d0-000000000001-000000000002.seg.zst";
    assert!(segment(rec, Kind::Records) && !segment(rec, Kind::Ops));
    assert!(!segment(seg, Kind::Records));
    let aside = format!("{rec}.quarantined-1700000000000");
    assert!(set_aside(&aside, Kind::Records) && !set_aside(&aside, Kind::Ops));
    for name in [
        format!(".{rec}.tmp"),
        format!("..{rec}.purge.tmp"),
        format!(".{aside}.tmp"),
    ] {
        assert!(leftover(&name), "{name}");
    }
}

pub(crate) fn w5b_files(root: &Path) -> Vec<(PathBuf, String)> {
    fn walk(root: &Path, path: &Path, files: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().into(),
                    crate::preview::file_version(&path).unwrap(),
                ));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
}

pub(crate) fn w5b_log_change(p: &Path) -> (Vec<u8>, Vec<u8>) {
    let mut raw = raw::open(p).unwrap();
    let mut event = raw::test_event(r#"{"prompt":"synthetic confirmed log canary"}"#);
    event.source = "transcript".into();
    let identity = raw::ImportIdentity {
        origin: crate::forget::origin("synthetic", "confirmed-log"),
        session: crate::forget::session(&event.agent, &event.session),
        ambiguous: None,
        unverified: false,
    };
    let seq = raw
        .append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            &[identity],
            "",
            None,
        )
        .unwrap()[0];
    export(p).unwrap();
    let shown = raw
        .forget_preview(crate::forget::Target::Record {
            device: raw.device().into(),
            seq,
        })
        .unwrap();
    drop(raw);
    crate::forget::start(p, &shown).unwrap();
    let original = std::fs::read(p.join("forget.log")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let mut changed: crate::forget::Request =
        serde_json::from_value(value["request"].clone()).unwrap();
    changed.job = "0".repeat(32);
    let body = serde_json::to_string(&changed).unwrap();
    let changed = format!(
        "{{\"sum\":\"{}\",\"request\":{body}}}\n",
        crate::forget::hash(body.as_bytes())
    )
    .into_bytes();
    assert_ne!(original, changed);
    assert_eq!(original.len(), changed.len());
    (original, changed)
}

#[test]
fn w5b_same_size_and_mtime_log_changes_require_fresh_confirmation() {
    for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
        for backup_log in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            let (_, changed) = w5b_log_change(p);
            let path = if backup_log {
                dir(p).unwrap().join("forget.log")
            } else {
                p.join("forget.log")
            };
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            let identity = crate::db::store_file(&path);
            let before = std::fs::read(p.join("raw.db")).unwrap();
            let preview = preview_maintenance(p, operation).unwrap();
            BEFORE_SWAP.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    std::fs::write(&path, changed).unwrap();
                    std::fs::File::options()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(modified))
                        .unwrap();
                    assert_eq!(crate::db::store_file(&path), identity);
                    assert_eq!(
                        std::fs::metadata(&path).unwrap().modified().unwrap(),
                        modified
                    );
                }));
            });
            let mut commits = Vec::new();
            let failure = match operation {
                MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                    p,
                    Some(&preview.key),
                    crate::executable::CommandCaller::Worker,
                    &mut |event| commits.push(event.clone()),
                ),
                MaintenanceOperation::Restore => crate::worker::restore_report(
                    p,
                    Some(&preview.key),
                    crate::executable::CommandCaller::Worker,
                    &mut |event| commits.push(event.clone()),
                ),
            }
            .unwrap_err();
            assert_eq!(failure.code, MaintenanceCode::Stale);
            assert!(!failure.outcome.committed());
            assert!(commits.is_empty());
            assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), before);
            assert!(!quarantined(p, "raw.db"));
            assert!(!quarantined(p, "knowledge.db"));
        }
    }
}

#[test]
fn w5b_restore_uses_the_confirmed_log_snapshot_through_an_aba_change() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let (original, changed) = w5b_log_change(p);
    let paths: Vec<_> = [p.join("forget.log"), dir(p).unwrap().join("forget.log")]
        .into_iter()
        .map(|path| {
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            (path, modified)
        })
        .collect();
    let original_job = raw::open(p).unwrap().forget_requests().unwrap()[0]
        .job
        .clone();
    let preview = preview_restore(p).unwrap();
    let changed_paths = paths.clone();
    AFTER_CONSENT.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            for (path, modified) in changed_paths {
                std::fs::write(&path, &changed).unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }
        }));
    });
    BEFORE_SWAP.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            for (path, modified) in paths {
                std::fs::write(&path, &original).unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }
        }));
    });
    crate::worker::restore_report(
        p,
        Some(&preview.key),
        crate::executable::CommandCaller::Worker,
        &mut |_| {},
    )
    .unwrap();
    let requests = raw::open(p).unwrap().forget_requests().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "a transient unconfirmed request entered the restored store"
    );
    assert_eq!(requests[0].job, original_job);
    assert!(
        crate::search::raw(p, "confirmed log canary", None, 5)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn w5b_quarantined_segments_are_counted_once_with_each_kept_file() {
    for fail_after_first_file in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 6, 3);
        let bad = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1);
        std::fs::write(&bad.path, "synthetic damaged segment").unwrap();
        let preview = preview_restore(p).unwrap();
        if fail_after_first_file {
            FAIL_AFTER.set(Some("segments_quarantined"));
        }
        let result = crate::worker::restore_report(
            p,
            Some(&preview.key),
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        );
        FAIL_AFTER.set(None);
        let outcome = if fail_after_first_file {
            *result.unwrap_err().outcome
        } else {
            result.unwrap()
        };
        let receipt = outcome.restore.unwrap();
        assert_eq!(receipt.skipped_segments, 1);
        assert_eq!(receipt.effects.segments_quarantined, 1);
        assert_eq!(
            receipt.backup_files_kept,
            if fail_after_first_file { 1 } else { 2 }
        );
        assert!(receipt.effects.committed());
    }
}

#[cfg(unix)]
#[test]
fn w5b_a_home_replaced_during_post_restore_reopen_is_untouched() {
    for stopped in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("memory");
        let replacement = root.path().join("replacement");
        let retired = root.path().join("retired");
        std::fs::create_dir(&home).unwrap();
        segmented(&home, 3, 3);
        std::fs::create_dir_all(replacement.join("state")).unwrap();
        std::fs::write(replacement.join("state/worker.lock"), "other home").unwrap();
        std::fs::write(replacement.join("canary"), "synthetic unrelated home").unwrap();
        if stopped {
            drop(raw::open(&replacement).unwrap());
            std::fs::rename(
                replacement.join("raw.db"),
                replacement.join("raw.db.restored"),
            )
            .unwrap();
        }
        let before = w5b_files(&replacement);
        let path = home.clone();
        BEFORE_REOPEN.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                let held = raw::lock_for_swap(&path).unwrap();
                raw::SWAP_BLOCKED.with_borrow_mut(|hook| {
                    *hook = Some(Box::new(move || {
                        std::fs::rename(&path, &retired).unwrap();
                        std::fs::rename(&replacement, &path).unwrap();
                        drop(held);
                    }));
                });
            }));
        });
        let result = crate::worker::restore_report(
            &home,
            None,
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        );
        BEFORE_REOPEN.with_borrow_mut(|hook| *hook = None);
        raw::SWAP_BLOCKED.with_borrow_mut(|hook| *hook = None);
        assert_eq!(
            w5b_files(&home),
            before,
            "post-restore reopen wrote into the replacement home (stopped={stopped})"
        );
        let failure = result.unwrap_err();
        assert!(failure.outcome.restore.unwrap().effects.raw_swapped);
        assert_eq!(
            failure.outcome.index.state,
            crate::worker::IndexState::Failed
        );
    }
}

#[test]
fn w5b_readonly_previews_keep_the_native_stores_wals_logs_and_paths_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event("synthetic hot WAL")).unwrap();
    let knowledge = crate::knowledge::open(p).unwrap();
    knowledge
        .execute_batch("CREATE TABLE preview_canary(x); INSERT INTO preview_canary VALUES(1)")
        .unwrap();
    let before = w5b_files(p);
    let rebuild = crate::worker::preview_rebuild(p).unwrap();
    let restore = preview_restore(p).unwrap();
    assert_eq!(rebuild.raw.unwrap().records, 4);
    assert_eq!(restore.backup.unwrap().record_lines, Some(3));
    assert!(!rebuild.hybrid_ready && !restore.hybrid_ready);
    assert_ne!(rebuild.key, restore.key);
    assert_eq!(w5b_files(p), before);
    assert!(!p.join("state/rebuild.lock").exists());
    assert!(!p.join("providers.db").exists());
    let absent = p.join("unconfigured");
    assert!(crate::worker::preview_rebuild(&absent).is_err());
    assert!(preview_restore(&absent).is_err());
    assert!(!absent.exists());
}

#[test]
fn w5b_changed_confirmation_refuses_before_any_data_effect() {
    for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        drop(crate::knowledge::open(p).unwrap());
        let shown = preview_maintenance(p, operation).unwrap();
        std::fs::write(p.join("config.toml"), "[capture]\nstore_prompts = false\n").unwrap();
        let before = w5b_files(p);
        let mut commits = Vec::new();
        let failure = match operation {
            MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut |event| commits.push(event.clone()),
            ),
            MaintenanceOperation::Restore => crate::worker::restore_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut |event| commits.push(event.clone()),
            ),
        }
        .unwrap_err();
        assert_eq!(failure.code, MaintenanceCode::Stale);
        assert!(!failure.outcome.committed());
        assert!(commits.is_empty());
        assert_eq!(w5b_files(p), before);
    }
}

#[test]
fn w5b_a_failure_after_quarantine_keeps_the_completed_swap_and_old_store_receipt() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    let shown = preview_restore(p).unwrap();
    FAIL_AFTER.set(Some("raw_quarantine_complete"));
    let failure = crate::worker::restore_report(
        p,
        Some(&shown.key),
        crate::executable::CommandCaller::Worker,
        &mut |_| {},
    )
    .unwrap_err();
    FAIL_AFTER.set(None);
    let restored = failure.outcome.restore.as_ref().unwrap();
    assert_eq!(
        failure.outcome.index.state,
        crate::worker::IndexState::Complete
    );
    assert!(restored.effects.raw_swapped);
    assert!(restored.effects.stopped_restore_finished);
    assert_eq!(restored.replayed_records, 3);
    assert!(restored.old_raw_kept_files > 0);
    assert!(failure.outcome.committed());
    assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 3);
    assert!(quarantined(p, "raw.db"));
}

#[test]
fn w5b_a_first_sidecar_failure_keeps_the_live_wal_and_unlogged_forget() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    let mut event = raw::test_event(r#"{"prompt":"synthetic live WAL denial"}"#);
    event.source = "transcript".into();
    let identity = raw::ImportIdentity {
        origin: crate::forget::origin("synthetic", "live-wal-denial"),
        session: crate::forget::session(&event.agent, &event.session),
        ambiguous: None,
        unverified: false,
    };
    let seq = raw
        .append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            &[identity],
            "",
            None,
        )
        .unwrap()[0];
    let device = raw.device().to_owned();
    export(p).unwrap();
    drop(raw);
    let raw = raw::open(p).unwrap();
    let shown = raw
        .forget_preview(crate::forget::Target::Record {
            device: device.clone(),
            seq,
        })
        .unwrap();
    crate::forget::start(p, &shown).unwrap();
    for ext in ["", "-wal"] {
        std::fs::copy(
            p.join(format!("raw.db{ext}")),
            p.join(format!("saved{ext}")),
        )
        .unwrap();
    }
    drop(raw);
    for ext in ["", "-wal"] {
        std::fs::copy(
            p.join(format!("saved{ext}")),
            p.join(format!("raw.db{ext}")),
        )
        .unwrap();
    }
    for log in [p.join("forget.log"), p.join("backups/forget.log")] {
        std::fs::remove_file(log).unwrap();
    }
    let shown = preview_restore(p).unwrap();
    FAIL_AFTER.set(Some("raw_quarantined"));
    let failed = crate::worker::restore_report(
        p,
        Some(&shown.key),
        crate::executable::CommandCaller::Worker,
        &mut |_| {},
    )
    .unwrap_err();
    FAIL_AFTER.set(None);
    assert!(!failed.outcome.restore.as_ref().unwrap().effects.raw_swapped);
    assert!(
        failed
            .outcome
            .restore
            .as_ref()
            .unwrap()
            .effects
            .raw_put_back
    );
    assert_eq!(
        failed.outcome.restore.as_ref().unwrap().old_raw_kept_files,
        0
    );
    assert!(failed.outcome.committed());
    let live = raw::open(p).unwrap();
    assert_eq!(live.forget_requests().unwrap().len(), 1);
    assert!(matches!(
        live.after(&device, seq - 1, 1).unwrap()[0].item,
        Item::Removed
    ));
}

#[test]
fn w5b_stopped_or_damaged_rebuild_previews_require_recovery_without_finishing_it() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    std::fs::rename(p.join("raw.db"), p.join("raw.db.restored")).unwrap();
    let before = w5b_files(p);
    for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
        assert_eq!(
            preview_maintenance(p, operation)
                .unwrap_err()
                .downcast_ref::<MaintenanceCode>(),
            Some(&MaintenanceCode::RecoveryRequired)
        );
    }
    assert_eq!(w5b_files(p), before);
    std::fs::rename(p.join("raw.db.restored"), p.join("raw.db")).unwrap();
    damage_raw(p);
    let before = w5b_files(p);
    assert_eq!(
        crate::worker::preview_rebuild(p)
            .unwrap_err()
            .downcast_ref::<MaintenanceCode>(),
        Some(&MaintenanceCode::RecoveryRequired)
    );
    let restore = preview_restore(p).unwrap();
    assert!(restore.raw.is_none());
    assert_eq!(restore.backup.unwrap().record_lines, Some(3));
    assert_eq!(w5b_files(p), before);
}

#[test]
fn w5b_backup_preview_reports_bad_segments_and_the_contiguous_op_prefix() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 6, 3);
    let records = segments(&p.join("backups"), Kind::Records).unwrap();
    std::fs::write(&records[1].path, b"synthetic damaged segment").unwrap();
    let mut raw = raw::open(p).unwrap();
    for _ in 0..3 {
        raw.append_ops(&[(
            raw::OpKind::Exclusion,
            serde_json::json!({"repo":"synthetic","undo":true}),
        )])
        .unwrap();
        export(p).unwrap();
    }
    drop(raw);
    let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
    std::fs::remove_file(&ops[1].path).unwrap();
    let before = w5b_files(p);
    let shown = preview_restore(p).unwrap();
    let backup = shown.backup.unwrap();
    assert_eq!(
        (backup.record_segments, backup.invalid_record_segments),
        (1, 1)
    );
    assert_eq!(
        (
            backup.op_segments,
            backup.skipped_op_segments,
            backup.op_prefix_through
        ),
        (1, 1, 1)
    );
    assert_eq!(backup.record_lines, Some(3));
    assert!(!backup.exact_drops_known);
    assert_eq!(w5b_files(p), before);
    let safe = serde_json::to_string(&preview_restore(p).unwrap()).unwrap();
    assert!(!safe.contains(&p.to_string_lossy().into_owned()));
    assert!(!safe.contains("damaged segment"));
}

#[test]
fn w5b_preview_refuses_aliases_and_does_not_guess_unreadable_mixed_device_metadata() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    let segment = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(0)
        .path;
    std::fs::remove_file(&segment).unwrap();
    std::fs::hard_link(p.join("raw.db"), &segment).unwrap();
    let before = w5b_files(p);
    assert!(preview_restore(p).is_err());
    assert_eq!(w5b_files(p), before);

    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    let segment = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(0)
        .path;
    let other = p.join("backups/otherdevice-000000000001-000000000003.seg.zst");
    std::fs::copy(&segment, &other).unwrap();
    std::fs::copy(
        PathBuf::from(format!("{}.sha256", segment.display())),
        PathBuf::from(format!("{}.sha256", other.display())),
    )
    .unwrap();
    damage_raw(p);
    let before = w5b_files(p);
    let failed = preview_restore(p).unwrap_err();
    assert!(failed.to_string().contains("2 devices"));
    assert_eq!(w5b_files(p), before);
}

#[test]
fn w5b_partial_staging_is_disclosed_and_is_not_finished_by_preview() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    std::fs::rename(p.join("raw.db"), p.join("raw.db.restoring")).unwrap();
    let before = w5b_files(p);
    let shown = preview_restore(p).unwrap();
    assert!(shown.staged_partial && shown.raw.is_none());
    assert_eq!(shown.backup.unwrap().record_lines, Some(3));
    assert!(!p.join("raw.db").exists());
    assert_eq!(w5b_files(p), before);
}

#[cfg(unix)]
#[test]
fn w5b_readonly_preview_copies_a_closed_hot_wal_without_source_sidecars() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    {
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("synthetic WAL-only event"))
            .unwrap();
        for ext in ["", "-wal"] {
            std::fs::copy(p.join(format!("raw.db{ext}")), p.join(format!("copy{ext}"))).unwrap();
        }
    }
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(p.join(format!("raw.db{ext}")));
    }
    for ext in ["", "-wal"] {
        std::fs::rename(p.join(format!("copy{ext}")), p.join(format!("raw.db{ext}"))).unwrap();
    }
    let before = w5b_files(p);
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o500)).unwrap();
    let shown = crate::worker::preview_rebuild(p);
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(shown.unwrap().raw.unwrap().records, 1);
    assert_eq!(w5b_files(p), before);
    assert!(!p.join("raw.db-shm").exists());
}

#[test]
fn w5b_raw_writes_after_admission_are_rechecked_under_the_swap_fence() {
    for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        let shown = preview_maintenance(p, operation).unwrap();
        let root = p.to_owned();
        BEFORE_SWAP.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                raw::open(&root)
                    .unwrap()
                    .append(&raw::test_event("synthetic unconfirmed hook event"))
                    .unwrap();
            }))
        });
        let mut commits = Vec::new();
        let failure = match operation {
            MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut |event| commits.push(event.clone()),
            ),
            MaintenanceOperation::Restore => crate::worker::restore_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut |event| commits.push(event.clone()),
            ),
        }
        .unwrap_err();
        assert_eq!(failure.code, MaintenanceCode::Stale);
        assert!(!failure.outcome.committed());
        assert!(commits.is_empty());
        assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 4);
        assert!(!quarantined(p, "raw.db"));
        assert!(!quarantined(p, "knowledge.db"));
    }
}

#[test]
fn w5b_the_confirmed_configuration_remains_locked_during_consumer_commits() {
    for operation in [MaintenanceOperation::Rebuild, MaintenanceOperation::Restore] {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        let shown = preview_maintenance(p, operation).unwrap();
        let mut observed = false;
        let mut committed = |event: &crate::worker::MaintenanceCommit| {
            if matches!(event, crate::worker::MaintenanceCommit::Index { .. }) {
                let lock = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(p.join("state/config.lock"))
                    .unwrap();
                observed = matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock));
            }
        };
        match operation {
            MaintenanceOperation::Rebuild => crate::worker::rebuild_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut committed,
            ),
            MaintenanceOperation::Restore => crate::worker::restore_report(
                p,
                Some(&shown.key),
                crate::executable::CommandCaller::Worker,
                &mut committed,
            ),
        }
        .unwrap();
        assert!(
            observed,
            "configuration writers could change the confirmed rules during {operation:?}"
        );
    }
}

#[test]
fn w5b_restore_receipts_retain_post_swap_failures_and_search_completion() {
    for stage in [
        "raw_swapped",
        "directory_synced",
        "restore_note",
        "index_commit",
    ] {
        if stage == "directory_synced" && !cfg!(unix) {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        let shown = preview_restore(p).unwrap();
        FAIL_AFTER.set(Some(stage));
        let failure = crate::worker::restore_report(
            p,
            Some(&shown.key),
            crate::executable::CommandCaller::Worker,
            &mut |_| {},
        )
        .unwrap_err();
        FAIL_AFTER.set(None);
        let restored = failure.outcome.restore.as_ref().unwrap();
        assert!(restored.effects.raw_swapped, "{stage}: {failure:?}");
        assert!(failure.outcome.committed());
        assert_eq!(restored.replayed_records, 3);
        assert!(restored.old_raw_kept_files > 0);
        assert_eq!(
            failure.outcome.index.state,
            if stage == "index_commit" {
                crate::worker::IndexState::Failed
            } else {
                crate::worker::IndexState::Complete
            }
        );
        assert!(!failure.outcome.hybrid_ready);
    }
}

#[test]
fn w5b_stopped_complete_restore_keeps_the_live_forget_authority() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    let mut event = raw::test_event(r#"{"prompt":"synthetic stopped-store canary"}"#);
    event.source = "transcript".into();
    let identity = raw::ImportIdentity {
        origin: crate::forget::origin("synthetic", "stopped-authority"),
        session: crate::forget::session(&event.agent, &event.session),
        ambiguous: None,
        unverified: false,
    };
    let seq = raw
        .append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            &[identity],
            "",
            None,
        )
        .unwrap()[0];
    export(p).unwrap(); // The backup predates the sole live denial.
    let preview = raw
        .forget_preview(crate::forget::Target::Record {
            device: raw.device().into(),
            seq,
        })
        .unwrap();
    crate::forget::start(p, &preview).unwrap();
    drop(raw);
    for log in [p.join("forget.log"), p.join("backups/forget.log")] {
        std::fs::remove_file(log).unwrap();
    }
    std::fs::rename(p.join("raw.db"), p.join("raw.db.restored")).unwrap();
    restore(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.forget_requests().unwrap().len(), 1);
    assert!(matches!(
        restored.after(restored.device(), seq - 1, 1).unwrap()[0].item,
        Item::Removed
    ));
    assert!(
        crate::search::raw(p, "stopped-store canary", None, 5)
            .unwrap()
            .is_empty()
    );
}

fn overwrite(path: &Path, at: u64, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(bytes).unwrap();
}

fn quarantined(home: &Path, name: &str) -> bool {
    std::fs::read_dir(home).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{name}.quarantined-"))
    })
}

#[test]
fn a_damaged_raw_db_is_quarantined_and_rebuilt_from_its_segments() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    for i in 0..100 {
        raw.append(&raw::test_event(&format!("event {i}"))).unwrap();
    }
    let (before, device) = (raw.hashes().unwrap(), raw.device().to_owned());
    assert!(export(p).unwrap().is_some());
    assert!(export(p).unwrap().is_none()); // nothing new
    drop(raw); // the last close checkpoints the WAL
    overwrite(&p.join("raw.db"), 4096, &[0xA5; 4096]); // page 2
    crate::worker::run_once(p).unwrap(); // quick_check fails: quarantine, restore
    assert!(quarantined(p, "raw.db"));
    let restored = raw::open(p).unwrap();
    assert_eq!(
        (restored.hashes().unwrap(), restored.device().to_owned()),
        (before, device)
    );
    assert!(
        std::fs::read_to_string(p.join("state/restored"))
            .unwrap()
            .contains("100 record(s)")
    );
    let seg = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(0)
        .path;
    overwrite(&seg, 10, &[0xA5; 16]);
    assert_eq!(verify(&p.join("backups")).unwrap().len(), 1); // the checksum names it
}

/// docs/store-format.md: a refused open is never damage, whatever context names it.
#[test]
fn a_newer_format_is_never_damage() {
    let newer = anyhow::Error::new(raw::NewerFormat { found: 9 }).context("quick_check");
    assert!(!damaged(&newer));
}

/// docs/store-format.md: `oboete restore` does not put an older home in place of a raw.db a
/// newer oboete wrote: it is refused, and raw.db stays where it is, as it was.
#[test]
fn a_restore_refuses_a_raw_db_a_newer_oboete_wrote() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event("kept")).unwrap();
    assert!(export(p).unwrap().is_some());
    drop(raw);
    rusqlite::Connection::open(p.join("raw.db"))
        .unwrap()
        .execute_batch(&format!("PRAGMA user_version = {}", raw::FORMAT + 1))
        .unwrap();
    let before = std::fs::read(p.join("raw.db")).unwrap();
    let e = crate::worker::restore(p).unwrap_err();
    assert!(e.is::<raw::NewerFormat>(), "{e:#}");
    assert!(!quarantined(p, "raw.db"));
    assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), before);
}

#[test]
fn tombstones_ledger_and_removed_records_survive_a_restore() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    let (masked, found) = crate::redact::scan(
        &format!("alpha {key} zqx-private-words tail"),
        &crate::redact::Rules::default(),
    );
    let finding = found.into_iter().next().unwrap();
    let a = raw
        .append_with_ledger(
            &raw::test_event(&masked),
            &[("/prompt".into(), finding)],
            "test",
        )
        .unwrap()
        .unwrap();
    let b = raw.append(&raw::test_event("bravo visible")).unwrap();
    let dev = raw.device().to_owned();
    raw.append_tombstone(Target::Record {
        device: dev.clone(),
        seq: b,
    })
    .unwrap();
    let at = masked.find("zqx").unwrap() as i64;
    raw.append_tombstone(Target::Range {
        device: dev.clone(),
        seq: a,
        offset: at,
        length: 17,
    })
    .unwrap();
    let before = raw.hashes().unwrap();
    let ledger = |r: &Raw| r.export_lines(0, usize::MAX).unwrap()[0].1.clone();
    let line = ledger(&raw);
    assert!(line.contains("\"ledger\":[{") && !line.contains(&key));
    export(p).unwrap();
    let seg = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(0)
        .path;
    let mut text = String::new();
    zstd::Decoder::new(std::fs::File::open(&seg).unwrap())
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert!(!text.contains("zqx-private-words") && !text.contains("bravo"));
    drop(raw);
    std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(p.join(f));
    }
    // The id cannot be read from the file: it comes from the one device the segments name.
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.device(), dev);
    assert_eq!(restored.hashes().unwrap(), before); // masking twice changes nothing
    assert_eq!(ledger(&restored), line);
    let recs = restored.after(&dev, 0, 10).unwrap();
    assert!(matches!(recs[1].item, Item::Removed));
}

#[test]
fn a_damaged_knowledge_db_is_rebuilt_and_raw_and_segments_stay() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    for i in 0..200 {
        raw.append(&raw::test_event(&format!("note zq{i:03}x kept")))
            .unwrap();
    }
    drop(raw);
    crate::worker::run_once(p).unwrap(); // indexes and backs up at idle exit
    let seg = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(0)
        .path;
    let (raw_bytes, seg_bytes) = (
        std::fs::read(p.join("raw.db")).unwrap(),
        std::fs::read(&seg).unwrap(),
    );
    overwrite(&p.join("knowledge.db"), 4096, &[0xA5; 4096]);
    crate::worker::run_once(p).unwrap();
    assert!(quarantined(p, "knowledge.db"));
    assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), raw_bytes);
    assert_eq!(std::fs::read(&seg).unwrap(), seg_bytes);
    let hits = crate::search::raw(p, "zq007x", None, 5).unwrap();
    assert_eq!(hits.first().map(|h| h.seq), Some(8));
}

#[test]
fn a_knowledge_db_opens_without_checking_its_search_indexes_against_their_text() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event("note zq001x kept")).unwrap();
    drop(raw);
    crate::worker::run_once(p).unwrap();
    let k = crate::knowledge::open(p).unwrap();
    // A search index whose text is gone while the index keeps its terms (raw_fts keeps no
    // text since #317; the claims' and the cards' do).
    k.execute_batch(
        "CREATE VIRTUAL TABLE kept_fts USING fts5(text, tokenize='trigram');
             INSERT INTO kept_fts(rowid, text) VALUES (1, 'note zq001x kept');
             DELETE FROM kept_fts_content;",
    )
    .unwrap();
    assert!(crate::db::quick_check(&k, "knowledge.db").is_err());
    drop(k);
    open_knowledge(p).unwrap();
    assert!(!quarantined(p, "knowledge.db"));
    let e = crate::setup::doctor(p).unwrap_err().to_string();
    assert!(e.contains("knowledge.db is damaged"), "{e}");
}

#[test]
fn a_knowledge_db_whose_two_tables_share_a_page_is_rebuilt() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    drop(crate::knowledge::open(p).unwrap());
    rusqlite::Connection::open(p.join("knowledge.db"))
        .unwrap()
        .execute_batch(
            "PRAGMA writable_schema = ON;
                 UPDATE sqlite_schema SET rootpage =
                   (SELECT rootpage FROM sqlite_schema WHERE name = 'checkpoints')
                 WHERE name = 'rewinds';",
        )
        .unwrap();
    open_knowledge(p).unwrap();
    assert!(quarantined(p, "knowledge.db"));
}

#[test]
fn doctor_finds_a_damaged_table_before_it_reads_it() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let (root, size): (i64, i64) = crate::knowledge::open(p)
        .unwrap()
        .query_row(
            "SELECT rootpage, (SELECT page_size FROM pragma_page_size)
                 FROM sqlite_schema WHERE name = 'rewinds'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let (root, size) = (root as u64, size as usize);
    overwrite(
        &p.join("knowledge.db"),
        (root - 1) * size as u64,
        &vec![0xA5; size],
    );
    let e = crate::setup::doctor(p).unwrap_err().to_string();
    assert!(e.contains("knowledge.db is damaged"), "{e}");
}

/// raw.db of `n` events, one segment exported after each `per` of them.
fn segmented(p: &Path, n: usize, per: usize) {
    let mut raw = raw::open(p).unwrap();
    for i in 0..n {
        raw.append(&raw::test_event(&format!("rec zq{i:03}x")))
            .unwrap();
        if (i + 1) % per == 0 {
            export(p).unwrap();
        }
    }
}

fn damage_raw(p: &Path) {
    std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(p.join(f));
    }
}

/// R14: a restore marks each segment it replays, so that a large one, which a worker runs
/// when asked to after its loop has begun, is not taken for a stuck loop (CodeRabbit on #613).
#[test]
fn a_restore_marks_each_segment_it_replays() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
    damage_raw(p);
    let watched = crate::watchdog::Watched::here();
    open_raw(p).unwrap();
    assert_eq!(watched.marks(), 3);
}

#[test]
fn a_skipped_segment_leaves_neither_its_records_in_search_nor_its_seqs_unbacked() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
    crate::worker::run_once(p).unwrap(); // index all 30
    let segs = segments(&p.join("backups"), Kind::Records).unwrap();
    assert_eq!(segs.len(), 3);
    std::fs::write(&segs[1].path, b"damaged").unwrap(); // the middle one
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    // Derived data was rebuilt: nothing of 11-20 is found, 21-30 is.
    let hits = crate::search::raw(p, "zq0", None, 100).unwrap();
    assert!(!hits.is_empty() && hits.iter().all(|h| !(11..=20).contains(&h.seq)));
    assert_eq!(crate::search::raw(p, "zq024x", None, 5).unwrap()[0].seq, 25);
    assert!(verify(&p.join("backups")).unwrap().is_empty()); // set aside, not trusted
    // The newest segment damaged: its seqs are reused and backed up again.
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 20, 10);
    let last = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(1)
        .path;
    std::fs::write(&last, b"damaged").unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let mut raw = raw::open(p).unwrap();
    assert_eq!(raw.max_seq().unwrap(), 10);
    raw.append(&raw::test_event("after the restore")).unwrap();
    drop(raw);
    export(p).unwrap();
    let segs = segments(&p.join("backups"), Kind::Records).unwrap();
    assert_eq!(segs.last().map(|s| (s.first, s.last)), Some((11, 11)));
}

#[test]
fn no_store_opens_while_a_restore_swaps_the_file_and_a_stopped_swap_is_finished() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 5, 5);
    let device = raw::open(p).unwrap().device().to_owned();
    let held = raw::lock_for_swap(p).unwrap();
    let t = std::time::Instant::now();
    assert!(raw::open(p).is_err()); // a hook's write waits, then fails (MUST-M16's marker)
    assert!(t.elapsed() >= std::time::Duration::from_secs(1));
    drop(held);
    // Stopped after the damaged file was moved aside: the rebuilt one is renamed in.
    std::fs::rename(p.join("raw.db"), p.join("raw.db.restored")).unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(p.join(f));
    }
    // A search finishes it too, before it reads.
    crate::search::raw(p, "event", None, 5).unwrap();
    assert!(p.join("raw.db").exists() && !p.join("raw.db.restored").exists());
    let raw = raw::open(p).unwrap();
    assert_eq!(
        (raw.device().to_owned(), raw.max_seq().unwrap()),
        (device, 5)
    );
}

#[test]
fn a_removed_segment_is_written_again_from_raw() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
    let ranges = |p: &Path| {
        let mut r: Vec<(i64, i64)> = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .iter()
            .map(|s| (s.first, s.last))
            .collect();
        r.sort();
        r
    };
    let middle = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .into_iter()
        .find(|s| s.first == 11)
        .unwrap()
        .path;
    std::fs::remove_file(&middle).unwrap();
    export(p).unwrap();
    assert_eq!(ranges(p), [(1, 10), (11, 30)]);
}

#[test]
fn records_raw_lost_are_backed_up_again_as_their_seqs_are_reused() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 10); // one segment: 1-10
    rusqlite::Connection::open(p.join("raw.db"))
        .unwrap()
        .execute("DELETE FROM records WHERE seq > 6", []) // lost commits (MUST-M14)
        .unwrap();
    let mut raw = raw::open(p).unwrap();
    for i in 0..2 {
        raw.append(&raw::test_event(&format!("new zn{i}x")))
            .unwrap();
    }
    drop(raw);
    export(p).unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap(); // restored from the backups
    let raw = raw::open(p).unwrap();
    let bodies: Vec<String> = raw
        .after(raw.device(), 0, 100)
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.item {
            raw::Item::Event(e) => Some(e.body),
            _ => None,
        })
        .collect();
    assert_eq!(bodies.len(), 8, "{bodies:?}");
    assert!(
        bodies[7].contains("zn1x") && !bodies.iter().any(|b| b.contains("zq009x")),
        "{bodies:?}"
    );
}

#[test]
fn a_tombstoned_key_never_reaches_a_segment_through_the_ledger() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    let (masked, found) = crate::redact::scan(
        &format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"),
        &crate::redact::Rules::default(),
    );
    let body = serde_json::json!({"trigger": {"zqx-private-words": masked}}).to_string();
    let finding = found.into_iter().next().unwrap();
    let seq = raw
        .append_with_ledger(
            &raw::test_event(&body),
            &[("/trigger/zqx-private-words".into(), finding)],
            "test",
        )
        .unwrap()
        .unwrap();
    let at = body.find("zqx").unwrap() as i64;
    raw.append_tombstone(raw::Target::Range {
        device: raw.device().to_owned(),
        seq,
        offset: at,
        length: 17,
    })
    .unwrap();
    let lines = raw.export_lines(0, usize::MAX).unwrap();
    assert!(!lines[0].1.contains("zqx"), "{}", lines[0].1);
    assert!(lines[0].1.contains("~tombstoned"));
}

#[test]
fn a_partial_rebuild_is_never_renamed_in() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    // A build that stopped before its records were committed keeps the `.restoring` name.
    std::fs::rename(p.join("raw.db"), p.join("raw.db.restoring")).unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(p.join(f));
    }
    assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 0);
    assert!(p.join("raw.db.restoring").exists());
}

#[test]
fn only_damage_restores_not_a_busy_or_missing_file() {
    assert!(damaged(&anyhow::anyhow!(
        "raw.db: quick_check: *** in database main *** Page 2: btreeInitPage() returns error code 11"
    )));
    let busy =
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None);
    assert!(!damaged(
        &anyhow::Error::from(busy).context("raw.db: quick_check")
    ));
    let corrupt = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
        None,
    );
    assert!(damaged(&anyhow::Error::from(corrupt)));
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(!damaged(&anyhow::Error::from(denied)));
}

#[test]
fn a_backup_in_a_synced_folder_is_named_in_doctor() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[backup]\ndir = \"OneDrive - Contoso/oboete\"\n",
    )
    .unwrap();
    let (lines, _) = doctor(home.path());
    assert!(
        lines
            .iter()
            .any(|l| l.contains("inside OneDrive - Contoso")),
        "{lines:?}"
    );
    for p in [
        "/Users/a/Library/Mobile Documents/x",
        "/Users/a/Dropbox/x",
        "/g/Google Drive/x",
    ] {
        assert!(in_cloud_folder(Path::new(p)).is_some(), "{p}");
    }
    assert!(in_cloud_folder(Path::new("/home/a/.oboete/backups")).is_none());
}

/// docs/backup-points.md test 1: an export that writes a segment notes one point with both
/// cursors, one that writes nothing notes none, and a torn line costs only itself.
#[test]
fn an_export_notes_a_point_only_when_it_writes() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // two exports: 1-5, 6-10
    let dir = p.join("backups");
    let cursors = |dir: &Path| -> Vec<(i64, i64)> {
        points(dir)
            .unwrap()
            .iter()
            .map(|x| (x.records, x.ops))
            .collect()
    };
    assert_eq!(cursors(&dir), [(5, 0), (10, 0)]);
    export(p).unwrap(); // nothing new
    assert_eq!(cursors(&dir), [(5, 0), (10, 0)]);
    // A line an export stopped in the middle of.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join(POINTS))
        .unwrap();
    f.write_all(b"{\"at\": 17").unwrap();
    drop(f);
    let mut raw = raw::open(p).unwrap();
    raw.append_ops(&[window(10), claim("a")]).unwrap();
    drop(raw);
    export(p).unwrap(); // ops only
    assert_eq!(cursors(&dir), [(5, 0), (10, 0), (10, 2)]);
}

/// A store of three points: A (5 records, no op), B (A and two ops) and C, past B with a mute
/// (before any op made from records, so only the point's cut takes it), five records whose own
/// times are older than every point (as an import's are), a window, an exclusion and a forget.
/// The home, the device and the forgotten uid.
fn three_points() -> (tempfile::TempDir, String, String) {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 5, 5); // A
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    raw.append_ops(&[window(5), claim("a")]).unwrap();
    drop(raw);
    export(p).unwrap(); // B
    let mut raw = raw::open(p).unwrap();
    raw.append_ops(&[(
        raw::OpKind::Correction,
        serde_json::json!({"uid": "a".repeat(64), "muted": true}),
    )])
    .unwrap();
    for i in 0..5 {
        let mut old = raw::test_event(&format!("imported zi{i}x"));
        old.ts = 1_000 + i; // 1970: older than every point
        raw.append(&old).unwrap();
    }
    raw.append_ops(&[window(10), claim("b")]).unwrap();
    raw.exclude("github.com/o/secret", false).unwrap();
    let uid = "f".repeat(64);
    raw.append_ops(&[(
        raw::OpKind::Forget,
        serde_json::json!({"uid": uid, "job": "j"}),
    )])
    .unwrap();
    drop(raw);
    export(p).unwrap(); // C
    (home, dev, uid)
}

fn op_kinds(raw: &Raw, dev: &str) -> Vec<(i64, raw::OpKind)> {
    raw.ops_after(dev, 0, 100)
        .unwrap()
        .iter()
        .map(|o| (o.op_seq, o.kind))
        .collect()
}

/// docs/backup-points.md tests 2, 4 and 5: going back to a point keeps its records and ops,
/// whatever the records' own times, and the forget and exclusion ops after it; a mute after
/// it goes; no point past it stays; and an older point can be gone back to after a new one.
#[test]
fn going_back_to_a_point_keeps_its_prefix_and_the_gates_after_it() {
    let (home, dev, uid) = three_points();
    let p = home.path();
    let dir = p.join("backups");
    let [a, b, c]: [Point; 3] = points(&dir).unwrap().try_into().unwrap();
    assert_eq!((b.records, b.ops, c.records, c.ops), (5, 2, 10, 7));
    let said = crate::worker::restore_to(p, &b).unwrap();
    assert!(said.contains("taken back to the backup point"), "{said}");
    let raw = raw::open(p).unwrap();
    assert_eq!(raw.max_seq().unwrap(), 5);
    use raw::OpKind::{Claim, Exclusion, Forget, Window};
    // The gates in their op order: the exclusion (op 6) before the forget (op 7).
    assert_eq!(
        op_kinds(&raw, &dev),
        [(1, Window), (2, Claim), (3, Exclusion), (4, Forget)]
    );
    assert!(raw.forgotten(&uid).unwrap());
    assert_eq!(raw.exclusions().unwrap(), ["github.com/o/secret"]);
    drop(raw);
    // C is gone; the restore's own drain backed up what it appended again, a point of its own.
    let after_b = points(&dir).unwrap();
    assert_eq!(after_b[..2], [a.clone(), b.clone()]);
    assert!(!after_b.contains(&c), "{after_b:?}");
    assert!(after_b.iter().all(|x| x.records <= 5), "{after_b:?}");
    let live = |kind| -> Vec<(i64, i64)> {
        segments(&dir, kind)
            .unwrap()
            .iter()
            .map(|s| (s.first, s.last))
            .collect()
    };
    assert_eq!(live(Kind::Records), [(1, 5)]);
    assert_eq!(live(Kind::Ops), [(1, 2), (3, 4)]);
    // A new point after it, then back to A: the gates are kept again.
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event("after zaq")).unwrap();
    drop(raw);
    export(p).unwrap();
    assert_eq!(points(&dir).unwrap().last().map(|x| x.records), Some(6));
    crate::worker::restore_to(p, &a).unwrap();
    let raw = raw::open(p).unwrap();
    assert_eq!(raw.max_seq().unwrap(), 5);
    assert_eq!(op_kinds(&raw, &dev), [(1, Exclusion), (2, Forget)]);
    assert_eq!(raw.exclusions().unwrap(), ["github.com/o/secret"]);
    let after_a = points(&dir).unwrap();
    assert_eq!(after_a.first(), Some(&a));
    assert!(!after_a.contains(&b), "{after_a:?}");
    assert!(after_a.iter().all(|x| x.records <= 5), "{after_a:?}");
}

/// docs/backup-points.md test 3: what a point took back stays back when raw.db is damaged
/// right after the swap, before any export, and is restored: the segments past the point were
/// set aside at the swap.
#[test]
fn a_damaged_store_after_going_back_is_restored_to_the_point() {
    let (home, dev, _) = three_points();
    let p = home.path();
    let b = points(&p.join("backups")).unwrap().remove(1);
    let config = crate::settings::config_lock(p).unwrap();
    restore_report_holding_to(p, None, None, Some(&b), &mut |_| {}).unwrap();
    drop(config);
    damage_raw(p);
    let raw = open_raw(p).unwrap(); // the automatic restore
    assert_eq!(raw.max_seq().unwrap(), 5);
    // The ops the backups held through the point. The gates' ops appended again after the swap
    // are backed up by the restore's own drain, which this test skips to reach the moment
    // before it, as any new op waits for the next backup.
    use raw::OpKind::{Claim, Window};
    assert_eq!(op_kinds(&raw, &dev), [(1, Window), (2, Claim)]);
}

/// docs/backup-points.md test 6: the preview changes no file and names the point that `--yes`
/// then goes back to, by the time the list prints.
#[test]
fn the_preview_changes_nothing_and_names_the_point_yes_takes() {
    let (home, _, _) = three_points();
    let p = home.path();
    let b = points(&p.join("backups")).unwrap().remove(1);
    fn files(p: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        for d in [p.to_owned(), p.join("backups")] {
            for e in std::fs::read_dir(d).unwrap() {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                let name = e.file_name().to_string_lossy().into_owned();
                // SQLite makes these beside a WAL store on any open, a hook's too.
                if m.is_file() && !name.ends_with("-wal") && !name.ends_with("-shm") {
                    out.push((e.path(), m.len(), m.modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    }
    let before = files(p);
    let to = point_time(b.at);
    let preview = point_preview(p, &point_at(p, parse_time(&to).unwrap()).unwrap()).unwrap();
    assert!(preview.contains(&to), "{preview}");
    assert!(
        preview.contains("the 5 record(s) and 3 op(s) after them"),
        "{preview}"
    );
    assert!(
        preview.contains("keeps the 2 forget and exclusion op(s)"),
        "{preview}"
    );
    assert_eq!(files(p), before);
    let chosen = point_at(p, parse_time(&to).unwrap()).unwrap();
    assert_eq!(chosen, b);
    assert!(
        preview.contains(&format!("oboete restore --to {to} --yes")),
        "{preview}"
    );
    // `--yes` takes the point at exactly the time the preview printed, and no other.
    assert_eq!(point_named(p, b.at).unwrap(), b);
    let off = point_named(p, b.at + 1).unwrap_err().to_string();
    assert!(off.contains("exactly as the preview"), "{off}");
    let none = point_at(p, b.at - 86_400_000).unwrap_err().to_string();
    assert!(none.contains("no backup point at or before"), "{none}");
    assert_eq!(
        parse_time("2026-11-07 05:30").unwrap(),
        parse_time("2026-11-07T05:30:00Z").unwrap()
    );
    assert_eq!(
        parse_time("2026-11-07T14:30:00+09:00").unwrap(),
        parse_time("2026-11-07T05:30:00Z").unwrap()
    );
    assert!(parse_time("yesterday").is_err());
}

/// docs/backup-points.md test 11 (B5, Codex on #880): an exclusion raw.db holds and no export
/// backed up yet is carried across the swap, by a restore to a point and by a restore of the
/// whole backup alike, and the same op is never appended twice.
#[test]
fn an_exclusion_no_backup_holds_yet_is_carried_across_a_restore() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 5, 5); // a point at seq 5, no op
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    raw.exclude("github.com/o/new", false).unwrap(); // after the last export
    drop(raw);
    let a = points(&p.join("backups")).unwrap().remove(0);
    crate::worker::restore_to(p, &a).unwrap();
    let raw = raw::open(p).unwrap();
    assert_eq!(raw.exclusions().unwrap(), ["github.com/o/new"]);
    use raw::OpKind::Exclusion;
    assert_eq!(op_kinds(&raw, &dev), [(1, Exclusion)]);
    drop(raw);
    let mut raw = raw::open(p).unwrap();
    raw.exclude("github.com/o/new", true).unwrap(); // the undo, not backed up either
    drop(raw);
    crate::worker::restore(p).unwrap();
    let raw = raw::open(p).unwrap();
    assert!(raw.exclusions().unwrap().is_empty());
    assert_eq!(op_kinds(&raw, &dev), [(1, Exclusion), (2, Exclusion)]);
}

/// Codex on #880: no point is noted before the first record is backed up (an export of ops
/// alone), so no point restores a store with no record; the first export with a record notes
/// one.
#[test]
fn no_point_is_noted_before_the_first_record() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    raw.exclude("github.com/o/first", false).unwrap();
    drop(raw);
    export(p).unwrap();
    let dir = p.join("backups");
    assert_eq!(segments(&dir, Kind::Ops).unwrap().len(), 1);
    assert!(points(&dir).unwrap().is_empty());
    segmented(p, 3, 3);
    let cursors: Vec<(i64, i64)> = points(&dir)
        .unwrap()
        .iter()
        .map(|x| (x.records, x.ops))
        .collect();
    assert_eq!(cursors, [(3, 1)]);
}

/// cubic on #880: raw.db that cannot be read for a reason other than damage stops the restore,
/// rather than dropping the exclusions no backup holds yet; raw.db stays where it is.
#[cfg(unix)]
#[test]
fn an_unreadable_store_stops_the_restore_rather_than_drop_its_exclusions() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 5, 5);
    let mut raw = raw::open(p).unwrap();
    raw.exclude("github.com/o/new", false).unwrap();
    drop(raw);
    let a = points(&p.join("backups")).unwrap().remove(0);
    let file = raw::path(p);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let config = crate::settings::config_lock(p).unwrap();
    let failed = restore_report_holding_to(p, None, None, Some(&a), &mut |_| {});
    drop(config);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let why = format!("{:#}", failed.err().unwrap().cause);
    assert!(why.contains("read raw.db's exclusions"), "{why}");
    let raw = raw::open(p).unwrap();
    assert_eq!(raw.exclusions().unwrap(), ["github.com/o/new"]);
}

/// Codex on #880: a backup run indexes what came while the worker stood aside, before its
/// lock goes, as a restore's drain does.
#[test]
fn a_backup_run_indexes_what_came_while_it_held_the_lock() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event("came while it held the lock zw1"))
        .unwrap();
    drop(raw);
    crate::worker::backup_run(p).unwrap();
    assert_eq!(crate::worker::backlog(p).unwrap(), 0);
}

/// cubic on #880: a point dropped after it was chosen (raw.db lost records past it, and their
/// seqs were used again) is refused under the locks, and raw.db is left as it was.
#[test]
fn a_point_no_longer_listed_is_refused() {
    let (home, _, _) = three_points();
    let p = home.path();
    let dir = p.join("backups");
    let [a, b, c]: [Point; 3] = points(&dir).unwrap().try_into().unwrap();
    let kept = [a, c]
        .iter()
        .map(|x| format!("{}\n", serde_json::to_string(x).unwrap()))
        .collect::<String>();
    std::fs::write(dir.join(POINTS), kept).unwrap();
    let why = crate::worker::restore_to(p, &b).unwrap_err().to_string();
    assert!(why.contains("no longer listed"), "{why}");
    assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 10);
}

/// cubic on #880 (B6): when the segment that ends at the point is damaged, the restore brings
/// back less than the point named, and the seqs past what came back are used again: that
/// point goes too.
#[test]
fn a_point_whose_records_did_not_all_come_back_is_dropped() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // points (5, 0) and (10, 0)
    let dir = p.join("backups");
    let [five, ten]: [Point; 2] = points(&dir).unwrap().try_into().unwrap();
    let newest = segments(&dir, Kind::Records).unwrap().remove(1).path;
    std::fs::write(&newest, b"damaged").unwrap();
    crate::worker::restore_to(p, &ten).unwrap();
    assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 5);
    assert_eq!(points(&dir).unwrap(), [five]);
}

/// Codex on #880: when a restore cuts the ops (records lost) and appends exclusions again in
/// the op seqs it freed, the old ops segment over those seqs is set aside at the swap, so the
/// backups hold what raw.db holds; a later restore brings back both exclusions.
#[test]
fn ops_appended_again_after_a_cut_replace_the_old_ops_segment() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // record segments 1-5 and 6-10
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    raw.append_ops(&[window(10)]).unwrap(); // op 1, made from records 6-10
    raw.exclude("github.com/o/a", false).unwrap(); // op 2
    drop(raw);
    export(p).unwrap(); // ops segment 1-2
    let mut raw = raw::open(p).unwrap();
    raw.exclude("github.com/o/b", false).unwrap(); // op 3, never backed up
    drop(raw);
    let newest = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(1)
        .path;
    std::fs::write(&newest, b"damaged").unwrap();
    crate::worker::restore(p).unwrap();
    let raw = raw::open(p).unwrap();
    use raw::OpKind::Exclusion;
    assert_eq!(op_kinds(&raw, &dev), [(1, Exclusion), (2, Exclusion)]);
    drop(raw);
    let held: String = segments(&p.join("backups"), Kind::Ops)
        .unwrap()
        .iter()
        .map(|s| read_segment(&s.path).unwrap())
        .collect();
    assert!(
        held.contains("github.com/o/b") && !held.contains("\"window\""),
        "{held}"
    );
    damage_raw(p);
    let mut excluded = open_raw(p).unwrap().exclusions().unwrap();
    excluded.sort();
    assert_eq!(excluded, ["github.com/o/a", "github.com/o/b"]);
}

/// cubic on #880: a backup whose record segments were lost, its newest point having kept
/// records, is not restored from its ops alone: that would leave a store with no record.
#[test]
fn ops_alone_are_not_restored_when_records_were_lost() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 5, 5);
    let mut raw = raw::open(p).unwrap();
    raw.exclude("github.com/o/a", false).unwrap();
    drop(raw);
    export(p).unwrap(); // a point at (5, 1)
    for s in segments(&p.join("backups"), Kind::Records).unwrap() {
        std::fs::remove_file(format!("{}.sha256", s.path.display())).unwrap();
        std::fs::remove_file(&s.path).unwrap();
    }
    let why = crate::worker::restore(p).unwrap_err().to_string();
    assert!(why.contains("no usable backup segment"), "{why}");
    assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 5);
}

/// cubic on #880: a backup run whose export fails still indexes what came while it held the
/// lock, then says the export failed.
#[test]
fn a_backup_run_that_fails_to_export_still_indexes() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    std::fs::write(p.join("backups"), b"a file where the folder would be").unwrap();
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event("came during a failed backup zf1"))
        .unwrap();
    drop(raw);
    assert!(crate::worker::backup_run(p).is_err());
    assert_eq!(crate::worker::backlog(p).unwrap(), 0);
}

/// Codex on #880 (B6): a sound segment can span a range raw.db had lost (written after a
/// rewind), so its name overstates what a restore replays; the points past the highest seq
/// replayed go, the one gone back to among them.
#[test]
fn points_go_past_the_highest_seq_replayed_not_past_a_segments_name() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    for i in 0..15 {
        raw.append(&raw::test_event(&format!("rec zg{i:02}x")))
            .unwrap();
    }
    drop(raw);
    rusqlite::Connection::open(p.join("raw.db"))
        .unwrap()
        .execute("DELETE FROM records WHERE seq BETWEEN 6 AND 10", [])
        .unwrap();
    export(p).unwrap(); // one segment named 1-15, holding 1-5 and 11-15
    let dir = p.join("backups");
    let ranges: Vec<(i64, i64)> = segments(&dir, Kind::Records)
        .unwrap()
        .iter()
        .map(|s| (s.first, s.last))
        .collect();
    assert_eq!(ranges, [(1, 15)]);
    let mut ten = points(&dir).unwrap().remove(0);
    ten.records = 10; // a point noted when raw.db still held 6-10
    std::fs::write(
        dir.join(POINTS),
        format!("{}\n", serde_json::to_string(&ten).unwrap()),
    )
    .unwrap();
    // The restore alone, with no export after it: its own pruning is what is checked.
    let config = crate::settings::config_lock(p).unwrap();
    restore_report_holding_to(p, None, None, Some(&ten), &mut |_| {}).unwrap();
    drop(config);
    assert!(points(&dir).unwrap().iter().all(|x| x.records <= 5));
    assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 5);
}

/// Codex on #880 (B6): the points past raw's end go at every export, before the segments past
/// it, so a stop between the two leaves them to the next export.
#[test]
fn points_past_raws_end_go_even_when_its_segments_already_went() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // points (5, 0) and (10, 0)
    rusqlite::Connection::open(p.join("raw.db"))
        .unwrap()
        .execute("DELETE FROM records WHERE seq > 6", [])
        .unwrap();
    // As if an export had set the segment past raw's end aside and stopped there.
    let dir = p.join("backups");
    let past: Vec<Segment> = segments(&dir, Kind::Records)
        .unwrap()
        .into_iter()
        .filter(|s| s.last > 6)
        .collect();
    set_aside_report(&past, &mut |_| Ok(())).unwrap();
    export(p).unwrap();
    let cursors: Vec<i64> = points(&dir).unwrap().iter().map(|x| x.records).collect();
    assert!(cursors.iter().all(|&r| r <= 6), "{cursors:?}");
}

/// docs/backup-points.md test 7: a segment the point cuts is replayed up to it and set aside,
/// and the next export writes its first part again.
#[test]
fn a_segment_the_point_cuts_is_replayed_up_to_it_and_written_again() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 10); // one segment: 1-10
    let dir = p.join("backups");
    let mut mid = points(&dir).unwrap().remove(0);
    mid.records = 4; // as after a rewind wrote the segments again with other bounds
    std::fs::write(
        dir.join(POINTS),
        format!("{}\n", serde_json::to_string(&mid).unwrap()),
    )
    .unwrap();
    crate::worker::restore_to(p, &mid).unwrap();
    assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 4);
    export(p).unwrap();
    let ranges: Vec<(i64, i64)> = segments(&dir, Kind::Records)
        .unwrap()
        .iter()
        .map(|s| (s.first, s.last))
        .collect();
    assert_eq!(ranges, [(1, 4)]);
}

/// docs/backup-points.md test 8 (B5): a restore of a damaged store that lost records keeps
/// the exclusion ops after its first lost record, as it keeps the forget ops.
#[test]
fn a_restore_that_lost_records_keeps_the_exclusions_after_them() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // segments 1-5, 6-10
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    raw.append_ops(&[window(3), claim("a")]).unwrap();
    raw.append_ops(&[window(10), claim("b")]).unwrap();
    raw.exclude("github.com/o/secret", false).unwrap();
    drop(raw);
    export(p).unwrap();
    let newest = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(1)
        .path;
    std::fs::write(&newest, b"damaged").unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.max_seq().unwrap(), 5);
    use raw::OpKind::{Claim, Exclusion, Window};
    assert_eq!(
        op_kinds(&restored, &dev),
        [(1, Window), (2, Claim), (3, Exclusion)]
    );
    assert_eq!(restored.exclusions().unwrap(), ["github.com/o/secret"]);
    let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
    assert!(
        note.contains("2 op(s) past the restored records dropped"),
        "{note}"
    );
}

/// docs/backup-points.md B6: an export that sets aside the segments past raw's end (raw lost
/// commits, MUST-M14) drops the points past it.
#[test]
fn points_past_raws_end_go_with_its_segments() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // points (5, 0) and (10, 0)
    rusqlite::Connection::open(p.join("raw.db"))
        .unwrap()
        .execute("DELETE FROM records WHERE seq > 6", [])
        .unwrap();
    export(p).unwrap();
    let cursors: Vec<(i64, i64)> = points(&p.join("backups"))
        .unwrap()
        .iter()
        .map(|x| (x.records, x.ops))
        .collect();
    assert_eq!(cursors, [(5, 0), (6, 0)]);
}

/// docs/backup-points.md test 9: doctor's newest point is late when it is two days old or more,
/// by the export's own time, with records or ops after it.
#[test]
fn doctor_warns_of_an_old_point_with_records_after_it() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let day = 86_400_000;
    let point = Point {
        at: 10 * day,
        device: "d0".into(),
        records: 5,
        ops: 2,
    };
    note_point(dir, &point).unwrap();
    let (line, late) = point_line(dir, "d0", 7, 2, 12 * day);
    assert!(
        late && line.starts_with("warning:") && line.contains("2 record(s)"),
        "{line}"
    );
    assert!(line.contains("`oboete backup run`"), "{line}");
    assert!(!point_line(dir, "d0", 5, 2, 12 * day).1);
    assert!(!point_line(dir, "d0", 7, 2, 11 * day).1);
    assert!(point_line(dir, "d0", 5, 3, 12 * day).1);
    assert_eq!(
        point_line(dir, "d1", 7, 2, 12 * day),
        (
            "backup point: none yet (the next backup makes one)".to_owned(),
            false
        )
    );
}

/// docs/backup-points.md test 10: a backup run exports what is new and notes a point.
#[test]
fn a_backup_run_exports_and_notes_a_point() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    raw.append(&raw::test_event("run zr1")).unwrap();
    drop(raw);
    let wrote = crate::worker::backup_run(p).unwrap();
    assert_eq!(wrote.len(), 1);
    assert_eq!(points(&p.join("backups")).unwrap().len(), 1);
    assert!(crate::worker::backup_run(p).unwrap().is_empty());
    let listed = list_points(p, false).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0].ends_with("through seq 1 (0 after) and op 0 (0 after)"),
        "{listed:?}"
    );
}

fn window(to_seq: i64) -> (raw::OpKind, serde_json::Value) {
    (
        raw::OpKind::Window,
        serde_json::json!({"from_seq": 1, "to_seq": to_seq, "to_offset": null, "outcome": "curated"}),
    )
}

fn claim(text: &str) -> (raw::OpKind, serde_json::Value) {
    (raw::OpKind::Claim, serde_json::json!({"text": text}))
}

#[test]
fn ops_survive_a_backup_and_restore() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 10);
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    raw.append_ops(&[window(10), claim("keep all timestamps in UTC")])
        .unwrap();
    let before = raw.ops_after(&dev, 0, 100).unwrap();
    drop(raw);
    export(p).unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.ops_after(&dev, 0, 100).unwrap(), before);
    assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (10, None));
    let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
    assert!(note.contains("10 record(s) and 2 op(s)"), "{note}");
}

/// #775: the owner's answer to a judge's suggestion (J4) is an op as any other: a restore
/// brings it back as it was.
#[test]
fn the_owners_answer_to_a_suggestion_survives_a_restore() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 10);
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    let fields = serde_json::json!({"conversation": ["claude", "s2"], "action": "join",
            "groups": [{"conversations": [["claude", "s1"], ["claude", "s2"]], "title": null,
                "links": []}]});
    let answer = crate::my_work_ops::MyWorkOp {
        job: "judge".into(),
        about: "github.com/o/r".into(),
        entry: String::new(),
        ts: 1,
        read: crate::my_work_ops::Read::default(),
        removed: vec![],
        fields: serde_json::from_value(fields).unwrap(),
        skipped: false,
        forgotten: None,
    };
    let answer = serde_json::to_value(answer).unwrap();
    raw.append_ops(&[(raw::OpKind::MyWork, answer)]).unwrap();
    let before = raw.ops_after(&dev, 0, 100).unwrap();
    drop(raw);
    export(p).unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.ops_after(&dev, 0, 100).unwrap(), before);
}

/// docs/work-state.md L4: work state is in the op log, so a restore brings it back as it was.
#[test]
fn work_state_survives_a_backup_and_restore() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 10);
    let mut raw = raw::open(p).unwrap();
    let fields = |v: serde_json::Value| v.as_object().unwrap().clone();
    let state = fields(serde_json::json!({"phase": "rc2"}));
    raw.work_state("github.com/o/r", "release", &state, None)
        .unwrap();
    let task = fields(serde_json::json!({"task": "notes", "status": "doing"}));
    raw.work_state("github.com/o/r", "release", &task, None)
        .unwrap();
    let before = raw.work_state_entries("github.com/o/r").unwrap();
    drop(raw);
    export(p).unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(
        restored.work_state_entries("github.com/o/r").unwrap(),
        before
    );
}

#[test]
fn an_op_appended_after_its_records_were_backed_up_is_exported() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 5, 5);
    let mut raw = raw::open(p).unwrap();
    raw.append_ops(&[window(5)]).unwrap();
    export(p).unwrap();
    // A correction with no new record: the records' cursor is already at the top.
    raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "fix"}))])
        .unwrap();
    let wrote = export(p).unwrap().unwrap();
    assert!(
        wrote
            .to_string_lossy()
            .ends_with("-000000000002-000000000002.ops.zst")
    );
    assert!(export(p).unwrap().is_none()); // nothing new
}

/// D6: a restore that lost an imported batch's records, the newest or one with later
/// segments restored, drops its checkpoint and every later op, so the next pass imports the
/// batch again; restored again, the checkpoint stays dropped.
#[test]
fn a_restore_that_lost_a_batch_drops_its_checkpoint() {
    // The segment lost, its seqs, the highest seq restored and the checkpoint left.
    for (lost, seqs, max, through) in [(3, (11, 12), 10, 5), (2, (9, 10), 12, 3)] {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 5, 5); // records 1-5
        let batch = |n: usize| -> Vec<crate::capture::Captured> {
            (0..n)
                .map(|i| crate::capture::Captured {
                    event: raw::Event {
                        source: "oboete-v1".into(),
                        ..raw::test_event(&format!("v1 zq{i:03}x"))
                    },
                    ledger: Vec::new(),
                })
                .collect()
        };
        let checkpoint = |through| raw::Checkpoint {
            key: "oboete-v1:d1".into(),
            through,
            row: None,
            prefix: None,
        };
        // Records 6-8, 9-10 and 11-12, a segment each.
        for (n, through) in [(3, 3), (2, 5), (2, 7)] {
            let mut raw = raw::open(p).unwrap();
            raw.append_imported(&batch(n), "v", Some(&checkpoint(through)))
                .unwrap();
            drop(raw);
            export(p).unwrap();
        }
        let lost = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(lost);
        assert_eq!((lost.first, lost.last), seqs);
        std::fs::remove_file(&lost.path).unwrap();
        for _ in 0..2 {
            damage_raw(p);
            crate::worker::run_once(p).unwrap();
            let restored = raw::open(p).unwrap();
            assert_eq!(restored.max_seq().unwrap(), max);
            let checkpoints = restored.migration_checkpoints("oboete-v1:").unwrap();
            assert_eq!(checkpoints["oboete-v1:d1"].through, through);
        }
    }
}

#[test]
fn a_window_op_past_the_restored_records_goes_with_every_op_after_it() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // segments 1-5, 6-10
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    raw.append_ops(&[window(3), claim("a")]).unwrap();
    raw.append_ops(&[window(10), claim("b")]).unwrap();
    raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "c"}))])
        .unwrap();
    drop(raw);
    export(p).unwrap();
    // The newest record segment is damaged: seqs 6-10 are not restored.
    let newest = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(1)
        .path;
    std::fs::write(&newest, b"damaged").unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.max_seq().unwrap(), 5);
    let kept: Vec<i64> = restored
        .ops_after(&dev, 0, 100)
        .unwrap()
        .iter()
        .map(|o| o.op_seq)
        .collect();
    assert_eq!(kept, [1, 2]);
    assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
    let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
    assert!(
        note.contains("3 op(s) past the restored records dropped"),
        "{note}"
    );
    // Their op seqs are reused and backed up again, as record seqs are.
    drop(restored);
    let mut raw = raw::open(p).unwrap();
    raw.append_ops(&[window(5)]).unwrap();
    drop(raw);
    export(p).unwrap();
    let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
    assert_eq!(ops.last().map(|s| (s.first, s.last)), Some((3, 3)));
}

/// docs/provenance.md F1: a turn op whose own records, or a My Work op whose read spans, pass
/// the restored records goes with every op after it, as a window op does.
#[test]
fn a_turn_or_my_work_op_past_the_restored_records_goes_with_every_op_after_it() {
    let turn = serde_json::json!({"agent": "claude", "session": "s", "repo": "github.com/o/r",
            "ts": 1, "from": 6, "through": 10, "read": [], "goals": [], "removed": [],
            "fields": {"request": "r", "learned": "l"}, "skipped": false, "excluded": false});
    let my_work = serde_json::json!({"job": "review", "about": "R1", "entry": "anthropic",
            "ts": 1, "read": {"spans": [[6, 10]], "ops": []}, "removed": [],
            "fields": {"summary": "s"}, "skipped": false});
    let mut kept = Vec::new();
    for (kind, body) in [(raw::OpKind::Turn, turn), (raw::OpKind::MyWork, my_work)] {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // segments 1-5, 6-10
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(3), claim("a")]).unwrap();
        raw.append_ops(&[(kind, body)]).unwrap();
        raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "c"}))])
            .unwrap();
        drop(raw);
        export(p).unwrap();
        // The newest record segment is damaged: seqs 6-10 are not restored.
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.max_seq().unwrap(), 5);
        let seqs: Vec<i64> = restored
            .ops_after(&dev, 0, 100)
            .unwrap()
            .iter()
            .map(|o| o.op_seq)
            .collect();
        kept.push((kind, seqs));
    }
    use raw::OpKind::{MyWork, Turn};
    assert_eq!(kept, [(Turn, vec![1, 2]), (MyWork, vec![1, 2])]);
}

/// #690 item 3: a window op is judged by its `to_seq` alone, as the SQL before #687 did, so one
/// with no `from_seq` past the restored records goes with every op after it; and a claim that
/// quotes, or a window whose goals name, a record the restore did not bring back goes the same
/// way (F1).
#[test]
fn an_op_judged_by_its_end_alone_or_its_quotes_goes_with_every_op_after_it() {
    let mut kept = Vec::new();
    for case in ["end alone", "quote", "goal"] {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // segments 1-5, 6-10
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(3), claim("a")]).unwrap();
        let op = match case {
            "end alone" => serde_json::json!({"to_seq": 10, "outcome": "skipped"}),
            "quote" => {
                serde_json::json!({"text": "q", "evidence": [{"device": dev, "seq": 8}]})
            }
            _ => {
                serde_json::json!({"from_seq": 1, "to_seq": 3, "goals": [8], "outcome": "curated"})
            }
        };
        let kind = match case {
            "quote" => raw::OpKind::Claim,
            _ => raw::OpKind::Window,
        };
        raw.append_ops(&[(kind, op)]).unwrap();
        raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "c"}))])
            .unwrap();
        drop(raw);
        export(p).unwrap();
        // The newest record segment is damaged: seqs 6-10 are not restored.
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.max_seq().unwrap(), 5);
        let seqs: Vec<i64> = (restored.ops_after(&dev, 0, 100).unwrap().iter())
            .map(|o| o.op_seq)
            .collect();
        kept.push((case, seqs));
    }
    assert_eq!(
        kept,
        [
            ("end alone", vec![1, 2]),
            ("quote", vec![1, 2]),
            ("goal", vec![1, 2])
        ]
    );
}

/// Milestone 5 D5 (Codex's adversarial review of slice 2a): a forget op past the restored
/// records is appended again, not dropped with the ops it follows, so its uid stays forgotten
/// with no request log to bring it back.
#[test]
fn a_forget_op_past_the_restored_records_is_appended_again() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 5); // segments 1-5, 6-10
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    raw.append_ops(&[window(3), claim("a")]).unwrap();
    raw.append_ops(&[window(10), claim("b")]).unwrap();
    drop(raw);
    // As a registration writes it, with no request log beside it.
    let uid = "f".repeat(64);
    let body = serde_json::json!({"uid": uid, "job": "j"}).to_string();
    rusqlite::Connection::open(p.join("raw.db"))
        .unwrap()
        .execute(
            "INSERT INTO ops(device, op_seq, type, ts, body, batch)
                 VALUES(?1, 5, 'forget', 0, ?2, 5)",
            rusqlite::params![dev, body],
        )
        .unwrap();
    export(p).unwrap();
    let newest = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(1)
        .path;
    std::fs::write(&newest, b"damaged").unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert!(restored.forgotten(&uid).unwrap());
    let kept: Vec<(i64, raw::OpKind)> = restored
        .ops_after(&dev, 0, 100)
        .unwrap()
        .iter()
        .map(|o| (o.op_seq, o.kind))
        .collect();
    use raw::OpKind::{Claim, Forget, Window};
    assert_eq!(kept, [(1, Window), (2, Claim), (3, Forget)]);
    let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
    assert!(
        note.contains("2 op(s) past the restored records dropped"),
        "{note}"
    );
}

/// docs/cards.md K4 after a restore that lost records: a removal made at a seq the lost
/// records had still hides the card of a window cut before the loss, as any removal its op
/// does not list does.
#[test]
fn a_removal_at_a_reused_seq_hides_the_card_of_a_window_cut_before_the_loss() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    for i in 0..10 {
        let event = raw::Event {
            kind: "tool".into(),
            repo: Some("r".into()),
            ..raw::test_event("{}")
        };
        raw.append(&event).unwrap();
        if (i + 1) % 5 == 0 {
            export(p).unwrap(); // segments 1-5, 6-10
        }
    }
    // Cut over records 1 to 3 while the store held ten.
    let op = serde_json::json!({"outcome": "curated", "summary": "Kept.", "from_seq": 1,
            "from_offset": null, "to_seq": 3, "to_offset": null});
    raw.append_ops(&[(raw::OpKind::Window, op)]).unwrap();
    drop(raw);
    export(p).unwrap();
    let newest = segments(&p.join("backups"), Kind::Records)
        .unwrap()
        .remove(1)
        .path;
    std::fs::write(&newest, b"damaged").unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let cards = |raw: &Raw| {
        let k = crate::knowledge::open(p).unwrap();
        let rules = crate::redact::Rules::default();
        crate::cards::recent(&k, raw, "r", 10, &rules)
            .unwrap()
            .len()
    };
    let mut raw = raw::open(p).unwrap();
    assert_eq!((raw.max_seq().unwrap(), cards(&raw)), (5, 1));
    let device = raw.device().to_owned();
    let removed = raw.append_tombstone(raw::Target::Record { device, seq: 2 });
    assert_eq!(removed.unwrap(), 6);
    assert_eq!(cards(&raw), 0);
}

#[test]
fn a_hole_in_the_op_log_sets_aside_every_op_after_it() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 10, 10);
    let mut raw = raw::open(p).unwrap();
    let dev = raw.device().to_owned();
    // One ops segment per export: windows to 3, 6 and 9.
    for to in [3, 6, 9] {
        raw.append_ops(&[window(to), claim("x")]).unwrap();
        export(p).unwrap();
    }
    drop(raw);
    let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
    assert_eq!(ops.len(), 3);
    std::fs::write(&ops[1].path, b"damaged").unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    // The window to 9 verifies, but the one to 6 is lost: curation goes on after 3.
    assert_eq!(restored.max_op_seq().unwrap(), 2);
    assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
    let left = segments(&p.join("backups"), Kind::Ops).unwrap();
    assert_eq!(left.iter().map(|s| s.first).collect::<Vec<_>>(), [1]);
    let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
    assert!(note.contains("after a hole in the op log"), "{note}");
    // A missing segment is a hole too.
    drop(restored);
    let mut raw = raw::open(p).unwrap();
    for to in [6, 9] {
        raw.append_ops(&[window(to)]).unwrap();
        export(p).unwrap();
    }
    drop(raw);
    let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
    std::fs::remove_file(&ops[1].path).unwrap();
    damage_raw(p);
    crate::worker::run_once(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
}

/// Slice 3e-4: an ops segment in a backup location given up still counts, as the purge it gates
/// walks there too. Before, only the location in use was looked in, so a rule's scan was taken as
/// done and the old location's ops kept their text.
#[test]
fn an_ops_segment_in_a_location_given_up_still_counts() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    std::fs::write(p.join("config.toml"), "[backup]\ndir = \"old-backups\"\n").unwrap();
    segmented(p, 5, 5);
    let mut raw = raw::open(p).unwrap();
    raw.append_ops(&[window(5), claim("a")]).unwrap();
    drop(raw);
    export(p).unwrap();
    assert!(
        !segments(&p.join("old-backups"), Kind::Ops)
            .unwrap()
            .is_empty()
    );
    std::fs::write(p.join("config.toml"), "[backup]\ndir = \"new-backups\"\n").unwrap();
    assert!(any_ops_segment(p).unwrap());
}

/// A forget_jobs table whose page is damaged, in a raw.db whose schema still reads, does not stop
/// the restore that is to repair it: the request the logs hold is applied. Before, the error
/// stepping its rows stopped the restore, and with it every worker's start.
#[test]
fn a_damaged_forget_jobs_page_does_not_stop_the_restore() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    let raw = raw::open(p).unwrap();
    let device = raw.device().to_owned();
    let preview = raw
        .forget_preview(crate::forget::Target::Record {
            device: device.clone(),
            seq: 2,
        })
        .unwrap();
    crate::forget::start(p, &preview).unwrap();
    drop(raw); // the last close checkpoints the WAL
    let conn = rusqlite::Connection::open(p.join("raw.db")).unwrap();
    let (root, size): (i64, i64) = (
        conn.query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = 'forget_jobs'",
            [],
            |r| r.get(0),
        )
        .unwrap(),
        conn.query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap(),
    );
    drop(conn);
    let at = ((root - 1) * size) as u64;
    overwrite(&p.join("raw.db"), at, &vec![0xA5; size as usize]);
    restore(p).unwrap();
    let restored = raw::open(p).unwrap();
    assert_eq!(restored.forget_requests().unwrap().len(), 1);
    assert!(matches!(
        restored.after(&device, 1, 1).unwrap()[0].item,
        Item::Removed
    ));
}

/// A raw.db that opens with no schema (an empty file) is rebuilt from the segments: it holds no
/// exclusion to carry. Before, its missing ops table stopped the restore.
#[test]
fn an_empty_raw_db_is_restored_from_its_segments() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    segmented(p, 3, 3);
    std::fs::write(p.join("raw.db"), b"").unwrap();
    for f in ["raw.db-wal", "raw.db-shm"] {
        let _ = std::fs::remove_file(p.join(f));
    }
    let note = restore(p).unwrap();
    assert!(note.contains("3 record(s)"), "{note}");
}

/// docs/backup-points.md B1: a point that another home sharing the backup directory notes while
/// this one rewrites the points is kept. Before, the rewrite's rename put back what it had read,
/// and the point noted in between was gone.
#[test]
fn a_point_noted_while_the_points_are_rewritten_is_kept() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().to_owned();
    let mine = |records| Point {
        at: 1,
        device: "a".into(),
        records,
        ops: 0,
    };
    note_point(&dir, &mine(5)).unwrap();
    note_point(&dir, &mine(10)).unwrap();
    let other = Point {
        at: 2,
        device: "b".into(),
        records: 7,
        ops: 0,
    };
    let (noted, shared) = (other.clone(), dir.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    BETWEEN_POINTS.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            let writer = std::thread::spawn(move || note_point(&shared, &noted).unwrap());
            // Time for the other writer's append, which nothing then holds back.
            std::thread::sleep(std::time::Duration::from_millis(300));
            tx.send(writer).unwrap();
        }));
    });
    keep_points(&dir, "a", |p| p.records <= 5).unwrap();
    BETWEEN_POINTS.with_borrow_mut(|hook| *hook = None);
    rx.recv().unwrap().join().unwrap();
    assert_eq!(points(&dir).unwrap(), [mine(5), other]);
}

/// docs/backup-points.md B6: points with none to drop are left as they are without the lock, so
/// a backup directory that cannot take the lock file still reads, and a restore from it goes on.
#[cfg(unix)]
#[test]
fn points_with_none_to_drop_need_no_lock() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("backups");
    std::fs::create_dir(&dir).unwrap();
    let point = Point {
        at: 1,
        device: "a".into(),
        records: 5,
        ops: 0,
    };
    note_point(&dir, &point).unwrap();
    std::fs::remove_file(dir.join("points.jsonl.lock")).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let kept = keep_points(&dir, "a", |p| p.records <= 5);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    kept.unwrap();
    assert_eq!(points(&dir).unwrap(), [point]);
}
