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
    for (n, event) in events.into_iter().enumerate() {
        // A long run of records is read with an eye on a step aside.
        if n % 256 == 255 && aside() {
            return Ok(None);
        }
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
    let on = match crate::config::load_capture(home) {
        Ok(c) => c.capture.deliverables,
        // A config.toml that does not read decides nothing: the checkpoint stays.
        Err(_) => return Ok(false),
    };
    if !on && db.is_none() && !home.join(DB).exists() {
        return Ok(false);
    }
    let db = match db {
        Some(db) => db,
        None => db.insert(open(home)?),
    };
    let device = raw.device().to_owned();
    let newest = raw.max_seq()?;
    // Off, the checkpoint follows the newest record, as the first run puts it: what is recorded
    // while it is off is not copied once it is on again (no history is copied).
    if !on {
        if checkpoint(db, &device)?.is_some_and(|c| c.seq < newest) {
            db.execute("DELETE FROM subagent_scan WHERE device = ?1", [&device])?;
            set_checkpoint(db, raw, newest, time_at(raw, newest)?)?;
        }
        return Ok(false);
    }
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
                Some((agent_id, launch)) => {
                    let read = crate::dispatch::reading(home, crate::db::OPEN_WRITE_WAIT)?;
                    let forgets = |(seq, ts): (i64, i64)| {
                        raw.forgets(&device, &e.agent, &e.session, e.repo.as_deref(), seq, ts)
                    };
                    if !launched_public(raw, (&e.agent, &e.session, agent_id), *launch)?
                        || forgets(c.call)?
                        || forgets(turn)?
                    {
                        continue;
                    }
                    #[cfg(test)]
                    seam(Stage::Fenced);
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
        Some(&seq) if launched_public(raw, who, seq)? => seq,
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

/// Whether the call at `seq` launched subagent `agent_id` and is seen public now: a hook's record
/// as raw reads it (a forget's registration tombstones it at once) that still names the subagent
/// (a mask of its id since names none), marked public, after a prompt or a harness message of its
/// own turn.
fn launched_public(
    raw: &Raw,
    (agent, session, agent_id): (&str, &str, &str),
    seq: i64,
) -> Result<bool> {
    let Some(launch) = raw.event_at(seq)?.filter(|x| x.source == "hook") else {
        return Ok(false);
    };
    let names = (crate::consumer::subagents::facts(&launch).iter())
        .any(|f| f.fact == "launch" && f.agent_id == agent_id);
    let marked =
        serde_json::from_str::<Value>(&launch.body).is_ok_and(|b| b["public_turn"] == true);
    Ok(names && marked && prompted_before(raw, agent, session, seq)?)
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
    /// A stop's row looked at again under the read fence, not yet written.
    Fenced,
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
mod tests;
