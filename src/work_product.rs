//! A request's work product (docs/my-work.md, slice 4): claude-mem's
//! `src/services/work-view/work-product.mjs` at v15.1.0 and its two test files (Apache-2.0;
//! NOTICE), ported to Rust and changed. The patterns keep JavaScript's meaning (`js_regex`,
//! `js_lines`, a title cut at 140 UTF-16 units), so a command reads as claude-mem reads it.
//! Changed: which recorded calls are read is oboete's (`may_make`, kept by the products
//! consumer), and a file is named by its path, not served.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Product {
    pub kind: String,
    pub title: String,
    pub url: Option<String>,
    pub path: Option<String>,
    pub sha: Option<String>,
    pub branch: Option<String>,
    pub repo: Option<String>,
    pub heads: Vec<String>,
    pub field: Option<String>,
    pub at: Option<i64>,
    pub source: Option<Source>,
    /// The repository of the record it was read from (`Event.repo`): a commit or a branch takes a
    /// push's repository only from a push recorded in the same one (#652).
    pub checkout: Option<String>,
    /// Its path as it was recorded, before the page's gate, where that is known: what a copy's own
    /// path is compared with (docs/deliverables.md, #840).
    pub recorded: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Source {
    pub kind: String,
    pub id: String,
    pub index: Option<usize>,
}

const FILE_KINDS: &[(&str, &str)] = &[
    (".png", "image"),
    (".jpg", "image"),
    (".jpeg", "image"),
    (".webp", "image"),
    (".gif", "image"),
    (".svg", "image"),
    (".mp4", "video"),
    (".mov", "video"),
    (".webm", "video"),
    (".pdf", "document"),
    (".docx", "document"),
    (".pptx", "deck"),
    (".key", "deck"),
    (".xlsx", "spreadsheet"),
    (".csv", "spreadsheet"),
    (".html", "page"),
];
const WORK_PRODUCT_FIELDS: &[&str] = &[
    "work_product",
    "work_products",
    "deliverable",
    "deliverables",
];
const LEGACY_KINDS: &[(&str, &str)] = &[
    ("docs_url", "documentation"),
    ("documentation_url", "documentation"),
    ("promo_url", "promotion"),
    ("announcement_url", "promotion"),
    ("pr_url", "pull-request"),
    ("deck_url", "deck"),
    ("artifact_url", "artifact"),
];
const GIT_PREFIX: &str =
    r#"(?:^|[;&|(\n])\s*(?:rtk\s+)?(?:\/usr\/bin\/)?git(?:\s+-[Cc]\s+(?:"[^"]*"|'[^']*'|\S+))*\s+"#;
const SHA: &str = "[0-9a-f]{7,40}";

/// Keep JavaScript's character classes in the ported patterns.
fn js_regex(pattern: &str) -> Result<Regex, regex::Error> {
    let pattern = pattern
        .replace(r"\s", r"[\s\u{feff}--\u{85}]")
        .replace(r"\S", r"[^\s\u{feff}--\u{85}]")
        .replace(r"\w", "[A-Za-z0-9_]")
        .replace(r"\d", "[0-9]")
        .replace(r"\b", r"(?-u:\b)");
    Regex::new(&pattern)
}

/// JavaScript's multiline anchors recognize all four line terminators.
fn js_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(['\n', '\r', '\u{2028}', '\u{2029}'])
}

pub(crate) fn whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

pub(crate) fn short(value: &str) -> Option<String> {
    let text = value
        .split(whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    if text.encode_utf16().count() <= 140 {
        return Some(text);
    }
    let units = text.encode_utf16().take(139).collect::<Vec<_>>();
    Some(format!(
        "{}…",
        String::from_utf16_lossy(&units).trim_end_matches(whitespace)
    ))
}

fn web_parts(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_matches(whitespace);
    let (scheme, rest) = value.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@')
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']')?;
        authority[1..end].parse::<std::net::Ipv6Addr>().ok()?;
        let suffix = &authority[end + 1..];
        (
            &authority[..=end],
            if suffix.is_empty() {
                None
            } else {
                Some(suffix.strip_prefix(':')?)
            },
        )
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        if host.contains(['[', ']']) {
            return None;
        }
        (host, port)
    };
    if host.is_empty() || port.is_some_and(|port| !port.is_empty() && port.parse::<u16>().is_err())
    {
        return None;
    }
    let path = rest[end..].split(['?', '#']).next().unwrap_or("");
    Some((host, path))
}

fn safe_web_url(value: &str) -> Option<String> {
    web_parts(value)?;
    Some(value.trim_matches(whitespace).to_owned())
}

