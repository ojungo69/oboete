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

/// A held, existing owner-only directory for the invalid-config recovery copy.
pub(crate) fn private_recovery_dir(path: &Path) -> Result<std::fs::File, Refused> {
    #[cfg(target_os = "linux")]
    {
        linux::check_dirs(path)?;
        linux::private_dir(path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        Err(Refused::Unsupported)
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

    pub(super) fn private_dir(path: &Path) -> Result<std::fs::File, Refused> {
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
mod tests;
