//! `oboete capture exclude` (milestone 5 slice 4a, spec 6.1): a repository or a folder is never
//! recorded from then on and the list is printed; what was recorded before stays, and the command
//! says how to forget it; `oboete exclude --undo` sends no repository that is never recorded;
//! doctor says when the current directory is not recorded; an undo records again.

fn run(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> String {
    run_both(home, cwd, args).0
}

/// What the command printed on stdout, and on stderr.
fn run_both(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> (String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(args)
        .current_dir(cwd)
        .env("OBOETE_NO_SPAWN", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (text(&out.stdout), text(&out.stderr))
}

#[test]
fn capture_exclude_lists_stops_recording_and_takes_it_back() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join(".git")).unwrap();
    std::fs::write(
        cwd.path().join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:o/secret.git\n",
    )
    .unwrap();
    std::fs::create_dir(cwd.path().join("private")).unwrap();
    let oboete = |args: &[&str]| run(home.path(), cwd.path(), args);
    let out = oboete(&["capture", "exclude"]);
    assert!(
        out.contains("never recorded from now on: github.com/o/secret")
            && out.contains("`oboete forget --repo github.com/o/secret` removes it"),
        "{out}"
    );
    let out = oboete(&["capture", "exclude", "--folder", "sub/../private"]);
    assert!(
        out.contains("never recorded: github.com/o/secret, ")
            && out.contains("private (folder)")
            && !out.contains(".."),
        "{out}"
    );
    let out = oboete(&["exclude", "--undo", "github.com/o/secret"]);
    assert!(out.contains("github.com/o/secret stays excluded"), "{out}");
    let out = oboete(&["doctor"]);
    assert!(
        out.contains("nothing in this directory is recorded: its repository github.com/o/secret"),
        "{out}"
    );
    let out = oboete(&["capture", "exclude", "--undo"]);
    assert!(
        out.contains("recorded again from now on: github.com/o/secret"),
        "{out}"
    );
    assert!(!oboete(&["doctor"]).contains("nothing in this directory is recorded"));
}

/// Codex on #489: an undo after a rule that masks part of a folder's path was added takes the
/// folder back out of the list as it was stored.
#[test]
fn an_undo_after_a_new_rule_takes_the_stored_folder_back() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let folder = cwd.path().join("acme-482915").join("private");
    std::fs::create_dir_all(&folder).unwrap();
    let folder = folder.to_string_lossy();
    let oboete = |args: &[&str]| run(home.path(), cwd.path(), args);
    let out = oboete(&["capture", "exclude", "--folder", &folder]);
    assert!(out.contains("never recorded from now on"), "{out}");
    std::fs::write(
        home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    let out = oboete(&["capture", "exclude", "--folder", &folder, "--undo"]);
    assert!(
        out.contains("recorded again from now on")
            && out.contains("everything is recorded: nothing is excluded"),
        "{out}"
    );
}

/// Codex on #489: two folders a rule added since reads alike: an undo by the name one was
/// excluded under takes that one back, and one by a name of neither is refused, nothing changed.
/// The rule still reads the one taken back as the other, so nothing there is recorded, and the
/// undo says so (CodeRabbit on #489).
#[test]
fn an_undo_takes_back_the_folder_named_when_two_read_alike_now() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (one, two, other) = (
        cwd.path().join("acme-111111"),
        cwd.path().join("acme-222222"),
        cwd.path().join("acme-333333"),
    );
    let oboete = |args: &[&str]| run_both(home.path(), cwd.path(), args);
    for folder in [&one, &two] {
        oboete(&["capture", "exclude", "--folder", &folder.to_string_lossy()]);
    }
    std::fs::write(
        home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    let (out, err) = oboete(&[
        "capture",
        "exclude",
        "--folder",
        &other.to_string_lossy(),
        "--undo",
    ]);
    assert!(
        err.contains("is how 2 excluded entries read now"),
        "{out}{err}"
    );
    let (out, _) = oboete(&[
        "capture",
        "exclude",
        "--folder",
        &two.to_string_lossy(),
        "--undo",
    ]);
    let one = one.to_string_lossy();
    assert!(
        out.contains(&format!(
            "{} is no longer excluded, but nothing there is recorded",
            two.to_string_lossy()
        )) && out.contains(&format!("never recorded: {one} (folder)")),
        "{out}"
    );
}

/// Codex on #489, owner 2026-10-13: under a rule that masks part of a repository's name, the
/// repository is stored and listed as given, and the forget hint names it as its earlier records
/// do, which `oboete forget --repo` selects.
#[test]
fn a_repository_is_stored_as_given_and_the_forget_hint_names_it_so() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
    )
    .unwrap();
    let out = run(
        home.path(),
        cwd.path(),
        &["capture", "exclude", "github.com/o/acme-482915"],
    );
    // Stored as given (owner, 2026-10-13): the list names it so, and the hint as its records do.
    assert!(
        out.contains("`oboete forget --repo github.com/o/acme-482915` removes it")
            && out.contains("never recorded from now on: github.com/o/acme-482915")
            && out.contains("never recorded: github.com/o/acme-482915"),
        "{out}"
    );
}