fn is_local_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    let absolute = value.starts_with('/')
        || value.starts_with(r"\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    absolute && !value.contains('\0') && !value.split(['\\', '/']).any(|part| part == "..")
}

fn basename(path: &str) -> &str {
    let windows = path.starts_with(r"\\") || path.as_bytes().get(1) == Some(&b':');
    let separator = |c| c == '/' || (windows && c == '\\');
    path.trim_end_matches(separator)
        .rsplit(separator)
        .next()
        .unwrap_or("")
}

fn extension_kind(path: &str) -> Option<&'static str> {
    let name = basename(path);
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    FILE_KINDS
        .iter()
        .find_map(|(suffix, kind)| suffix[1..].eq_ignore_ascii_case(extension).then_some(*kind))
}

pub(crate) fn file_kind(path: &str) -> Option<&'static str> {
    is_local_path(path).then(|| extension_kind(path)).flatten()
}

fn kind_for_url(url: &str, fallback: &str) -> String {
    static PULL: LazyLock<Regex> = LazyLock::new(|| js_regex(r"/pull/\d+").unwrap());
    static DEPLOYMENT: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"\.(?:vercel\.app|netlify\.app|pages\.dev)$").unwrap());
    let Some((host, path)) = web_parts(url) else {
        return fallback.to_owned();
    };
    let host = host.to_ascii_lowercase();
    if host == "github.com" && PULL.is_match(path) {
        "pull-request".to_owned()
    } else if DEPLOYMENT.is_match(&host) {
        "deployment".to_owned()
    } else {
        extension_kind(path).unwrap_or(fallback).to_owned()
    }
}

fn parse(value: &Value) -> Cow<'_, Value> {
    match value.as_str() {
        Some(text) => Cow::Owned(
            serde_json::from_str(text)
                .ok()
                .or_else(|| uncut(text))
                .unwrap_or(Value::Null),
        ),
        None => Cow::Borrowed(value),
    }
}

/// JSON text that capture cut over its cap (spec 2.4) read as JSON again: the marker between the
/// kept head and tail, whose line breaks are its own, becomes text of the string it fell in.
/// shortcut: a cut between two strings or inside an escape stays unread, as before; read the
/// head and the tail apart if such answers are found to lose a product.
fn uncut(text: &str) -> Option<Value> {
    let (head, rest) = text.split_once("\n…[cut: ")?;
    let (size, tail) = rest.split_once(" bytes in full]…\n")?;
    serde_json::from_str(&format!("{head}\\n…[cut: {size} bytes in full]…\\n{tail}")).ok()
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn urls_in(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(value) => found.extend(safe_web_url(value)),
        Value::Array(values) => {
            for value in values {
                urls_in(value, found);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if key.to_ascii_lowercase().ends_with("url")
                    || value.is_object()
                    || value.is_array()
                {
                    urls_in(value, found);
                }
            }
        }
        _ => {}
    }
}

fn git_pattern(subcommand: &str) -> Regex {
    js_regex(&format!("{GIT_PREFIX}{subcommand}")).unwrap()
}

fn unquote(token: &str) -> &str {
    if token.len() >= 2
        && matches!(token.as_bytes()[0], b'\'' | b'"')
        && token.as_bytes().first() == token.as_bytes().last()
        && !token.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
    {
        &token[1..token.len() - 1]
    } else {
        token
    }
}

fn github_repo(remote: &str) -> Option<String> {
    static REMOTE: LazyLock<Regex> = LazyLock::new(|| {
        js_regex(r"^(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)([\w.-]+)/([\w.-]+?)(?:\.git)?/?$").unwrap()
    });
    let found = REMOTE.captures(remote.trim_matches(whitespace))?;
    Some(format!("{}/{}", &found[1], &found[2]))
}

/// What a branch's name holds that a link's path cannot hold as it is: the URL standard's path
/// set, and `%`. Its slashes stay, as GitHub's tree links keep them.
const BRANCH_PATH: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

fn git_url(product: &Product) -> Option<String> {
    let repo = product.repo.as_deref().filter(|repo| !repo.is_empty())?;
    if product.kind == "commit" {
        Some(format!(
            "https://github.com/{repo}/commit/{}",
            product.sha.as_deref()?
        ))
    } else {
        Some(format!(
            "https://github.com/{repo}/tree/{}",
            percent_encoding::utf8_percent_encode(product.branch.as_deref()?, BRANCH_PATH)
        ))
    }
}

