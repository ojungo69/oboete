//! Repository key from the filesystem only (no `git` process): walk up to the nearest `.git`.
//! Linked worktrees share the main repository's key.

use std::path::{Path, PathBuf};

pub fn key(cwd: &Path) -> String {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut dir: Option<&Path> = Some(&start);
    while let Some(d) = dir {
        let git = d.join(".git");
        if git.is_dir() {
            return d.to_string_lossy().into_owned();
        }
        if git.is_file() {
            if let Some(main) = worktree_main(&git) {
                return main.to_string_lossy().into_owned();
            }
            return d.to_string_lossy().into_owned();
        }
        dir = d.parent();
    }
    start.to_string_lossy().into_owned()
}

/// `.git` file → `gitdir: <path>` → `<path>/commondir` → main repository root.
fn worktree_main(git_file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_file).ok()?;
    let gitdir = text.strip_prefix("gitdir:")?.trim();
    let gitdir = git_file.parent()?.join(gitdir);
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common_dir = gitdir.join(common.trim()).canonicalize().ok()?;
    common_dir.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_git_dir_wins_and_worktree_maps_to_main() {
        let tmp = std::env::temp_dir().join(format!("oboete-repo-{}", std::process::id()));
        let main = tmp.join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(main.join("src/deep")).unwrap();
        assert_eq!(
            key(&main.join("src/deep")),
            main.canonicalize().unwrap().to_string_lossy()
        );

        let wt = tmp.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        assert_eq!(key(&wt), main.canonicalize().unwrap().to_string_lossy());

        let plain = tmp.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(key(&plain), plain.canonicalize().unwrap().to_string_lossy());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
