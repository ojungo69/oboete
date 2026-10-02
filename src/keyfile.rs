//! A key typed on the settings page, written into its chain entry's key file (#94, part 3): line 2
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
const PRIVATE_FS: &[u32] = &[
    0xEF53,      // ext2, ext3, ext4
    0x5846_5342, // XFS
    0x9123_683E, // Btrfs
    0xF2F5_2010, // F2FS
    0x2FC1_2FC1, // ZFS
    0xCA45_1A4E, // bcachefs
    0x0102_1994, // tmpfs
    0x794C_7630, // overlayfs
];

/// 8 to 512 characters of letters, digits and `._~+/=:-`: every provider's keys, and no quote,
/// backslash, space or control character, so a key can add no line and no TOML string.
pub(crate) fn valid(key: &str) -> bool {
    KEY_LEN.contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~+/=:-".contains(&b))
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
    /// The filesystem magic `private` sees instead of the real one.
    static FS: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
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
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

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
        let dir = std::fs::symlink_metadata(&at).map_err(|_| Refused::NoDir)?;
        if dir.mode() & 0o022 != 0 {
            return Err(Refused::SharedDir);
        }
        Ok(())
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
            let entry = std::fs::symlink_metadata(&next).map_err(|_| Refused::NoDir)?;
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
    fn private(file: &std::fs::File) -> bool {
        use std::os::fd::AsRawFd;
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: `fstatfs` gets an open descriptor and a buffer of its type, which it fills when
        // it returns 0; only then is the buffer read.
        if unsafe { libc::fstatfs(file.as_raw_fd(), fs.as_mut_ptr()) } != 0 {
            return false;
        }
        // SAFETY: filled above. The magic is 32 bits, whatever the width of `f_type`.
        let magic = unsafe { fs.assume_init() }.f_type as u32;
        #[cfg(test)]
        let magic = FS.with(|f| f.get()).unwrap_or(magic);
        PRIVATE_FS.contains(&magic)
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