fn output_text(raw: &Value, response: &Value) -> String {
    match response {
        Value::String(text) => text.clone(),
        Value::Object(_) | Value::Array(_) => [response.get("stdout"), response.get("stderr")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => raw.as_str().unwrap_or("").to_owned(),
    }
}

fn commit_subjects(command: &str) -> Vec<Option<String>> {
    static COMMIT: LazyLock<Regex> = LazyLock::new(|| git_pattern(r"commit\b"));
    static INLINE: LazyLock<Regex> = LazyLock::new(|| {
        js_regex(r#"\s-[a-zA-Z]*m\s+(?:"((?:[^"\\]|\\[^\r\n\u{2028}\u{2029}])*)"|'([^']*)')"#)
            .unwrap()
    });
    static HEREDOC: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r#"\s-F\s+-\s*<<-?\s*['"]?\w+['"]?\n([^\n]*)"#).unwrap());
    // Claude Code's form, `-m "$(cat <<'EOF'` with the message on the lines after it: the
    // heredoc's first line is the subject, a double quote in it too.
    static SUBSTITUTED: LazyLock<Regex> = LazyLock::new(|| {
        js_regex(r#"\s-[a-zA-Z]*m\s+"\$\(\s*cat\s*<<-?\s*['"]?\w+['"]?\n([^\n]*)"#).unwrap()
    });
    static ESCAPE: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"\\([^\r\n\u{2028}\u{2029}])").unwrap());
    let starts = COMMIT
        .find_iter(command)
        .map(|found| found.start())
        .collect::<Vec<_>>();
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let invocation =
                &command[*start..starts.get(index + 1).copied().unwrap_or(command.len())];
            let text = if let Some(found) = SUBSTITUTED.captures(invocation) {
                found[1].to_owned()
            } else if let Some(found) = INLINE.captures(invocation) {
                ESCAPE
                    .replace_all(
                        found.get(1).or_else(|| found.get(2)).unwrap().as_str(),
                        "$1",
                    )
                    .into_owned()
            } else {
                HEREDOC.captures(invocation)?.get(1)?.as_str().to_owned()
            };
            short(text.split('\n').next().unwrap_or(""))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct Commit {
    pub sha: String,
    pub branch: Option<String>,
    pub subject: Option<String>,
}

fn sha_key(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn add_commit(commits: &mut Vec<Commit>, sha: &str, branch: Option<&str>, subject: Option<String>) {
    if let Some(known) = commits
        .iter_mut()
        .find(|known| sha_key(&known.sha) == sha_key(sha))
    {
        if sha.len() >= known.sha.len() {
            known.sha = sha.to_owned();
        }
        if let Some(branch) = branch {
            known.branch = Some(branch.to_owned());
        }
        if subject.is_some() {
            known.subject = subject;
        }
    } else {
        commits.push(Commit {
            sha: sha.to_owned(),
            branch: branch.map(str::to_owned),
            subject,
        });
    }
}

fn commits_from(command: &str, response: &Value, out: &str) -> Vec<Commit> {
    static PRINTED: LazyLock<Regex> = LazyLock::new(|| {
        js_regex(&format!(
            r"(?m)^\[(.+?)(?: \(root-commit\))? ({SHA})\] (.+)$"
        ))
        .unwrap()
    });
    static VALID_SHA: LazyLock<Regex> = LazyLock::new(|| js_regex(&format!(r"^{SHA}$")).unwrap());
    static RTK: LazyLock<Regex> =
        LazyLock::new(|| js_regex(&format!(r"(?m)^ok ({SHA})$")).unwrap());
    static LOG: LazyLock<Regex> =
        LazyLock::new(|| js_regex(&format!(r"(?m)^({SHA}) (.+)$")).unwrap());
    let recorded = &response["gitOperation"]["commit"];
    let subjects = commit_subjects(command);
    let mut commits = Vec::new();
    if subjects.is_empty() && !truthy(&recorded["sha"]) {
        return commits;
    }
    for found in js_lines(out).filter_map(|line| PRINTED.captures(line)) {
        let branch = (!found[1].starts_with("detached HEAD")).then_some(&found[1]);
        add_commit(&mut commits, &found[2], branch, short(&found[3]));
    }
    if let Some(sha) = recorded["sha"]
        .as_str()
        .filter(|sha| VALID_SHA.is_match(sha))
    {
        add_commit(&mut commits, sha, recorded["branch"].as_str(), None);
    }
    if commits.is_empty() {
        if let Some(found) = (subjects.len() == 1)
            .then(|| js_lines(out).find_map(|line| RTK.captures(line)))
            .flatten()
        {
            add_commit(&mut commits, &found[1], None, subjects[0].clone());
        } else {
            let printed = js_lines(out)
                .filter_map(|line| LOG.captures(line))
                .map(|found| (found[1].to_owned(), short(&found[2])))
                .collect::<Vec<_>>();
            for subject in subjects.into_iter().flatten() {
                if let Some((sha, _)) = printed
                    .iter()
                    .find(|(_, printed)| printed.as_deref() == Some(&subject))
                {
                    add_commit(&mut commits, sha, None, Some(subject));
                }
            }
        }
    }
    commits
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Branch {
    pub branch: String,
    pub repo: Option<String>,
    pub heads: Vec<String>,
}

/// The arguments of the command's first `git push`, unquoted.
fn push_arguments(command: &str) -> Vec<&str> {
    static PUSH: LazyLock<Regex> =
        LazyLock::new(|| git_pattern(r#"push((?:[ \t]+(?:"[^"]*"|'[^']*'|[^\s;&|)]+))*)"#));
    PUSH.captures(command)
        .and_then(|found| found.get(1))
        .map_or("", |found| found.as_str())
        .split(whitespace)
        .filter(|token| !token.is_empty())
        .map(unquote)
        .collect()
}

fn push_target(tokens: Vec<&str>) -> Option<Branch> {
    if tokens
        .iter()
        .any(|token| ["-d", "--delete", "--tags", "--mirror", "--all"].contains(token))
    {
        return None;
    }
    let mut positional = tokens.into_iter().filter(|token| !token.starts_with('-'));
    let repo = positional.next().and_then(github_repo);
    let branch = positional
        .next()
        .filter(|token| !token.starts_with(':'))
        .map(|token| {
            token
                .strip_prefix('+')
                .unwrap_or(token)
                .rsplit(':')
                .next()
                .unwrap_or("")
        })
        .filter(|branch| !branch.starts_with("refs/tags/"))
        .map(|branch| branch.strip_prefix("refs/heads/").unwrap_or(branch))
        .filter(|branch| *branch != "HEAD")
        .unwrap_or("")
        .to_owned();
    Some(Branch {
        branch,
        repo,
        heads: Vec::new(),
    })
}

/// `branch` in `branches`, in place of the entry of its name in its repository or in none (a
/// branch the command created, then pushed): a push of it to another remote is one more entry.
fn set_branch(branches: &mut Vec<Branch>, branch: Branch) {
    if let Some(known) = branches.iter_mut().find(|known| {
        known.branch == branch.branch && (known.repo.is_none() || known.repo == branch.repo)
    }) {
        *known = branch;
    } else {
        branches.push(branch);
    }
}

fn pushes_from(command: &str, response: &Value, out: &str, failed: bool) -> Vec<Branch> {
    static PUSH: LazyLock<Regex> = LazyLock::new(|| git_pattern(r"push\b"));
    static REMOTE: LazyLock<Regex> = LazyLock::new(|| js_regex(r"^To (\S+)$").unwrap());
    static FETCH: LazyLock<Regex> = LazyLock::new(|| js_regex(r"^From \S+$").unwrap());
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        js_regex(&format!(
            r"^\s*[ +*=]?\s*(?:{SHA}\.\.\.?({SHA})|\[new branch\])\s+(\S+)\s+->\s+(\S+)"
        ))
        .unwrap()
    });
    static REJECTED: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"(?m)\[rejected\]|failed to push|^fatal:").unwrap());
    static TAG: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"^\s*\*?\s*\[new tag\]\s+\S+\s+->\s+(\S+)").unwrap());
    let runs = PUSH.is_match(command);
    let recorded = response["gitOperation"]["push"]["branch"].as_str();
    if !runs && recorded.is_none_or(str::is_empty) {
        return Vec::new();
    }
    let arguments = if runs {
        push_arguments(command)
    } else {
        Vec::new()
    };
    // A dry run prints the lines a push does and pushes nothing. shortcut: only the command's
    // first `git push` is read for it; read each push's own if a dry run is followed by a push.
    if arguments
        .iter()
        .any(|argument| ["-n", "--dry-run"].contains(argument))
    {
        return Vec::new();
    }
    let target = runs.then(|| push_target(arguments)).flatten();
    let mut repo = None;
    // The ref lines `git fetch` and `git pull` print under `From`, until a push's `To`.
    let mut fetched = false;
    let mut pushes = Vec::new();
    for line in out.split('\n') {
        if let Some(found) = REMOTE.captures(line) {
            repo = github_repo(&found[1]);
            fetched = false;
            continue;
        }
        if FETCH.is_match(line) {
            fetched = true;
            continue;
        }
        if fetched {
            continue;
        }
        if let Some(found) = REF.captures(line) {
            let branch = found[3].strip_prefix("refs/heads/").unwrap_or(&found[3]);
            set_branch(
                &mut pushes,
                Branch {
                    branch: branch.to_owned(),
                    repo: repo
                        .clone()
                        .or_else(|| target.as_ref().and_then(|target| target.repo.clone())),
                    heads: found
                        .get(1)
                        .map(|head| head.as_str().to_owned())
                        .into_iter()
                        .collect(),
                },
            );
        }
    }
    let branch = recorded.or_else(|| target.as_ref().map(|target| target.branch.as_str()));
    // A failed call pushed only the refs its output names (CodeRabbit on #650), and a name the
    // output shows as a new tag is no branch.
    if pushes.is_empty()
        && !failed
        && !js_lines(out).any(|line| REJECTED.is_match(line))
        && let Some(branch) = branch.filter(|branch| !branch.is_empty())
        && !js_lines(out).any(|line| {
            TAG.captures(line).is_some_and(|found| {
                found[1].strip_prefix("refs/tags/").unwrap_or(&found[1]) == branch
            })
        })
    {
        pushes.push(Branch {
            branch: branch.to_owned(),
            repo: target.and_then(|target| target.repo),
            heads: Vec::new(),
        });
    }
    pushes
}

