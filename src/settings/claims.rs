//! Typed owner-claim actions for the guarded local viewer.

use std::path::Path;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Refusal, refused};
use crate::claims::{OwnerReceipt, OwnerRefusal, OwnerRefusalCode, PendingReason};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Correction {
    uid: String,
    status: Option<String>,
    body: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mute {
    uid: String,
    muted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    text: String,
    apply_to_all_repos: bool,
}

fn full_uid(uid: &str) -> Result<(), Refusal> {
    if uid.len() == 64
        && uid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(refused(422, "claim_uid", "uid"))
    }
}

fn answer(result: Result<OwnerReceipt, OwnerRefusal>) -> Result<Value, Refusal> {
    match result {
        Ok(OwnerReceipt::Applied { uid }) => Ok(json!({"state": "applied", "uid": uid})),
        Ok(OwnerReceipt::Pending { uid, reason, .. }) => {
            let code = match reason {
                PendingReason::Application => "claim_pending",
                PendingReason::NotKept => "claim_not_applied",
            };
            Ok(json!({"state": "pending", "uid": uid, "code": code}))
        }
        Ok(OwnerReceipt::DirectiveOnly { .. }) => {
            Ok(json!({"state": "directive_only", "code": "preference_partly_recorded"}))
        }
        Err(e) => Err(match e.code {
            OwnerRefusalCode::ClaimMissing => refused(404, "claim_not_found", "uid"),
            OwnerRefusalCode::CorrectionInvalid => refused(422, "claim_invalid", ""),
            OwnerRefusalCode::Unavailable => refused(503, "claim_unavailable", ""),
            OwnerRefusalCode::PreferenceEmpty => refused(422, "preference_empty", "text"),
            OwnerRefusalCode::PreferenceTooLong => refused(422, "preference_too_long", "text"),
        }),
    }
}

pub(crate) fn correct(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Correction =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    full_uid(&posted.uid)?;
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    answer(crate::claims::correct_recorded(
        home,
        &posted.uid,
        posted.status.as_deref(),
        posted.body.as_deref(),
    ))
}

pub(crate) fn mute(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Mute = serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    full_uid(&posted.uid)?;
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    answer(crate::claims::mute_recorded(
        home,
        &posted.uid,
        posted.muted,
    ))
}

