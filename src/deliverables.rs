//! Copies of what a turn made (docs/deliverables.md; parity rows X23, G01, X16; decision 73):
//! claude-mem 15.9.0's deliverables (`src/cli/deliverables-capture.ts`, `src/shared/
//! deliverables.ts`; Apache-2.0, NOTICE), read from oboete's own records by a worker phase at a
//! turn's end. The files are copied as they are, kept in this device's deliverables.db and
//! `<home>/deliverables/`, never searched, sent to a model or synced, and removed when forget
//! removes their records.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::raw::{Event, Item, Raw};

/// The folder under the home the copies live in.
pub(crate) const FOLDER: &str = "deliverables";
/// Their rows, beside raw.db: device-local, never synced, untouched by a rebuild or a restore.
const DB: &str = "deliverables.db";
/// The most bytes one copy may have, as claude-mem's (`DELIVERABLE_MAX_BYTES`).
pub(crate) const MAX_BYTES: u64 = 512 << 20;
/// The most files one turn copies, declared first (claude-mem's `MAX_FILES_PER_STOP`).
pub(crate) const MAX_FILES: usize = 50;
/// NAME_MAX on APFS, ext4 and NTFS: the bound on a folder's and a copy's name.
const NAME_MAX: usize = 255;
/// Turn ends read a round.
const BATCH: usize = 200;
/// How long one git check may take (claude-mem's 5 s).
const GIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// The agents whose prompt hook sees each prompt as it comes, so a private turn is marked in its
/// records: agy's are built from its transcript afterwards, and Cursor's print mode runs its tool
/// hooks without its prompt hook.
const AGENTS: [&str; 3] = ["claude", "codex", "grok"];
/// The tools a file is written with, and the input key that names it (claude-mem reads Claude
/// Code's four; Grok's `search_replace` and `write` take `file_path` as Edit and Write do).
const WRITES: [(&str, &str); 6] = [
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("MultiEdit", "file_path"),
    ("NotebookEdit", "notebook_path"),
    ("search_replace", "file_path"),
    ("write", "file_path"),
];

const SCHEMA: &str = "
-- docs/deliverables.md: a copy of a file a turn made, by the records that made and closed it.
CREATE TABLE IF NOT EXISTS deliverables(
  id INTEGER PRIMARY KEY,
  device TEXT NOT NULL, agent TEXT NOT NULL, session TEXT NOT NULL, repo TEXT,
  turn_seq INTEGER NOT NULL, turn_ts INTEGER NOT NULL,
  call_seq INTEGER NOT NULL, call_ts INTEGER NOT NULL,
  title TEXT NOT NULL, kind TEXT NOT NULL, source_path TEXT NOT NULL,
  copy TEXT NOT NULL,
  bytes INTEGER NOT NULL, sha256 TEXT NOT NULL,
  declared INTEGER NOT NULL, list TEXT, field TEXT,
  copied INTEGER NOT NULL,
  UNIQUE(device, turn_seq, turn_ts, copy)
);
CREATE INDEX IF NOT EXISTS deliverables_session ON deliverables(agent, session);
-- The last turn end the phase took on each device, with that record's time.
CREATE TABLE IF NOT EXISTS deliverable_scan(
  device TEXT PRIMARY KEY, seq INTEGER NOT NULL, ts INTEGER, file TEXT
);
-- docs/subagents.md, slice 3: the newest stop the phase took of each Claude Code subagent on each
-- device, so a later stop takes only the calls after it, though that stop is forgotten since.
CREATE TABLE IF NOT EXISTS subagent_scan(
  device TEXT NOT NULL, session TEXT NOT NULL, agent_id TEXT NOT NULL, seq INTEGER NOT NULL,
  PRIMARY KEY(device, session, agent_id)
);
";

/// The copies' rows (docs/deliverables.md), made when first needed, readable by this user alone.
pub(crate) fn open(home: &Path) -> Result<Connection> {
    crate::raw::refuse_newer_home(home)?;
    let deadline = std::time::Instant::now() + crate::db::OPEN_WRITE_WAIT;
    let path = home.join(DB);
    crate::db::private(home, 0o700);
    let mut create = std::fs::OpenOptions::new();
    create.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    match create.open(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
            return Err(e).context("create deliverables.db");
        }
        _ => {}
    }
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    crate::db::wal_until(&conn, "NORMAL", deadline)?;
    crate::db::ensure_schema_until(&conn, SCHEMA, deadline).context("deliverables schema")?;
    // #836 item 3: raw.db's file beside the phase's checkpoint, in a store made before it.
    crate::claims::migrate(
        &conn,
        "SELECT count(*) = 0 FROM pragma_table_info('deliverable_scan') WHERE name = 'file'",
        "ALTER TABLE deliverable_scan ADD COLUMN file TEXT;",
    )?;
    for file in [DB, "deliverables.db-wal", "deliverables.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    Ok(conn)
}

/// The wholly private prompt's turn of one session (docs/deliverables.md): its flag is a file under
/// `state/private-turns/`, set by the prompt hook and cleared by the session's next public prompt
/// and its end, not by a reply (a turn a task's notification starts after it goes on with the
/// private work). It holds no text.
pub(crate) struct PrivateTurn(PathBuf);

impl PrivateTurn {
    pub(crate) fn new(home: &Path, agent: &str, session: &str) -> Self {
        let key = crate::curate::sha256_hex(&format!("{agent}\0{session}"));
        Self(home.join("state").join("private-turns").join(&key[..32]))
    }

    /// Whether the session's turn is a wholly private prompt's. A flag that cannot be read is set:
    /// a copy of a private turn's file is what must not happen (fail closed).
    pub(crate) fn is_set(&self) -> bool {
        match std::fs::symlink_metadata(&self.0) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    }

    pub(crate) fn set(&self) -> std::io::Result<()> {
        if let Some(dir) = self.0.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.0, b"")
    }

    pub(crate) fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.0) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Marks a tool call's or a reply's record as of a turn whose prompt the hooks saw public
/// (docs/deliverables.md): only such calls are copied from. Added after the gate, at the top of the
/// body, so no field's text or its ledger's offsets change.
pub(crate) fn mark_public(e: &mut Event) {
    if !matches!(e.kind.as_str(), "tool" | "reply") || !AGENTS.contains(&e.agent.as_str()) {
        return;
    }
    if let Ok(Value::Object(mut body)) = serde_json::from_str::<Value>(&e.body) {
        body.insert("public_turn".into(), Value::Bool(true));
        e.body = Value::Object(body).to_string();
    }
}

/// `value` with what no common file system takes in a name made `-`, its whitespace one space,
/// and no space, dot or dash at either end (claude-mem's `safeName`).
fn safe_name(value: &str) -> String {
    let unsafe_char = |c: char| {
        matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            || c <= '\u{1f}'
            || c == '\u{7f}'
    };
    let mut out = String::new();
    let (mut dash, mut space) = (false, false);
    for c in value.chars() {
        if unsafe_char(c) {
            if !dash {
                out.push('-');
            }
            (dash, space) = (true, false);
        } else if crate::work_product::whitespace(c) {
            if !space {
                out.push(' ');
            }
            (dash, space) = (false, true);
        } else {
            out.push(c);
            (dash, space) = (false, false);
        }
    }
    let edge = |c: char| crate::work_product::whitespace(c) || c == '.' || c == '-';
    out.trim_matches(edge).to_owned()
}

/// `value` cut whole characters at a time until `value + suffix` fits in NAME_MAX bytes, its first
/// character always kept, and no space, dot or dash at its end (claude-mem's `fitName`).
fn fit_name(value: &str, suffix: &str) -> String {
    let mut chars: Vec<char> = value.chars().collect();
    while chars.len() > 1
        && chars.iter().map(|c| c.len_utf8()).sum::<usize>() + suffix.len() > NAME_MAX
    {
        chars.pop();
    }
    let edge = |c: char| crate::work_product::whitespace(c) || c == '.' || c == '-';
    chars
        .into_iter()
        .collect::<String>()
        .trim_end_matches(edge)
        .to_owned()
}

/// A repository's folder under `deliverables/` (claude-mem's `deliverableBucketFolderName`).
pub(crate) fn folder_name(key: &str) -> String {
    let cleaned = safe_name(key);
    if cleaned.is_empty() {
        return "unknown-project".into();
    }
    if cleaned.len() <= NAME_MAX {
        return cleaned;
    }
    let hash = &crate::curate::sha256_hex(key)[..8];
    format!("{}-{hash}", fit_name(&cleaned, &format!("-{hash}")))
}

/// A file name's extension as Node's `path.extname` reads it: from its last dot, none for a name
/// that starts with its only dot.
fn extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[i..],
        _ => "",
    }
}

/// A copy's name: the source's stem made safe, then `hash` (the content hash's first 8, or all of
/// it), then the extension, within NAME_MAX (claude-mem's `deliverableBucketFileName`).
fn copy_name(source: &Path, hash: &str) -> Result<String> {
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = extension(&name);
    let stem = &name[..name.len() - ext.len()];
    // The extension made safe as the stem is (`.p\df` is a name POSIX allows), so every name a
    // copy gets is one `remove_copy` removes.
    let ext = match ext.strip_prefix('.') {
        Some(rest) => format!(".{}", safe_name(rest)),
        None => String::new(),
    };
    let suffix = format!("-{hash}{ext}");
    let stem = safe_name(stem);
    let stem = if stem.is_empty() {
        "deliverable"
    } else {
        &stem
    };
    let name = format!("{}{suffix}", fit_name(stem, &suffix));
    anyhow::ensure!(
        name.len() <= NAME_MAX,
        "its extension leaves no room for a {NAME_MAX}-byte name"
    );
    Ok(name)
}

/// The kind My Work shows a copy as (claude-mem's `deliverableKind`).
fn kind_of(path: &str) -> String {
    crate::work_product::file_kind(path).map_or_else(
        || {
            let lower = path.to_ascii_lowercase();
            if [".md", ".markdown", ".txt", ".rst"]
                .iter()
                .any(|e| lower.ends_with(e))
            {
                "document".into()
            } else {
                "file".into()
            }
        },
        str::to_owned,
    )
}

/// `path` with `.` and `..` taken out by its own parts, as Node's `path.resolve` gives it.
fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A field's value as the absolute path of a file it names: absolute, `~/…` or relative to the
/// call's folder; a URL or an empty one names none (claude-mem's `resolveCandidatePath`, whose
/// check of where it resolves is made on the copy's handle here).
fn candidate_path(value: &Value, cwd: &Path) -> Option<PathBuf> {
    let text = value.as_str()?.trim();
    if text.is_empty() || text.contains('\0') {
        return None;
    }
    let scheme = text.split_once("://").is_some_and(|(s, _)| {
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    if scheme {
        return None;
    }
    let expanded = match text.strip_prefix("~/") {
        Some(rest) => crate::config::home_dir().join(rest),
        None => PathBuf::from(text),
    };
    Some(if expanded.is_absolute() {
        expanded
    } else {
        normal(&cwd.join(expanded))
    })
}

/// One file a turn made, before it is copied.
#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    path: PathBuf,
    title: String,
    kind: String,
    declared: bool,
    list: Option<String>,
    field: Option<String>,
    call: (i64, i64),
    cwd: PathBuf,
}

/// The files one work_state_write call attached (claude-mem's `declaredCandidates`): any file in a
/// work product field, a known kind's in another.
fn declared(input: &Value, call: (i64, i64), cwd: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Some(fields) = input["fields"].as_object() else {
        return out;
    };
    let list = input["list"].as_str().map(str::to_owned);
    let base = |path: PathBuf, title: String, kind: String, field: &str| Candidate {
        path,
        title,
        kind,
        declared: true,
        list: list.clone(),
        field: Some(field.to_owned()),
        call,
        cwd: cwd.to_owned(),
    };
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    for (field, value) in fields {
        if crate::work_product::is_work_product_field(field) {
            let decoded = match value.as_str() {
                Some(s) if s.trim_start().starts_with(['[', '{']) => {
                    serde_json::from_str(s).unwrap_or_else(|_| value.clone())
                }
                _ => value.clone(),
            };
            let entries = match decoded {
                Value::Array(a) => a,
                other => vec![other],
            };
            for entry in entries {
                let record = if entry.is_object() {
                    entry
                } else {
                    serde_json::json!({ "url": entry })
                };
                // The first of the three that is there and not null, as JavaScript's `??`.
                let named = ["path", "url", "link"]
                    .iter()
                    .find_map(|k| record.get(*k).filter(|v| !v.is_null()));
                let Some(path) = named.and_then(|v| candidate_path(v, cwd)) else {
                    continue;
                };
                let text = |k: &str| {
                    record
                        .get(k)
                        .filter(|v| !v.is_null())
                        .and_then(Value::as_str)
                };
                let title = text("title")
                    .or_else(|| text("name"))
                    .and_then(crate::work_product::short)
                    .unwrap_or_else(|| name(&path));
                let kind = text("kind")
                    .and_then(crate::work_product::short)
                    .unwrap_or_else(|| kind_of(&path.to_string_lossy()));
                out.push(base(path, title, kind, field));
            }
            continue;
        }
        if let Some(path) = candidate_path(value, cwd)
            && crate::work_product::file_kind(&path.to_string_lossy()).is_some()
        {
            let kind = kind_of(&path.to_string_lossy());
            out.push(base(path.clone(), name(&path), kind, field));
        }
    }
    out
}

/// The path a write call names: its input's key, or, for an input cut at the field cap (not JSON
/// any more), the key at its head.
fn written(tool: &str, input: &str) -> Option<String> {
    let key = WRITES.iter().find(|(t, _)| *t == tool)?.1;
    if let Ok(v) = serde_json::from_str::<Value>(input) {
        return v[key].as_str().map(str::to_owned);
    }
    let head = format!("{{\"{key}\":\"");
    let rest = input.strip_prefix(&head)?;
    let mut end = None;
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    serde_json::from_str(&format!("\"{}\"", &rest[..end?])).ok()
}

/// A command run with a time bound, its status and outputs; none when it could not run or took
/// too long.
fn run_bounded(cmd: &mut std::process::Command) -> Option<(Option<i32>, String, String)> {
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < GIT_WAIT => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    Some((
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Whether a written file is outside the source tree: in no repository, or one git ignores; a git
/// that fails or takes too long counts as inside (claude-mem's `isOutsideSourceTree`), each folder
/// asked once a turn.
fn outside_source(path: &Path, seen: &mut HashMap<PathBuf, Option<PathBuf>>) -> bool {
    outside_source_by("git".as_ref(), path, seen)
}

/// `outside_source` with `git` the program run.
fn outside_source_by(
    git: &std::ffi::OsStr,
    path: &Path,
    seen: &mut HashMap<PathBuf, Option<PathBuf>>,
) -> bool {
    let Some(dir) = path.parent().filter(|d| d.is_dir()) else {
        return false;
    };
    let top = seen
        .entry(dir.to_owned())
        .or_insert_with(|| {
            // Its words in English whatever the user's language (gettext reads no `LANGUAGE`
            // in the C locale), since "not a git repository" is read from them.
            let found = run_bounded(
                std::process::Command::new(git)
                    .args(["rev-parse", "--show-toplevel"])
                    .env("LC_ALL", "C")
                    .current_dir(dir),
            );
            match found {
                Some((Some(0), out, _)) if !out.trim().is_empty() => {
                    Some(PathBuf::from(out.trim()))
                }
                // In no repository: outside.
                Some((Some(128), _, err))
                    if err.to_ascii_lowercase().contains("not a git repository") =>
                {
                    None
                }
                // Unknown: inside, so nothing is copied on it.
                _ => Some(PathBuf::new()),
            }
        })
        .clone();
    match top {
        None => true,
        Some(top) if top.as_os_str().is_empty() => false,
        Some(top) => matches!(
            run_bounded(
                std::process::Command::new(git)
                    .args(["check-ignore", "-q", "--"])
                    .arg(path)
                    .current_dir(&top),
            ),
            Some((Some(0), _, _))
        ),
    }
}

/// The real path of an open file, from its handle: a path or a folder above it swapped for a link
/// before or after the open is not followed. None where the system gives no such path.
fn handle_path(file: &std::fs::File) -> std::io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
    }
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: the descriptor is open and the buffer holds PATH_MAX bytes, as F_GETPATH needs.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..end])))
    }
    #[cfg(windows)]
    {
        crate::setup::diagnostic_file_name(file)
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
    {
        let _ = file;
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// The folders a copy may come from: the call's folder and the temp folders, each as it really is,
/// those that exist (claude-mem's `deliverableRoots`).
fn roots(cwd: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in [cwd.to_owned(), std::env::temp_dir(), PathBuf::from("/tmp")] {
        if let Ok(real) = std::fs::canonicalize(&root)
            && !out.contains(&real)
        {
            out.push(real);
        }
    }
    out
}

/// A source opened for reading without blocking (a FIFO put in its place cannot hold the worker),
/// refused unless the handle is a regular file within the bound and inside a root.
fn open_source(path: &Path, roots: &[PathBuf]) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let meta = file.metadata()?;
    anyhow::ensure!(meta.is_file(), "not a regular file");
    anyhow::ensure!(meta.len() <= MAX_BYTES, "over the {MAX_BYTES}-byte bound");
    let real = handle_path(&file).context("where the file is")?;
    anyhow::ensure!(
        roots.iter().any(|root| real.starts_with(root)),
        "outside the checkout and the temp folders"
    );
    Ok(file)
}

/// A folder of `deliverables/` held while a copy or a removal acts in it (#836 item 2). On Linux
/// and macOS by its descriptor: what is made, read, renamed and removed in it is relative to the
/// folder opened, never through a link, so one swapped for a link afterwards is not followed.
/// Elsewhere by its path, checked first as before (a stated limit: Windows).
struct Dir {
    path: PathBuf,
    #[cfg(unix)]
    fd: std::fs::File,
}

/// What a name in a held folder is, read without following a link.
#[derive(Debug, PartialEq)]
enum Entry {
    File,
    Other,
}

/// One plain name in a folder: no separator, not `.` or `..`, no NUL.
fn plain(name: &str) -> std::io::Result<&str> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(name)
}