fn created_branches_from(command: &str, out: &str) -> Vec<Branch> {
    static FATAL: LazyLock<Regex> = LazyLock::new(|| js_regex(r"(?m)^fatal:").unwrap());
    static CREATE: LazyLock<Regex> = LazyLock::new(|| {
        git_pattern(r#"(?:checkout\s+-[bB]|switch\s+-[cC])\s+("[^"]+"|'[^']+'|[^\s;&|)]+)"#)
    });
    if js_lines(out).any(|line| FATAL.is_match(line)) {
        return Vec::new();
    }
    CREATE
        .captures_iter(command)
        .map(|found| Branch {
            branch: unquote(&found[1]).to_owned(),
            ..Branch::default()
        })
        .collect()
}

fn creates_pull_request(command: &str) -> bool {
    static CREATE: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"(?:^|[;&|(\n])\s*(?:rtk\s+)?gh\s+pr\s+create\b").unwrap());
    CREATE.is_match(command)
}

fn pull_request_from(
    command: &str,
    response: &Value,
    out: &str,
    at: Option<i64>,
    source: Option<Source>,
) -> Vec<Product> {
    static PRINTED: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"(?m)^https://github\.com/[\w.-]+/[\w.-]+/pull/\d+$").unwrap());
    static TITLE: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r#"--title\s+(?:"([^"]+)"|'([^']+)')"#).unwrap());
    if !creates_pull_request(command) {
        return Vec::new();
    }
    // Without the empty lines, as `\r\n` is two breaks here.
    let lines = js_lines(out)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    // The pull request gh names on the line after `already exists:` is the one the branch had
    // already, which this call did not make, whether or not the call failed.
    let existing = |url: &str| {
        (lines.windows(2))
            .any(|pair| pair[0].trim_end().ends_with("already exists:") && pair[1] == url)
    };
    let printed = (lines.iter())
        .filter_map(|line| PRINTED.find(line))
        .find(|found| !existing(found.as_str()));
    let recorded = response["gitOperation"]["pr"]
        .get("url")
        .filter(|value| !value.is_null());
    let url = if let Some(recorded) = recorded {
        recorded.as_str().and_then(safe_web_url)
    } else {
        printed.and_then(|found| safe_web_url(found.as_str()))
    };
    let Some(url) = url.filter(|url| !existing(url)) else {
        return Vec::new();
    };
    let title = TITLE
        .captures(command)
        .and_then(|found| short(found.get(1).or_else(|| found.get(2))?.as_str()))
        .unwrap_or_else(|| format!("PR #{}", url.rsplit('/').next().unwrap_or("")));
    vec![Product {
        kind: "pull-request".to_owned(),
        title,
        url: Some(url),
        at,
        source,
        ..Product::default()
    }]
}

