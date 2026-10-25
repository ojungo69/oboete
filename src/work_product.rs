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

fn whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

fn short(value: &str) -> Option<String> {
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

fn file_kind(path: &str) -> Option<&'static str> {
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
        Some(text) => Cow::Owned(serde_json::from_str(text).unwrap_or(Value::Null)),
        None => Cow::Borrowed(value),
    }
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
            product.branch.as_deref()?
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
            let text = if let Some(found) = INLINE.captures(invocation) {
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

fn push_target(command: &str) -> Option<Branch> {
    static PUSH: LazyLock<Regex> =
        LazyLock::new(|| git_pattern(r#"push((?:[ \t]+(?:"[^"]*"|'[^']*'|[^\s;&|)]+))*)"#));
    let found = PUSH.captures(command);
    let tokens = found
        .as_ref()
        .and_then(|found| found.get(1))
        .map_or("", |found| found.as_str())
        .split(whitespace)
        .filter(|token| !token.is_empty())
        .map(unquote)
        .collect::<Vec<_>>();
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

fn set_branch(branches: &mut Vec<Branch>, branch: Branch) {
    if let Some(known) = branches
        .iter_mut()
        .find(|known| known.branch == branch.branch)
    {
        *known = branch;
    } else {
        branches.push(branch);
    }
}

fn pushes_from(command: &str, response: &Value, out: &str) -> Vec<Branch> {
    static PUSH: LazyLock<Regex> = LazyLock::new(|| git_pattern(r"push\b"));
    static REMOTE: LazyLock<Regex> = LazyLock::new(|| js_regex(r"^To (\S+)$").unwrap());
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        js_regex(&format!(
            r"^\s*[ +*=]?\s*(?:{SHA}\.\.\.?({SHA})|\[new branch\])\s+(\S+)\s+->\s+(\S+)"
        ))
        .unwrap()
    });
    static REJECTED: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"(?m)\[rejected\]|failed to push|^fatal:").unwrap());
    let runs = PUSH.is_match(command);
    let recorded = response["gitOperation"]["push"]["branch"].as_str();
    if !runs && recorded.is_none_or(str::is_empty) {
        return Vec::new();
    }
    let target = runs.then(|| push_target(command)).flatten();
    let mut repo = None;
    let mut pushes = Vec::new();
    for line in out.split('\n') {
        if let Some(found) = REMOTE.captures(line) {
            repo = github_repo(&found[1]);
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
    if pushes.is_empty()
        && !js_lines(out).any(|line| REJECTED.is_match(line))
        && let Some(branch) = branch.filter(|branch| !branch.is_empty())
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

fn pull_request_from(
    command: &str,
    response: &Value,
    out: &str,
    at: Option<i64>,
    source: Option<Source>,
) -> Vec<Product> {
    static CREATE: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"(?:^|[;&|(\n])\s*(?:rtk\s+)?gh\s+pr\s+create\b").unwrap());
    static PRINTED: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r"(?m)^https://github\.com/[\w.-]+/[\w.-]+/pull/\d+$").unwrap());
    static TITLE: LazyLock<Regex> =
        LazyLock::new(|| js_regex(r#"--title\s+(?:"([^"]+)"|'([^']+)')"#).unwrap());
    if !CREATE.is_match(command) {
        return Vec::new();
    }
    let printed = js_lines(out).find_map(|line| PRINTED.find(line));
    let recorded = response["gitOperation"]["pr"]
        .get("url")
        .filter(|value| !value.is_null());
    let url = if let Some(recorded) = recorded {
        recorded.as_str().and_then(safe_web_url)
    } else {
        printed.and_then(|found| safe_web_url(found.as_str()))
    };
    let Some(url) = url else { return Vec::new() };
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
) -> Vec<Product> {
    let pushes = pushes_from(command, response, out);
    let mut branches = Vec::new();
    for branch in created_branches_from(command, out)
        .into_iter()
        .chain(pushes.iter().cloned())
    {
        set_branch(&mut branches, branch);
    }
    let mut products = Vec::new();
    for commit in commits_from(command, response, out) {
        let pushed = commit
            .branch
            .as_ref()
            .filter(|branch| !branch.is_empty())
            .and_then(|branch| pushes.iter().find(|push| push.branch == *branch))
            .or_else(|| {
                pushes.iter().find(|push| {
                    push.heads
                        .iter()
                        .any(|head| head.starts_with(sha_key(&commit.sha)))
                })
            })
            .or_else(|| {
                (commit.branch.as_deref().is_none_or(str::is_empty) && pushes.len() == 1)
                    .then(|| &pushes[0])
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

/// Whether a tool record's body may hold work product: a publishing tool's call, or a `Bash` call
/// that runs `gh pr create` or git's commit, push or branch creation, as claude-mem picks the
/// calls it reads (`readProductRecords`).
pub(crate) fn may_make(body: &str) -> bool {
    let Ok(record) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let tool = record["tool"].as_str().unwrap_or("");
    if tool != "Bash" {
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
}

pub(crate) fn from_tool_use(
    tool: &str,
    input: &Value,
    response: &Value,
    at: Option<i64>,
    source: Option<Source>,
) -> Vec<Product> {
    static API: LazyLock<Regex> = LazyLock::new(|| js_regex(r"/(?:api|oauth)/").unwrap());
    let input = parse(input);
    let raw = response;
    let response = parse(response);
    if tool == "Bash"
        && let Some(command) = input["command"].as_str()
    {
        let out = output_text(raw, &response);
        let mut products = pull_request_from(command, &response, &out, at, source.clone());
        products.extend(from_command(command, &response, &out, at, source));
        return products;
    }
    let Some(kind) = publisher(tool) else {
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

fn is_work_product_field(field: &str) -> bool {
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
        Some(format!("branch:{branch}"))
    } else {
        product.url.clone().or_else(|| product.path.clone())
    }
}

pub(crate) fn merge(lists: Vec<Vec<Product>>, limit: usize) -> Vec<Product> {
    let mut products = lists.into_iter().flatten().collect::<Vec<_>>();
    let mut pushes_by_branch: HashMap<Option<String>, Vec<(Option<i64>, String)>> = HashMap::new();
    let mut repo_by_head = HashMap::new();
    let mut repo_by_source = HashMap::new();
    for product in &products {
        if product.kind == "branch"
            && let Some(repo) = product.repo.as_ref().filter(|repo| !repo.is_empty())
        {
            pushes_by_branch
                .entry(product.branch.clone())
                .or_default()
                .push((product.at, repo.clone()));
            for head in &product.heads {
                repo_by_head.insert(sha_key(head).to_owned(), repo.clone());
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
                    .get(&product.branch)?
                    .iter()
                    .find(|(at, _)| {
                        product.kind == "branch" || at.unwrap_or(0) >= product.at.unwrap_or(0)
                    })
                    .map(|(_, repo)| repo.clone())
            })
            .or_else(|| repo_by_head.get(sha_key(product.sha.as_deref()?)).cloned())
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
            if !seen.insert(product_key(&product)) {
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
mod tests {
    use super::*;
    use serde_json::json;

    fn source(kind: &str, id: &str) -> Source {
        Source {
            kind: kind.to_owned(),
            id: id.to_owned(),
            index: None,
        }
    }

    fn bash_at(command: &str, response: Value, at: i64, id: &str) -> Vec<Product> {
        from_tool_use(
            "Bash",
            &json!(json!({ "command": command }).to_string()),
            &json!(response.to_string()),
            Some(at),
            Some(source("tool-use", id)),
        )
    }

    fn bash(command: &str, response: Value) -> Vec<Product> {
        bash_at(command, response, 100, "5")
    }

    fn brief(products: &[Product]) -> Vec<Value> {
        products.iter().map(|product| {
            let mut value = json!({ "kind": product.kind, "title": product.title, "branch": product.branch, "url": product.url });
            if let Some(sha) = &product.sha { value["sha"] = json!(sha); }
            value
        }).collect()
    }

    #[test]
    fn an_artifact_publish_records_its_link_and_title() {
        let products = from_tool_use(
            "Artifact",
            &json!(json!({ "file_path": "/w/deck.html", "title": "Launch deck" }).to_string()),
            &json!(
                json!({ "url": "https://claude.ai/artifact/abc", "title": "Launch deck" })
                    .to_string()
            ),
            Some(100),
            Some(source("tool-use", "5")),
        );
        assert_eq!(
            products,
            vec![Product {
                kind: "artifact".to_owned(),
                title: "Launch deck".to_owned(),
                url: Some("https://claude.ai/artifact/abc".to_owned()),
                at: Some(100),
                source: Some(source("tool-use", "5")),
                ..Product::default()
            }]
        );
    }

    #[test]
    fn artifact_lists_reads_and_assets_publish_nothing() {
        for input in [
            json!({ "action": "list" }),
            json!({ "action": "read" }),
            json!({ "action": "describe" }),
            json!({ "action": "publish", "asset": true }),
            json!({ "asset": {} }),
        ] {
            assert!(
                from_tool_use(
                    "Artifact",
                    &input,
                    &json!({ "artifacts": [{ "url": "https://claude.ai/artifact/x" }] }),
                    None,
                    None
                )
                .is_empty()
            );
        }
        assert!(
            from_tool_use(
                "Read",
                &json!({}),
                &json!({ "url": "https://example.com/report" }),
                None,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn gh_pr_create_records_the_pull_request_url_and_title() {
        let products = bash(
            r#"git push && rtk gh pr create --title "Add work product" --body x"#,
            json!({ "stdout": "https://github.com/o/r/pull/42\n" }),
        );
        assert_eq!(products[0].kind, "pull-request");
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(products[0].title, "Add work product");
        assert!(
            bash(
                r#"sqlite3 db "select * from tool_uses where tool_input like '%gh pr create%'""#,
                json!({ "stdout": "x|https://github.com/o/r/pull/7", "gitOperation": { "pr": {
                "number": 7, "url": "https://github.com/o/r/pull/7", "action": "created" } } })
            )
            .is_empty()
        );
    }

    #[test]
    fn a_pull_request_uses_its_recorded_url_and_defaults_to_its_number() {
        let products = bash(
            "gh pr create --body x",
            json!({ "stdout": "https://github.com/o/r/pull/7\n",
            "gitOperation": { "pr": { "url": "https://github.com/o/r/pull/42" } } }),
        );
        assert_eq!(products[0].title, "PR #42");
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(
            bash(
                "gh pr create --title 'A title'",
                json!({ "stdout": "https://github.com/o/r/pull/7" })
            )[0]
            .title,
            "A title"
        );
        assert!(bash("gh pr create", json!({ "stdout": "https://github.com/o/r/pull/7", "gitOperation": { "pr": { "url": "javascript:alert(1)" } } })).is_empty());
    }

    #[test]
    fn observers_record_only_human_facing_files_with_their_positions() {
        let paths = ["/w/src/app.ts", "/w/design/pass-01.png", "/w/report.html"].map(str::to_owned);
        let products = from_files(&paths, Some(50), Some(source("observation", "9")));
        assert_eq!(
            products
                .iter()
                .map(|product| (
                    product.kind.as_str(),
                    product.title.as_str(),
                    product.source.as_ref().unwrap().index
                ))
                .collect::<Vec<_>>(),
            vec![
                ("image", "pass-01.png", Some(1)),
                ("page", "report.html", Some(2))
            ]
        );
        assert!(products.iter().all(|product| product.at == Some(50) && product.source.as_ref().unwrap().id == "9"));
        assert_eq!(file_kind("relative/x.png"), None);
        assert_eq!(file_kind("/w/../etc/x.png"), None);
    }

    #[test]
    fn an_index_file_takes_its_folders_name() {
        let products = from_files(
            &[
                "/w/launch-review/index.html".to_owned(),
                "/index.html".to_owned(),
                "/w/hero.PNG".to_owned(),
            ],
            None,
            None,
        );
        assert_eq!(
            products
                .iter()
                .map(|product| product.title.as_str())
                .collect::<Vec<_>>(),
            vec!["launch-review", "index.html", "hero.PNG"]
        );
        assert!(products.iter().all(|product| product.source.is_none()));
    }

    #[test]
    fn unicode_paths_with_trailing_separators_keep_their_file_titles() {
        let paths = [
            "/tmp/é.png//",
            "/😀.png///",
            "/資料/index.html",
            "/資料/index.html///",
        ]
        .map(str::to_owned);
        let products = from_files(&paths, None, None);
        assert_eq!(
            products
                .iter()
                .map(|product| product.title.as_str())
                .collect::<Vec<_>>(),
            vec!["é.png", "😀.png", "資料", "in"]
        );
    }

    #[test]
    fn github_patterns_use_javascripts_ascii_words_and_digits() {
        assert_eq!(github_repo("https://github.com/owner/répo.git"), None);
        assert_eq!(
            kind_for_url("https://github.com/o/r/pull/１２", "link"),
            "link"
        );
        assert!(
            bash(
                "gh pr create",
                json!({"stdout": "https://github.com/o/r/pull/１２"})
            )
            .is_empty()
        );
    }

    #[test]
    fn git_commands_keep_javascripts_word_boundaries() {
        let products = from_command(
            "git commit中文 -m 'subject'",
            &json!({}),
            "[main abc1234] subject",
            None,
            None,
        );
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].sha.as_deref(), Some("abc1234"));
    }

    #[test]
    fn git_patterns_keep_javascripts_whitespace_classes() {
        let products = from_command(
            "git\u{feff}commit -m 'subject'",
            &json!({}),
            "[main abc1234] subject",
            None,
            None,
        );
        assert_eq!(products.len(), 1);
        assert!(
            from_command(
                "git\u{85}commit -m 'subject'",
                &json!({}),
                "[main abc1234] subject",
                None,
                None
            )
            .is_empty()
        );
        let pattern = js_regex(r"^[\w.-]+\s+\S+$").unwrap();
        assert!(pattern.is_match("ascii\u{feff}name\u{85}x"));
        assert!(!pattern.is_match("répo name"));
    }

    #[test]
    fn printed_commits_and_pull_requests_accept_javascripts_line_terminators() {
        for separator in ["\r\n", "\r", "\u{2028}", "\u{2029}"] {
            let products = bash(
                "rtk git commit -m 'subject'",
                json!(format!("before{separator}ok abc1234{separator}")),
            );
            assert_eq!(products[0].title, "abc1234 subject", "{separator:?}");
            let products = bash(
                "gh pr create",
                json!({ "stdout": format!("before{separator}https://github.com/o/r/pull/42{separator}") }),
            );
            assert_eq!(products[0].title, "PR #42", "{separator:?}");
            assert!(
                bash(
                    "git checkout -b feat/x",
                    json!(format!("before{separator}fatal: branch exists"))
                )
                .is_empty()
            );
            assert!(
                bash(
                    "git push origin feat/x",
                    json!(format!("before{separator}fatal: remote failed"))
                )
                .is_empty()
            );
        }
        let products = bash(
            "git push origin feat/x",
            json!({ "stdout": "To https://github.com/o/r.git\r\n   1111111..2222222 feat/x -> feat/x\r\n" }),
        );
        assert_eq!(products[0].repo, None);
    }

    #[test]
    fn work_product_fields_attach_links_and_files_and_drop_unsafe_urls() {
        let fields = json!({
            "work_product": [{ "title": "Q4 deck", "url": "https://docs.google.com/presentation/d/1" },
                { "path": "/w/out/hero.png" }, "javascript:alert(1)"],
            "docs_url": "https://docs.example.com/guide", "status": "done"
        });
        let products = from_fields(
            fields.as_object().unwrap(),
            Some(3),
            Some(source("work-state", "11")),
        );
        assert_eq!(
            products
                .iter()
                .map(|product| (
                    product.kind.as_str(),
                    product.title.as_str(),
                    product.field.as_deref(),
                    product.source.as_ref().unwrap().index
                ))
                .collect::<Vec<_>>(),
            vec![
                ("link", "Q4 deck", Some("work_product"), Some(0)),
                ("image", "hero.png", Some("work_product"), Some(1)),
                (
                    "documentation",
                    "docs.example.com",
                    Some("docs_url"),
                    Some(3)
                )
            ]
        );
        assert_eq!(products[1].path.as_deref(), Some("/w/out/hero.png"));
        assert!(products.iter().all(|product| product.at == Some(3)));
    }

    #[test]
    fn several_work_products_with_titles_arrive_as_one_json_string() {
        let fields = json!({ "work_product": json!([
            { "title": "Q4 deck", "url": "https://docs.google.com/presentation/d/1" }, { "path": "/w/out/hero.png" }
        ]).to_string() });
        assert_eq!(
            from_fields(fields.as_object().unwrap(), None, None)
                .iter()
                .map(|product| (product.kind.as_str(), product.title.as_str()))
                .collect::<Vec<_>>(),
            vec![("link", "Q4 deck"), ("image", "hero.png")]
        );
        let fields = json!({ "work_product": "/w/out/report.pdf" });
        assert_eq!(
            from_fields(fields.as_object().unwrap(), None, None)[0].title,
            "report.pdf"
        );
    }

    #[test]
    fn local_paths_are_absolute_and_have_no_parent_part_or_nul() {
        for path in [
            r"C:\work\hero.png",
            "C:/work/report.pdf",
            r"\\server\share\hero.png",
            "/w/out/hero.png",
        ] {
            assert!(is_local_path(path), "{path}");
        }
        for path in [
            r"C:\work\..\secret.png",
            r"work\hero.png",
            "C:hero.png",
            "/w/../hero.png",
            "/w/hero\0.png",
            r"/w\..\hero.png",
        ] {
            assert!(!is_local_path(path), "{path}");
        }
        assert_eq!(file_kind(r"C:\work\hero.PNG"), Some("image"));
        assert_eq!(file_kind(r"\\server\share\deck.pptx"), Some("deck"));
    }

    #[test]
    fn merging_keeps_the_newest_entry_for_each_link_or_file() {
        let merged = merge(
            vec![
                vec![Product {
                    url: Some("https://a".to_owned()),
                    at: Some(1),
                    title: "old".to_owned(),
                    ..Product::default()
                }],
                vec![
                    Product {
                        url: Some("https://a".to_owned()),
                        at: Some(2),
                        title: "new".to_owned(),
                        ..Product::default()
                    },
                    Product {
                        path: Some("/b.png".to_owned()),
                        at: Some(0),
                        ..Product::default()
                    },
                ],
            ],
            60,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "new");
        assert_eq!(merged[1].path.as_deref(), Some("/b.png"));
    }

    #[test]
    fn a_git_commit_names_its_subject_and_has_no_link_without_a_push() {
        let products = bash(
            "/usr/bin/git -C /w/repo commit -F /w/repo/.scratch/message.txt",
            json!({
                "stdout": "[teambrain-purchase 8ade2c4] fix(teambrain): QA for self-serve purchase (#333)\n 9 files changed, 642 insertions(+), 84 deletions(-)\n create mode 100644 src/lib/teambrain/billing-card.ts",
                "stderr": "", "gitOperation": { "commit": { "sha": "8ade2c4", "kind": "committed", "branch": "teambrain-purchase" } }
            }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "8ade2c4 fix(teambrain): QA for self-serve purchase (#333)",
            "sha": "8ade2c4", "branch": "teambrain-purchase", "url": null })
            ]
        );
        let detached = bash(
            r#"git commit -m "fix(pro): the opt-out button ignores presses no person could have made""#,
            json!({ "stdout": "[detached HEAD 6edf530] fix(pro): the opt-out button ignores presses no person could have made\n 7 files changed, 567 insertions(+), 146 deletions(-)" }),
        );
        assert_eq!(detached[0].branch, None);
        assert_eq!(
            detached[0].title,
            "6edf530 fix(pro): the opt-out button ignores presses no person could have made"
        );
    }

    #[test]
    fn a_quiet_commit_takes_its_subject_from_git_log() {
        let command = [
            "cd \"/w/claude-mem 🧠/my-work-react-viewer\"; git reset -q; git add src/services/work-view/request-projection.mjs tests/worker/work-view/request-projection.test.mjs && git commit -q -F - <<'EOF'",
            "fix(work-view): English asks need an action verb before they become requests", "", "Port the prototype's action-verb rule.", "EOF",
            "git add -A src tests plugin && git commit -q -F - <<'EOF'",
            "feat(work-view): progress readings, measured ETA, goal links and the build story", "", "- record-progress keeps readings[].", "EOF",
            "git log --oneline -3; git status --short | grep -v \"^??\"",
        ].join("\n");
        let products = bash(
            &command,
            json!({ "stdout": "bc8cd096b feat(work-view): progress readings, measured ETA, goal links and the build story\naf4ba749c fix(work-view): English asks need an action verb before they become requests\n3990ea7e1 fix(work-view): port request-grouping review fixes from the prototype", "stderr": "" }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "af4ba74 fix(work-view): English asks need an action verb before they become requests", "sha": "af4ba749c", "branch": null, "url": null }),
                json!({ "kind": "commit", "title": "bc8cd09 feat(work-view): progress readings, measured ETA, goal links and the build story", "sha": "bc8cd096b", "branch": null, "url": null })
            ]
        );
    }

    #[test]
    fn an_rtk_commit_takes_its_sha_from_the_ok_line() {
        let products = bash(
            "rtk git commit -m 'Align timeline hydration assertions with expanded scope'",
            json!("ok bb07907\n"),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "bb07907 Align timeline hydration assertions with expanded scope", "sha": "bb07907", "branch": null, "url": null })
            ]
        );
    }

    #[test]
    fn a_quiet_commit_with_no_printed_subject_is_not_guessed() {
        assert!(
            bash(
                "/usr/bin/git commit -q -F .scratch/hooks/message.txt",
                json!({ "stdout": "", "stderr": "" })
            )
            .is_empty()
        );
    }

    #[test]
    fn a_command_that_only_mentions_git_commit_or_git_push_made_nothing() {
        let out =
            json!({ "stdout": "[feat/x abc1234] subject\n   1111111..2222222  feat/x -> feat/x" });
        assert!(bash(r#"cd ~/.claude-mem && sqlite3 -readonly claude-mem.db "SELECT id FROM tool_uses WHERE tool_input LIKE '%git commit%' AND tool_input LIKE '%git push%'""#,
            out.clone()).is_empty());
        assert!(bash("echo 'git commit'; printf 'git push'", out).is_empty());
    }

    #[test]
    fn a_git_push_links_its_branch_and_the_commit_made_by_the_same_command() {
        let command = [
            "W=\"/w/claude-mem 🧠/install-funnel\"; cd \"$W\"; git add src/services/agent-runner/detect.ts tests/services/agent-runner/detect.test.ts && git commit -q -F - <<'EOF'",
            "feat(agent-runner): detect which agent CLIs are installed and logged in", "", "Adds detectUsableAgents().", "EOF",
            "git log --oneline -1; git push -u origin feat/install-funnel-early 2>&1 | tail -6",
        ].join("\n");
        let products = bash(
            &command,
            json!({
                "stdout": "30d795d5a feat(agent-runner): detect which agent CLIs are installed and logged in\nremote: GitHub found 2 vulnerabilities on thedotmack/claude-mem's default branch (2 critical). To find out more, visit:\nremote:      https://github.com/thedotmack/claude-mem/security/dependabot\nremote:\nTo https://github.com/thedotmack/claude-mem.git\n * [new branch]          feat/install-funnel-early -> feat/install-funnel-early\nbranch 'feat/install-funnel-early' set up to track 'origin/feat/install-funnel-early'.",
                "stderr": "", "gitOperation": { "push": { "branch": "feat/install-funnel-early" } }
            }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "30d795d feat(agent-runner): detect which agent CLIs are installed and logged in", "sha": "30d795d5a", "branch": "feat/install-funnel-early", "url": "https://github.com/thedotmack/claude-mem/commit/30d795d5a" }),
                json!({ "kind": "branch", "title": "feat/install-funnel-early", "branch": "feat/install-funnel-early", "url": "https://github.com/thedotmack/claude-mem/tree/feat/install-funnel-early" })
            ]
        );
    }

    #[test]
    fn a_push_uses_the_commands_remote_when_the_to_line_is_missing() {
        let products = bash(
            "/usr/bin/git push https://github.com/SuperMarioYL/claude-mem.git HEAD:perf/search-reads-read-worker 2>&1 | tail -1",
            json!({ "stdout": "   92aa26602..659ae0d38  HEAD -> perf/search-reads-read-worker", "stderr": "", "gitOperation": { "push": { "branch": "perf/search-reads-read-worker" } } }),
        );
        assert_eq!(
            products[0].branch.as_deref(),
            Some("perf/search-reads-read-worker")
        );
        assert_eq!(products[0].heads, vec!["659ae0d38"]);
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/SuperMarioYL/claude-mem/tree/perf/search-reads-read-worker")
        );
    }

    #[test]
    fn a_push_with_only_a_remote_name_has_no_link() {
        let products = bash(
            "cd \"/w/claude-mem 🧠/my-work-react-viewer\" && for i in 1 2 3; do git push -u origin feat/my-work-react-viewer 2>&1 | tail -2 && break; sleep 5; done",
            json!({ "stdout": "   81cc75296..3edf6fd8e  feat/my-work-react-viewer -> feat/my-work-react-viewer\nbranch 'feat/my-work-react-viewer' set up to track 'origin/feat/my-work-react-viewer'.",
                "gitOperation": { "push": { "branch": "feat/my-work-react-viewer" } } }),
        );
        assert_eq!(products[0].kind, "branch");
        assert_eq!(
            products[0].branch.as_deref(),
            Some("feat/my-work-react-viewer")
        );
        assert_eq!(products[0].url, None);
    }

    #[test]
    fn one_rtk_sha_for_two_commits_is_not_guessed() {
        let products = bash(
            "rtk git add -A && rtk git commit -m 'chore: bump version to 15.0.1' && rtk git tag -a v15.0.1 -m 'Version 15.0.1' && rtk git push origin HEAD:main && rtk git push origin v15.0.1 && rtk git add CHANGELOG.md && rtk git commit -m 'docs: update changelog for v15.0.1' && rtk git push origin HEAD:main",
            json!(
                "ok 1 file changed, 12 insertions(+)\nok 85e4ee3\nremote:\nTo https://github.com/thedotmack/claude-mem.git\n   0e7de4ee4..85e4ee31f  HEAD -> main\nok main\n"
            ),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "branch", "title": "main", "branch": "main", "url": "https://github.com/thedotmack/claude-mem/tree/main" })
            ]
        );
    }

    #[test]
    fn a_quiet_push_names_its_branch_without_a_ref_line() {
        let products = bash(
            "git push -q -u origin feat/work-view-git-products",
            json!({ "stdout": "", "stderr": "" }),
        );
        assert_eq!(products[0].kind, "branch");
        assert_eq!(
            products[0].branch.as_deref(),
            Some("feat/work-view-git-products")
        );
        assert_eq!(products[0].url, None);
        let products = bash(
            "git push -q https://github.com/o/r.git HEAD:refs/heads/feat/x",
            json!({ "stdout": "" }),
        );
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/o/r/tree/feat/x")
        );
    }

    #[test]
    fn rejected_or_deleting_pushes_make_nothing_and_other_hosts_have_no_link() {
        assert!(bash("git push origin feat/x", json!({ "stdout": "", "stderr": " ! [rejected]        feat/x -> feat/x (fetch first)\nerror: failed to push some refs to 'https://github.com/o/r.git'" })).is_empty());
        for flags in ["--delete", "-d", "--tags", "--all", "--mirror"] {
            assert!(
                bash(
                    &format!("git push {flags} origin feat/x"),
                    json!({ "stdout": "" })
                )
                .is_empty()
            );
        }
        let products = bash(
            "git push origin feat/x",
            json!({ "stdout": "", "stderr": "To gitlab.com:o/r.git\n   1111111..2222222  feat/x -> feat/x" }),
        );
        assert_eq!(products[0].branch.as_deref(), Some("feat/x"));
        assert_eq!(products[0].url, None);
    }

    #[test]
    fn github_remotes_name_their_owner_and_repository() {
        for remote in [
            "https://github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r",
            " https://github.com/o/r.git/ ",
        ] {
            assert_eq!(github_repo(remote).as_deref(), Some("o/r"), "{remote}");
        }
        for remote in [
            "https://gitlab.com/o/r.git",
            "origin",
            "https://github.com/o/r/pull/1",
        ] {
            assert_eq!(github_repo(remote), None, "{remote}");
        }
    }

    #[test]
    fn git_checkout_and_git_switch_record_the_branch_they_created() {
        let products = bash(
            "/usr/bin/git fetch origin -q && /usr/bin/git checkout -b fix/launcher-bun-be-bun-reentry-4933 origin/main && /usr/bin/git log -1 --oneline",
            json!({ "stdout": "Switched to a new branch 'fix/launcher-bun-be-bun-reentry-4933'\nbranch 'fix/launcher-bun-be-bun-reentry-4933' set up to track 'origin/main'.\nf3cb32e43 fix: skip reasoning on OpenRouter field condensation requests (#4925)" }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "branch", "title": "fix/launcher-bun-be-bun-reentry-4933", "branch": "fix/launcher-bun-be-bun-reentry-4933", "url": null })
            ]
        );
        let products = bash(
            "rtk git switch -c fix/cloud-sync-census-api-execution origin/main",
            json!(
                "Switched to a new branch 'fix/cloud-sync-census-api-execution'\nbranch 'fix/cloud-sync-census-api-execution' set up to track 'origin/main'.\n"
            ),
        );
        assert_eq!(
            products[0].branch.as_deref(),
            Some("fix/cloud-sync-census-api-execution")
        );
        assert!(
            bash(
                "git checkout -b feat/x",
                json!({ "stdout": "", "stderr": "fatal: a branch named 'feat/x' already exists" })
            )
            .is_empty()
        );
    }

    #[test]
    fn merging_links_commits_from_a_later_push_and_their_shared_source() {
        let commits = bash_at(
            "git commit -q -m \"fix(work-view): English asks need an action verb before they become requests\" && git commit -q -m \"feat(work-view): progress readings, measured ETA, goal links and the build story\"; git log --oneline -2",
            json!({ "stdout": "bc8cd096b feat(work-view): progress readings, measured ETA, goal links and the build story\naf4ba749c fix(work-view): English asks need an action verb before they become requests" }),
            10,
            "179579",
        );
        let created = bash_at(
            "git checkout -b feat/my-work-react-viewer",
            json!({ "stdout": "Switched to a new branch 'feat/my-work-react-viewer'" }),
            5,
            "9",
        );
        let pushed = bash_at(
            "rtk git push origin feat/my-work-react-viewer 2>&1 | tail -3",
            json!({ "stdout": " 23 files changed, 1101 insertions(+), 597 deletions(-)\nTo https://github.com/thedotmack/claude-mem.git\n   3990ea7e1..bc8cd096b  feat/my-work-react-viewer -> feat/my-work-react-viewer\nok feat/my-work-react-viewer",
                "gitOperation": { "push": { "branch": "feat/my-work-react-viewer" } } }),
            20,
            "179704",
        );
        let again = bash_at(
            "git commit -m x",
            json!({ "stdout": "[feat/my-work-react-viewer bc8cd096b] feat(work-view): progress readings, measured ETA, goal links and the build story" }),
            8,
            "11",
        );
        let merged = merge(vec![commits, created, pushed.clone(), again], 60);
        assert_eq!(
            merged
                .iter()
                .map(|product| json!([product.kind, product.title, product.url]))
                .collect::<Vec<_>>(),
            vec![
                json!([
                    "branch",
                    "feat/my-work-react-viewer",
                    "https://github.com/thedotmack/claude-mem/tree/feat/my-work-react-viewer"
                ]),
                json!([
                    "commit",
                    "af4ba74 fix(work-view): English asks need an action verb before they become requests",
                    "https://github.com/thedotmack/claude-mem/commit/af4ba749c"
                ]),
                json!([
                    "commit",
                    "bc8cd09 feat(work-view): progress readings, measured ETA, goal links and the build story",
                    "https://github.com/thedotmack/claude-mem/commit/bc8cd096b"
                ])
            ]
        );
        let later = bash_at(
            "git commit -m y",
            json!({ "stdout": "[feat/my-work-react-viewer 20c6a29] fix(agent-runner): wait for each agent CLI instead of timing out" }),
            30,
            "12",
        );
        assert_eq!(
            merge(vec![pushed, later], 60)
                .iter()
                .find(|product| product.kind == "commit")
                .unwrap()
                .url,
            None
        );
    }

    #[test]
    fn safe_web_urls_have_a_host_and_no_credentials() {
        for url in [
            "https://example.com",
            " http://example.com/report.pdf ",
            "HTTPS://EXAMPLE.COM:443/a",
            "https://[::1]:8080/a",
        ] {
            assert_eq!(safe_web_url(url).as_deref(), Some(url.trim()));
        }
        for url in [
            "javascript:alert(1)",
            "file:///w/report.pdf",
            "https://",
            "https:///path",
            "https://:80/x",
            "https://u:p@example.com/x",
            "https://@example.com",
            "https://a b/x",
            "https://host:wrong/x",
            "https://host:65536/x",
            "https://[wrong]/",
        ] {
            assert_eq!(safe_web_url(url), None, "{url}");
        }
    }

    #[test]
    fn url_kinds_follow_github_deployments_and_file_extensions() {
        for (url, expected) in [
            ("https://GITHUB.com/o/r/pull/42", "pull-request"),
            ("https://preview.vercel.app/hero.png", "deployment"),
            ("https://preview.netlify.app/", "deployment"),
            ("https://preview.pages.dev/", "deployment"),
            (
                "https://example.com/out/REPORT.PDF?download=1#page",
                "document",
            ),
            ("https://example.com/deck.pptx", "deck"),
            ("https://example.com/report", "link"),
            ("https://vercel.app/", "link"),
        ] {
            assert_eq!(kind_for_url(url, "link"), expected, "{url}");
        }
        for (extension, kind) in FILE_KINDS {
            assert_eq!(file_kind(&format!("/w/report{extension}")), Some(*kind));
        }
        for path in ["/w/app.rs", "/w/data.json", "/w/.png"] {
            assert_eq!(file_kind(path), None);
        }
    }

    #[test]
    fn publishers_take_the_first_public_link_and_apply_the_title_rule() {
        for (tool, kind) in [
            ("mcp__notion-create-pages", "page"),
            ("mcp__Claude_Docs__batch", "document"),
            ("mcp__Claude_Docs__create", "document"),
            ("mcp__chatgpt_space__create_page", "page"),
            ("mcp__sites__save_version_and_deploy_extra", "deployment"),
        ] {
            let products = from_tool_use(
                tool,
                &json!({ "title": " Input   title " }),
                &json!({
                    "apiUrl": "https://example.com/api/page", "nested": [{ "url": "https://example.com/oauth/token" },
                        { "url": "https://example.com/first" }, { "url": "https://example.com/second" }],
                    "title": " Response\n title "
                }),
                None,
                None,
            );
            assert_eq!(products.len(), 1, "{tool}");
            assert_eq!(products[0].kind, kind, "{tool}");
            assert_eq!(products[0].title, "Response title");
            assert_eq!(
                products[0].url.as_deref(),
                Some("https://example.com/first")
            );
        }
        for (input, response, title) in [
            (
                json!({ "name": "Named" }),
                json!({ "url": "https://example.com/x" }),
                "Named",
            ),
            (
                json!({ "file_path": "/w/deck.html" }),
                json!({ "url": "https://example.com/x" }),
                "deck.html",
            ),
            (
                json!({ "title": "Ignored", "file_path": "/w/deck.html" }),
                json!({ "title": "", "url": "https://example.com/x" }),
                "deck.html",
            ),
            (
                json!({}),
                json!({ "url": "https://EXAMPLE.com:443/x" }),
                "example.com",
            ),
            (
                json!({ "action": null, "asset": false }),
                json!("https://example.com/x"),
                "example.com",
            ),
        ] {
            assert_eq!(
                from_tool_use("Artifact", &input, &response.to_string().into(), None, None)[0]
                    .title,
                title
            );
        }
        assert!(
            from_tool_use(
                "Artifact",
                &json!({}),
                &json!({ "text": "https://example.com/ignored" }),
                None,
                None
            )
            .is_empty()
        );
        assert!(from_tool_use("Artifact", &json!({}), &json!("not JSON"), None, None).is_empty());
    }

    #[test]
    fn work_product_fields_keep_overrides_aliases_and_indices_across_fields() {
        let fields = json!({
            "status": "done",
            "deliverable": "{\"link\":\"https://example.com/report\",\"name\":\"A report\",\"kind\":\"custom\"}",
            "deliverables": [{ "path": "/w/result.txt", "kind": "text" }, { "path": "relative.pdf" }],
            "work_products": [{ "url": "/w/result.csv", "title": "Results" }, { "url": "https://example.com/page.html", "path": "/w/other.pdf" }],
            "custom_url": "https://example.com/deck.key", "ignored_url": { "url": "https://example.com/ignored" },
            "Work_Product": "https://example.com/ignored"
        });
        let products = from_fields(
            fields.as_object().unwrap(),
            None,
            Some(source("work-state", "2")),
        );
        assert_eq!(
            products
                .iter()
                .map(|product| (
                    product.kind.as_str(),
                    product.title.as_str(),
                    product.source.as_ref().unwrap().index
                ))
                .collect::<Vec<_>>(),
            vec![
                ("custom", "A report", Some(0)),
                ("text", "result.txt", Some(1)),
                ("spreadsheet", "Results", Some(3)),
                ("page", "example.com", Some(4)),
                ("deck", "example.com", Some(5))
            ]
        );
        for (field, kind) in LEGACY_KINDS {
            let fields = Map::from_iter([(field.to_string(), json!("https://example.com/report"))]);
            assert_eq!(from_fields(&fields, None, None)[0].kind, *kind);
        }
        assert!(is_work_product_field("custom_url"));
        assert!(!is_work_product_field("custom_URL"));
        assert!(
            from_fields(
                json!({ "work_product": "[broken" }).as_object().unwrap(),
                None,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn bash_responses_may_be_objects_json_strings_or_raw_text() {
        for response in [
            json!({ "stdout": "ok abc1234" }),
            json!("ok abc1234"),
            json!(json!({ "stderr": "ok abc1234" }).to_string()),
            json!(json!("ok abc1234").to_string()),
        ] {
            let products = from_tool_use(
                "Bash",
                &json!({ "command": "rtk git commit -m 'subject'" }),
                &response,
                None,
                None,
            );
            assert_eq!(products[0].title, "abc1234 subject", "{response}");
        }
        assert_eq!(
            output_text(&Value::Null, &json!({ "stdout": "a", "stderr": "b" })),
            "a\nb"
        );
        assert_eq!(
            output_text(&json!("raw"), &json!({ "stdout": 3, "stderr": null })),
            ""
        );
    }

    #[test]
    fn commit_subjects_unescape_inline_messages_and_take_only_the_first_line() {
        assert_eq!(
            commit_subjects(
                "git -c user.name='A' -C \"/w/a b\" commit -am \"first \\\"quoted\\\"\nbody\" && rtk git commit -m 'second\\ subject'"
            ),
            vec![
                Some("first \"quoted\"".to_owned()),
                Some("second subject".to_owned())
            ]
        );
        assert_eq!(
            commit_subjects("git commit -F - <<-\"MSG\"\n subject  line\nbody\nMSG"),
            vec![Some("subject line".to_owned())]
        );
        assert_eq!(commit_subjects("git commit -F file"), vec![None]);
        for separator in ["\n", "\r", "\u{2028}", "\u{2029}"] {
            assert_eq!(
                commit_subjects(&format!("git commit -m \"a\\{separator}b\"")),
                vec![None]
            );
        }
    }

    #[test]
    fn recorded_commits_keep_the_longest_sha_and_the_printed_subject() {
        let response =
            json!({ "gitOperation": { "commit": { "sha": "abc1234567890", "branch": "main" } } });
        let products = from_command(
            "git commit -m 'subject'",
            &response,
            "[main (root-commit) abc1234] subject",
            Some(2),
            None,
        );
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].sha.as_deref(), Some("abc1234567890"));
        assert_eq!(products[0].branch.as_deref(), Some("main"));
        assert_eq!(products[0].title, "abc1234 subject");
        let products = from_command("git status", &response, "", None, None);
        assert_eq!(products[0].title, "abc1234");
        assert!(
            from_command(
                "git status",
                &json!({ "gitOperation": { "commit": { "sha": "bad" } } }),
                "",
                None,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn pushes_preserve_ref_order_and_match_heads_across_branches() {
        let products = from_command(
            "git commit -m 'subject'; git push https://github.com/o/r.git HEAD:main",
            &json!({}),
            "[other abc1234] subject\nTo https://github.com/o/r.git\n + 1111111...abc123456  HEAD -> refs/heads/main\n * [new branch] feat/x -> feat/x",
            None,
            None,
        );
        assert_eq!(products.len(), 3);
        assert_eq!(products[0].repo.as_deref(), Some("o/r"));
        assert_eq!(products[0].branch.as_deref(), Some("other"));
        assert_eq!(products[1].branch.as_deref(), Some("main"));
        assert_eq!(products[1].heads, vec!["abc123456"]);
        assert_eq!(products[2].branch.as_deref(), Some("feat/x"));
        let products = from_command(
            "git checkout -b 'feat/x' && git push https://github.com/o/r.git feat/x",
            &json!({}),
            "",
            None,
            None,
        );
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].repo.as_deref(), Some("o/r"));
    }

    #[test]
    fn merging_is_stable_for_equal_times_and_applies_its_limit_after_deduplication() {
        let lists = vec![vec![
            Product {
                title: "first".to_owned(),
                path: Some("/a.png".to_owned()),
                ..Product::default()
            },
            Product {
                title: "duplicate".to_owned(),
                path: Some("/a.png".to_owned()),
                at: Some(0),
                ..Product::default()
            },
            Product {
                title: "second".to_owned(),
                path: Some("/b.png".to_owned()),
                ..Product::default()
            },
            Product {
                title: "third".to_owned(),
                path: Some("/c.png".to_owned()),
                at: Some(-1),
                ..Product::default()
            },
        ]];
        assert!(merge(lists.clone(), 0).is_empty());
        assert_eq!(
            merge(lists.clone(), 2)
                .iter()
                .map(|product| product.title.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(merge(lists, 60).len(), 3);
    }

    #[test]
    fn titles_collapse_whitespace_and_use_the_javascript_utf16_limit() {
        assert_eq!(short(" \n A\t title \r "), Some("A title".to_owned()));
        assert_eq!(short(" \n\t"), None);
        assert_eq!(short(&"a".repeat(140)), Some("a".repeat(140)));
        assert_eq!(
            short(&"a".repeat(141)),
            Some(format!("{}…", "a".repeat(139)))
        );
        assert_eq!(short(&"😀".repeat(70)), Some("😀".repeat(70)));
        assert_eq!(
            short(&format!("a{}", "😀".repeat(70))),
            Some(format!("a{}…", "😀".repeat(69)))
        );
        assert_eq!(short("\u{feff}title\u{feff}"), Some("title".to_owned()));
        assert_eq!(short("a\u{85}b"), Some("a\u{85}b".to_owned()));
        assert_eq!(
            short(&format!("{}\u{85}bc", "a".repeat(138))),
            Some(format!("{}\u{85}…", "a".repeat(138)))
        );
        assert_eq!(
            short(&"😀".repeat(71)),
            Some(format!("{}\u{fffd}…", "😀".repeat(69)))
        );
    }
}