pub(crate) fn pref_add(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Preference =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if !posted.apply_to_all_repos {
        return Err(refused(
            422,
            "preference_confirmation",
            "apply_to_all_repos",
        ));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    answer(crate::claims::pref_add_recorded(home, &posted.text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::b::fixture::Store;
    use serde_json::json;

    fn owner_claim() -> (Store, String) {
        let mut store = Store::new();
        let text = "Use tabs.";
        let seq = store.said("s", "repo", 1_000, text);
        let uid = store.claim(seq, text, ("preference", "decided", "user"), &[]);
        store.run();
        (store, uid)
    }

    fn durable_store_files(home: &Path) -> std::collections::BTreeMap<std::ffi::OsString, String> {
        use sha2::{Digest, Sha256};
        std::fs::read_dir(home)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_type().unwrap().is_file())
            .filter_map(|entry| {
                let bytes = std::fs::read(entry.path()).unwrap();
                let name = entry.file_name();
                let filename = name.to_string_lossy();
                // SQLite readers may create empty WALs and update the volatile shared index.
                if filename.ends_with("-shm") || (filename.ends_with("-wal") && bytes.is_empty()) {
                    return None;
                }
                Some((name, format!("{:x}", Sha256::digest(bytes))))
            })
            .collect()
    }

    #[test]
    fn w3_correct_applies_the_gated_owner_body_and_status() {
        let mut store = Store::new();
        let text = "Use tabs.";
        let seq = store.said("s", "repo", 1_000, text);
        let uid = store.claim(seq, text, ("preference", "decided", "user"), &[]);
        store.run();
        std::fs::write(
            store.home.path().join("config.toml"),
            "[summary]\ncurate = true\n",
        )
        .unwrap();
        let answer = correct(
            store.home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({
                "uid": uid,
                "status": "done",
                "body": "Use spaces.<private>customer-private-value</private>"
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(answer, json!({"state": "applied", "uid": uid}));
        let claim = crate::search::b::claim(store.home.path(), &uid)
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, "done");
        assert_eq!(claim.text, "Use spaces.");
        assert_eq!(claim.scope, "repo");
        assert!(!store.home.path().join("providers.db").exists());
    }

    #[test]
    fn w3_mute_and_unmute_preserve_search_and_change_injection_eligibility() {
        let (store, uid) = owner_claim();
        let saving = Mutex::new(());
        for muted in [true, false] {
            let answer = mute(
                store.home.path(),
                &saving,
                &serde_json::to_vec(&json!({"uid": uid, "muted": muted})).unwrap(),
            )
            .unwrap();
            assert_eq!(answer, json!({"state": "applied", "uid": uid}));
            let claim = crate::search::b::claim(store.home.path(), &uid)
                .unwrap()
                .unwrap();
            assert_eq!(claim.muted, muted);
            assert_eq!(claim.text, "Use tabs.");
            assert_eq!(claim.status, "decided");
            let k = crate::knowledge::open(store.home.path()).unwrap();
            let eligible = crate::claims::decisions(&k, "repo", 20).unwrap();
            assert_eq!(eligible.iter().any(|claim| claim.uid == uid), !muted);
        }
        assert!(!store.home.path().join("providers.db").exists());
    }

    #[test]
    fn w3_explicit_global_preference_is_applied_without_inference() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[summary]\ncurate = true\n",
        )
        .unwrap();
        let answer = pref_add(
            home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({
                "text": "Always answer in Japanese.<private>customer-private-value</private>",
                "apply_to_all_repos": true
            }))
            .unwrap(),
        )
        .unwrap();
        let uid = answer["uid"].as_str().unwrap();
        assert_eq!(answer["state"], "applied");
        assert_eq!(uid.len(), 64);
        let claim = crate::search::b::claim(home.path(), uid).unwrap().unwrap();
        assert_eq!(claim.uid, uid);
        assert_eq!(claim.text, "Always answer in Japanese.");
        assert_eq!(claim.scope, "global");
        assert_eq!(claim.status, "decided");
        assert_eq!(claim.kind, "preference");
        assert_eq!(claim.speaker, "user");
        assert_eq!(claim.repo, None);
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn w3_a_checkpoint_without_the_preference_is_still_pending() {
        let home = tempfile::tempdir().unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        crate::claims::schema(&k).unwrap();
        // An older or faulty consumer can pass an op without keeping its derivation. Exercise
        // that boundary with the real consumer and an isolated database storage fault.
        k.execute_batch(
            "CREATE TRIGGER lose_preference BEFORE INSERT ON derivations
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        let answer = pref_add(
            home.path(),
            &Mutex::new(()),
            br#"{"text":"Always answer in Japanese.","apply_to_all_repos":true}"#,
        )
        .unwrap();
        assert_eq!(
            crate::knowledge::checkpoint::get_in(
                &k,
                crate::knowledge::checkpoint::OPS,
                "claims",
                raw.device()
            )
            .unwrap(),
            raw.max_op_seq().unwrap()
        );
        let uid = answer["uid"].as_str().unwrap();
        assert!(crate::search::b::claim(home.path(), uid).unwrap().is_none());
        assert_eq!(answer["state"], "pending");
        assert_eq!(answer["code"], "claim_not_applied");
    }

    #[test]
    fn w3_a_preference_partial_append_is_reported_once_without_a_claim_uid() {
        let home = tempfile::tempdir().unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let db = rusqlite::Connection::open(crate::raw::path(home.path())).unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_claim BEFORE INSERT ON ops
             BEGIN SELECT RAISE(FAIL, 'synthetic private storage failure'); END;",
        )
        .unwrap();
        let answer = pref_add(
            home.path(),
            &Mutex::new(()),
            br#"{"text":"Always answer in Japanese.","apply_to_all_repos":true}"#,
        )
        .unwrap();
        assert_eq!(
            answer,
            json!({"state": "directive_only", "code": "preference_partly_recorded"})
        );
        assert_eq!(raw.max_seq().unwrap(), 1);
        assert_eq!(raw.max_op_seq().unwrap(), 0);
        assert_eq!(raw.export_lines(0, 1 << 20).unwrap().len(), 1);
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn w3_recorded_actions_wait_safely_for_worker_recovery() {
        let (store, uid) = owner_claim();
        let lock = store.home.path().join("state/worker.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir(&lock).unwrap();
        let before = (
            store.raw.max_seq().unwrap(),
            store.raw.max_op_seq().unwrap(),
        );
        let saving = Mutex::new(());
        let correction = correct(
            store.home.path(),
            &saving,
            &serde_json::to_vec(&json!({"uid": uid, "body": "Use spaces."})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            correction,
            json!({"state": "pending", "uid": uid, "code": "claim_pending"})
        );
        let preference = pref_add(
            store.home.path(),
            &saving,
            br#"{"text":"Always answer in Japanese.","apply_to_all_repos":true}"#,
        )
        .unwrap();
        assert_eq!(preference["state"], "pending");
        assert_eq!(preference["code"], "claim_pending");
        let preference_uid = preference["uid"].as_str().unwrap();
        assert_eq!(store.raw.max_seq().unwrap(), before.0 + 1);
        assert_eq!(store.raw.max_op_seq().unwrap(), before.1 + 2);
        std::fs::remove_dir(lock).unwrap();
        store.run();
        let corrected = crate::search::b::claim(store.home.path(), &uid)
            .unwrap()
            .unwrap();
        assert_eq!(corrected.text, "Use spaces.");
        let preferred = crate::search::b::claim(store.home.path(), preference_uid)
            .unwrap()
            .unwrap();
        assert_eq!(preferred.text, "Always answer in Japanese.");
        assert_eq!(preferred.scope, "global");
        assert_eq!(store.raw.max_seq().unwrap(), before.0 + 1);
        assert_eq!(store.raw.max_op_seq().unwrap(), before.1 + 2);
        assert!(!store.home.path().join("providers.db").exists());
    }

    #[test]
    fn w3_a_checkpoint_without_the_correction_is_still_pending() {
        let (store, uid) = owner_claim();
        let k = crate::knowledge::open(store.home.path()).unwrap();
        k.execute_batch(
            "CREATE TRIGGER lose_correction BEFORE INSERT ON corrections
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        let answer = correct(
            store.home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({"uid": uid, "body": "Use spaces."})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            answer,
            json!({"state": "pending", "uid": uid, "code": "claim_not_applied"})
        );
        let claim = crate::search::b::claim(store.home.path(), &uid)
            .unwrap()
            .unwrap();
        assert_eq!(claim.text, "Use tabs.");
        assert_eq!(
            crate::knowledge::checkpoint::get_in(
                &k,
                crate::knowledge::checkpoint::OPS,
                "claims",
                store.raw.device()
            )
            .unwrap(),
            store.raw.max_op_seq().unwrap()
        );
    }

    #[test]
    fn w3_unknown_claim_refusals_leave_fresh_and_empty_homes_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let saving = Mutex::new(());
        let uid = "a".repeat(64);
        for empty in [true, false] {
            let home = dir.path().join(if empty { "empty" } else { "absent" });
            if empty {
                std::fs::create_dir(&home).unwrap();
            }
            let expected = refused(404, "claim_not_found", "uid");
            assert_eq!(
                correct(
                    &home,
                    &saving,
                    &serde_json::to_vec(&json!({"uid": uid, "body": "Use spaces."})).unwrap()
                )
                .unwrap_err(),
                expected
            );
            assert_eq!(
                mute(
                    &home,
                    &saving,
                    &serde_json::to_vec(&json!({"uid": uid, "muted": true})).unwrap()
                )
                .unwrap_err(),
                expected
            );
            assert_eq!(
                crate::claims::correct(&home, &uid, Some("decided"), None)
                    .unwrap_err()
                    .to_string(),
                format!("no claim has the uid {uid}")
            );
            assert_eq!(
                crate::claims::mute(&home, &uid, false)
                    .unwrap_err()
                    .to_string(),
                format!("no claim has the uid {uid}")
            );
            if empty {
                assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
            } else {
                assert!(
                    !home.exists(),
                    "an unknown claim must not initialize its home"
                );
            }
        }
    }

    #[test]
    fn w3_incomplete_claim_schemas_are_refused_without_initialization() {
        let uid = "a".repeat(64);
        for missing in ["base_only", "active", "derivations", "corrections"] {
            let home = tempfile::tempdir().unwrap();
            drop(crate::raw::open(home.path()).unwrap());
            let k = crate::knowledge::open(home.path()).unwrap();
            if missing != "base_only" {
                crate::claims::schema(&k).unwrap();
                let object = if missing == "active" { "VIEW" } else { "TABLE" };
                k.execute_batch(&format!("DROP {object} {missing}"))
                    .unwrap();
            }
            drop(k);
            let before = durable_store_files(home.path());
            for is_mute in [false, true] {
                let action = if is_mute { mute } else { correct };
                let body = if is_mute {
                    json!({"uid": uid, "muted": true})
                } else {
                    json!({"uid": uid, "body": "Use spaces."})
                };
                let refusal = action(
                    home.path(),
                    &Mutex::new(()),
                    &serde_json::to_vec(&body).unwrap(),
                )
                .unwrap_err();
                assert_eq!(refusal, refused(503, "claim_unavailable", ""), "{missing}");
                assert_eq!(durable_store_files(home.path()), before, "{missing}");
            }
        }
    }

    #[test]
    fn w3_incomplete_live_wal_claim_state_is_refused_without_changing_durable_data() {
        let (store, uid) = owner_claim();
        let k = crate::knowledge::open(store.home.path()).unwrap();
        k.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); DROP VIEW active;")
            .unwrap();
        assert!(
            std::fs::metadata(store.home.path().join("knowledge.db-wal"))
                .unwrap()
                .len()
                > 0
        );
        let before = durable_store_files(store.home.path());
        for is_mute in [false, true] {
            let action = if is_mute { mute } else { correct };
            let body = if is_mute {
                json!({"uid": uid, "muted": true})
            } else {
                json!({"uid": uid, "body": "Use spaces."})
            };
            assert_eq!(
                action(
                    store.home.path(),
                    &Mutex::new(()),
                    &serde_json::to_vec(&body).unwrap()
                )
                .unwrap_err(),
                refused(503, "claim_unavailable", "")
            );
            assert_eq!(durable_store_files(store.home.path()), before);
        }
    }

    #[test]
    fn w3_zero_length_claim_stores_keep_their_durable_files() {
        for empty in ["both", "raw.db", "knowledge.db"] {
            let (store, uid) = owner_claim();
            let Store { home, raw } = store;
            drop(raw);
            for file in ["raw.db", "knowledge.db"] {
                if empty == "both" || empty == file {
                    std::fs::write(home.path().join(file), []).unwrap();
                }
            }
            let before = durable_store_files(home.path());
            for is_mute in [false, true] {
                let action = if is_mute { mute } else { correct };
                let body = if is_mute {
                    json!({"uid": uid, "muted": true})
                } else {
                    json!({"uid": uid, "body": "Use spaces."})
                };
                let refusal = action(
                    home.path(),
                    &Mutex::new(()),
                    &serde_json::to_vec(&body).unwrap(),
                )
                .unwrap_err();
                assert_eq!(durable_store_files(home.path()), before, "{empty}");
                assert_eq!(refusal, refused(503, "claim_unavailable", ""), "{empty}");
            }
        }
    }

    #[test]
    fn w3_existing_older_claim_state_migrates_only_for_a_valid_target() {
        let (store, uid) = owner_claim();
        let k = crate::knowledge::open(store.home.path()).unwrap();
        k.execute_batch(
            "DROP VIEW active;
             ALTER TABLE corrections DROP COLUMN muted;
             ALTER TABLE evidence DROP COLUMN claim_at;
             CREATE VIEW active AS
               SELECT c.uid, d.kind, d.speaker, d.scope, d.repo, d.valid_from,
                 d.anchor_device, d.anchor_seq,
                 COALESCE((SELECT x.status FROM corrections x WHERE x.uid = c.uid
                           AND x.status IS NOT NULL
                           ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                          d.status) AS status,
                 COALESCE((SELECT x.body FROM corrections x WHERE x.uid = c.uid
                           AND x.body IS NOT NULL
                           ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                          d.body) AS body
               FROM claims c JOIN derivations d
                 ON d.op_device = c.op_device AND d.op_seq = c.op_seq;",
        )
        .unwrap();
        let before = durable_store_files(store.home.path());
        let saving = Mutex::new(());
        assert_eq!(
            mute(
                store.home.path(),
                &saving,
                &serde_json::to_vec(&json!({"uid": "0".repeat(64), "muted": true})).unwrap()
            )
            .unwrap_err(),
            refused(404, "claim_not_found", "uid")
        );
        assert_eq!(durable_store_files(store.home.path()), before);
        assert_eq!(
            correct(
                store.home.path(),
                &saving,
                &serde_json::to_vec(&json!({"uid": uid, "body": "Use spaces."})).unwrap()
            )
            .unwrap(),
            json!({"state": "applied", "uid": uid})
        );
        assert_eq!(
            mute(
                store.home.path(),
                &saving,
                &serde_json::to_vec(&json!({"uid": uid, "muted": true})).unwrap()
            )
            .unwrap(),
            json!({"state": "applied", "uid": uid})
        );
        let claim = crate::search::b::claim(store.home.path(), &uid)
            .unwrap()
            .unwrap();
        assert_eq!(claim.text, "Use spaces.");
        assert!(claim.muted);
        assert!(!store.home.path().join("providers.db").exists());
    }

    #[test]
    fn w3_incomplete_or_corrupt_claim_stores_stay_unavailable() {
        let saving = Mutex::new(());
        let uid = "a".repeat(64);
        for state in [
            "raw_only",
            "knowledge_only",
            "corrupt_knowledge",
            "corrupt_raw",
        ] {
            let home = tempfile::tempdir().unwrap();
            if state != "knowledge_only" {
                drop(crate::raw::open(home.path()).unwrap());
            }
            if state != "raw_only" {
                drop(crate::knowledge::open(home.path()).unwrap());
            }
            let corrupt = match state {
                "corrupt_knowledge" => Some(home.path().join("knowledge.db")),
                "corrupt_raw" => Some(home.path().join("raw.db")),
                _ => None,
            };
            if let Some(path) = &corrupt {
                std::fs::write(path, "synthetic damaged store").unwrap();
            }
            assert_eq!(
                correct(
                    home.path(),
                    &saving,
                    &serde_json::to_vec(&json!({"uid": uid, "body": "Use spaces."})).unwrap()
                )
                .unwrap_err(),
                refused(503, "claim_unavailable", ""),
                "{state}"
            );
            assert_eq!(
                mute(
                    home.path(),
                    &saving,
                    &serde_json::to_vec(&json!({"uid": uid, "muted": true})).unwrap()
                )
                .unwrap_err(),
                refused(503, "claim_unavailable", ""),
                "{state}"
            );
            if state == "raw_only" {
                assert!(!home.path().join("knowledge.db").exists());
            } else if state == "knowledge_only" {
                assert!(!home.path().join("raw.db").exists());
                assert!(!home.path().join("raw.lock").exists());
            }
            if let Some(path) = corrupt {
                assert_eq!(std::fs::read(path).unwrap(), b"synthetic damaged store");
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn w3_an_unresolvable_claim_store_is_unavailable_not_missing() {
        let home = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("raw.db", home.path().join("raw.db")).unwrap();
        let refusal = mute(
            home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({"uid": "a".repeat(64), "muted": true})).unwrap(),
        )
        .unwrap_err();
        assert_eq!(refusal, refused(503, "claim_unavailable", ""));
        assert!(!home.path().join("knowledge.db").exists());
        assert!(!home.path().join("raw.lock").exists());
    }

    #[test]
    fn w3_restoring_claim_stores_are_not_recreated_by_a_refusal() {
        let (store, uid) = owner_claim();
        let Store { home, raw } = store;
        drop(raw);
        let held = crate::raw::lock_for_swap(home.path()).unwrap();
        for file in ["raw.db", "knowledge.db"] {
            std::fs::rename(
                home.path().join(file),
                home.path().join(format!("{file}.held")),
            )
            .unwrap();
        }
        let refusal = mute(
            home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({"uid": uid, "muted": true})).unwrap(),
        )
        .unwrap_err();
        assert_eq!(refusal, refused(503, "claim_unavailable", ""));
        assert!(!home.path().join("raw.db").exists());
        assert!(!home.path().join("knowledge.db").exists());
        drop(held);
    }

    #[test]
    fn w3_claim_actions_resume_the_same_store_after_a_stopped_restore() {
        let (store, uid) = owner_claim();
        let Store { home, raw } = store;
        drop(raw);
        let saving = Mutex::new(());
        std::fs::rename(
            home.path().join("raw.db"),
            home.path().join("raw.db.restored"),
        )
        .unwrap();
        let corrected = correct(
            home.path(),
            &saving,
            &serde_json::to_vec(&json!({"uid": uid, "body": "Use spaces."})).unwrap(),
        )
        .unwrap();
        assert_eq!(corrected, json!({"state": "applied", "uid": uid}));
        std::fs::rename(
            home.path().join("raw.db"),
            home.path().join("raw.db.restored"),
        )
        .unwrap();
        let muted = mute(
            home.path(),
            &saving,
            &serde_json::to_vec(&json!({"uid": uid, "muted": true})).unwrap(),
        )
        .unwrap();
        assert_eq!(muted, json!({"state": "applied", "uid": uid}));
        assert!(home.path().join("raw.db").exists());
        assert!(!home.path().join("raw.db.restored").exists());
        let claim = crate::search::b::claim(home.path(), &uid).unwrap().unwrap();
        assert_eq!(claim.text, "Use spaces.");
        assert!(claim.muted);
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn w3_invalid_claim_protocol_and_unconfirmed_global_scope_touch_no_store() {
        type Action = fn(&Path, &Mutex<()>, &[u8]) -> Result<Value, Refusal>;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("unused");
        let saving = Mutex::new(());
        let cases: Vec<(Action, Value, &str)> = vec![
            (correct, json!({"uid": "abc", "body": "text"}), "claim_uid"),
            (
                mute,
                json!({"uid": "A".repeat(64), "muted": true}),
                "claim_uid",
            ),
            (
                correct,
                json!({"uid": "a".repeat(64), "body": "text", "scope": "global"}),
                "bad_request",
            ),
            (
                mute,
                json!({"uid": "a".repeat(64), "muted": "yes"}),
                "bad_request",
            ),
            (
                pref_add,
                json!({"text": "text", "apply_to_all_repos": false}),
                "preference_confirmation",
            ),
            (pref_add, json!({"text": "text"}), "bad_request"),
            (
                pref_add,
                json!({"text": "text", "apply_to_all_repos": true, "scope": "repo"}),
                "bad_request",
            ),
            (
                pref_add,
                json!({"text": "<private>private value</private>", "apply_to_all_repos": true}),
                "preference_empty",
            ),
            (
                pref_add,
                json!({"text": "a".repeat(1_001), "apply_to_all_repos": true}),
                "preference_too_long",
            ),
        ];
        for (action, body, code) in cases {
            let refusal = action(&home, &saving, &serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!(refusal.code, code);
            assert!(!home.exists());
        }
    }

    #[test]
    fn w3_invalid_corrections_append_nothing_and_never_echo_the_input() {
        let (store, uid) = owner_claim();
        let before = (
            store.raw.max_seq().unwrap(),
            store.raw.max_op_seq().unwrap(),
        );
        for body in [
            json!({"uid": uid}),
            json!({"uid": uid, "status": "secret-invalid-status"}),
            json!({"uid": uid, "body": "<private>secret-private-body</private>"}),
            json!({"uid": uid, "body": "a".repeat(1_001)}),
        ] {
            let refusal = correct(
                store.home.path(),
                &Mutex::new(()),
                &serde_json::to_vec(&body).unwrap(),
            )
            .unwrap_err();
            assert_eq!(refusal, refused(422, "claim_invalid", ""));
        }
        let missing = mute(
            store.home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({"uid": "0".repeat(64), "muted": true})).unwrap(),
        )
        .unwrap_err();
        assert_eq!(missing, refused(404, "claim_not_found", "uid"));
        assert_eq!(
            (
                store.raw.max_seq().unwrap(),
                store.raw.max_op_seq().unwrap()
            ),
            before
        );
    }

    #[test]
    fn w3_owner_claim_actions_survive_rebuild() {
        let (store, uid) = owner_claim();
        let saving = Mutex::new(());
        correct(
            store.home.path(),
            &saving,
            &serde_json::to_vec(&json!({"uid": uid, "body": "Use spaces.", "status": "done"}))
                .unwrap(),
        )
        .unwrap();
        mute(
            store.home.path(),
            &saving,
            &serde_json::to_vec(&json!({"uid": uid, "muted": true})).unwrap(),
        )
        .unwrap();
        let preference = pref_add(
            store.home.path(),
            &saving,
            br#"{"text":"Always answer in Japanese.","apply_to_all_repos":true}"#,
        )
        .unwrap();
        let preference_uid = preference["uid"].as_str().unwrap();
        let Store { home, raw } = store;
        drop(raw);
        crate::worker::rebuild(home.path()).unwrap();
        let claim = crate::search::b::claim(home.path(), &uid).unwrap().unwrap();
        assert_eq!(claim.text, "Use spaces.");
        assert_eq!(claim.status, "done");
        assert!(claim.muted);
        let preferred = crate::search::b::claim(home.path(), preference_uid)
            .unwrap()
            .unwrap();
        assert_eq!(preferred.scope, "global");
        assert_eq!(preferred.text, "Always answer in Japanese.");
        assert!(!home.path().join("providers.db").exists());
    }
}
