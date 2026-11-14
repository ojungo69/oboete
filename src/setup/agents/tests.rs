use super::*;

#[test]
#[cfg(unix)]
fn w6a_expected_alias_unlink_keeps_referent_and_fresh_resource_state() {
    let root = tempfile::tempdir().unwrap();
    let referent = root.path().join("owned-plugin");
    let alias = root.path().join("plugin");
    let intermediate = root.path().join("intermediate");
    std::fs::write(&referent, "owned plugin bytes").unwrap();
    std::os::unix::fs::symlink(&referent, &intermediate).unwrap();
    std::os::unix::fs::symlink(&intermediate, &alias).unwrap();
    let original = files::input(&alias).unwrap();
    let mut resources = Resources::default();
    resources.observe(&alias);
    std::fs::remove_file(&alias).unwrap();
    resources.refresh(&alias, true).unwrap();
    assert!(resources.unchanged());
    assert!(!alias.exists());
    assert_eq!(
        std::fs::read_to_string(referent).unwrap(),
        "owned plugin bytes"
    );
    std::fs::remove_file(&intermediate).unwrap();
    std::os::unix::fs::symlink(root.path().join("unexpected"), &intermediate).unwrap();
    assert!(
        !original.after_unlink(&files::input(&alias).unwrap()),
        "expected leaf deletion must not admit another alias changing"
    );
}

#[test]
#[cfg(unix)]
fn w6a_native_readback_keeps_the_original_physical_route() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let other = root.path().join("other");
    let alias = root.path().join("alias");
    std::fs::write(&first, "{}").unwrap();
    std::fs::write(&other, "{}").unwrap();
    std::os::unix::fs::symlink(&first, &alias).unwrap();
    let mut resources = Resources::default();
    resources.observe(&alias);
    // An agent may replace the contents/inode at the agreed target.
    let staged = root.path().join("new");
    std::fs::write(&staged, "{\"native\":true}").unwrap();
    std::fs::rename(staged, &first).unwrap();
    resources.refresh(&alias, false).unwrap();
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&other, &alias).unwrap();
    assert!(
        resources.refresh(&alias, false).is_err(),
        "readback accepted a different physical target"
    );
    assert_eq!(
        resources.get(&alias).unwrap().text.as_deref(),
        Some("{\"native\":true}")
    );
    assert!(!resources.unchanged());
    assert_eq!(std::fs::read_to_string(other).unwrap(), "{}");
}

#[test]
fn w6a_same_id_replays_an_established_result_while_saving_is_locked() {
    let agents = Agents::default();
    let saving = Mutex::new(());
    let _guard = saving.lock().unwrap();
    let operation_id = "a".repeat(64);
    let key = "b".repeat(64);
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({
                "action":"wire","agents":["codex"],"preview_key":key
            }))
            .unwrap()
        )
    );
    assert!(
        agents
            .replay(&operation_id, &fingerprint)
            .unwrap()
            .is_none()
    );
    let receipt = Receipt {
        operation_id: operation_id.clone(),
        action: Action::Wire,
        phase: Phase::Complete,
        agents: vec![],
        live_verified: false,
        activation: "next_session",
    };
    // A has completed since B's first lookup. B must use the receipt before
    // collecting resources again, even if the supplied caller/home is unusable.
    agents.state.lock().unwrap().last = Some(Run {
        fingerprint,
        receipt: receipt.clone(),
    });
    let body = serde_json::to_vec(&json!({"action":"wire","agents":["codex"],
            "preview_key":key,"operation_id":operation_id,"confirmed":true}))
    .unwrap();
    assert_eq!(
        agents
            .start(None, Path::new("unused"), &saving, &body)
            .unwrap(),
        json!(receipt)
    );
}
