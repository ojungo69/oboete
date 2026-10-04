//! Resident R9: the executable noted at startup, also used by detached starts.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct Binary {
    path: PathBuf,
    #[cfg(target_os = "linux")]
    identity: crate::worker::FileId,
    #[cfg(target_os = "linux")]
    arguments: Vec<std::ffi::OsString>,
    #[cfg(target_os = "linux")]
    viewer: std::sync::Mutex<Option<u32>>,
}

pub(crate) const VIEWER_HANDOFF: &str = "OBOETE_EXEC_VIEWER";
pub(crate) const HOME_HANDOFF: &str = "OBOETE_EXEC_HOME";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Worker,
    Viewer,
}

impl Role {
    fn lock(self) -> &'static str {
        match self {
            Self::Worker => "worker.lock",
            Self::Viewer => "view.lock",
        }
    }
}

#[derive(Clone, Copy)]
struct Home {
    role: Role,
    identity: (u64, u64),
}

#[cfg(target_os = "linux")]
static HOME: OnceLock<Result<Option<Home>, ()>> = OnceLock::new();

#[cfg(target_os = "linux")]
fn inherited_home() -> Result<Option<Home>, ()> {
    let Some(value) = std::env::var_os(HOME_HANDOFF) else {
        return Ok(None);
    };
    let value = value.into_string().map_err(|_| ())?;
    let mut fields = value.split(':');
    let pid = fields.next().ok_or(())?.parse::<u32>().map_err(|_| ())?;
    if pid != std::process::id() {
        return Ok(None);
    }
    let role = match fields.next() {
        Some("worker.lock") => Role::Worker,
        Some("view.lock") => Role::Viewer,
        _ => return Err(()),
    };
    let device = fields.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    let inode = fields.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    if fields.next().is_some() {
        return Err(());
    }
    Ok(Some(Home {
        role,
        identity: (device, inode),
    }))
}

fn resumed() -> std::io::Result<Option<Home>> {
    #[cfg(target_os = "linux")]
    return HOME
        .get_or_init(inherited_home)
        .as_ref()
        .copied()
        .map_err(|_| std::io::Error::other("invalid resident exec home handoff"));
    #[cfg(not(target_os = "linux"))]
    Ok(None)
}

/// The previous image's home remains the authority, including after the new lock is opened.
pub(crate) fn expected(role: Option<Role>) -> std::io::Result<crate::worker::FileId> {
    match resumed()? {
        Some(home) if role == Some(home.role) => Ok(Some(home.identity)),
        Some(_) => Err(std::io::Error::other("resident exec changed its command")),
        None => Ok(None),
    }
}

pub(crate) fn check_home(home: &Path, role: Option<Role>) -> std::io::Result<()> {
    if expected(role)?.is_some() {
        let lock = role.expect("validated resident role").lock();
        check_lock(
            role,
            crate::worker::file_id(std::fs::metadata(home.join("state").join(lock))),
        )?;
    }
    Ok(())
}

