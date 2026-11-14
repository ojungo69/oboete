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

/// The top of the working tree at or above `start`: the nearest directory holding `.git` (a linked
/// worktree's own, unlike `find`'s root). Cards name files relative to it (docs/file-note.md F3).
pub(crate) fn top(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
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
mod tests;
