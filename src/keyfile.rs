//! A key typed on the settings page, registered in a new managed file or written into its
//! existing chain entry's key file (#94, part 3): line 2
//! of a file named `*_KEY.md` outside the oboete home, on Linux, on a filesystem known to enforce a
//! Unix mode for every local user. Everything else in the file stays as it was, and the key goes
//! nowhere but that file.
//! macOS and Windows are refused until a file there can be made owner-only from its first instant
//! (#281): a folder's inherited ACL entries would reach it through a 0600 mode.

// Off Linux only the refusal and the tests' pure helpers are used (#281).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

/// Why a key was not written; `code` is what the page puts in words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Refused {
    BadKey,
    NotAbsolute,
    NotAKeyFile,
    Protected,
    NoDir,
    NotAFile,
    TooBig,
    NotUtf8,
    NotPrivate,
    /// Another local user could replace a folder on the key file's path (#285).
    SharedDir,
    Changed,
    // Made off Linux only (#281).
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Unsupported,
    Failed,
}

impl Refused {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Refused::BadKey => "bad_key",
            Refused::NotAbsolute => "not_absolute",
            Refused::NotAKeyFile => "not_a_key_file",
            Refused::Protected => "protected",
            Refused::NoDir => "no_dir",
            Refused::NotAFile => "not_a_file",
            Refused::TooBig => "too_big",
            Refused::NotUtf8 => "not_utf8",
            Refused::NotPrivate => "not_private",
            Refused::SharedDir => "shared_folder",
            Refused::Changed => "changed",
            Refused::Unsupported => "unsupported",
            Refused::Failed => "failed",
        }
    }

    pub(crate) fn status(self) -> u16 {
        match self {
            Refused::Changed => 409,
            Refused::Failed => 500,
            _ => 422,
        }
    }
}

/// A key is in its file. `durable` is false when the folder could not be synced after the rename:
/// the key is in place, but a power loss could still undo it.
#[derive(Debug, PartialEq)]
pub(crate) struct Written {
    pub durable: bool,
}

/// A newly registered key. Retain it only after its config reference is committed; dropping it
/// first removes only the newly created file while its device/inode still match. `durable` is
/// false when a directory sync failed, even though the file itself was synced.
#[derive(Debug)]
pub(crate) struct Managed {
    path: PathBuf,
    pub durable: bool,
    file: Option<std::fs::File>,
}

impl Managed {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn retain(mut self) {
        self.file.take();
    }
}

impl Drop for Managed {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(file) = &self.file {
            linux::discard(&self.path, file);
        }
    }
}

/// Registers a fresh key outside the corpus. Owner/data locations come from the trusted process
/// caller, never a client's filesystem path. Existing key files are neither read nor replaced.
/// Managed storage also needs kernel mount observations proving physical separation. Unproven
/// backing trees cannot provide that proof, so use a safe native fallback or refuse.
pub(crate) fn managed(
    key: &str,
    corpus_home: &Path,
    owner_home: &Path,
    local_data: Option<&Path>,
) -> Result<Managed, Refused> {
    if !valid(key) {
        return Err(Refused::BadKey);
    }
    #[cfg(target_os = "linux")]
    return linux::managed(key, corpus_home, owner_home, local_data);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (corpus_home, owner_home, local_data);
        Err(Refused::Unsupported)
    }
}

const KEY_LEN: RangeInclusive<usize> = 8..=512;
/// A key file holds a few lines.
const MAX_FILE: u64 = 64 * 1024;
/// A new file's line 1.
const TITLE: &str = "API key (oboete)";
/// Filesystems whose Unix mode the kernel enforces for every local user, by their `statfs` magic.
/// Any other is refused, as a 0600 there may not keep the key to its owner: a Windows drive under
/// WSL (9p), FUSE (sshfs with `allow_other` shows 0600 and lets every local user read), network
/// shares, FAT, exFAT and NTFS among them, and eCryptfs, which leaves the check to the filesystem
/// under it.
// The magic controls mode enforcement; the names identify provable native mount coordinates.
// Overlay enforces modes but exposes no complete backing-tree coordinates, so has no names.
const PRIVATE_FS: &[(u32, &[&[u8]])] = &[
    (0xEF53, &[b"ext2", b"ext3", b"ext4"]),
    (0x5846_5342, &[b"xfs"]),
    (0x9123_683E, &[b"btrfs"]),
    (0xF2F5_2010, &[b"f2fs"]),
    (0x2FC1_2FC1, &[b"zfs"]),
    (0xCA45_1A4E, &[b"bcachefs"]),
    (0x0102_1994, &[b"tmpfs"]),
    (0x794C_7630, &[]),
];

/// 8 to 512 characters of letters, digits and `._~+/=:-`: every provider's keys, and no quote,
/// backslash, space or control character, so a key can add no line and no TOML string.
pub(crate) fn valid(key: &str) -> bool {
    KEY_LEN.contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~+/=:-".contains(&b))
}

/// Whether `file` is on a filesystem in `PRIVATE_FS`, where a mode keeps it its owner's: the
/// resident viewer's token is kept only there (docs/resident.md R6). Not yet off Linux (#281).
pub(crate) fn private_fs(file: &std::fs::File) -> bool {
    #[cfg(target_os = "linux")]
    return linux::private(file);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        false
    }
}

/// The filesystem magic `private_fs` sees on this thread instead of the real one; `None`, the
/// real one.
#[cfg(test)]
pub(crate) fn fake_fs(magic: Option<u32>) {
    FS.with(|f| f.set(magic));
}

/// Writes `key` as line 2 of `path`, the key file a chain entry names.
pub(crate) fn write(path: &Path, key: &str, home: &Path) -> Result<Written, Refused> {
    if !valid(key) {
        return Err(Refused::BadKey);
    }
    #[cfg(target_os = "linux")]
    return linux::write(path, key, home);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, home);
        Err(Refused::Unsupported)
    }
}

/// The file a key may be written to, and its folder: `path` as the config names it, when it is
/// absolute, named `*_KEY.md`, and in a folder that exists outside the oboete home (which holds
/// config.toml and the stores).
fn destination(path: &Path, home: &Path) -> Result<(PathBuf, PathBuf), Refused> {
    if !path.is_absolute() {
        return Err(Refused::NotAbsolute);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.ends_with("_KEY.md"))
        .ok_or(Refused::NotAKeyFile)?;
    let parent = path.parent().ok_or(Refused::NoDir)?;
    // Check before resolving links/`..`, which can hide an unsafe part of the configured path.
    #[cfg(target_os = "linux")]
    linux::check_dirs(parent)?;
    let dir = parent
        .canonicalize()
        .ok()
        .filter(|p| p.is_dir())
        .ok_or(Refused::NoDir)?;
    #[cfg(target_os = "linux")]
    linux::check_dirs(&dir)?;
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    if dir.starts_with(&home) {
        return Err(Refused::Protected);
    }
    Ok((dir.join(name), dir))
}

/// `old` with `key` as its line 2: the bytes before and after it as they were, with the file's
/// line ending (CRLF when its first line ends so). A file of one line gets a second; line 2 that
/// ends the file without a line break is replaced through the end; a new or empty file gets a
/// title.
fn with_key(old: Option<&[u8]>, key: &str) -> Vec<u8> {
    let key = key.as_bytes();
    let Some(old) = old.filter(|o| !o.is_empty()) else {
        return [TITLE.as_bytes(), b"\n", key, b"\n"].concat();
    };
    let Some(first) = old.iter().position(|&b| b == b'\n') else {
        return [old, b"\n", key, b"\n"].concat();
    };
    let eol: &[u8] = if first > 0 && old[first - 1] == b'\r' {
        b"\r\n"
    } else {
        b"\n"
    };
    let rest = &old[first + 1..];
    match rest.iter().position(|&b| b == b'\n') {
        // Line 2's text ends before its line ending, a "\r" included.
        Some(second) => {
            let end = if second > 0 && rest[second - 1] == b'\r' {
                second - 1
            } else {
                second
            };
            [&old[..=first], key, &rest[end..]].concat()
        }
        None => [&old[..=first], key, eol].concat(),
    }
}

/// Kernel mount observations supplied at the filesystem boundary in tests.
#[cfg(test)]
struct MountFixture {
    table: Vec<u8>,
    ids: Vec<(PathBuf, u64)>,
}