pub(crate) fn from_command(
    command: &str,
    response: &Value,
    out: &str,
    at: Option<i64>,
    source: Option<Source>,
    failed: bool,
) -> Vec<Product> {
    let pushes = pushes_from(command, response, out, failed);
    // A failed call made only what its output confirms: the branches its command names may never
    // have been made (Codex on #650).
    let created = if failed {
        Vec::new()
    } else {
        created_branches_from(command, out)
    };
    let mut branches = Vec::new();
    for branch in created.into_iter().chain(pushes.iter().cloned()) {
        set_branch(&mut branches, branch);
    }
    // A branch's push, its push to GitHub first where it went to several remotes.
    let pushed_to = |branch: &str| {
        (pushes.iter())
            .filter(|push| push.branch == branch)
            .min_by_key(|push| push.repo.is_none())
    };
    let mut products = Vec::new();
    for commit in commits_from(command, response, out) {
        let pushed = commit
            .branch
            .as_deref()
            .filter(|branch| !branch.is_empty())
            .and_then(pushed_to)
            .or_else(|| {
                (pushes.iter())
                    .filter(|push| {
                        push.heads
                            .iter()
                            .any(|head| head.starts_with(sha_key(&commit.sha)))
                    })
                    .min_by_key(|push| push.repo.is_none())
            })
            .or_else(|| {
                let first = pushes.first()?;
                (commit.branch.as_deref().is_none_or(str::is_empty)
                    && pushes.iter().all(|push| push.branch == first.branch))
                .then(|| pushed_to(&first.branch))
                .flatten()
            });
        products.push(Product {
            kind: "commit".to_owned(),
            title: commit.subject.map_or_else(
                || sha_key(&commit.sha).to_owned(),
                |subject| format!("{} {subject}", sha_key(&commit.sha)),
            ),
            sha: Some(commit.sha),
            branch: commit
                .branch
                .or_else(|| pushed.map(|push| push.branch.clone())),
            repo: pushed.and_then(|push| push.repo.clone()),
            at,
            source: source.clone(),
            ..Product::default()
        });
    }
    for branch in branches {
        products.push(Product {
            kind: "branch".to_owned(),
            title: branch.branch.clone(),
            branch: Some(branch.branch),
            repo: branch.repo,
            heads: branch.heads,
            at,
            source: source.clone(),
            ..Product::default()
        });
    }
    for product in &mut products {
        product.url = git_url(product);
    }
    products
}