/// Codex on #489: a folder excluded again in another spelling of letter case, where the platform's
/// file systems ignore it, keeps the one entry, so the undo leaves none; elsewhere they are two
/// folders.
#[test]
fn a_folder_excluded_in_another_spelling_keeps_one_entry_where_case_is_ignored() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let oboete = |args: &[&str]| run(home.path(), cwd.path(), args);
    let folder = cwd.path().join("Private").to_string_lossy().into_owned();
    let other = cwd.path().join("private").to_string_lossy().into_owned();
    oboete(&["capture", "exclude", "--folder", &folder]);
    let out = oboete(&["capture", "exclude", "--folder", &other]);
    let listed = |f: &str| out.contains(&format!("{f} (folder)"));
    if !cfg!(any(windows, target_os = "macos")) {
        assert!(listed(&folder) && listed(&other), "{out}");
        return;
    }
    assert!(
        out.contains(&format!("never recorded from now on: {folder}")) && !listed(&other),
        "{out}"
    );
    let out = oboete(&["capture", "exclude", "--folder", &other, "--undo"]);
    assert!(
        out.contains(&format!("recorded again from now on: {folder}"))
            && out.contains("everything is recorded"),
        "{out}"
    );
}

/// Codex on #491: a folder taken off the list while a folder around it is listed is not said to be
/// recorded again.
#[test]
fn an_undo_under_a_listed_folder_says_nothing_there_is_recorded() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let oboete = |args: &[&str]| run(home.path(), cwd.path(), args);
    let around = cwd.path().join("private").to_string_lossy().into_owned();
    let inside = cwd
        .path()
        .join("private")
        .join("project")
        .to_string_lossy()
        .into_owned();
    oboete(&["capture", "exclude", "--folder", &around]);
    oboete(&["capture", "exclude", "--folder", &inside]);
    let out = oboete(&["capture", "exclude", "--folder", &inside, "--undo"]);
    assert!(
        out.contains(&format!(
            "{inside} is no longer excluded, but {around} around it is: nothing there is recorded"
        )) && !out.contains("recorded again"),
        "{out}"
    );
    let out = oboete(&["capture", "exclude", "--folder", &around, "--undo"]);
    assert!(
        out.contains(&format!("recorded again from now on: {around}")),
        "{out}"
    );
}

/// CodeRabbit on #489: a folder taken off the list in a repository that is never recorded is not
/// said to be recorded again: the repository is named.
#[test]
fn an_undo_in_a_repository_never_recorded_says_nothing_there_is_recorded() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join(".git")).unwrap();
    std::fs::write(
        cwd.path().join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:o/secret.git\n",
    )
    .unwrap();
    let oboete = |args: &[&str]| run(home.path(), cwd.path(), args);
    let folder = cwd.path().join("private").to_string_lossy().into_owned();
    std::fs::create_dir(&folder).unwrap();
    oboete(&["capture", "exclude"]);
    oboete(&["capture", "exclude", "--folder", &folder]);
    let out = oboete(&["capture", "exclude", "--folder", &folder, "--undo"]);
    assert!(
        out.contains(&format!(
            "{folder} is no longer excluded, but nothing there is recorded"
        )) && out.contains("its repository github.com/o/secret is excluded")
            && !out.contains("recorded again"),
        "{out}"
    );
}
