//! `oboete capture exclude` (milestone 5 slice 4a, spec 6.1): a repository or a folder is never
//! recorded from then on and the list is printed; what was recorded before stays, and the command
//! says how to forget it; `oboete exclude --undo` sends no repository that is never recorded;
//! doctor says when the current directory is not recorded; an undo records again.

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
    let oboete = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_oboete"))
            .arg("--home")
            .arg(home.path())
            .args(args)
            .current_dir(cwd.path())
            .env("OBOETE_NO_SPAWN", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
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