/// The kind a publishing tool's link is, by the tool's name: claude-mem's `PUBLISHING_TOOLS`.
fn publisher(tool: &str) -> Option<&'static str> {
    static PUBLISHERS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
        [
            (r"^Artifact$", "artifact"),
            (r"notion-create-pages$", "page"),
            (r"Claude_Docs__(?:batch|create)$", "document"),
            (r"chatgpt_space__create_page$", "page"),
            (r"sites__save_version_and_deploy", "deployment"),
        ]
        .into_iter()
        .map(|(pattern, kind)| (js_regex(pattern).unwrap(), kind))
        .collect()
    });
    (PUBLISHERS.iter()).find_map(|(pattern, kind)| pattern.is_match(tool).then_some(*kind))
}

/// Whether a tool record's body may hold work product: a publishing tool's call, or a shell call
/// (`hook::SHELLS`: Claude Code's and Codex's `Bash`, Grok's) that runs `gh pr create` or git's
/// commit, push or branch creation, as claude-mem picks the calls it reads (`readProductRecords`)
/// and as the patterns that read them find it (`checkout -B`, `gh  pr create`).
pub(crate) fn may_make(body: &str) -> bool {
    let Ok(record) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let tool = record["tool"].as_str().unwrap_or("");
    if !crate::hook::SHELLS.contains(&tool) {
        return publisher(tool).is_some();
    }
    let input = parse(&record["input"]);
    let Some(command) = input["command"].as_str() else {
        return false;
    };
    command.contains("gh pr create")
        || (command.contains("git")
            && ["commit", "push", "checkout -b", "switch -c"]
                .iter()
                .any(|word| command.contains(word)))
        || creates_pull_request(command)
        || !created_branches_from(command, "").is_empty()
}