#[cfg(unix)]
impl Dir {
    const FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;

    /// `<home>/deliverables`, made (0700) when `create` and missing. None when it is not there or
    /// not a real folder (a link); the home itself as given, a link to it allowed.
    fn top(home: &Path, create: bool) -> std::io::Result<Option<Dir>> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let path = home.join(FOLDER);
        if create {
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
                _ => {}
            }
        }
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path);
        match opened {
            Ok(fd) => Ok(Some(Dir { path, fd })),
            Err(e) if not_a_folder(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The folder `name` of this one, made (0700) when `create` and missing. None when it is not
    /// there or not a real folder.
    fn child(&self, name: &str, create: bool) -> std::io::Result<Option<Dir>> {
        use std::os::fd::AsRawFd;
        if create {
            let c = c_name(name)?;
            // SAFETY: the held descriptor is open and `c` is one NUL-terminated name.
            if unsafe { libc::mkdirat(self.fd.as_raw_fd(), c.as_ptr(), 0o700) } < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
            }
        }
        match self.open_at(name, Self::FLAGS, 0) {
            Ok(fd) => Ok(Some(Dir {
                path: self.path.join(name),
                fd,
            })),
            Err(e) if not_a_folder(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// A new file `name` (0600) for writing, never one already there or a link.
    fn create_new(&self, name: &str) -> std::io::Result<std::fs::File> {
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
        self.open_at(name, flags, 0o600)
    }

    /// `name` opened for reading without following a link or blocking (a FIFO holds nothing up).
    fn open_read(&self, name: &str) -> std::io::Result<std::fs::File> {
        self.open_at(
            name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0,
        )
    }

    fn open_at(
        &self,
        name: &str,
        flags: libc::c_int,
        mode: libc::c_uint,
    ) -> std::io::Result<std::fs::File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let c = c_name(name)?;
        // SAFETY: the held descriptor is open and `c` is one NUL-terminated name; the mode is
        // read only with O_CREAT.
        let fd = unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                c.as_ptr(),
                flags | libc::O_CLOEXEC,
                mode,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a new descriptor openat returned, owned here alone.
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    /// What `name` is, its link not followed; None when it is not there.
    fn entry(&self, name: &str) -> std::io::Result<Option<Entry>> {
        use std::os::fd::AsRawFd;
        let c = c_name(name)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the held descriptor is open, `c` is NUL-terminated and `stat` is writable for
        // one stat.
        let status = unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                c.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status < 0 {
            let e = std::io::Error::last_os_error();
            return match e.kind() {
                std::io::ErrorKind::NotFound => Ok(None),
                _ => Err(e),
            };
        }
        // SAFETY: a successful fstatat filled it.
        let stat = unsafe { stat.assume_init() };
        Ok(Some(if stat.st_mode & libc::S_IFMT == libc::S_IFREG {
            Entry::File
        } else {
            Entry::Other
        }))
    }

    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let (from, to) = (c_name(from)?, c_name(to)?);
        let fd = self.fd.as_raw_fd();
        // SAFETY: the held descriptor is open and both names are NUL-terminated.
        if unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Removes `name` itself (a link is removed, never followed).
    fn remove(&self, name: &str) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let c = c_name(name)?;
        // SAFETY: the held descriptor is open and `c` is NUL-terminated.
        if unsafe { libc::unlinkat(self.fd.as_raw_fd(), c.as_ptr(), 0) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// The names in it, read on the held descriptor (setup.rs's reader takes it alone).
    fn names(&mut self) -> std::io::Result<Vec<std::ffi::OsString>> {
        crate::setup::diagnostic_directory_names(&mut self.fd)
    }
}

/// `name` as one NUL-terminated plain name.
#[cfg(unix)]
fn c_name(name: &str) -> std::io::Result<std::ffi::CString> {
    std::ffi::CString::new(plain(name)?).map_err(|_| std::io::ErrorKind::InvalidInput.into())
}

/// A folder open under `O_NOFOLLOW` that is not there or not a real folder: a link fails with
/// `ELOOP`, a file with `ENOTDIR` (Linux and macOS alike).
#[cfg(unix)]
fn not_a_folder(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound
        || matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR))
}

#[cfg(not(unix))]
impl Dir {
    fn top(home: &Path, create: bool) -> std::io::Result<Option<Dir>> {
        Self::at(home.join(FOLDER), create)
    }

    fn child(&self, name: &str, create: bool) -> std::io::Result<Option<Dir>> {
        Self::at(self.path.join(plain(name)?), create)
    }

    /// `path` as a real folder, made when `create` and missing; None when it is not one.
    fn at(path: PathBuf, create: bool) -> std::io::Result<Option<Dir>> {
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => Ok(Some(Dir { path })),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                std::fs::create_dir(&path)?;
                Ok(Some(Dir { path }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn create_new(&self, name: &str) -> std::io::Result<std::fs::File> {
        let path = self.path.join(plain(name)?);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    }

    fn open_read(&self, name: &str) -> std::io::Result<std::fs::File> {
        std::fs::File::open(self.path.join(plain(name)?))
    }

    fn entry(&self, name: &str) -> std::io::Result<Option<Entry>> {
        match std::fs::symlink_metadata(self.path.join(plain(name)?)) {
            Ok(m) if m.is_file() => Ok(Some(Entry::File)),
            Ok(_) => Ok(Some(Entry::Other)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        std::fs::rename(self.path.join(plain(from)?), self.path.join(plain(to)?))
    }

    fn remove(&self, name: &str) -> std::io::Result<()> {
        std::fs::remove_file(self.path.join(plain(name)?))
    }

    fn names(&mut self) -> std::io::Result<Vec<std::ffi::OsString>> {
        std::fs::read_dir(&self.path)?
            .map(|e| e.map(|e| e.file_name()))
            .collect()
    }
}

/// `deliverables/<folder>`, made where missing, held (#836 item 2).
fn held_folder(home: &Path, folder: &str) -> Result<Dir> {
    let not_real = |path: PathBuf| anyhow::anyhow!("{} is not a real folder", path.display());
    let top = Dir::top(home, true)?.ok_or_else(|| not_real(home.join(FOLDER)))?;
    #[cfg(test)]
    seam(Stage::Top);
    top.child(folder, true)?
        .ok_or_else(|| not_real(top.path.join(folder)))
}

/// The sha256 of an existing copy `name` of `len` bytes in a held folder, opened without blocking
/// or following a link and refused unless a regular file, read to one byte past `len` at most: a
/// FIFO, a link or a file of any other size put under a copy's name holds nothing up (None for
/// another size).
fn sha256_held(dir: &Dir, name: &str, len: u64) -> Result<Option<String>> {
    let file = dir.open_read(name)?;
    anyhow::ensure!(file.metadata()?.is_file(), "not a regular file");
    let mut hash = Sha256::new();
    let read = std::io::copy(&mut file.take(len + 1), &mut hash)?;
    Ok((read == len).then(|| hex(&hash.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A copy made: its path under `deliverables/`, bytes and sha256.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Copied {
    pub copy: String,
    pub bytes: u64,
    pub sha256: String,
}

/// Copies `file` (opened by `open_source`) into `deliverables/<folder>/`: staged under a new name,
/// hashed as written, then named by its content; an existing name is reused only for the same
/// whole hash, else the copy takes its whole hash (claude-mem's `copyDeliverableIntoBucket`).
fn copy_into(home: &Path, folder: &str, source: &Path, mut file: std::fs::File) -> Result<Copied> {
    let dir = held_folder(home, folder)?;
    #[cfg(test)]
    seam(Stage::Folder);
    let mut nonce = [0u8; 6];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("{e}"))?;
    let partial = format!(".partial-{}-{}", std::process::id(), hex(&nonce));
    let result = (|| -> Result<Copied> {
        // A copy is the owner's alone, whatever its source's mode (0600 where it is made).
        let mut out = dir.create_new(&partial)?;
        #[cfg(test)]
        seam(Stage::Partial);
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            anyhow::ensure!(bytes <= MAX_BYTES, "grew over the {MAX_BYTES}-byte bound");
            hash.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
        out.sync_all()?;
        let sha256 = hex(&hash.finalize());
        for name in [
            copy_name(source, &sha256[..8])?,
            copy_name(source, &sha256)?,
        ] {
            let copied = || Copied {
                copy: format!("{folder}/{name}"),
                bytes,
                sha256: sha256.clone(),
            };
            match dir.entry(&name)? {
                None => {
                    dir.rename(&partial, &name)?;
                    return Ok(copied());
                }
                Some(Entry::File) if sha256_held(&dir, &name, bytes)?.as_ref() == Some(&sha256) => {
                    dir.remove(&partial)?;
                    return Ok(copied());
                }
                // Another file under the short name: the whole hash's.
                Some(_) => {}
            }
        }
        anyhow::bail!("both names are taken by other files")
    })();
    if result.is_err() {
        let _ = dir.remove(&partial);
    }
    result
}

/// The phase's place on a device: the last record it read (a turn end, or one it read past), that
/// record's time, and raw.db's file then (#836 item 3).
#[derive(Debug)]
struct Checkpoint {
    seq: i64,
    ts: Option<i64>,
    file: Option<String>,
}

fn checkpoint(db: &Connection, device: &str) -> Result<Option<Checkpoint>> {
    Ok(db
        .query_row(
            "SELECT seq, ts, file FROM deliverable_scan WHERE device = ?1",
            [device],
            |r| {
                Ok(Checkpoint {
                    seq: r.get(0)?,
                    ts: r.get(1)?,
                    file: r.get(2)?,
                })
            },
        )
        .optional()?)
}

/// The checkpoint of `raw`'s device at `seq`, its record's time `ts`, in raw.db's file as `raw`
/// has it open.
fn set_checkpoint(db: &Connection, raw: &Raw, seq: i64, ts: Option<i64>) -> Result<()> {
    db.execute(
        "INSERT INTO deliverable_scan(device, seq, ts, file) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(device) DO UPDATE SET seq = excluded.seq, ts = excluded.ts,
           file = excluded.file",
        params![raw.device(), seq, ts, raw.file_identity()],
    )?;
    Ok(())
}

/// The time of the record at `seq`, none for one with no labels (removed, a tombstone) or none.
fn time_at(raw: &Raw, seq: i64) -> Result<Option<i64>> {
    Ok(raw
        .after(raw.device(), seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == seq)
        .and_then(|r| match r.item {
            Item::Event(e) | Item::Expired(e) => Some(e.ts),
            _ => None,
        }))
}

/// Whether raw.db still holds the record the checkpoint names: not after a restore that ends below
/// it or gives its seq to another record. A record with a time is known by it. One with none now (a
/// tombstone, a removed one, also one a forget made so since) is known by raw.db's file as well,
/// which a restore replaces (#836 item 3); a checkpoint kept before the file was, or a raw.db with
/// no file identity, is taken as it is. The empty store's checkpoint (seq 0) names none.
fn still_there(raw: &Raw, c: &Checkpoint) -> Result<bool> {
    if c.seq == 0 {
        return Ok(true);
    }
    let at = raw
        .after(raw.device(), c.seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == c.seq);
    let same_file = match (c.file.as_deref(), raw.file_identity()) {
        (Some(then), Some(now)) => then == now,
        _ => true,
    };
    Ok(match (c.ts, at.map(|r| r.item)) {
        (_, None) => false,
        (Some(ts), Some(Item::Event(e) | Item::Expired(e))) => e.ts == ts,
        (None, Some(Item::Event(_) | Item::Expired(_))) => false,
        (_, Some(_)) => same_file,
    })
}

/// The work_state_write tool of oboete, under the name any agent gives it.
fn is_work_state_write(tool: &str) -> bool {
    crate::curate::served(tool)
        .is_some_and(|(server, name)| name == "work_state_write" && crate::curate::oboete(server))
}

/// A turn's files, declared first, then those written outside the source tree (claude-mem's
/// capture order). A call counts only after the record that started its turn, a prompt or the
/// harness's message (`envelope`: a task's notification, another session's message), which
/// claude-mem reads as a turn of its own too; and only when the hooks marked it as of a public
/// prompt's turn: a wholly private prompt is recorded as nothing, and a mark a rule masked stops
/// the copy rather than allows it. The events come one at a time and only what a copy needs is
/// kept of each, so a long turn is never held whole. `prompted`: a subagent's calls in a gap,
/// whose launch's turn had its prompt (`stop_calls`).
fn candidates(
    events: impl IntoIterator<Item = Result<(i64, Event)>>,
    mut prompted: bool,
    aside: &dyn Fn() -> bool,
) -> Result<Option<Vec<Candidate>>> {
    let mut declared_list: Vec<Candidate> = Vec::new();
    let mut writes: Vec<(PathBuf, (i64, i64), PathBuf)> = Vec::new();
    for event in events {
        let (seq, e) = event?;
        prompted |= matches!(e.kind.as_str(), "prompt" | "envelope");
        if e.kind != "tool" || !prompted {
            continue;
        }
        let Ok(body) = serde_json::from_str::<Value>(&e.body) else {
            continue;
        };
        // A call of a turn not seen public, and a call that failed, name nothing.
        if body["public_turn"] != true || body["failed"] == true {
            continue;
        }
        let tool = body["tool"].as_str().unwrap_or("");
        let input = body["input"].as_str().unwrap_or("");
        // A call with no folder of its own has no root to hold its paths to (claude-mem's roots
        // start at it): it names nothing, rather than taking the whole disk for its folder.
        let Some(cwd) = e
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .filter(|c| c.is_absolute())
        else {
            continue;
        };
        let call = (seq, e.ts);
        if is_work_state_write(tool) {
            if let Ok(input) = serde_json::from_str::<Value>(input) {
                // Grok's hooks give an MCP call's input as {"tool_name", "tool_input"}.
                let input = if input["tool_input"].is_object() {
                    &input["tool_input"]
                } else {
                    &input
                };
                for c in declared(input, call, &cwd) {
                    if !declared_list.iter().any(|d| d.path == c.path) {
                        declared_list.push(c);
                    }
                }
            }
        } else if let Some(path) =
            written(tool, input).and_then(|p| candidate_path(&Value::String(p), &cwd))
        {
            match writes.iter_mut().find(|(p, _, _)| *p == path) {
                // One a path: the last write's call, at the first write's place.
                Some(w) => w.1 = call,
                None => writes.push((path, call, cwd)),
            }
        }
    }
    let mut out = declared_list;
    let mut seen: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    for (path, call, cwd) in writes {
        if out.iter().any(|c| c.path == path) {
            continue;
        }
        let text = path.to_string_lossy().into_owned();
        if crate::work_product::file_kind(&text).is_none() {
            continue;
        }
        // Each git check takes up to 5 s: a command waiting for the worker comes first.
        if aside() {
            return Ok(None);
        }
        if !outside_source(&path, &mut seen) {
            continue;
        }
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(Candidate {
            kind: kind_of(&text),
            path,
            title,
            declared: false,
            list: None,
            field: None,
            call,
            cwd,
        });
    }
    Ok(Some(out))
}

/// The phase (docs/deliverables.md): each turn end recorded on this device since its checkpoint,
/// copied from, and each Claude Code subagent's stop (docs/subagents.md, slice 3), whose facts it
/// reads in `k`, knowledge.db. True when it took one; it stops when a command asks the worker to
/// step aside, the turn then taken again whole (names are by content and rows unique), and before
/// a stop the `subagents` consumer has not reached. `db` is opened when first needed.
pub(crate) fn run(
    home: &Path,
    raw: &Raw,
    k: &Connection,
    db: &mut Option<Connection>,
    aside: &dyn Fn() -> bool,
) -> Result<bool> {
    if !crate::config::load_capture(home).is_ok_and(|c| c.capture.deliverables) {
        return Ok(false);
    }
    let db = match db {
        Some(db) => db,
        None => db.insert(open(home)?),
    };
    let device = raw.device().to_owned();
    let newest = raw.max_seq()?;
    let from = match checkpoint(db, &device)? {
        Some(c) if still_there(raw, &c)? => {
            // One kept before raw.db's file was takes the file now, once, also when no record
            // follows it (#836 item 3).
            if c.file.is_none() && raw.file_identity().is_some() {
                set_checkpoint(db, raw, c.seq, c.ts)?;
            }
            c.seq
        }
        // The first run, or a raw.db restored below the checkpoint: nothing older is copied, and
        // the stops taken before name seqs other records may hold now.
        _ => {
            db.execute("DELETE FROM subagent_scan WHERE device = ?1", [&device])?;
            set_checkpoint(db, raw, newest, time_at(raw, newest)?)?;
            return Ok(false);
        }
    };
    let records = raw.after(&device, from, BATCH)?;
    let Some(last) = records.last().map(|r| r.seq) else {
        return Ok(false);
    };
    let mut took = false;
    for r in records {
        let Item::Event(e) = &r.item else { continue };
        if e.source != "hook" || !AGENTS.contains(&e.agent.as_str()) {
            continue;
        }
        // Each file found with the turn its row is of; a stop's subagent and its launch.
        let (found, stopped): (Vec<(Candidate, (i64, i64))>, _) = match e.kind.as_str() {
            "reply" | "end" => {
                let start = raw.hook_turn_start(&e.agent, &e.session, r.seq)?;
                let seqs = raw.turn_seqs(&e.agent, &e.session, start - 1, r.seq)?;
                let events = (seqs.into_iter())
                    .filter_map(|seq| raw.event_at(seq).transpose().map(|e| e.map(|e| (seq, e))));
                let Some(found) = candidates(events, false, aside)? else {
                    return Ok(true);
                };
                (
                    found.into_iter().map(|c| (c, (r.seq, e.ts))).collect(),
                    None,
                )
            }
            crate::raw::SUBAGENT_STOP => {
                // One with no id to know its subagent by (none, masked, not Claude Code's) is read
                // past.
                let Some(stop) = crate::consumer::subagents::facts(e).into_iter().next() else {
                    continue;
                };
                let (launch, calls) =
                    match stop_calls(raw, k, db, (r.seq, e), &stop.agent_id, aside)? {
                        // A later round takes it, once the consumer has: the checkpoint stays below.
                        Stop::Wait => return Ok(took),
                        Stop::Aside => return Ok(true),
                        Stop::Unseen => (0, Vec::new()),
                        Stop::Calls(launch, calls) => (launch, calls),
                    };
                let turns: HashMap<i64, (i64, i64)> = calls.iter().copied().collect();
                // One at a time, as a turn's: a long run of calls is never held whole.
                let events = calls
                    .iter()
                    .filter_map(|&(seq, _)| match raw.event_at(seq) {
                        Ok(Some(call)) if call.source == "hook" => Some(Ok((seq, call))),
                        Ok(_) => None,
                        Err(err) => Some(Err(err)),
                    });
                let Some(found) = candidates(events, true, aside)? else {
                    return Ok(true);
                };
                let found = (found.into_iter())
                    .filter_map(|c| {
                        let turn = *turns.get(&c.call.0)?;
                        Some((c, turn))
                    })
                    .collect();
                (found, Some((stop.agent_id, launch)))
            }
            _ => continue,
        };
        let folder = folder_name(
            e.repo
                .as_deref()
                .and_then(|k| k.rsplit(['/', '\\']).find(|p| !p.is_empty()))
                .unwrap_or(""),
        );
        // At most `MAX_FILES` a turn or a stop, of those that open as a file inside a root, as
        // claude-mem counts the ones that resolve.
        let mut opened = 0;
        for (c, turn) in found {
            if opened == MAX_FILES {
                eprintln!(
                    "oboete: over {MAX_FILES} files for one turn: {} not copied",
                    c.path.display()
                );
                continue;
            }
            // Taken again whole next round, from the last turn end done.
            if aside() {
                return Ok(true);
            }
            let copied = open_source(&c.path, &roots(&c.cwd)).and_then(|file| {
                opened += 1;
                copy_into(home, &folder, &c.path, file)
            });
            let copied = match copied {
                Ok(copied) => copied,
                Err(err) => {
                    eprintln!("oboete: {} was not copied: {err:#}", c.path.display());
                    continue;
                }
            };
            // A stop's row under the read fence, so no forget registers between the look and the
            // row: its launch still seen public (a forget of it denies no row, the launch being no
            // origin of one), and neither its call nor its gap's reply forgotten. A copy left with
            // no row is the idle sweep's.
            #[cfg(test)]
            seam(Stage::Row);
            let _read = match &stopped {
                Some((_, launch)) => {
                    let read = crate::dispatch::reading(home, crate::db::OPEN_WRITE_WAIT)?;
                    let forgets = |(seq, ts): (i64, i64)| {
                        raw.forgets(&device, &e.agent, &e.session, e.repo.as_deref(), seq, ts)
                    };
                    if !launched_public(raw, &e.agent, &e.session, *launch)?
                        || forgets(c.call)?
                        || forgets(turn)?
                    {
                        continue;
                    }
                    Some(read)
                }
                None => None,
            };
            db.execute(
                "INSERT OR IGNORE INTO deliverables(device, agent, session, repo, turn_seq, turn_ts,
                   call_seq, call_ts, title, kind, source_path, copy, bytes, sha256, declared, list,
                   field, copied)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                   ?18)",
                params![
                    device,
                    e.agent,
                    e.session,
                    e.repo,
                    turn.0,
                    turn.1,
                    c.call.0,
                    c.call.1,
                    c.title,
                    c.kind,
                    c.path.to_string_lossy(),
                    copied.copy,
                    copied.bytes as i64,
                    copied.sha256,
                    c.declared,
                    c.list,
                    c.field,
                    crate::db::now_ms(),
                ],
            )?;
        }
        // A stop's seq kept with the checkpoint, in one transaction: a restore that keeps the
        // checkpoint's record keeps no seq past it.
        let tx = db.transaction()?;
        if let Some((agent_id, _)) = stopped {
            tx.execute(
                "INSERT INTO subagent_scan(device, session, agent_id, seq) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(device, session, agent_id) DO UPDATE SET seq = excluded.seq",
                params![device, e.session, agent_id, r.seq],
            )?;
        }
        set_checkpoint(&tx, raw, r.seq, Some(e.ts))?;
        tx.commit()?;
        took = true;
    }
    let seq = checkpoint(db, &device)?.map_or(from, |c| c.seq);
    if seq < last {
        set_checkpoint(db, raw, last, time_at(raw, last)?)?;
    }
    // Records read past is progress too: more may wait behind them.
    Ok(took || last > from)
}

/// What the phase does at a subagent's stop (docs/subagents.md, slice 3).
enum Stop {
    /// The `subagents` consumer has not reached it: a later round takes it.
    Wait,
    /// A command asks the worker to step aside.
    Aside,
    /// Not launched from a turn the hooks saw public: it copies nothing.
    Unseen,
    /// Its launch's seq, and its calls in a gap since its previous stop, each with the reply its
    /// gap follows (seq and time).
    Calls(i64, Vec<(i64, (i64, i64))>),
}

/// A subagent's stop, read from the `subagents` consumer's facts and raw's labels: the calls are
/// read one at a time by the caller, so a long run of them is never held whole.
fn stop_calls(
    raw: &Raw,
    k: &Connection,
    db: &Connection,
    (stop, e): (i64, &Event),
    agent_id: &str,
    aside: &dyn Fn() -> bool,
) -> Result<Stop> {
    let device = raw.device();
    if !crate::consumer::subagents::reached(k, device, stop)? {
        return Ok(Stop::Wait);
    }
    let who = (e.agent.as_str(), e.session.as_str(), agent_id);
    let facts = |fact: &str, after: i64| {
        crate::consumer::subagents::fact_seqs(k, device, who, fact, (after, stop))
    };
    // Its launch, the newest.
    let launch = match facts("launch", 0)?.last() {
        Some(&seq) if launched_public(raw, &e.agent, &e.session, seq)? => seq,
        _ => return Ok(Stop::Unseen),
    };
    // After the newest stop taken, or the newest the facts hold that raw reads as a hook's (the
    // facts keep no source).
    let taken: i64 = db
        .query_row(
            "SELECT seq FROM subagent_scan WHERE device = ?1 AND session = ?2 AND agent_id = ?3",
            params![device, e.session, agent_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let mut since = taken;
    for seq in facts("stop", taken)?.into_iter().rev() {
        if raw.event_at(seq)?.is_some_and(|x| x.source == "hook") {
            since = seq;
            break;
        }
    }
    // Each step's gap, looked up again only past a record that may end one: a gap of many steps
    // is read once.
    let (mut out, mut gap, mut last) = (Vec::new(), None, None);
    for (n, seq) in facts("step", since)?.into_iter().enumerate() {
        if n % 256 == 255 && aside() {
            return Ok(Stop::Aside);
        }
        let bound = match last {
            Some(last) => raw.gap_bound_between(&e.agent, &e.session, last, seq)?,
            None => true,
        };
        if bound {
            gap = match raw.gap_reply(&e.agent, &e.session, seq)? {
                Some(reply) => {
                    let (reply_seq, reply_ts) = reply;
                    let forgotten = raw.forgets(
                        device,
                        &e.agent,
                        &e.session,
                        e.repo.as_deref(),
                        reply_seq,
                        reply_ts,
                    )?;
                    (!forgotten).then_some(reply)
                }
                None => None,
            };
        }
        last = Some(seq);
        if let Some(reply) = gap {
            out.push((seq, reply));
        }
    }
    Ok(Stop::Calls(launch, out))
}

/// Whether the call at `seq` that launched a subagent is seen public now: a hook's record no forget
/// denies, marked public, after a prompt or a harness message of its own turn.
fn launched_public(raw: &Raw, agent: &str, session: &str, seq: i64) -> Result<bool> {
    let Some(launch) = raw.event_at(seq)?.filter(|x| x.source == "hook") else {
        return Ok(false);
    };
    let marked =
        serde_json::from_str::<Value>(&launch.body).is_ok_and(|b| b["public_turn"] == true);
    let device = raw.device();
    let repo = launch.repo.as_deref();
    Ok(marked
        && !raw.forgets(device, agent, session, repo, seq, launch.ts)?
        && prompted_before(raw, agent, session, seq)?)
}

/// Whether a prompt or a harness message of the session reads before `seq` in its own turn, as a
/// turn's copy asks of a call (`candidates`).
fn prompted_before(raw: &Raw, agent: &str, session: &str, seq: i64) -> Result<bool> {
    let start = raw.hook_turn_start(agent, session, seq)?;
    for s in raw.turn_seqs(agent, session, start - 1, seq)? {
        let read = raw.event_at(s)?;
        if read.is_some_and(|x| matches!(x.kind.as_str(), "prompt" | "envelope")) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The rows forget's denials cover (docs/deliverables.md): the origin of the call or the turn end
/// in `denied_records`, or a `denied_scopes` page of their session, repository or time range.
fn forgotten(raw: &Raw, db: &Connection) -> Result<Vec<(i64, String)>> {
    let mut out = Vec::new();
    let mut rows = db.prepare(
        "SELECT id, device, agent, session, repo, turn_seq, turn_ts, call_seq, call_ts, copy
         FROM deliverables",
    )?;
    let rows = rows
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                (r.get::<_, i64>(5)?, r.get::<_, i64>(6)?),
                (r.get::<_, i64>(7)?, r.get::<_, i64>(8)?),
                r.get::<_, String>(9)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, device, agent, session, repo, turn, call, copy) in rows {
        let mut gone = false;
        for (seq, ts) in [turn, call] {
            gone = gone || raw.forgets(&device, &agent, &session, repo.as_deref(), seq, ts)?;
        }
        if gone {
            out.push((id, copy));
        }
    }
    Ok(out)
}

/// Removes the copies forget's denials cover: each row's file when no other row names it, then
/// the row (a file it cannot remove keeps its row for the next sweep).
/// After forget's purge and at the worker's idle time; `orphans` also removes the files no row
/// names at all, only where no copy is being made (the worker's own idle step).
pub(crate) fn sweep(home: &Path, raw: &Raw, orphans: bool) -> Result<usize> {
    if !home.join(DB).exists() {
        return Ok(0);
    }
    let db = open(home)?;
    let gone = forgotten(raw, &db)?;
    let (mut removed, mut kept) = (0, 0);
    for (id, copy) in &gone {
        // The file before its row: a file that cannot be removed keeps its row for the next sweep,
        // and the others go on.
        let named: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM deliverables WHERE copy = ?1 AND id <> ?2)",
            params![copy, id],
            |r| r.get(0),
        )?;
        if !named {
            match remove_copy(home, copy) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(e) => {
                    eprintln!("oboete: the copy {copy} was not removed yet: {e:#}");
                    kept += 1;
                    continue;
                }
            }
        }
        db.execute("DELETE FROM deliverables WHERE id = ?1", [id])?;
    }
    // A forgotten copy still there keeps forget's last step owed (`forget --continue`).
    anyhow::ensure!(
        kept == 0,
        "{kept} forgotten copies could not be removed yet"
    );
    if orphans {
        let named: HashSet<String> = db
            .prepare("SELECT copy FROM deliverables")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        // Only a real folder is read, each held while its files are removed (#836 item 2): a
        // link at `deliverables/` or in it leads elsewhere.
        let Ok(Some(mut top)) = Dir::top(home, false) else {
            return Ok(removed);
        };
        let Ok(folders) = top.names() else {
            return Ok(removed);
        };
        for name in folders {
            let name = name.to_string_lossy();
            // A folder that cannot be read is said and passed over: the others are still swept.
            let not_swept = |e: std::io::Error| eprintln!("oboete: {name} was not swept: {e:#}");
            let mut dir = match top.child(&name, false) {
                Ok(Some(dir)) => dir,
                Ok(None) => continue,
                Err(e) => {
                    not_swept(e);
                    continue;
                }
            };
            #[cfg(test)]
            seam(Stage::Folder);
            let files = match dir.names() {
                Ok(files) => files,
                Err(e) => {
                    not_swept(e);
                    continue;
                }
            };
            for file in files {
                let file = file.to_string_lossy();
                let copy = format!("{name}/{file}");
                if named.contains(&copy) {
                    continue;
                }
                match remove_held(&dir, &file) {
                    Ok(true) => removed += 1,
                    Ok(false) => {}
                    Err(e) => eprintln!("oboete: {copy} was not removed: {e:#}"),
                }
            }
        }
    }
    Ok(removed)
}

/// A copy's folder and file name, each one plain part: anything else is no copy's name.
fn copy_parts(copy: &str) -> Option<(&str, &str)> {
    copy.split_once('/').filter(|(folder, name)| {
        [folder, name]
            .iter()
            .all(|p| !p.is_empty() && **p != "." && **p != ".." && !p.contains(['/', '\\']))
    })
}

/// Removes one copy, only a regular file directly in a folder of `deliverables/`, no link
/// followed, the folder held while it is removed (#836 item 2).
fn remove_copy(home: &Path, copy: &str) -> Result<bool> {
    // A name no copy is given is refused, never taken as gone: its row stays.
    let Some((folder, name)) = copy_parts(copy) else {
        anyhow::bail!("not a copy's name");
    };
    // A folder or a file that cannot be read is an error, so its row stays for the next sweep;
    // one that is not there, or not a real folder, is gone already.
    let Some(top) = Dir::top(home, false)? else {
        return Ok(false);
    };
    let Some(dir) = top.child(folder, false)? else {
        return Ok(false);
    };
    #[cfg(test)]
    seam(Stage::Folder);
    remove_held(&dir, name)
}

/// Removes `name` from a held folder when it is a regular file there: whether it did.
fn remove_held(dir: &Dir, name: &str) -> Result<bool> {
    match dir.entry(name)? {
        Some(Entry::File) => {
            dir.remove(name)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Whether the rule `agent` reads at session start says its files are copied (row G01's second
/// line): an agent copied from, while `[capture] deliverables` is on. Settings that do not read
/// say no.
pub(crate) fn copies_for(home: &Path, agent: &str) -> bool {
    AGENTS.contains(&agent)
        && crate::config::load_capture(home).is_ok_and(|c| c.capture.deliverables)
}

/// A copy's row as the viewer reads it to serve or list it (docs/deliverables.md, slice 2).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub id: i64,
    pub device: String,
    pub agent: String,
    pub session: String,
    pub repo: Option<String>,
    pub turn_seq: i64,
    pub turn_ts: i64,
    pub call_seq: i64,
    pub call_ts: i64,
    pub title: String,
    pub kind: String,
    pub source_path: String,
    pub copy: String,
    pub bytes: u64,
    pub sha256: String,
    pub copied: i64,
}

const ROW: &str = "SELECT id, device, agent, session, repo, turn_seq, turn_ts, call_seq, call_ts,
    title, kind, source_path, copy, bytes, sha256, copied FROM deliverables";

fn row_of(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(0)?,
        device: r.get(1)?,
        agent: r.get(2)?,
        session: r.get(3)?,
        repo: r.get(4)?,
        turn_seq: r.get(5)?,
        turn_ts: r.get(6)?,
        call_seq: r.get(7)?,
        call_ts: r.get(8)?,
        title: r.get(9)?,
        kind: r.get(10)?,
        source_path: r.get(11)?,
        copy: r.get(12)?,
        bytes: r.get::<_, i64>(13)?.max(0) as u64,
        sha256: r.get(14)?,
        copied: r.get(15)?,
    })
}

/// deliverables.db opened to read, none when it was never made: a reader neither makes nor
/// changes it.
pub(crate) fn open_read(home: &Path) -> Result<Option<Connection>> {
    let path = home.join(DB);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(m) => anyhow::ensure!(m.is_file(), "deliverables.db is not a regular file"),
    }
    // SQLite's NOFOLLOW refuses a link anywhere in the path: the home is resolved and the leaf is
    // not, as `raw::read_only` opens raw.db.
    let db = Connection::open_with_flags(
        home.canonicalize()?.join(DB),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(Some(db))
}

/// Every row, by id.
pub(crate) fn rows(db: &Connection) -> Result<Vec<Row>> {
    let mut q = db.prepare(&format!("{ROW} ORDER BY id"))?;
    let rows = q.query_map([], row_of)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The rows of one conversation, by id.
pub(crate) fn rows_of(db: &Connection, agent: &str, session: &str) -> Result<Vec<Row>> {
    let mut q = db.prepare_cached(&format!(
        "{ROW} WHERE agent = ?1 AND session = ?2 ORDER BY id"
    ))?;
    let rows = q.query_map([agent, session], row_of)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A row's provenance, each field prefixed by its length: what its link's tag is made of, and
/// what the page's fingerprints of copies compare. A row id a sweep frees and another copy takes
/// gives another provenance unless the turn, the call and the name are the same too.
pub(crate) fn provenance(r: &Row) -> Vec<u8> {
    let mut out = Vec::new();
    let mut put = |b: &[u8]| {
        out.extend_from_slice(&(b.len() as u64).to_be_bytes());
        out.extend_from_slice(b);
    };
    put(r.device.as_bytes());
    for n in [r.id, r.turn_seq, r.turn_ts, r.call_seq, r.call_ts] {
        put(&n.to_be_bytes());
    }
    put(r.sha256.as_bytes());
    put(r.copy.as_bytes());
    out
}

/// A short hash of these rows' provenance, in their order: what changes when a copy is made or
/// swept, even one that takes a swept row's id.
pub(crate) fn fingerprint<'a>(rows: impl IntoIterator<Item = &'a Row>) -> String {
    let mut hash = Sha256::new();
    for r in rows {
        let p = provenance(r);
        hash.update((p.len() as u64).to_be_bytes());
        hash.update(&p);
    }
    hex(&hash.finalize()[..8])
}

/// Whether forget's denials cover the row's call or the end of its turn, on any connection to
/// raw.db (`raw::forgets_in`).
pub(crate) fn denied(raw: &Connection, r: &Row) -> Result<bool> {
    denied_by(r, |seq, ts| {
        crate::raw::forgets_in(
            raw,
            &r.device,
            &r.agent,
            &r.session,
            r.repo.as_deref(),
            seq,
            ts,
        )
    })
}

/// `denied`, `forgets` asked of the call, then of the turn's end (a copy's answer sets its wait
/// before each).
pub(crate) fn denied_by(
    r: &Row,
    mut forgets: impl FnMut(i64, i64) -> Result<bool>,
) -> Result<bool> {
    Ok(forgets(r.call_seq, r.call_ts)? || forgets(r.turn_seq, r.turn_ts)?)
}

/// Whether a copy is on disk as its row says, as far as a list looks (claude-mem's
/// `bucketFileAvailable`): its file directly in a folder of `<home>/deliverables/`, opened without
/// following a link or blocking, a regular file of the row's size whose handle is at that very path
/// (the home canonicalized, then the fixed folder), as `open_copy` opens it: a folder above it
/// swapped for a link, whenever, is not followed. Serving reads its bytes too.
pub(crate) fn on_disk(home: &Path, r: &Row) -> bool {
    let Some((folder, name)) = copy_parts(&r.copy) else {
        return false;
    };
    let at = |home: PathBuf| -> std::io::Result<bool> {
        let path = home.join(FOLDER).join(folder).join(name);
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(&path)?;
        let meta = file.metadata()?;
        Ok(meta.is_file() && meta.len() == r.bytes && handle_path(&file)? == path)
    };
    home.canonicalize().and_then(at).unwrap_or(false)
}

/// A copy opened to serve: its folder and file directly in `<home>/deliverables/`, each a real
/// folder, the file opened without following a link or blocking, a regular file whose handle is
/// at that very path (the home canonicalized once, then the fixed folder: a `deliverables/`
/// swapped for a link before the open is refused), whose bytes, read on the handle within
/// `deadline` and no further than the row's size, are the row's: its size and its sha256. The
/// handle comes back at its start. None when the copy is not as its row says (gone, a link or not
/// a regular file in its place, other bytes); an error when it could not be read in time or at all.
pub(crate) fn open_copy(
    home: &Path,
    r: &Row,
    deadline: std::time::Instant,
) -> Result<Option<std::fs::File>> {
    use std::io::Seek;
    let Some((folder, name)) = copy_parts(&r.copy) else {
        return Ok(None);
    };
    let gone = |e: &std::io::Error| {
        // A link in the file's place fails the open with ELOOP under O_NOFOLLOW.
        #[cfg(unix)]
        let link = e.raw_os_error() == Some(libc::ELOOP);
        #[cfg(not(unix))]
        let link = false;
        e.kind() == std::io::ErrorKind::NotFound || link
    };
    let dir = home.canonicalize()?.join(FOLDER).join(folder);
    for d in [dir.parent().context("deliverables")?, &dir] {
        match std::fs::symlink_metadata(d) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Ok(None),
            Err(e) if gone(&e) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    let path = dir.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if gone(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() || handle_path(&file)? != path {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut read = 0u64;
    loop {
        anyhow::ensure!(std::time::Instant::now() < deadline, "out of time");
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        read += n as u64;
        if read > r.bytes {
            return Ok(None);
        }
        hash.update(&buf[..n]);
    }
    if read != r.bytes || hex(&hash.finalize()) != r.sha256 {
        return Ok(None);
    }
    file.seek(std::io::SeekFrom::Start(0))?;
    Ok(Some(file))
}

/// Where a test acts while a copy or a removal holds `deliverables/` (#836 item 2).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    /// `deliverables/` held, its folder not yet made or opened.
    Top,
    /// The folder held.
    Folder,
    /// A copy's partial file made in it.
    Partial,
    /// A stop's copy made, its row not yet written.
    Row,
}

/// A test's act at each stage.
#[cfg(test)]
type Act = Box<dyn FnMut(Stage)>;

#[cfg(test)]
thread_local! {
    static SEAM: std::cell::RefCell<Option<Act>> = const { std::cell::RefCell::new(None) };
}

/// The test's act at `stage`, on this thread.
#[cfg(test)]
fn seam(stage: Stage) {
    SEAM.with(|s| {
        if let Some(act) = s.borrow_mut().as_mut() {
            act(stage);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(session: &str, kind: &str, body: Value, cwd: &Path) -> Event {
        Event {
            agent: "claude".into(),
            session: session.into(),
            kind: kind.into(),
            ts: crate::db::now_ms(),
            repo: Some("github.com/example/shop".into()),
            branch: None,
            head: None,
            gitdir: None,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            source: "hook".into(),
            body: body.to_string(),
            original_bytes: None,
        }
    }

    /// A call as the hooks record one in a public prompt's turn (`mark_public`).
    fn tool(session: &str, name: &str, input: Value, cwd: &Path) -> Event {
        let body = json!({"tool": name, "input": input.to_string(), "output": "", "failed": false});
        let mut e = ev(session, "tool", body, cwd);
        mark_public(&mut e);
        e
    }

    /// A turn of `session`: its prompt, its calls and its reply.
    fn turn(raw: &mut Raw, session: &str, cwd: &Path, calls: &[(&str, Value)]) {
        raw.append(&ev(session, "prompt", json!({"prompt": "Make it."}), cwd))
            .unwrap();
        for (name, input) in calls {
            raw.append(&tool(session, name, input.clone(), cwd))
                .unwrap();
        }
        raw.append(&ev(session, "reply", json!({"assistant": "Done."}), cwd))
            .unwrap();
    }

    /// A store whose phase has had its first run: its checkpoint at the newest record.
    fn ready() -> (tempfile::TempDir, Raw) {
        let home = tempfile::tempdir().unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        assert!(!step(home.path(), &raw));
        (home, raw)
    }

    /// knowledge.db with the `subagents` consumer drained, as the worker's round has it.
    fn drained(home: &Path, raw: &Raw) -> Connection {
        let mut consumers: Vec<Box<dyn crate::worker::Consumer>> =
            vec![Box::new(crate::consumer::subagents::Subagents)];
        let mut k = crate::knowledge::open(home).unwrap();
        crate::worker::drain(raw, &mut k, &mut consumers).unwrap();
        k
    }

    fn step(home: &Path, raw: &Raw) -> bool {
        run(home, raw, &drained(home, raw), &mut None, &|| false).unwrap()
    }

    /// Each row's title, its copy's path and whether it was declared.
    fn copies(home: &Path) -> Vec<(String, String, bool)> {
        let db = open(home).unwrap();
        db.prepare("SELECT title, copy, declared FROM deliverables ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// claude-mem's names (tests/shared/deliverables.test.ts): unsafe characters made dashes, a
    /// long key cut with a hash of the whole, a stem giving way before the hash and extension.
    #[test]
    fn names_are_claude_mems() {
        assert_eq!(
            folder_name("thedotmack/claude-mem"),
            "thedotmack-claude-mem"
        );
        assert_eq!(folder_name(r"C:\work\repo"), "C-work-repo");
        assert_eq!(folder_name(".."), "unknown-project");
        let long = "k".repeat(300);
        let folder = folder_name(&long);
        let hash = &crate::curate::sha256_hex(&long)[..8];
        assert!(
            folder.len() <= NAME_MAX && folder.ends_with(&format!("-{hash}")),
            "{folder}"
        );
        let sha = format!("3f9a1c2b{}", "0".repeat(56));
        let name = |path: &str, hash: &str| copy_name(Path::new(path), hash);
        assert_eq!(
            name("/w/report.md", &sha[..8]).unwrap(),
            "report-3f9a1c2b.md"
        );
        assert_eq!(name("/w/.hidden", &sha[..8]).unwrap(), "hidden-3f9a1c2b");
        assert_eq!(name("/w/a:b?.pdf", &sha[..8]).unwrap(), "a-b-3f9a1c2b.pdf");
        let whole = name(&format!("/w/{}.pdf", "s".repeat(300)), &sha).unwrap();
        assert!(
            whole.len() <= NAME_MAX && whole.ends_with(&format!("-{sha}.pdf")),
            "{whole}"
        );
        assert!(name(&format!("/w/a.{}", "x".repeat(260)), &sha[..8]).is_err());
        // An extension is made safe too, so every copy's name is one the sweep removes.
        assert_eq!(
            name("/w/report.p*df", &sha[..8]).unwrap(),
            "report-3f9a1c2b.p-df"
        );
        // A backslash is in a name only where it separates no folders.
        #[cfg(unix)]
        {
            assert_eq!(
                name(r"/w/report.p\df", &sha[..8]).unwrap(),
                "report-3f9a1c2b.p-df"
            );
        }
    }

    /// X23: a turn's declared file (relative to its folder) and the files it wrote outside the
    /// source tree (in no repository, or one git ignores) are copied, declared first; a source
    /// file it edited is not; the same bytes another turn makes are kept once.
    #[test]
    fn a_turn_copies_what_it_declared_and_wrote_outside_the_source_tree() {
        let (home, mut raw) = ready();
        let (work, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (w, r) = (work.path(), repo.path());
        git(r, &["init", "-q"]);
        std::fs::write(r.join(".gitignore"), "out/\n").unwrap();
        std::fs::create_dir(r.join("out")).unwrap();
        std::fs::write(w.join("report.pdf"), b"%PDF made").unwrap();
        std::fs::write(w.join("notes.md"), b"# Notes").unwrap();
        std::fs::write(r.join("out/chart.png"), b"chart").unwrap();
        std::fs::write(r.join("logo.png"), b"logo").unwrap();
        let fields =
            json!({"list": "report", "fields": {"task": "report", "work_product": "notes.md"}});
        turn(
            &mut raw,
            "s1",
            w,
            &[
                (
                    "Write",
                    json!({"file_path": w.join("report.pdf"), "content": "%PDF made"}),
                ),
                ("Write", json!({"file_path": r.join("out/chart.png")})),
                ("Edit", json!({"file_path": r.join("logo.png")})),
                ("mcp__oboete__work_state_write", fields),
            ],
        );
        assert!(step(home.path(), &raw));
        let got = copies(home.path());
        let titles: Vec<&str> = got.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(titles, ["notes.md", "report.pdf", "chart.png"]);
        assert!(got[0].1.starts_with("shop/notes-") && got[0].2 && !got[1].2);
        let read = |copy: &str| std::fs::read(home.path().join(FOLDER).join(copy)).unwrap();
        assert_eq!(read(&got[1].1), b"%PDF made");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let copy = home.path().join(FOLDER).join(&got[1].1);
            let mode = std::fs::metadata(copy).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "a copy is the owner's alone");
        }
        turn(
            &mut raw,
            "s2",
            w,
            &[("Write", json!({"file_path": w.join("report.pdf")}))],
        );
        assert!(step(home.path(), &raw));
        let again = copies(home.path());
        assert_eq!(
            (again.len(), &again[3].1),
            (4, &got[1].1),
            "the same bytes, one file"
        );
        assert!(!step(home.path(), &raw), "nothing new");
    }

    /// X23: a failed call names nothing, and nor does a call of a replayed session or an agent
    /// whose prompts the hooks do not all see.
    #[test]
    fn a_failed_call_a_replay_and_another_agent_copy_nothing() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
        let input = json!({"file_path": w.join("report.pdf")}).to_string();
        let failed = json!({"tool": "Write", "input": input, "output": "", "failed": true,
            "public_turn": true});
        raw.append(&ev("s1", "prompt", json!({"prompt": "Go."}), w))
            .unwrap();
        raw.append(&ev("s1", "tool", failed, w)).unwrap();
        raw.append(&ev("s1", "reply", json!({"assistant": "No."}), w))
            .unwrap();
        for (agent, source) in [("claude", "replay"), ("cursor", "hook"), ("agy", "hook")] {
            let at = |e: Event| Event {
                agent: agent.into(),
                source: source.into(),
                ..e
            };
            raw.append(&at(ev("s2", "prompt", json!({"prompt": "Go."}), w)))
                .unwrap();
            let call = json!({"file_path": w.join("report.pdf")});
            raw.append(&at(tool("s2", "Write", call, w))).unwrap();
            raw.append(&at(ev("s2", "reply", json!({"assistant": "Done."}), w)))
                .unwrap();
        }
        step(home.path(), &raw);
        assert!(copies(home.path()).is_empty());
    }

    /// Grok writes with `write` and `search_replace`, and its hooks give an MCP call's input as
    /// `{"tool_name", "tool_input"}`: its turn's files are copied as Claude Code's are.
    #[test]
    fn a_grok_turns_writes_and_wrapped_declaration_are_read() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        for (name, bytes) in [
            ("notes.md", "# Notes"),
            ("deck.pdf", "deck"),
            ("chart.png", "c"),
        ] {
            std::fs::write(w.join(name), bytes).unwrap();
        }
        let grok = |e: Event| Event {
            agent: "grok".into(),
            ..e
        };
        raw.append(&grok(ev("g1", "prompt", json!({"prompt": "Go."}), w)))
            .unwrap();
        let declare = json!({"tool_name": "oboete__work_state_write", "tool_input":
            {"list": "deck", "fields": {"work_product": w.join("notes.md")}}});
        let calls = [
            (
                "write",
                json!({"file_path": w.join("deck.pdf"), "content": "deck"}),
            ),
            ("search_replace", json!({"file_path": w.join("chart.png")})),
            ("oboete__work_state_write", declare),
        ];
        for (name, input) in calls {
            raw.append(&grok(tool("g1", name, input, w))).unwrap();
        }
        raw.append(&grok(ev("g1", "reply", json!({"assistant": "Done."}), w)))
            .unwrap();
        assert!(step(home.path(), &raw));
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["notes.md", "deck.pdf", "chart.png"]);
    }

    /// A call recorded with no folder, an empty one or one not absolute has no root to hold its
    /// paths to, so it names nothing: claude-mem's roots start at the call's folder, which keeps
    /// out `work_product: ~/.ssh/id_rsa`.
    #[test]
    fn a_call_with_no_folder_names_nothing() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("notes.md"), b"# Notes").unwrap();
        std::fs::write(w.join("deck.pdf"), b"deck").unwrap();
        for (i, cwd) in [None, Some(""), Some("work")].into_iter().enumerate() {
            let session = format!("s{i}");
            let at = |e: Event| Event {
                cwd: cwd.map(str::to_owned),
                ..e
            };
            raw.append(&at(ev(&session, "prompt", json!({"prompt": "Go."}), w)))
                .unwrap();
            let declare = json!({"list": "l", "fields": {"work_product": w.join("notes.md")}});
            let write = json!({"file_path": w.join("deck.pdf")});
            for (name, input) in [("mcp__oboete__work_state_write", declare), ("Write", write)] {
                raw.append(&at(tool(&session, name, input, w))).unwrap();
            }
            raw.append(&at(ev(&session, "reply", json!({"assistant": "Done."}), w)))
                .unwrap();
        }
        step(home.path(), &raw);
        assert!(copies(home.path()).is_empty());
    }

    /// A turn the harness's message starts (a task's notification, recorded as an `envelope`) is a
    /// turn as claude-mem reads one, whose transcript line starts a turn: its files are copied as a
    /// prompt's turn's are.
    #[test]
    fn a_turn_a_notification_starts_copies_its_files() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("deck.pdf"), b"deck").unwrap();
        let note = json!({"prompt": "<task-notification>\n<status>completed</status>"});
        raw.append(&ev("s1", "envelope", note, w)).unwrap();
        let write = json!({"file_path": w.join("deck.pdf")});
        raw.append(&tool("s1", "Write", write, w)).unwrap();
        raw.append(&ev("s1", "reply", json!({"assistant": "Done."}), w))
            .unwrap();
        assert!(step(home.path(), &raw));
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["deck.pdf"]);
    }

    /// Decision 73: a public prompt the agent never answered, then a wholly private prompt whose
    /// turn declares a file: the private turn's records are marked by the hooks, and the file is
    /// not copied, nor is one a turn the harness starts after it declares (its reply leaves the
    /// flag); the next public prompt's is.
    #[test]
    fn a_private_turns_files_are_not_copied() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        for name in ["plan.pdf", "note.pdf", "public.pdf"] {
            std::fs::write(w.join(name), name).unwrap();
        }
        let settings = crate::capture::Settings::default();
        let hook = |raw: &mut Raw, event: &str, mut payload: Value| {
            payload["session_id"] = json!("s1");
            payload["cwd"] = json!(w);
            let (mut failed, mut back) = (0, 0);
            let ts = crate::db::now_ms();
            crate::hook::record(
                home.path(),
                raw,
                "claude",
                event,
                &payload,
                ts,
                &settings,
                &mut failed,
                &mut back,
            )
            .unwrap();
        };
        let declare = |file: &str| {
            json!({"tool_name": "mcp__oboete__work_state_write", "tool_response": "ok",
                "tool_input": {"list": "plan", "fields": {"work_product": w.join(file)}}})
        };
        hook(
            &mut raw,
            "UserPromptSubmit",
            json!({"prompt": "Start the plan."}),
        );
        hook(
            &mut raw,
            "UserPromptSubmit",
            json!({"prompt": "<private>the real plan</private>"}),
        );
        hook(&mut raw, "PostToolUse", declare("plan.pdf"));
        hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
        // A turn a task's notification starts after it goes on with the private work.
        let note = "<task-notification>\n<status>completed</status>";
        hook(&mut raw, "UserPromptSubmit", json!({ "prompt": note }));
        hook(&mut raw, "PostToolUse", declare("note.pdf"));
        hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
        hook(
            &mut raw,
            "UserPromptSubmit",
            json!({"prompt": "Now the public one."}),
        );
        hook(&mut raw, "PostToolUse", declare("public.pdf"));
        hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
        step(home.path(), &raw);
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["public.pdf"]);
        assert!(
            !PrivateTurn::new(home.path(), "claude", "s1").is_set(),
            "cleared by the public prompt"
        );
    }

    /// A call before any prompt record of its turn (hooks set up during a turn, a wholly private
    /// prompt), and a call whose mark a rule masked, copy nothing; past the cap of a turn, only the
    /// files that open count, so fifty missing ones leave a real one copied.
    #[test]
    fn a_call_with_no_prompt_or_a_masked_mark_copies_nothing() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
        let write = json!({"file_path": w.join("report.pdf")});
        raw.append(&tool("s1", "Write", write.clone(), w)).unwrap();
        raw.append(&ev("s1", "reply", json!({"assistant": "Done."}), w))
            .unwrap();
        let masked = json!({"tool": "Write", "input": write.to_string(), "output": "",
            "failed": false, "[REDACTED:word]": true});
        raw.append(&ev("s2", "prompt", json!({"prompt": "Go."}), w))
            .unwrap();
        raw.append(&ev("s2", "tool", masked, w)).unwrap();
        raw.append(&ev("s2", "reply", json!({"assistant": "Done."}), w))
            .unwrap();
        step(home.path(), &raw);
        assert!(copies(home.path()).is_empty());
        let mut many: Vec<Value> = (0..MAX_FILES)
            .map(|i| json!(w.join(format!("gone-{i}.pdf"))))
            .collect();
        many.push(json!(w.join("report.pdf")));
        let declare =
            json!({"list": "l", "fields": {"work_products": Value::Array(many).to_string()}});
        turn(
            &mut raw,
            "s3",
            w,
            &[("mcp__oboete__work_state_write", declare)],
        );
        step(home.path(), &raw);
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["report.pdf"]);
    }

    /// A seq a restore gave again is another turn: its row and the old one are both kept, so
    /// forgetting one leaves the copy the other names.
    #[test]
    fn a_turn_end_is_known_by_its_seq_and_time() {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let row = |turn_ts: i64| {
            db.execute(
                "INSERT OR IGNORE INTO deliverables(device, agent, session, turn_seq, turn_ts,
                   call_seq, call_ts, title, kind, source_path, copy, bytes, sha256, declared,
                   copied)
                 VALUES('d', 'claude', 's', 7, ?1, 6, 1, 't', 'file', '/w/a', 'shop/a-1', 1, 'h', 0, 1)",
                [turn_ts],
            )
            .unwrap()
        };
        assert_eq!((row(100), row(200), row(200)), (1, 1, 0));
    }

    /// A forgotten copy the sweep cannot remove yet keeps its row and fails the sweep, so forget's
    /// last step stays owed; once it can, the sweep and `forget --continue`'s purge remove it.
    #[cfg(unix)]
    #[test]
    fn a_forgotten_copy_not_yet_removed_keeps_forget_owed() {
        use std::os::unix::fs::PermissionsExt;
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("own.pdf"), b"own").unwrap();
        turn(
            &mut raw,
            "s1",
            w,
            &[("Write", json!({"file_path": w.join("own.pdf")}))],
        );
        let first = raw.max_seq().unwrap();
        step(home.path(), &raw);
        let copy = copies(home.path()).remove(0).1;
        let folder = home
            .path()
            .join(FOLDER)
            .join(copy.split('/').next().unwrap());
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        let handle = format!("{}:{first}", raw.device());
        let target = crate::forget::session_target(home.path(), &handle).unwrap();
        let preview = crate::forget::preview(home.path(), target).unwrap();
        crate::forget::start(home.path(), &preview).unwrap();
        drop(raw);
        let raw = crate::raw::open(home.path()).unwrap();
        let blocked = std::fs::remove_file(folder.join("probe"))
            .err()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
            || std::fs::write(folder.join("probe"), b"").is_err();
        let swept = sweep(home.path(), &raw, false);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        if blocked {
            assert!(swept.is_err());
            assert_eq!(copies(home.path()).len(), 1, "its row stays");
        }
        drop(raw);
        crate::forget::purge(home.path()).unwrap();
        assert!(copies(home.path()).is_empty());
        assert!(!home.path().join(FOLDER).join(&copy).exists());
    }

    /// The sweep never follows a link at `deliverables/` itself: files outside no row names stay,
    /// and a folder there that cannot be read is not read; a copy removed through a folder link is
    /// refused too.
    #[cfg(unix)]
    #[test]
    fn the_sweep_follows_no_link_at_its_folder() {
        use std::os::unix::fs::PermissionsExt;
        let (home, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir(outside.path().join("shop")).unwrap();
        std::fs::write(outside.path().join("shop").join("keep.txt"), b"keep").unwrap();
        let closed = outside.path().join("closed");
        std::fs::create_dir(&closed).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join(FOLDER)).unwrap();
        drop(open(home.path()).unwrap());
        let raw = crate::raw::open(home.path()).unwrap();
        let swept = sweep(home.path(), &raw, true);
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(swept.unwrap(), 0);
        assert!(!remove_copy(home.path(), "shop/keep.txt").unwrap());
        assert!(outside.path().join("shop").join("keep.txt").is_file());
    }

    /// No Stop clears the flag, an empty one (its turn may go on) or one with a reply (a turn the
    /// harness starts next may go on with the private work), nor does the harness's message: the
    /// next public prompt does.
    #[test]
    fn a_private_turn_lasts_until_a_public_prompt() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let settings = crate::capture::Settings::default();
        let (mut failed, mut back) = (0, 0);
        let mut hook = |raw: &mut Raw, event: &str, mut payload: Value| {
            payload["session_id"] = json!("s1");
            payload["cwd"] = json!(home.path());
            crate::hook::record(
                home.path(),
                raw,
                "claude",
                event,
                &payload,
                1,
                &settings,
                &mut failed,
                &mut back,
            )
            .unwrap();
        };
        hook(
            &mut raw,
            "UserPromptSubmit",
            json!({"prompt": "<private>plan</private>"}),
        );
        let set = || PrivateTurn::new(home.path(), "claude", "s1").is_set();
        hook(&mut raw, "Stop", json!({"last_assistant_message": ""}));
        assert!(set());
        hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
        assert!(set());
        let note = "<task-notification>\n<status>completed</status>";
        hook(&mut raw, "UserPromptSubmit", json!({ "prompt": note }));
        assert!(set());
        hook(&mut raw, "UserPromptSubmit", json!({"prompt": "Go on."}));
        assert!(!set());
    }

    /// A replayed reply in a live session is no boundary of its live turn: the turn's file is
    /// copied at the live reply.
    #[test]
    fn a_replayed_reply_is_no_boundary_of_a_live_turn() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
        raw.append(&ev("s1", "prompt", json!({"prompt": "Go."}), w))
            .unwrap();
        raw.append(&tool(
            "s1",
            "Write",
            json!({"file_path": w.join("report.pdf")}),
            w,
        ))
        .unwrap();
        let replayed = Event {
            source: "replay".into(),
            ..ev("s1", "reply", json!({"assistant": "Replayed."}), w)
        };
        raw.append(&replayed).unwrap();
        raw.append(&ev("s1", "reply", json!({"assistant": "Done."}), w))
            .unwrap();
        step(home.path(), &raw);
        assert_eq!(copies(home.path()).len(), 1);
    }

    /// The checkpoint's record: the empty store's (seq 0) names none; a record with a time where
    /// the checkpoint had none is another one; one a forget made a tombstone since is the same
    /// record in the same raw.db file, or with no file kept, and not in another (#836 item 3).
    #[test]
    fn a_checkpoint_knows_its_record() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let at = |seq, ts, file: Option<&str>| Checkpoint {
            seq,
            ts,
            file: file.map(str::to_owned),
        };
        let file = raw.file_identity().map(str::to_owned);
        assert!(file.is_some());
        assert!(still_there(&raw, &at(0, None, None)).unwrap());
        let seq = raw
            .append(&ev("s1", "prompt", json!({"prompt": "Go."}), home.path()))
            .unwrap();
        let ts = time_at(&raw, seq).unwrap();
        assert!(still_there(&raw, &at(seq, ts, None)).unwrap());
        assert!(!still_there(&raw, &at(seq, None, None)).unwrap());
        assert!(!still_there(&raw, &at(seq, Some(1), None)).unwrap());
        assert!(!still_there(&raw, &at(seq + 1, ts, None)).unwrap());
        let device = raw.device().to_owned();
        raw.append_tombstone(crate::raw::Target::Record { device, seq })
            .unwrap();
        assert_eq!(time_at(&raw, seq).unwrap(), None, "removed now");
        for kept in [ts, None] {
            assert!(still_there(&raw, &at(seq, kept, file.as_deref())).unwrap());
            assert!(still_there(&raw, &at(seq, kept, None)).unwrap());
            assert!(!still_there(&raw, &at(seq, kept, Some("0:0"))).unwrap());
        }
    }

    /// A replay's hooks for the same session neither clear nor read a live private turn's flag.
    #[test]
    fn a_replay_leaves_a_live_private_turn_alone() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let replay = crate::capture::Settings {
            source: "replay",
            ..crate::capture::Settings::default()
        };
        let flag = PrivateTurn::new(home.path(), "claude", "s1");
        flag.set().unwrap();
        let (mut failed, mut back) = (0, 0);
        let prompt = json!({"session_id": "s1", "cwd": home.path(), "prompt": "A public one."});
        crate::hook::record(
            home.path(),
            &mut raw,
            "claude",
            "UserPromptSubmit",
            &prompt,
            1,
            &replay,
            &mut failed,
            &mut back,
        )
        .unwrap();
        assert!(flag.is_set());
    }

    /// Records read past a whole round with no turn end are progress: the worker takes the next
    /// round, where a turn end may wait.
    #[test]
    fn records_read_past_are_progress() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        raw.append(&ev("s1", "prompt", json!({"prompt": "Go."}), w))
            .unwrap();
        for _ in 0..BATCH + 5 {
            raw.append(&tool("s1", "Read", json!({"file_path": "/w/a"}), w))
                .unwrap();
        }
        assert!(step(home.path(), &raw), "a full round");
        assert!(step(home.path(), &raw), "the rest");
        assert!(!step(home.path(), &raw), "nothing left");
    }

    /// The flag the hooks keep: a flag that cannot be read is set (fail closed).
    #[test]
    fn a_private_turn_flag_that_cannot_be_read_is_set() {
        let home = tempfile::tempdir().unwrap();
        let flag = PrivateTurn::new(home.path(), "claude", "s1");
        assert!(!flag.is_set());
        flag.set().unwrap();
        assert!(flag.is_set());
        flag.clear().unwrap();
        flag.clear().unwrap();
        assert!(!flag.is_set());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = home.path().join("state").join("private-turns");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
            let unreadable = std::fs::read_dir(&dir).is_err();
            let set = flag.is_set();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(set || !unreadable, "an unreadable flag is set");
        }
    }

    /// A source is checked on its handle: outside the roots, through a link (to a file or a folder)
    /// leading out, or a FIFO, it is refused, the FIFO without blocking.
    #[test]
    fn a_source_outside_the_roots_a_link_out_or_a_fifo_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let (a, b) = (base.path().join("a"), base.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("key"), b"secret").unwrap();
        std::fs::write(a.join("ok.pdf"), b"ok").unwrap();
        let roots = vec![std::fs::canonicalize(&a).unwrap()];
        assert!(open_source(&a.join("ok.pdf"), &roots).is_ok());
        assert!(open_source(&b.join("key"), &roots).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(b.join("key"), a.join("link.pdf")).unwrap();
            assert!(open_source(&a.join("link.pdf"), &roots).is_err());
            std::os::unix::fs::symlink(&b, a.join("dir")).unwrap();
            assert!(open_source(&a.join("dir").join("key"), &roots).is_err());
            let pipe = std::ffi::CString::new(a.join("pipe.pdf").to_str().unwrap()).unwrap();
            // SAFETY: a NUL-terminated path, a fresh name in a folder of this test.
            assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
            let started = std::time::Instant::now();
            assert!(open_source(&a.join("pipe.pdf"), &roots).is_err());
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
        }
    }

    /// A name another file holds (two contents whose hashes start alike) leaves the copy its whole
    /// hash's name; the same bytes again are found under it.
    #[test]
    fn a_name_other_bytes_hold_leaves_the_copy_its_whole_hash() {
        let (home, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let src = work.path().join("report.pdf");
        std::fs::write(&src, b"new").unwrap();
        let sha = hex(&Sha256::digest(b"new"));
        let dir = held_folder(home.path(), "shop").unwrap().path;
        let short = dir.join(format!("report-{}.pdf", &sha[..8]));
        std::fs::write(&short, b"other").unwrap();
        let roots = roots(work.path());
        let copy = |src: &Path| {
            copy_into(home.path(), "shop", src, open_source(src, &roots).unwrap()).unwrap()
        };
        let copied = copy(&src);
        assert_eq!(copied.copy, format!("shop/report-{sha}.pdf"));
        assert_eq!(std::fs::read(&short).unwrap(), b"other");
        assert_eq!(copy(&src), copied);
        #[cfg(unix)]
        {
            std::fs::remove_file(&short).unwrap();
            std::os::unix::fs::symlink(&src, &short).unwrap();
            assert_eq!(copy(&src), copied, "a link under the name is not read");
            // A larger file under the name (sparse here) is read no further than the copy's size.
            std::fs::remove_file(&short).unwrap();
            let big = std::fs::File::create(&short).unwrap();
            big.set_len(MAX_BYTES + 1).unwrap();
            let started = std::time::Instant::now();
            assert_eq!(copy(&src), copied);
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
        }
    }

    /// A cut input names its path by its head.
    #[test]
    fn a_cut_write_input_names_its_path_by_its_head() {
        let cut = format!(
            "{{\"file_path\":\"/w/report.pdf\",\"content\":\"{}\n…[cut: 99 bytes in full]…\ntail\"}}",
            "x".repeat(10)
        );
        assert_eq!(written("Write", &cut).as_deref(), Some("/w/report.pdf"));
        let quoted = r#"{"file_path":"/w/a\"b.pdf","content":"c"#;
        assert_eq!(written("Write", quoted).as_deref(), Some("/w/a\"b.pdf"));
        let notebook = r#"{"notebook_path":"/w/n.ipynb"}"#;
        assert_eq!(
            written("NotebookEdit", notebook).as_deref(),
            Some("/w/n.ipynb")
        );
        assert_eq!(written("Read", r#"{"file_path":"/w/r.pdf"}"#), None);
    }

    /// Runs `f` with `act` called at each stage of a copy's or a removal's hold, on this thread.
    fn with_seam<T>(act: impl FnMut(Stage) + 'static, f: impl FnOnce() -> T) -> T {
        SEAM.with(|s| *s.borrow_mut() = Some(Box::new(act)));
        let out = f();
        SEAM.with(|s| *s.borrow_mut() = None);
        out
    }

    /// An act at `at` that moves `dir` to `moved` and puts a link to `outside` in its place, once.
    #[cfg(unix)]
    fn swap_at(at: Stage, dir: &Path, moved: &Path, outside: &Path) -> impl FnMut(Stage) + 'static {
        let (dir, moved, outside) = (dir.to_owned(), moved.to_owned(), outside.to_owned());
        let mut done = false;
        move |stage| {
            if stage == at && !done {
                done = true;
                std::fs::rename(&dir, &moved).unwrap();
                std::os::unix::fs::symlink(&outside, &dir).unwrap();
            }
        }
    }

    /// #836 item 2: a copy holds `deliverables/` and its folder by their descriptors. The folder
    /// swapped for a link to a folder outside once it is open receives the copy where it was moved:
    /// the folder outside, holding a file of the copy's short name with other bytes, is neither
    /// read for that name nor written; the same bytes already under the short name in the folder
    /// held are reused there.
    #[cfg(unix)]
    #[test]
    fn a_copy_holds_its_folder_when_it_is_swapped_for_a_link() {
        for held_same in [false, true] {
            let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let work = tempfile::tempdir().unwrap();
            let src = work.path().join("deck.pdf");
            std::fs::write(&src, b"deck").unwrap();
            let short = copy_name(&src, &hex(&Sha256::digest(b"deck"))[..8]).unwrap();
            let folder = home.path().join(FOLDER).join("shop");
            std::fs::create_dir_all(&folder).unwrap();
            if held_same {
                std::fs::write(folder.join(&short), b"deck").unwrap();
            }
            let outside = out.path().join("shop");
            std::fs::create_dir(&outside).unwrap();
            std::fs::write(outside.join(&short), b"other").unwrap();
            let moved = out.path().join("moved");
            let act = swap_at(Stage::Folder, &folder, &moved, &outside);
            let copied = with_seam(act, || {
                copy_into(
                    home.path(),
                    "shop",
                    &src,
                    std::fs::File::open(&src).unwrap(),
                )
            })
            .unwrap();
            assert_eq!(
                copied.copy,
                format!("shop/{short}"),
                "held_same {held_same}"
            );
            assert_eq!(std::fs::read(moved.join(&short)).unwrap(), b"deck");
            assert_eq!(std::fs::read(outside.join(&short)).unwrap(), b"other");
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
            assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
        }
    }

    /// #836 item 2: a copy whose read fails after its partial file is made removes the partial
    /// from the folder it holds, also when that folder was swapped for a link meanwhile.
    #[cfg(unix)]
    #[test]
    fn a_failed_copy_removes_its_partial_from_the_held_folder() {
        let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let folder = home.path().join(FOLDER).join("shop");
        std::fs::create_dir_all(&folder).unwrap();
        let outside = out.path().join("shop");
        std::fs::create_dir(&outside).unwrap();
        let moved = out.path().join("moved");
        // A folder's handle read as the source: its read fails, every time.
        let source = std::fs::File::open(out.path()).unwrap();
        let act = swap_at(Stage::Partial, &folder, &moved, &outside);
        let copied = with_seam(act, || {
            copy_into(home.path(), "shop", Path::new("/w/deck.pdf"), source)
        });
        assert!(copied.is_err());
        assert_eq!(
            std::fs::read_dir(&moved).unwrap().count(),
            0,
            "no partial left"
        );
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    /// #836 item 2: `deliverables/` swapped for a link once it is open, before a folder of it is
    /// made: the folder is made in the one held, and nothing outside.
    #[cfg(unix)]
    #[test]
    fn a_missing_folder_is_made_in_the_held_top() {
        let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let top = home.path().join(FOLDER);
        std::fs::create_dir(&top).unwrap();
        let outside = out.path().join("top");
        std::fs::create_dir(&outside).unwrap();
        let moved = out.path().join("moved");
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("deck.pdf");
        std::fs::write(&src, b"deck").unwrap();
        let act = swap_at(Stage::Top, &top, &moved, &outside);
        let copied = with_seam(act, || {
            copy_into(
                home.path(),
                "shop",
                &src,
                std::fs::File::open(&src).unwrap(),
            )
        })
        .unwrap();
        assert_eq!(std::fs::read(moved.join(&copied.copy)).unwrap(), b"deck");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    /// #836 item 2: a removal holds its folder: swapped for a link once it is open, the folder where
    /// it was moved loses the file and the folder outside keeps its file of the same name; the
    /// sweep's walk of files no row names does the same, reading the names of the folder it holds
    /// (one outside holds none of them).
    #[cfg(unix)]
    #[test]
    fn a_removal_holds_its_folder_when_it_is_swapped_for_a_link() {
        for orphans in [false, true] {
            let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let folder = home.path().join(FOLDER).join("shop");
            std::fs::create_dir_all(&folder).unwrap();
            let held = if orphans { "gone.pdf" } else { "old.pdf" };
            std::fs::write(folder.join(held), b"old").unwrap();
            let outside = out.path().join("shop");
            std::fs::create_dir(&outside).unwrap();
            std::fs::write(outside.join("old.pdf"), b"keep").unwrap();
            let moved = out.path().join("moved");
            drop(open(home.path()).unwrap());
            let raw = crate::raw::open(home.path()).unwrap();
            let act = swap_at(Stage::Folder, &folder, &moved, &outside);
            let removed = with_seam(act, || {
                if orphans {
                    sweep(home.path(), &raw, true).unwrap() == 1
                } else {
                    remove_copy(home.path(), "shop/old.pdf").unwrap()
                }
            });
            assert!(removed, "orphans {orphans}");
            assert!(!moved.join(held).exists(), "orphans {orphans}");
            assert_eq!(std::fs::read(outside.join("old.pdf")).unwrap(), b"keep");
        }
    }

    /// The sweep's walk of files no row names passes over a folder it cannot read, said, and sweeps
    /// the others, in whatever order the folders are read (CodeRabbit on #845).
    #[cfg(unix)]
    #[test]
    fn the_orphan_walk_passes_over_a_folder_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let top = home.path().join(FOLDER);
        for folder in ["closed", "open"] {
            std::fs::create_dir_all(top.join(folder)).unwrap();
            std::fs::write(top.join(folder).join("orphan.pdf"), b"x").unwrap();
        }
        let closed = top.join("closed");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Where permissions bind (not as root).
        let binds = std::fs::read_dir(&closed).is_err();
        drop(open(home.path()).unwrap());
        let raw = crate::raw::open(home.path()).unwrap();
        let swept = sweep(home.path(), &raw, true);
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
        if binds {
            assert_eq!(swept.unwrap(), 1);
            assert!(closed.join("orphan.pdf").exists());
        }
        assert!(!top.join("open").join("orphan.pdf").exists());
    }

    /// A folder of `deliverables/` that is a link already is neither written through nor emptied
    /// through (as before #836 item 2: its directory opens never follow a link).
    #[cfg(unix)]
    #[test]
    fn a_folder_that_is_a_link_is_not_followed() {
        let (home, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let top = home.path().join(FOLDER);
        std::fs::create_dir(&top).unwrap();
        let outside = out.path().join("shop");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("old.pdf"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, top.join("shop")).unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("deck.pdf");
        std::fs::write(&src, b"deck").unwrap();
        let file = std::fs::File::open(&src).unwrap();
        assert!(copy_into(home.path(), "shop", &src, file).is_err());
        assert!(!remove_copy(home.path(), "shop/old.pdf").unwrap());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
        assert_eq!(std::fs::read(outside.join("old.pdf")).unwrap(), b"keep");
    }

    /// raw.db rebuilt from this store's records below `cut` and its ops under the same device, and
    /// put in place as a restore puts one (`turns.rs`'s #646 test does the same): another file.
    fn rebuilt_below(home: &Path, raw: Raw, cut: i64) -> Raw {
        let (device, lines) = (
            raw.device().to_owned(),
            raw.export_lines(0, usize::MAX).unwrap(),
        );
        let ops = raw.export_op_lines(0, usize::MAX).unwrap();
        let before = raw.file_identity().map(str::to_owned);
        drop(raw);
        let rebuilt = home.join("raw.db.rebuilt");
        let mut rebuild = crate::raw::Rebuild::new(&rebuilt, &device).unwrap();
        for (_, line) in lines.iter().filter(|(seq, _)| *seq < cut) {
            rebuild.add(line).unwrap();
        }
        for (_, line) in &ops {
            rebuild.add_op(line).unwrap();
        }
        rebuild.finish().unwrap();
        for file in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.join(file));
        }
        std::fs::rename(&rebuilt, home.join("raw.db")).unwrap();
        let raw = crate::raw::open(home).unwrap();
        assert_eq!(raw.device(), device);
        assert_ne!(raw.file_identity().map(str::to_owned), before);
        raw
    }

    /// #836 item 3: a checkpoint at a record with no time (a tombstone) is known by raw.db's file
    /// too. A restore below it that gives its seq to another tombstone is seen, and the phase starts
    /// again at the newest record as at any restore, copying nothing older; in the same file the
    /// records after it are read.
    #[test]
    fn a_restore_that_gives_a_timeless_checkpoints_seq_to_another_record_is_seen() {
        for restored in [true, false] {
            let (home, mut raw) = ready();
            let work = tempfile::tempdir().unwrap();
            let w = work.path();
            std::fs::write(w.join("plan.pdf"), b"plan").unwrap();
            turn(&mut raw, "s1", w, &[]);
            let target = raw.max_seq().unwrap();
            let device = raw.device().to_owned();
            let tombstone = |raw: &mut Raw| {
                raw.append_tombstone(crate::raw::Target::Record {
                    device: device.clone(),
                    seq: target,
                })
                .unwrap()
            };
            let cut = tombstone(&mut raw);
            assert!(step(home.path(), &raw));
            let db = open(home.path()).unwrap();
            let at = checkpoint(&db, &device).unwrap().map(|c| (c.seq, c.ts));
            assert_eq!(at, Some((cut, None)));
            if restored {
                raw = rebuilt_below(home.path(), raw, cut);
                assert_eq!(tombstone(&mut raw), cut, "the seq given again");
            } else {
                tombstone(&mut raw);
            }
            let write = [("Write", json!({"file_path": w.join("plan.pdf")}))];
            turn(&mut raw, "s2", w, &write);
            step(home.path(), &raw);
            assert_eq!(
                copies(home.path()).len(),
                usize::from(!restored),
                "restored: {restored}"
            );
        }
    }

    /// #836 item 3: a checkpoint kept before raw.db's file was (`file` NULL) is taken as it is, and
    /// the file is written to it then, also when no record follows it, so a restore after that is
    /// seen; with a turn after it, the turn is copied.
    #[test]
    fn a_checkpoint_kept_before_the_file_takes_it_once() {
        for pending in [false, true] {
            let (home, mut raw) = ready();
            let work = tempfile::tempdir().unwrap();
            let w = work.path();
            std::fs::write(w.join("plan.pdf"), b"plan").unwrap();
            turn(&mut raw, "s1", w, &[]);
            let target = raw.max_seq().unwrap();
            let device = raw.device().to_owned();
            let tombstone = |raw: &mut Raw| {
                raw.append_tombstone(crate::raw::Target::Record {
                    device: device.clone(),
                    seq: target,
                })
                .unwrap()
            };
            let cut = tombstone(&mut raw);
            assert!(step(home.path(), &raw));
            let db = open(home.path()).unwrap();
            db.execute("UPDATE deliverable_scan SET file = NULL", [])
                .unwrap();
            let write = [("Write", json!({"file_path": w.join("plan.pdf")}))];
            if pending {
                turn(&mut raw, "s2", w, &write);
                assert!(step(home.path(), &raw));
                assert_eq!(copies(home.path()).len(), 1);
                continue;
            }
            assert!(!step(home.path(), &raw));
            let c = checkpoint(&db, &device).unwrap().unwrap();
            assert_eq!(
                (c.seq, c.ts, c.file.as_deref()),
                (cut, None, raw.file_identity())
            );
            raw = rebuilt_below(home.path(), raw, cut);
            assert_eq!(tombstone(&mut raw), cut);
            turn(&mut raw, "s2", w, &write);
            step(home.path(), &raw);
            assert!(copies(home.path()).is_empty());
        }
    }

    /// The first run copies none of the records already there; a restore that ends below the
    /// checkpoint moves it to the newest record, copying nothing older.
    #[test]
    fn the_first_run_and_a_restore_copy_nothing_older() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
        let write = || [("Write", json!({"file_path": w.join("report.pdf")}))];
        turn(&mut raw, "s1", w, &write());
        assert!(!step(home.path(), &raw));
        assert!(copies(home.path()).is_empty());
        let db = open(home.path()).unwrap();
        // raw.db ends below the checkpoint, or another record's time is at its seq.
        for ahead in [10, 0] {
            let seq = raw.max_seq().unwrap() + ahead;
            set_checkpoint(&db, &raw, seq, Some(1)).unwrap();
            turn(&mut raw, "s1", w, &write());
            assert!(!step(home.path(), &raw), "{ahead}");
            assert!(copies(home.path()).is_empty(), "{ahead}");
            let at = checkpoint(&db, raw.device()).unwrap().map(|c| c.seq);
            assert_eq!(at, Some(raw.max_seq().unwrap()), "{ahead}");
        }
        turn(&mut raw, "s1", w, &write());
        assert!(step(home.path(), &raw));
        assert_eq!(copies(home.path()).len(), 1);
    }

    /// A command asking the worker to step aside between two files leaves the turn to the next
    /// round, which copies it once.
    #[test]
    fn a_step_aside_leaves_the_turn_to_the_next_round() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("report.pdf"), b"pdf").unwrap();
        // Declared: no git check comes first, so the copy's own wait is what gives way.
        let declare = json!({"list": "l", "fields": {"work_product": w.join("report.pdf")}});
        turn(
            &mut raw,
            "s1",
            w,
            &[("mcp__oboete__work_state_write", declare)],
        );
        assert!(
            run(
                home.path(),
                &raw,
                &drained(home.path(), &raw),
                &mut None,
                &|| true
            )
            .unwrap()
        );
        assert!(copies(home.path()).is_empty());
        assert!(step(home.path(), &raw));
        assert_eq!(copies(home.path()).len(), 1);
        assert!(!step(home.path(), &raw));
    }

    /// Forgetting a session removes its rows and the files no other row names; a copy whose
    /// records are only not found keeps them.
    #[test]
    fn forgetting_the_session_removes_its_copies() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("shared.pdf"), b"shared").unwrap();
        std::fs::write(w.join("own.pdf"), b"own").unwrap();
        let write = |f: &str| ("Write", json!({"file_path": w.join(f)}));
        turn(&mut raw, "s1", w, &[write("shared.pdf"), write("own.pdf")]);
        let first = raw.max_seq().unwrap();
        turn(&mut raw, "s2", w, &[write("shared.pdf")]);
        let second = raw.max_seq().unwrap();
        step(home.path(), &raw);
        let before = copies(home.path());
        assert_eq!(before.len(), 3);
        let handle = format!("{}:{first}", raw.device());
        let target = crate::forget::session_target(home.path(), &handle).unwrap();
        let preview = crate::forget::preview(home.path(), target).unwrap();
        crate::forget::start(home.path(), &preview).unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        sweep(home.path(), &raw, false).unwrap();
        let after = copies(home.path());
        assert_eq!(after.len(), 1);
        let file = |c: &str| home.path().join(FOLDER).join(c);
        assert!(file(&after[0].1).is_file(), "s2's copy stays");
        assert!(!file(&before[1].1).exists(), "s1's own copy goes");
        // A row whose records are not found (a restored raw.db) is no forget.
        let db = open(home.path()).unwrap();
        db.execute(
            "UPDATE deliverables SET turn_seq = turn_seq + 1000, call_seq = call_seq + 1000",
            [],
        )
        .unwrap();
        sweep(home.path(), &raw, true).unwrap();
        assert_eq!(copies(home.path()).len(), 1);
        assert!(file(&after[0].1).is_file());
        // Its session forgotten later removes it still, by the forget's scope.
        let handle = format!("{}:{second}", raw.device());
        let target = crate::forget::session_target(home.path(), &handle).unwrap();
        let preview = crate::forget::preview(home.path(), target).unwrap();
        crate::forget::start(home.path(), &preview).unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        sweep(home.path(), &raw, false).unwrap();
        assert!(copies(home.path()).is_empty());
        assert!(!file(&after[0].1).exists());
    }

    /// Slice 2: a copy is opened to serve only at its place, of its size and bytes, within its
    /// time: a same-size file put in its place, another size, a link there, and `deliverables/`
    /// swapped for a link to a folder holding the same name and bytes are refused. A home reached
    /// through a link reads its copies' store; a link in the store's place is refused.
    #[test]
    fn a_copy_opens_only_as_its_row_says() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("deck.pdf"), b"deck one").unwrap();
        let write = json!({"file_path": w.join("deck.pdf")});
        turn(&mut raw, "s1", w, &[("Write", write)]);
        assert!(step(home.path(), &raw));
        let db = open_read(home.path()).unwrap().unwrap();
        let row = rows(&db).unwrap().remove(0);
        let later = || std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut got = Vec::new();
        (open_copy(home.path(), &row, later()).unwrap().unwrap())
            .read_to_end(&mut got)
            .unwrap();
        assert_eq!(got, b"deck one");
        let copy = home.path().join(FOLDER).join(&row.copy);
        std::fs::write(&copy, b"deck two").unwrap();
        let none = |deadline| matches!(open_copy(home.path(), &row, deadline), Ok(None));
        assert!(none(later()), "same size");
        std::fs::write(&copy, b"deck").unwrap();
        assert!(none(later()), "another size");
        std::fs::write(&copy, b"deck one").unwrap();
        let past = std::time::Instant::now();
        assert!(open_copy(home.path(), &row, past).is_err(), "out of time");
        assert!(open_copy(home.path(), &row, later()).unwrap().is_some());
        #[cfg(unix)]
        {
            std::fs::remove_file(&copy).unwrap();
            std::fs::write(w.join("other"), b"deck one").unwrap();
            std::os::unix::fs::symlink(w.join("other"), &copy).unwrap();
            assert!(none(later()), "a link");
            std::fs::remove_file(&copy).unwrap();
            std::fs::write(&copy, b"deck one").unwrap();
            let top = home.path().join(FOLDER);
            let moved = w.join("moved");
            std::fs::rename(&top, &moved).unwrap();
            std::os::unix::fs::symlink(&moved, &top).unwrap();
            assert!(none(later()), "a linked folder");
            let alias = w.join("home");
            std::os::unix::fs::symlink(home.path(), &alias).unwrap();
            let db = open_read(&alias)
                .unwrap()
                .expect("a home reached through a link");
            assert_eq!(rows(&db).unwrap(), std::slice::from_ref(&row));
            let leaf = w.join("leaf");
            std::fs::create_dir(&leaf).unwrap();
            std::os::unix::fs::symlink(home.path().join(DB), leaf.join(DB)).unwrap();
            assert!(open_read(&leaf).is_err(), "the store a link");
        }
    }

    /// Slice 2b: a list takes a copy as on disk only as `open_copy` would open it: of its row's
    /// size, with no link in its place nor for its folder, its home reached through a link too, by
    /// one open and the handle's own path, so a folder swapped for a link between two checks is
    /// not followed (Codex's local review of slice 2b).
    #[test]
    fn a_copy_is_on_disk_only_where_it_would_open() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("deck.pdf"), b"deck one").unwrap();
        let write = json!({"file_path": w.join("deck.pdf")});
        turn(&mut raw, "s1", w, &[("Write", write)]);
        assert!(step(home.path(), &raw));
        let db = open_read(home.path()).unwrap().unwrap();
        let row = rows(&db).unwrap().remove(0);
        assert!(on_disk(home.path(), &row));
        let copy = home.path().join(FOLDER).join(&row.copy);
        std::fs::write(&copy, b"deck").unwrap();
        assert!(!on_disk(home.path(), &row), "another size");
        std::fs::write(&copy, b"deck one").unwrap();
        #[cfg(unix)]
        {
            let alias = w.join("home");
            std::os::unix::fs::symlink(home.path(), &alias).unwrap();
            assert!(on_disk(&alias, &row), "a home reached through a link");
            // Its folder swapped for a link to one outside that holds the same name and size.
            let folder = copy.parent().unwrap();
            let outside = w.join("outside");
            std::fs::rename(folder, &outside).unwrap();
            std::os::unix::fs::symlink(&outside, folder).unwrap();
            assert!(!on_disk(home.path(), &row), "a linked folder");
            std::fs::remove_file(folder).unwrap();
            std::fs::rename(&outside, folder).unwrap();
            assert!(on_disk(home.path(), &row));
            std::fs::remove_file(&copy).unwrap();
            std::os::unix::fs::symlink(w.join("deck.pdf"), &copy).unwrap();
            assert!(!on_disk(home.path(), &row), "a link in its place");
        }
        let _ = std::fs::remove_file(&copy);
        assert!(!on_disk(home.path(), &row), "gone");
    }

    /// A row's provenance names its turn, call and name too: a row id a sweep freed, taken by a
    /// copy of the same bytes at the same time in another turn, has another provenance and
    /// fingerprint (and so another link's tag).
    #[test]
    fn a_reused_row_id_is_another_copy() {
        let row = Row {
            id: 7,
            device: "d".into(),
            agent: "claude".into(),
            session: "s".into(),
            repo: None,
            turn_seq: 10,
            turn_ts: 100,
            call_seq: 9,
            call_ts: 99,
            title: "a.pdf".into(),
            kind: "document".into(),
            source_path: "/w/a.pdf".into(),
            copy: "r/a-1.pdf".into(),
            bytes: 3,
            sha256: "ab".into(),
            copied: 1_000,
        };
        let other = Row {
            turn_seq: 20,
            turn_ts: 200,
            ..row.clone()
        };
        assert_ne!(provenance(&row), provenance(&other));
        assert_ne!(fingerprint([&row]), fingerprint([&other]));
        assert_eq!(fingerprint([&row]), fingerprint([&row.clone()]));
        assert_ne!(fingerprint([&row]), fingerprint(std::iter::empty::<&Row>()));
    }

    /// A long turn's records are read one at a time (#836): a turn of hundreds of records copies
    /// its file all the same.
    #[test]
    fn a_long_turn_copies_its_file() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("deck.pdf"), b"deck").unwrap();
        let mut calls: Vec<(&str, Value)> = (0..450)
            .map(|i| ("Read", json!({"file_path": format!("/w/{i}.rs")})))
            .collect();
        calls.push(("Write", json!({"file_path": w.join("deck.pdf")})));
        turn(&mut raw, "s1", w, &calls);
        // The phase reads `BATCH` records a round, as the worker calls it while it goes on.
        while step(home.path(), &raw) {}
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["deck.pdf"]);
    }

    /// git's verdict is read in English whatever the user's language (#836): a git that answers
    /// in another language unless `LC_ALL=C` still leaves a folder in no repository outside.
    #[cfg(unix)]
    #[test]
    fn a_folder_in_no_repository_is_outside_in_any_language() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let git = bin.path().join("git");
        let script = "#!/bin/sh\nif [ \"$LC_ALL\" = C ]; then\n  \
            echo 'fatal: not a git repository (or any of the parent directories): .git' >&2\n\
            else\n  echo 'fatal: git リポジトリではありません' >&2\nfi\nexit 128\n";
        std::fs::write(&git, script).unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join("deck.pdf");
        std::fs::write(&file, b"deck").unwrap();
        assert!(outside_source_by(
            git.as_os_str(),
            &file,
            &mut HashMap::new()
        ));
    }

    /// Slice 3 (docs/subagents.md): subagent `id`'s call, marked public as the hooks mark one.
    fn sub_call(session: &str, id: &str, file: &Path, cwd: &Path) -> Event {
        let input = json!({"file_path": file}).to_string();
        let body = json!({"tool": "Write", "input": input, "output": "", "failed": false,
            "agent_id": id, "agent_type": "general-purpose"});
        let mut e = ev(session, "tool", body, cwd);
        mark_public(&mut e);
        e
    }

    /// The parent's call that launched subagent `id`, in a public turn: in the background, or
    /// returned with it.
    fn launch_of(session: &str, id: &str, background: bool, cwd: &Path) -> Event {
        let input = json!({"description": "Draw the chart", "prompt": "Draw it."}).to_string();
        let status = if background {
            "async_launched"
        } else {
            "completed"
        };
        let body = json!({"tool": "Agent", "input": input, "output": "", "failed": false,
            "launched": {"agent_id": id, "async": background, "status": status}});
        let mut e = ev(session, "tool", body, cwd);
        mark_public(&mut e);
        e
    }

    /// Subagent `id`'s stop, as the hook keeps one.
    fn stop_of(session: &str, id: &str, cwd: &Path) -> Event {
        let body = json!({"agent_id": id, "agent_type": "general-purpose"});
        ev(session, crate::raw::SUBAGENT_STOP, body, cwd)
    }

    fn said(raw: &mut Raw, session: &str, kind: &str, cwd: &Path) -> i64 {
        raw.append(&ev(session, kind, json!({"text": "Go on."}), cwd))
            .unwrap()
    }

    /// Each row's title, its turn's seq and its call's seq, by id.
    fn origins(home: &Path) -> Vec<(String, i64, i64)> {
        let db = open(home).unwrap();
        db.prepare("SELECT title, turn_seq, call_seq FROM deliverables ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// The owner's forget of one record of this device: registered, and when `purge`, purged as
    /// the worker finishes one (its row rewritten without its labels, its copies swept).
    fn forget_record(home: &Path, raw: Raw, seq: i64, purge: bool) -> Raw {
        let device = raw.device().to_owned();
        drop(raw);
        let target = crate::forget::Target::Record { device, seq };
        let preview = crate::forget::preview(home, target).unwrap();
        crate::forget::start(home, &preview).unwrap();
        if purge {
            crate::forget::purge(home).unwrap();
        }
        crate::raw::open(home).unwrap()
    }

    fn files(dir: &Path, names: &[&str]) {
        for name in names {
            if let Some(parent) = dir.join(name).parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(dir.join(name), name.as_bytes()).unwrap();
        }
    }

    /// Slice 3: a file a background subagent wrote after its parent's reply, with no prompt
    /// between, is copied at its stop, once, its row the reply's turn and the writing call's; one
    /// it wrote inside its parent's turn is the turn's, a foreground subagent's too, and not taken
    /// again at its stop; a stop after the parent's next prompt still takes the gap's call; a
    /// session's end with no reply copies the prompt-backed file, and the stop nothing again.
    #[test]
    fn a_background_subagents_file_is_copied_at_its_stop_once() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(
            w,
            &[
                "in.pdf", "out.pdf", "fore.pdf", "mid.pdf", "late.pdf", "end.pdf",
            ],
        );
        // Straddling its parent's reply.
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        let inside = raw
            .append(&sub_call("s1", "a1", &w.join("in.pdf"), w))
            .unwrap();
        let reply = said(&mut raw, "s1", "reply", w);
        let gap = raw
            .append(&sub_call("s1", "a1", &w.join("out.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        // A foreground one: it stops before the call that started it returns.
        said(&mut raw, "s2", "prompt", w);
        let fore = raw
            .append(&sub_call("s2", "f1", &w.join("fore.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s2", "f1", w)).unwrap();
        raw.append(&launch_of("s2", "f1", false, w)).unwrap();
        let reply2 = said(&mut raw, "s2", "reply", w);
        // One that writes in its parent's next turn too, and stops after it.
        said(&mut raw, "s3", "prompt", w);
        raw.append(&launch_of("s3", "l1", true, w)).unwrap();
        let reply3 = said(&mut raw, "s3", "reply", w);
        let late = raw
            .append(&sub_call("s3", "l1", &w.join("late.pdf"), w))
            .unwrap();
        said(&mut raw, "s3", "prompt", w);
        let mid = raw
            .append(&sub_call("s3", "l1", &w.join("mid.pdf"), w))
            .unwrap();
        let next = said(&mut raw, "s3", "reply", w);
        raw.append(&stop_of("s3", "l1", w)).unwrap();
        // A session that ends with no reply.
        said(&mut raw, "s4", "prompt", w);
        raw.append(&launch_of("s4", "e1", true, w)).unwrap();
        let ended = raw
            .append(&sub_call("s4", "e1", &w.join("end.pdf"), w))
            .unwrap();
        let end = said(&mut raw, "s4", "end", w);
        raw.append(&stop_of("s4", "e1", w)).unwrap();
        while step(home.path(), &raw) {}
        let expected: Vec<(String, i64, i64)> = vec![
            ("in.pdf".into(), reply, inside),
            ("out.pdf".into(), reply, gap),
            ("fore.pdf".into(), reply2, fore),
            ("mid.pdf".into(), next, mid),
            ("late.pdf".into(), reply3, late),
            ("end.pdf".into(), end, ended),
        ];
        assert_eq!(origins(home.path()), expected);
        let db = open(home.path()).unwrap();
        set_checkpoint(&db, &raw, 0, None).unwrap();
        while step(home.path(), &raw) {}
        assert_eq!(origins(home.path()), expected, "nothing twice");
    }

    /// Slice 3: one stop over two gaps keeps each gap's copies apart under the unique key:
    /// identical content at two paths is two rows, each its gap's reply's and its call's; one path
    /// written in both is one row, the last write's (the bytes on disk are its). A forget of either
    /// call leaves the other's row and the file; of both, neither.
    #[test]
    fn one_stop_over_two_gaps_keeps_each_gaps_copies_apart() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["notes.pdf"]);
        for d in ["one", "two"] {
            std::fs::create_dir(w.join(d)).unwrap();
            std::fs::write(w.join(d).join("report.pdf"), b"the same bytes").unwrap();
        }
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        let r1 = said(&mut raw, "s1", "reply", w);
        let c1 = (raw.append(&sub_call("s1", "a1", &w.join("one/report.pdf"), w))).unwrap();
        (raw.append(&sub_call("s1", "a1", &w.join("notes.pdf"), w))).unwrap();
        said(&mut raw, "s1", "prompt", w);
        let r2 = said(&mut raw, "s1", "reply", w);
        let c3 = (raw.append(&sub_call("s1", "a1", &w.join("two/report.pdf"), w))).unwrap();
        let c4 = (raw.append(&sub_call("s1", "a1", &w.join("notes.pdf"), w))).unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        step(home.path(), &raw);
        let expected: Vec<(String, i64, i64)> = vec![
            ("report.pdf".into(), r1, c1),
            ("notes.pdf".into(), r2, c4),
            ("report.pdf".into(), r2, c3),
        ];
        assert_eq!(origins(home.path()), expected);
        let copy = copies(home.path())[0].1.clone();
        assert_eq!(copies(home.path())[2].1, copy, "one file for one content");
        let file = home.path().join(FOLDER).join(&copy);
        let raw = forget_record(home.path(), raw, c1, true);
        assert_eq!(origins(home.path()), expected[1..]);
        assert!(file.is_file(), "the other call's row names it");
        drop(forget_record(home.path(), raw, c3, true));
        assert_eq!(origins(home.path()), expected[1..2]);
        assert!(!file.exists());
    }

    /// Slice 3: a later stop takes only the calls after the one the phase took, though that stop
    /// is forgotten since (its copies stay: their origins are the calls and the reply) and a
    /// source changed since; so a resumed subagent's new file is copied though its earlier calls
    /// named `MAX_FILES` files. A stop with no call since copies nothing; a replay's and a
    /// transcript's stop are no boundary; a stop taken again after an interruption, before its
    /// place was kept or after, makes no row twice; a restore empties the places kept.
    #[test]
    fn a_later_stop_takes_only_the_calls_after_the_one_taken() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        let names: Vec<String> = (0..MAX_FILES).map(|i| format!("f{i}.pdf")).collect();
        files(w, &names.iter().map(String::as_str).collect::<Vec<_>>());
        files(w, &["new.pdf", "after.pdf"]);
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        let reply = said(&mut raw, "s1", "reply", w);
        for name in &names {
            raw.append(&sub_call("s1", "a1", &w.join(name), w)).unwrap();
        }
        let first = raw.append(&stop_of("s1", "a1", w)).unwrap();
        step(home.path(), &raw);
        assert_eq!(origins(home.path()).len(), MAX_FILES);
        let mut raw = forget_record(home.path(), raw, first, true);
        assert_eq!(origins(home.path()).len(), MAX_FILES, "the copies stay");
        std::fs::write(w.join("f0.pdf"), b"changed since").unwrap();
        let resumed = raw
            .append(&sub_call("s1", "a1", &w.join("new.pdf"), w))
            .unwrap();
        let second = raw.append(&stop_of("s1", "a1", w)).unwrap();
        step(home.path(), &raw);
        let rows = origins(home.path());
        assert_eq!(rows.len(), MAX_FILES + 1);
        assert_eq!(rows[MAX_FILES], ("new.pdf".into(), reply, resumed));
        // No call since: nothing, the stop read past.
        let empty = raw.append(&stop_of("s1", "a1", w)).unwrap();
        step(home.path(), &raw);
        assert_eq!(origins(home.path()).len(), MAX_FILES + 1);
        let db = open(home.path()).unwrap();
        assert_eq!(checkpoint(&db, raw.device()).unwrap().unwrap().seq, empty);
        // Taken again: before its place was kept, and after.
        for kept in [first, second] {
            (db.execute("UPDATE subagent_scan SET seq = ?1", [kept])).unwrap();
            set_checkpoint(&db, &raw, resumed, time_at(&raw, resumed).unwrap()).unwrap();
            while step(home.path(), &raw) {}
            assert_eq!(origins(home.path()).len(), MAX_FILES + 1);
        }
        // A replay's and a transcript's stop between a call and the hook's stop.
        let after = raw
            .append(&sub_call("s1", "a1", &w.join("after.pdf"), w))
            .unwrap();
        for source in ["replay", "transcript"] {
            let mut other = stop_of("s1", "a1", w);
            other.source = source.into();
            raw.append(&other).unwrap();
        }
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        while step(home.path(), &raw) {}
        let rows = origins(home.path());
        assert_eq!(rows.last().unwrap(), &("after.pdf".into(), reply, after));
        let kept = |db: &Connection| -> i64 {
            (db.query_row("SELECT count(*) FROM subagent_scan", [], |r| r.get(0))).unwrap()
        };
        assert_eq!(kept(&db), 1);
        drop(db);
        let raw = rebuilt_below(home.path(), raw, resumed);
        step(home.path(), &raw);
        assert_eq!(kept(&open(home.path()).unwrap()), 0);
    }

    /// Slice 3: a stop the phase did not take, forgotten before it read it, leaves its calls to
    /// the next stop.
    #[test]
    fn a_stop_forgotten_before_the_phase_read_it_leaves_its_calls_to_the_next() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["one.pdf", "two.pdf"]);
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        let reply = said(&mut raw, "s1", "reply", w);
        let one = raw
            .append(&sub_call("s1", "a1", &w.join("one.pdf"), w))
            .unwrap();
        let first = raw.append(&stop_of("s1", "a1", w)).unwrap();
        let mut raw = forget_record(home.path(), raw, first, true);
        let two = raw
            .append(&sub_call("s1", "a1", &w.join("two.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        while step(home.path(), &raw) {}
        let expected: Vec<(String, i64, i64)> = vec![
            ("one.pdf".into(), reply, one),
            ("two.pdf".into(), reply, two),
        ];
        assert_eq!(origins(home.path()), expected);
    }

    /// Slice 3, through the hooks: a subagent launched from a wholly private prompt's turn, or from
    /// the turn a task's notification starts after it, copies nothing, though a later public
    /// prompt cleared the session's flag before it wrote; one launched from a public turn copies
    /// its file.
    #[test]
    fn a_subagent_launched_from_a_private_turn_copies_nothing() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["secret.pdf", "notified.pdf", "public.pdf"]);
        let settings = crate::capture::Settings::default();
        let hook = |raw: &mut Raw, session: &str, event: &str, mut payload: Value| {
            payload["session_id"] = json!(session);
            payload["cwd"] = json!(w);
            let (mut failed, mut back) = (0, 0);
            let ts = crate::db::now_ms();
            crate::hook::record(
                home.path(),
                raw,
                "claude",
                event,
                &payload,
                ts,
                &settings,
                &mut failed,
                &mut back,
            )
            .unwrap();
        };
        // The prompts before the launch: a wholly private one; the same, then the turn a task's
        // notification starts after it, private still though its record is there; a public one.
        let private = "<private>the real plan</private>";
        let note = "<task-notification>\n<status>completed</status>";
        let cases: [(&str, &[&str], &str); 3] = [
            ("s1", &[private], "secret.pdf"),
            ("s2", &[private, note], "notified.pdf"),
            ("s3", &["Draw the chart."], "public.pdf"),
        ];
        for (session, prompts, file) in cases {
            let id = format!("{session}a");
            for (i, prompt) in prompts.iter().enumerate() {
                if i > 0 {
                    let done = json!({"last_assistant_message": "Done."});
                    hook(&mut raw, session, "Stop", done);
                }
                hook(
                    &mut raw,
                    session,
                    "UserPromptSubmit",
                    json!({ "prompt": prompt }),
                );
            }
            let launched = json!({"tool_name": "Agent",
                "tool_input": {"description": "Draw", "prompt": "Draw it."},
                "tool_response": {"agentId": id, "status": "async_launched", "isAsync": true}});
            hook(&mut raw, session, "PostToolUse", launched);
            hook(
                &mut raw,
                session,
                "Stop",
                json!({"last_assistant_message": "On it."}),
            );
            hook(
                &mut raw,
                session,
                "UserPromptSubmit",
                json!({"prompt": "And a legend?"}),
            );
            hook(
                &mut raw,
                session,
                "Stop",
                json!({"last_assistant_message": "Soon."}),
            );
            let wrote = json!({"tool_name": "Write", "agent_id": id,
                "tool_input": {"file_path": w.join(file), "content": "x"},
                "tool_response": {"type": "create"}});
            hook(&mut raw, session, "PostToolUse", wrote);
            hook(&mut raw, session, "SubagentStop", json!({ "agent_id": id }));
        }
        while step(home.path(), &raw) {}
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["public.pdf"]);
    }

    /// Slice 3: with no launch record, its launch forgotten (purged or registered alone) or its id
    /// masked, launched from a turn with no prompt observed, a replay's or a transcript's launch, a
    /// call that failed or is not marked public, or a Codex session's, a stop copies nothing;
    /// beside them a subagent launched as the hooks see a public turn copies its file.
    #[test]
    fn a_subagent_with_no_public_launch_or_call_copies_nothing() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        let cases = [
            "nolaunch",
            "forgotten",
            "registered",
            "masked",
            "unseen",
            "replayed",
            "imported",
            "failed",
            "unmarked",
            "codex",
            "notified",
            "control",
        ];
        let names: Vec<String> = cases.iter().map(|c| format!("{c}.pdf")).collect();
        files(w, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let mut launches = HashMap::new();
        for case in cases {
            let agent = |mut e: Event| {
                if case == "codex" {
                    e.agent = "codex".into();
                }
                e
            };
            match case {
                "unseen" => {}
                // A turn a task's notification starts.
                "notified" => {
                    said(&mut raw, case, "envelope", w);
                }
                _ => {
                    let prompt = ev(case, "prompt", json!({"prompt": "Go."}), w);
                    raw.append(&agent(prompt)).unwrap();
                }
            }
            if case != "nolaunch" {
                let mut launch = agent(launch_of(case, case, true, w));
                launch.source = match case {
                    "replayed" => "replay".into(),
                    "imported" => "transcript".into(),
                    _ => launch.source,
                };
                let seq = raw.append(&launch).unwrap();
                launches.insert(case, seq);
            }
            raw.append(&agent(ev(case, "reply", json!({"assistant": "On it."}), w)))
                .unwrap();
            let mut call = sub_call(case, case, &w.join(format!("{case}.pdf")), w);
            if case == "failed" {
                call.body = call.body.replace("\"failed\":false", "\"failed\":true");
            }
            if case == "unmarked" {
                let body = json!({"tool": "Write",
                    "input": json!({"file_path": w.join("unmarked.pdf")}).to_string(),
                    "output": "", "failed": false, "agent_id": case});
                call = ev(case, "tool", body, w);
            }
            raw.append(&agent(call)).unwrap();
            raw.append(&agent(stop_of(case, case, w))).unwrap();
        }
        let device = raw.device().to_owned();
        let masked = launches["masked"];
        let body = match &raw.after(&device, masked - 1, 1).unwrap()[0].item {
            Item::Event(e) => e.body.clone(),
            _ => panic!("an event"),
        };
        let at = body.find("\"agent_id\":\"masked\"").unwrap() + "\"agent_id\":\"".len();
        let target = crate::raw::Target::Range {
            device,
            seq: masked,
            offset: at as i64,
            length: "masked".len() as i64,
        };
        raw.append_tombstone(target).unwrap();
        let raw = forget_record(home.path(), raw, launches["forgotten"], true);
        // Registered, not yet purged.
        let raw = forget_record(home.path(), raw, launches["registered"], false);
        while step(home.path(), &raw) {}
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["notified.pdf", "control.pdf"]);
    }

    /// Slice 3: the phase's first run copies nothing older; a stop after it takes no call before
    /// the subagent's earlier stop, which the facts hold though the phase never took it.
    #[test]
    fn a_stop_after_the_first_run_takes_no_call_before_an_earlier_stop() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["old.pdf", "new.pdf"]);
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        let reply = said(&mut raw, "s1", "reply", w);
        raw.append(&sub_call("s1", "a1", &w.join("old.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        assert!(!step(home.path(), &raw));
        let new = raw
            .append(&sub_call("s1", "a1", &w.join("new.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        while step(home.path(), &raw) {}
        let expected: Vec<(String, i64, i64)> = vec![("new.pdf".into(), reply, new)];
        assert_eq!(origins(home.path()), expected);
    }

    /// Slice 3: a gap whose reply a forget denies takes nothing, registered (its labels still
    /// there) or purged; a prompt forgotten and purged between the reply and a call makes the call
    /// no gap's, as it is no copy of its turn's, also when an earlier call of the same stop lay in
    /// the gap before it; beside them a gap copies.
    #[test]
    fn a_gap_whose_reply_or_prompt_was_forgotten_takes_nothing() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(
            w,
            &[
                "denied.pdf",
                "purged.pdf",
                "prompt.pdf",
                "between-gap.pdf",
                "between.pdf",
                "control.pdf",
            ],
        );
        let (mut purged, mut denied) = (Vec::new(), 0);
        for case in ["denied", "purged", "prompt", "between", "control"] {
            said(&mut raw, case, "prompt", w);
            raw.append(&launch_of(case, case, true, w)).unwrap();
            let reply = said(&mut raw, case, "reply", w);
            // A call in the gap, then the purged prompt's turn: the second call is no gap's.
            if case == "between" {
                let file = w.join("between-gap.pdf");
                raw.append(&sub_call(case, case, &file, w)).unwrap();
            }
            if matches!(case, "prompt" | "between") {
                purged.push(said(&mut raw, case, "prompt", w));
            }
            let file = w.join(format!("{case}.pdf"));
            raw.append(&sub_call(case, case, &file, w)).unwrap();
            if matches!(case, "prompt" | "between") {
                said(&mut raw, case, "reply", w);
            }
            raw.append(&stop_of(case, case, w)).unwrap();
            match case {
                "denied" => denied = reply,
                "purged" => purged.push(reply),
                _ => {}
            }
        }
        for seq in purged {
            raw = forget_record(home.path(), raw, seq, true);
        }
        // Registered, not yet purged: the reply's labels are still read.
        let raw = forget_record(home.path(), raw, denied, false);
        let db = Connection::open(home.path().join("raw.db")).unwrap();
        let labels: Option<String> = (db.query_row(
            "SELECT session FROM records WHERE device = ?1 AND seq = ?2",
            params![raw.device(), denied],
            |r| r.get(0),
        ))
        .unwrap();
        assert_eq!(labels.as_deref(), Some("denied"));
        while step(home.path(), &raw) {}
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["between-gap.pdf", "control.pdf"]);
    }

    /// Slice 3: with the `subagents` consumer behind, reply A is taken and the stop waits with
    /// reply B behind it, the phase's checkpoint below the stop; with no checkpoint, or a rebuild
    /// owed, it waits too; once the consumer has reached it, both are taken, no row twice.
    #[test]
    fn a_stop_waits_for_the_subagents_facts() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["a.pdf", "gap.pdf", "b.pdf"]);
        let parent = |file: &str| tool("s1", "Write", json!({"file_path": w.join(file)}), w);
        said(&mut raw, "s1", "prompt", w);
        raw.append(&parent("a.pdf")).unwrap();
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        let a = said(&mut raw, "s1", "reply", w);
        raw.append(&sub_call("s1", "a1", &w.join("gap.pdf"), w))
            .unwrap();
        let behind = drained(home.path(), &raw);
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        said(&mut raw, "s1", "prompt", w);
        raw.append(&parent("b.pdf")).unwrap();
        said(&mut raw, "s1", "reply", w);
        let titles = || -> Vec<String> { copies(home.path()).into_iter().map(|c| c.0).collect() };
        let at = || {
            checkpoint(&open(home.path()).unwrap(), raw.device())
                .unwrap()
                .unwrap()
                .seq
        };
        assert!(run(home.path(), &raw, &behind, &mut None, &|| false).unwrap());
        assert_eq!(titles(), ["a.pdf"]);
        assert_eq!(at(), a);
        assert!(!run(home.path(), &raw, &behind, &mut None, &|| false).unwrap());
        let empty = tempfile::tempdir().unwrap();
        let none = crate::knowledge::open(empty.path()).unwrap();
        assert!(!run(home.path(), &raw, &none, &mut None, &|| false).unwrap());
        let owed = drained(home.path(), &raw);
        let mut consumer = crate::consumer::subagents::Subagents;
        crate::worker::Consumer::rewind(&mut consumer, &owed, raw.device(), 0).unwrap();
        assert!(!run(home.path(), &raw, &owed, &mut None, &|| false).unwrap());
        assert_eq!((titles(), at()), (vec!["a.pdf".to_owned()], a));
        while step(home.path(), &raw) {}
        assert_eq!(titles(), ["a.pdf", "gap.pdf", "b.pdf"]);
        let db = open(home.path()).unwrap();
        set_checkpoint(&db, &raw, a, time_at(&raw, a).unwrap()).unwrap();
        while step(home.path(), &raw) {}
        assert_eq!(titles(), ["a.pdf", "gap.pdf", "b.pdf"]);
    }

    /// Slice 3: a stop's copy goes with a forget of its call or of its gap's reply, not with one
    /// of the stop alone, and stays through retention that expires its call, its reply, its
    /// launch and its stop.
    #[test]
    fn a_stops_copy_goes_with_its_call_or_reply_and_stays_through_retention() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        let cases = ["call", "reply", "stop", "expired"];
        let names: Vec<String> = cases.iter().map(|c| format!("{c}.pdf")).collect();
        files(w, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let mut seqs = HashMap::new();
        for case in cases {
            said(&mut raw, case, "prompt", w);
            let launch = raw.append(&launch_of(case, case, true, w)).unwrap();
            let reply = said(&mut raw, case, "reply", w);
            let file = w.join(format!("{case}.pdf"));
            let call = raw.append(&sub_call(case, case, &file, w)).unwrap();
            let stop = raw.append(&stop_of(case, case, w)).unwrap();
            seqs.insert(case, (launch, reply, call, stop));
        }
        step(home.path(), &raw);
        assert_eq!(copies(home.path()).len(), 4);
        let (launch, reply, call, stop) = seqs["expired"];
        raw.expire_for_test(&[launch, reply, call, stop]);
        sweep(home.path(), &raw, true).unwrap();
        assert_eq!(copies(home.path()).len(), 4);
        let raw = forget_record(home.path(), raw, seqs["call"].2, true);
        let raw = forget_record(home.path(), raw, seqs["reply"].1, true);
        let raw = forget_record(home.path(), raw, seqs["stop"].3, true);
        sweep(home.path(), &raw, true).unwrap();
        let left = copies(home.path());
        let titles: Vec<&str> = left.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(titles, ["stop.pdf", "expired.pdf"]);
        for (_, copy, _) in &left {
            assert!(home.path().join(FOLDER).join(copy).is_file());
        }
    }

    /// Slice 3: a stop's kept seq and the phase's checkpoint are written together: when the
    /// checkpoint's write fails neither is kept, so a restore that keeps the older checkpoint and
    /// gives the stop's seqs to new records copies the new file.
    #[test]
    fn a_stops_seq_is_kept_with_the_checkpoint() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["old.pdf", "new.pdf"]);
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        said(&mut raw, "s1", "reply", w);
        step(home.path(), &raw);
        let write = raw
            .append(&sub_call("s1", "a1", &w.join("old.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        let db = open(home.path()).unwrap();
        db.execute_batch(
            "CREATE TRIGGER interrupted BEFORE UPDATE ON deliverable_scan
             BEGIN SELECT RAISE(ABORT, 'interrupted'); END;",
        )
        .unwrap();
        let k = drained(home.path(), &raw);
        assert!(run(home.path(), &raw, &k, &mut None, &|| false).is_err());
        db.execute_batch("DROP TRIGGER interrupted").unwrap();
        let kept: i64 =
            (db.query_row("SELECT count(*) FROM subagent_scan", [], |r| r.get(0))).unwrap();
        assert_eq!(kept, 0);
        let mut raw = rebuilt_below(home.path(), raw, write);
        drop(drained(home.path(), &raw));
        let new = raw
            .append(&sub_call("s1", "a1", &w.join("new.pdf"), w))
            .unwrap();
        assert_eq!(new, write, "the seq is given again");
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        while step(home.path(), &raw) {}
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert!(titles.contains(&"new.pdf".to_owned()), "{titles:?}");
    }

    /// Slice 3: a stop stepped aside between its files is taken whole next round, no row twice;
    /// a long run of calls is read with an eye on a step aside too, the stop then waiting for the
    /// next round below its checkpoint.
    #[test]
    fn a_stop_stepped_aside_is_taken_again_whole() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["one.pdf", "two.pdf"]);
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        said(&mut raw, "s1", "reply", w);
        for f in ["one.pdf", "two.pdf"] {
            raw.append(&sub_call("s1", "a1", &w.join(f), w)).unwrap();
        }
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        // Two git checks, then a look before each file: the second file is not reached.
        let asked = std::cell::Cell::new(0);
        let aside = || {
            asked.set(asked.get() + 1);
            asked.get() > 3
        };
        let k = drained(home.path(), &raw);
        assert!(run(home.path(), &raw, &k, &mut None, &aside).unwrap());
        assert_eq!(copies(home.path()).len(), 1);
        while step(home.path(), &raw) {}
        let titles: Vec<String> = copies(home.path()).into_iter().map(|c| c.0).collect();
        assert_eq!(titles, ["one.pdf", "two.pdf"]);
        // Three hundred reads in a gap: asked to step aside, the stop is not taken.
        said(&mut raw, "s2", "prompt", w);
        raw.append(&launch_of("s2", "b1", true, w)).unwrap();
        said(&mut raw, "s2", "reply", w);
        for _ in 0..300 {
            let mut read = sub_call("s2", "b1", &w.join("one.pdf"), w);
            read.body = read.body.replace("\"Write\"", "\"Read\"");
            raw.append(&read).unwrap();
        }
        let stop = raw.append(&stop_of("s2", "b1", w)).unwrap();
        let k = drained(home.path(), &raw);
        // The phase reads `BATCH` records a round: the stop is in the second.
        for _ in 0..3 {
            assert!(run(home.path(), &raw, &k, &mut None, &|| true).unwrap());
        }
        let at = || {
            checkpoint(&open(home.path()).unwrap(), raw.device())
                .unwrap()
                .unwrap()
                .seq
        };
        assert!(at() < stop);
        while step(home.path(), &raw) {}
        assert_eq!(at(), stop);
    }

    /// Slice 3: a stop's row is denied from the registration of a forget of its gap's reply, as of
    /// its call, before any purge; a forget of the stop or of the launch alone denies none, the
    /// copy having been made from a public launch's calls.
    #[test]
    fn a_stops_row_is_denied_by_its_reply_not_by_its_stop_or_launch() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        let cases = ["reply", "stop", "launch"];
        files(w, &["reply.pdf", "stop.pdf", "launch.pdf"]);
        let mut seqs = HashMap::new();
        for case in cases {
            said(&mut raw, case, "prompt", w);
            let launch = raw.append(&launch_of(case, case, true, w)).unwrap();
            let reply = said(&mut raw, case, "reply", w);
            let file = w.join(format!("{case}.pdf"));
            raw.append(&sub_call(case, case, &file, w)).unwrap();
            let stop = raw.append(&stop_of(case, case, w)).unwrap();
            seqs.insert(case, [reply, stop, launch]);
        }
        step(home.path(), &raw);
        let mut raw = raw;
        for (i, case) in cases.iter().enumerate() {
            raw = forget_record(home.path(), raw, seqs[case][i], false);
        }
        let db = open(home.path()).unwrap();
        let denied: Vec<(String, bool)> = (rows(&db).unwrap().into_iter())
            .map(|r| (r.title.clone(), raw.denies(&r).unwrap()))
            .collect();
        let expected: Vec<(String, bool)> = vec![
            ("reply.pdf".into(), true),
            ("stop.pdf".into(), false),
            ("launch.pdf".into(), false),
        ];
        assert_eq!(denied, expected);
    }

    /// Slice 3: a taken stop whose id a mask takes since still bounds the next stop, though a
    /// source changed since.
    #[test]
    fn a_taken_stop_masked_since_still_bounds_the_next() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["one.pdf", "two.pdf"]);
        said(&mut raw, "s1", "prompt", w);
        raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        let reply = said(&mut raw, "s1", "reply", w);
        let one = raw
            .append(&sub_call("s1", "a1", &w.join("one.pdf"), w))
            .unwrap();
        let first = raw.append(&stop_of("s1", "a1", w)).unwrap();
        step(home.path(), &raw);
        let device = raw.device().to_owned();
        let body = match &raw.after(&device, first - 1, 1).unwrap()[0].item {
            Item::Event(e) => e.body.clone(),
            _ => panic!("an event"),
        };
        let at = body.find("\"a1\"").unwrap() + 1;
        let target = crate::raw::Target::Range {
            device,
            seq: first,
            offset: at as i64,
            length: 2,
        };
        raw.append_tombstone(target).unwrap();
        std::fs::write(w.join("one.pdf"), b"changed since").unwrap();
        let two = raw
            .append(&sub_call("s1", "a1", &w.join("two.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        while step(home.path(), &raw) {}
        let expected: Vec<(String, i64, i64)> = vec![
            ("one.pdf".into(), reply, one),
            ("two.pdf".into(), reply, two),
        ];
        assert_eq!(origins(home.path()), expected);
    }

    /// Slice 3: a forget of the launch registered between a stop's copy and its row leaves no
    /// row: the row is written under the read fence once the launch is looked at again.
    #[test]
    fn a_launch_forgotten_while_its_stops_file_is_copied_leaves_no_row() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        files(w, &["late.pdf"]);
        said(&mut raw, "s1", "prompt", w);
        let launch = raw.append(&launch_of("s1", "a1", true, w)).unwrap();
        said(&mut raw, "s1", "reply", w);
        raw.append(&sub_call("s1", "a1", &w.join("late.pdf"), w))
            .unwrap();
        raw.append(&stop_of("s1", "a1", w)).unwrap();
        let k = drained(home.path(), &raw);
        let (dir, device) = (home.path().to_owned(), raw.device().to_owned());
        let mut once = true;
        let act = move |stage| {
            if stage == Stage::Row && std::mem::take(&mut once) {
                let target = crate::forget::Target::Record {
                    device: device.clone(),
                    seq: launch,
                };
                let preview = crate::forget::preview(&dir, target).unwrap();
                crate::forget::start(&dir, &preview).unwrap();
            }
        };
        with_seam(act, || {
            run(home.path(), &raw, &k, &mut None, &|| false).unwrap();
        });
        assert!(copies(home.path()).is_empty());
    }
}
