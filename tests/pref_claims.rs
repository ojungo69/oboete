//! `oboete claims` lists the global preferences with the repository's claims: `oboete correct`
//! takes their full ids, and no repository's list holds them.

#[test]
fn claims_lists_a_preference_added_for_every_repository() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let oboete = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
            .arg("--home")
            .arg(home.path())
            .args(args)
            .current_dir(cwd.path())
            .env("OBOETE_NO_SPAWN", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let added = oboete(&["pref", "add", "返事は日本語で書く"]);
    let id = added
        .trim()
        .strip_prefix("kept for every repository (")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or_else(|| panic!("{added}"));
    let listed = oboete(&["claims"]);
    assert!(
        listed
            .lines()
            .any(|l| l.starts_with(id) && l.contains("返事は日本語で書く")),
        "{listed}"
    );
}