pub(crate) fn from_tool_use(
    tool: &str,
    input: &Value,
    response: &Value,
    at: Option<i64>,
    source: Option<Source>,
    failed: bool,
) -> Vec<Product> {
    static API: LazyLock<Regex> = LazyLock::new(|| js_regex(r"/(?:api|oauth)/").unwrap());
    let input = parse(input);
    let raw = response;
    let response = parse(response);
    if crate::hook::SHELLS.contains(&tool)
        && let Some(command) = input["command"].as_str()
    {
        let out = output_text(raw, &response);
        let mut products = pull_request_from(command, &response, &out, at, source.clone());
        products.extend(from_command(command, &response, &out, at, source, failed));
        return products;
    }
    // A publishing tool that failed published nothing: a link in its error is not its work.
    let Some(kind) = publisher(tool).filter(|_| !failed) else {
        return Vec::new();
    };
    if !truthy(&response)
        || (tool == "Artifact"
            && (input
                .get("action")
                .is_some_and(|action| !action.is_null() && action != "publish")
                || truthy(&input["asset"])))
    {
        return Vec::new();
    }
    let mut urls = Vec::new();
    urls_in(&response, &mut urls);
    let Some(url) = urls
        .into_iter()
        .find(|url| web_parts(url).is_some_and(|(_, path)| !API.is_match(path)))
    else {
        return Vec::new();
    };
    let host = web_parts(&url).unwrap().0.to_ascii_lowercase();
    let title = response
        .get("title")
        .filter(|value| !value.is_null())
        .or_else(|| input.get("title").filter(|value| !value.is_null()))
        .or_else(|| input.get("name"))
        .and_then(Value::as_str)
        .and_then(short)
        .or_else(|| {
            input["file_path"]
                .as_str()
                .filter(|path| is_local_path(path))
                .map(|path| basename(path).to_owned())
        })
        .unwrap_or(host);
    vec![Product {
        kind: kind_for_url(&url, kind),
        title,
        url: Some(url),
        at,
        source,
        ..Product::default()
    }]
}

pub(crate) fn from_files(
    paths: &[String],
    at: Option<i64>,
    source: Option<Source>,
) -> Vec<Product> {
    static INDEX: LazyLock<Regex> = LazyLock::new(|| js_regex(r"(?i)^index\.\w+$").unwrap());
    paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            let kind = file_kind(path)?;
            let name = basename(path);
            let title = if INDEX.is_match(name) {
                let units = path.encode_utf16().collect::<Vec<_>>();
                let end = units.len().saturating_sub(name.encode_utf16().count() + 1);
                let parent = String::from_utf16_lossy(&units[..end]);
                let folder = basename(&parent);
                if folder.is_empty() {
                    name.to_owned()
                } else {
                    folder.to_owned()
                }
            } else {
                name.to_owned()
            };
            let source = source.clone().map(|mut source| {
                source.index = Some(index);
                source
            });
            Some(Product {
                kind: kind.to_owned(),
                title,
                path: Some(path.clone()),
                at,
                source,
                ..Product::default()
            })
        })
        .collect()
}

pub(crate) fn is_work_product_field(field: &str) -> bool {
    WORK_PRODUCT_FIELDS.contains(&field) || field.ends_with("_url")
}

pub(crate) fn from_fields(
    fields: &Map<String, Value>,
    at: Option<i64>,
    source: Option<Source>,
) -> Vec<Product> {
    static JSON: LazyLock<Regex> = LazyLock::new(|| js_regex(r"^\s*[\[{]").unwrap());
    let mut products = Vec::new();
    let mut index = 0;
    for (field, value) in fields {
        if !is_work_product_field(field) {
            continue;
        }
        let direct = WORK_PRODUCT_FIELDS.contains(&field.as_str());
        let decoded = if direct && value.as_str().is_some_and(|value| JSON.is_match(value)) {
            let parsed = parse(value);
            if parsed.is_null() {
                Cow::Borrowed(value)
            } else {
                parsed
            }
        } else {
            Cow::Borrowed(value)
        };
        let values = if direct {
            decoded
                .as_array()
                .map_or_else(|| std::slice::from_ref(decoded.as_ref()), Vec::as_slice)
        } else if value.is_string() {
            std::slice::from_ref(value)
        } else {
            &[]
        };
        for value in values {
            let wrapped;
            let entry = if value.is_object() {
                value
            } else {
                wrapped = serde_json::json!({ "url": value });
                &wrapped
            };
            let url = entry
                .get("url")
                .filter(|value| !value.is_null())
                .or_else(|| entry.get("link"))
                .and_then(Value::as_str)
                .and_then(safe_web_url);
            let path = entry["path"]
                .as_str()
                .filter(|path| is_local_path(path))
                .or_else(|| entry["url"].as_str().filter(|path| is_local_path(path)));
            let kind = entry["kind"].as_str().and_then(short);
            let title = entry
                .get("title")
                .filter(|value| !value.is_null())
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .and_then(short);
            let source = source.clone().map(|mut source| {
                source.index = Some(index);
                source
            });
            index += 1;
            if let Some(url) = url {
                let fallback = LEGACY_KINDS
                    .iter()
                    .find_map(|(name, kind)| (*name == field).then_some(*kind))
                    .unwrap_or("link");
                let host = web_parts(&url).unwrap().0.to_ascii_lowercase();
                products.push(Product {
                    kind: kind.unwrap_or_else(|| kind_for_url(&url, fallback)),
                    title: title.unwrap_or(host),
                    url: Some(url),
                    field: Some(field.clone()),
                    at,
                    source,
                    ..Product::default()
                });
            } else if let Some(path) = path
                && let Some(kind) = kind.or_else(|| file_kind(path).map(str::to_owned))
            {
                products.push(Product {
                    kind,
                    title: title.unwrap_or_else(|| basename(path).to_owned()),
                    path: Some(path.to_owned()),
                    field: Some(field.clone()),
                    at,
                    source,
                    ..Product::default()
                });
            }
        }
    }
    products
}