/// The steps a failure can be injected at, in tests.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Stage,
    Write,
    Sync,
    Rename,
    DirSync,
}

#[cfg(test)]
thread_local! {
    static FAIL: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
    static MOUNTS: std::cell::RefCell<Option<MountFixture>> = const { std::cell::RefCell::new(None) };
    /// Deterministic entropy for the managed filename's collision test.
    static MANAGED_TAG: std::cell::Cell<Option<[u8; 16]>> = const { std::cell::Cell::new(None) };
    /// The filesystem magic `private` sees instead of the real one.
    static FS: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
    /// A folder on the way to the key folder, and the filesystem magic the walk sees for it.
    static FS_AT: std::cell::RefCell<Option<(PathBuf, u32)>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_CHECK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

fn step(at: Step) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL.with(|f| f.get()) == Some(at) {
        return Err(std::io::Error::other("injected"));
    }
    let _ = at;
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

    /// No other user may replace a part of the path to the key folder (#285). The path is
    /// resolved as the kernel resolves it, one name at a time from the root, and each folder and
    /// each link is checked where it is found, so a parent that passed protects the check of what
    /// it holds. A trusted sticky folder on the way (e.g. /tmp) protects a trusted entry, but the
    /// key folder itself must not let other users create or replace key-file names.
    pub(super) fn check_dirs(path: &Path) -> Result<(), Refused> {
        // SAFETY: geteuid has no arguments and cannot fail.
        let uid = unsafe { libc::geteuid() };
        let mut at = PathBuf::new();
        walk(path, &mut at, &mut 0, uid)?;
        if entry(&at)?.mode() & 0o022 != 0 {
            return Err(Refused::SharedDir);
        }
        Ok(())
    }

    /// What `path` itself is (a link is not followed), read from the entry while it is held open
    /// and only on a filesystem in `PRIVATE_FS`: one that enforces no Unix mode, or whose answers
    /// a program makes up (a FUSE mount another user made), can show a folder as this user's,
    /// 0700, and then turn it into a link.
    fn entry(path: &Path) -> Result<std::fs::Metadata, Refused> {
        let held = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| Refused::NoDir)?;
        let fs = magic(&held);
        #[cfg(test)]
        let fs = FS_AT.with(|f| match &*f.borrow() {
            Some((at, fake)) if at == path => Some(*fake),
            _ => fs,
        });
        if !fs.is_some_and(|m| PRIVATE_FS.iter().any(|(magic, _)| *magic == m)) {
            return Err(Refused::NotPrivate);
        }
        held.metadata().map_err(|_| Refused::NoDir)
    }

    /// Follows `path` from `at`, the folder resolved so far (no link is left in it), and leaves
    /// `at` where `path` ends. A link's target is followed from the folder the link is in, name
    /// by name like the path itself: asking the kernel about `hop/` would follow `hop` without
    /// showing the folders behind it. `links` counts each link once, up to the kernel's 40.
    fn walk(path: &Path, at: &mut PathBuf, links: &mut u8, uid: u32) -> Result<(), Refused> {
        use std::path::Component;
        for part in path.components() {
            let next = match part {
                Component::Prefix(_) => return Err(Refused::NoDir),
                Component::CurDir => continue,
                Component::ParentDir => {
                    // `at` holds no link, so its parent is the folder `..` names.
                    at.pop();
                    continue;
                }
                Component::RootDir => PathBuf::from("/"),
                Component::Normal(name) => at.join(name),
            };
            let entry = entry(&next)?;
            // In a sticky parent a link's owner matters as well as its target's owner.
            if ![0, uid].contains(&entry.uid()) {
                return Err(Refused::SharedDir);
            }
            if entry.file_type().is_symlink() {
                *links += 1;
                if *links > 40 {
                    return Err(Refused::NoDir);
                }
                let target = std::fs::read_link(&next).map_err(|_| Refused::NoDir)?;
                walk(&target, at, links, uid)?;
                continue;
            }
            if !entry.is_dir() {
                return Err(Refused::NoDir);
            }
            if entry.mode() & 0o022 != 0 && entry.mode() & 0o1000 == 0 {
                return Err(Refused::SharedDir);
            }
            *at = next;
        }
        Ok(())
    }

    /// Resolve and guard the existing prefix before creating any missing directories. Reject
    /// `..` in managed roots: unlike legacy destinations these are process-selected locations.
    fn planned_dir(path: &Path) -> Result<(PathBuf, PathBuf), Refused> {
        if !path.is_absolute() {
            return Err(Refused::NotAbsolute);
        }
        if path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(Refused::NoDir);
        }
        let mut existing = path;
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(existing) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(existing.file_name().ok_or(Refused::NoDir)?.to_owned());
                    existing = existing.parent().ok_or(Refused::NoDir)?;
                }
                Err(_) => return Err(Refused::NoDir),
            }
        }
        let mut base = PathBuf::new();
        // SAFETY: geteuid has no arguments and cannot fail.
        walk(existing, &mut base, &mut 0, unsafe { libc::geteuid() })?;
        let mut dest = base.clone();
        for name in missing.into_iter().rev() {
            dest.push(name);
        }
        Ok((dest, base))
    }

    /// A mount's root is relative to its filesystem, not its mount point. This distinction
    /// detects binds of corpus descendants even when no visible ancestor is the corpus inode.
    struct Mount {
        id: u64,
        device: (u64, u64),
        root: PathBuf,
        at: PathBuf,
        native_coordinates: bool,
    }

    /// Translate native filesystem names to the existing mode-enforcement policy. Layered and
    /// userspace views (including overlay) do not expose sufficient backing-tree coordinates.
    fn native_coordinates(kind: &[u8]) -> bool {
        PRIVATE_FS.iter().any(|(_, names)| names.contains(&kind))
    }

    const MAX_MOUNTINFO: u64 = 1024 * 1024;

    fn proc_bytes(path: &Path, limit: u64) -> Result<Vec<u8>, Refused> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| Refused::Protected)?;
        // These observations must come from the kernel, not a substituted ordinary file.
        if magic(&file) != Some(0x9fa0) {
            return Err(Refused::Protected);
        }
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Refused::Protected)?;
        if bytes.len() as u64 > limit {
            return Err(Refused::Protected);
        }
        Ok(bytes)
    }

    fn mount_number(bytes: &[u8]) -> Result<u64, Refused> {
        if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
            return Err(Refused::Protected);
        }
        std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or(Refused::Protected)
    }

    /// mountinfo escapes space, tab, newline and backslash in its path fields. Preserve all
    /// other bytes, including non-UTF-8 names, instead of treating them as separators.
    fn mount_path(bytes: &[u8]) -> Result<PathBuf, Refused> {
        let mut path = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] == b'\\' {
                let escaped = bytes.get(at + 1..at + 4).ok_or(Refused::Protected)?;
                path.push(match escaped {
                    b"040" => b' ',
                    b"011" => b'\t',
                    b"012" => b'\n',
                    b"134" => b'\\',
                    _ => return Err(Refused::Protected),
                });
                at += 4;
            } else {
                path.push(bytes[at]);
                at += 1;
            }
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(path));
        if path.as_os_str().as_bytes().contains(&0)
            || path
                .components()
                .any(|p| p == std::path::Component::ParentDir)
        {
            return Err(Refused::Protected);
        }
        Ok(path)
    }

    fn mount_table() -> Result<Vec<Mount>, Refused> {
        #[cfg(test)]
        let supplied = MOUNTS.with(|f| f.borrow().as_ref().map(|f| f.table.clone()));
        #[cfg(not(test))]
        let supplied: Option<Vec<u8>> = None;
        let bytes = match supplied {
            Some(bytes) => bytes,
            None => proc_bytes(Path::new("/proc/self/mountinfo"), MAX_MOUNTINFO)?,
        };
        if bytes.len() as u64 > MAX_MOUNTINFO {
            return Err(Refused::Protected);
        }
        let mut mounts = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for line in bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
            let fields: Vec<_> = line.split(|&b| b == b' ').collect();
            let separator = fields
                .iter()
                .position(|&f| f == b"-")
                .ok_or(Refused::Protected)?;
            if separator < 6
                || fields.len() != separator + 4
                || fields.iter().any(|f| f.is_empty())
                || mounts.len() == 4096
            {
                return Err(Refused::Protected);
            }
            let id = mount_number(fields[0])?;
            mount_number(fields[1])?;
            let device = fields[2].split(|&b| b == b':').collect::<Vec<_>>();
            if device.len() != 2 || !ids.insert(id) {
                return Err(Refused::Protected);
            }
            let at = mount_path(fields[4])?;
            if !at.is_absolute() {
                return Err(Refused::Protected);
            }
            // nsfs and other pseudo-filesystems can have opaque roots such as `net:[id]`.
            // Retain unrelated records; a selected/protected root must still be a real path.
            mounts.push(Mount {
                id,
                device: (mount_number(device[0])?, mount_number(device[1])?),
                root: mount_path(fields[3])?,
                at,
                native_coordinates: native_coordinates(fields[separator + 1]),
            });
        }
        Ok(mounts)
    }

    fn mount_position<'a>(
        dest: &Path,
        base: &Path,
        folder: &std::fs::File,
        mounts: &'a [Mount],
    ) -> Result<(&'a Mount, PathBuf, PathBuf), Refused> {
        // The kernel's spelling also avoids deriving physical coordinates from user casing.
        let visible = std::fs::read_link(format!("/proc/self/fd/{}", folder.as_raw_fd()))
            .map_err(|_| Refused::Protected)?;
        if !visible.is_absolute() || visible.as_os_str().as_bytes().ends_with(b" (deleted)") {
            return Err(Refused::Protected);
        }
        #[cfg(test)]
        let supplied = MOUNTS.with(|f| {
            f.borrow().as_ref().map(|f| {
                f.ids
                    .iter()
                    .filter(|(p, _)| visible.starts_with(p))
                    .max_by_key(|(p, _)| p.components().count())
                    .map(|(_, id)| *id)
                    .ok_or(Refused::Protected)
            })
        });
        #[cfg(not(test))]
        let supplied: Option<Result<u64, Refused>> = None;
        let id = match supplied {
            Some(id) => id?,
            None => {
                let bytes = proc_bytes(
                    Path::new(&format!("/proc/self/fdinfo/{}", folder.as_raw_fd())),
                    4096,
                )?;
                let text = std::str::from_utf8(&bytes).map_err(|_| Refused::Protected)?;
                let mut ids = text.lines().filter_map(|l| l.strip_prefix("mnt_id:"));
                let id = mount_number(ids.next().ok_or(Refused::Protected)?.trim().as_bytes())?;
                if ids.next().is_some() {
                    return Err(Refused::Protected);
                }
                id
            }
        };
        // An FD's actual mount ID selects the top visible mount even with stacked mount points.
        // Btrfs stat devices identify subvolumes and need not equal mountinfo's superblock device.
        let mount = mounts
            .iter()
            .find(|m| m.id == id)
            .ok_or(Refused::Protected)?;
        if !mount.root.is_absolute() {
            return Err(Refused::Protected);
        }
        let relative = visible
            .strip_prefix(&mount.at)
            .map_err(|_| Refused::Protected)?;
        let suffix = dest.strip_prefix(base).map_err(|_| Refused::Protected)?;
        Ok((
            mount,
            mount.root.join(relative).join(suffix),
            visible.join(suffix),
        ))
    }

    fn outside_corpus(
        dest: &Path,
        base: &Path,
        folder: &std::fs::File,
        corpus: &Path,
    ) -> Result<(), Refused> {
        let (home_path, home_base) = planned_dir(corpus)?;
        let home = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(&home_base)
            .map_err(|_| Refused::Protected)?;
        if !private(&home) {
            return Err(Refused::NotPrivate);
        }
        let identity = home.metadata().map_err(|_| Refused::Protected)?;
        if home_path == home_base {
            for parent in base.ancestors() {
                let meta = std::fs::metadata(parent).map_err(|_| Refused::Protected)?;
                if (meta.dev(), meta.ino()) == (identity.dev(), identity.ino()) {
                    return Err(Refused::Protected);
                }
            }
        }
        let mounts = mount_table()?;
        let (key_mount, key_root, _) = mount_position(dest, base, folder, &mounts)?;
        let (home_mount, home_root, home_visible) =
            mount_position(&home_path, &home_base, &home, &mounts)?;
        // Layered or userspace filesystems can expose independently located backing trees.
        // Managed registration therefore needs a provably native location; legacy writes and
        // PRIVATE_FS remain unchanged. Missing/ambiguous kernel observations fail closed too.
        if !key_mount.native_coordinates
            || !home_mount.native_coordinates
            || (key_mount.device == home_mount.device && key_root.starts_with(&home_root))
        {
            return Err(Refused::Protected);
        }
        for mount in mounts.iter().filter(|m| m.at.starts_with(&home_visible)) {
            // Include reported hidden submounts conservatively: missing one could expose a bind
            // of a corpus child whose filesystem differs from the corpus root's filesystem.
            if !mount.root.is_absolute()
                || !mount.native_coordinates
                || (mount.device == key_mount.device && key_root.starts_with(&mount.root))
            {
                return Err(Refused::Protected);
            }
        }
        Ok(())
    }

    fn managed_dir(path: &Path, corpus: &Path) -> Result<(PathBuf, std::fs::File, bool), Refused> {
        let (dest, mut at) = planned_dir(path)?;
        if dest.starts_with(corpus) {
            return Err(Refused::Protected);
        }
        let mut folder = std::fs::File::open(&at).map_err(|_| Refused::NoDir)?;
        if !private(&folder) {
            return Err(Refused::NotPrivate);
        }
        outside_corpus(&dest, &at, &folder, corpus)?;
        let mut durable = true;
        let missing = dest
            .strip_prefix(&at)
            .map_err(|_| Refused::NoDir)?
            .to_path_buf();
        for name in missing.components() {
            at.push(name);
            match std::fs::DirBuilder::new().mode(0o700).create(&at) {
                Ok(()) => {
                    durable &= step(Step::DirSync).and_then(|()| folder.sync_all()).is_ok();
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(Refused::NoDir),
            }
            // A concurrent creator's directory must meet the same policy, without chmod.
            check_dirs(&at)?;
            folder = private_dir(&at)?;
        }
        check_dirs(&dest)?;
        folder = private_dir(&dest)?;
        outside_corpus(&dest, &dest, &folder, corpus)?;
        Ok((dest, folder, durable))
    }

    fn private_dir(path: &Path) -> Result<std::fs::File, Refused> {
        let folder = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)
            .map_err(|_| Refused::NoDir)?;
        let meta = folder.metadata().map_err(|_| Refused::NoDir)?;
        // SAFETY: geteuid has no arguments and cannot fail.
        if !private(&folder)
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o700
        {
            return Err(Refused::NotPrivate);
        }
        Ok(folder)
    }

    /// The descriptor keeps the original inode alive until the identity comparison. A renamed
    /// or replaced path is left alone, as are all pre-existing keys.
    pub(super) fn discard(path: &Path, file: &std::fs::File) {
        let Ok(owned) = file.metadata() else { return };
        let Ok(current) = std::fs::symlink_metadata(path) else {
            return;
        };
        if current.dev() == owned.dev() && current.ino() == owned.ino() {
            let _ = std::fs::remove_file(path);
        }
    }

    pub(super) fn managed(
        key: &str,
        corpus_home: &Path,
        owner_home: &Path,
        local_data: Option<&Path>,
    ) -> Result<Managed, Refused> {
        let (corpus, _) = planned_dir(corpus_home)?;
        let data = local_data
            .map(Path::to_path_buf)
            .unwrap_or_else(|| owner_home.join(".local/share"));
        let (dir, folder, durable) = managed_dir(&data.join("oboete/keys"), &corpus)
            .or_else(|_| managed_dir(&owner_home.join(".oboete-keys"), &corpus))?;
        let mut tag = [0u8; 16];
        getrandom::fill(&mut tag).map_err(|_| Refused::Failed)?;
        #[cfg(test)]
        if let Some(fixed) = MANAGED_TAG.with(|f| f.take()) {
            tag = fixed;
        }
        let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("{tag}_KEY.md"));
        step(Step::Stage).map_err(|_| Refused::Failed)?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|_| Refused::Failed)?;
        let mut registration = Managed {
            path,
            durable,
            file: Some(file),
        };
        #[cfg(test)]
        if let Some(f) = BEFORE_WRITE.with(|b| b.borrow_mut().take()) {
            f();
        }
        let file = registration.file.as_mut().ok_or(Refused::Failed)?;
        let meta = file.metadata().map_err(|_| Refused::Failed)?;
        // SAFETY: geteuid has no arguments and cannot fail.
        if !private(file)
            || !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o600
        {
            return Err(Refused::NotPrivate);
        }
        step(Step::Write)
            .and_then(|()| file.write_all(&with_key(None, key)))
            .map_err(|_| Refused::Failed)?;
        step(Step::Sync)
            .and_then(|()| file.sync_all())
            .map_err(|_| Refused::Failed)?;
        registration.durable &= step(Step::DirSync).and_then(|()| folder.sync_all()).is_ok();
        Ok(registration)
    }

    pub(super) fn write(path: &Path, key: &str, home: &Path) -> Result<Written, Refused> {
        let (dest, dir) = destination(path, home)?;
        #[cfg(test)]
        if let Some(f) = BEFORE_WRITE.with(|b| b.borrow_mut().take()) {
            f();
        }
        let old = read(&dest)?;
        let new = with_key(old.as_deref(), key);
        let mut tag = [0u8; 8];
        getrandom::fill(&mut tag).map_err(|_| Refused::Failed)?;
        let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("KEY");
        let stage = dir.join(format!(".{name}.{tag}.tmp"));
        let staged = (|| {
            step(Step::Stage).map_err(|_| Refused::Failed)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&stage)
                .map_err(|_| Refused::Failed)?;
            // The filesystem the key would go to, asked of the staged file itself (a path could
            // name a mount that a later one hides), and the mode it kept, before the key is written.
            let mode = file
                .metadata()
                .map_err(|_| Refused::Failed)?
                .permissions()
                .mode();
            if !private(&file) || mode & 0o777 != 0o600 {
                return Err(Refused::NotPrivate);
            }
            step(Step::Write)
                .and_then(|()| file.write_all(&new))
                .map_err(|_| Refused::Failed)?;
            step(Step::Sync)
                .and_then(|()| file.sync_all())
                .map_err(|_| Refused::Failed)?;
            #[cfg(test)]
            if let Some(f) = BEFORE_CHECK.with(|b| b.borrow_mut().take()) {
                f();
            }
            // Not overwritten when it changed since it was read (not an atomic compare-and-swap:
            // a write between this read and the rename is still lost).
            if read(&dest)? != old {
                return Err(Refused::Changed);
            }
            step(Step::Rename)
                .and_then(|()| std::fs::rename(&stage, &dest))
                .map_err(|_| Refused::Failed)
        })();
        if let Err(e) = staged {
            let _ = std::fs::remove_file(&stage);
            return Err(e);
        }
        // After the rename the key is in place whatever happens here.
        let durable = step(Step::DirSync)
            .and_then(|()| std::fs::File::open(&dir))
            .and_then(|d| d.sync_all())
            .is_ok();
        Ok(Written { durable })
    }

    /// Whether `file` is on a filesystem in `PRIVATE_FS`; not when that cannot be told.
    pub(super) fn private(file: &std::fs::File) -> bool {
        let magic = magic(file);
        #[cfg(test)]
        let magic = FS.with(|f| f.get()).or(magic);
        magic.is_some_and(|m| PRIVATE_FS.iter().any(|(allowed, _)| *allowed == m))
    }

    /// The magic number of the filesystem `file` is on, when the kernel tells it.
    fn magic(file: &std::fs::File) -> Option<u32> {
        use std::os::fd::AsRawFd;
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: `fstatfs` gets an open descriptor and a buffer of its type, which it fills when
        // it returns 0; only then is the buffer read.
        if unsafe { libc::fstatfs(file.as_raw_fd(), fs.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: filled above. The magic is 32 bits, whatever the width of `f_type`.
        Some(unsafe { fs.assume_init() }.f_type as u32)
    }

    /// The key file as it is now, none when there is none: opened without following a link and
    /// without waiting on a FIFO, a regular file of at most `MAX_FILE` bytes, in UTF-8 (as
    /// `config::read_key` reads it).
    pub(super) fn read(dest: &Path) -> Result<Option<Vec<u8>>, Refused> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(dest);
        let file = match file {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Refused::NotAFile),
        };
        let meta = file.metadata().map_err(|_| Refused::NotAFile)?;
        if !meta.is_file() {
            return Err(Refused::NotAFile);
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Refused::Failed)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(Refused::TooBig);
        }
        std::str::from_utf8(&bytes).map_err(|_| Refused::NotUtf8)?;
        Ok(Some(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_letters_digits_and_a_few_signs() {
        let long = |n: usize| "k".repeat(n);
        assert!(valid(&long(8)) && valid(&long(512)));
        assert!(valid(&format!("gsk_{}", "Ab9._~+/=:-")));
        for bad in [
            String::new(),
            long(7),
            long(513),
            format!("{}\nbase_url", long(8)),
            format!("{} x", long(8)),
            format!("{}\tx", long(8)),
            format!("{}\"x", long(8)),
            format!("{}\\x", long(8)),
            format!("{}é", long(8)),
            format!("\u{feff}{}", long(8)),
        ] {
            assert!(!valid(&bad), "{bad:?}");
        }
    }

    #[test]
    fn line_2_is_replaced_and_every_other_byte_kept() {
        let with = |old: &str| String::from_utf8(with_key(Some(old.as_bytes()), "KEY")).unwrap();
        assert_eq!(with("Groq\nold\nnotes\n\n"), "Groq\nKEY\nnotes\n\n");
        assert_eq!(with("Groq\r\nold\r\nnotes"), "Groq\r\nKEY\r\nnotes");
        assert_eq!(with("Groq\nold"), "Groq\nKEY\n");
        assert_eq!(with("Groq\n"), "Groq\nKEY\n");
        assert_eq!(with("Groq\r\n"), "Groq\r\nKEY\r\n");
        assert_eq!(with("Groq"), "Groq\nKEY\n");
        assert_eq!(with("\n"), "\nKEY\n");
        assert_eq!(with(""), "API key (oboete)\nKEY\n");
        assert_eq!(
            String::from_utf8(with_key(None, "KEY")).unwrap(),
            "API key (oboete)\nKEY\n"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn managed_registration_waits_for_owner_only_storage_off_linux() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            managed(
                "key-abcdefgh",
                &root.path().join("corpus"),
                &root.path().join("owner"),
                None
            )
            .unwrap_err(),
            Refused::Unsupported
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn saving_a_key_waits_for_owner_only_files_off_linux() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GROQ_KEY.md");
        let home = dir.path().join("home");
        assert_eq!(
            write(&path, "key-abcdefgh", &home),
            Err(Refused::Unsupported)
        );
        assert!(!path.exists());
    }

    #[cfg(target_os = "linux")]
    mod on_linux {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        const KEY: &str = "canary-9f3e1c7a";

        /// A key folder and an oboete home beside it, on the test's temporary filesystem.
        fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let root = tempfile::tempdir().unwrap();
            let keys = root.path().join("keys");
            let home = root.path().join("home");
            std::fs::create_dir_all(&keys).unwrap();
            std::fs::create_dir_all(&home).unwrap();
            for dir in [root.path(), &keys, &home] {
                private(dir);
            }
            (root, keys, home)
        }

        /// Owner-only whatever the umask: under 0002 a new folder is group-writable, which the
        /// check refuses on the key file's path.
        fn private(dir: &Path) {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        fn mode(p: &Path) -> u32 {
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        }

        /// Every regular file under `dirs` that holds `text` (a FIFO is not opened).
        fn holding(dirs: &[&Path], text: &str) -> Vec<PathBuf> {
            let mut out = Vec::new();
            for d in dirs {
                for e in std::fs::read_dir(d).unwrap().flatten() {
                    let p = e.path();
                    if e.file_type().is_ok_and(|t| t.is_file())
                        && std::fs::read(&p)
                            .is_ok_and(|b| String::from_utf8_lossy(&b).contains(text))
                    {
                        out.push(p);
                    }
                }
            }
            out
        }

        /// A trusted kernel mount table and the mount IDs reported for opened fixture folders.
        /// Actual files, permissions, owner checks and writes still use the temporary filesystem.
        fn mounts(root: &Path, alias: &Path, source: &Path) {
            use std::os::unix::fs::MetadataExt;
            let dev = std::fs::metadata(root).unwrap().dev();
            let dev = format!("{}:{}", libc::major(dev), libc::minor(dev));
            let escape = |path: &Path| {
                path.to_str()
                    .unwrap()
                    .replace('\\', "\\134")
                    .replace(' ', "\\040")
                    .replace('\t', "\\011")
                    .replace('\n', "\\012")
            };
            let table = format!(
                "1 0 {dev} / / rw - ext4 none rw\n2 1 {dev} {} {} rw shared:10 - ext4 none rw\n7 1 0:999 net:[12345] /unrelated-namespace rw - nsfs nsfs rw\n",
                escape(source),
                escape(alias)
            );
            MOUNTS.with(|f| {
                *f.borrow_mut() = Some(MountFixture {
                    table: table.into_bytes(),
                    ids: vec![(root.to_path_buf(), 1), (alias.to_path_buf(), 2)],
                })
            });
        }

        #[test]
        fn managed_registration_rejects_bound_corpus_descendants_before_mkdir() {
            let (root, _keys, corpus) = setup();
            let source = corpus.join("deep/descendant\t\n\\");
            std::fs::create_dir_all(&source).unwrap();
            let alias = root.path().join("ali as\t\n\\");
            std::fs::create_dir(&alias).unwrap();
            private(&alias);
            mounts(root.path(), &alias, &source);
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert!(!alias.join("oboete").exists());
            assert!(!source.join("oboete").exists());
        }

        #[test]
        fn managed_registration_refuses_unproven_corpus_submounts_before_mkdir() {
            for kind in ["fuse.passthrough", "ecryptfs", "9p", "nfs", "unprovenfs"] {
                let (root, _keys, corpus) = setup();
                let data = root.path().join("data");
                std::fs::create_dir(&data).unwrap();
                private(&data);
                let layer = corpus.join("layer");
                std::fs::create_dir(&layer).unwrap();
                private(&layer);
                mounts(root.path(), &data, &root.path().join("separate"));
                MOUNTS.with(|f| {
                    f.borrow_mut().as_mut().unwrap().table.extend_from_slice(
                        format!(
                            "8 1 0:997 / {} rw - {kind} opaque-source rw\n",
                            layer.display()
                        )
                        .as_bytes(),
                    )
                });
                let result = managed(KEY, &corpus, root.path(), Some(&data));
                MOUNTS.with(|f| *f.borrow_mut() = None);
                assert_eq!(result.unwrap_err(), Refused::Protected, "{kind}");
                assert!(!data.join("oboete").exists(), "{kind}");
                assert!(!root.path().join(".oboete-keys").exists(), "{kind}");
            }
        }

        #[test]
        fn managed_registration_keeps_unrelated_opaque_and_disjoint_native_mounts_usable() {
            for (kind, protected) in [
                ("fuse.passthrough", false),
                ("unprovenfs", false),
                ("tmpfs", true),
            ] {
                let (root, _keys, corpus) = setup();
                let data = root.path().join("data");
                std::fs::create_dir(&data).unwrap();
                private(&data);
                let at = if protected {
                    corpus.join("layer")
                } else {
                    root.path().join("unrelated")
                };
                std::fs::create_dir(&at).unwrap();
                private(&at);
                mounts(root.path(), &data, &root.path().join("separate"));
                MOUNTS.with(|f| {
                    f.borrow_mut().as_mut().unwrap().table.extend_from_slice(
                        format!(
                            "8 1 0:997 / {} rw - {kind} opaque-source rw\n",
                            at.display()
                        )
                        .as_bytes(),
                    )
                });
                let result = managed(KEY, &corpus, root.path(), Some(&data));
                MOUNTS.with(|f| *f.borrow_mut() = None);
                let registration = result.unwrap();
                assert_eq!(
                    registration.path().parent().unwrap(),
                    data.join("oboete/keys"),
                    "{kind}"
                );
                assert_eq!(mode(registration.path()), 0o600);
            }
        }

        #[test]
        fn managed_registration_uses_mount_identity_when_subvolume_stat_devices_differ() {
            use std::os::unix::fs::MetadataExt;
            let (root, _keys, corpus) = setup();
            let alias = root.path().join("alias");
            std::fs::create_dir(&alias).unwrap();
            private(&alias);
            let stat_dev = std::fs::metadata(root.path()).unwrap().dev();
            let observed = format!(" {}:{} ", libc::major(stat_dev), libc::minor(stat_dev));
            // Btrfs reports the subvolume's anon_dev through stat and the superblock's
            // different s_dev in mountinfo. Only the mount observations are injected.
            let mounted = format!(
                " {}:{} ",
                libc::major(stat_dev),
                u64::from(libc::minor(stat_dev)) + 1
            );
            let describe_subvolume = || {
                MOUNTS.with(|f| {
                    let mut f = f.borrow_mut();
                    let fixture = f.as_mut().unwrap();
                    fixture.table = String::from_utf8(fixture.table.clone())
                        .unwrap()
                        .replace(&observed, &mounted)
                        .replace("- ext4 ", "- btrfs ")
                        .into_bytes();
                })
            };
            mounts(root.path(), &alias, &root.path().join("separate"));
            describe_subvolume();
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                alias.join("oboete/keys")
            );
            assert_eq!(mode(registration.path()), 0o600);
            drop(registration);

            // Different stat devices must not hide a bind into the same protected filesystem.
            mounts(root.path(), &alias, &corpus.join("descendant"));
            describe_subvolume();
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(
                std::fs::read_dir(alias.join("oboete/keys"))
                    .unwrap()
                    .count(),
                0
            );
        }

        #[test]
        fn managed_registration_protects_a_missing_corpus_without_creating_it() {
            let (root, _keys, _corpus) = setup();
            let corpus = root.path().join("not-yet-created/corpus");
            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".local/share/oboete/keys")
            );
            assert!(!root.path().join("not-yet-created").exists());
        }

        #[test]
        fn managed_registration_requires_complete_bounded_mount_observations() {
            let (root, _keys, corpus) = setup();
            let alias = root.path().join("alias");
            std::fs::create_dir(&alias).unwrap();
            private(&alias);
            mounts(root.path(), &alias, &root.path().join("separate"));
            let good = MOUNTS.with(|f| f.borrow().as_ref().unwrap().table.clone());
            let empty_type = String::from_utf8(good.clone())
                .unwrap()
                .replace("- ext4 ", "-  ")
                .into_bytes();
            let bad_escape = String::from_utf8(good.clone())
                .unwrap()
                .replace(" / / ", " /bad\\999 / ")
                .into_bytes();
            let mut duplicate = good.clone();
            duplicate.extend_from_slice(&good);
            for bad in [
                empty_type,
                Vec::new(),
                b"not a mount table".to_vec(),
                bad_escape,
                duplicate,
                vec![b'x'; 1_048_577],
            ] {
                MOUNTS.with(|f| f.borrow_mut().as_mut().unwrap().table = bad);
                let result = managed(KEY, &corpus, root.path(), Some(&alias));
                assert_eq!(result.unwrap_err(), Refused::Protected);
                assert!(!alias.join("oboete").exists());
                assert!(!root.path().join(".oboete-keys").exists());
            }
            MOUNTS.with(|f| {
                let mut f = f.borrow_mut();
                let fixture = f.as_mut().unwrap();
                fixture.table = good;
                fixture.ids = vec![(root.path().to_path_buf(), 999)];
            });
            let result = managed(KEY, &corpus, root.path(), Some(&alias));
            MOUNTS.with(|f| *f.borrow_mut() = None);
            assert_eq!(result.unwrap_err(), Refused::Protected);
            assert!(!alias.join("oboete").exists());
            assert!(!root.path().join(".oboete-keys").exists());
        }

        #[test]
        fn managed_registration_creates_private_directories_and_a_fresh_key() {
            let (root, _keys, corpus) = setup();
            let owner = root.path().join("owner");
            std::fs::create_dir(&owner).unwrap();
            private(&owner);
            let registration = managed(KEY, &corpus, &owner, None).unwrap();
            let path = registration.path().to_path_buf();
            assert_eq!(
                path.parent().unwrap(),
                owner.join(".local/share/oboete/keys")
            );
            for name in [
                ".local",
                ".local/share",
                ".local/share/oboete",
                ".local/share/oboete/keys",
            ] {
                assert_eq!(mode(&owner.join(name)), 0o700);
            }
            assert_eq!(mode(&path), 0o600);
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(name.ends_with("_KEY.md"));
            assert_eq!(name.len(), 39);
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
            assert!(registration.durable);
            registration.retain();
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
        }

        #[test]
        fn managed_registration_falls_back_outside_the_corpus_before_creating_directories() {
            let (root, _keys, corpus) = setup();
            let owner = root.path().join("owner");
            std::fs::create_dir(&owner).unwrap();
            private(&owner);
            let ordinary = corpus.join("local-data");
            let registration = managed(KEY, &corpus, &owner, Some(&ordinary)).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                owner.join(".oboete-keys")
            );
            assert!(!ordinary.exists());
            assert!(!owner.join(".oboete").exists());
            assert_eq!(mode(registration.path().parent().unwrap()), 0o700);
            assert_eq!(crate::config::read_key(registration.path()).unwrap(), KEY);
        }

        #[test]
        fn managed_registration_removes_only_its_own_uncommitted_file() {
            let (root, _keys, corpus) = setup();
            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            let path = registration.path().to_path_buf();
            let legacy = path.parent().unwrap().join("LEGACY_KEY.md");
            std::fs::write(&legacy, "legacy unchanged").unwrap();
            drop(registration);
            assert!(!path.exists());
            assert_eq!(
                std::fs::read_to_string(&legacy).unwrap(),
                "legacy unchanged"
            );

            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            let path = registration.path().to_path_buf();
            let moved = path.with_extension("saved");
            std::fs::rename(&path, &moved).unwrap();
            std::fs::write(&path, "concurrent replacement").unwrap();
            drop(registration);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "concurrent replacement"
            );
            assert_eq!(crate::config::read_key(&moved).unwrap(), KEY);
            assert_eq!(
                std::fs::read_to_string(&legacy).unwrap(),
                "legacy unchanged"
            );
        }

        #[test]
        fn managed_registration_write_and_sync_failures_leave_no_unused_key() {
            for at in [Step::Stage, Step::Write, Step::Sync] {
                let (root, _keys, corpus) = setup();
                FAIL.with(|f| f.set(Some(at)));
                let result = managed(KEY, &corpus, root.path(), None);
                FAIL.with(|f| f.set(None));
                assert_eq!(result.unwrap_err(), Refused::Failed, "{at:?}");
                assert_eq!(
                    std::fs::read_dir(root.path().join(".local/share/oboete/keys"))
                        .unwrap()
                        .count(),
                    0,
                    "{at:?}"
                );
            }
        }

        #[test]
        fn managed_registration_does_not_overwrite_or_delete_a_filename_collision() {
            let (root, _keys, corpus) = setup();
            let registration = managed(KEY, &corpus, root.path(), None).unwrap();
            let dir = registration.path().parent().unwrap().to_path_buf();
            drop(registration);
            let collision = dir.join("abababababababababababababababab_KEY.md");
            std::fs::write(&collision, "legacy bytes unchanged").unwrap();
            std::fs::set_permissions(&collision, std::fs::Permissions::from_mode(0o640)).unwrap();
            MANAGED_TAG.with(|f| f.set(Some([0xab; 16])));
            assert_eq!(
                managed(KEY, &corpus, root.path(), None).unwrap_err(),
                Refused::Failed
            );
            assert_eq!(
                std::fs::read_to_string(&collision).unwrap(),
                "legacy bytes unchanged"
            );
            assert_eq!(mode(&collision), 0o640);
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
        }

        #[test]
        fn managed_registration_refuses_a_bad_file_filesystem_before_writing_the_key() {
            for magic in [0x0102_1997, 0x6573_5546] {
                let (root, _keys, corpus) = setup();
                let dir = root.path().join(".local/share/oboete/keys");
                let observed = dir.clone();
                BEFORE_WRITE.with(|b| {
                    *b.borrow_mut() = Some(Box::new(move || {
                        let files: Vec<_> =
                            std::fs::read_dir(&observed).unwrap().flatten().collect();
                        assert_eq!(files.len(), 1);
                        assert_eq!(mode(&observed), 0o700);
                        assert_eq!(mode(&files[0].path()), 0o600);
                        assert!(std::fs::read(files[0].path()).unwrap().is_empty());
                        fake_fs(Some(magic));
                    }))
                });
                let result = managed(KEY, &corpus, root.path(), None);
                fake_fs(None);
                assert_eq!(result.unwrap_err(), Refused::NotPrivate);
                assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
            }
        }

        #[test]
        fn managed_registration_uses_safe_fallback_without_changing_shared_ancestors() {
            let (root, _keys, corpus) = setup();
            let shared = root.path().join("shared");
            std::fs::create_dir(&shared).unwrap();
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
            let alias = root.path().join("data-alias");
            std::os::unix::fs::symlink(&shared, &alias).unwrap();
            let registration = managed(KEY, &corpus, root.path(), Some(&alias)).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(mode(&shared), 0o777);
            assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 0);
            drop(registration);

            std::fs::remove_dir(root.path().join(".oboete-keys")).unwrap();
            let owner = shared.join("owner");
            std::fs::create_dir(&owner).unwrap();
            private(&owner);
            assert_eq!(
                managed(KEY, &corpus, &owner, Some(&alias)).unwrap_err(),
                Refused::SharedDir
            );
            assert!(!owner.join(".oboete-keys").exists());
        }

        #[test]
        fn managed_registration_keeps_existing_folder_modes_and_refuses_both_unsafe_locations() {
            let (root, _keys, corpus) = setup();
            let data = root.path().join("data");
            let ordinary = data.join("oboete/keys");
            std::fs::create_dir_all(&ordinary).unwrap();
            for dir in [&data, &data.join("oboete")] {
                private(dir);
            }
            std::fs::set_permissions(&ordinary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let fallback = root.path().join(".oboete-keys");
            std::fs::create_dir(&fallback).unwrap();
            std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                managed(KEY, &corpus, root.path(), Some(&data)).unwrap_err(),
                Refused::NotPrivate
            );
            assert_eq!(mode(&ordinary), 0o755);
            assert_eq!(mode(&fallback), 0o755);
            assert_eq!(std::fs::read_dir(&ordinary).unwrap().count(), 0);
            assert_eq!(std::fs::read_dir(&fallback).unwrap().count(), 0);
        }

        #[test]
        fn managed_registration_refuses_unknown_ancestor_filesystems_before_creation() {
            let (root, _keys, corpus) = setup();
            let data = root.path().join("data");
            std::fs::create_dir(&data).unwrap();
            private(&data);
            FS_AT.with(|f| *f.borrow_mut() = Some((data.clone(), 0x6573_5546)));
            let result = managed(KEY, &corpus, root.path(), Some(&data));
            FS_AT.with(|f| *f.borrow_mut() = None);
            let registration = result.unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
            drop(registration);

            FS_AT.with(|f| *f.borrow_mut() = Some((data.clone(), 0x6573_5546)));
            let result = managed(KEY, &corpus, &data, Some(&data));
            FS_AT.with(|f| *f.borrow_mut() = None);
            assert_eq!(result.unwrap_err(), Refused::NotPrivate);
            assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
        }

        #[test]
        fn managed_registration_refuses_corpus_aliases_and_never_creates_an_owner_corpus() {
            let (root, _keys, corpus) = setup();
            let alias = root.path().join("corpus-alias");
            std::os::unix::fs::symlink(&corpus, &alias).unwrap();
            let registration = managed(KEY, &corpus, root.path(), Some(&alias)).unwrap();
            assert_eq!(
                registration.path().parent().unwrap(),
                root.path().join(".oboete-keys")
            );
            assert_eq!(std::fs::read_dir(&corpus).unwrap().count(), 0);
            assert_eq!(
                managed(KEY, &corpus, &corpus, None).unwrap_err(),
                Refused::Protected
            );
            assert_eq!(std::fs::read_dir(&corpus).unwrap().count(), 0);
            assert!(!root.path().join(".oboete").exists());
        }

        #[test]
        fn managed_registration_reports_durability_without_losing_a_retained_key() {
            let (root, _keys, corpus) = setup();
            FAIL.with(|f| f.set(Some(Step::DirSync)));
            let result = managed(KEY, &corpus, root.path(), None);
            FAIL.with(|f| f.set(None));
            let registration = result.unwrap();
            let path = registration.path().to_path_buf();
            assert!(!registration.durable);
            registration.retain();
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
        }

        #[test]
        fn managed_registration_bad_key_errors_never_include_the_input() {
            let (root, _keys, corpus) = setup();
            let input = "secret-canary\nrefused";
            let refused = managed(input, &corpus, root.path(), None).unwrap_err();
            assert_eq!(refused, Refused::BadKey);
            assert!(!format!("{refused:?} {}", refused.code()).contains(input));
            assert!(!root.path().join(".local").exists());
            assert!(!root.path().join(".oboete-keys").exists());
        }

        #[test]
        fn a_key_goes_on_line_2_of_its_file_owner_only() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            std::fs::write(&path, "Groq\r\nold-key-123\r\nrotate yearly\r\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                format!("Groq\r\n{KEY}\r\nrotate yearly\r\n")
            );
            assert_eq!(mode(&path), 0o600);
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
            assert_eq!(holding(&[&keys, &home], KEY), [path]);
        }

        #[test]
        fn a_new_key_file_gets_a_title() {
            let (_root, keys, home) = setup();
            let path = keys.join("NEW_KEY.md");
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                format!("API key (oboete)\n{KEY}\n")
            );
            assert_eq!(mode(&path), 0o600);
            let missing = keys.join("no-such-dir").join("X_KEY.md");
            assert_eq!(write(&missing, KEY, &home), Err(Refused::NoDir));
            assert!(!keys.join("no-such-dir").exists());
        }

        #[test]
        fn an_ancestor_swap_is_refused_before_any_key_is_written() {
            let (root, _keys, home) = setup();
            let shared = root.path().join("shared");
            let keys = shared.join("keys");
            let moved = root.path().join("moved");
            std::fs::create_dir_all(&keys).unwrap();
            private(&keys);
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
            let path = keys.join("GROQ_KEY.md");
            let before = "Groq\nold-key-123\n";
            std::fs::write(&path, before).unwrap();
            let (swap, into, target) = (keys.clone(), moved.clone(), home.clone());
            BEFORE_WRITE.with(|b| {
                *b.borrow_mut() = Some(Box::new(move || {
                    std::fs::rename(&swap, &into).unwrap();
                    std::os::unix::fs::symlink(&target, &swap).unwrap();
                }))
            });
            // Without the ancestry check this swaps in the home after destination() has
            // approved the old key folder, and the save writes the key into that home.
            let result = write(&path, KEY, &home);
            let swap = BEFORE_WRITE.with(|b| b.borrow_mut().take());
            assert_eq!(result, Err(Refused::SharedDir));
            assert!(swap.is_some(), "must refuse before the read/stage/write");
            swap.unwrap()();
            assert_eq!(
                std::fs::read_to_string(moved.join("GROQ_KEY.md")).unwrap(),
                before
            );
            assert!(!path.exists());
            assert!(holding(&[&moved, &home], KEY).is_empty());
            assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
            assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
        }

        #[test]
        fn original_and_resolved_ancestors_must_both_be_protected() {
            let (root, keys, home) = setup();
            let shared = root.path().join("shared");
            let nested = shared.join("nested");
            let alias = root.path().join("alias");
            std::fs::create_dir_all(&nested).unwrap();
            private(&nested);
            std::os::unix::fs::symlink(&nested, &alias).unwrap();
            for mode in [0o775, 0o757, 0o777] {
                std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(mode)).unwrap();
                for path in [
                    nested.join("GROQ_KEY.md"),
                    shared.join("..").join("keys").join("GROQ_KEY.md"),
                    alias.join("GROQ_KEY.md"),
                ] {
                    assert_eq!(write(&path, KEY, &home), Err(Refused::SharedDir));
                }
            }
            assert!(holding(&[&keys, &nested, &home], KEY).is_empty());
        }

        /// A link whose target is reached through another link: the folder that holds the second
        /// link is on the path too, and a user who can write there can point it anywhere.
        #[test]
        fn a_folder_a_second_link_passes_through_is_checked() {
            let (root, keys, home) = setup();
            let shared = root.path().join("shared");
            std::fs::create_dir_all(&shared).unwrap();
            let hop = shared.join("hop");
            std::os::unix::fs::symlink(&keys, &hop).unwrap();
            let alias = root.path().join("alias");
            std::os::unix::fs::symlink(&hop, &alias).unwrap();
            let path = alias.join("GROQ_KEY.md");
            // Both links in folders only the owner writes: the chain is as safe as its target.
            private(&shared);
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            // The second link's folder open to others: `hop` can be replaced under the first link.
            for mode in [0o775, 0o757, 0o777] {
                std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(mode)).unwrap();
                assert_eq!(
                    write(&path, "canary-second-hop", &home),
                    Err(Refused::SharedDir),
                    "{mode:o}"
                );
            }
            assert!(holding(&[&keys, &home], "canary-second-hop").is_empty());
            // A chain that never ends is no folder.
            let (a, b) = (root.path().join("a"), root.path().join("b"));
            std::os::unix::fs::symlink(&b, &a).unwrap();
            std::os::unix::fs::symlink(&a, &b).unwrap();
            assert_eq!(
                write(&a.join("GROQ_KEY.md"), KEY, &home),
                Err(Refused::NoDir)
            );
        }

        /// A link target that ends in `/` or `/.` is still a link: the kernel follows it when it
        /// is asked about `hop/`, so the folders behind it must be walked, not skipped (Codex's
        /// review of #355).
        #[test]
        fn a_link_target_with_a_trailing_slash_does_not_hide_its_hops() {
            for suffix in ["/", "/."] {
                let (root, keys, home) = setup();
                let shared = root.path().join("shared");
                std::fs::create_dir_all(&shared).unwrap();
                std::os::unix::fs::symlink(&keys, shared.join("bridge")).unwrap();
                std::os::unix::fs::symlink(shared.join("bridge"), root.path().join("hop")).unwrap();
                let alias = root.path().join("alias");
                std::os::unix::fs::symlink(format!("hop{suffix}"), &alias).unwrap();
                let path = alias.join("GROQ_KEY.md");
                private(&shared);
                assert_eq!(
                    write(&path, KEY, &home),
                    Ok(Written { durable: true }),
                    "{suffix}"
                );
                std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
                assert_eq!(
                    write(&path, "canary-trailing-slash", &home),
                    Err(Refused::SharedDir),
                    "{suffix}"
                );
                assert!(holding(&[&keys, &home], "canary-trailing-slash").is_empty());
            }
        }

        /// Each link is followed once, as the kernel counts them: a path through several links is
        /// not refused for its length, and the kernel's limit of 40 is the limit.
        #[test]
        fn links_are_counted_once_each_up_to_the_kernels_forty() {
            let (root, keys, home) = setup();
            // l1 -> d1, d1/l2 -> d2, ... d5/l6 -> the key folder: six links in one path, each
            // target relative to the folder its link is in.
            let mut at = root.path().to_path_buf();
            let mut path = root.path().to_path_buf();
            for n in 1..=6 {
                let target = if n == 6 {
                    "../../../../../keys".to_string()
                } else {
                    format!("d{n}")
                };
                std::os::unix::fs::symlink(target, at.join(format!("l{n}"))).unwrap();
                path.push(format!("l{n}"));
                if n < 6 {
                    at.push(format!("d{n}"));
                    std::fs::create_dir(&at).unwrap();
                    private(&at);
                }
            }
            assert_eq!(
                write(&path.join("GROQ_KEY.md"), KEY, &home),
                Ok(Written { durable: true })
            );
            // A chain: c40 -> c39 -> ... -> c1 -> the key folder is 40 links; one more is refused.
            let mut target = keys.clone();
            for n in 1..=41 {
                let link = root.path().join(format!("c{n}"));
                std::os::unix::fs::symlink(&target, &link).unwrap();
                target = link;
            }
            assert_eq!(
                write(&root.path().join("c40").join("GROQ_KEY.md"), KEY, &home),
                Ok(Written { durable: true })
            );
            assert_eq!(
                write(&root.path().join("c41").join("GROQ_KEY.md"), KEY, &home),
                Err(Refused::NoDir)
            );
            // Asked of the walk itself: in `write` the kernel refuses a 41st link after it, which
            // would hide a walk that no longer counts (and then never ends on a loop of links).
            assert_eq!(
                crate::keyfile::linux::check_dirs(&root.path().join("c41")),
                Err(Refused::NoDir)
            );
        }

        #[test]
        fn trusted_sticky_ancestors_and_private_aliases_keep_working() {
            let (root, keys, home) = setup();
            let alias = root.path().join("alias");
            std::os::unix::fs::symlink(&keys, &alias).unwrap();
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
            let path = alias.join("GROQ_KEY.md");
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            assert_eq!(mode(&path), 0o600);
            // The key folder cannot itself be shared: a new key's name has no owner yet.
            std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o1777)).unwrap();
            let new = keys.join("NEW_KEY.md");
            assert_eq!(write(&new, KEY, &home), Err(Refused::SharedDir));
            assert!(!new.exists());
        }

        #[test]
        fn a_key_goes_only_into_a_dedicated_key_file() {
            let (_root, keys, home) = setup();
            let config = home.join("config.toml");
            std::fs::write(&config, "[summary]\nlanguage = \"ja\"\n").unwrap();
            for (path, why) in [
                (PathBuf::from("GROQ_KEY.md"), Refused::NotAbsolute),
                (keys.join("groq.txt"), Refused::NotAKeyFile),
                (config.clone(), Refused::NotAKeyFile),
                (home.join("GROQ_KEY.md"), Refused::Protected),
                (
                    keys.join("..").join("home").join("GROQ_KEY.md"),
                    Refused::Protected,
                ),
            ] {
                assert_eq!(write(&path, KEY, &home), Err(why), "{}", path.display());
            }
            assert_eq!(
                std::fs::read_to_string(&config).unwrap(),
                "[summary]\nlanguage = \"ja\"\n"
            );
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        #[test]
        fn a_key_file_that_is_no_plain_small_utf8_file_is_left_alone() {
            let (_root, keys, home) = setup();
            let target = keys.join("target.txt");
            std::fs::write(&target, "untouched").unwrap();
            let link = keys.join("LINK_KEY.md");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(write(&link, KEY, &home), Err(Refused::NotAFile));
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
            let folder = keys.join("DIR_KEY.md");
            std::fs::create_dir(&folder).unwrap();
            assert_eq!(write(&folder, KEY, &home), Err(Refused::NotAFile));
            // A FIFO is refused at once, not waited on.
            let fifo = keys.join("FIFO_KEY.md");
            let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            assert_eq!(write(&fifo, KEY, &home), Err(Refused::NotAFile));
            let big = keys.join("BIG_KEY.md");
            std::fs::write(&big, vec![b'a'; 64 * 1024 + 1]).unwrap();
            assert_eq!(write(&big, KEY, &home), Err(Refused::TooBig));
            let latin = keys.join("LATIN_KEY.md");
            std::fs::write(&latin, b"Groq \xe9\nold\n").unwrap();
            assert_eq!(write(&latin, KEY, &home), Err(Refused::NotUtf8));
            assert_eq!(std::fs::read(&latin).unwrap(), b"Groq \xe9\nold\n");
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        #[test]
        fn a_failure_before_the_rename_leaves_the_file_and_no_stage() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            let before = "Groq\nold-key-123\n";
            for at in [Step::Stage, Step::Write, Step::Sync, Step::Rename] {
                std::fs::write(&path, before).unwrap();
                FAIL.with(|f| f.set(Some(at)));
                let result = write(&path, KEY, &home);
                FAIL.with(|f| f.set(None));
                assert_eq!(result, Err(Refused::Failed), "{at:?}");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "{at:?}");
                let names: Vec<_> = std::fs::read_dir(&keys)
                    .unwrap()
                    .flatten()
                    .map(|e| e.file_name())
                    .collect();
                assert_eq!(names, ["GROQ_KEY.md"], "{at:?}");
            }
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        /// A folder's owner and mode are believed only on a filesystem known to enforce them: a
        /// FUSE mount another user made can show its folders as this user's, 0700, and then turn
        /// one into a link (Codex's second review of #355). Such a folder on the way, or as the
        /// key folder, is refused before any read or stage.
        #[test]
        fn a_folder_on_the_way_must_be_on_a_filesystem_known_to_keep_a_mode() {
            let (root, _keys, home) = setup();
            let mount = root.path().join("m");
            let keys = mount.join("keys");
            std::fs::create_dir_all(&keys).unwrap();
            private(&mount);
            private(&keys);
            let path = keys.join("GROQ_KEY.md");
            assert_eq!(write(&path, KEY, &home), Ok(Written { durable: true }));
            for at in [&mount, &keys] {
                FS_AT.with(|f| *f.borrow_mut() = Some((at.clone(), 0x6573_5546)));
                let result = write(&path, "canary-fuse-on-the-way", &home);
                FS_AT.with(|f| *f.borrow_mut() = None);
                assert_eq!(result, Err(Refused::NotPrivate), "{}", at.display());
            }
            assert!(holding(&[&keys, &home], "canary-fuse-on-the-way").is_empty());
            let names: Vec<_> = std::fs::read_dir(&keys)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(names, ["GROQ_KEY.md"]);
            // The kernel's own answer, with no seam: /proc is on no filesystem of the list.
            assert_eq!(
                write(Path::new("/proc/GROQ_KEY.md"), KEY, &home),
                Err(Refused::NotPrivate)
            );
        }

        /// The staged file's own filesystem decides, before the key is written: 9p (a Windows
        /// drive under WSL) and FUSE are refused, and the file and folder stay as they were.
        #[test]
        fn a_filesystem_not_known_to_keep_a_mode_gets_no_key() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            let before = "Groq\nold-key-123\n";
            std::fs::write(&path, before).unwrap();
            for magic in [0x0102_1997, 0x6573_5546] {
                FS.with(|f| f.set(Some(magic)));
                let result = write(&path, KEY, &home);
                FS.with(|f| f.set(None));
                assert_eq!(result, Err(Refused::NotPrivate), "{magic:x}");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
                let names: Vec<_> = std::fs::read_dir(&keys)
                    .unwrap()
                    .flatten()
                    .map(|e| e.file_name())
                    .collect();
                assert_eq!(names, ["GROQ_KEY.md"], "{magic:x}");
            }
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }

        #[test]
        fn a_failed_folder_sync_after_the_rename_keeps_the_key() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            std::fs::write(&path, "Groq\nold-key-123\n").unwrap();
            FAIL.with(|f| f.set(Some(Step::DirSync)));
            let result = write(&path, KEY, &home);
            FAIL.with(|f| f.set(None));
            assert_eq!(result, Ok(Written { durable: false }));
            assert_eq!(crate::config::read_key(&path).unwrap(), KEY);
        }

        #[test]
        fn a_file_changed_since_it_was_read_is_not_overwritten() {
            let (_root, keys, home) = setup();
            let path = keys.join("GROQ_KEY.md");
            std::fs::write(&path, "Groq\nold-key-123\n").unwrap();
            let edit = path.clone();
            BEFORE_CHECK.with(|b| {
                *b.borrow_mut() = Some(Box::new(move || {
                    std::fs::write(&edit, "Groq\nold-key-123\nnote added meanwhile\n").unwrap();
                }))
            });
            assert_eq!(write(&path, KEY, &home), Err(Refused::Changed));
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "Groq\nold-key-123\nnote added meanwhile\n"
            );
            // A file made meanwhile where there was none counts as a change too.
            let new = keys.join("NEW_KEY.md");
            let made = new.clone();
            BEFORE_CHECK.with(|b| {
                *b.borrow_mut() = Some(Box::new(move || std::fs::write(&made, "mine\n").unwrap()))
            });
            assert_eq!(write(&new, KEY, &home), Err(Refused::Changed));
            assert_eq!(std::fs::read_to_string(&new).unwrap(), "mine\n");
            assert!(holding(&[&keys, &home], KEY).is_empty());
        }
    }
}
