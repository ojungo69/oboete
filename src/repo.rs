//! Repository key from the filesystem only (no `git` process). The key is the `origin` remote's
//! URL reduced to `host[:port]/path`, so the same repository cloned anywhere, over ssh or https,
//! gets the same key on every device (proposal §4.2 1, decision 7). A repository without an
//! origin, and a directory outside any repository, keep their path. Linked worktrees share the
//! main repository's key.

use std::path::{Path, PathBuf};

pub fn key(cwd: &Path) -> String {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let Some((root, git_dir)) = find(&start) else {
        return start.to_string_lossy().into_owned();
    };
    read_config(&git_dir.join("config"), 0)
        .and_then(|config| origin_url(&config))
        .and_then(|url| normalize(&url))
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// The nearest repository root at or above `start`, and the git directory holding its `config`
/// (a linked worktree's is the main repository's; a submodule's is its own).
fn find(start: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut dir: Option<&Path> = Some(start);
    while let Some(d) = dir {
        let git = d.join(".git");
        if git.is_dir() {
            return Some((d.to_path_buf(), git));
        }
        if git.is_file() {
            let gitdir = gitdir(&git);
            let common = gitdir.as_deref().and_then(common_dir);
            return Some(match (gitdir, common) {
                (_, Some(common)) => (
                    common
                        .parent()
                        .map_or_else(|| d.to_path_buf(), Path::to_path_buf),
                    common,
                ),
                (Some(gitdir), None) => (d.to_path_buf(), gitdir),
                (None, None) => (d.to_path_buf(), git),
            });
        }
        dir = d.parent();
    }
    None
}

/// `.git` file → `gitdir: <path>`.
pub(crate) fn gitdir(git_file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_file).ok()?;
    let gitdir = text.strip_prefix("gitdir:")?.trim();
    Some(git_file.parent()?.join(gitdir))
}

/// A linked worktree's `<gitdir>/commondir` → the main repository's git directory.
pub(crate) fn common_dir(gitdir: &Path) -> Option<PathBuf> {
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    gitdir.join(common.trim()).canonicalize().ok()
}

/// `url` of `[remote "origin"]` in a git config file.
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in logical_lines(config) {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = origin_header(line);
        } else if in_origin
            && let Some((name, value)) = line.split_once('=')
            && name.trim().eq_ignore_ascii_case("url")
        {
            return Some(config_value(value));
        }
    }
    None
}

/// A git config file as logical lines: a backslash at the end of a line outside a comment joins
/// the next line (git reads a value across them; a comment ends at its line). CRLF reads as LF.
fn logical_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let (mut quoted, mut comment) = (false, false);
    let mut chars = text.chars().filter(|&c| c != '\r');
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                lines.push(std::mem::take(&mut line));
                (quoted, comment) = (false, false);
            }
            '\\' if !comment => match chars.next() {
                Some('\n') => {}
                Some(x) => {
                    line.push('\\');
                    line.push(x);
                }
                None => line.push('\\'),
            },
            '"' if !comment => {
                quoted = !quoted;
                line.push(c);
            }
            '#' | ';' if !quoted => {
                comment = true;
                line.push(c);
            }
            _ => line.push(c),
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A git config value: `#` or `;` outside double quotes starts a comment, the quotes are dropped,
/// a backslash escapes the next character, surrounding whitespace is trimmed.
fn config_value(raw: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(x) => out.push(x),
                None => {}
            },
            '#' | ';' if !quoted => break,
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// A git config file with the files its `[include] path = ...` lines name spliced in where they
/// appear, as git reads them: a relative path is from the including file's directory, `~/` is the
/// home directory, at most 10 levels (git's limit). `includeIf` is not evaluated.
fn read_config(path: &Path, depth: u8) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut out = String::with_capacity(text.len());
    let mut in_include = false;
    for line in logical_lines(&text) {
        out.push_str(&line);
        out.push('\n');
        let line = line.trim();
        if line.starts_with('[') {
            in_include = header(line).is_some_and(|h| h.eq_ignore_ascii_case("include"));
        } else if in_include
            && depth < 10
            && let Some((name, value)) = line.split_once('=')
            && name.trim().eq_ignore_ascii_case("path")
        {
            let value = config_value(value);
            let target = match value.strip_prefix("~/") {
                Some(rest) => crate::config::home_dir().join(rest),
                None => path.parent().unwrap_or(Path::new("")).join(&value),
            };
            if let Some(included) = read_config(&target, depth + 1) {
                // The rest of this file is still in `[include]`, not the included file's last section.
                out.push_str(&included);
                out.push_str("[include]\n");
            }
        }
    }
    Some(out)
}