pub(crate) fn check_lock(
    role: Option<Role>,
    identity: crate::worker::FileId,
) -> std::io::Result<()> {
    if expected(role)?.is_some_and(|expected| Some(expected) != identity) {
        return Err(std::io::Error::other(
            "the resident home changed across binary exec",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn inherited_viewer() -> Option<u32> {
    let value = std::env::var(VIEWER_HANDOFF).ok()?;
    let (parent, child) = value.split_once(':')?;
    if parent.parse::<u32>().ok()? != std::process::id() {
        return None;
    }
    let child = child.parse::<u32>().ok()?;
    (child > 0 && i32::try_from(child).is_ok() && child != std::process::id()).then_some(child)
}

static BINARY: OnceLock<Option<Binary>> = OnceLock::new();

fn binary() -> Option<&'static Binary> {
    BINARY
        .get_or_init(|| {
            Some(Binary {
                path: std::env::current_exe().ok()?,
                #[cfg(target_os = "linux")]
                identity: crate::worker::file_id(std::fs::metadata("/proc/self/exe")),
                #[cfg(target_os = "linux")]
                arguments: std::env::args_os().collect(),
                #[cfg(target_os = "linux")]
                viewer: std::sync::Mutex::new(inherited_viewer()),
            })
        })
        .as_ref()
}

pub(crate) fn init() {
    let _ = resumed();
    let _ = binary();
}

pub(crate) fn path() -> Option<&'static Path> {
    binary().map(|binary| binary.path.as_path())
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // R9 changes are constructed on Linux only.
pub(crate) enum Change {
    Stay,
    Replaced,
    Missing,
}

#[derive(Default)]
pub(crate) struct Watch {
    #[cfg(target_os = "linux")]
    missing: Option<std::time::Instant>,
}

impl Watch {
    pub(crate) fn change(&mut self) -> Change {
        #[cfg(target_os = "linux")]
        if let Some(binary) = binary() {
            return self.at(&binary.path, binary.identity, std::time::Instant::now());
        }
        Change::Stay
    }

    #[cfg(target_os = "linux")]
    fn at(
        &mut self,
        path: &Path,
        loaded: crate::worker::FileId,
        now: std::time::Instant,
    ) -> Change {
        match std::fs::metadata(path) {
            Ok(metadata) => {
                self.missing = None;
                let current = crate::worker::file_id(Ok(metadata));
                if loaded.is_some() && current.is_some() && current != loaded {
                    Change::Replaced
                } else {
                    Change::Stay
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let missing = *self.missing.get_or_insert(now);
                if now.saturating_duration_since(missing) >= std::time::Duration::from_secs(60) {
                    Change::Missing
                } else {
                    Change::Stay
                }
            }
            Err(_) => {
                // An inaccessible/non-directory path is not proof that it remained absent.
                self.missing = None;
                Change::Stay
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn take_viewer() -> Option<u32> {
    if let Some(binary) = binary() {
        return binary
            .viewer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
    None
}

pub(crate) fn exec(
    role: Role,
    home: crate::worker::FileId,
    viewer: Option<u32>,
) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    if let Some(binary) = binary() {
        return binary.exec(role, home, viewer);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (role, home, viewer);
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "startup executable unavailable",
    ))
}

#[cfg(target_os = "linux")]
impl Binary {
    fn exec(
        &self,
        role: Role,
        home: crate::worker::FileId,
        viewer: Option<u32>,
    ) -> std::io::Result<()> {
        use std::ffi::{CString, OsStr};
        use std::os::unix::ffi::OsStrExt;

        fn string(value: &OsStr, field: &str) -> std::io::Result<CString> {
            CString::new(value.as_bytes()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{field} contains NUL"),
                )
            })
        }
        let path = string(self.path.as_os_str(), "executable path")?;
        let (device, inode) = home
            .ok_or_else(|| std::io::Error::other("resident lock identity unavailable for exec"))?;
        let arguments: Vec<_> = self
            .arguments
            .iter()
            .map(|argument| string(argument, "argument"))
            .collect::<std::io::Result<_>>()?;
        let mut environment: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| key != VIEWER_HANDOFF && key != HOME_HANDOFF)
            .map(|(mut key, value)| {
                key.push("=");
                key.push(value);
                string(&key, "environment")
            })
            .collect::<std::io::Result<_>>()?;
        environment.push(
            CString::new(format!(
                "{HOME_HANDOFF}={}:{}:{device}:{inode}",
                std::process::id(),
                role.lock(),
            ))
            .expect("numeric home handoff contains no NUL"),
        );
        if let Some(viewer) = viewer {
            environment.push(
                CString::new(format!("{VIEWER_HANDOFF}={}:{viewer}", std::process::id()))
                    .expect("numeric PID handoff contains no NUL"),
            );
        }
        let argv: Vec<_> = arguments
            .iter()
            .map(|argument| argument.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let envp: Vec<_> = environment
            .iter()
            .map(|variable| variable.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        // CommandExt::exec changes SIGPIPE even when exec fails. R9 must preserve serving on
        // failure, so this boundary changes no stdio, cwd, environment or signal disposition.
        // SAFETY: path and every argv/envp entry are stable NUL-terminated buffers; each pointer
        // array has its required trailing null, and all buffers live through the syscall.
        unsafe { libc::execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn missing_requires_the_next_minute_and_recovery_or_metadata_errors_keep_serving() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join("bin");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("oboete");
        std::fs::write(&path, "synthetic executable").unwrap();
        let _loaded = std::fs::File::open(&path).unwrap();
        let identity = crate::worker::file_id(std::fs::metadata(&path));
        let start = Instant::now();
        let mut watch = Watch::default();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(watch.at(&path, identity, start), Change::Stay);
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(59)),
            Change::Stay
        );
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(60)),
            Change::Missing,
            "a continuously absent executable did not end at its next minute check"
        );
        let mut watch = Watch::default();
        assert_eq!(watch.at(&path, identity, start), Change::Stay);
        std::fs::write(&path, "replacement").unwrap();
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(10)),
            Change::Replaced
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(20)),
            Change::Stay
        );
        std::fs::remove_dir(&directory).unwrap();
        std::fs::write(&directory, "not a directory").unwrap();
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(90)),
            Change::Stay,
            "ENOTDIR is not proof of a continuously missing path"
        );
        std::fs::remove_file(&directory).unwrap();
        std::fs::create_dir(&directory).unwrap();
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(100)),
            Change::Stay
        );
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(159)),
            Change::Stay
        );
        assert_eq!(
            watch.at(&path, identity, start + Duration::from_secs(160)),
            Change::Missing
        );
    }
}