fn product_key(product: &Product) -> Option<String> {
    if product.kind == "commit"
        && let Some(sha) = product.sha.as_deref().filter(|sha| !sha.is_empty())
    {
        Some(format!("commit:{}", sha_key(sha)))
    } else if product.kind == "branch"
        && let Some(branch) = product
            .branch
            .as_deref()
            .filter(|branch| !branch.is_empty())
    {
        // One for each checkout (#652): a branch of the name in another repository is another.
        Some(format!(
            "branch:{}:{branch}",
            product.checkout.as_deref().unwrap_or("")
        ))
    } else {
        // A link the gate changed is its title alone (my_work): one for each field and title.
        // shortcut: two such links of one field and title (an untitled link's is its host) are
        // one; key them by their place in the field if both are to be shown.
        (product.url.clone().or_else(|| product.path.clone())).or_else(|| {
            Some(format!(
                "field:{}:{}",
                product.field.as_ref()?,
                product.title
            ))
        })
    }
}

pub(crate) fn merge(lists: Vec<Vec<Product>>, limit: usize) -> Vec<Product> {
    let mut products = lists.into_iter().flatten().collect::<Vec<_>>();
    // By checkout (#652): a push recorded in another repository of the request links nothing here.
    type Pushes = HashMap<(Option<String>, Option<String>), Vec<(Option<i64>, String)>>;
    let mut pushes_by_branch: Pushes = HashMap::new();
    let mut repo_by_head = HashMap::new();
    let mut repo_by_source = HashMap::new();
    for product in &products {
        if product.kind == "branch"
            && let Some(repo) = product.repo.as_ref().filter(|repo| !repo.is_empty())
        {
            pushes_by_branch
                .entry((product.checkout.clone(), product.branch.clone()))
                .or_default()
                .push((product.at, repo.clone()));
            for head in &product.heads {
                let head = (product.checkout.clone(), sha_key(head).to_owned());
                repo_by_head.insert(head, repo.clone());
            }
        }
    }
    let source_key = |product: &Product| {
        product
            .source
            .as_ref()
            .map(|source| (source.kind.clone(), source.id.clone()))
    };
    let repo_of = |product: &Product| {
        product
            .repo
            .clone()
            .or_else(|| {
                pushes_by_branch
                    .get(&(product.checkout.clone(), product.branch.clone()))?
                    .iter()
                    .find(|(at, _)| {
                        product.kind == "branch" || at.unwrap_or(0) >= product.at.unwrap_or(0)
                    })
                    .map(|(_, repo)| repo.clone())
            })
            .or_else(|| {
                let head = sha_key(product.sha.as_deref()?).to_owned();
                repo_by_head.get(&(product.checkout.clone(), head)).cloned()
            })
    };
    for product in &products {
        if product.kind == "commit"
            && let Some(repo) = repo_of(product).filter(|repo| !repo.is_empty())
            && let Some(source) = source_key(product)
        {
            repo_by_source.insert(source, repo);
        }
    }
    products.sort_by_key(|product| std::cmp::Reverse(product.at.unwrap_or(0)));
    let mut seen = HashSet::new();
    products
        .into_iter()
        .filter_map(|mut product| {
            // A product with no key is one of its own.
            if let Some(key) = product_key(&product)
                && !seen.insert(key)
            {
                return None;
            }
            if (product.kind == "commit" || product.kind == "branch")
                && product.repo.as_deref().is_none_or(str::is_empty)
            {
                let repo = repo_of(&product).or_else(|| {
                    (product.kind == "commit")
                        .then(|| repo_by_source.get(&source_key(&product)?).cloned())
                        .flatten()
                });
                if repo.as_ref().is_some_and(|repo| !repo.is_empty()) {
                    product.repo = repo;
                    product.url = git_url(&product);
                }
            }
            Some(product)
        })
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests;