/// The text between `[` and `]` of a section header line.
fn header(line: &str) -> Option<&str> {
    let (header, _) = line.strip_prefix('[')?.split_once(']')?;
    Some(header.trim())
}

/// `[remote "origin"]`, in any case for the section name, with a trailing comment, or in the old
/// `[remote.origin]` form (git reads all of them).
fn origin_header(line: &str) -> bool {
    let Some(header) = header(line) else {
        return false;
    };
    match header.split_once(char::is_whitespace) {
        Some((section, name)) => {
            section.eq_ignore_ascii_case("remote") && name.trim() == "\"origin\""
        }
        None => header.eq_ignore_ascii_case("remote.origin"),
    }
}

/// `host[:port]/path` from a remote URL: the scheme, credentials, query, fragment, the scheme's
/// default port and a trailing `.git` are dropped and the host is lowercased, so ssh, scp-style
/// and https clones of one repository agree and no secret stays in the key. None for a local
/// path or `file://`, which name nothing another device could reach.
pub fn normalize(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            (scheme.to_ascii_lowercase(), authority, path)
        }
        // scp-style `[user@]host:path`, the host maybe `[ipv6]`. One letter before the colon is
        // a Windows drive.
        None => {
            let colon = match url.find('[') {
                Some(open) if !url[..open].contains(':') => open + url[open..].find("]:")? + 1,
                _ => url.find(':')?,
            };
            let (authority, path) = (&url[..colon], &url[colon + 1..]);
            if authority.contains(['/', '\\']) || authority.chars().count() < 2 {
                return None;
            }
            ("ssh".to_string(), authority, path)
        }
    };
    if scheme == "file" {
        return None;
    }
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, p),
        _ => (host_port, ""),
    };
    let default = matches!(
        (scheme.as_str(), port),
        ("ssh" | "git+ssh" | "ssh+git", "22") | ("https", "443") | ("http", "80") | ("git", "9418")
    );
    let port = if port.is_empty() || default {
        String::new()
    } else {
        format!(":{port}")
    };
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}{port}/{path}", host.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_key_for_every_way_to_clone_and_no_secrets() {
        for url in [
            "https://github.com/ojungo69/oboete.git",
            "https://github.com/ojungo69/oboete",
            "git@github.com:ojungo69/oboete.git",
            "ssh://git@github.com/ojungo69/oboete.git",
            "ssh://git@GitHub.com:22/ojungo69/oboete/",
            "https://user:ghp_secret@github.com:443/ojungo69/oboete.git?ref=x#frag",
            "git+ssh://git@github.com/ojungo69/oboete",
        ] {
            assert_eq!(
                normalize(url).as_deref(),
                Some("github.com/ojungo69/oboete"),
                "{url}"
            );
        }
        // A port other than the scheme's default names another service on the same host.
        assert_eq!(
            normalize("https://git.example.com:8443/team/app.git").as_deref(),
            Some("git.example.com:8443/team/app")
        );
        assert_eq!(
            normalize("ssh://git@git.example.com:2222/team/app.git").as_deref(),
            Some("git.example.com:2222/team/app")
        );
        // A bracketed IPv6 host, scp-style or ssh://, with and without the default port.
        for url in [
            "git@[2001:db8::1]:team/app.git",
            "ssh://git@[2001:db8::1]/team/app.git",
            "ssh://git@[2001:db8::1]:22/team/app",
        ] {
            assert_eq!(
                normalize(url).as_deref(),
                Some("[2001:db8::1]/team/app"),
                "{url}"
            );
        }
        for local in [
            "/srv/git/app.git",
            "../app",
            "C:\\repos\\app",
            "C:/repos/app",
            "file:///srv/git/app.git",
            "https://github.com/",
            "",
        ] {
            assert_eq!(normalize(local), None, "{local}");
        }
    }

    #[test]
    fn config_origin_is_read_without_git() {
        let config = "[core]\n\turl = nope\n[remote \"upstream\"]\n\turl = https://x.org/u/up.git\n\
                      [remote \"origin\"]\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
                      \tURL = \"git@github.com:o/r.git\" # comment\n";
        assert_eq!(
            origin_url(config).as_deref(),
            Some("git@github.com:o/r.git")
        );
        assert_eq!(origin_url("[remote \"upstream\"]\nurl = x\n"), None);
        for header in [
            "[REMOTE \"origin\"]",
            "[remote.origin]",
            "[remote \"origin\"] # comment",
        ] {
            assert_eq!(
                origin_url(&format!("{header}\n\turl = https://x.org/o/r\n")).as_deref(),
                Some("https://x.org/o/r"),
                "{header}"
            );
        }
        assert_eq!(origin_url("[remote \"Origin\"]\nurl = x\n"), None);
        for value in [
            "https://x.org/o/r;comment",
            "https://x.org/o/r\t# comment",
            "\"https://x.org/o/r\" ; c",
            "  https://x.org/o/r  ",
        ] {
            assert_eq!(config_value(value), "https://x.org/o/r", "{value:?}");
        }
        assert_eq!(config_value("\"a#b\" # c"), "a#b");
        // A trailing backslash continues the value on the next line; the value ends at the line.
        assert_eq!(
            origin_url(
                "[remote \"origin\"]\r\n\turl = https://github.com/owner/\\\r\nrepo.git\r\n"
            )
            .as_deref(),
            Some("https://github.com/owner/repo.git")
        );
        assert_eq!(
            origin_url("[remote \"origin\"]\n\turl = https://x.org/o/r\n\tfetch = y\n").as_deref(),
            Some("https://x.org/o/r")
        );
    }

    #[test]
    fn nearest_git_dir_wins_and_worktree_maps_to_main() {
        let tmp = std::env::temp_dir().join(format!("oboete-repo-{}", std::process::id()));
        let main = tmp.join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(main.join("src/deep")).unwrap();
        // No origin: the path.
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

        // With an origin, the main repository, its worktree and a clone elsewhere agree.
        std::fs::write(
            main.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:o/r.git\n",
        )
        .unwrap();
        let clone = tmp.join("elsewhere/renamed");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        std::fs::write(
            clone.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/o/r\n",
        )
        .unwrap();
        for dir in [main.join("src/deep"), wt.clone(), clone] {
            assert_eq!(key(&dir), "github.com/o/r", "{}", dir.display());
        }

        // An origin that `.git/config` pulls in with `[include]`, relative to the git directory.
        let inc = tmp.join("inc");
        std::fs::create_dir_all(inc.join(".git")).unwrap();
        std::fs::write(
            inc.join(".git/config"),
            "[core]\n\tbare = false # c \\\n[include]\n\tpath = remo\\\ntes.inc # c\n\tpath = loop\n",
        )
        .unwrap();
        std::fs::write(
            inc.join(".git/remotes.inc"),
            "[remote \"origin\"]\n\turl = git@github.com:o/inc.git\n",
        )
        .unwrap();
        // A file that includes itself stops at the depth limit.
        std::fs::write(inc.join(".git/loop"), "[include]\n\tpath = loop\n").unwrap();
        assert_eq!(key(&inc), "github.com/o/inc");
        // The parent's lines after the include are not in the included file's section.
        std::fs::write(
            inc.join(".git/config"),
            "[include]\n\tpath = upstream.inc\n\turl = https://x.org/not/origin\n",
        )
        .unwrap();
        std::fs::write(
            inc.join(".git/upstream.inc"),
            "[remote \"origin\"]\n\tfetch = x\n",
        )
        .unwrap();
        assert_eq!(key(&inc), inc.canonicalize().unwrap().to_string_lossy());

        let plain = tmp.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(key(&plain), plain.canonicalize().unwrap().to_string_lossy());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
