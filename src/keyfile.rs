//! A key typed on the settings page, written into its chain entry's key file (#94, part 3): line 2
//! of a file named `*_KEY.md` outside the oboete home, on Linux, on a filesystem that keeps a Unix
//! mode. Everything else in the file stays as it was, and the key goes nowhere but that file.
//! macOS and Windows are refused until a file there can be made owner-only from its first instant
//! (#281): a folder's inherited ACL entries would reach it through a 0600 mode.

// Off Linux only the refusal and the tests' pure helpers are used (#281).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io::{Read, Write};
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
/// Filesystems where a file's Unix mode does not keep it to its owner: a Windows drive under WSL,
/// network shares, FAT and NTFS volumes.
const NOT_PRIVATE: &[&str] = &[
    "9p", "drvfs", "cifs", "smb3", "smbfs", "ntfs", "ntfs3", "fuseblk", "vfat", "exfat", "msdos",
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
    let dir = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .filter(|p| p.is_dir())
        .ok_or(Refused::NoDir)?;
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

/// The filesystem type of the mount that holds `dir` (canonical), from `/proc/self/mountinfo`: the
/// longest mount point that is a whole-component prefix of it, the later of two on one point.
fn mount_type<'a>(info: &'a str, dir: &Path) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for line in info.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let Some(dash) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        let (Some(point), Some(kind)) = (fields.get(4), fields.get(dash + 1)) else {
            continue;
        };
        let point = PathBuf::from(unescape(point));
        let len = point.components().count();
        if dir.starts_with(&point) && best.is_none_or(|(l, _)| len >= l) {
            best = Some((len, kind));
        }
    }
    best.map(|(_, kind)| kind)
}

/// mountinfo writes a space, tab, newline and backslash in a path as a three-digit octal escape.
fn unescape(field: &str) -> String {
    let b = field.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let octal = b
            .get(i + 1..i + 4)
            .filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
        match (b[i], octal) {
            (b'\\', Some(d)) => {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
    static BEFORE_CHECK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
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
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    pub(super) fn write(path: &Path, key: &str, home: &Path) -> Result<Written, Refused> {
        let (dest, dir) = destination(path, home)?;
        let info =
            std::fs::read_to_string("/proc/self/mountinfo").map_err(|_| Refused::NotPrivate)?;
        if mount_type(&info, &dir).is_none_or(|kind| NOT_PRIVATE.contains(&kind)) {
            return Err(Refused::NotPrivate);
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
            // A filesystem that ignores the mode is found before the key is written.
            let mode = file
                .metadata()
                .map_err(|_| Refused::Failed)?
                .permissions()
                .mode();
            if mode & 0o777 != 0o600 {
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

    #[test]
    fn a_mount_is_matched_by_whole_components_and_its_escapes() {
        let info = "22 1 8:1 / / rw - ext4 /dev/sda1 rw\n\
                    95 22 0:52 / /mnt/c rw - 9p drvfs rw\n\
                    96 22 0:53 / /mnt/my\\040disk rw - vfat /dev/sdb1 rw\n\
                    97 22 0:54 / /mnt/c rw - ext4 /dev/sdc1 rw\n";
        let kind = |p: &str| mount_type(info, Path::new(p));
        assert_eq!(kind("/home/me"), Some("ext4"));
        // The later of two mounts on one point hides the earlier.
        assert_eq!(kind("/mnt/c/Users"), Some("ext4"));
        assert_eq!(kind("/mnt/cx"), Some("ext4"));
        assert_eq!(kind("/mnt/my disk/keys"), Some("vfat"));
        assert_eq!(
            mount_type(
                "95 22 0:52 / /mnt/c rw - 9p drvfs rw",
                Path::new("/mnt/c/x")
            ),
            Some("9p")
        );
        assert_eq!(mount_type("", Path::new("/x")), None);
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
            (root, keys, home)
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
