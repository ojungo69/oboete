//! `oboete capture exclude` (milestone 5 slice 4a, spec 6.1): a repository or a folder is never
//! recorded from then on and the list is printed; what was recorded before stays, and the command
//! says how to forget it; `oboete exclude --undo` sends no repository that is never recorded;
//! doctor says when the current directory is not recorded; an undo records again.

fn run(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
        .arg("--home")
        .arg(home)
        .args(args)
        .current_dir(cwd)
        .env("OBOETE_NO_SPAWN", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
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
    let out = oboete(&["capture", "exclude", "--folder", "private"]);
    assert!(
        out.contains("never recorded: github.com/o/secret, ") && out.contains("private (folder)"),
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
