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
  device TEXT PRIMARY KEY, seq INTEGER NOT NULL, ts INTEGER
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
    for file in [DB, "deliverables.db-wal", "deliverables.db-shm"] {
        crate::db::private(&home.join(file), 0o600);
    }
    Ok(conn)
}

/// The wholly private prompt's turn of one session (docs/deliverables.md): its flag is a file under
/// `state/private-turns/`, set by the prompt hook and cleared by the session's next public prompt,
/// its reply and its end. It holds no text.
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
    let Some(dir) = path.parent().filter(|d| d.is_dir()) else {
        return false;
    };
    let top = seen
        .entry(dir.to_owned())
        .or_insert_with(|| {
            let found = run_bounded(
                std::process::Command::new("git")
                    .args(["rev-parse", "--show-toplevel"])
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
                std::process::Command::new("git")
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

/// A folder under `deliverables/` made or found as a real directory, never through a link.
fn real_dir(home: &Path, folder: &str) -> Result<PathBuf> {
    let top = home.join(FOLDER);
    for dir in [top.clone(), top.join(folder)] {
        match std::fs::symlink_metadata(&dir) {
            Ok(m) => anyhow::ensure!(m.is_dir(), "{} is not a real folder", dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&dir)?,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(top.join(folder))
}

/// The sha256 of an existing copy of `len` bytes, opened without blocking or following a link and
/// refused unless a regular file, read to one byte past `len` at most: a FIFO, a link or a file of
/// any other size put under a copy's name holds nothing up (None for another size).
fn sha256_file(path: &Path, len: u64) -> Result<Option<String>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
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
    let dir = real_dir(home, folder)?;
    let mut nonce = [0u8; 6];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("{e}"))?;
    let partial = dir.join(format!(".partial-{}-{}", std::process::id(), hex(&nonce)));
    let result = (|| -> Result<Copied> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            // A copy is the owner's alone, whatever its source's mode.
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options.open(&partial)?;
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
            let path = dir.join(&name);
            match std::fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::rename(&partial, &path)?;
                    return Ok(Copied {
                        copy: format!("{folder}/{name}"),
                        bytes,
                        sha256,
                    });
                }
                Ok(m) if m.is_file() && sha256_file(&path, bytes)?.as_ref() == Some(&sha256) => {
                    std::fs::remove_file(&partial)?;
                    return Ok(Copied {
                        copy: format!("{folder}/{name}"),
                        bytes,
                        sha256,
                    });
                }
                // Another file under the short name: the whole hash's.
                Ok(_) => {}
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("both names are taken by other files")
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// The phase's last turn end on this device and that record's time.
fn checkpoint(db: &Connection, device: &str) -> Result<Option<(i64, Option<i64>)>> {
    Ok(db
        .query_row(
            "SELECT seq, ts FROM deliverable_scan WHERE device = ?1",
            [device],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

fn set_checkpoint(db: &Connection, device: &str, seq: i64, ts: Option<i64>) -> Result<()> {
    db.execute(
        "INSERT INTO deliverable_scan(device, seq, ts) VALUES(?1, ?2, ?3)
         ON CONFLICT(device) DO UPDATE SET seq = excluded.seq, ts = excluded.ts",
        params![device, seq, ts],
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
/// it or gives its seq to another record. A record with a time is known by it; one with none (a
/// tombstone, a removed one) is known by having none; one forget removed since keeps its seq; the
/// empty store's checkpoint (seq 0) names none.
fn still_there(raw: &Raw, seq: i64, ts: Option<i64>) -> Result<bool> {
    if seq == 0 {
        return Ok(true);
    }
    let at = raw
        .after(raw.device(), seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == seq);
    Ok(match (ts, at.map(|r| r.item)) {
        (_, None) => false,
        (Some(ts), Some(Item::Event(e) | Item::Expired(e))) => e.ts == ts,
        (None, Some(Item::Event(_) | Item::Expired(_))) => false,
        (_, Some(_)) => true,
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
/// the copy rather than allows it.
fn candidates(events: &[(i64, Event)], aside: &dyn Fn() -> bool) -> Option<Vec<Candidate>> {
    let mut declared_list: Vec<Candidate> = Vec::new();
    let mut writes: Vec<(PathBuf, (i64, i64), PathBuf)> = Vec::new();
    let mut prompted = false;
    for (seq, e) in events {
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
        let call = (*seq, e.ts);
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
            return None;
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
    Some(out)
}

/// The phase (docs/deliverables.md): each turn end recorded on this device since its checkpoint,
/// copied from. True when it took one; it stops when a command asks the worker to step aside, the
/// turn then taken again whole (names are by content and rows unique). `db` is opened when first
/// needed.
pub(crate) fn run(
    home: &Path,
    raw: &Raw,
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
        Some((seq, ts)) if still_there(raw, seq, ts)? => seq,
        // The first run, or a raw.db restored below the checkpoint: nothing older is copied.
        _ => {
            set_checkpoint(db, &device, newest, time_at(raw, newest)?)?;
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
        if !matches!(e.kind.as_str(), "reply" | "end")
            || e.source != "hook"
            || !AGENTS.contains(&e.agent.as_str())
        {
            continue;
        }
        let start = raw.hook_turn_start(&e.agent, &e.session, r.seq)?;
        let events = raw.turn_events(&e.agent, &e.session, start - 1, r.seq)?;
        let folder = folder_name(
            e.repo
                .as_deref()
                .and_then(|k| k.rsplit(['/', '\\']).find(|p| !p.is_empty()))
                .unwrap_or(""),
        );
        // At most `MAX_FILES` a turn, of those that open as a file inside a root, as claude-mem
        // counts the ones that resolve.
        let mut opened = 0;
        let Some(found) = candidates(&events, aside) else {
            return Ok(true);
        };
        for c in found {
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
                    r.seq,
                    e.ts,
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
        set_checkpoint(db, &device, r.seq, Some(e.ts))?;
        took = true;
    }
    let (seq, _) = checkpoint(db, &device)?.unwrap_or((from, None));
    if seq < last {
        set_checkpoint(db, &device, last, time_at(raw, last)?)?;
    }
    // Records read past is progress too: more may wait behind them.
    Ok(took || last > from)
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
        // Only a real folder is read: a link at `deliverables/` leads elsewhere.
        let top = home.join(FOLDER);
        if !std::fs::symlink_metadata(&top).is_ok_and(|m| m.is_dir()) {
            return Ok(removed);
        }
        let Ok(folders) = std::fs::read_dir(&top) else {
            return Ok(removed);
        };
        for folder in folders.flatten() {
            if !folder.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = folder.file_name().to_string_lossy().into_owned();
            for file in std::fs::read_dir(folder.path())?.flatten() {
                let copy = format!("{name}/{}", file.file_name().to_string_lossy());
                if named.contains(&copy) {
                    continue;
                }
                match remove_copy(home, &copy) {
                    Ok(true) => removed += 1,
                    Ok(false) => {}
                    Err(e) => eprintln!("oboete: {copy} was not removed: {e:#}"),
                }
            }
        }
    }
    Ok(removed)
}

/// Removes one copy, only a regular file directly in a folder of `deliverables/`, no link
/// followed.
fn remove_copy(home: &Path, copy: &str) -> Result<bool> {
    // A name no copy is given is refused, never taken as gone: its row stays.
    let parts = copy.split_once('/').filter(|(folder, name)| {
        [folder, name]
            .iter()
            .all(|p| !p.is_empty() && **p != "." && **p != ".." && !p.contains(['/', '\\']))
    });
    let Some((folder, name)) = parts else {
        anyhow::bail!("not a copy's name");
    };
    // A folder or a file that cannot be read is an error, so its row stays for the next sweep;
    // one that is not there is gone already.
    let meta = |p: &Path| match std::fs::symlink_metadata(p) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    };
    let top = home.join(FOLDER);
    let dir = top.join(folder);
    for d in [&top, &dir] {
        if !meta(d)?.is_some_and(|m| m.is_dir()) {
            return Ok(false);
        }
    }
    let path = dir.join(name);
    match meta(&path)? {
        Some(m) if m.is_file() => {
            std::fs::remove_file(&path)?;
            Ok(true)
        }
        _ => Ok(false),
    }
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
        assert!(!run(home.path(), &raw, &mut None, &|| false).unwrap());
        (home, raw)
    }

    fn step(home: &Path, raw: &Raw) -> bool {
        run(home, raw, &mut None, &|| false).unwrap()
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
    /// not copied; the next public prompt's is.
    #[test]
    fn a_private_turns_files_are_not_copied() {
        let (home, mut raw) = ready();
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("plan.pdf"), b"plan").unwrap();
        std::fs::write(w.join("public.pdf"), b"public").unwrap();
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
            "cleared by the reply"
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

    /// An empty Stop records no reply and leaves the flag set: its turn may go on.
    #[test]
    fn an_empty_stop_leaves_the_private_turn_set() {
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
        hook(&mut raw, "Stop", json!({"last_assistant_message": ""}));
        assert!(PrivateTurn::new(home.path(), "claude", "s1").is_set());
        hook(&mut raw, "Stop", json!({"last_assistant_message": "Done."}));
        assert!(!PrivateTurn::new(home.path(), "claude", "s1").is_set());
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
    /// the checkpoint had none is another one.
    #[test]
    fn a_checkpoint_knows_its_record() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        assert!(still_there(&raw, 0, None).unwrap());
        let seq = raw
            .append(&ev("s1", "prompt", json!({"prompt": "Go."}), home.path()))
            .unwrap();
        let ts = time_at(&raw, seq).unwrap();
        assert!(still_there(&raw, seq, ts).unwrap());
        assert!(!still_there(&raw, seq, None).unwrap());
        assert!(!still_there(&raw, seq, Some(1)).unwrap());
        assert!(!still_there(&raw, seq + 1, ts).unwrap());
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
        let dir = real_dir(home.path(), "shop").unwrap();
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
            set_checkpoint(&db, raw.device(), seq, Some(1)).unwrap();
            turn(&mut raw, "s1", w, &write());
            assert!(!step(home.path(), &raw), "{ahead}");
            assert!(copies(home.path()).is_empty(), "{ahead}");
            let at = checkpoint(&db, raw.device()).unwrap().map(|c| c.0);
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
        assert!(run(home.path(), &raw, &mut None, &|| true).unwrap());
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
}
