//! `oboete view`: the memory in a browser, on 127.0.0.1. std `TcpListener` + `httparse`, one
//! thread per connection, one bundled page and a small JSON API: reads over `search`, and the two
//! requests with a body, the settings page's saves of the settings and of a key (#94). Every
//! `/api` request carries the per-launch token, which the page reads from the URL fragment and
//! sends as a header. docs/m1.md decisions 11 and 14 have the reasons (tiny_http's open CVEs) and
//! the threat model; spec 6.6 has the save's.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{redact, search};

const INDEX: &str = include_str!("../assets/viewer/index.html");
const APP_JS: &str = include_str!("../assets/viewer/app.js");
const APP_CSS: &str = include_str!("../assets/viewer/app.css");
/// Request line and headers; a browser's GET is well under 2 KB.
const MAX_HEAD: usize = 16 * 1024;
/// A settings save's body; the page's is under 2 KB.
const MAX_BODY: usize = 16 * 1024;
/// A key save's body: a key of at most 512 characters, an entry name of at most 64 bytes and the
/// version always fit.
const MAX_KEY_BODY: usize = 1024;
/// Head and body together: a slow sender holds its thread this long at most.
const REQUEST_TIME: Duration = Duration::from_secs(5);
/// The answer, all of it: a client that stops reading holds its thread this long at most.
const ANSWER_TIME: Duration = Duration::from_secs(10);
/// Connections served at once, before the token or after; one more is closed at once (#53). A
/// browser keeps at most 6 to one host.
const MAX_CONNECTIONS: usize = 32;
const MAX_LIMIT: usize = 200;
const SECURITY_HEADERS: &str = "Content-Security-Policy: default-src 'none'; script-src 'self'; \
    style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; \
    frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
    Cache-Control: no-store\r\n";

struct Viewer {
    home: PathBuf,
    /// The held resident lock's identity from listen; foreground viewers have none.
    home_lock: crate::worker::FileId,
    /// Where `oboete view` was started: its checkout is the page's default scope and its search's
    /// caller (`checkout`). The resident viewer has none, and every repository is its default
    /// scope (docs/resident.md R8).
    cwd: Option<PathBuf>,
    /// The port it takes connections on: the resident viewer's moves onto another (`move_to`).
    port: AtomicU16,
    /// What the accept loop does at its next connection.
    next: Mutex<Option<Next>>,
    token: Token,
    /// Settings saves, one at a time.
    saving: Mutex<()>,
    /// A port saved and the listener moved onto it, one at a time, a page's new token's too: the
    /// moves take effect in the order their ports were written (Codex on W6).
    moving: Mutex<()>,
    recovery: crate::settings::recovery::Recovery,
    /// One synchronous maintenance operation and its last bounded receipt.
    maintenance: crate::settings::maintenance::Maintenance,
    agents: crate::setup::agents::Agents,
    /// The page `--open` gave the browser opener, removed by the first request with the token.
    opener: Mutex<Option<PathBuf>>,
    /// Connections being served: at most `MAX_CONNECTIONS`.
    live: AtomicUsize,
    /// Connections taken since the start: whether one came since the resident viewer's last look
    /// (R4).
    requests: AtomicUsize,
    /// Set while the resident viewer decides whether it leaves, and kept once it does: no
    /// connection is taken then.
    closing: AtomicBool,
}

/// What a request's `X-Oboete-Token` must be.
enum Token {
    /// This run's, made at its start: `oboete view` in the foreground.
    Run(String),
    /// The resident viewer's: `state/view-token`, read for each request (`file_token`), so it
    /// outlives a restart and a file that changed is never trusted from before (R6).
    File,
}

/// What the accept loop does instead of serving its next connection (`Viewer::wake`): that
/// connection, the one that woke it, reached the old address and is closed unread.
enum Next {
    /// Take connections on this listener, of this port, from now on: the resident viewer moves
    /// at once (a saved port, a new token), and the old address stops answering.
    Move(u16, TcpListener),
    /// Take no more and return once the live connections are done: a foreground run whose page
    /// went on to the resident viewer.
    End,
}

/// One of `MAX_CONNECTIONS`, given back when its connection's thread ends, however it ends.
struct Slot(Arc<Viewer>);

impl Slot {
    fn take(v: &Arc<Viewer>) -> Option<Slot> {
        // Counted before `closing` is read, as `may_leave` sets it before it reads the count:
        // one of the two sees the other.
        if v.live.fetch_add(1, Ordering::SeqCst) < MAX_CONNECTIONS
            && !v.closing.load(Ordering::SeqCst)
        {
            return Some(Slot(Arc::clone(v)));
        }
        v.live.fetch_sub(1, Ordering::SeqCst);
        None
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What a request's head leads to: an answer, or a save whose body of this many bytes is read
/// first and handed to it, with the token its request brought (`Viewer::take`).
enum Head {
    Answer(Response),
    Body(usize, Save, String),
}

/// A typed operation that takes a request's body and the token it brought: one that moves the
/// page's address checks that token again under its lock (R6).
type Save = fn(&Viewer, &[u8], &str) -> Response;

#[derive(Debug)]
struct Response {
    status: u16,
    ctype: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn new(status: u16, ctype: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            ctype,
            body: body.into(),
        }
    }
    fn text(status: u16, msg: &str) -> Self {
        Self::new(status, "text/plain; charset=utf-8", msg)
    }
    fn json(v: &Value) -> Self {
        Self::new(
            200,
            "application/json",
            serde_json::to_vec(v).unwrap_or_default(),
        )
    }
    fn bytes(&self, head_only: bool) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Content Too Large",
            422 => "Unprocessable Content",
            503 => "Service Unavailable",
            _ => "Internal Server Error",
        };
        let mut out = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{SECURITY_HEADERS}Connection: close\r\n\r\n",
            self.status,
            self.ctype,
            self.body.len()
        )
        .into_bytes();
        if !head_only {
            out.extend_from_slice(&self.body);
        }
        out
    }
}

/// Serve until interrupted. In a resident home with no `--port`, it makes sure the resident
/// viewer runs and prints its address instead (R7); when that viewer does not come up, it says
/// why and serves here, on an address of this run, as in any other home.
pub fn run(home: &Path, port: Option<u16>, open: bool) -> Result<()> {
    if port.is_none() && resident_home(home) {
        match bring_up(home, Duration::from_secs(3)) {
            Ok(port) => return show_resident(home, port, open),
            Err(why) => eprintln!(
                "(the resident viewer is not up: {why}; this run serves the page on its own address)"
            ),
        }
    }
    let listener = TcpListener::bind(("127.0.0.1", port.unwrap_or(0)))?;
    let port = listener.local_addr()?.port();
    let token = fresh_token()?;
    let url = format!("http://127.0.0.1:{port}/#t={token}");
    let viewer = Arc::new(Viewer::new(
        home,
        Some(std::env::current_dir()?),
        port,
        Token::Run(token),
    ));
    println!("{url}\n(open it in a browser; Ctrl-C stops the viewer)");
    if open {
        viewer.open(home, &url, open_browser);
    }
    accept(listener, &viewer);
    Ok(())
}

/// Whether this home keeps a resident worker and viewer (R2): Linux only, as they are for now.
fn resident_home(home: &Path) -> bool {
    cfg!(target_os = "linux") && crate::config::worker(home).is_ok_and(|w| w.resident)
}

/// R7: makes sure the resident viewer runs, and starts the worker too when nothing holds its
/// lock, as a hook does: ready when `state/view.lock` is held and the outcome says it listens on
/// the configured port, waited for up to `wait`. Nothing is sent to the port; why it is not
/// ready otherwise.
fn bring_up(home: &Path, wait: Duration) -> std::result::Result<u16, String> {
    std::fs::create_dir_all(home.join("state")).map_err(|e| e.to_string())?;
    if !owner_only(home) {
        return Err(NOT_PRIVATE.into());
    }
    let port = crate::config::view(home)
        .map_err(|e| format!("{e:#}"))?
        .port
        .get();
    let worker = crate::hook::start_worker(home).unwrap_or_else(|e| {
        eprintln!("(the worker did not start: {e:#})");
        None
    });
    let viewer = (!view_held(home))
        .then(|| {
            forget_outcome(home);
            crate::hook::spawn_detached(home, &["view", "--resident"])
        })
        .flatten();
    // Each is reaped when it leaves: this run may serve on in the foreground, and a child it
    // never waited for would stay a zombie under it (Codex on #378).
    for mut child in [worker, viewer].into_iter().flatten() {
        std::thread::spawn(move || child.wait());
    }
    let listening = format!("listening {port}");
    let deadline = Instant::now() + wait;
    loop {
        let outcome = outcome(home);
        if view_held(home) && outcome.as_deref() == Some(listening.as_str()) {
            return Ok(port);
        }
        if Instant::now() >= deadline {
            return Err(outcome.unwrap_or_else(|| "it did not start".into()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The resident viewer's address, with the token of its file, and `--open` through the opener
/// page, which the viewer removes when the browser brings the token (R7).
fn show_resident(home: &Path, port: u16, open: bool) -> Result<()> {
    let url = resident_address(home, port).ok_or_else(|| {
        anyhow!("the resident viewer moved or its token file cannot be read; run this again")
    })?;
    println!("{url}\n(the resident viewer: bookmark this address; it stays up)");
    if open {
        match opener_page(home, port, &url) {
            Ok(page) => open_browser(&page),
            Err(e) => eprintln!("(could not write the page for the browser: {e})"),
        }
    }
    Ok(())
}

/// The resident viewer's address once `bring_up` saw it listen on `port`: the port and the token
/// read as one pair under config.lock, where every move writes the port before the token, so a
/// new token made in between never goes out with the old port (Codex on W6). None when the saved
/// port is another by then, or the token file cannot be read.
fn resident_address(home: &Path, port: u16) -> Option<String> {
    let _held = crate::settings::config_lock(home).ok()?;
    let token = file_token(home)
        .filter(|_| crate::config::view(home).is_ok_and(|v| v.port.get() == port))?;
    Some(format!("http://127.0.0.1:{port}/#t={token}"))
}

/// `oboete view --new-token` (R6): a new token file and, in a resident home, the next free port
/// in `[view] port`, so the viewer comes back on a new address: the old one's tick sees the port
/// change and it leaves, and the worker starts it again. Whether the port moved, and the address
/// to bookmark. The move comes first: one that fails changes nothing, and the old bookmark keeps
/// working; a token write that fails after it is mended by running the command again.
pub fn new_token(home: &Path) -> Result<(bool, String)> {
    let _config = crate::settings::config_lock(home)?;
    let (port, listener, written) = rotate(home)?;
    written?;
    let token = file_token(home).ok_or_else(|| anyhow!("the new token file"))?;
    Ok((
        listener.is_some(),
        format!("http://127.0.0.1:{port}/#t={token}"),
    ))
}

/// `new_token`'s two steps, the port's listener kept: the page's new token serves on it at once
/// (`Viewer::view_token`), the command lets it go. An error changed nothing; the token write's
/// own result comes after the move it follows. The caller holds config.lock, so new tokens are
/// made one at a time, by the command and the page alike.
fn rotate(home: &Path) -> Result<(u16, Option<TcpListener>, Result<()>)> {
    std::fs::create_dir_all(home.join("state"))?;
    anyhow::ensure!(owner_only(home), NOT_PRIVATE);
    let from = crate::config::view(home)?.port.get();
    let (port, listener) = if resident_home(home) {
        let (port, listener) = (from..=u16::MAX)
            .skip(1)
            .find_map(|p| Some((p, TcpListener::bind(("127.0.0.1", p)).ok()?)))
            .ok_or_else(|| anyhow!("no free port after {from}"))?;
        crate::settings::set_view_port(home, port)?;
        (port, Some(listener))
    } else {
        (from, None)
    };
    Ok((port, listener, write_token(home)))
}

/// 16 bytes of the OS generator, in lower-case hex.
fn fresh_token() -> Result<String> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| anyhow!("random token: {e}"))?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// The outcome of a resident start on a filesystem whose modes keep no file its owner's alone.
pub(crate) const NOT_PRIVATE: &str =
    "this home's filesystem cannot keep the page's token to its owner";

/// The outcome of a resident start whose port another program or home holds.
pub(crate) const PORT_IN_USE: &str = "port in use";

/// How often the resident viewer looks at its home, and the worker at the viewer (R4).
const MINUTE: Duration = Duration::from_secs(60);

/// How long the worker waits to start the viewer again after "port in use" (R4).
const AFTER_PORT_IN_USE: Duration = Duration::from_secs(600);

/// Whether the home's files can be its owner's alone, which the token file needs (R6).
fn owner_only(home: &Path) -> bool {
    own_state(home)
        && std::fs::File::open(home.join("state")).is_ok_and(|d| crate::keyfile::private_fs(&d))
}

/// Whether a viewer holds `state/view.lock` now.
fn view_held(home: &Path) -> bool {
    view_lock(&home.join("state"))
        .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

/// `state/view-outcome`, as the resident viewer last wrote it.
fn outcome(home: &Path) -> Option<String> {
    std::fs::read_to_string(home.join("state").join("view-outcome")).ok()
}

/// R13: a read-only status, without starting a viewer, touching its token or creating a lock.
pub(crate) fn resident_line(home: &Path, port: u16) -> String {
    let outcome = outcome(home);
    if resident_up(
        outcome.as_deref(),
        port,
        crate::worker::lock_held(&home.join("state/view.lock")),
    ) {
        return format!("page: http://127.0.0.1:{port} is up");
    }
    let why = match outcome.as_deref().map(str::trim) {
        Some(PORT_IN_USE) => format!("another program or another oboete home holds port {port}"),
        Some(why) if !why.is_empty() => why.to_owned(),
        _ => "it has not started".to_owned(),
    };
    format!("page: not running: {why}; run `oboete view`")
}

pub(crate) fn resident_up(outcome: Option<&str>, port: u16, held: bool) -> bool {
    held && outcome.map(str::trim) == Some(format!("listening {port}").as_str())
}

/// Before a viewer is started: the outcome one that is gone left is not the new one's (Codex on
/// #378). Called only while no viewer holds the lock, so a live viewer's outcome is never taken.
fn forget_outcome(home: &Path) {
    let _ = std::fs::remove_file(home.join("state").join("view-outcome"));
}

/// Replaces `state/view-outcome` whole, so a reader never sees half of it.
fn say(home: &Path, what: &str) -> Result<()> {
    let state = home.join("state");
    let next = state.join("view-outcome.next");
    // Made anew, so a link planted before is not written through.
    clear(&next)?;
    let mut file = (std::fs::OpenOptions::new().write(true).create_new(true)).open(&next)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    file.write_all(what.as_bytes())?;
    std::fs::rename(&next, state.join("view-outcome"))?;
    Ok(())
}

/// How a `Starter` starts the viewer: the child it reaps.
type Spawn = Box<dyn FnMut(&Path) -> Option<std::process::Child>>;

enum ViewerChild {
    Owned(std::process::Child),
    #[cfg(target_os = "linux")]
    Inherited(u32),
}

impl ViewerChild {
    fn id(&self) -> u32 {
        match self {
            Self::Owned(child) => child.id(),
            #[cfg(target_os = "linux")]
            Self::Inherited(pid) => *pid,
        }
    }

    fn reaped(&mut self) -> bool {
        match self {
            Self::Owned(child) => match child.try_wait() {
                Ok(Some(_)) => true,
                #[cfg(target_os = "linux")]
                Err(error) if error.raw_os_error() == Some(libc::ECHILD) => true,
                _ => false,
            },
            #[cfg(target_os = "linux")]
            Self::Inherited(pid) => {
                let mut status = 0;
                // SAFETY: a positive, known viewer PID inherited from this same process's old
                // image. Specific nonblocking wait cannot reap another child.
                let waited = unsafe {
                    libc::waitpid(
                        i32::try_from(*pid).expect("validated PID"),
                        &mut status,
                        libc::WNOHANG,
                    )
                };
                waited > 0
                    || waited == -1
                        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
            }
        }
    }
}

/// What starts the resident viewer for a resident worker (R4): at the worker's start and then at
/// most once a minute, where the worker looks at its backup deadline, when the home's files can
/// be its owner's and nothing holds `state/view.lock`; after "port in use", only every 10
/// minutes. It keeps the viewer it started and reaps it before it starts another.
pub struct Starter {
    every: Duration,
    after_busy: Duration,
    next: Instant,
    child: Option<ViewerChild>,
    spawn: Spawn,
}

impl Starter {
    pub fn new() -> Self {
        let starter = Self::with(
            MINUTE,
            AFTER_PORT_IN_USE,
            Box::new(|home| crate::hook::spawn_detached(home, &["view", "--resident"])),
        );
        #[cfg(target_os = "linux")]
        let starter = {
            let child = crate::executable::take_viewer().map(ViewerChild::Inherited);
            let next = if child.is_some() {
                Instant::now() + starter.every
            } else {
                starter.next
            };
            Self {
                child,
                next,
                ..starter
            }
        };
        starter
    }

    fn with(every: Duration, after_busy: Duration, spawn: Spawn) -> Self {
        Self {
            every,
            after_busy,
            next: Instant::now(),
            child: None,
            spawn,
        }
    }

    pub fn due(&mut self, home: &Path) {
        // Reaped as soon as it has left, so it is no zombie for a minute.
        self.reap();
        let now = Instant::now();
        if now < self.next || self.child.is_some() {
            return;
        }
        self.next = now + self.every;
        // The wait is the outcome's own age, so a worker started anew waits too (Codex on #378).
        let busy = outcome(home).as_deref() == Some(PORT_IN_USE)
            && std::fs::metadata(home.join("state").join("view-outcome"))
                .and_then(|m| m.modified())
                .is_ok_and(|at| at.elapsed().is_ok_and(|age| age < self.after_busy));
        if busy {
            return;
        }
        if !owner_only(home) || view_held(home) {
            return;
        }
        forget_outcome(home);
        self.child = (self.spawn)(home).map(ViewerChild::Owned);
    }

    pub(crate) fn reap(&mut self) {
        if self.child.as_mut().is_some_and(ViewerChild::reaped) {
            self.child = None;
        }
    }

    pub(crate) fn child_id(&self) -> Option<u32> {
        self.child.as_ref().map(ViewerChild::id)
    }
}

/// A resident viewer that started: its lock, held while it serves, its listener and itself.
struct Resident {
    lock: std::fs::File,
    listener: TcpListener,
    viewer: Arc<Viewer>,
}

/// The resident viewer (docs/resident.md R5, R6, R8): on `[view] port`, with the token of
/// `state/view-token` and no checkout. It says in `state/view-outcome` that it listens, or why it
/// did not start; one that finds another holding `state/view.lock` exits and writes nothing. Once
/// a minute it looks at its home and leaves when `leaving` says so, while no connection is live
/// and no save runs (R4).
pub fn resident(home: &Path) -> Result<()> {
    let Some(Resident {
        lock,
        listener,
        viewer,
    }) = listen(home)?
    else {
        return Ok(());
    };
    let id = viewer.home_lock;
    let looking = Arc::clone(&viewer);
    std::thread::spawn(move || {
        let mut seen = looking.requests.load(Ordering::SeqCst);
        let mut executable = crate::executable::Watch::default();
        loop {
            std::thread::sleep(MINUTE);
            let now = looking.requests.load(Ordering::SeqCst);
            let quiet = std::mem::replace(&mut seen, now) == now;
            if let Some(why) = looking.leaving(id, quiet) {
                if looking.may_leave() {
                    if !matches!(why, Leaving::Gone) {
                        let _ = say(&looking.home, &format!("left: {}", why.text()));
                    }
                    std::process::exit(0);
                }
                continue;
            }
            match executable.change() {
                crate::executable::Change::Replaced
                    if crate::executable::startup_ready(&looking.home) =>
                {
                    looking.replace(|| {
                        crate::executable::exec(crate::executable::Role::Viewer, id, None)
                    });
                }
                crate::executable::Change::Missing if looking.may_leave() => {
                    let _ = say(&looking.home, "left: executable missing");
                    std::process::exit(0);
                }
                _ => {}
            }
        }
    });
    accept(listener, &viewer);
    drop(lock);
    Ok(())
}

/// `state/view.lock`, a regular file only: a link or a FIFO planted while another user could write
/// in `state` is replaced, not followed or waited on (Codex on #376).
fn view_lock(state: &Path) -> Result<std::fs::File> {
    let path = state.join("view.lock");
    match std::fs::symlink_metadata(&path) {
        Ok(m) if !m.is_file() => clear(&path)?,
        // One made under a umask that took the owner's write away is given it back, not
        // replaced: a viewer may hold it (Codex on #376).
        Ok(_) => crate::db::private(&path, 0o600),
        Err(_) => {}
    }
    let mut file = std::fs::OpenOptions::new();
    file.create(true).truncate(false).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut file, libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = file.open(&path)?;
    // A new one: the umask takes bits from the mode asked for.
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(file)
}

/// Takes away whatever is at a fixed name in `state` before it is made anew: what was planted
/// there while another user could write in `state`, a folder too (Codex on #376).
fn clear(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Why the resident viewer leaves (R4).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Leaving {
    /// Its home is another now, or none: nothing is written into it.
    Gone,
    Moved,
    NotResident,
}

impl Leaving {
    fn text(self) -> &'static str {
        match self {
            Leaving::Gone => "its home is gone",
            Leaving::Moved => "[view] port names another port",
            Leaving::NotResident => "the home is no longer resident",
        }
    }
}

/// The resident viewer's start: the lock first, then `starting`, the filesystem check, the token
/// file, the port, and `listening <port>`; a start that fails says why instead.
fn listen(home: &Path) -> Result<Option<Resident>> {
    crate::executable::check_home(home, Some(crate::executable::Role::Viewer))?;
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    // A `state` folder another user could change is not written into at all (CodeRabbit on #376).
    if !own_state(home) {
        eprintln!(
            "oboete: {} is not this user's alone; no resident viewer",
            state.display()
        );
        return Ok(None);
    }
    let lock = view_lock(&state)?;
    let home_lock = crate::worker::file_id(lock.metadata());
    crate::executable::check_lock(Some(crate::executable::Role::Viewer), home_lock)?;
    match crate::worker::try_lock(&lock) {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let failed = |why: &str| say(home, why).map(|()| None);
    say(home, "starting")?;
    if !owner_only(home) {
        return failed(NOT_PRIVATE);
    }
    if let Err(e) = ensure_token(home) {
        return failed(&format!("{e:#}"));
    }
    let port = match crate::config::view(home) {
        Ok(v) => v.port.get(),
        Err(e) => return failed(&format!("{e:#}")),
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return failed(PORT_IN_USE),
        Err(e) => return failed(&e.to_string()),
    };
    say(home, &format!("listening {port}"))?;
    Ok(Some(Resident {
        lock,
        listener,
        viewer: Arc::new(Viewer {
            home_lock,
            ..Viewer::new(home, None, port, Token::File)
        }),
    }))
}

/// R6: whether `state` may hold the token: a folder of this user's, not a link, in a home of this
/// user's, neither of which another user may write into, made so first as the stores make the
/// home. Off Unix the resident viewer does not run.
fn own_state(home: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments and cannot fail.
        let me = unsafe { libc::geteuid() };
        let state = home.join("state");
        // The home may be a link the owner made; `state` is the viewer's own, never one.
        let ours =
            |m: std::io::Result<std::fs::Metadata>| m.is_ok_and(|m| m.is_dir() && m.uid() == me);
        if !ours(std::fs::metadata(home)) || !ours(std::fs::symlink_metadata(&state)) {
            return false;
        }
        crate::db::private(home, 0o700);
        crate::db::private(&state, 0o700);
        let closed = |m: std::io::Result<std::fs::Metadata>| m.is_ok_and(|m| m.mode() & 0o022 == 0);
        closed(std::fs::metadata(home)) && closed(std::fs::symlink_metadata(&state))
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        false
    }
}

/// R6: the resident viewer's token, `state/view-token`, when it is a regular file only its owner
/// may read, on a filesystem that enforces that (keyfile's check), and exactly 32 lower-case hex
/// characters. Anything else is none, and every request is refused.
fn file_token(home: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        // No link is followed, and a FIFO planted there does not stall the request.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(home.join("state").join("view-token"))
            .ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 || !crate::keyfile::private_fs(&file) {
            return None;
        }
        let mut text = String::new();
        file.take(33).read_to_string(&mut text).ok()?;
        (text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then_some(text)
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        None
    }
}

/// R6: at the resident viewer's start, under its lock, a new token file when the file is not as
/// `file_token` takes it (missing, or another shape, or readable by others).
fn ensure_token(home: &Path) -> Result<()> {
    if file_token(home).is_some() {
        return Ok(());
    }
    write_token(home)
}

/// A new token file: staged (`stage_token`), then put in place (`put_token`).
fn write_token(home: &Path) -> Result<()> {
    let (staged, _) = stage_token(home)?;
    put_token(home, &staged)
}

/// A new token in a file beside the token file, with mode 0600 under a name of this process's (a
/// viewer's start and `--new-token` may write at once), and synced: the file and the token.
fn stage_token(home: &Path) -> Result<(PathBuf, String)> {
    let staged = home
        .join("state")
        .join(format!("view-token.{}.tmp", std::process::id()));
    // Made anew, so it has this mode and is no link planted before.
    clear(&staged)?;
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut file, 0o600);
    let mut file = file.open(&staged)?;
    // The umask takes bits from the mode asked for, the owner's own read among them.
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    let token = fresh_token()?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok((staged, token))
}

/// The staged token renamed over the token file, and its folder synced.
fn put_token(home: &Path, staged: &Path) -> Result<()> {
    let state = home.join("state");
    std::fs::rename(staged, state.join("view-token"))?;
    std::fs::File::open(&state)?.sync_all()?;
    Ok(())
}

fn accept(mut listener: TcpListener, viewer: &Arc<Viewer>) {
    loop {
        let Ok((stream, _)) = listener.accept() else {
            continue;
        };
        // The connection that woke it for a move or an end, and any other that reached the old
        // address before it, is closed unread.
        match viewer
            .next
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            Some(Next::Move(port, to)) => {
                listener = to;
                viewer.port.store(port, Ordering::SeqCst);
                continue;
            }
            Some(Next::End) => break,
            None => {}
        }
        // Over the cap, the connection is dropped here: closed before a byte is read.
        let Some(slot) = Slot::take(viewer) else {
            continue;
        };
        // A browser keeps idle pre-connected sockets open; one thread each keeps them from
        // stalling the rest.
        std::thread::spawn(move || slot.0.serve(stream));
    }
    drop(listener);
    // The request that ended it is answered before the run returns.
    while viewer.live.load(Ordering::SeqCst) != 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Writes all of `out` within `within`, however slowly the client reads; false once the time is
/// up, the client is gone, or the deadline cannot be set.
fn send(stream: &mut TcpStream, out: &[u8], within: Duration) -> bool {
    let deadline = Instant::now() + within;
    let mut sent = 0;
    while sent < out.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_write_timeout(Some(left)).is_err() {
            return false;
        }
        match stream.write(&out[sent..]) {
            // A stop and continue interrupts a write that has a timeout (#283).
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Ok(0) | Err(_) => return false,
            Ok(n) => sent += n,
        }
    }
    true
}

/// The page `--open` hands the browser opener: owner-only, it sends the browser on to the
/// address, so that the token goes on no command line, where the machine's other users could
/// read it while the opener runs (#269).
fn opener_page(home: &Path, port: u16, url: &str) -> std::io::Result<PathBuf> {
    let page = opener_path(home, port);
    // Made anew, so it has this mode and is no link planted before.
    let _ = std::fs::remove_file(&page);
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut file, 0o600);
    file.open(&page)?.write_all(
        format!(
            "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\
             <meta http-equiv=\"refresh\" content=\"0;url={url}\"><title>oboete</title>\n"
        )
        .as_bytes(),
    )?;
    Ok(page)
}

/// The page for `port`: one per port, so another viewer of this home neither replaces nor removes
/// it. Absolute: the opener, and a browser already running, resolve a relative path in their own
/// working directory.
fn opener_path(home: &Path, port: u16) -> PathBuf {
    let page = opener_dir(home).join(format!("view-open-{port}.html"));
    std::path::absolute(&page).unwrap_or(page)
}

/// Where the page goes: the home, where the page's mode keeps it the user's own. A Windows file
/// takes its folder's ACL instead, and the user's local app data folder is theirs alone, where a
/// folder given as --home may not be.
fn opener_dir(home: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(dir);
    }
    home.to_path_buf()
}

/// The page as the opener takes it: a Windows browser under WSL reads it by its Windows path
/// (`wslview` takes either); the page's own path when `wslpath` fails.
fn opener_arg(page: &Path, wsl: bool) -> std::ffi::OsString {
    wsl.then(|| {
        std::process::Command::new("wslpath")
            .arg("-w")
            .arg(page)
            .output()
    })
    .and_then(Result::ok)
    .filter(|o| o.status.success())
    .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
    .unwrap_or_else(|| page.as_os_str().to_owned())
}

/// Best effort.
fn open_browser(page: &Path) {
    let wsl = std::fs::read_to_string("/proc/version")
        .is_ok_and(|v| v.to_ascii_lowercase().contains("microsoft"));
    let page = opener_arg(page, wsl);
    let openers: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["open"]]
    } else if cfg!(windows) {
        // Not `cmd /C start`: cmd would split a path at a `&`.
        &[&["explorer.exe"]]
    } else if wsl {
        &[&["wslview"], &["explorer.exe"]]
    } else {
        &[&["xdg-open"]]
    };
    let launched = openers.iter().any(|o| {
        std::process::Command::new(o[0])
            .args(&o[1..])
            .arg(&page)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|mut child| {
                // Reap it, or it sits as a zombie for as long as the viewer runs.
                std::thread::spawn(move || child.wait());
            })
            .is_ok()
    });
    if !launched {
        eprintln!("(no browser opener found; paste the address into one)");
    }
}

/// A save's answer: what it saved, or `{code, field}` for the page to put in words.
fn saved(result: std::result::Result<Value, crate::settings::Refusal>) -> Response {
    match result {
        Ok(v) => Response::json(&v),
        Err(r) => Response::new(
            r.status,
            "application/json",
            serde_json::to_vec(&json!({"code": r.code, "field": r.field})).unwrap_or_default(),
        ),
    }
}

/// `saved`'s refusal, from the viewer's own operations.
fn refused(status: u16, code: &'static str, field: &str) -> Response {
    saved(Err(crate::settings::Refusal {
        status,
        code,
        field: field.into(),
    }))
}

/// A body that must be `{}`: an operation that takes no input says so.
fn empty_object(body: &[u8]) -> bool {
    matches!(
        serde_json::from_slice::<serde_json::Map<String, Value>>(body),
        Ok(fields) if fields.is_empty()
    )
}

impl Viewer {
    fn new(home: &Path, cwd: Option<PathBuf>, port: u16, token: Token) -> Self {
        Self {
            home: home.to_path_buf(),
            home_lock: None,
            cwd,
            port: AtomicU16::new(port),
            next: Mutex::new(None),
            token,
            saving: Mutex::new(()),
            moving: Mutex::new(()),
            recovery: crate::settings::recovery::Recovery::default(),
            maintenance: crate::settings::maintenance::Maintenance::default(),
            agents: crate::setup::agents::Agents::default(),
            opener: Mutex::new(None),
            live: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
        }
    }

    fn port(&self) -> u16 {
        self.port.load(Ordering::SeqCst)
    }

    /// The accept loop takes connections on `listener`, of `port`, from its next one on, and the
    /// old address stops answering. A move asked again before the loop took one replaces it. The
    /// outcome names the new port first (R5), as `oboete view` and doctor read it: the listener is
    /// bound, so a connection to it waits for the loop.
    fn move_to(&self, port: u16, listener: TcpListener) {
        let _ = say(&self.home, &format!("listening {port}"));
        self.wake(Next::Move(port, listener));
    }

    /// A connection to the port the loop waits on wakes it.
    fn wake(&self, next: Next) {
        *self.next.lock().unwrap_or_else(PoisonError::into_inner) = Some(next);
        let at = std::net::SocketAddr::from(([127, 0, 0, 1], self.port()));
        let _ = TcpStream::connect_timeout(&at, Duration::from_secs(1));
    }

    fn replace(&self, exec: impl FnOnce() -> std::io::Result<()>) {
        if self.may_leave() {
            let _ = exec();
            // A returned exec installed no image; retain the lock/listener and serving.
            self.closing.store(false, Ordering::SeqCst);
        }
    }

    /// Whether the resident viewer may leave now: no connection open, so no save either. Once it
    /// may, it takes no new connection, and its exit cuts none off (Codex on #378).
    fn may_leave(&self) -> bool {
        self.closing.store(true, Ordering::SeqCst);
        let free = self.live.load(Ordering::SeqCst) == 0;
        if !free {
            self.closing.store(false, Ordering::SeqCst);
        }
        free
    }

    /// R4: why the resident viewer leaves at a look, if it does: its home is gone (its lock file
    /// is another than `lock`, or none), `[view] port` names another port, or config.toml loads,
    /// does not say `resident = true`, and no request came since the last look (`quiet`). A file
    /// that does not load leaves it as it is.
    fn leaving(&self, lock: crate::worker::FileId, quiet: bool) -> Option<Leaving> {
        let state = self.home.join("state");
        if crate::worker::file_id(std::fs::metadata(state.join("view.lock"))) != lock {
            return Some(Leaving::Gone);
        }
        if crate::config::view(&self.home).is_ok_and(|v| v.port.get() != self.port()) {
            return Some(Leaving::Moved);
        }
        (quiet && crate::config::worker(&self.home).is_ok_and(|w| !w.resident))
            .then_some(Leaving::NotResident)
    }

    fn serve(&self, mut stream: TcpStream) {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + REQUEST_TIME;
        let mut buf = Vec::with_capacity(2048);
        let mut chunk = [0u8; 4096];
        // False once the peer is gone or the request's time is up.
        let mut more = |stream: &mut TcpStream, buf: &mut Vec<u8>| loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
                return false;
            }
            match stream.read(&mut chunk) {
                // A stop and continue interrupts a read that has a timeout (#283).
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Ok(0) | Err(_) => return false,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    return true;
                }
            }
        };
        let (head, at, head_only) = loop {
            if !more(&mut stream, &mut buf) {
                return;
            }
            let mut headers = [httparse::EMPTY_HEADER; 64];
            let mut req = httparse::Request::new(&mut headers);
            match req.parse(&buf) {
                // A head is at most `MAX_HEAD` however its reads fell (#53).
                Ok(httparse::Status::Complete(at)) if at <= MAX_HEAD => {
                    let method = req.method.unwrap_or("");
                    let pairs: Vec<(&str, &str)> = req
                        .headers
                        .iter()
                        .map(|h| (h.name, std::str::from_utf8(h.value).unwrap_or("")))
                        .collect();
                    break (
                        self.head(method, req.path.unwrap_or(""), &pairs),
                        at,
                        method == "HEAD",
                    );
                }
                Ok(httparse::Status::Partial) if buf.len() < MAX_HEAD => continue,
                // Malformed, too many headers, or a head over the cap.
                _ => break (Head::Answer(Response::text(400, "bad request")), 0, false),
            }
        };
        let len = match &head {
            Head::Answer(_) => 0,
            Head::Body(len, ..) => *len,
        };
        // Read only once the head has passed every check (`save_gate`).
        while buf.len() < at + len {
            if !more(&mut stream, &mut buf) {
                return;
            }
        }
        let body = &buf[at..at + len];
        let resp = match head {
            Head::Answer(r) => r,
            Head::Body(_, save, given) => self.take(save, body, &given),
        };
        send(&mut stream, &resp.bytes(head_only), ANSWER_TIME);
    }

    /// Typed operations with a body go through `save_gate`; other requests use `route`.
    fn head(&self, method: &str, target: &str, headers: &[(&str, &str)]) -> Head {
        let (cap, save): (usize, Save) = match (method, target) {
            ("POST", "/api/doctor") => (MAX_BODY, |v, b, _| v.doctor(b)),
            ("POST", "/api/settings") => (MAX_BODY, Self::save),
            ("POST", "/api/settings/recovery/preview") => {
                (MAX_BODY, |v, b, _| v.recovery_preview(b))
            }
            ("POST", "/api/settings/recovery/start") => (MAX_BODY, |v, b, _| v.recovery_start(b)),
            ("POST", "/api/providers") => (MAX_BODY, |v, b, _| v.save_provider(b)),
            ("POST", "/api/providers/key") => (MAX_KEY_BODY, |v, b, _| v.save_provider_key(b)),
            ("POST", "/api/providers/test/preview") => {
                (MAX_BODY, |v, b, _| v.preview_provider_test(b))
            }
            ("POST", "/api/providers/test") => (MAX_BODY, |v, b, _| v.test_provider(b)),
            ("POST", "/api/key") => (MAX_KEY_BODY, |v, b, _| v.save_key(b)),
            ("POST", "/api/resume") => (MAX_BODY, |v, b, _| v.resume(b)),
            ("POST", "/api/privacy/exclude") => (MAX_BODY, |v, b, _| v.exclude(b)),
            ("POST", "/api/claims/correct") => (MAX_BODY, |v, b, _| v.claim_correct(b)),
            ("POST", "/api/claims/mute") => (MAX_BODY, |v, b, _| v.claim_mute(b)),
            ("POST", "/api/preferences") => (MAX_BODY, |v, b, _| v.preference(b)),
            ("POST", "/api/maintenance/preview") => (MAX_BODY, |v, b, _| v.maintenance_preview(b)),
            ("POST", "/api/setup/preview") => (MAX_BODY, |v, b, _| v.agent_preview(b)),
            ("POST", "/api/setup/start") => (MAX_BODY, |v, b, _| v.agent_start(b)),
            ("POST", "/api/maintenance/start") => (MAX_BODY, |v, b, _| v.maintenance_start(b)),
            ("POST", "/api/view/resident") => (MAX_BODY, |v, b, _| v.view_resident(b)),
            ("POST", "/api/view/token") => (MAX_BODY, Self::view_token),
            _ => return Head::Answer(self.route(method, target, headers)),
        };
        // `save_gate` passes a request with exactly one, the viewer's.
        let given = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("x-oboete-token"))
            .map_or("", |(_, t)| t);
        match self.save_gate(headers, cap) {
            Ok(len) => Head::Body(len, save, given.to_owned()),
            Err(r) => Head::Answer(r),
        }
    }

    /// A save once its body is in, only while the token its request brought is still this
    /// viewer's: the head was checked before the body, which the client may send up to the
    /// request's deadline later, after a new token (R6).
    fn take(&self, save: Save, body: &[u8], given: &str) -> Response {
        if !self.token_ok(Some(given)) {
            return Response::text(401, "missing or wrong token");
        }
        save(self, body, given)
    }

    /// Spec 6.6 for the writes with a body, all on the head, before any byte of the body is
    /// read: no chunked framing, the Host and token as for every `/api` request, an `Origin` of
    /// this viewer (a browser sends one on every POST; the page's fetch asks for it with
    /// `referrerPolicy: 'same-origin'`, as the document's `no-referrer` would make it `null`),
    /// JSON, and one `Content-Length` of at most `cap`.
    fn save_gate(
        &self,
        headers: &[(&str, &str)],
        cap: usize,
    ) -> std::result::Result<usize, Response> {
        // Exactly one: a request with two of these is no browser's.
        let only = |name: &str| {
            let mut all = headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name));
            match (all.next(), all.next()) {
                (Some((_, v)), None) => Some(*v),
                _ => None,
            }
        };
        if headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("transfer-encoding"))
        {
            return Err(Response::text(400, "requests carry no chunked body"));
        }
        if !self.host_ok(only("host")) {
            return Err(Response::text(
                403,
                "open the viewer through 127.0.0.1 or localhost",
            ));
        }
        if !self.token_ok(only("x-oboete-token")) {
            return Err(Response::text(401, "missing or wrong token"));
        }
        self.token_arrived();
        let origin = only("origin").and_then(|o| o.strip_prefix("http://"));
        if !self.host_ok(origin) {
            return Err(Response::text(403, "a save comes from this viewer's page"));
        }
        // The media type itself, parameters aside: `application/jsonp` is not JSON.
        let media = only("content-type").map(|t| t.split(';').next().unwrap_or("").trim());
        if !media.is_some_and(|m| m.eq_ignore_ascii_case("application/json")) {
            return Err(Response::text(400, "a save is JSON"));
        }
        let len = match only("content-length") {
            Some(l) if !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()) => {
                l.parse::<usize>().unwrap_or(usize::MAX)
            }
            _ => return Err(Response::text(400, "a save declares one length")),
        };
        if len > cap {
            return Err(Response::text(413, "a save's body is over its cap"));
        }
        Ok(len)
    }

    fn doctor(&self, body: &[u8]) -> Response {
        if !empty_object(body) {
            return Response::text(400, "doctor takes an empty JSON object");
        }
        // Reuse the viewer's operation gate so simultaneous clients cannot multiply copies.
        let Ok(_running) = self.saving.try_lock() else {
            return Response::text(503, "the viewer is busy");
        };
        Response::json(&json!(crate::setup::doctor_report(&self.home)))
    }

    /// The settings as saved. The resident viewer binds a saved port other than its own first
    /// and moves onto it at once with a new token, so the page goes on at the new address and the
    /// old bookmark, whose port another program may take now, holds a token that no longer works
    /// (R5, R6); a port it cannot bind is refused before anything is written. A foreground run
    /// keeps its own port: `[view] port` is the resident viewer's.
    fn save(&self, body: &[u8], given: &str) -> Response {
        let _moving = self.moving.lock().unwrap_or_else(PoisonError::into_inner);
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(_config) = crate::settings::config_lock(&self.home) else {
            return refused(500, "write_failed", "");
        };
        // Checked again under the hold new tokens are made under: a save that waited behind one
        // brought the old token and changes nothing, not even a port the new token moved from.
        if !self.token_ok(Some(given)) {
            return Response::text(401, "missing or wrong token");
        }
        let was = crate::config::view(&self.home).map(|v| v.port.get());
        // The resident address changes: the resident page moves now, or, saved on a foreground
        // run's page, the resident viewer comes back there at its tick (R4). Its token goes with
        // the old address either way.
        let moves = |port: &u16| match self.token {
            Token::File => *port != self.port(),
            Token::Run(_) => file_token(&self.home).is_some() && was.as_ref().ok() != Some(port),
        };
        let posted = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|posted| posted["view"]["port"].as_u64())
            .and_then(|port| u16::try_from(port).ok())
            .filter(|&port| port != 0);
        match posted.filter(moves) {
            Some(port) => self.save_with_new_token(body, port),
            None => saved(
                crate::settings::save_held(&self.home, body)
                    .map(|report| self.runtime(report, self.port())),
            ),
        }
    }

    /// A resident port saved with a new token, still under `save`'s hold of config.lock, where
    /// new tokens are made: the resident page's new port bound, the settings checked and staged,
    /// a new token staged and put in place, the settings written, and only then the resident page
    /// moved. A refusal or a failure before the token is in place changes nothing (the staged
    /// settings go when dropped); settings that cannot follow it leave the old port with a token
    /// that no page holds (Codex and CodeRabbit on W6). A foreground run moves nothing and answers
    /// no address: the token is the resident viewer's, which its page starts or `oboete view`
    /// prints.
    fn save_with_new_token(&self, body: &[u8], port: u16) -> Response {
        let listener = match self.token {
            Token::File => match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => Some(listener),
                Err(_) => return refused(409, "port_unavailable", "view.port"),
            },
            Token::Run(_) => None,
        };
        let settings = match crate::settings::stage_held(&self.home, body) {
            Ok(settings) => settings,
            Err(refusal) => return saved(Err(refusal)),
        };
        let Ok((staged, token)) = stage_token(&self.home) else {
            return refused(500, "write_failed", "");
        };
        // A folder that is not synced after the rename still has the token in place.
        if put_token(&self.home, &staged).is_err() && file_token(&self.home) != Some(token.clone())
        {
            let _ = clear(&staged);
            return refused(500, "write_failed", "");
        }
        if settings.is_some_and(|s| s.commit().is_err()) {
            return refused(500, "token_replaced", "");
        }
        let report = crate::settings::show(&self.home);
        let Some(listener) = listener else {
            return saved(Ok(self.runtime(report, self.port())));
        };
        self.move_to(port, listener);
        let mut report = self.runtime(report, port);
        report["view_runtime"]["url"] = json!(format!("http://127.0.0.1:{port}/#t={token}"));
        saved(Ok(report))
    }

    /// The settings with this viewer's own state beside them: the port it serves on, or moves to
    /// with this answer (then with the address, which carries the new token), and whether it is
    /// the resident viewer or a foreground run.
    fn runtime(&self, mut report: Value, port: u16) -> Value {
        if report.get("error").is_none() {
            let mode = match self.token {
                Token::Run(_) => "foreground",
                Token::File => "resident",
            };
            report["view_runtime"] = json!({"port": port, "mode": mode});
        }
        report
    }

    /// The page's `oboete view --new-token` (R6): a new token and, in a resident home, the next
    /// free port, which this viewer serves on at once, so the old bookmark and every page still on
    /// the old address stop working. The answer is the new address. A foreground run's token is
    /// its own and ends with it (spec 6.6): it has none to replace. `given` is the token the
    /// request brought: one that waited while another new token was made (here or by the command)
    /// brought the old one, and makes none (Codex on W6).
    fn view_token(&self, body: &[u8], given: &str) -> Response {
        if !empty_object(body) {
            return Response::text(400, "a new token takes an empty JSON object");
        }
        if matches!(self.token, Token::Run(_)) {
            return refused(409, "foreground", "");
        }
        let _moving = self.moving.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(_config) = crate::settings::config_lock(&self.home) else {
            return refused(503, "unchanged", "");
        };
        if !self.token_ok(Some(given)) {
            return Response::text(401, "missing or wrong token");
        }
        let Ok((port, listener, written)) = rotate(&self.home) else {
            return refused(503, "unchanged", "");
        };
        if let Some(listener) = listener {
            self.move_to(port, listener);
        }
        // What the file holds now: a write that failed after its rename has replaced it anyway.
        let Some(token) = file_token(&self.home) else {
            return refused(500, "unknown", "");
        };
        let url = format!("http://127.0.0.1:{port}/#t={token}");
        match written {
            Ok(()) => Response::json(&json!({"url": url})),
            // The move stands; the address carries the token that works there.
            Err(_) => Response::new(
                500,
                "application/json",
                serde_json::to_vec(&json!({"code": "token_unsure", "url": url}))
                    .unwrap_or_default(),
            ),
        }
    }

    /// R7 from the page: a foreground run in a home now saved resident brings up the worker and
    /// the resident viewer as `oboete view` does there, answers with the resident address, and
    /// ends once its connections are done, as the page goes on to that address.
    fn view_resident(&self, body: &[u8]) -> Response {
        if !empty_object(body) {
            return Response::text(
                400,
                "starting the resident viewer takes an empty JSON object",
            );
        }
        if matches!(self.token, Token::File) {
            return refused(409, "resident", "");
        }
        if !resident_home(&self.home) {
            return refused(409, "not_resident", "");
        }
        let up = bring_up(&self.home, Duration::from_secs(3));
        let address = up
            .as_ref()
            .ok()
            .and_then(|&port| Some((port, resident_address(&self.home, port)?)));
        let Some((port, url)) = address else {
            let why = match &up {
                Err(why) if why.trim() == PORT_IN_USE => "port_in_use",
                Err(why) if why.trim() == NOT_PRIVATE => "not_private",
                _ => "not_started",
            };
            return refused(503, why, "");
        };
        println!(
            "(the page went on to the resident viewer at http://127.0.0.1:{port}; this run ends)"
        );
        // The accept loop takes no more connections, and returns once the live ones are done.
        self.wake(Next::End);
        Response::json(&json!({"url": url}))
    }

    fn save_provider(&self, body: &[u8]) -> Response {
        saved(crate::settings::save_provider(
            &self.home,
            &self.saving,
            body,
        ))
    }

    fn save_provider_key(&self, body: &[u8]) -> Response {
        saved(crate::settings::save_provider_key(
            &self.home,
            &self.saving,
            body,
        ))
    }

    fn preview_provider_test(&self, body: &[u8]) -> Response {
        saved(crate::settings::preview_provider_test(&self.home, body))
    }

    fn test_provider(&self, body: &[u8]) -> Response {
        saved(crate::settings::test_provider(&self.home, body))
    }

    /// A key written to its entry's key file (#94 part 3); the answer never holds it.
    fn save_key(&self, body: &[u8]) -> Response {
        saved(crate::settings::save_key(&self.home, &self.saving, body))
    }

    fn resume(&self, body: &[u8]) -> Response {
        saved(crate::settings::resume(&self.home, &self.saving, body))
    }

    fn exclude(&self, body: &[u8]) -> Response {
        saved(crate::settings::privacy::exclude(
            &self.home,
            &self.saving,
            body,
        ))
    }

    fn claim_correct(&self, body: &[u8]) -> Response {
        saved(crate::settings::claims::correct(
            &self.home,
            &self.saving,
            body,
        ))
    }

    fn claim_mute(&self, body: &[u8]) -> Response {
        saved(crate::settings::claims::mute(
            &self.home,
            &self.saving,
            body,
        ))
    }

    fn preference(&self, body: &[u8]) -> Response {
        saved(crate::settings::claims::pref_add(
            &self.home,
            &self.saving,
            body,
        ))
    }

    /// DNS rebinding: a page on another name that resolves to 127.0.0.1 sends its own Host.
    /// Browsers leave port 80 out of Host.
    fn host_ok(&self, host: Option<&str>) -> bool {
        let port = self.port();
        host.is_some_and(|h| {
            h == format!("127.0.0.1:{port}")
                || h == format!("localhost:{port}")
                || (port == 80 && (h == "127.0.0.1" || h == "localhost"))
        })
    }

    /// A token is 32 characters, so an empty or absent header never passes (R6).
    fn token_ok(&self, given: Option<&str>) -> bool {
        let want = match &self.token {
            Token::Run(t) => Some(t.clone()),
            Token::File => file_token(&self.home),
        };
        want.is_some_and(|w| {
            Sha256::digest(given.unwrap_or("").as_bytes()) == Sha256::digest(w.as_bytes())
        })
    }

    /// A request brought the token, on any path and whatever it asks: the browser has it, and
    /// the page `--open` took it there through has done its work.
    /// The resident viewer removes its port's page by name: `oboete view --open` wrote it (R7).
    fn token_arrived(&self) {
        let page = match self.token {
            Token::File => Some(opener_path(&self.home, self.port())),
            Token::Run(_) => self
                .opener
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        };
        if let Some(page) = page {
            let _ = std::fs::remove_file(page);
        }
    }

    fn recovery_preview(&self, body: &[u8]) -> Response {
        saved(self.recovery.preview(&self.home, body))
    }

    fn recovery_start(&self, body: &[u8]) -> Response {
        saved(
            self.recovery
                .start(self.maintenance_caller(), &self.home, &self.saving, body),
        )
    }

    fn maintenance_preview(&self, body: &[u8]) -> Response {
        saved(
            self.maintenance
                .preview(self.maintenance_caller(), &self.home, body),
        )
    }

    fn agent_preview(&self, body: &[u8]) -> Response {
        saved(
            self.agents
                .preview(self.maintenance_caller(), &self.home, body),
        )
    }

    fn agent_start(&self, body: &[u8]) -> Response {
        // This request keeps its original Slot until the synchronous receipt is stored.
        saved(
            self.agents
                .start(self.maintenance_caller(), &self.home, &self.saving, body),
        )
    }

    fn maintenance_caller(&self) -> Option<crate::executable::CommandCaller> {
        match &self.token {
            Token::Run(_) => Some(crate::executable::CommandCaller::Worker),
            Token::File => self.home_lock.map(crate::executable::CommandCaller::Viewer),
        }
    }

    fn maintenance_start(&self, body: &[u8]) -> Response {
        // The caller's original connection Slot remains held through native completion,
        // including when the peer disconnects; status reads use another short-lived Slot.
        let Some(caller) = self.maintenance_caller() else {
            return saved(Err(crate::settings::Refusal {
                status: 422,
                code: "maintenance_busy",
                field: String::new(),
            }));
        };
        saved(self.maintenance.start(caller, &self.home, body))
    }

    /// `--open`: the page for the browser, registered before `launch` starts the opener.
    fn open(&self, home: &Path, url: &str, launch: impl FnOnce(&Path)) {
        match opener_page(home, self.port(), url) {
            Ok(page) => {
                *self.opener.lock().unwrap_or_else(PoisonError::into_inner) = Some(page.clone());
                launch(&page);
            }
            Err(e) => eprintln!("(could not write the page for the browser: {e})"),
        }
    }

    fn route(&self, method: &str, target: &str, headers: &[(&str, &str)]) -> Response {
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
        };
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        // Reads only: the stores' writes (forget) come with milestone 5 (D11), and the settings
        // saves take `head`'s other path. OPTIONS stays 405, so no cross-site preflight passes.
        if method != "GET" && method != "HEAD" {
            return Response::text(405, "method not allowed");
        }
        // No request here has a body; refusing framing headers outright leaves nothing to
        // smuggle.
        if header("transfer-encoding").is_some() || header("content-length").is_some() {
            return Response::text(400, "requests carry no body");
        }
        if !self.host_ok(header("host")) {
            return Response::text(403, "open the viewer through 127.0.0.1 or localhost");
        }
        match path {
            "/" => return Response::new(200, "text/html; charset=utf-8", INDEX),
            "/app.js" => return Response::new(200, "text/javascript; charset=utf-8", APP_JS),
            "/app.css" => return Response::new(200, "text/css; charset=utf-8", APP_CSS),
            "/favicon.ico" => return Response::new(204, "text/plain", ""),
            p if !p.starts_with("/api/") => return Response::text(404, "not found"),
            _ => {}
        }
        if !self.token_ok(header("x-oboete-token")) {
            return Response::text(401, "missing or wrong token");
        }
        self.token_arrived();
        let q = params(query);
        let name = &path["/api/".len()..];
        // config.toml, not the stores: no error text in the answer.
        if name == "settings" {
            return Response::json(&self.runtime(crate::settings::show(&self.home), self.port()));
        }
        if name == "privacy" {
            return Response::json(&crate::settings::privacy::show(&self.home));
        }
        if name == "setup" {
            return if query.is_empty() {
                Response::json(&json!(crate::setup::readiness(&self.home)))
            } else {
                Response::text(400, "inventory carries no query")
            };
        }
        if name == "setup/operation" {
            return if query.is_empty() {
                Response::json(&self.agents.show())
            } else {
                Response::text(400, "status carries no query")
            };
        }
        if name == "maintenance" {
            return if query.is_empty() {
                Response::json(&self.maintenance.show(&self.home))
            } else {
                Response::text(400, "status carries no query")
            };
        }
        self.api(name, &q).unwrap_or_else(|e| failed(&e))
    }

    /// The label and branch of the checkout the viewer was started in, read per request as
    /// `oboete exclude` labels it (through capture's gate), so an exclusion added while the viewer
    /// runs applies to its next search (D11); none for the resident viewer (R8).
    fn checkout(&self) -> Result<Option<(String, String)>> {
        let Some(cwd) = &self.cwd else {
            return Ok(None);
        };
        let settings = crate::capture::Settings::load(&self.home)?;
        let cwd = cwd.to_string_lossy();
        let (_, repo, branch) = crate::capture::checkout(&json!({ "cwd": cwd }), &settings);
        Ok(Some((repo, branch.unwrap_or_default())))
    }

    /// Every route reads raw.db and knowledge.db (milestone 4 D11), never the old oboete.db;
    /// a bad parameter is 400 before either is opened.
    fn api(&self, name: &str, q: &HashMap<String, String>) -> Result<Response> {
        let arg = |k: &str| q.get(k).map(String::as_str).filter(|v| !v.is_empty());
        let id = arg("id").unwrap_or("");
        let answer = match name {
            "search" => {
                let mut query = match search_query(q) {
                    Ok(query) => query,
                    Err(bad) => return Ok(bad),
                };
                match self.checkout()? {
                    Some((repo, _)) => query.caller = Some(repo),
                    // No checkout (R8): every repository, as a search of all of them.
                    None => query.all |= query.repo.is_none(),
                }
                let answer = search::b::query(&self.home, &query)?;
                let (vector, why) = match answer.vector {
                    search::b::Vector::Used => (json!("used"), None),
                    search::b::Vector::Skipped(s) => (json!(s), Some(s.why())),
                };
                json!({
                    "hits": answer.hits,
                    "vector": vector,
                    "why": why,
                })
            }
            "doc" => match search::b::get(&self.home, id)? {
                Some(text) => json!({ "id": id, "text": text }),
                None => return Ok(Response::text(404, "no such document")),
            },
            "claim" => match search::b::claim(&self.home, id)? {
                Some(c) => serde_json::to_value(c)?,
                None => return Ok(Response::text(404, "no such claim")),
            },
            "feed" => {
                let (all, limit, cursor) = match (flag(q, "all"), limit(q, 50), feed_page(q)) {
                    (Ok(all), Ok(limit), Ok(cursor)) => (all, limit.min(100), cursor),
                    (Err(bad), _, _) | (_, Err(bad), _) | (_, _, Err(bad)) => return Ok(bad),
                };
                let repo = match (all, arg("repo")) {
                    (true, _) => None,
                    (false, Some(r)) => Some(r.to_owned()),
                    // No checkout (the resident viewer, R8): every repository.
                    (false, None) => self.checkout()?.map(|(repo, _)| repo),
                };
                feed(&self.home, repo.as_deref(), cursor, limit)?
            }
            "timeline" => {
                let (all, limit, before) = match (flag(q, "all"), limit(q, 50), page(q)) {
                    (Ok(all), Ok(limit), Ok(before)) => (all, limit, before),
                    (Err(bad), _, _) | (_, Err(bad), _) | (_, _, Err(bad)) => return Ok(bad),
                };
                let repo = match (all, arg("repo")) {
                    (true, _) => None,
                    (false, Some(r)) => Some(r.to_owned()),
                    (false, None) => self.checkout()?.map(|(repo, _)| repo),
                };
                let items = search::b::timeline(&self.home, repo.as_deref(), None, before, limit)?;
                // The next page starts after the last entry's time and key (never an anchor,
                // which reads around an item and would return a time's other entries again).
                let next = match items.last() {
                    Some(i) if items.len() == limit => Some(format!("{}:{}", i.when, i.key)),
                    _ => None,
                };
                json!({
                    "repo": repo,
                    "items": items,
                    "next": next,
                })
            }
            // Its text is SessionStart's, which `hook::start_text_read` gated itself.
            "context" => return Ok(Response::json(&self.context(arg("repo"))?)),
            "repos" => {
                let (current, branch) = self.checkout()?.unwrap_or_default();
                json!({ "current": current, "branch": branch, "repos": repos(&self.home)? })
            }
            "version" => json!({ "v": version(&self.home)? }),
            "stats" => stats(&self.home)?,
            _ => return Ok(Response::text(404, "not found")),
        };
        Ok(Response::json(&answered(name, answer)))
    }

    /// The Context page: what SessionStart shows a new session in `repo`'s checkout (the viewer's
    /// own by default; another repository's at the branch of its newest manifest), as `oboete
    /// inject` joins it (D4).
    fn context(&self, repo: Option<&str>) -> Result<Value> {
        let on = crate::config::inject(&self.home).is_ok_and(|i| i.session_start);
        let (repo, branch) = match repo {
            None => match self.checkout()? {
                Some(own) => own,
                // No checkout (R8): the page asks for a repository.
                None => {
                    return Ok(json!({
                        "repo": "", "branch": "", "on": on, "chars": 0, "text": "", "choose": true,
                    }));
                }
            },
            Some(r) => (r.to_owned(), newest_branch(&self.home, r)?),
        };
        // A home with no store yet shows what its first session start will: the work state
        // section alone. The page creates no store.
        let settings = crate::capture::Settings::load(&self.home)?;
        let raw = if crate::raw::exists(&self.home) {
            Some(crate::raw::open(&self.home)?)
        } else {
            None
        };
        let session = match &raw {
            Some(raw) => crate::hook::own_session("unknown".into(), raw),
            None => "unknown".into(),
        };
        let manifest = crate::hook::start_text_read(
            &self.home,
            raw.as_ref(),
            &repo,
            &branch,
            &session,
            &settings,
            true,
        )?;
        let text = crate::hook::joined(&self.home, manifest.as_ref());
        Ok(json!({
            "repo": redact::outbound(&repo),
            "branch": redact::outbound(&branch),
            "on": on,
            "chars": text.chars().count(),
            "text": text,
        }))
    }
}

/// A route's failure (D11): a restore holding raw.db past `raw::open`'s wait, or SQLite busy or
/// locked during a rebuild, is 503 "try again"; anything else is 500, its text gated (spec 6.5).
fn failed(e: &anyhow::Error) -> Response {
    let again = e.chain().any(|c| {
        c.is::<crate::raw::Restoring>()
            || matches!(
                c.downcast_ref::<rusqlite::Error>(),
                Some(rusqlite::Error::SqliteFailure(f, _))
                    if matches!(f.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
            )
    });
    if again {
        Response::text(
            503,
            "the memory is being restored or rebuilt: try again in a moment",
        )
    } else {
        Response::text(500, &redact::outbound(&format!("{e:#}")))
    }
}

/// `v` with every string through the outbound gate (spec 6.5), keys included.
/// What leaves the API: every text gated (K6), except a page's cursor, the page's own state of
/// ids and times, passed back as it came: a rule that matched it broke the paging (Codex on #373).
fn answered(name: &str, mut answer: Value) -> Value {
    let next = matches!(name, "feed" | "timeline")
        .then(|| answer.get_mut("next").map(Value::take))
        .flatten();
    let mut answer = if matches!(name, "claim" | "search" | "timeline" | "doc") {
        gated_with_ids(answer, true)
    } else {
        gated(answer)
    };
    if let Some(next) = next {
        answer["next"] = next;
    }
    answer
}

fn gated(v: Value) -> Value {
    gated_with_ids(v, false)
}

/// Claim identifiers come from the typed store responses. They are returned to the API as
/// selectors, rather than editable display text; a custom hash rule must not invalidate them.
/// Other fields, malformed identifiers and APIs without claim selectors keep the normal gate.
fn gated_with_ids(v: Value, ids: bool) -> Value {
    match v {
        Value::String(s) => Value::String(redact::outbound(&s)),
        Value::Array(xs) => Value::Array(xs.into_iter().map(|v| gated_with_ids(v, ids)).collect()),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, v)| {
                    let selector =
                        ids && matches!(
                            k.as_str(),
                            "uid" | "key" | "id" | "later" | "by" | "supersedes" | "ended_by"
                        ) && canonical_uid_value(&v);
                    if selector {
                        (k, v)
                    } else {
                        (redact::outbound(&k), gated_with_ids(v, ids))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

fn canonical_uid_value(v: &Value) -> bool {
    match v {
        Value::String(uid) => {
            uid.len() == 64
                && uid
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        Value::Array(ids) => ids
            .iter()
            .all(|id| matches!(id, Value::String(_)) && canonical_uid_value(id)),
        _ => false,
    }
}

fn bad(name: &str) -> Response {
    Response::text(400, &format!("bad {name}"))
}

/// `1`/`true` or `0`/`false` (or absent).
fn flag(q: &HashMap<String, String>, name: &str) -> std::result::Result<bool, Response> {
    match q.get(name).map(String::as_str) {
        None | Some("" | "0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(_) => Err(bad(name)),
    }
}

/// `limit`, at most `MAX_LIMIT`.
fn limit(q: &HashMap<String, String>, default: usize) -> std::result::Result<usize, Response> {
    match q.get("limit").map(String::as_str) {
        None | Some("") => Ok(default),
        Some(l) => l
            .parse::<usize>()
            .map(|n| n.clamp(1, MAX_LIMIT))
            .map_err(|_| bad("limit")),
    }
}

/// `before=<ts>:<key>`, the timeline's next page.
fn page(q: &HashMap<String, String>) -> std::result::Result<Option<(i64, String)>, Response> {
    match q.get("before").map(String::as_str) {
        None | Some("") => Ok(None),
        Some(b) => match b.split_once(':') {
            Some((ts, key)) if !key.is_empty() => ts
                .parse()
                .map(|ts| Some((ts, key.to_owned())))
                .map_err(|_| bad("before")),
            _ => Err(bad("before")),
        },
    }
}

/// page.md P3: each kind's position after the last one of its kind kept, as JSON the page sends
/// back as it got it. Only kept rows move a position: one taken from the merged page's last time
/// would skip a kind's rows at a tie.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedCursor {
    cards: Option<crate::cards::Position>,
    summaries: Option<crate::turns::Position>,
    prompts: Option<crate::raw::PromptPosition>,
}

fn feed_page(q: &HashMap<String, String>) -> std::result::Result<FeedCursor, Response> {
    let Some(cursor) = q.get("before").filter(|s| !s.is_empty()) else {
        return Ok(FeedCursor::default());
    };
    // Refused before any store is opened.
    if cursor.len() > 4096 {
        return Err(bad("before"));
    }
    serde_json::from_str(cursor).map_err(|_| bad("before"))
}

/// The feed's three bounded pages, merged by own time, kind, then stored ID (newest first).
/// Every payload is already gated by its table's one reader; `api` gates the answer too.
fn feed(home: &Path, repo: Option<&str>, mut cursor: FeedCursor, limit: usize) -> Result<Value> {
    let Some((raw, k)) = search::b::stores(home)? else {
        return Ok(json!({"items": [], "next": null}));
    };
    let rules = redact::Rules::load(home)?;
    let (cards, cards_more) =
        crate::cards::page(&k, &raw, repo, cursor.cards.as_ref(), limit, &rules)?;
    let (summaries, summaries_more) =
        crate::turns::page(&k, &raw, repo, cursor.summaries.as_ref(), limit, &rules)?;
    let (prompts, prompts_more) = raw.prompts(repo, cursor.prompts.as_ref(), limit, &rules)?;
    let name =
        |repo: Option<&str>| crate::consumer::manifest::repo_name(repo.unwrap_or(""), &rules);
    // A common sort key, not another reader: summaries and prompts have no card ordinal.
    let mut rows = Vec::new();
    for c in cards {
        rows.push((
            0,
            c.position(),
            json!({
                "kind": "card", "id": c.id(raw.device()), "ts": c.ts,
                "agent": c.agent, "repo_name": name(c.repo.as_deref()), "repo": c.repo,
                "type": c.kind, "title": c.title, "subtitle": c.subtitle, "narrative": c.narrative,
                "facts": c.facts, "concepts": c.concepts,
                "files_read": c.files_read, "files_modified": c.files_modified,
            }),
        ));
    }
    for s in summaries {
        let (ts, device, seq) = s.position();
        rows.push((
            1,
            (ts, device, seq, 0),
            json!({
                "kind": "summary", "id": s.id(raw.device()), "ts": s.ts,
                "agent": s.agent, "repo_name": name(s.repo.as_deref()), "repo": s.repo,
                "fields": s.fields,
            }),
        ));
    }
    for p in prompts {
        let (ts, device, seq) = p.position();
        let mut text: String = p.text.chars().take(2_000).collect();
        if p.text.chars().count() > 2_000 {
            text.push('…');
        }
        rows.push((2, (ts, device, seq, 0), json!({
            "kind": "prompt", "id": p.key(), "ts": p.ts,
            "agent": p.agent, "repo_name": name(p.repo.as_deref()), "repo": p.repo, "text": text,
        })));
    }
    rows.sort_by(|(ak, a, _), (bk, b, _)| {
        b.0.cmp(&a.0)
            .then_with(|| ak.cmp(bk))
            .then_with(|| b.cmp(a))
    });
    let more = rows.len() > limit || cards_more || summaries_more || prompts_more;
    rows.truncate(limit);
    let mut items = Vec::new();
    for (kind, (ts, device, seq, n), value) in rows {
        match kind {
            0 => cursor.cards = Some((ts, device, seq, n)),
            1 => cursor.summaries = Some((ts, device, seq)),
            _ => cursor.prompts = Some((ts, device, seq)),
        }
        items.push(value);
    }
    let next = if more {
        Some(serde_json::to_string(&cursor)?)
    } else {
        None
    };
    Ok(json!({"items": items, "next": next}))
}

/// The page's search as the search core takes it (MUST-M11 to M13); its `caller` is the
/// viewer's checkout, set by the route.
fn search_query(q: &HashMap<String, String>) -> std::result::Result<search::b::Query, Response> {
    let when = |name: &str, until: bool| match q.get(name).map(|s| s.trim()) {
        None | Some("") => Ok(None),
        Some(s) => search::b::time(s, until).map(Some).map_err(|_| bad(name)),
    };
    let raw = match q.get("raw").map(String::as_str) {
        None | Some("") => search::b::RawArm::default(),
        Some(r @ ("off" | "below" | "only")) => r.parse().map_err(|_| bad("raw"))?,
        Some(_) => return Err(bad("raw")),
    };
    Ok(search::b::Query {
        text: q.get("q").cloned().unwrap_or_default(),
        repo: q.get("repo").filter(|r| !r.is_empty()).cloned(),
        all: flag(q, "all")?,
        since: when("since", false)?,
        until: when("until", true)?,
        history: flag(q, "history")?,
        raw,
        limit: limit(q, 50)?,
        ..Default::default()
    })
}

/// Another repository's branch for its Context page: its newest manifest's, or none.
fn newest_branch(home: &Path, repo: &str) -> Result<String> {
    let Some((_raw, k)) = search::b::stores(home)? else {
        return Ok(String::new());
    };
    if !crate::consumer::manifest::exists(&k, "table", "manifests")? {
        return Ok(String::new());
    }
    Ok(k.query_row(
        "SELECT branch FROM manifests WHERE repo = ?1 ORDER BY built_at DESC LIMIT 1",
        [repo],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or_default())
}

/// The repositories the stores know, the most recent first: claims (by their derivations),
/// imported documents and records, each counted, with the newest time.
fn repos(home: &Path) -> Result<Vec<Value>> {
    let Some((_raw, k)) = search::b::stores(home)? else {
        return Ok(Vec::new());
    };
    repos_in(&k)
}

/// Existing repository enumeration on a caller-owned connection, without opening stores.
pub(crate) fn repos_in(k: &rusqlite::Connection) -> Result<Vec<Value>> {
    let mut st = k.prepare(
        "SELECT repo, SUM(claims), SUM(imported), SUM(records), MAX(last) FROM (
           SELECT repo, COUNT(DISTINCT uid) AS claims, 0 AS imported, 0 AS records,
                  MAX(valid_from) AS last
           FROM derivations WHERE repo IS NOT NULL GROUP BY repo
           UNION ALL
           SELECT repo, 0, COUNT(DISTINCT uid), 0, MAX(ts) FROM imported GROUP BY repo
           UNION ALL
           SELECT repo, 0, 0, COUNT(*), MAX(ts) FROM raw_docs WHERE repo IS NOT NULL GROUP BY repo)
         GROUP BY repo ORDER BY MAX(last) DESC, repo",
    )?;
    let rows = st.query_map([], |r| {
        Ok(json!({
            "repo": r.get::<_, String>(0)?,
            "claims": r.get::<_, i64>(1)?,
            "imported": r.get::<_, i64>(2)?,
            "records": r.get::<_, i64>(3)?,
            "last": r.get::<_, i64>(4)?,
        }))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Changes when what the page shows changes: an op appended (a claim, a correction, an exclusion,
/// an import), an op the worker applied, a session started, a prompt typed, or a record hidden (a
/// tombstone, which reads hide before the worker applies it). Live prompts move it at capture
/// too, since the feed reads them directly (P5). Other records (tool calls, replies) do not
/// move it, so a working agent does not redraw the page between prompts.
fn version(home: &Path) -> Result<String> {
    let Some((raw, k)) = search::b::stores(home)? else {
        return Ok("0".into());
    };
    let applied: (i64, i64) = k.query_row(
        "SELECT (SELECT COALESCE(SUM(seq), 0) FROM op_checkpoints),
                (SELECT COALESCE(MAX(rowid), 0) FROM raw_docs WHERE kind IN ('start', 'prompt'))",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(format!(
        "{}:{}:{}:{}:{}",
        raw.max_op_seq()?,
        applied.0,
        applied.1,
        raw.tombstones()?,
        raw.prompt_version()?
    ))
}

/// Records per device, claims by kind and status, skipped claim ops by reason, the last seven
/// days' provider calls, the stores' bytes, and whether a rebuild is running (D11).
fn stats(home: &Path) -> Result<Value> {
    // What a store takes on disk: the file plus the WAL not yet checkpointed into it.
    let bytes: u64 = [
        "raw.db",
        "raw.db-wal",
        "knowledge.db",
        "knowledge.db-wal",
        "providers.db",
        "providers.db-wal",
    ]
    .into_iter()
    .map(|name| std::fs::metadata(home.join(name)).map_or(0, |m| m.len()))
    .sum();
    // R13: a resident worker and a file left by a failed rebuild are not a live rebuild.
    let rebuilding = crate::worker::lock_held(&home.join("state/rebuild.lock"));
    let (records, claims, skips) = match search::b::stores(home)? {
        None => (Vec::new(), Vec::new(), Vec::new()),
        Some((raw, k)) => {
            // A device's records by its highest seq (one sequence holds its events and tombstones),
            // read from the primary key without a scan.
            let records = raw
                .devices()?
                .into_iter()
                .map(|d| {
                    let n = raw.max_seq_of(&d)?;
                    Ok(json!({ "device": d, "records": n }))
                })
                .collect::<Result<_>>()?;
            let rows = |sql: &str, f: fn(&rusqlite::Row) -> rusqlite::Result<Value>| {
                let mut st = k.prepare(sql)?;
                let rows = st.query_map([], f)?;
                Ok::<Vec<Value>, anyhow::Error>(rows.collect::<Result<_, _>>()?)
            };
            let claims = rows(
                "SELECT kind, status, COUNT(*) FROM active GROUP BY kind, status
                 ORDER BY COUNT(*) DESC, kind, status",
                |r| {
                    Ok(json!({
                        "kind": r.get::<_, String>(0)?,
                        "status": r.get::<_, String>(1)?,
                        "count": r.get::<_, i64>(2)?,
                    }))
                },
            )?;
            let skips = rows(
                "SELECT reason, COUNT(*) FROM claim_skips GROUP BY reason ORDER BY COUNT(*) DESC",
                |r| Ok(json!({ "reason": r.get::<_, String>(0)?, "count": r.get::<_, i64>(1)? })),
            )?;
            (records, claims, skips)
        }
    };
    Ok(json!({
        "records": records,
        "claims": claims,
        "claim_skips": skips,
        "providers": providers(home)?,
        "bytes": bytes,
        "rebuilding": rebuilding,
    }))
}

/// The last seven days' calls per provider and role, the busiest first, from providers.db (not
/// made when absent).
fn providers(home: &Path) -> Result<Vec<Value>> {
    if !home.join("providers.db").exists() {
        return Ok(Vec::new());
    }
    let p = crate::providers_db::open(home)?;
    let mut st = p.prepare(
        "SELECT provider, role, SUM(outcome = 'ok'),
                SUM(outcome IN ('error','invalid','empty','prose','shape','over_cap','unanchored')),
                SUM(outcome = 'wait'), SUM(CASE WHEN outcome = 'ok' THEN ms ELSE 0 END)
         FROM provider_calls WHERE ts >= ?1 GROUP BY provider, role
         ORDER BY SUM(outcome = 'ok') DESC, provider, role",
    )?;
    let rows = st.query_map([crate::db::now_ms() - 7 * 86_400_000], |r| {
        let ok: i64 = r.get(2)?;
        let ok_ms: i64 = r.get(5)?;
        Ok(json!({
            "provider": r.get::<_, String>(0)?,
            "role": r.get::<_, String>(1)?,
            "ok": ok,
            "failed": r.get::<_, i64>(3)?,
            "waited": r.get::<_, i64>(4)?,
            "avg_ms": (ok > 0).then(|| ok_ms / ok),
        }))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `a=1&b=x+y` with percent-decoding (`+` is a space, as `URLSearchParams` writes it).
fn params(query: &str) -> HashMap<String, String> {
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(&s.replace('+', " "))
            .decode_utf8_lossy()
            .into_owned()
    };
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::b::fixture::Store;

    #[cfg(target_os = "linux")]
    fn w5b_post(v: &Viewer, target: &str, value: Value) -> Response {
        let body = serde_json::to_vec(&value).unwrap();
        let host = format!("127.0.0.1:{}", v.port());
        let origin = format!("http://{host}");
        let token = file_token(&v.home).unwrap();
        let length = body.len().to_string();
        request(
            v,
            "POST",
            target,
            &[
                ("Host", &host),
                ("Origin", &origin),
                ("X-Oboete-Token", &token),
                ("Content-Type", "application/json"),
                ("Content-Length", &length),
            ],
            &body,
        )
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w5b_an_initial_resident_viewer_keeps_its_original_home_for_maintenance() {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            crate::executable::expected(Some(crate::executable::Role::Viewer))
                .unwrap()
                .is_none()
        );
        for kind in ["rebuild", "restore"] {
            let (port, home, started) = listening();
            let p = home.path();
            let retired = tempfile::tempdir().unwrap();
            std::fs::rename(p, retired.path().join("previous")).unwrap();
            std::fs::create_dir(p).unwrap();
            std::fs::write(
                p.join("config.toml"),
                format!("providers = []\n[summary]\ncurate = false\n[view]\nport = {port}\n"),
            )
            .unwrap();
            crate::raw::open(p)
                .unwrap()
                .append(&crate::raw::test_event("synthetic replacement home"))
                .unwrap();
            crate::worker::run_once(p).unwrap();
            std::fs::set_permissions(p.join("state"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            std::fs::write(p.join("state/view.lock"), "replacement viewer authority").unwrap();
            ensure_token(p).unwrap();
            let preview = w5b_post(
                &started.viewer,
                "/api/maintenance/preview",
                json!({"operation":{"kind":kind}}),
            );
            assert_eq!(preview.status, 200);
            let preview: Value = serde_json::from_slice(&preview.body).unwrap();
            let before = crate::backup::tests::w5b_files(p);
            let slot = Slot::take(&started.viewer).unwrap();
            let answer = w5b_post(
                &started.viewer,
                "/api/maintenance/start",
                json!({
                    "operation":{"kind":kind}, "preview_key":preview["preview_key"],
                    "operation_id":"a".repeat(64), "confirmed":true,
                }),
            );
            drop(slot);
            assert_eq!(answer.status, 200);
            let answer: Value = serde_json::from_slice(&answer.body).unwrap();
            assert_eq!(
                answer["last"]["phase"], "failed",
                "old viewer adopted the replacement home"
            );
            assert_eq!(answer["last"]["committed"], false);
            assert_eq!(crate::backup::tests::w5b_files(p), before);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w5c_preparation_refuses_unpinned_and_replaced_resident_homes() {
        use std::os::unix::fs::PermissionsExt;
        let (port, home, started) = listening();
        let p = home.path();
        crate::raw::open(p)
            .unwrap()
            .append(&crate::raw::test_event("synthetic original home"))
            .unwrap();
        crate::worker::run_once(p).unwrap();
        let operation = json!({"kind":"recurate","scope":{"kind":"queued"}});
        let detached = Viewer::new(p, None, port, Token::File);
        let before = crate::backup::tests::w5b_files(p);
        let readonly = w5b_post(
            &detached,
            "/api/maintenance/preview",
            json!({"operation":{"kind":"rebuild"}}),
        );
        assert_eq!(readonly.status, 200);
        assert_eq!(crate::backup::tests::w5b_files(p), before);
        let unpinned = w5b_post(
            &detached,
            "/api/maintenance/preview",
            json!({"operation":operation}),
        );
        assert_eq!(unpinned.status, 422);
        assert_eq!(crate::backup::tests::w5b_files(p), before);

        let retired = tempfile::tempdir().unwrap();
        std::fs::rename(p, retired.path().join("previous")).unwrap();
        std::fs::create_dir(p).unwrap();
        std::fs::write(
            p.join("config.toml"),
            format!("providers = []\n[summary]\ncurate = false\n[view]\nport = {port}\n"),
        )
        .unwrap();
        crate::raw::open(p)
            .unwrap()
            .append(&crate::raw::test_event("synthetic replacement home"))
            .unwrap();
        crate::worker::run_once(p).unwrap();
        std::fs::set_permissions(p.join("state"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(p.join("state/view.lock"), "replacement viewer authority").unwrap();
        ensure_token(p).unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let slot = Slot::take(&started.viewer).unwrap();
        let response = w5b_post(
            &started.viewer,
            "/api/maintenance/preview",
            json!({"operation":operation}),
        );
        drop(slot);
        assert_eq!(response.status, 200);
        let response: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(response["preview_key"], Value::Null);
        assert_eq!(response["preparation"]["last"]["phase"], "failed");
        assert_eq!(response["preparation"]["last"]["committed"], false);
        assert_eq!(crate::backup::tests::w5b_files(p), before);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w5c_confirmed_maintenance_keeps_the_original_resident_home_after_a_hardlinked_replacement() {
        use std::os::unix::fs::PermissionsExt;

        for kind in ["recurate", "finish"] {
            let (_, home, started) = listening();
            let p = home.path();
            crate::raw::open(p)
                .unwrap()
                .append(&crate::raw::test_event("synthetic original queued home"))
                .unwrap();
            crate::worker::run_once(p).unwrap();
            if kind == "finish" {
                drop(crate::db::open(p).unwrap());
            }
            let files = if kind == "finish" {
                vec!["config.toml", "raw.db", "knowledge.db", "oboete.db"]
            } else {
                vec!["config.toml", "raw.db", "knowledge.db"]
            };
            let operation = if kind == "finish" {
                json!({"kind":"finish"})
            } else {
                json!({"kind":"recurate","scope":{"kind":"queued"}})
            };
            let preview = w5b_post(
                &started.viewer,
                "/api/maintenance/preview",
                json!({"operation":operation}),
            );
            assert_eq!(preview.status, 200);
            let preview: Value = serde_json::from_slice(&preview.body).unwrap();
            assert_eq!(preview["kind"], kind);
            assert_eq!(preview["no_model_request"], true);
            if kind == "recurate" {
                assert_eq!(preview["preparation"]["last"]["phase"], "prepared");
            }
            assert!(
                preview["preview_key"]
                    .as_str()
                    .is_some_and(|key| key.len() == 64)
            );

            // Preserve each operation's consent bytes and file identities so that
            // refusal must come from the original Viewer proof, not physical staleness.
            for name in &files {
                assert!(p.join(name).is_file(), "missing {name} before replacement");
            }
            for name in [
                "raw.db-wal",
                "knowledge.db-wal",
                "oboete.db-wal",
                "oboete.db-shm",
                "forget.log",
            ] {
                assert!(!p.join(name).exists(), "{name} would change consent");
            }
            assert!(!crate::backup::dir(p).unwrap().join("forget.log").exists());
            assert!(!p.join("providers.db").exists());
            let original_lock =
                crate::worker::file_id(std::fs::metadata(p.join("state/view.lock")));
            assert!(original_lock.is_some());
            let original_token = file_token(p).unwrap();

            let retired = tempfile::tempdir().unwrap();
            let previous = retired.path().join("previous");
            std::fs::rename(p, &previous).unwrap();
            std::fs::create_dir(p).unwrap();
            for name in &files {
                let old = previous.join(name);
                let replacement = p.join(name);
                std::fs::hard_link(&old, &replacement).unwrap();
                assert_eq!(
                    crate::db::store_file(&old),
                    crate::db::store_file(&replacement)
                );
                assert!(!crate::db::store_file(&old).is_empty());
                assert_eq!(
                    crate::migrate::file_version(&old).unwrap(),
                    crate::migrate::file_version(&replacement).unwrap()
                );
            }
            std::fs::create_dir(p.join("state")).unwrap();
            std::fs::set_permissions(p.join("state"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            std::fs::write(p.join("state/view.lock"), "replacement authority").unwrap();
            ensure_token(p).unwrap();
            assert_ne!(file_token(p).unwrap(), original_token);
            assert_ne!(
                crate::worker::file_id(std::fs::metadata(p.join("state/view.lock"))),
                original_lock
            );
            for name in [
                "raw.db-wal",
                "knowledge.db-wal",
                "oboete.db-wal",
                "oboete.db-shm",
                "forget.log",
            ] {
                assert!(!p.join(name).exists(), "replacement introduced {name}");
            }
            assert!(!crate::backup::dir(p).unwrap().join("forget.log").exists());
            let before = crate::backup::tests::w5b_files(p);

            // The replacement token authenticates the guarded handler, while the
            // Viewer still carries the old view.lock identity from its original home.
            let slot = Slot::take(&started.viewer).unwrap();
            let response = w5b_post(
                &started.viewer,
                "/api/maintenance/start",
                json!({
                    "operation":operation,
                    "preview_key":preview["preview_key"],
                    "operation_id":"c".repeat(64),
                    "confirmed":true,
                }),
            );
            drop(slot);
            assert_eq!(response.status, 200);
            let response: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(response["last"]["phase"], "failed");
            assert_eq!(response["last"]["committed"], false);
            assert_eq!(crate::backup::tests::w5b_files(p), before);
            assert!(!p.join("providers.db").exists());
            assert!(!previous.join("providers.db").exists());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w5b_a_fifo_view_lock_is_refused_without_blocking_or_replacing_it() {
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
        let (_, home, started) = listening();
        let p = home.path();
        crate::raw::open(p)
            .unwrap()
            .append(&crate::raw::test_event("synthetic FIFO proof"))
            .unwrap();
        crate::worker::run_once(p).unwrap();
        let preview = w5b_post(
            &started.viewer,
            "/api/maintenance/preview",
            json!({"operation":{"kind":"rebuild"}}),
        );
        assert_eq!(preview.status, 200);
        let preview: Value = serde_json::from_slice(&preview.body).unwrap();
        let lock = p.join("state/view.lock");
        std::fs::remove_file(&lock).unwrap();
        let path = std::ffi::CString::new(lock.to_str().unwrap()).unwrap();
        // SAFETY: the owned fixture path is NUL-terminated and remains valid.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let identity = crate::worker::file_id(std::fs::symlink_metadata(&lock));
        let before = std::fs::read(p.join("raw.db")).unwrap();
        let viewer = Arc::clone(&started.viewer);
        let (send, receive) = std::sync::mpsc::channel();
        let call = std::thread::spawn(move || {
            let _slot = Slot::take(&viewer).unwrap();
            send.send(w5b_post(
                &viewer,
                "/api/maintenance/start",
                json!({
                    "operation":{"kind":"rebuild"}, "preview_key":preview["preview_key"],
                    "operation_id":"b".repeat(64), "confirmed":true,
                }),
            ))
            .unwrap();
        });
        let first = receive.recv_timeout(Duration::from_secs(5));
        let prompt = first.is_ok();
        // Let a faulty blocking reader finish before failing, so the test leaves no thread.
        let (answer, _rescue) = match first {
            Ok(answer) => (answer, None),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let rescue = std::fs::File::options()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&lock)
                    .unwrap();
                (
                    receive.recv_timeout(Duration::from_secs(5)).unwrap(),
                    Some(rescue),
                )
            }
            Err(error) => panic!("viewer request failed: {error}"),
        };
        call.join().unwrap();
        assert!(prompt, "a FIFO blocked maintenance admission");
        assert_eq!(answer.status, 200);
        let answer: Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(answer["last"]["phase"], "failed");
        assert_eq!(answer["last"]["committed"], false);
        assert!(
            std::fs::symlink_metadata(&lock)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        assert_eq!(
            crate::worker::file_id(std::fs::symlink_metadata(&lock)),
            identity
        );
        assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), before);
        assert_eq!(started.viewer.live.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn w6_agent_registrations_ui_preserves_drafts_and_reads_without_posts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let result = std::process::Command::new("node")
            .arg(root.join("src/testdata/viewer-readiness/test.mjs"))
            .arg(root.join("assets/viewer/app.js"))
            .status();
        let status = match result {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("readiness UI harness skipped: node is not installed");
                return;
            }
            other => other.unwrap(),
        };
        assert!(status.success(), "readonly registrations UI check failed");
    }

    #[test]
    fn w6d_doctor_uses_the_exact_legacy_checkpoint_across_raw_devices() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        raw.append_ops(&[
            (
                crate::raw::OpKind::Migration,
                json!({"key":"oboete-v1:private-v1","through":1}),
            ),
            (
                crate::raw::OpKind::Migration,
                json!({"key":"oboete-v1:private-v1","through":3}),
            ),
            (
                crate::raw::OpKind::Migration,
                json!({"key":"oboete-v1:private-v1:suffix","through":99}),
            ),
        ])
        .unwrap();
        let writer = rusqlite::Connection::open(p.join("raw.db")).unwrap();
        writer
            .execute(
                "UPDATE ops SET device='private-remote-device' WHERE op_seq=2",
                [],
            )
            .unwrap();
        let v1 = rusqlite::Connection::open(p.join("oboete.db")).unwrap();
        v1.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE meta(key TEXT,value TEXT);
             INSERT INTO meta VALUES('device_id','private-v1');
             CREATE TABLE sessions(id INTEGER);
             CREATE TABLE events(id INTEGER);
             INSERT INTO events VALUES(1),(3),(5),(9);
             CREATE TABLE observations(id INTEGER);
             CREATE TABLE summaries(id INTEGER);
             CREATE TABLE provider_calls(id INTEGER PRIMARY KEY,provider TEXT,outcome TEXT,ms INTEGER,detail TEXT);"
        ).unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        let answer = request(&v, "POST", "/api/doctor", &headers, b"{}");
        let report = json_of(&answer);
        assert_eq!(
            report["checks"]["legacy"]["remaining_events"]["value"], 2,
            "Doctor did not use the native exact checkpoint on all Raw devices"
        );
        assert_eq!(
            report["checks"]["legacy"]["remaining_events"]["state"],
            "known"
        );
        assert_eq!(report["checks"]["legacy"]["events"]["value"], 4);
        let text = String::from_utf8(answer.body).unwrap();
        assert!(!text.contains("private-v1") && !text.contains("private-remote-device"));
        assert_eq!(crate::backup::tests::w5b_files(p), before);
    }

    #[test]
    fn w6d_doctor_reads_legacy_counts_and_keeps_call_details_private() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let v1 = rusqlite::Connection::open(p.join("oboete.db")).unwrap();
        v1.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE sessions(id INTEGER);
             CREATE TABLE events(id INTEGER);
             CREATE TABLE observations(id INTEGER);
             CREATE TABLE summaries(id INTEGER);
             CREATE TABLE provider_calls(id INTEGER PRIMARY KEY,provider TEXT,outcome TEXT,ms INTEGER,detail TEXT);
             INSERT INTO sessions VALUES(1);
             INSERT INTO events VALUES(1),(3),(5);
             INSERT INTO observations VALUES(1),(2);
             INSERT INTO summaries VALUES(1);
             INSERT INTO provider_calls VALUES(1,'private-v1-provider-canary','ok',25,'private-v1-detail-canary');"
        ).unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        let answer = request(&v, "POST", "/api/doctor", &headers, b"{}");
        let report = json_of(&answer);
        let legacy = &report["checks"]["legacy"];
        assert_eq!(
            legacy["events"]["value"], 3,
            "readonly Doctor has no native legacy counts"
        );
        for (name, count) in [
            ("sessions", 1),
            ("events", 3),
            ("observations", 2),
            ("summaries", 1),
        ] {
            assert_eq!(legacy[name], json!({"state":"known","value":count}));
        }
        assert_eq!(legacy["recent"]["calls"], json!([{"outcome":"ok","ms":25}]));
        let text = String::from_utf8(answer.body).unwrap();
        assert!(
            !text.contains("private-v1-provider-canary")
                && !text.contains("private-v1-detail-canary")
        );
        assert_eq!(crate::backup::tests::w5b_files(p), before);
    }

    #[test]
    fn w6d_doctor_keeps_knowledge_facts_when_raw_is_damaged_and_gaps_are_old() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("raw.db"), b"synthetic damaged raw store").unwrap();
        let k = rusqlite::Connection::open(p.join("knowledge.db")).unwrap();
        k.execute_batch("CREATE TABLE rewinds(ts INTEGER); INSERT INTO rewinds VALUES(2000);")
            .unwrap();
        let before = crate::backup::tests::w5b_files(p);
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        let report = json_of(&request(&v, "POST", "/api/doctor", &headers, b"{}"));
        assert_eq!(report["checks"]["raw"]["integrity"], "damaged");
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], Value::Null);
        assert_eq!(report["checks"]["knowledge"]["integrity"], "known");
        assert_eq!(
            report["checks"]["knowledge"]["rewinds"]["count"]["value"],
            1
        );
        assert_eq!(
            report["checks"]["knowledge"]["gaps"]["state"],
            "schema_missing"
        );
        assert_eq!(report["checks"]["knowledge"]["gaps"]["rows"], Value::Null);
        assert_eq!(crate::backup::tests::w5b_files(p), before);
    }

    #[test]
    fn w6d_doctor_reads_knowledge_without_exposing_stored_agent_names() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let raw = crate::raw::open(p).unwrap();
        let k = crate::knowledge::open(p).unwrap();
        crate::worker::Consumer::step(
            &mut crate::consumer::gaps::Gaps::new(p),
            &raw,
            &k,
            raw.device(),
            0,
        )
        .unwrap();
        k.execute_batch(
            "INSERT INTO rewinds VALUES(1000,'claims','owned-private-device',3,1);
             INSERT INTO rewinds VALUES(2000,'turns','owned-private-device',4,2);
             INSERT INTO gaps VALUES('owned-private-device','claude','one',1,5,3,100);
             INSERT INTO gaps VALUES('owned-private-device','claude','two',2,NULL,1,100);
             INSERT INTO gaps VALUES('owned-private-device','untrusted-agent-canary','three',3,4,1,100);"
        ).unwrap();
        assert!(p.join("knowledge.db-wal").is_file());
        let before = crate::backup::tests::w5b_files(p);
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        let answer = request(&v, "POST", "/api/doctor", &headers, b"{}");
        let report = json_of(&answer);
        let knowledge = &report["checks"]["knowledge"];
        assert_eq!(
            knowledge["rewinds"]["count"]["value"], 2,
            "readonly Doctor has no native Knowledge findings"
        );
        assert_eq!(knowledge["integrity"], "known");
        assert_eq!(knowledge["rewinds"]["last_at_ms"], 2000);
        let rows = knowledge["gaps"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 8);
        assert_eq!(
            rows[0],
            json!({"agent":"claude","ended":2,"checked":1,"short":1,"missing":2})
        );
        assert_eq!(
            rows[7],
            json!({"agent":"other","ended":1,"checked":1,"short":1,"missing":3})
        );
        let text = String::from_utf8(answer.body).unwrap();
        assert!(!text.contains("untrusted-agent-canary") && !text.contains("owned-private-device"));
        assert_eq!(crate::backup::tests::w5b_files(p), before);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn w6d_doctor_refuses_a_known_fifo_without_opening_it() {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        };
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("raw.db");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: only an owned, NUL-terminated synthetic path is created.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // SAFETY: flags are the supported nonblocking/close-on-exec inotify flags.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0);
        // SAFETY: the successful init returned this newly owned descriptor exactly once.
        let mut watcher = unsafe { std::fs::File::from_raw_fd(fd) };
        // SAFETY: the live descriptor and owned path outlive this watch registration.
        assert!(
            unsafe { libc::inotify_add_watch(watcher.as_raw_fd(), name.as_ptr(), libc::IN_OPEN) }
                >= 0
        );
        let v = Viewer::new(home.path(), None, 4321, Token::Run("t0k".into()));
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        let report = json_of(&request(&v, "POST", "/api/doctor", &headers, b"{}"));
        assert_eq!(report["checks"]["raw"]["integrity"], "unavailable");
        let mut events = [0u8; 64];
        let bytes = match watcher.read(&mut events) {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
            other => other.unwrap(),
        };
        assert_eq!(bytes, 0, "Doctor opened the known FIFO before refusing it");
    }

    #[test]
    fn w6d_doctor_reads_live_raw_wal_without_rebinding_its_device() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = crate::raw::open(p).unwrap();
        let seq = raw
            .append(&crate::raw::test_event("private Doctor WAL fixture"))
            .unwrap();
        let device = raw.device().to_owned();
        let op_seq = raw.max_op_seq_of(&device).unwrap();
        assert!(p.join("raw.db-wal").is_file());
        let before = crate::backup::tests::w5b_files(p);
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        let answer = request(&v, "POST", "/api/doctor", &headers, b"{}");
        let report = json_of(&answer);
        assert_eq!(
            report["checks"]["raw"]["max_seq"]["value"], seq,
            "readonly Doctor did not read the original stored device"
        );
        assert_eq!(report["checks"]["raw"]["max_seq"]["state"], "known");
        assert_eq!(report["checks"]["raw"]["max_op_seq"]["value"], op_seq);
        assert_eq!(report["checks"]["raw"]["integrity"], "known");
        assert!(!String::from_utf8(answer.body).unwrap().contains(&device));
        assert_eq!(crate::backup::tests::w5b_files(p), before);
        assert_eq!(raw.device(), device);
    }

    #[test]
    fn w6d_busy_doctor_is_refused_before_opening_or_copying_a_store() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("absent");
        let v = Viewer::new(&home, None, 4321, Token::Run("t0k".into()));
        let busy = v.saving.lock().unwrap();
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        assert_eq!(
            request(&v, "POST", "/api/doctor", &headers, b"{}").status,
            503
        );
        assert_eq!(
            request(&v, "POST", "/api/doctor", &headers, b"[]").status,
            400,
            "body validation precedes the operation gate"
        );
        assert!(!home.exists());
        drop(busy);
        assert_eq!(
            request(&v, "POST", "/api/doctor", &headers, b"{}").status,
            200
        );
        assert!(!home.exists());
    }

    #[test]
    fn w6d_doctor_requires_an_explicit_empty_object_and_keeps_absence() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("absent");
        let v = Viewer::new(&home, None, 4321, Token::Run("t0k".into()));
        let path = "/api/doctor";
        let headers = [
            HOST,
            TOKEN,
            ("Origin", "http://127.0.0.1:4321"),
            ("Content-Type", "application/json"),
            ("Content-Length", "2"),
        ];
        let answer = request(&v, "POST", path, &headers, b"{}");
        assert_eq!(
            answer.status, 200,
            "explicit readonly Doctor is unavailable"
        );
        let report = json_of(&answer);
        assert_eq!(report["complete"], true);
        assert_eq!(report["checks"]["remaining"], "none");
        assert_eq!(report["unhealthy"], json!([]));
        assert_eq!(report["checks"]["raw"]["integrity"], "absent");
        assert_eq!(report["checks"]["raw"]["max_seq"]["value"], Value::Null);
        assert_eq!(report["inventory"]["home"], "missing");
        assert_eq!(report["inventory"]["config"], "missing");
        assert_eq!(report["inventory"]["agents"].as_array().unwrap().len(), 7);
        for name in ["raw", "raw_restored", "knowledge", "legacy", "providers"] {
            assert_eq!(
                report["stores"][name],
                json!({"file":"absent","wal":"absent"})
            );
        }
        for body in [
            b"[]".as_slice(),
            b"null",
            b"0",
            br#""{}""#,
            br#"{"repair":true}"#,
            b"",
        ] {
            let length = body.len().to_string();
            let mut headers = headers;
            headers[4] = ("Content-Length", length.as_str());
            assert_eq!(request(&v, "POST", path, &headers, body).status, 400);
        }
        save_guards(&v, path, MAX_BODY, b"{}");
        assert_ne!(v.route("GET", path, &[HOST, TOKEN]).status, 200);
        assert_eq!(v.route("POST", "/api/doctor?repair", &headers).status, 405);
        assert!(!home.exists(), "readonly Doctor created the absent home");
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[cfg(target_os = "linux")]
    fn w6f_post(v: &Viewer, route: &str, value: Value) -> Response {
        let body = serde_json::to_vec(&value).unwrap();
        request(
            v,
            "POST",
            route,
            &[
                HOST,
                TOKEN,
                ("Origin", "http://127.0.0.1:4321"),
                ("Content-Type", "application/json"),
                ("Content-Length", &body.len().to_string()),
            ],
            &body,
        )
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6f_confirmation_is_opaque_and_expires_on_a_new_preview_or_viewer() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(p.join("config.toml"), "invalid = [").unwrap();
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let first = json_of(&v.recovery_preview(b"{}"));
        let second = json_of(&v.recovery_preview(b"{}"));
        assert!(
            first["preview_key"] != second["preview_key"],
            "each preview needs an unrelated confirmation key"
        );
        let before = crate::backup::tests::w5b_files(p);
        let body = |key: &Value| json!({"preview_key":key,"confirmed":true});
        assert_eq!(
            w6f_post(
                &v,
                "/api/settings/recovery/start",
                body(&first["preview_key"])
            )
            .status,
            409
        );
        let restarted = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        assert_eq!(
            w6f_post(
                &restarted,
                "/api/settings/recovery/start",
                body(&second["preview_key"])
            )
            .status,
            409
        );
        assert_eq!(crate::backup::tests::w5b_files(p), before);
        assert_eq!(
            w6f_post(
                &v,
                "/api/settings/recovery/start",
                body(&second["preview_key"])
            )
            .status,
            200
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6f_recovery_previews_without_writes_and_keeps_the_exact_invalid_file() {
        use std::os::unix::fs::MetadataExt;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let path = p.join("config.toml");
        let original = b"private-recovery-canary = [\xff\n";
        std::fs::write(&path, original).unwrap();
        // Existing settings saves can leave a 0755 state directory inside the private home.
        std::fs::create_dir(p.join("state")).unwrap();
        std::fs::set_permissions(
            p.join("state"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let before = crate::backup::tests::w5b_files(p);
        let preview_path = "/api/settings/recovery/preview";
        let start_path = "/api/settings/recovery/start";
        for body in [b"[]".as_slice(), b"null", br#"{"path":"other.toml"}"#] {
            assert_eq!(v.recovery_preview(body).status, 400);
        }
        for route in [preview_path, start_path] {
            save_guards(&v, route, MAX_BODY, b"{}");
        }
        let shown = w6f_post(&v, preview_path, json!({}));
        assert_eq!(
            shown.status,
            200,
            "{}",
            String::from_utf8_lossy(&shown.body)
        );
        let shown: Value = serde_json::from_slice(&shown.body).unwrap();
        assert_eq!(crate::backup::tests::w5b_files(p), before);
        assert!(!shown.to_string().contains("private-recovery-canary"));
        let body = json!({"preview_key":shown["preview_key"],"confirmed":true});
        let mut unconfirmed = body.clone();
        unconfirmed["confirmed"] = json!(false);
        assert_eq!(w6f_post(&v, start_path, unconfirmed).status, 422);
        let mut arbitrary = body.clone();
        arbitrary["path"] = json!("other.toml");
        assert_eq!(w6f_post(&v, start_path, arbitrary).status, 400);
        let mut stale = body.clone();
        stale["preview_key"] = json!("0".repeat(64));
        assert_eq!(w6f_post(&v, start_path, stale).status, 409);
        assert_eq!(crate::backup::tests::w5b_files(p), before);
        let recovered = w6f_post(&v, start_path, body.clone());
        assert_eq!(recovered.status, 200);
        let recovered: Value = serde_json::from_slice(&recovered.body).unwrap();
        let copy = p.join(recovered["backup"].as_str().unwrap());
        assert_eq!(std::fs::read(&copy).unwrap(), original);
        assert_eq!(std::fs::metadata(&copy).unwrap().mode() & 0o777, 0o600);
        let settings = get(&v, "/api/settings");
        assert_eq!(settings["summary"]["curate"], false);
        assert_eq!(settings["worker"]["resident"], false);
        assert_eq!(settings["paid_usd_per_month"].as_f64(), Some(0.0));
        assert!(settings["providers"].as_array().unwrap().is_empty());
        assert_eq!(crate::config::load(p).unwrap().embedding.provider, "none");
        assert!(!p.join("raw.db").exists() && !p.join("providers.db").exists());
        assert_eq!(w6f_post(&v, start_path, body).status, 409);
        assert_eq!(std::fs::read(copy).unwrap(), original);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6f_a_failed_stage_keeps_the_original_and_its_verified_private_copy() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = b"stage failure private canary = [";
        std::fs::write(p.join("config.toml"), original).unwrap();
        // An unrelated directory at the existing stager's fixed scratch name refuses stage.
        let collision = p.join(format!(".config.toml.{}.oboete-tmp", std::process::id()));
        std::fs::create_dir(&collision).unwrap();
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let preview = v.recovery_preview(b"{}");
        assert_eq!(preview.status, 200);
        let body = serde_json::to_vec(
            &json!({"preview_key":json_of(&preview)["preview_key"],"confirmed":true}),
        )
        .unwrap();
        let result = v.recovery_start(&body);
        assert_eq!(result.status, 500);
        assert_eq!(
            v.recovery_start(&body).status,
            409,
            "a consumed confirmation must not repeat partial work"
        );
        assert_eq!(std::fs::read(p.join("config.toml")).unwrap(), original);
        let copies: Vec<_> = std::fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".recovery-")
            })
            .collect();
        assert_eq!(copies.len(), 1);
        assert_eq!(std::fs::read(&copies[0]).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&copies[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(collision.is_dir());
        assert!(!p.join("raw.db").exists());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn w6f_recovery_waits_for_native_private_storage_proof() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(p.join("config.toml"), "invalid = [").unwrap();
        let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let before = crate::backup::tests::w5b_files(p);
        let result = v.recovery_preview(b"{}");
        assert_eq!(result.status, 422);
        let answer: Value = serde_json::from_slice(&result.body).unwrap();
        assert_eq!(answer["code"], "recovery_unavailable");
        assert_eq!(crate::backup::tests::w5b_files(p), before);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6f_recovery_leaves_valid_missing_large_and_unsafe_homes_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        for kind in [
            "valid",
            "missing",
            "missing_home",
            "large",
            "directory",
            "shared_home",
        ] {
            let root = tempfile::tempdir().unwrap();
            let p = root.path();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
            let home = if kind == "missing_home" {
                p.join("missing")
            } else {
                p.to_path_buf()
            };
            let file = home.join("config.toml");
            match kind {
                "valid" => std::fs::write(&file, "[summary]\ncurate = false\n").unwrap(),
                "large" => std::fs::write(&file, vec![b'x'; 1024 * 1024 + 1]).unwrap(),
                "directory" => std::fs::create_dir(&file).unwrap(),
                "shared_home" => {
                    std::fs::write(&file, "invalid = [").unwrap();
                    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o777)).unwrap();
                }
                _ => {}
            }
            let before = crate::backup::tests::w5b_files(p);
            let v = Viewer::new(&home, None, 4321, Token::Run("t0k".into()));
            let result = v.recovery_preview(b"{}");
            assert_eq!(result.status, 422, "{kind}");
            let answer: Value = serde_json::from_slice(&result.body).unwrap();
            assert_eq!(answer["code"], "recovery_unavailable");
            assert_eq!(crate::backup::tests::w5b_files(p), before, "{kind}");
            assert!(!home.join("state").exists(), "{kind}");
            if kind == "missing_home" {
                assert!(!home.exists());
            }
            if kind == "directory" {
                assert!(file.is_dir());
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6f_private_copy_refusal_precedes_bytes_and_postcommit_failure_is_unknown() {
        use std::os::unix::fs::PermissionsExt;
        fn untrusted_copy(_home: &Path) {
            crate::keyfile::fake_fs(Some(0x0102_1997));
        }
        fn replace_committed(home: &Path) {
            std::fs::write(home.join("config.toml"), "later owner edit = [").unwrap();
        }
        for after_commit in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
            let original = b"private destination refusal canary = [";
            std::fs::write(p.join("config.toml"), original).unwrap();
            let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
            let preview = v.recovery_preview(b"{}");
            assert_eq!(preview.status, 200);
            let body = serde_json::to_vec(
                &json!({"preview_key":json_of(&preview)["preview_key"],"confirmed":true}),
            )
            .unwrap();
            if after_commit {
                crate::settings::recovery::AFTER_COMMIT.set(Some(replace_committed));
            } else {
                crate::settings::recovery::BEFORE_COPY_WRITE.set(Some(untrusted_copy));
            }
            let result = v.recovery_start(&body);
            crate::keyfile::fake_fs(None);
            assert!(
                crate::settings::recovery::BEFORE_COPY_WRITE
                    .take()
                    .is_none()
            );
            assert!(crate::settings::recovery::AFTER_COMMIT.take().is_none());
            let copies: Vec<_> = std::fs::read_dir(p)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .contains(".recovery-")
                })
                .collect();
            assert_eq!(copies.len(), 1);
            if after_commit {
                assert_eq!(result.status, 200);
                assert_eq!(json_of(&result)["phase"], "unknown");
                assert_eq!(std::fs::read(&copies[0]).unwrap(), original);
                assert_eq!(
                    std::fs::read(p.join("config.toml")).unwrap(),
                    b"later owner edit = ["
                );
            } else {
                assert_eq!(result.status, 422);
                assert_eq!(std::fs::read(&copies[0]).unwrap(), b"");
                assert_eq!(std::fs::read(p.join("config.toml")).unwrap(), original);
            }
            assert!(
                !String::from_utf8_lossy(&result.body)
                    .contains("private destination refusal canary")
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn w6f_recovery_refuses_changed_or_unsafe_settings_without_replacing_them() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
        for kind in [
            "edit",
            "same_bytes_new_file",
            "symlink",
            "hardlink",
            "readonly",
            "state_alias",
            "lock_alias",
        ] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
            let file = p.join("config.toml");
            std::fs::write(&file, "invalid = [").unwrap();
            let v = Viewer::new(p, None, 4321, Token::Run("t0k".into()));
            let shown = w6f_post(&v, "/api/settings/recovery/preview", json!({}));
            assert_eq!(shown.status, 200, "{kind}");
            let shown = json_of(&shown);
            let body = json!({"preview_key":shown["preview_key"],"confirmed":true});
            match kind {
                "edit" => std::fs::write(&file, "later private content = [").unwrap(),
                "same_bytes_new_file" => {
                    // Keep the old inode alive, so this proves identity and not inode reuse.
                    std::fs::rename(&file, p.join("previous")).unwrap();
                    std::fs::write(&file, "invalid = [").unwrap();
                }
                "symlink" => {
                    std::fs::rename(&file, p.join("previous")).unwrap();
                    symlink(p.join("previous"), &file).unwrap();
                }
                "hardlink" => std::fs::hard_link(&file, p.join("other")).unwrap(),
                "readonly" => {
                    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap()
                }
                "state_alias" => {
                    std::fs::create_dir(p.join("other-state")).unwrap();
                    symlink(p.join("other-state"), p.join("state")).unwrap();
                }
                "lock_alias" => {
                    std::fs::DirBuilder::new()
                        .mode(0o700)
                        .create(p.join("state"))
                        .unwrap();
                    std::fs::write(p.join("other-lock"), "unchanged lock canary").unwrap();
                    symlink(p.join("other-lock"), p.join("state/config.lock")).unwrap();
                }
                _ => unreachable!(),
            }
            let original = std::fs::read(&file).unwrap();
            let refused = w6f_post(&v, "/api/settings/recovery/start", body);
            assert!(
                [409, 422].contains(&refused.status),
                "{kind}: recovery was not refused"
            );
            assert_eq!(std::fs::read(&file).unwrap(), original, "{kind}");
            assert!(!String::from_utf8_lossy(&refused.body).contains("private content"));
            assert!(
                !std::fs::read_dir(p).unwrap().any(|e| e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".recovery-")),
                "{kind}"
            );
        }
    }

    #[test]
    fn w6_setup_inventory_is_authenticated_and_keeps_an_absent_home_absent() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("missing");
        let v = Viewer::new(&home, None, 4321, Token::Run("t0k".into()));
        let target = "/api/setup";
        assert_eq!(v.route("GET", target, &[HOST]).status, 401);
        assert_eq!(
            v.route("GET", target, &[("Host", "elsewhere:4321"), TOKEN])
                .status,
            403
        );
        assert_eq!(v.route("POST", target, &[HOST, TOKEN]).status, 405);
        let answer = v.route("GET", target, &[HOST, TOKEN]);
        assert_eq!(
            answer.status, 200,
            "query-only agent inventory is unavailable"
        );
        let value: Value = serde_json::from_slice(&answer.body).unwrap();
        let agents = value["agents"].as_array().unwrap();
        assert_eq!(
            agents
                .iter()
                .map(|row| row["agent"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["claude", "codex", "grok", "agy", "opencode", "pi", "cursor"]
        );
        assert!(agents.iter().all(|row| row["live_verified"] == false));
        for target in [
            "/api/setup?extra=1",
            "/api/setup?extra",
            "/api/setup?extra&ignored",
        ] {
            assert_eq!(v.route("GET", target, &[HOST, TOKEN]).status, 400);
        }
        assert!(!home.exists(), "inventory initialized the absent home");
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    fn w6_setup_inventory_classifies_config_without_opening_stores_or_echoing_errors() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let secret = "SyntheticReadinessPrivateValue";
        let untouched = ["raw.db", "knowledge.db", "providers.db"];
        for name in untouched {
            std::fs::write(home.join(name), b"not a store; must remain untouched").unwrap();
        }
        let v = Viewer::new(home, None, 4321, Token::Run("t0k".into()));
        for (config, expected) in [
            (
                "providers = []\n[worker]\nresident = false\n".to_owned(),
                "valid",
            ),
            (format!("private_value = '{secret}"), "invalid"),
        ] {
            std::fs::write(home.join("config.toml"), &config).unwrap();
            let answer = v.route("GET", "/api/setup", &[HOST, TOKEN]);
            assert_eq!(answer.status, 200);
            let value: Value = serde_json::from_slice(&answer.body).unwrap();
            assert_eq!(value["home"], "present");
            assert_eq!(value["config"], expected);
            assert_eq!(value["agents"].as_array().unwrap().len(), 7);
            assert!(!String::from_utf8(answer.body).unwrap().contains(secret));
            assert!(std::fs::read_to_string(home.join("config.toml")).unwrap() == config);
            for name in untouched {
                assert!(
                    std::fs::read(home.join(name)).unwrap()
                        == b"not a store; must remain untouched"
                );
            }
            assert_eq!(std::fs::read_dir(home).unwrap().count(), 4);
        }
    }

    #[test]
    fn maintenance_status_is_authenticated_bounded_and_opens_no_store() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("missing");
        let v = Viewer::new(&home, None, 4321, Token::Run("t0k".into()));
        let target = "/api/maintenance";
        assert_eq!(v.route("GET", target, &[HOST]).status, 401);
        assert_eq!(
            v.route("GET", target, &[("Host", "elsewhere:4321"), TOKEN])
                .status,
            403
        );
        assert_eq!(v.route("POST", target, &[HOST, TOKEN]).status, 405);
        let answer = v.route("GET", target, &[HOST, TOKEN]);
        assert_eq!(answer.status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&answer.body).unwrap(),
            json!({"available":false,"active":null,"last":null})
        );
        for target in [
            "/api/maintenance?extra=1",
            "/api/maintenance?extra",
            "/api/maintenance?extra&ignored",
        ] {
            assert_eq!(v.route("GET", target, &[HOST, TOKEN]).status, 400);
        }
        assert!(!home.exists(), "status initialized the absent destination");
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    fn w6a_agent_operations_reject_untyped_input_before_effects() {
        let home = tempfile::tempdir().unwrap();
        let v = Viewer::new(home.path(), None, 4321, Token::Run("t0k".into()));
        for path in ["/api/setup/preview", "/api/setup/start"] {
            save_guards(&v, path, MAX_BODY, b"{}");
            for body in [
                json!({}),
                json!({"action":"repair","agents":["claude"]}),
                json!({"action":"wire","agents":[]}),
                json!({"action":"wire","agents":["claude","claude"]}),
                json!({"action":"wire","agents":["not-an-agent"]}),
                json!({"action":"wire","agents":["claude"],"path":"private-input-canary"}),
                json!({"action":"wire","agents":["claude"],"command":"private-input-canary"}),
            ] {
                let body = body.to_string();
                let length = body.len().to_string();
                let answer = request(
                    &v,
                    "POST",
                    path,
                    &[
                        HOST,
                        TOKEN,
                        ("Origin", "http://127.0.0.1:4321"),
                        ("Content-Type", "application/json"),
                        ("Content-Length", &length),
                    ],
                    body.as_bytes(),
                );
                assert_eq!(answer.status, 400);
                assert!(
                    !String::from_utf8(answer.body)
                        .unwrap()
                        .contains("private-input-canary")
                );
            }
        }
        let path = "/api/setup/operation";
        assert_eq!(v.route("GET", path, &[HOST]).status, 401);
        assert_eq!(
            v.route("GET", "/api/setup/operation?extra=1", &[HOST, TOKEN])
                .status,
            400
        );
        assert_eq!(get(&v, path), json!({"active":null,"last":null}));
        assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
    }

    #[test]
    fn maintenance_posts_share_all_head_guards_before_operation_work() {
        let (dir, v) = viewer("maintenance-head-guards");
        let body = br#"{"operation":{"kind":"not_supported"}}"#;
        for path in ["/api/maintenance/preview", "/api/maintenance/start"] {
            save_guards(&v, path, MAX_BODY, body);
            let len = body.len().to_string();
            let response = request(
                &v,
                "POST",
                path,
                &[
                    HOST,
                    TOKEN,
                    ("Origin", "http://127.0.0.1:4321"),
                    ("Content-Type", "application/json"),
                    ("Content-Length", len.as_str()),
                ],
                body,
            );
            assert_eq!(response.status, 400);
            assert_eq!(
                serde_json::from_slice::<Value>(&response.body).unwrap(),
                json!({"code":"bad_request","field":""})
            );
        }
        assert!(!dir.join("raw.db").exists());
        assert!(!dir.join("config.toml").exists());
        assert!(!dir.join("state").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn w5c_recuration_preparation_is_typed_stable_and_sends_nothing() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(
            p.join("config.toml"),
            r#"[[providers]]
kind = "openai"
name = "synthetic-paid"
base_url = "http://127.0.0.1:9/v1"
model = "synthetic"
limits = { usd_per_mtok_in = 0.0, usd_per_mtok_out = 1000.0, max_output_tokens = 2000 }
[summary]
curate = false
"#,
        )
        .unwrap();
        let mut raw = crate::raw::open(p).unwrap();
        let mut event = crate::raw::test_event(r#"{"prompt":"We use tabs."}"#);
        event.kind = "prompt".into();
        raw.append(&event).unwrap();
        raw.append_ops(&[(
            crate::raw::OpKind::Window,
            json!({"from_seq":1,"from_offset":null,"to_seq":1,"to_offset":null,
                "outcome":"curated","elided":[]}),
        )])
        .unwrap();
        drop(raw);
        let v = Viewer::new(p, Some(p.to_path_buf()), 4321, Token::Run("t0k".into()));
        let operation = json!({"kind":"recurate","scope":{"kind":"records","from":1,"to":1}});
        let body = serde_json::to_vec(&json!({"operation":operation})).unwrap();
        let length = body.len().to_string();
        let prepare = || {
            request(
                &v,
                "POST",
                "/api/maintenance/preview",
                &[
                    HOST,
                    TOKEN,
                    ("Origin", "http://127.0.0.1:4321"),
                    ("Content-Type", "application/json"),
                    ("Content-Length", &length),
                ],
                &body,
            )
        };
        let first = prepare();
        assert_eq!(
            first.status, 200,
            "typed recuration preparation is unavailable"
        );
        let first: Value = serde_json::from_slice(&first.body).unwrap();
        assert_eq!(first["kind"], "recurate");
        assert_eq!(first["scope"], operation["scope"]);
        assert_eq!(first["no_model_request"], true);
        assert_eq!(first["local_preparation"], true);
        assert_eq!(first["plan"]["windows"], 1);
        assert_eq!(first["plan"]["worst_paid_usd"], 2.0);
        assert_eq!(first["preparation"]["last"]["phase"], "prepared");
        assert_eq!(
            first["preparation"]["last"]["result"]["outcome"]["index"]["state"],
            "complete"
        );
        assert_eq!(first["preview_key"].as_str().unwrap().len(), 64);
        assert!(
            !p.join("providers.db").exists(),
            "preparation opened the paid ledger"
        );
        let second = prepare();
        assert_eq!(second.status, 200);
        let second: Value = serde_json::from_slice(&second.body).unwrap();
        assert_eq!(
            second["preview_key"], first["preview_key"],
            "unchanged preparation changed consent"
        );
        assert!(!p.join("providers.db").exists());
    }

    #[test]
    fn w5c_confirmation_requires_its_prepared_scope_and_unchanged_sources() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(
            p.join("config.toml"),
            "providers = []\n[summary]\ncurate = false\n[embedding]\nprovider = 'none'\n",
        )
        .unwrap();
        crate::raw::open(p)
            .unwrap()
            .append(&crate::raw::test_event("synthetic first record"))
            .unwrap();
        let viewer = || Viewer::new(p, None, 4321, Token::Run("t0k".into()));
        let v = viewer();
        let post = |v: &Viewer, route: &str, value: Value| {
            let body = serde_json::to_vec(&value).unwrap();
            let length = body.len().to_string();
            request(
                v,
                "POST",
                route,
                &[
                    HOST,
                    TOKEN,
                    ("Origin", "http://127.0.0.1:4321"),
                    ("Content-Type", "application/json"),
                    ("Content-Length", &length),
                ],
                &body,
            )
        };
        let operation = json!({"kind":"recurate","scope":{"kind":"queued"}});
        let shown = post(
            &v,
            "/api/maintenance/preview",
            json!({"operation":operation}),
        );
        assert_eq!(shown.status, 200);
        let shown: Value = serde_json::from_slice(&shown.body).unwrap();
        let body = json!({"operation":operation, "preview_key":shown["preview_key"],
            "operation_id":"a".repeat(64), "confirmed":true});
        let before = crate::backup::tests::w5b_files(p);
        let mut wrong_scope = body.clone();
        wrong_scope["operation"]["scope"] = json!({"kind":"skipped"});
        assert_eq!(post(&v, "/api/maintenance/start", wrong_scope).status, 409);
        assert_eq!(
            post(&viewer(), "/api/maintenance/start", body.clone()).status,
            409,
            "a restarted viewer accepted consent it never prepared"
        );
        assert_eq!(crate::backup::tests::w5b_files(p), before);
        assert!(get(&v, "/api/maintenance")["last"].get("consent").is_none());

        crate::raw::open(p)
            .unwrap()
            .append(&crate::raw::test_event("synthetic later record"))
            .unwrap();
        let changed = crate::backup::tests::w5b_files(p);
        let refused = post(&v, "/api/maintenance/start", body);
        assert_eq!(refused.status, 200);
        let refused: Value = serde_json::from_slice(&refused.body).unwrap();
        assert_eq!(refused["last"]["phase"], "failed");
        assert_eq!(refused["last"]["result"]["code"], "maintenance_stale");
        assert_eq!(refused["last"]["committed"], false);
        assert_eq!(
            crate::backup::tests::w5b_files(p),
            changed,
            "already-stale consent changed stores or native admission state"
        );
        assert!(!p.join("providers.db").exists());
    }

    #[test]
    fn w5c_preparation_indexes_a_hook_record_appended_while_its_lock_was_held() {
        fn append_while_busy(home: &Path) {
            assert!(crate::worker::lock(home).unwrap().is_none());
            let mut raw = crate::raw::open(home).unwrap();
            let mut event = crate::raw::test_event(r#"{"prompt":"lateprepneedle"}"#);
            event.kind = "prompt".into();
            raw.append(&event).unwrap();
        }
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::write(
            p.join("config.toml"),
            "providers = []\n[summary]\ncurate = false\n[embedding]\nprovider = 'none'\n",
        )
        .unwrap();
        let mut raw = crate::raw::open(p).unwrap();
        raw.append(&crate::raw::test_event("first synthetic record"))
            .unwrap();
        drop(raw);
        let v = Viewer::new(p, Some(p.to_path_buf()), 4321, Token::Run("t0k".into()));
        let body = br#"{"operation":{"kind":"recurate","scope":{"kind":"queued"}}}"#;
        let length = body.len().to_string();
        crate::worker::AFTER_PREPARATION_PLAN.set(Some(append_while_busy));
        let response = request(
            &v,
            "POST",
            "/api/maintenance/preview",
            &[
                HOST,
                TOKEN,
                ("Origin", "http://127.0.0.1:4321"),
                ("Content-Type", "application/json"),
                ("Content-Length", &length),
            ],
            body,
        );
        let unused = crate::worker::AFTER_PREPARATION_PLAN.take();
        assert!(
            unused.is_none(),
            "the late capture boundary was not exercised"
        );
        assert_eq!(response.status, 200);
        let status = get(&v, "/api/maintenance");
        assert_eq!(status["active"], Value::Null);
        assert_eq!(
            status["last"]["result"]["outcome"]["index"]["state"], "complete",
            "late-capture receipt: {}",
            status["last"]["result"]
        );
        let found = get(&v, "/api/search?q=lateprepneedle&all=1");
        assert_eq!(
            found["hits"].as_array().unwrap().len(),
            1,
            "a hook that found preparation busy was left unindexed"
        );
        assert!(!p.join("providers.db").exists());
    }

    #[cfg(unix)]
    #[test]
    fn status_probe_does_not_report_up_from_a_symlink_and_stale_listening_outcome() {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let target = home.path().join("unrelated-held-file");
        std::fs::write(&target, "synthetic unrelated status canary").unwrap();
        let held = std::fs::File::open(&target).unwrap();
        held.lock().unwrap();
        std::os::unix::fs::symlink(&target, state.join("view.lock")).unwrap();
        std::fs::write(state.join("view-outcome"), "listening 17373").unwrap();
        let line = resident_line(home.path(), 17373);
        assert!(
            line.contains("not running"),
            "stale outcome plus a link reported the viewer up"
        );
        assert!(
            std::fs::symlink_metadata(state.join("view.lock"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(target).unwrap(),
            b"synthetic unrelated status canary"
        );
        assert_eq!(
            std::fs::read(state.join("view-outcome")).unwrap(),
            b"listening 17373"
        );
        assert!(!state.join("view-token").exists());
    }

    /// A viewer over an empty home, for the guards and the saves.
    fn viewer(name: &str) -> (PathBuf, Viewer) {
        let dir = std::env::temp_dir().join(format!("oboete-view-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = Viewer::new(&dir, Some(dir.clone()), 4321, Token::Run("t0k".into()));
        (dir, v)
    }

    const R: &str = "github.com/o/r";

    /// What `seeded` put in the stores.
    struct Seeded {
        /// A decision a proposal ended: no delivered pair, so not delivered (D11).
        old: String,
        proposal: String,
        current: String,
        imported: String,
        start: String,
    }

    /// Design B's stores with, in `github.com/o/r`: a decision a proposal ended, the proposal, a
    /// current decision whose text holds markup, a session start and a claude-mem document;
    /// providers.db with calls of two providers; and the viewer started in that repository's
    /// checkout, on branch `main`.
    fn seeded() -> (Store, Viewer, Seeded) {
        let mut s = Store::new();
        let dir = s.home.path().join("r");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(
            dir.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:o/r.git\n",
        )
        .unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let old = s.decided(
            R,
            1_700_000_000_000,
            "Parser caches stay in Redis with a TTL.",
            &[],
        );
        let seq = s.said(
            "s",
            R,
            1_700_100_000_000,
            "Move the parser caches to files.",
        );
        let proposal = s.claim(
            seq,
            "Move the parser caches to files.",
            ("decision", "proposed", "assistant proposal"),
            &[&old],
        );
        let current = s.decided(
            R,
            1_700_200_000_000,
            "Parser errors go to stderr as <b>bold</b> text.",
            &[],
        );
        let imported = s.imported(
            "o1",
            "r",
            1_699_000_000_000,
            "Parser notes",
            "The parser caches its tables.",
        );
        let start = crate::raw::Event {
            kind: "start".into(),
            session: "s2".into(),
            repo: Some(R.into()),
            branch: Some("main".into()),
            ts: 1_700_300_000_000,
            ..crate::raw::test_event(r#"{"source":"startup"}"#)
        };
        let seq = s.raw.append(&start).unwrap();
        let start = s.key(seq);
        s.run();
        let p = crate::providers_db::open(s.home.path()).unwrap();
        for (provider, outcome, ms) in [
            ("groq", "ok", 400),
            ("groq", "ok", 800),
            ("mistral", "error", 50),
        ] {
            crate::providers_db::record(
                &p,
                &crate::providers_db::Call {
                    provider,
                    role: "curator",
                    span: "s1",
                    outcome,
                    ms,
                    detail: None,
                    bytes_out: 1,
                    est_tokens: None,
                    usage: Default::default(),
                    usd: None,
                },
            )
            .unwrap();
        }
        let v = Viewer::new(s.home.path(), Some(dir), 4321, Token::Run("t0k".into()));
        (
            s,
            v,
            Seeded {
                old,
                proposal,
                current,
                imported,
                start,
            },
        )
    }

    const HOST: (&str, &str) = ("Host", "127.0.0.1:4321");
    const TOKEN: (&str, &str) = ("X-Oboete-Token", "t0k");

    fn json_of(r: &Response) -> Value {
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        serde_json::from_slice(&r.body).unwrap()
    }

    fn get(v: &Viewer, target: &str) -> Value {
        json_of(&v.route("GET", target, &[HOST, TOKEN]))
    }

    /// A feed over invented records, without the older timeline fixture's claims or imports.
    fn feed_fixture() -> (Store, Viewer) {
        let s = Store::new();
        let v = Viewer::new(
            s.home.path(),
            Some(s.home.path().to_owned()),
            4321,
            Token::Run("t0k".into()),
        );
        (s, v)
    }

    /// A window's cards; the source seq and their op seq (also used by their public IDs).
    fn feed_cards(s: &mut Store, repo: &str, ts: i64, texts: &[&str]) -> (i64, i64) {
        let seq = s.event(
            "tool",
            "s",
            (repo, "main"),
            ts,
            json!({"output": "Read a file."}),
        );
        let observations: Vec<_> = texts
            .iter()
            .map(|text| {
                json!({
                    "type": "feature", "title": text, "subtitle": text, "narrative": text,
                    "facts": [text], "concepts": ["what-changed"],
                    "files_read": ["src/parser.rs"], "files_modified": ["docs/parser.md"]
                })
            })
            .collect();
        let op = json!({"outcome": "curated", "from_seq": seq, "to_seq": seq,
            "observations": observations});
        let op_seq = s
            .raw
            .append_ops(&[(crate::raw::OpKind::Window, op)])
            .unwrap()[0];
        (seq, op_seq)
    }

    /// A turn's summary, as the consumer reads it from the op log; its source seq and public ID.
    fn feed_summary(s: &mut Store, repo: &str, ts: i64, text: &str) -> (i64, String) {
        let seq = s.event(
            "reply",
            "s",
            (repo, "main"),
            ts,
            json!({"assistant": "Done."}),
        );
        let op = crate::turns::TurnOp {
            agent: "claude".into(),
            session: "s".into(),
            repo: Some(repo.into()),
            ts,
            from: seq,
            through: seq,
            read: Vec::new(),
            goals: Vec::new(),
            removed: Vec::new(),
            fields: [
                ("request", text),
                ("investigated", text),
                ("learned", text),
                ("completed", text),
                ("next_steps", text),
                ("notes", text),
            ]
            .into_iter()
            .map(|(f, t)| (f.to_owned(), t.to_owned()))
            .collect(),
            skipped: false,
            excluded: false,
        };
        let op_seq = s
            .raw
            .append_ops(&[(crate::raw::OpKind::Turn, serde_json::to_value(op).unwrap())])
            .unwrap()[0];
        (seq, format!("S{op_seq}"))
    }

    fn feed_ids(page: &Value) -> Vec<(String, String)> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["kind"].as_str().unwrap().to_owned(),
                    item["id"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn feed_before(cursor: &str, limit: usize) -> String {
        let cursor =
            percent_encoding::utf8_percent_encode(cursor, percent_encoding::NON_ALPHANUMERIC);
        format!("/api/feed?all=1&limit={limit}&before={cursor}")
    }

    /// page.md test 1: each kind's own time, kind and numeric ID ties, bounded pages, and a
    /// cursor that still reaches every original row once after writes between pages.
    #[test]
    fn feed_orders_and_pages_each_kind_without_repeats_or_skips() {
        let (mut s, v) = feed_fixture();
        let (_, card1) = feed_cards(&mut s, "example.test/team/fern", 3_000, &["First card"]);
        let (_, card2) = feed_cards(
            &mut s,
            "example.test/team/fern",
            3_000,
            &["Second", "Third"],
        );
        let (_, summary1) = feed_summary(&mut s, "example.test/team/fern", 3_000, "First summary");
        let (_, summary2) = feed_summary(&mut s, "example.test/team/fern", 3_000, "Second summary");
        let prompt1 = s.said("s", "example.test/team/fern", 3_000, "First prompt");
        let prompt2 = s.said("s", "example.test/team/fern", 3_000, "Second prompt");
        feed_cards(&mut s, "example.test/team/fern", 2_000, &["Older card"]);
        feed_summary(&mut s, "example.test/team/fern", 1_000, "Older summary");
        for n in 0..105 {
            s.said(
                "s",
                "example.test/team/fern",
                500,
                &format!("Older prompt {n}"),
            );
        }
        s.run();
        let first = get(&v, "/api/feed?all=1&limit=100");
        assert_eq!(first["items"].as_array().unwrap().len(), 100);
        assert_eq!(
            get(&v, "/api/feed?all=1")["items"]
                .as_array()
                .unwrap()
                .len(),
            50
        );
        assert_eq!(
            get(&v, "/api/feed?all=1&limit=999")["items"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
        let expected = [
            ("card", format!("{card2}.1")),
            ("card", format!("{card2}.0")),
            ("card", format!("{card1}.0")),
            ("summary", summary2),
            ("summary", summary1),
            ("prompt", s.key(prompt2)),
            ("prompt", s.key(prompt1)),
        ]
        .map(|(kind, id)| (kind.to_owned(), id));
        assert_eq!(&feed_ids(&first)[..7], &expected);
        let rest = get(&v, &feed_before(first["next"].as_str().unwrap(), 100));
        assert!(rest["next"].is_null());
        let original: Vec<_> = feed_ids(&first)
            .into_iter()
            .chain(feed_ids(&rest))
            .collect();
        assert_eq!(original.len(), 114);

        let mut page = get(&v, "/api/feed?all=1&limit=2");
        let mut paged = feed_ids(&page);
        // Only cards have been consumed. Other kinds' unchanged positions must survive.
        feed_cards(
            &mut s,
            "example.test/team/fern",
            4_000,
            &["Just added card"],
        );
        feed_summary(
            &mut s,
            "example.test/team/fern",
            4_000,
            "Just added summary",
        );
        s.said("s", "example.test/team/fern", 4_000, "Just added prompt");
        s.run();
        for _ in 0..original.len() {
            let Some(next) = page["next"].as_str() else {
                break;
            };
            page = get(&v, &feed_before(next, 2));
            assert!(page["items"].as_array().unwrap().len() <= 2);
            paged.extend(feed_ids(&page));
        }
        assert!(page["next"].is_null());
        let unique: std::collections::HashSet<_> = paged.iter().collect();
        assert_eq!(unique.len(), paged.len());
        assert!(original.iter().all(|id| unique.contains(id)));
    }

    /// page.md test 2: one repository scopes every kind, including the default checkout;
    /// all=1 overrides it and names each repository as the manifest does.
    #[test]
    fn feed_scopes_every_kind_to_the_repository_or_all() {
        let (mut s, mut v) = feed_fixture();
        for repo in ["example.test/team/fern", "example.test/team/moss"] {
            feed_cards(&mut s, repo, 3_000, &["A card"]);
            feed_summary(&mut s, repo, 2_000, "A summary");
            s.said("s", repo, 1_000, "A prompt");
        }
        s.run();
        let cwd = s.home.path().join("fern");
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        std::fs::write(
            cwd.join(".git/config"),
            "[remote \"origin\"]\nurl = https://example.test/team/fern.git\n",
        )
        .unwrap();
        std::fs::write(cwd.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        v.cwd = Some(cwd);
        for url in ["/api/feed?repo=example.test%2Fteam%2Ffern", "/api/feed"] {
            let page = get(&v, url);
            assert_eq!(
                feed_ids(&page)
                    .iter()
                    .map(|(kind, _)| kind.as_str())
                    .collect::<Vec<_>>(),
                ["card", "summary", "prompt"]
            );
            for item in page["items"].as_array().unwrap() {
                assert_eq!(item["repo"], "example.test/team/fern");
                assert_eq!(item["repo_name"], "fern");
            }
            assert!(page["next"].is_null());
        }
        let all = get(&v, "/api/feed?repo=example.test%2Fteam%2Ffern&all=1");
        assert_eq!(all["items"].as_array().unwrap().len(), 6);
        assert!(
            all["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["repo"] == "example.test/team/moss")
        );
        let missing = get(&v, "/api/feed?repo=missing");
        assert_eq!(missing["items"], json!([]));
        assert!(missing["next"].is_null());
    }

    /// page.md test 3: all three readers honor removals before a consumer catches up; skipped
    /// summaries and imported-source prompts never enter the feed, or consume a page slot.
    #[test]
    fn feed_hides_removed_and_skipped_items_and_imported_prompts() {
        let (mut s, v) = feed_fixture();
        let repo = "example.test/team/fern";
        let (card_seq, _) = feed_cards(&mut s, repo, 9_000, &["Hidden card"]);
        let (summary_seq, _) = feed_summary(&mut s, repo, 8_000, "Hidden summary");
        let prompt_seq = s.said("s", repo, 7_000, "Hidden prompt");
        let (seq, _) = feed_summary(&mut s, repo, 6_000, "Skipped summary");
        let skip = json!({"agent": "claude", "session": "s", "repo": repo, "ts": 6_000,
            "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {}, "skipped": true});
        s.raw
            .append_ops(&[(crate::raw::OpKind::Turn, skip)])
            .unwrap();
        // The earlier summary is separately hidden: a skipped op does not replace it.
        s.raw
            .append_tombstone(crate::raw::Target::Record {
                device: s.raw.device().to_owned(),
                seq,
            })
            .unwrap();
        for source in ["transcript", "oboete-v1"] {
            s.raw
                .append(&crate::raw::Event {
                    kind: "prompt".into(),
                    source: source.into(),
                    session: "s".into(),
                    repo: Some(repo.into()),
                    ts: 10_000,
                    ..crate::raw::test_event(r#"{"prompt":"Imported prompt"}"#)
                })
                .unwrap();
        }
        let (_, card) = feed_cards(&mut s, repo, 3_000, &["Visible card"]);
        let (_, summary) = feed_summary(&mut s, repo, 2_000, "Visible summary");
        let prompt = s
            .raw
            .append(&crate::raw::Event {
                kind: "prompt".into(),
                source: "replay".into(),
                session: "s".into(),
                repo: Some(repo.into()),
                ts: 1_000,
                ..crate::raw::test_event(r#"{"prompt":"Visible prompt"}"#)
            })
            .unwrap();
        s.run();
        for seq in [card_seq, summary_seq, prompt_seq] {
            s.raw
                .append_tombstone(crate::raw::Target::Record {
                    device: s.raw.device().to_owned(),
                    seq,
                })
                .unwrap();
        }
        let mut page = get(&v, "/api/feed?all=1&limit=1");
        let mut ids = feed_ids(&page);
        for _ in 0..4 {
            let Some(cursor) = page["next"].as_str() else {
                break;
            };
            page = get(&v, &feed_before(cursor, 1));
            ids.extend(feed_ids(&page));
        }
        assert_eq!(
            ids,
            [
                ("card".to_owned(), format!("{card}.0")),
                ("summary".to_owned(), summary),
                ("prompt".to_owned(), s.key(prompt))
            ]
        );
        assert!(page["next"].is_null());
        // A range removal leaves the record present, with only that text masked (D8).
        let seq = s.said("s", repo, 11_000, "keep remove keep");
        let body = json!({"prompt": "keep remove keep"}).to_string();
        s.raw
            .append_tombstone(crate::raw::Target::Range {
                device: s.raw.device().to_owned(),
                seq,
                offset: body.find("remove").unwrap() as i64,
                length: 6,
            })
            .unwrap();
        let page = get(&v, "/api/feed?all=1&limit=1");
        assert_eq!(page["items"][0]["id"], s.key(seq));
        assert_eq!(page["items"][0]["text"], "keep ****** keep");
    }

    /// page.md test 4: a newly added field-anchored rule applies on read, not only on capture;
    /// each card/summary field and each prompt is gated before the whole API answer.
    #[test]
    fn feed_applies_rules_added_after_every_kind_was_written() {
        let (mut s, v) = feed_fixture();
        let repo = "example.test/team/fern";
        feed_cards(&mut s, repo, 3_000, &["invented-FERN"]);
        feed_summary(&mut s, repo, 2_000, "invented-MOSS");
        s.said("s", repo, 1_000, "invented-REED");
        s.run();
        std::fs::write(s.home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{id = \"invented\", regex = 'invented-([A-Z]+)$', secret_group = 1}]\n").unwrap();
        let page = get(&v, "/api/feed?all=1");
        assert_eq!(page["items"][0]["title"], "invented-[REDACTED]");
        assert_eq!(page["items"][0]["facts"], json!(["invented-[REDACTED]"]));
        assert_eq!(page["items"][1]["fields"]["request"], "invented-[REDACTED]");
        assert_eq!(page["items"][2]["text"], "invented-[REDACTED]");
        let answer = page.to_string();
        for text in ["FERN", "MOSS", "REED"] {
            assert!(!answer.contains(text));
        }
        assert_eq!(page["items"][1]["fields"]["notes"], "invented-[REDACTED]");
    }

    /// Codex on #373: a page's cursor is its own state of ids and times, passed back as it came:
    /// a rule that matched it would break the paging. The rest of the answer is gated.
    #[test]
    fn a_page_cursor_leaves_as_it_came_and_the_rest_gated() {
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // split: scanners
        let cursor = format!("{{\"prompts\":[1,\"{token}\",2]}}");
        for name in ["feed", "timeline"] {
            let out = answered(
                name,
                json!({"items": [{"text": token.clone()}], "next": cursor.clone()}),
            );
            assert_eq!(out["next"], cursor, "{name}");
            assert_ne!(out["items"][0]["text"], token, "{name}");
        }
        let other = answered("search", json!({"next": cursor.clone()}));
        assert_ne!(other["next"], cursor);
    }

    /// page.md test 5: the display cap counts Unicode characters after gating, says it cut,
    /// and leaves the full prompt available through get's record key.
    #[test]
    fn feed_cuts_long_prompts_but_get_keeps_the_full_text() {
        let (mut s, v) = feed_fixture();
        let full = format!("{}終", "あ".repeat(2_000));
        let seq = s.said("s", "example.test/team/fern", 1_000, &full);
        let page = get(&v, "/api/feed?all=1");
        let item = &page["items"][0];
        assert_eq!(item["text"], format!("{}…", "あ".repeat(2_000)));
        assert_eq!(item["id"], s.key(seq));
        let doc = get(&v, &format!("/api/doc?id={}", item["id"].as_str().unwrap()));
        assert!(doc["text"].as_str().unwrap().contains(&full));
        let exact = "い".repeat(2_000);
        s.said("s", "example.test/team/fern", 2_000, &exact);
        assert_eq!(
            get(&v, "/api/feed?all=1&limit=1")["items"][0]["text"],
            exact
        );
    }

    /// page.md test 6: the new endpoint passes through the same guards, before any store read.
    #[test]
    fn feed_requires_the_token_local_host_and_read_method() {
        let (dir, v) = viewer("feed-guards");
        let url = "/api/feed?all=1";
        assert_eq!(v.route("GET", url, &[HOST]).status, 401);
        assert_eq!(
            v.route("GET", url, &[HOST, ("X-Oboete-Token", "wrong")])
                .status,
            401
        );
        assert_eq!(
            v.route("GET", url, &[("Host", "foreign.example:4321"), TOKEN])
                .status,
            403
        );
        for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
            assert_eq!(v.route(method, url, &[HOST, TOKEN]).status, 405);
        }
        for framing in [("Transfer-Encoding", "chunked"), ("Content-Length", "0")] {
            assert_eq!(v.route("GET", url, &[HOST, TOKEN, framing]).status, 400);
        }
        assert_eq!(v.route("HEAD", url, &[HOST, TOKEN]).status, 200);
        assert_eq!(get(&v, url), json!({"items": [], "next": null}));
        for cursor in ["%7B", "%7B%22pages%22%3A1%7D", &"7".repeat(4097)] {
            let url = format!("{url}&before={cursor}");
            assert_eq!(v.route("GET", &url, &[HOST, TOKEN]).status, 400, "{cursor}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// P5: prompts are read directly, so the poll marker must move on capture even while the
    /// worker has not consumed them. Tool calls and imported prompts still do not move it.
    #[test]
    fn feed_version_changes_for_live_prompts_before_the_worker_runs() {
        let (mut s, v) = feed_fixture();
        let repo = "example.test/team/fern";
        s.said("s", repo, 1_000, "Earlier prompt");
        s.run();
        let before = get(&v, "/api/version")["v"].clone();
        s.event(
            "tool",
            "s",
            (repo, "main"),
            2_000,
            json!({"output": "Read a file."}),
        );
        assert_eq!(get(&v, "/api/version")["v"], before);
        let seq = s.said("s", repo, 3_000, "A just captured prompt");
        let after = get(&v, "/api/version")["v"].clone();
        assert_ne!(after, before);
        assert_eq!(
            get(&v, "/api/feed?all=1&limit=1")["items"][0]["id"],
            s.key(seq)
        );
        s.raw
            .append(&crate::raw::Event {
                kind: "prompt".into(),
                source: "transcript".into(),
                session: "s".into(),
                repo: Some(repo.into()),
                ts: 4_000,
                ..crate::raw::test_event(r#"{"prompt":"Imported prompt"}"#)
            })
            .unwrap();
        assert_eq!(get(&v, "/api/version")["v"], after);
    }

    fn keys(hits: &Value, field: &str) -> Vec<String> {
        hits[field]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["key"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The owner's correction of claim `uid` (`oboete correct`'s op), applied.
    fn correct(s: &mut Store, uid: &str, status: Option<&str>, body: Option<&str>) {
        s.correct(uid, status, body);
        s.run();
    }

    #[test]
    fn viewer_json_labels_muted_claims_in_search_and_details() {
        let (s, v, x) = seeded();
        for muted in [true, false] {
            crate::claims::mute(s.home.path(), &x.current, muted).unwrap();
            let claim = get(&v, &format!("/api/claim?id={}", x.current));
            assert_eq!(claim["muted"], muted);
            assert_eq!(claim["status"], "decided");
            let found = get(&v, "/api/search?q=parser&raw=off");
            let hit = found["hits"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["key"] == x.current)
                .unwrap();
            assert_eq!(hit["muted"], muted);
        }
    }

    /// The stores alone, the fixture's own raw.db closed: a restore or a rebuild can swap them.
    fn closed(s: Store) -> tempfile::TempDir {
        let Store { home, raw } = s;
        drop(raw);
        home
    }

    /// Rows 53-1 and 53-4: every `/api` route asks for the token and this viewer's Host, the
    /// ones that only read included; the page's own files come without it, as a URL fragment
    /// never reaches the server (#374).
    #[test]
    fn every_api_route_answers_401_without_the_token_and_403_for_a_foreign_host() {
        let (_s, v, x) = seeded();
        let routes = [
            "/api/search?q=parser".to_owned(),
            format!("/api/doc?id={}", x.current),
            format!("/api/claim?id={}", x.current),
            "/api/timeline".into(),
            "/api/context".into(),
            "/api/repos".into(),
            "/api/version".into(),
            "/api/stats".into(),
            "/api/settings".into(),
            "/api/privacy".into(),
            "/api/setup".into(),
            "/api/setup/operation".into(),
            "/api/maintenance".into(),
        ];
        for route in &routes {
            assert_eq!(v.route("GET", route, &[HOST]).status, 401, "{route}");
            let foreign = [("Host", "evil.example:4321"), TOKEN];
            assert_eq!(v.route("GET", route, &foreign).status, 403, "{route}");
            assert_eq!(v.route("GET", route, &[HOST, TOKEN]).status, 200, "{route}");
        }
        for page in ["/", "/app.js", "/app.css"] {
            assert_eq!(v.route("GET", page, &[HOST]).status, 200, "{page}");
        }
        assert_eq!(
            v.route("GET", "/api/claim?id=nope", &[HOST, TOKEN]).status,
            404
        );
        assert_eq!(
            v.route("GET", "/api/doc?id=nope", &[HOST, TOKEN]).status,
            404
        );
    }

    #[test]
    fn w3_privacy_and_claim_writes_pass_every_guard_before_store_work() {
        let home = tempfile::tempdir().unwrap();
        let v = Viewer::new(home.path(), None, 4321, Token::Run("t0k".into()));
        assert_eq!(v.route("GET", "/api/privacy", &[HOST]).status, 401);
        assert_eq!(
            v.route(
                "GET",
                "/api/privacy",
                &[("Host", "foreign.example:4321"), TOKEN]
            )
            .status,
            403
        );
        let shown = get(&v, "/api/privacy");
        assert_eq!(shown["available"], true);
        assert_eq!(shown["rescan"]["state"], "empty");
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
        for path in [
            "/api/privacy/exclude",
            "/api/claims/correct",
            "/api/claims/mute",
            "/api/preferences",
        ] {
            save_guards(&v, path, MAX_BODY, b"{}");
            for method in ["PUT", "PATCH", "OPTIONS", "DELETE"] {
                assert_eq!(request(&v, method, path, &[HOST, TOKEN], b"").status, 405);
            }
            assert_eq!(
                request(&v, "POST", &format!("{path}?x=1"), &[HOST, TOKEN], b"{}").status,
                405
            );
        }
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }

    #[test]
    fn w3_claim_protocol_ids_survive_display_redaction_in_public_routes() {
        const CHILD: &str = "OBOETE_W3_UID_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Egress is process-wide. Exercise its real configuration in one isolated test,
            // without changing the rules seen by unrelated tests running in this process.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "view::tests::w3_claim_protocol_ids_survive_display_redaction_in_public_routes",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        let (mut store, v, x) = seeded();
        store.correct(&x.current, None, Some(&x.current));
        store.run();
        std::fs::write(store.home.path().join("config.toml"),
            "providers = []\n[summary]\ncurate = false\n[redaction]\nextra_rules = [{id = 'hash-text', regex = '^[0-9a-f]{64}$'}]\n").unwrap();
        redact::set_home(store.home.path()).unwrap();
        let route = format!("/api/claim?id={}", x.current);
        let shown = get(&v, &route);
        assert_eq!(shown["uid"], x.current);
        assert_eq!(
            shown["text"], "[REDACTED]",
            "claim text must still be gated"
        );
        let old = get(&v, &format!("/api/claim?id={}", x.old));
        assert_eq!(old["uid"], x.old);
        assert_eq!(old["later"], x.proposal);
        let proposal = get(&v, &format!("/api/claim?id={}", x.proposal));
        assert_eq!(proposal["supersedes"], json!([x.old]));
        let hits = get(&v, "/api/search?q=parser&history=1");
        assert!(
            hits["hits"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["key"] == x.old)
        );
        let timeline = get(&v, "/api/timeline?all=1");
        assert!(
            timeline["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["key"] == x.current)
        );
        let post = |path: &str, value: Value| {
            let body = serde_json::to_vec(&value).unwrap();
            let len = body.len().to_string();
            let response = request(
                &v,
                "POST",
                path,
                &[
                    HOST,
                    TOKEN,
                    ("Origin", "http://127.0.0.1:4321"),
                    ("Content-Type", "application/json"),
                    ("Content-Length", &len),
                ],
                &body,
            );
            assert_eq!(response.status, 200);
            json_of(&response)
        };
        assert_eq!(
            post(
                "/api/claims/correct",
                json!({"uid": shown["uid"], "body": "Keep the corrected parser design.", "status": "done"})
            ),
            json!({"state": "applied", "uid": x.current})
        );
        assert_eq!(get(&v, &route)["text"], "Keep the corrected parser design.");
        for muted in [true, false] {
            assert_eq!(
                post(
                    "/api/claims/mute",
                    json!({"uid": x.current, "muted": muted})
                ),
                json!({"state": "applied", "uid": x.current})
            );
            assert_eq!(get(&v, &route)["muted"], muted);
        }
        let preference = post(
            "/api/preferences",
            json!({"text": "Always use concise Japanese.", "apply_to_all_repos": true}),
        );
        assert_eq!(preference["state"], "applied");
        let uid = preference["uid"].as_str().unwrap();
        assert_eq!(uid.len(), 64);
        assert_eq!(get(&v, &format!("/api/claim?id={uid}"))["uid"], uid);
    }

    #[test]
    fn guards_come_before_any_data() {
        let (dir, v) = viewer("guards");
        let status = |m: &str, t: &str, h: &[(&str, &str)]| v.route(m, t, h).status;
        assert_eq!(status("GET", "/", &[HOST]), 200);
        assert_eq!(status("GET", "/", &[("host", "localhost:4321")]), 200);
        assert_eq!(status("POST", "/api/repos", &[HOST, TOKEN]), 405);
        // Nothing is deleted here (D11): DELETE is refused before the token, and a preflight
        // (OPTIONS) never succeeds.
        for target in ["/api/doc?id=o1", "/api/session?id=s1", "/api/repos", "/"] {
            assert_eq!(status("DELETE", target, &[]), 405, "{target}");
            assert_eq!(status("DELETE", target, &[HOST, TOKEN]), 405, "{target}");
        }
        assert_eq!(status("OPTIONS", "/api/doc?id=o1", &[HOST, TOKEN]), 405);
        // A GET's framing headers are refused: no request here has a body.
        for framing in [("Transfer-Encoding", "chunked"), ("Content-Length", "0")] {
            assert_eq!(status("GET", "/api/repos", &[HOST, TOKEN, framing]), 400);
            assert_eq!(status("GET", "/", &[HOST, framing]), 400);
        }
        assert_eq!(
            status("GET", "/", &[("Host", "evil.example:4321"), TOKEN]),
            403
        );
        assert_eq!(status("GET", "/", &[]), 403);
        assert_eq!(status("GET", "/", &[("Host", "127.0.0.1")]), 403);
        let (dir80, mut v80) = viewer("guards80");
        v80.port = 80.into();
        assert_eq!(v80.route("GET", "/", &[("Host", "127.0.0.1")]).status, 200);
        assert_eq!(
            v80.route("GET", "/", &[("Host", "localhost:80")]).status,
            200
        );
        std::fs::remove_dir_all(&dir80).ok();
        assert_eq!(status("GET", "/api/repos", &[HOST]), 401);
        assert_eq!(
            status("GET", "/api/repos", &[HOST, ("x-oboete-token", "t0")]),
            401
        );
        assert_eq!(status("GET", "/api/repos", &[HOST, TOKEN]), 200);
        assert_eq!(status("GET", "/../etc/passwd", &[HOST]), 404);
        assert_eq!(status("GET", "/api/nope", &[HOST, TOKEN]), 404);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The viewer on a socket of its own, answering `n` connections: its port and its thread.
    fn serving(mut v: Viewer, n: usize) -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        v.port = listener.local_addr().unwrap().port().into();
        let port = v.port();
        let server = std::thread::spawn(move || {
            for _ in 0..n {
                let (s, _) = listener.accept().unwrap();
                v.serve(s);
            }
        });
        (port, server)
    }

    fn ask(port: u16, raw: &[u8]) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(raw).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).ok();
        out
    }

    #[test]
    fn socket_round_trip() {
        let (_s, v, _) = seeded();
        let (port, server) = serving(v, 3);
        let ok = ask(
            port,
            format!(
                "GET /api/repos HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: t0k\r\n\r\n"
            )
            .as_bytes(),
        );
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(
            ok.contains("Content-Security-Policy: default-src 'none'")
                && ok.contains(&format!("\"current\":\"{R}\""))
        );
        let delete = ask(
            port,
            format!(
                "DELETE /api/doc?id=o1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: t0k\r\nContent-Length: 0\r\n\r\n"
            )
            .as_bytes(),
        );
        assert!(delete.starts_with("HTTP/1.1 405 "), "{delete}");
        let mut huge = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ").into_bytes();
        // Exactly the cap: every byte is read before the 400, so the close is a FIN, not a RST.
        huge.resize(MAX_HEAD, b'a');
        assert!(ask(port, &huge).starts_with("HTTP/1.1 400 "));
        server.join().unwrap();
    }

    /// Row 53-2: a head of exactly `MAX_HEAD` bytes is served; one byte more is refused.
    #[test]
    fn a_head_that_ends_exactly_at_the_cap_is_served_and_one_byte_more_is_refused() {
        let (dir, v) = viewer("head-exact");
        let (port, server) = serving(v, 2);
        let head = |size: usize| {
            let start = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ");
            let pad = size - start.len() - "\r\n\r\n".len();
            format!("{start}{}\r\n\r\n", "a".repeat(pad)).into_bytes()
        };
        assert_eq!(head(MAX_HEAD).len(), MAX_HEAD);
        assert!(ask(port, &head(MAX_HEAD)).starts_with("HTTP/1.1 200 "));
        assert!(ask(port, &head(MAX_HEAD + 1)).starts_with("HTTP/1.1 400 "));
        server.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Row 53-3: a head that keeps arriving a byte at a time is closed at `REQUEST_TIME` from
    /// its first byte, not reset by each byte.
    #[test]
    fn a_head_sent_a_little_at_a_time_is_closed_at_the_total_deadline() {
        let (dir, v) = viewer("head-slow");
        let (port, server) = serving(v, 1);
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let started = Instant::now();
        c.write_all(b"GET / HTTP/1.1\r\nX-Pad: ").unwrap();
        // A byte every 200 ms never completes the head; the server closes on its own clock.
        while c.write_all(b"a").is_ok() && started.elapsed() < REQUEST_TIME * 2 {
            std::thread::sleep(Duration::from_millis(200));
            if server.is_finished() {
                break;
            }
        }
        server.join().unwrap();
        let took = started.elapsed();
        assert!(
            took >= REQUEST_TIME && took < REQUEST_TIME + Duration::from_secs(2),
            "{took:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MUST-M11 to M13: each of the page's parameters reaches the search core as it is meant, a
    /// day for `until` counted to its end, `limit` held to `MAX_LIMIT`, and a bad value is 400.
    #[test]
    fn search_passes_every_parameter_to_the_search_core() {
        let q = search_query(&params(
            "q=parser+caches&repo=github.com%2Fx%2Fy&all=1&since=2023-11-14&until=2023-11-15&history=true&raw=only&limit=1000",
        ))
        .unwrap();
        assert_eq!(
            (
                q.text.as_str(),
                q.repo.as_deref(),
                q.all,
                q.history,
                q.raw,
                q.limit
            ),
            (
                "parser caches",
                Some("github.com/x/y"),
                true,
                true,
                search::b::RawArm::Only,
                MAX_LIMIT
            )
        );
        assert_eq!(q.since, Some(1_699_920_000_000));
        // `until` is exclusive: a day runs to the next one's start.
        assert_eq!(q.until, Some(1_700_092_800_000));
        let d = search_query(&params("q=x")).unwrap();
        assert_eq!(
            (d.repo, d.all, d.history, d.raw, d.limit, d.since, d.until),
            (None, false, false, search::b::RawArm::Below, 50, None, None)
        );
        for bad in [
            "since=yesterday",
            "until=2023",
            "raw=rrf:5",
            "raw=sideways",
            "all=yes",
            "history=2",
            "limit=-1",
            "limit=many",
        ] {
            assert_eq!(search_query(&params(bad)).unwrap_err().status, 400, "{bad}");
        }
        // Through the route: the decision of 2023-11-14 (22:13 UTC) is inside `until`'s day,
        // the later claims are not.
        let (_s, v, x) = seeded();
        let found = get(
            &v,
            "/api/search?q=parser&until=2023-11-14&history=1&raw=off",
        );
        let found = keys(&found, "hits");
        assert!(found.contains(&x.old), "{found:?}");
        assert!(
            !found.contains(&x.current) && !found.contains(&x.proposal),
            "{found:?}"
        );
        assert_eq!(
            v.route("GET", "/api/search?q=parser&since=soon", &[HOST, TOKEN])
                .status,
            400
        );
        let all = get(&v, "/api/search?q=parser&limit=1");
        assert_eq!(all["hits"].as_array().unwrap().len(), 1);
    }

    /// Row 30-2: the viewer's search is checked against the checkout it was started in, read
    /// when the search runs: once that label is excluded, a search of another repository sends
    /// no query text out either.
    #[test]
    fn a_viewer_search_is_checked_against_its_own_checkout() {
        let (mut s, v, _) = seeded();
        let other = "/api/search?q=parser&repo=github.com%2Fx%2Fother";
        assert_ne!(get(&v, other)["vector"], "excluded");
        s.exclude(R);
        let after = get(&v, other);
        assert_eq!(after["vector"], "excluded");
        assert!(after["why"].as_str().unwrap().contains("not sent out"));
        assert_eq!(get(&v, "/api/search?q=parser")["vector"], "excluded");
    }

    /// D11: a claim with its status, whether it is delivered, its links both ways, its quotes and
    /// its history; a document by any id `get` takes, its text as data.
    #[test]
    fn doc_and_claim_show_status_quotes_links_and_history() {
        let (mut s, v, x) = seeded();
        let old = get(&v, &format!("/api/claim?id={}", x.old));
        assert_eq!(
            (
                old["status"].as_str(),
                old["delivered"].as_bool(),
                old["label"].as_str()
            ),
            (Some("decided"), Some(false), Some("citable"))
        );
        assert_eq!(old["ended_by"], json!([x.proposal]));
        assert_eq!(old["later"], json!(x.proposal));
        assert_eq!(
            old["quotes"][0]["text"],
            "Parser caches stay in Redis with a TTL."
        );
        let proposal = get(&v, &format!("/api/claim?id={}", x.proposal));
        assert_eq!(proposal["supersedes"], json!([x.old]));
        assert_eq!(proposal["delivered"], true);
        // The owner marks the current decision done: its history has both, oldest first.
        let anchor = old["quotes"][0]["key"].as_str().unwrap().to_owned();
        let current = get(&v, &format!("/api/claim?id={}", &x.current[..12]));
        assert_eq!(current["uid"], json!(x.current));
        correct(&mut s, &x.current, Some("done"), None);
        let current = get(&v, &format!("/api/claim?id={}", x.current));
        assert_eq!(current["status"], "done");
        let history = current["history"].as_array().unwrap();
        assert_eq!(history.len(), 2, "{history:?}");
        assert_eq!(
            (history[0]["tier"].as_i64(), history[0]["status"].as_str()),
            (Some(1), Some("decided"))
        );
        assert_eq!(
            (history[1]["tier"].as_i64(), history[1]["status"].as_str()),
            (None, Some("done"))
        );
        // Not a claim: 404 here, a document for `doc`.
        assert_eq!(
            v.route(
                "GET",
                &format!("/api/claim?id={}", x.imported),
                &[HOST, TOKEN]
            )
            .status,
            404
        );
        let doc = |id: &str| {
            get(&v, &format!("/api/doc?id={id}"))["text"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert!(doc(&x.current).contains("<b>bold</b>"));
        assert!(doc(&x.imported).contains("Parser notes"));
        assert!(doc(&x.start).contains("start"));
        assert!(doc(&anchor).contains("Parser caches stay in Redis"));
    }

    /// D11: the timeline, the repositories and the version read raw.db and knowledge.db; an
    /// exclusion moves the version.
    #[test]
    fn timeline_repos_and_version_answer_from_the_b_stores() {
        let (mut s, v, x) = seeded();
        let tl = get(&v, "/api/timeline");
        assert_eq!(tl["repo"], R);
        let items = tl["items"].as_array().unwrap();
        assert_eq!(
            (items[0]["key"].as_str(), items[0]["class"].as_str()),
            (Some(x.start.as_str()), Some("start"))
        );
        let listed = keys(&tl, "items");
        for key in [&x.current, &x.proposal, &x.old, &x.imported] {
            assert!(listed.contains(key), "{listed:?}");
        }
        assert_eq!(tl["next"], Value::Null);
        assert_eq!(get(&v, "/api/timeline?all=1")["repo"], Value::Null);
        assert!(
            keys(
                &get(&v, "/api/timeline?repo=github.com%2Fx%2Fother"),
                "items"
            )
            .is_empty()
        );
        // Pages of 2 reach every entry once.
        let mut paged = Vec::new();
        let mut next = "/api/timeline?limit=2".to_owned();
        loop {
            let page = get(&v, &next);
            paged.extend(keys(&page, "items"));
            match page["next"].as_str() {
                Some(after) => next = format!("/api/timeline?limit=2&before={after}"),
                None => break,
            }
        }
        assert_eq!(paged, listed);
        assert_eq!(
            v.route("GET", "/api/timeline?before=soon", &[HOST, TOKEN])
                .status,
            400
        );
        let repos = get(&v, "/api/repos");
        assert_eq!(
            (repos["current"].as_str(), repos["branch"].as_str()),
            (Some(R), Some("main"))
        );
        let ours = repos["repos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["repo"] == R)
            .unwrap();
        assert_eq!(
            (ours["claims"].as_i64(), ours["records"].as_i64()),
            (Some(3), Some(4))
        );
        let v0 = get(&v, "/api/version")["v"].as_str().unwrap().to_owned();
        assert_eq!(get(&v, "/api/version")["v"], v0);
        s.exclude("github.com/x/other");
        let v1 = get(&v, "/api/version")["v"].as_str().unwrap().to_owned();
        assert_ne!(v1, v0);
        // A record hidden: the page redraws without it (CodeRabbit on #316).
        let device = s.raw.device().to_owned();
        s.raw
            .append_tombstone(crate::raw::Target::Record { device, seq: 1 })
            .unwrap();
        assert_ne!(get(&v, "/api/version")["v"], v1);
    }

    /// D11: entries that share one time are paged by key, each reached once.
    #[test]
    fn timeline_pages_reach_every_entry_of_one_time_once() {
        let (mut s, v, _) = seeded();
        let uids = s.imported_all(
            (0..7)
                .map(|i| {
                    (
                        format!("o{}", 10 + i),
                        "cm",
                        "decision",
                        1_600_000_000_000,
                        format!("note {i}"),
                    )
                })
                .collect(),
        );
        s.run();
        let mut paged = Vec::new();
        let mut next = "/api/timeline?repo=claude-mem%3Ap&limit=3".to_owned();
        loop {
            let page = get(&v, &next);
            paged.extend(keys(&page, "items"));
            match page["next"].as_str() {
                Some(after) => {
                    let after = percent_encoding::utf8_percent_encode(
                        after,
                        percent_encoding::NON_ALPHANUMERIC,
                    );
                    next = format!("/api/timeline?repo=claude-mem%3Ap&limit=3&before={after}");
                }
                None => break,
            }
        }
        let mut sorted = uids.clone();
        sorted.sort();
        assert_eq!(paged, sorted);
    }

    /// D4, D11: the Context page is what SessionStart shows a new session in the viewer's
    /// checkout, as `oboete inject` prints it.
    #[test]
    fn the_context_page_is_what_session_start_shows_for_that_checkout() {
        let (_s, v, _) = seeded();
        let ctx = get(&v, "/api/context");
        assert_eq!(
            (
                ctx["repo"].as_str(),
                ctx["branch"].as_str(),
                ctx["on"].as_bool()
            ),
            (Some(R), Some("main"), Some(true))
        );
        let shown = crate::hook::inject_text(&v.home, v.cwd.as_deref().unwrap(), None);
        assert!(shown.contains("Parser errors go to stderr"), "{shown}");
        assert_eq!(ctx["text"], shown);
        assert_eq!(ctx["chars"], shown.chars().count());
        let other = get(&v, "/api/context?repo=github.com%2Fx%2Fother");
        assert_eq!(other["repo"], "github.com/x/other");
        assert!(!other["text"].as_str().unwrap().contains("Parser"));
    }

    /// Codex on #408: the Context page of a home with no store yet shows what its first session
    /// start will, the work state section alone, and makes no store.
    #[test]
    fn the_context_page_of_a_home_with_no_store_shows_the_work_state_section() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("r");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let v = Viewer {
            token: Token::Run("t0k".into()),
            ..Viewer::new(home.path(), Some(dir), 4321, Token::File)
        };
        let ctx = get(&v, "/api/context");
        assert_eq!(ctx["text"], crate::work_state::nothing_open());
        assert!(!crate::raw::exists(home.path()));
    }

    /// Codex's security review of Task 7: the Context page answers a manifest it cannot read
    /// (D11: busy or locked is 503, `failed`), where a SessionStart hook only logs it and shows
    /// nothing: never an empty 200.
    #[test]
    fn a_context_page_that_cannot_read_the_manifest_answers_an_error() {
        let (s, v, _) = seeded();
        assert_eq!(v.route("GET", "/api/context", &[HOST, TOKEN]).status, 200);
        let home = s.home.path().to_owned();
        for name in ["knowledge.db-wal", "knowledge.db-shm"] {
            let _ = std::fs::remove_file(home.join(name));
        }
        std::fs::write(home.join("knowledge.db"), b"not a database, only text").unwrap();
        assert_eq!(v.route("GET", "/api/context", &[HOST, TOKEN]).status, 500);
    }

    /// D11: stats from raw.db, knowledge.db and providers.db; a leftover rebuild file alone is no
    /// rebuild.
    #[test]
    fn stats_read_raw_knowledge_and_providers() {
        let (s, v, _) = seeded();
        let st = get(&v, "/api/stats");
        assert_eq!(
            st["records"],
            json!([{"device": s.raw.device(), "records": 4}])
        );
        let claims = st["claims"].as_array().unwrap();
        let count = |kind: &str, status: &str| {
            claims
                .iter()
                .find(|c| c["kind"] == kind && c["status"] == status)
                .and_then(|c| c["count"].as_i64())
        };
        assert_eq!(
            (count("decision", "decided"), count("decision", "proposed")),
            (Some(2), Some(1))
        );
        assert_eq!(st["claim_skips"], json!([]));
        assert_eq!(
            (
                st["providers"][0]["provider"].as_str(),
                st["providers"][0]["ok"].as_i64(),
                st["providers"][0]["avg_ms"].as_i64()
            ),
            (Some("groq"), Some(2), Some(600))
        );
        assert_eq!(
            (
                st["providers"][1]["provider"].as_str(),
                st["providers"][1]["failed"].as_i64()
            ),
            (Some("mistral"), Some(1))
        );
        assert!(st["bytes"].as_u64().unwrap() > 0);
        assert_eq!(st["rebuilding"], false);
        std::fs::write(v.home.join("knowledge.db.rebuilding-1"), b"").unwrap();
        assert_eq!(get(&v, "/api/stats")["rebuilding"], false);
    }

    #[test]
    fn a_resident_worker_and_a_leftover_file_are_not_a_rebuild() {
        let (_stores, v, _) = seeded();
        let _worker = crate::worker::lock(&v.home).unwrap().unwrap();
        std::fs::write(v.home.join("knowledge.db.rebuilding-1"), b"leftover").unwrap();
        assert_eq!(get(&v, "/api/stats")["rebuilding"], false);
        let rebuild = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(v.home.join("state/rebuild.lock"))
            .unwrap();
        // Concurrent status readers are not the operation which owns this marker.
        rebuild.lock_shared().unwrap();
        assert_eq!(get(&v, "/api/stats")["rebuilding"], false);
        rebuild.unlock().unwrap();
        rebuild.lock().unwrap();
        assert_eq!(get(&v, "/api/stats")["rebuilding"], true);
        drop(rebuild);
        assert_eq!(get(&v, "/api/stats")["rebuilding"], false);
        assert!(v.home.join("state/rebuild.lock").exists());
    }

    /// D11: no route opens the old store: a junk oboete.db is left as it was.
    #[test]
    fn no_route_opens_the_old_store() {
        let (_s, v, x) = seeded();
        let junk = v.home.join("oboete.db");
        std::fs::write(&junk, b"not a database").unwrap();
        for route in [
            "/api/search?q=parser".to_owned(),
            format!("/api/doc?id={}", x.current),
            format!("/api/doc?id={}", x.imported),
            format!("/api/claim?id={}", x.old),
            "/api/timeline".into(),
            "/api/context".into(),
            "/api/repos".into(),
            "/api/version".into(),
            "/api/stats".into(),
        ] {
            assert_eq!(
                v.route("GET", &route, &[HOST, TOKEN]).status,
                200,
                "{route}"
            );
        }
        assert_eq!(std::fs::read(&junk).unwrap(), b"not a database");
        for side in ["oboete.db-wal", "oboete.db-shm", "oboete.db-journal"] {
            assert!(!v.home.join(side).exists(), "{side}");
        }
    }

    /// Spec 6.5: every text the viewer serves passes the outbound gate, an error's text included.
    #[test]
    fn every_text_the_viewer_serves_passes_the_outbound_gate() {
        let (mut s, v, x) = seeded();
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // split: scanners
        // A claim's quote must read in raw, so the token reaches the claim through the owner's
        // correction; and an imported document and a record carry it too.
        let uid = x.current.clone();
        correct(
            &mut s,
            &uid,
            None,
            Some(&format!("Deploy with token {token} today.")),
        );
        let seq = s.said(
            "s3",
            R,
            1_700_600_000_000,
            &format!("Deploy it with {token}"),
        );
        let record = s.key(seq);
        let imported = s.imported(
            "o2",
            "r",
            1_700_500_000_000,
            &format!("Token {token}"),
            &format!("Deploy notes {token}"),
        );
        s.run();
        for route in [
            "/api/search?q=Deploy".to_owned(),
            format!("/api/doc?id={uid}"),
            format!("/api/doc?id={imported}"),
            format!("/api/doc?id={record}"),
            format!("/api/claim?id={uid}"),
            "/api/timeline".into(),
            "/api/context".into(),
            "/api/repos".into(),
            "/api/stats".into(),
        ] {
            let r = v.route("GET", &route, &[HOST, TOKEN]);
            let body = String::from_utf8_lossy(&r.body).into_owned();
            assert_eq!(r.status, 200, "{route}: {body}");
            assert!(!body.contains(&token), "{route}: {body}");
        }
        assert_eq!(
            get(&v, "/api/search?q=Deploy")["hits"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let failed = failed(&anyhow!("could not read {token}"));
        assert_eq!(failed.status, 500);
        assert!(!String::from_utf8_lossy(&failed.body).contains(&token));
    }

    /// D11: a restore holding raw.db, or SQLite busy or locked, anywhere in the chain, is 503;
    /// anything else 500.
    #[test]
    fn an_error_chain_with_restoring_busy_or_locked_is_503() {
        let sqlite = |code| {
            anyhow::Error::new(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                None,
            ))
            .context("read the claims")
        };
        assert_eq!(
            failed(&anyhow::Error::new(crate::raw::Restoring).context("open")).status,
            503
        );
        assert_eq!(failed(&sqlite(rusqlite::ffi::SQLITE_BUSY)).status, 503);
        assert_eq!(failed(&sqlite(rusqlite::ffi::SQLITE_LOCKED)).status, 503);
        assert_eq!(failed(&sqlite(rusqlite::ffi::SQLITE_CORRUPT)).status, 500);
        assert_eq!(failed(&anyhow!("no such table")).status, 500);
    }

    /// Review Focus 6: a read waits for a restore's swap, and answers 503 when the swap holds
    /// raw.db past `raw::open`'s wait.
    #[test]
    fn a_read_waits_for_the_swap_and_answers_503_while_a_restore_holds_it() {
        let (s, v, _) = seeded();
        let _home = closed(s);
        let held = crate::raw::lock_for_swap(&v.home).unwrap();
        let r = v.route("GET", "/api/timeline", &[HOST, TOKEN]);
        assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body));
        // A swap that ends within the wait: the read waits for it and answers.
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let started = Instant::now();
        assert_eq!(v.route("GET", "/api/timeline", &[HOST, TOKEN]).status, 200);
        assert!(started.elapsed() >= Duration::from_millis(250));
        releaser.join().unwrap();
    }

    /// Review Focus 6: during a rebuild a read answers 200 or 503 (a claim may be 404 while its
    /// table is made again), and the same hits once the rebuild is done.
    #[test]
    fn a_read_during_a_rebuild_answers_200_or_503_and_the_same_hits_after() {
        let (s, v, x) = seeded();
        let _home = closed(s);
        let before = keys(&get(&v, "/api/search?q=parser"), "hits");
        let home = v.home.clone();
        let rebuild = std::thread::spawn(move || crate::worker::rebuild(&home));
        let claim = format!("/api/claim?id={}", x.current);
        while !rebuild.is_finished() {
            let s = v
                .route("GET", "/api/search?q=parser", &[HOST, TOKEN])
                .status;
            assert!(s == 200 || s == 503, "{s}");
            let c = v.route("GET", &claim, &[HOST, TOKEN]).status;
            assert!(c == 200 || c == 503 || c == 404, "{c}");
        }
        rebuild.join().unwrap().unwrap();
        assert_eq!(keys(&get(&v, "/api/search?q=parser"), "hits"), before);
    }

    /// D11: store text uses text nodes; page writes use fixed POST operations.
    #[test]
    fn the_page_puts_store_text_into_text_nodes_only() {
        for sink in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "DOMParser",
            "createContextualFragment",
            "srcdoc",
            "setHTML",
        ] {
            assert!(!APP_JS.contains(sink), "{sink}");
        }
        let methods = regex::Regex::new(r"\bmethod\s*:\s*([^,}\r\n]+)").unwrap();
        let fixed_methods = |js: &str| {
            methods
                .captures_iter(js)
                .all(|found| matches!(found[1].trim(), "'POST'" | "\"POST\"" | "'GET'" | "\"GET\""))
        };
        assert!(
            fixed_methods(APP_JS),
            "page request uses an unsupported or dynamic method"
        );
        assert!(!fixed_methods("fetch('/api/x', {method: 'DELETE'})"));
        assert!(!fixed_methods("fetch('/api/x', {method: chosenMethod})"));
    }

    /// #269: `--open` hands the opener an owner-only page that sends the browser on to the
    /// address, and the first request with the token removes it; one without leaves it.
    #[test]
    fn the_opener_page_carries_the_token_and_goes_once_the_browser_has_it() {
        let (dir, v) = viewer("opener");
        let url = "http://127.0.0.1:4321/#t=t0k";
        let page = opener_page(&dir, 4321, url).unwrap();
        let text = std::fs::read_to_string(&page).unwrap();
        assert!(text.contains(&format!("content=\"0;url={url}\"")), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&page).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Written again over one a stopped viewer of this port left; another port's is its own.
        assert_eq!(opener_page(&dir, 4321, url).unwrap(), page);
        let other = opener_page(&dir, 4322, "http://127.0.0.1:4322/#t=x").unwrap();
        *v.opener.lock().unwrap() = Some(page.clone());
        assert_eq!(v.route("GET", "/api/repos", &[HOST]).status, 401);
        assert!(page.exists());
        assert_eq!(v.route("GET", "/api/repos", &[HOST, TOKEN]).status, 200);
        assert!(!page.exists() && other.exists());
        // A settings save that is the first request with the token removes it too.
        let page = opener_page(&dir, 4321, url).unwrap();
        *v.opener.lock().unwrap() = Some(page.clone());
        assert!(v.save_gate(&[HOST, TOKEN], MAX_BODY).is_err());
        assert!(!page.exists());
        std::fs::remove_file(other).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #269: `--open` registers the page before it starts the opener, which gets the page's path;
    /// a page it cannot write starts nothing. Under WSL the opener gets the Windows path.
    #[test]
    fn open_registers_the_page_then_launches_the_opener_with_its_path() {
        let (dir, mut v) = viewer("open");
        // Its own port: on Windows every test's page is in the one %LOCALAPPDATA%.
        v.port = 4323.into();
        let mut launched = None;
        v.open(&dir, "http://127.0.0.1:4323/#t=t0k", |p| {
            assert_eq!(v.opener.lock().unwrap().as_deref(), Some(p));
            launched = Some(p.to_owned());
        });
        let page = launched.unwrap();
        assert!(page.exists() && page.is_absolute() && page.starts_with(opener_dir(&dir)));
        assert_eq!(opener_arg(&page, false), page.as_os_str());
        let wsl = opener_arg(&page, true);
        assert!(
            wsl == page.as_os_str() || wsl.to_string_lossy().starts_with(r"\\"),
            "{wsl:?}"
        );
        std::fs::remove_file(&page).unwrap();
        // A home it cannot write the page in (off Windows, where the page is in the home).
        #[cfg(not(windows))]
        {
            v.opener.lock().unwrap().take();
            v.open(&dir.join("missing"), "http://127.0.0.1:4323/#t=t0k", |_| {
                panic!("launched")
            });
            assert!(v.opener.lock().unwrap().is_none());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// What `serve` does with a request, without a socket: a save's body is `body`.
    fn request(
        v: &Viewer,
        method: &str,
        target: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Response {
        match v.head(method, target, headers) {
            Head::Answer(r) => r,
            Head::Body(len, save, given) => {
                assert_eq!(len, body.len());
                v.take(save, body, &given)
            }
        }
    }

    /// A save of the settings `shown`, with injection at session start turned off.
    fn save_body(shown: &Value) -> Vec<u8> {
        let chain: Vec<Value> = (shown["chain"].as_array().unwrap().iter())
            .map(|e| {
                json!({"name": e["name"], "on": e["on"], "daily_budget": e["daily_budget"],
                    "timeout_s": e["timeout_s"], "model": e["model"]})
            })
            .collect();
        serde_json::to_vec(&json!({"version": shown["version"],
            "inject": {"session_start": false, "session_start_chars": 6000, "per_prompt": false,
                "per_prompt_chars": 1500, "correction": true, "correction_chars": 800},
            "summary": shown["summary"], "paid_usd_per_month": shown["paid_usd_per_month"],
            "gemini": shown["gemini"], "capture": shown["capture"], "chain": chain}))
        .unwrap()
    }

    #[test]
    fn resume_passes_the_save_guards_and_only_clears_the_stop() {
        let (dir, v) = viewer("w1-resume");
        let config = "providers = []\n[summary]\ncurate = false # invented settings\n";
        std::fs::write(dir.join("config.toml"), config).unwrap();
        let db = crate::providers_db::open(&dir).unwrap();
        for provider in ["owner-stopped", "other-stopped"] {
            crate::providers_db::set_state(
                &db,
                provider,
                crate::providers_db::State {
                    down_until: crate::providers_db::OWNER_HOLD,
                    fails: 3,
                    backoff: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        for (device, hold) in [("owner-device", "owner"), ("time-device", "time")] {
            crate::providers_db::set_pending(
                &db,
                &crate::providers_db::Pending {
                    device: device.into(),
                    from_seq: 1,
                    from_offset: None,
                    to_seq: 2,
                    to_offset: None,
                    reason: "invented-reason".into(),
                    hold: hold.into(),
                    attempts: 2,
                    next_attempt_at: 5_000_000_000_000,
                    since: 1,
                    prompt: "invented-hash".into(),
                },
            )
            .unwrap();
        }
        let calls = crate::providers_db::last_calls(&db, 10).unwrap();
        let body = br#"{"provider":"owner-stopped"}"#;
        save_guards(&v, "/api/resume", MAX_BODY, body);
        for method in ["GET", "HEAD", "PUT", "PATCH", "OPTIONS", "DELETE"] {
            let r = request(&v, method, "/api/resume", &[HOST, TOKEN], b"");
            assert_eq!(
                r.status,
                if matches!(method, "GET" | "HEAD") {
                    404
                } else {
                    405
                }
            );
        }
        let send = |target: &str, body: &[u8]| {
            let len = body.len().to_string();
            request(
                &v,
                "POST",
                target,
                &[
                    HOST,
                    TOKEN,
                    ("Origin", "http://127.0.0.1:4321"),
                    ("Content-Type", "application/json"),
                    ("Content-Length", &len),
                ],
                body,
            )
        };
        assert_eq!(send("/api/resume?x=1", body).status, 405);
        for invalid in [
            br#"{"provider":true}"#.as_slice(),
            br#"{"provider":"owner-stopped","command":"anything"}"#,
            br#"{}"#,
            b"not JSON",
        ] {
            assert_eq!(send("/api/resume", invalid).status, 400);
        }
        assert_eq!(send("/api/resume", br#"{"provider":""}"#).status, 422);
        let shown = get(&v, "/api/settings");
        assert_eq!(shown["stopped"], json!(["other-stopped", "owner-stopped"]));
        assert_eq!(
            crate::providers_db::state(&db, "owner-stopped")
                .unwrap()
                .down_until,
            crate::providers_db::OWNER_HOLD
        );
        let resumed = json_of(&send("/api/resume", body));
        assert_eq!(
            resumed,
            json!({"provider": "owner-stopped", "resumed": true})
        );
        assert_eq!(
            crate::providers_db::state(&db, "owner-stopped").unwrap(),
            crate::providers_db::State::default()
        );
        assert_eq!(
            get(&v, "/api/settings")["stopped"],
            json!(["other-stopped"])
        );
        assert_eq!(
            crate::providers_db::pending_of(&db, "owner-device")
                .unwrap()
                .unwrap()
                .next_attempt_at,
            0
        );
        assert_eq!(
            crate::providers_db::pending_of(&db, "time-device")
                .unwrap()
                .unwrap()
                .next_attempt_at,
            5_000_000_000_000
        );
        assert_eq!(json_of(&send("/api/resume", body))["resumed"], false);
        assert_eq!(crate::providers_db::last_calls(&db, 10).unwrap(), calls);
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml")).unwrap(),
            config
        );
        assert!(!dir.join("raw.db").exists() && !dir.join("knowledge.db").exists());
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// #94 test 3: the two saves are the requests with a body, and a head passes every check
    /// (spec 6.6) before a byte of its body is read; a key save's cap is 1 KiB (part 3).
    #[test]
    fn a_settings_save_passes_every_guard_first() {
        let (dir, v) = viewer("save-guards");
        let origin = ("Origin", "http://127.0.0.1:4321");
        let json_type = ("Content-Type", "application/json");
        assert_eq!(v.route("GET", "/api/settings", &[HOST]).status, 401);
        let shown = json_of(&v.route("GET", "/api/settings", &[HOST, TOKEN]));
        let body = save_body(&shown);
        let len = body.len().to_string();
        let cl = ("Content-Length", len.as_str());
        for (path, cap) in [
            ("/api/settings", MAX_BODY),
            ("/api/settings/recovery/preview", MAX_BODY),
            ("/api/settings/recovery/start", MAX_BODY),
            ("/api/key", MAX_KEY_BODY),
            ("/api/providers", MAX_BODY),
            ("/api/providers/key", MAX_KEY_BODY),
            ("/api/providers/test/preview", MAX_BODY),
            ("/api/providers/test", MAX_BODY),
        ] {
            save_guards(&v, path, cap, &body);
        }
        // Every other method on it, and a POST anywhere else, stay 405.
        for m in ["PUT", "PATCH", "OPTIONS", "DELETE"] {
            let r = request(&v, m, "/api/settings", &[HOST, TOKEN, origin], b"");
            assert_eq!(r.status, 405, "{m}");
        }
        for t in [
            "/api/repos",
            "/api/settings?x=1",
            "/api/settings/recovery/preview?x=1",
            "/api/settings/recovery/start?x=1",
            "/api/key?x=1",
            "/api/providers?x=1",
            "/api/providers/key?x=1",
            "/api/providers/test/preview?x=1",
            "/api/providers/test?x=1",
            "/api/doc?id=o1",
        ] {
            let r = request(&v, "POST", t, &[HOST, TOKEN, origin, json_type, cl], &body);
            assert_eq!(r.status, 405, "{t}");
        }
        assert!(!dir.join("config.toml").exists());
        let saved = request(
            &v,
            "POST",
            "/api/settings",
            &[
                HOST,
                TOKEN,
                origin,
                ("Content-Type", "Application/JSON; charset=utf-8"),
                cl,
            ],
            &body,
        );
        assert_eq!(json_of(&saved)["inject"]["session_start"], false);
        assert!(!crate::config::inject(&dir).unwrap().session_start);
        // The same body again names the file as it was: refused, and nothing echoes an error.
        let stale = request(
            &v,
            "POST",
            "/api/settings",
            &[HOST, TOKEN, origin, json_type, cl],
            &body,
        );
        assert_eq!(stale.status, 409);
        assert_eq!(
            serde_json::from_slice::<Value>(&stale.body).unwrap(),
            json!({"code": "stale", "field": ""})
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Every head check of a save at `path`, whose body may be at most `cap` bytes.
    fn save_guards(v: &Viewer, path: &str, cap: usize, body: &[u8]) {
        let origin = ("Origin", "http://127.0.0.1:4321");
        let json_type = ("Content-Type", "application/json");
        let len = body.len().to_string();
        let cl = ("Content-Length", len.as_str());
        let status = |h: &[(&str, &str)]| request(v, "POST", path, h, body).status;
        assert_eq!(status(&[HOST, origin, json_type, cl]), 401);
        assert_eq!(
            status(&[("Host", "evil.example:4321"), TOKEN, origin, json_type, cl]),
            403
        );
        for bad in [
            None,
            Some("null"),
            Some("http://evil.example:4321"),
            Some("https://127.0.0.1:4321"),
            Some("http://127.0.0.1:4322"),
        ] {
            let mut h = vec![HOST, TOKEN, json_type, cl];
            h.extend(bad.map(|o| ("Origin", o)));
            assert_eq!(status(&h), 403, "{bad:?}");
        }
        // Two of a header a browser sends once (Codex on #94).
        assert_eq!(status(&[HOST, TOKEN, origin, origin, json_type, cl]), 403);
        assert_eq!(status(&[HOST, HOST, TOKEN, origin, json_type, cl]), 403);
        for t in ["text/plain", "application/jsonp", "application/json-seq"] {
            let h = [HOST, TOKEN, origin, ("Content-Type", t), cl];
            assert_eq!(status(&h), 400, "{t}");
        }
        assert_eq!(
            status(&[HOST, TOKEN, origin, json_type, json_type, cl]),
            400
        );
        assert_eq!(status(&[HOST, TOKEN, origin, json_type]), 400);
        assert_eq!(status(&[HOST, TOKEN, origin, json_type, cl, cl]), 400);
        assert_eq!(
            status(&[
                HOST,
                TOKEN,
                origin,
                json_type,
                cl,
                ("Transfer-Encoding", "chunked")
            ]),
            400
        );
        for l in ["+1", " 1", "1x", ""] {
            let h = [HOST, TOKEN, origin, json_type, ("Content-Length", l)];
            assert_eq!(status(&h), 400, "{l:?}");
        }
        let at_cap = cap.to_string();
        let at_cap = [
            HOST,
            TOKEN,
            origin,
            json_type,
            ("Content-Length", at_cap.as_str()),
        ];
        assert!(matches!(v.head("POST", path, &at_cap), Head::Body(l, ..) if l == cap));
        let over = (cap + 1).to_string();
        let over = [
            HOST,
            TOKEN,
            origin,
            json_type,
            ("Content-Length", over.as_str()),
        ];
        match v.head("POST", path, &over) {
            Head::Answer(r) => assert_eq!(r.status, 413, "{path}"),
            Head::Body(..) => panic!("a body over {path}'s cap would be read"),
        }
    }

    /// #94 part 3: a key save through the viewer reaches its file, and neither its answer, a
    /// refusal, nor the settings shown after it hold the key.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_key_save_answers_without_the_key() {
        let (dir, v) = viewer("key-save");
        let keys = tempfile::tempdir().unwrap();
        let file = keys.path().join("LOCAL_KEY.md");
        let config = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\n\
             base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\nkey_file = {:?}\n",
            file.display().to_string()
        );
        std::fs::write(dir.join("config.toml"), config).unwrap();
        let shown = json_of(&v.route("GET", "/api/settings", &[HOST, TOKEN]));
        let canary = format!("{}-{}", "canary", "5f2c9a17");
        let post = |key: &str| {
            let body = json!({"entry": "local", "key": key, "version": shown["version"]});
            let body = serde_json::to_vec(&body).unwrap();
            let len = body.len().to_string();
            let origin = ("Origin", "http://127.0.0.1:4321");
            let json_type = ("Content-Type", "application/json");
            let h = [
                HOST,
                TOKEN,
                origin,
                json_type,
                ("Content-Length", len.as_str()),
            ];
            request(&v, "POST", "/api/key", &h, &body)
        };
        let saved = post(&canary);
        assert_eq!(
            json_of(&saved),
            json!({"entry": "local", "key": "ok", "durable": true})
        );
        assert_eq!(crate::config::read_key(&file).unwrap(), canary);
        let refused = post(&format!("{canary} x"));
        assert_eq!(
            (
                refused.status,
                serde_json::from_slice::<Value>(&refused.body).unwrap()
            ),
            (422, json!({"code": "bad_key", "field": "chain.local.key"}))
        );
        let after = v.route("GET", "/api/settings", &[HOST, TOKEN]);
        assert!(!String::from_utf8_lossy(&after.body).contains("canary"));
        assert_eq!(json_of(&after)["chain"][0]["key"], "ok");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Over a socket: a head declaring too large a body is answered at once, with the body never
    /// sent, and a save whose body comes in pieces is read whole.
    #[test]
    fn a_save_is_read_only_after_its_head_passes() {
        let (dir, mut v) = viewer("save-socket");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        v.port = listener.local_addr().unwrap().port().into();
        let port = v.port();
        let home = dir.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (s, _) = listener.accept().unwrap();
                v.serve(s);
            }
        });
        let head = |len: usize| {
            format!(
                "POST /api/settings HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: t0k\r\n\
                 Origin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
                 Content-Length: {len}\r\n\r\n"
            )
        };
        let started = Instant::now();
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(head(20_000).as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(
            out.starts_with("HTTP/1.1 413 Content Too Large\r\n"),
            "{out}"
        );
        assert!(started.elapsed() < REQUEST_TIME);
        let body = save_body(&crate::settings::show(&home));
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(head(body.len()).as_bytes()).unwrap();
        c.write_all(&body[..10]).unwrap();
        c.flush().unwrap();
        c.write_all(&body[10..]).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
        assert!(!crate::config::inject(&home).unwrap().session_start);
        server.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A head over `MAX_HEAD` is refused even when it completes in the read that crosses the cap
    /// (OpenCodeReview on #270; #53).
    #[test]
    fn a_head_over_the_cap_is_refused_however_its_reads_fall() {
        let (dir, mut v) = viewer("head-cap");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        v.port = listener.local_addr().unwrap().port().into();
        let port = v.port();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            v.serve(s);
        });
        let start = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ");
        let first = format!("{start}{}", "a".repeat(MAX_HEAD - 1 - start.len()));
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(first.as_bytes()).unwrap();
        c.flush().unwrap();
        // One byte under the cap is read and still partial; the rest completes the head past it.
        std::thread::sleep(Duration::from_millis(200));
        c.write_all(format!("{}\r\n\r\n", "a".repeat(1000)).as_bytes())
            .unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(
            out.starts_with("HTTP/1.1 400 "),
            "{}",
            &out[..out.len().min(80)]
        );
        server.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #53: connections past `MAX_CONNECTIONS` are closed unread, idle ones before the token
    /// included, and the viewer answers again once they go.
    #[test]
    fn connections_past_the_cap_are_closed_and_their_slots_come_back() {
        let (dir, mut v) = viewer("conn-cap");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        v.port = listener.local_addr().unwrap().port().into();
        let port = v.port();
        let v = Arc::new(v);
        let server = Arc::clone(&v);
        std::thread::spawn(move || accept(listener, &server));
        let until = |held: usize| {
            let started = Instant::now();
            while v.live.load(Ordering::SeqCst) != held {
                assert!(started.elapsed() < Duration::from_secs(2), "{held}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let get = || {
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let head = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
            let _ = c.write_all(head.as_bytes());
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out);
            String::from_utf8_lossy(&out).into_owned()
        };
        let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS)
            .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect();
        until(MAX_CONNECTIONS);
        assert_eq!(get(), "");
        drop(idle);
        until(0);
        assert!(get().starts_with("HTTP/1.1 200 "));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #53: an answer to a client that stops reading is given up at its deadline; one that reads
    /// gets all of it.
    #[test]
    fn an_answer_nobody_reads_is_given_up_at_its_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stalled = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (mut s, _) = listener.accept().unwrap();
        let started = Instant::now();
        // Far more than the two sockets' buffers hold. Windows takes a whole write while its
        // buffer has room, so there the second one is what waits.
        let big = vec![b'a'; 64 << 20];
        let within = Duration::from_millis(300);
        assert!(!(send(&mut s, &big, within) && send(&mut s, &big, within)));
        assert!(started.elapsed() < Duration::from_secs(3));
        drop((s, stalled));
        let mut reader = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (mut s, _) = listener.accept().unwrap();
        let read = std::thread::spawn(move || {
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            out.len()
        });
        assert!(send(&mut s, &vec![b'a'; 8 << 20], Duration::from_secs(10)));
        drop(s);
        assert_eq!(read.join().unwrap(), 8 << 20);
    }

    /// #283: a signal that interrupts the read of a request or the write of an answer drops
    /// neither. Linux fails a socket call that has a timeout with EINTR when a handler runs (and
    /// after a stop and continue with none); here SIGUSR1, with a handler that does nothing, is
    /// sent to the one thread.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_interrupted_read_or_write_goes_on() {
        extern "C" fn nothing(_: libc::c_int) {}
        // SAFETY: a zeroed `sigaction` with a handler that does nothing and no SA_RESTART, for a
        // signal no other test sends.
        unsafe {
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = nothing as extern "C" fn(libc::c_int) as libc::sighandler_t;
            assert_eq!(
                libc::sigaction(libc::SIGUSR1, &act, std::ptr::null_mut()),
                0
            );
        }
        use std::os::unix::thread::JoinHandleExt;
        let poke = |thread: libc::pthread_t| {
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(50));
                // SAFETY: the thread is not joined yet, so its id is still valid.
                unsafe { libc::pthread_kill(thread, libc::SIGUSR1) };
            }
        };
        let (dir, mut v) = viewer("interrupted");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        v.port = listener.local_addr().unwrap().port().into();
        let port = v.port();
        // The request's read, interrupted before the client sends it.
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (s, _) = listener.accept().unwrap();
        let server = std::thread::spawn(move || v.serve(s));
        poke(server.as_pthread_t());
        write!(c, "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(
            out.starts_with("HTTP/1.1 200 "),
            "{}",
            &out[..out.len().min(60)]
        );
        server.join().unwrap();
        // The answer's write, interrupted while the client does not read yet.
        let mut reader = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (mut s, _) = listener.accept().unwrap();
        let writer = std::thread::spawn(move || {
            send(&mut s, &vec![b'a'; 64 << 20], Duration::from_secs(10))
        });
        poke(writer.as_pthread_t());
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        assert_eq!(got.len(), 64 << 20);
        assert!(writer.join().unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A resident viewer of `home` on `port` (docs/resident.md R8): no checkout, the token in its
    /// file.
    fn resident_of(home: &Path, port: u16) -> Viewer {
        Viewer::new(home, None, port, Token::File)
    }

    /// A port nothing listens on now.
    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A resident viewer listening in a new home, on a port `free_port` gave: a test running
    /// alongside may take that port before `listen` binds it, so a port in use is tried again
    /// with another.
    #[cfg(target_os = "linux")]
    fn listening() -> (u16, tempfile::TempDir, Resident) {
        for _ in 0..5 {
            let port = free_port();
            let home = resident_home(port);
            if let Some(started) = listen(home.path()).unwrap() {
                return (port, home, started);
            }
        }
        panic!("no port stayed free in five tries");
    }

    /// A home whose config.toml names `port` as the resident viewer's.
    fn resident_home(port: u16) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!("[view]\nport = {port}\n"),
        )
        .unwrap();
        home
    }

    fn view_outcome(home: &Path) -> String {
        std::fs::read_to_string(home.join("state/view-outcome")).unwrap()
    }

    /// Resident test 4 (R6): the token is the file's, read for each request, and the file is 0600
    /// when made. A file readable by group or others, one on a filesystem that does not enforce
    /// modes, a missing, an empty, a 31-character and an upper-case file each refuse every
    /// request, with and without a token header.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_viewers_token_is_its_files_and_any_other_file_refuses_every_request() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        ensure_token(p).unwrap();
        let file = p.join("state/view-token");
        let mode = |m| std::fs::set_permissions(&file, std::fs::Permissions::from_mode(m)).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let token = std::fs::read_to_string(&file).unwrap();
        let v = resident_of(p, 4321);
        let status = |given: Option<&str>| {
            let mut h = vec![HOST];
            h.extend(given.map(|t| ("X-Oboete-Token", t)));
            v.route("GET", "/api/repos", &h).status
        };
        assert_eq!(status(Some(&token)), 200);
        for given in [None, Some(""), Some("t0k")] {
            assert_eq!(status(given), 401, "{given:?}");
        }
        for m in [0o640, 0o604] {
            mode(m);
            assert_eq!((status(Some(&token)), status(None)), (401, 401), "{m:o}");
        }
        mode(0o600);
        assert_eq!(status(Some(&token)), 200);
        crate::keyfile::fake_fs(Some(0x6969));
        let nfs = status(Some(&token));
        crate::keyfile::fake_fs(None);
        assert_eq!(nfs, 401);
        let upper = token.to_uppercase();
        for text in ["", &token[..31], upper.as_str()] {
            std::fs::write(&file, text).unwrap();
            for given in [Some(token.as_str()), Some(text), None] {
                assert_eq!(status(given), 401, "{text:?} {given:?}");
            }
        }
        std::fs::remove_file(&file).unwrap();
        assert_eq!((status(Some(&token)), status(None)), (401, 401));
    }

    /// R6: a start keeps a token file of its shape, so a bookmark outlives a restart, and replaces
    /// one that is not (a token others could read is a new one).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_start_keeps_a_good_token_file_and_replaces_any_other() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let file = p.join("state/view-token");
        let read = || std::fs::read_to_string(&file).unwrap();
        ensure_token(p).unwrap();
        let first = read();
        assert!(first.len() == 32 && first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(first, first.to_lowercase());
        ensure_token(p).unwrap();
        assert_eq!(read(), first);
        std::fs::write(&file, "x").unwrap();
        ensure_token(p).unwrap();
        let second = read();
        assert!(second.len() == 32 && second != first);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        ensure_token(p).unwrap();
        assert_ne!(read(), second);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let staged = std::fs::read_dir(p.join("state"))
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".tmp"))
            .count();
        assert_eq!(staged, 0);
    }

    /// Resident tests 3 and 5 (R5): the viewer takes its lock, binds the configured port, says
    /// `listening <port>`, and answers there with the file's token; a second start finds the lock
    /// held and writes nothing; after a restart the same token works. Another home's viewer on
    /// the same port says the port is in use and serves nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_viewer_listens_on_its_port_and_says_so() {
        let (port, home, started) = listening();
        let p = home.path();
        assert_eq!(view_outcome(p), format!("listening {port}"));
        std::fs::write(p.join("state/view-outcome"), "mark").unwrap();
        assert!(listen(p).unwrap().is_none());
        assert_eq!(view_outcome(p), "mark");
        let token = std::fs::read_to_string(p.join("state/view-token")).unwrap();
        let Resident {
            lock,
            listener,
            viewer,
        } = started;
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            viewer.serve(s);
            listener
        });
        let answer = ask(
            port,
            format!(
                "GET /api/repos HTTP/1.1\r\nHost: localhost:{port}\r\nX-Oboete-Token: {token}\r\n\r\n"
            )
            .as_bytes(),
        );
        let listener = server.join().unwrap();
        assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
        let other = resident_home(port);
        assert!(listen(other.path()).unwrap().is_none());
        assert_eq!(view_outcome(other.path()), "port in use");
        let staged = std::fs::read_dir(other.path().join("state"))
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .any(|n| n.ends_with(".tmp"));
        assert!(!staged);
        drop((lock, listener));
        let again = listen(p).unwrap().unwrap();
        assert_eq!(view_outcome(p), format!("listening {port}"));
        assert_eq!(
            std::fs::read_to_string(p.join("state/view-token")).unwrap(),
            token
        );
        drop(again);
    }

    /// Resident test 6 (R6): on a filesystem that does not enforce modes no resident viewer
    /// starts: no token file, no listener, and the outcome says why.
    #[cfg(target_os = "linux")]
    #[test]
    fn no_resident_viewer_starts_where_files_cannot_be_the_owners_alone() {
        let port = free_port();
        let home = resident_home(port);
        let p = home.path();
        crate::keyfile::fake_fs(Some(0x6969));
        let started = listen(p);
        crate::keyfile::fake_fs(None);
        assert!(started.unwrap().is_none());
        assert!(!p.join("state/view-token").exists());
        assert_eq!(view_outcome(p), NOT_PRIVATE);
        TcpListener::bind(("127.0.0.1", port)).unwrap();
    }

    /// CodeRabbit on #376 (R6): a `state` folder another user could write into is made the owner's
    /// alone first, as the stores make the home; one that is a link is refused, and nothing is
    /// written through it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_viewer_keeps_its_state_to_its_owner_and_follows_no_link() {
        use std::os::unix::fs::PermissionsExt;
        let home = resident_home(free_port());
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let mode = |q: &Path| std::fs::metadata(q).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(p.join("state"), std::fs::Permissions::from_mode(0o777)).unwrap();
        // A link planted while another user could write there is not written through (Codex on
        // #376).
        let theirs = tempfile::tempdir().unwrap();
        let victim = theirs.path().join("victim");
        std::fs::write(&victim, "theirs").unwrap();
        std::os::unix::fs::symlink(&victim, p.join("state/view-outcome.next")).unwrap();
        // Nor is a FIFO planted as the lock waited on (Codex on #376): it is replaced.
        let fifo = std::ffi::CString::new(p.join("state/view.lock").to_str().unwrap()).unwrap();
        // SAFETY: a valid, NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let started = listen(p).unwrap();
        assert!(started.is_some());
        assert!(
            std::fs::symlink_metadata(p.join("state/view.lock"))
                .unwrap()
                .is_file()
        );
        // Nor do folders planted at the fixed names stop the start (Codex on #376).
        let folders = resident_home(free_port());
        let f = folders.path();
        for name in ["view.lock", "view-outcome.next", "view-token.tmp"] {
            std::fs::create_dir_all(f.join("state").join(name).join("inner")).unwrap();
        }
        assert!(listen(f).unwrap().is_some());
        assert!(
            std::fs::symlink_metadata(f.join("state/view.lock"))
                .unwrap()
                .is_file()
        );
        assert_eq!(mode(&p.join("state")), 0o700);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "theirs");
        // Nothing goes through a link, not even a change of the mode of what it points to.
        let elsewhere = tempfile::tempdir().unwrap();
        let mode_of = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(elsewhere.path(), mode_of).unwrap();
        let other = resident_home(free_port());
        std::os::unix::fs::symlink(elsewhere.path(), other.path().join("state")).unwrap();
        assert!(listen(other.path()).unwrap().is_none());
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
        assert_eq!(mode(elsewhere.path()), 0o755);
    }

    /// Codex on #376: a lock made under a umask that took the owner's write away is given it
    /// back and kept, not replaced, as a viewer may hold it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_held_lock_the_owner_cannot_write_is_kept() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let held = std::fs::File::create(state.join("view.lock")).unwrap();
        held.try_lock().unwrap();
        let mode = std::fs::Permissions::from_mode(0o466);
        std::fs::set_permissions(state.join("view.lock"), mode).unwrap();
        let again = view_lock(&state).unwrap();
        assert_eq!(
            again.metadata().unwrap().ino(),
            held.metadata().unwrap().ino()
        );
        assert!(matches!(
            again.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
    }

    /// Resident test 7: the Host check holds to the configured port.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_viewers_host_is_its_ports() {
        let home = tempfile::tempdir().unwrap();
        let v = resident_of(home.path(), 17373);
        for (host, want) in [
            ("127.0.0.1:17373", 200),
            ("localhost:17373", 200),
            ("127.0.0.1:17374", 403),
            ("localhost", 403),
            ("evil.example:17373", 403),
        ] {
            assert_eq!(
                v.route("GET", "/", &[("Host", host)]).status,
                want,
                "{host}"
            );
        }
    }

    /// Resident test 3 (R7): the first request with the token removes the opener page of the
    /// viewer's port, which `oboete view --open` wrote; another port's stays.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_viewer_removes_its_ports_opener_page_once_the_token_arrives() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        ensure_token(p).unwrap();
        let token = std::fs::read_to_string(p.join("state/view-token")).unwrap();
        let v = resident_of(p, 4321);
        let page = opener_page(p, 4321, "http://127.0.0.1:4321/").unwrap();
        let other = opener_page(p, 4322, "http://127.0.0.1:4322/").unwrap();
        assert_eq!(v.route("GET", "/api/repos", &[HOST]).status, 401);
        assert!(page.exists());
        let with = [HOST, ("X-Oboete-Token", token.as_str())];
        assert_eq!(v.route("GET", "/api/repos", &with).status, 200);
        assert!(!page.exists() && other.exists());
    }

    /// Resident test 8 (R8): with no checkout, `/api/repos` names none, the timeline and a search
    /// cover every repository, and `/api/context` with no repository asks for one.
    #[test]
    fn a_resident_viewer_has_no_checkout_and_every_repository_is_its_scope() {
        let (mut s, foreground, x) = seeded();
        let other = s.decided(
            "github.com/o/q",
            1_700_400_000_000,
            "Parser output goes to a log file.",
            &[],
        );
        s.run();
        let v = Viewer {
            token: Token::Run("t0k".into()),
            ..resident_of(s.home.path(), 4321)
        };
        let repos = get(&v, "/api/repos");
        assert_eq!(
            (&repos["current"], &repos["branch"]),
            (&json!(""), &json!(""))
        );
        let has = |answer: &Value, field: &str, key: &str| {
            answer[field]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["key"] == key)
        };
        let timeline = get(&v, "/api/timeline");
        assert!(has(&timeline, "items", &other) && has(&timeline, "items", &x.current));
        assert!(!has(&get(&foreground, "/api/timeline"), "items", &other));
        let found = get(&v, "/api/search?q=parser&raw=off");
        assert!(has(&found, "hits", &other) && has(&found, "hits", &x.current));
        let context = get(&v, "/api/context");
        assert_eq!(context["choose"], true);
        assert_eq!(
            (&context["text"], &context["repo"]),
            (&json!(""), &json!(""))
        );
        let chosen = get(&v, &format!("/api/context?repo={R}"));
        assert_eq!(chosen["repo"], R);
        assert!(chosen.get("choose").is_none());
    }

    /// R8 with row 30-2: a search of every repository, which a viewer with no checkout makes when
    /// it is given none, keeps its text from the embedder while a repository is excluded.
    #[test]
    fn a_resident_viewers_search_of_every_repository_keeps_its_text_from_the_embedder() {
        let stub = crate::embed::stub::Stub::start();
        let mut s = Store::new();
        crate::embed_phase::fixture::config(&s, &stub);
        s.said("s", R, 1_000, "Open words.");
        s.run();
        crate::embed_phase::fixture::embed_all(&s);
        s.raw.exclude("github.com/o/secret", false).unwrap();
        let sent = stub.requests();
        let v = Viewer {
            token: Token::Run("t0k".into()),
            ..resident_of(s.home.path(), 4321)
        };
        let found = get(&v, "/api/search?q=Open+words");
        assert_eq!(found["vector"], "excluded", "{found}");
        assert_eq!(stub.requests(), sent);
    }

    /// A starter whose viewer is a short `sleep`: how many it started, and the starter.
    #[cfg(target_os = "linux")]
    fn sleeper(every: Duration, after_busy: Duration) -> (Arc<AtomicUsize>, Starter) {
        let started = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&started);
        let spawn = Box::new(move |_: &Path| {
            count.fetch_add(1, Ordering::SeqCst);
            std::process::Command::new("sleep").arg("0.3").spawn().ok()
        });
        (started, Starter::with(every, after_busy, spawn))
    }

    /// Codex on #378: the outcome a viewer that is gone left is taken away before another is
    /// started, so no one reads it as the new one's.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_start_takes_the_last_viewers_outcome_away() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        say(p, "listening 17373").unwrap();
        let (started, mut starter) = sleeper(Duration::ZERO, Duration::from_secs(600));
        starter.due(p);
        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(outcome(p), None);
    }

    /// Resident tests 2 and 5 (R4): the starter starts a viewer when `state/view.lock` is free,
    /// none while the one it started runs or another holds the lock, and reaps each that left
    /// before it starts another, so none is left a zombie.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_starter_starts_a_viewer_only_when_its_lock_is_free_and_reaps_the_one_that_left() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let (started, mut starter) = sleeper(Duration::ZERO, Duration::from_secs(600));
        starter.due(p);
        assert_eq!(started.load(Ordering::SeqCst), 1);
        // Its own still runs: none other.
        starter.due(p);
        assert_eq!(started.load(Ordering::SeqCst), 1);
        // Another holds the lock: once its own has left, it is reaped and none is started.
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(p.join("state/view.lock"))
            .unwrap();
        crate::worker::try_lock(&lock).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        starter.due(p);
        assert!(
            starter.child.is_none(),
            "the viewer that left was not reaped"
        );
        assert_eq!(started.load(Ordering::SeqCst), 1);
        drop(lock);
        // A child another test thread forked holds the lock file until it execs: looked at again
        // for a moment, as `worker::try_lock` waits under `cargo test`.
        let t = Instant::now();
        while started.load(Ordering::SeqCst) < 2 && t.elapsed() < Duration::from_secs(2) {
            starter.due(p);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(started.load(Ordering::SeqCst), 2);
    }

    /// Resident test 5 (R4): after an outcome of "port in use" the starter tries again only once
    /// its wait for that has passed, and it waits its minute between any two looks.
    #[cfg(target_os = "linux")]
    #[test]
    fn after_a_port_in_use_the_starter_waits_before_it_tries_again() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let (started, mut starter) = sleeper(Duration::ZERO, Duration::from_millis(800));
        starter.due(p);
        assert_eq!(started.load(Ordering::SeqCst), 1);
        say(p, PORT_IN_USE).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        starter.due(p);
        assert_eq!(started.load(Ordering::SeqCst), 1, "tried again at once");
        // A worker started anew waits too: the wait is the outcome's (Codex on #378).
        let (anew, mut fresh) = sleeper(Duration::ZERO, Duration::from_millis(800));
        fresh.due(p);
        assert_eq!(
            anew.load(Ordering::SeqCst),
            0,
            "a new starter tried at once"
        );
        std::thread::sleep(Duration::from_millis(500));
        starter.due(p);
        assert_eq!(started.load(Ordering::SeqCst), 2);
        // Between two looks, its minute.
        let (started, mut starter) = sleeper(Duration::from_secs(60), Duration::ZERO);
        starter.due(p);
        std::thread::sleep(Duration::from_millis(400));
        starter.due(p);
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    /// Resident test 6 (R4, R6): where the home's files cannot be its owner's alone the starter
    /// starts nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_starter_starts_nothing_where_files_cannot_be_the_owners_alone() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let (started, mut starter) = sleeper(Duration::ZERO, Duration::ZERO);
        crate::keyfile::fake_fs(Some(0x6969));
        starter.due(p);
        crate::keyfile::fake_fs(None);
        assert_eq!(started.load(Ordering::SeqCst), 0);
    }

    /// Resident test 14 and R4's tick: the viewer leaves when config.toml loads and does not say
    /// `resident = true` and no request came since the last look, when its home is gone, or when
    /// `[view] port` names another port; a file that does not load leaves it as it is.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_resident_viewer_leaves_when_its_home_says_so() {
        let port = free_port();
        let home = resident_home(port);
        let p = home.path();
        let config = |text: &str| std::fs::write(p.join("config.toml"), text).unwrap();
        config(&format!(
            "[worker]\nresident = true\n[view]\nport = {port}\n"
        ));
        let Resident { lock, viewer, .. } = listen(p).unwrap().unwrap();
        let id = crate::worker::file_id(lock.metadata());
        assert_eq!(viewer.leaving(id, true), None);
        config(&format!(
            "[worker]\nresident = false\n[view]\nport = {port}\n"
        ));
        assert_eq!(
            viewer.leaving(id, false),
            None,
            "a request came since the last look"
        );
        assert!(viewer.leaving(id, true).is_some());
        config("[worker]\nresident = false\n[view\n");
        assert_eq!(viewer.leaving(id, true), None, "a file that does not load");
        config(&format!(
            "[worker]\nresident = true\n[view]\nport = {}\n",
            port + 1
        ));
        assert!(viewer.leaving(id, true).is_some());
        config(&format!(
            "[worker]\nresident = true\n[view]\nport = {port}\n"
        ));
        assert_eq!(viewer.leaving(id, true), None);
        std::fs::remove_file(p.join("state/view.lock")).unwrap();
        std::fs::write(p.join("state/view.lock"), "").unwrap();
        assert!(viewer.leaving(id, true).is_some(), "a home replaced");
    }

    /// Resident test 3 (R6): `--new-token` replaces the token file, so the old token fails and
    /// the new one works, and in a resident home writes the next free port into `[view] port`,
    /// the rest of config.toml as it was.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_token_replaces_the_file_and_moves_a_resident_viewer_to_a_free_port() {
        use std::os::unix::fs::PermissionsExt;
        let port = free_port();
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let config = format!("# mine\n[worker]\nresident = true\n[view]\nport = {port}\n");
        std::fs::write(p.join("config.toml"), &config).unwrap();
        ensure_token(p).unwrap();
        let file = p.join("state/view-token");
        let old = std::fs::read_to_string(&file).unwrap();
        let (moved, url) = new_token(p).unwrap();
        let new = std::fs::read_to_string(&file).unwrap();
        assert_ne!(new, old);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let to = crate::config::view(p).unwrap().port.get();
        assert!(moved && to > port, "{to}");
        assert_eq!(url, format!("http://127.0.0.1:{to}/#t={new}"));
        let text = std::fs::read_to_string(p.join("config.toml")).unwrap();
        assert!(
            text.starts_with("# mine\n[worker]\nresident = true\n"),
            "{text}"
        );
        let v = resident_of(p, to);
        let host = format!("127.0.0.1:{to}");
        let status = |t: &str| {
            v.route(
                "GET",
                "/api/repos",
                &[("Host", &host), ("X-Oboete-Token", t)],
            )
            .status
        };
        assert_eq!((status(&old), status(&new)), (401, 200));
        // Not resident: the file alone.
        std::fs::write(p.join("config.toml"), "[view]\nport = 17399\n").unwrap();
        let (moved, _) = new_token(p).unwrap();
        assert!(!moved);
        assert_eq!(crate::config::view(p).unwrap().port.get(), 17399);
    }

    /// A FIFO planted as the lock does not stall a look at the viewer, the worker's or `oboete
    /// view`'s (Codex on #376).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_look_at_the_viewer_waits_on_no_planted_fifo() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("state")).unwrap();
        let lock = home.path().join("state/view.lock");
        let fifo = std::ffi::CString::new(lock.to_str().unwrap()).unwrap();
        // SAFETY: a valid, NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(!view_held(home.path()));
        assert!(std::fs::symlink_metadata(&lock).unwrap().is_file());
    }

    /// Codex on #378: the resident viewer leaves only with no connection open, and takes none
    /// once it may leave, so its exit cuts none off; one that stays takes them as before.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_viewer_leaves_with_no_connection_and_takes_none_after() {
        let home = tempfile::tempdir().unwrap();
        let v = Arc::new(resident_of(home.path(), 17373));
        let open = Slot::take(&v).unwrap();
        assert!(!v.may_leave());
        assert!(
            Slot::take(&v).is_some(),
            "a viewer that stays takes connections"
        );
        drop(open);
        assert!(v.may_leave());
        assert!(Slot::take(&v).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_replacement_reopens_admission_and_never_runs_during_a_request() {
        let home = tempfile::tempdir().unwrap();
        let viewer = Arc::new(resident_of(home.path(), 17373));
        let request = Slot::take(&viewer).unwrap();
        viewer.replace(|| panic!("replacement ran while a request/save was live"));
        assert!(Slot::take(&viewer).is_some());
        drop(request);
        let mut called = false;
        viewer.replace(|| {
            called = true;
            assert!(
                Slot::take(&viewer).is_none(),
                "exec attempt did not freeze admission"
            );
            Err(std::io::Error::other("synthetic exec failure"))
        });
        assert!(called);
        assert!(
            Slot::take(&viewer).is_some(),
            "failed exec left admission closed"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_inherited_viewer_is_reaped_by_its_specific_pid_and_echild_ends_ownership() {
        use std::io::Write;
        struct Reap(ViewerChild);
        impl Drop for Reap {
            fn drop(&mut self) {
                if let ViewerChild::Owned(child) = &mut self.0 {
                    drop(child.stdin.take());
                    match child.try_wait() {
                        Ok(None) => {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        Err(error) if error.raw_os_error() != Some(libc::ECHILD) => {
                            let _ = child.wait();
                        }
                        _ => {}
                    }
                }
            }
        }
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", "read -r release; exit 0"])
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut original = Reap(ViewerChild::Owned(child));
        let pid = original.0.id();
        let mut inherited = ViewerChild::Inherited(pid);
        assert_eq!(inherited.id(), pid);
        assert!(!inherited.reaped());
        if let ViewerChild::Owned(child) = &mut original.0 {
            child.stdin.take().unwrap().write_all(b"go\n").unwrap();
        }
        let start = Instant::now();
        while !inherited.reaped() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(inherited.reaped(), "ECHILD did not end inherited ownership");
        assert_eq!(original.0.id(), pid);
        assert!(
            original.0.reaped(),
            "an already-reaped owned Child was retained"
        );
    }

    /// Codex on #378: a move that cannot be made changes nothing, so the old bookmark keeps
    /// working. Above port 65535 there is none.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_token_whose_move_fails_leaves_the_token_and_the_port() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        std::fs::create_dir_all(p.join("state")).unwrap();
        let config = "[worker]\nresident = true\n[view]\nport = 65535\n";
        std::fs::write(p.join("config.toml"), config).unwrap();
        ensure_token(p).unwrap();
        let file = p.join("state/view-token");
        let old = std::fs::read_to_string(&file).unwrap();
        assert!(new_token(p).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), old);
        assert_eq!(
            std::fs::read_to_string(p.join("config.toml")).unwrap(),
            config
        );
    }

    #[test]
    fn query_params_decode() {
        let p = params("q=a+b%20c&repo=%2Fhome%2Fx&x");
        assert_eq!((p["q"].as_str(), p["repo"].as_str()), ("a b c", "/home/x"));
        assert!(!p.contains_key("x"));
    }

    /// One request to `port` as the page sends it, over a socket: the status and the JSON answer
    /// (`Null` when there is none).
    fn w6p_call(
        port: u16,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let body = body
            .map(|b| serde_json::to_vec(b).unwrap())
            .unwrap_or_default();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: {token}\r\n"
        );
        if method == "POST" {
            head.push_str(&format!(
                "Origin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n",
                body.len()
            ));
        }
        let mut raw = format!("{head}\r\n").into_bytes();
        raw.extend_from_slice(&body);
        let answer = ask(port, &raw);
        let status = answer.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
        let json = (answer.split_once("\r\n\r\n"))
            .and_then(|(_, b)| serde_json::from_str(b).ok())
            .unwrap_or(Value::Null);
        (status, json)
    }

    /// A POST from the page of `v`, without a socket.
    fn w6p_post(v: &Viewer, token: &str, target: &str, value: &Value) -> (u16, Value) {
        let body = serde_json::to_vec(value).unwrap();
        let host = format!("127.0.0.1:{}", v.port());
        let origin = format!("http://{host}");
        let length = body.len().to_string();
        let headers = [
            ("Host", host.as_str()),
            ("Origin", &origin),
            ("X-Oboete-Token", token),
            ("Content-Type", "application/json"),
            ("Content-Length", &length),
        ];
        let r = request(v, "POST", target, &headers, &body);
        (
            r.status,
            serde_json::from_slice(&r.body).unwrap_or(Value::Null),
        )
    }

    /// Whether nothing answers on `port` any more, looked at for up to 2 s.
    fn w6p_closed(port: u16) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// A resident home's resident viewer, serving over sockets on a thread of its own, its token
    /// and itself.
    #[cfg(target_os = "linux")]
    fn w6p_resident() -> (tempfile::TempDir, u16, String, Arc<Viewer>) {
        let (port, home, started) = listening();
        std::fs::write(
            home.path().join("config.toml"),
            format!("[worker]\nresident = true\n[view]\nport = {port}\n"),
        )
        .unwrap();
        let token = file_token(home.path()).unwrap();
        let Resident {
            lock,
            listener,
            viewer,
        } = started;
        let serving = Arc::clone(&viewer);
        std::thread::spawn(move || {
            accept(listener, &serving);
            drop(lock);
        });
        (home, port, token, viewer)
    }

    /// W6: a port saved on the resident viewer's page is bound first and served at once with a new
    /// token, and the old address and token stop working; one another program holds is refused
    /// before anything is written.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_saved_port_moves_the_resident_page_there_at_once() {
        let (home, port, token, _) = w6p_resident();
        let (status, shown) = w6p_call(port, "GET", "/api/settings", &token, None);
        assert_eq!(status, 200, "{shown}");
        assert_eq!(
            shown["view_runtime"],
            json!({"port": port, "mode": "resident"})
        );
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let config = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        body["view"] = json!({"port": held.local_addr().unwrap().port()});
        let (status, refusal) = w6p_call(port, "POST", "/api/settings", &token, Some(&body));
        assert_eq!(
            (status, refusal["code"].as_str()),
            (409, Some("port_unavailable"))
        );
        let file = || std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert_eq!(file(), config);
        // A test running alongside may take the free port first: another is tried then.
        let (to, saved) = (0..5)
            .find_map(|_| {
                let to = free_port();
                body["view"] = json!({"port": to});
                let (status, saved) = w6p_call(port, "POST", "/api/settings", &token, Some(&body));
                (status == 200).then_some((to, saved))
            })
            .expect("no port stayed free in five tries");
        let new = file_token(home.path()).unwrap();
        assert_ne!(new, token);
        assert_eq!(
            saved["view_runtime"],
            json!({"port": to, "mode": "resident", "url": format!("http://127.0.0.1:{to}/#t={new}")})
        );
        assert_eq!(saved["view"]["port"], to);
        assert_eq!(crate::config::view(home.path()).unwrap().port.get(), to);
        let (status, shown) = w6p_call(to, "GET", "/api/settings", &new, None);
        assert_eq!((status, &shown["view_runtime"]["port"]), (200, &json!(to)));
        // The old bookmark's token ends with its address.
        assert_eq!(w6p_call(to, "GET", "/api/settings", &token, None).0, 401);
        assert_eq!(view_outcome(home.path()), format!("listening {to}"));
        assert!(w6p_closed(port), "the old address still answers");
    }

    /// W6: a new token from the resident viewer's page (R6): the next free port, served at once,
    /// and a new token in the file; the old token and the old address stop working. A move that
    /// cannot be made changes nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_new_token_moves_the_resident_page_and_ends_the_old_one() {
        let (home, port, old, _) = w6p_resident();
        let call = |body: Value| w6p_call(port, "POST", "/api/view/token", &old, Some(&body));
        assert_eq!(call(json!({"rotate": true})).0, 400);
        let path = home.path().join("config.toml");
        let config = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, "[worker]\nresident = true\n[view]\nport = 65535\n").unwrap();
        let (status, refusal) = call(json!({}));
        assert_eq!((status, refusal["code"].as_str()), (503, Some("unchanged")));
        assert_eq!(file_token(home.path()).unwrap(), old);
        std::fs::write(&path, &config).unwrap();

        let (status, answer) = call(json!({}));
        assert_eq!(status, 200, "{answer}");
        let new = file_token(home.path()).unwrap();
        let to = crate::config::view(home.path()).unwrap().port.get();
        assert!(to > port && new != old, "{to} after {port}");
        assert_eq!(answer["url"], format!("http://127.0.0.1:{to}/#t={new}"));
        assert_eq!(w6p_call(to, "GET", "/api/settings", &new, None).0, 200);
        assert_eq!(w6p_call(to, "GET", "/api/settings", &old, None).0, 401);
        assert_eq!(view_outcome(home.path()), format!("listening {to}"));
        assert!(w6p_closed(port), "the old address still answers");
    }

    /// W6 (Codex on its security review): a request for a new token whose head passed with the
    /// old token, and whose body came after another new token was made, on the page or by
    /// `oboete view --new-token`, makes none: the token it brought is checked again under the
    /// lock, so a holder of an old token cannot outlast its replacement.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_new_token_asked_with_a_replaced_token_makes_none() {
        let (home, port, old, v) = w6p_resident();
        let head = |token: &str| {
            let host = format!("127.0.0.1:{}", v.port());
            let origin = format!("http://{host}");
            let h = [
                ("Host", host.as_str()),
                ("Origin", &origin),
                ("X-Oboete-Token", token),
                ("Content-Type", "application/json"),
                ("Content-Length", "2"),
            ];
            match v.head("POST", "/api/view/token", &h) {
                Head::Body(2, _, given) => given,
                _ => panic!("the head did not pass"),
            }
        };
        // Replaced on the page while the request waits for its body.
        let waiting = head(&old);
        let (status, answer) = w6p_call(port, "POST", "/api/view/token", &old, Some(&json!({})));
        assert_eq!(status, 200, "{answer}");
        let new = file_token(home.path()).unwrap();
        let to = crate::config::view(home.path()).unwrap().port.get();
        assert_eq!(v.view_token(b"{}", &waiting).status, 401);
        assert_eq!(file_token(home.path()).unwrap(), new);
        assert_eq!(crate::config::view(home.path()).unwrap().port.get(), to);
        assert_eq!(w6p_call(to, "GET", "/api/settings", &new, None).0, 200);
        // Replaced by the command while the request waits for its body.
        let waiting = head(&new);
        new_token(home.path()).unwrap();
        let newer = file_token(home.path()).unwrap();
        let moved = crate::config::view(home.path()).unwrap().port.get();
        assert_eq!(v.view_token(b"{}", &waiting).status, 401);
        assert_eq!(file_token(home.path()).unwrap(), newer);
        assert_eq!(crate::config::view(home.path()).unwrap().port.get(), moved);
    }

    /// W6 (Codex on its security review): the resident address goes out with the token of its
    /// own port: after a new token moved the page (here by the command, which leaves the running
    /// viewer on the old port until its tick), the old port is not paired with the new token.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_the_resident_address_pairs_its_port_and_its_token() {
        let (home, port, old, _v) = w6p_resident();
        assert_eq!(
            resident_address(home.path(), port).unwrap(),
            format!("http://127.0.0.1:{port}/#t={old}")
        );
        new_token(home.path()).unwrap();
        let new = file_token(home.path()).unwrap();
        let to = crate::config::view(home.path()).unwrap().port.get();
        assert_eq!(resident_address(home.path(), port), None);
        assert_eq!(
            resident_address(home.path(), to).unwrap(),
            format!("http://127.0.0.1:{to}/#t={new}")
        );
    }

    /// W6 (the commit security review): a write whose head passed with the old token, and whose
    /// body came after a new token was made, does nothing: neither a settings save, which could
    /// move the page off the address the new token was given, nor any other write. A save checks
    /// the token again under the hold a new token takes, so one that knew the new version of
    /// config.toml is refused too.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_write_that_waited_out_a_new_token_does_nothing() {
        let (home, port, old, v) = w6p_resident();
        let (status, shown) = w6p_call(port, "GET", "/api/settings", &old, None);
        assert_eq!(status, 200, "{shown}");
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        body["view"] = json!({"port": free_port()});
        let body = serde_json::to_vec(&body).unwrap();
        let head = |path: &str, len: usize| {
            let host = format!("127.0.0.1:{}", v.port());
            let origin = format!("http://{host}");
            let len = len.to_string();
            let h = [
                ("Host", host.as_str()),
                ("Origin", &origin),
                ("X-Oboete-Token", &old),
                ("Content-Type", "application/json"),
                ("Content-Length", &len),
            ];
            match v.head("POST", path, &h) {
                Head::Body(_, save, given) => (save, given),
                Head::Answer(_) => panic!("the head did not pass"),
            }
        };
        let (save, given) = head("/api/settings", body.len());
        let (preview, preview_given) = head("/api/settings/recovery/preview", 2);
        let (status, answer) = w6p_call(port, "POST", "/api/view/token", &old, Some(&json!({})));
        assert_eq!(status, 200, "{answer}");
        let new = file_token(home.path()).unwrap();
        let to = crate::config::view(home.path()).unwrap().port.get();
        assert_eq!(v.take(save, &body, &given).status, 401);
        assert_eq!(v.take(preview, b"{}", &preview_given).status, 401);
        // One that knew the version the new token wrote.
        let (_, shown) = w6p_call(to, "GET", "/api/settings", &new, None);
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        body["view"] = json!({"port": free_port()});
        assert_eq!(
            v.save(&serde_json::to_vec(&body).unwrap(), &old).status,
            401
        );
        assert_eq!(crate::config::view(home.path()).unwrap().port.get(), to);
        assert_eq!(file_token(home.path()).unwrap(), new);
        assert_eq!(v.port(), to);
    }

    /// W6 (Codex on its security review): a port save that passed its token's checks and then
    /// waited for config.lock while `oboete view --new-token` held it (which wrote a port and a
    /// token there) makes no token: it checks again under the lock, even when its body names the
    /// version the command wrote.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_port_save_behind_the_commands_new_token_makes_none() {
        let (home, port, old, v) = w6p_resident();
        let held = crate::settings::config_lock(home.path()).unwrap();
        // The command's port, under its hold; the token comes after.
        let theirs = free_port();
        crate::settings::set_view_port(home.path(), theirs).unwrap();
        let shown = crate::settings::show(home.path());
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        body["view"] = json!({"port": free_port()});
        let saver = {
            let (v, old) = (Arc::clone(&v), old.clone());
            std::thread::spawn(move || w6p_post(&v, &old, "/api/settings", &body))
        };
        std::thread::sleep(Duration::from_millis(300));
        write_token(home.path()).unwrap();
        let theirs_token = file_token(home.path()).unwrap();
        drop(held);
        let (status, answer) = saver.join().unwrap();
        assert_eq!(status, 401, "{answer}");
        assert_eq!(file_token(home.path()).unwrap(), theirs_token);
        assert_eq!(crate::config::view(home.path()).unwrap().port.get(), theirs);
        assert_eq!(v.port(), port);
    }

    /// W6 (Codex's final security review): a save that names no other port (the port it serves
    /// on, or no `view` at all) and waited for config.lock while `oboete view --new-token` held it
    /// changes nothing either, even with the version the command wrote: it would put the old
    /// port back while the command hands out the new one.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_plain_save_behind_the_commands_new_token_changes_nothing() {
        let (home, port, old, v) = w6p_resident();
        for view in [Some(json!({"port": port})), None] {
            let held = crate::settings::config_lock(home.path()).unwrap();
            let theirs = free_port();
            crate::settings::set_view_port(home.path(), theirs).unwrap();
            let mut body: Value =
                serde_json::from_slice(&save_body(&crate::settings::show(home.path()))).unwrap();
            match &view {
                Some(view) => body["view"] = view.clone(),
                None => drop(body.as_object_mut().unwrap().remove("view")),
            }
            let saver = {
                let (v, old) = (Arc::clone(&v), old.clone());
                std::thread::spawn(move || w6p_post(&v, &old, "/api/settings", &body))
            };
            std::thread::sleep(Duration::from_millis(300));
            write_token(home.path()).unwrap();
            let theirs_token = file_token(home.path()).unwrap();
            drop(held);
            let (status, answer) = saver.join().unwrap();
            assert_eq!(status, 401, "{view:?}: {answer}");
            assert_eq!(file_token(home.path()).unwrap(), theirs_token);
            assert_eq!(crate::config::view(home.path()).unwrap().port.get(), theirs);
            // Back to the page's token and port for the next round.
            crate::settings::set_view_port(home.path(), port).unwrap();
            std::fs::write(home.path().join("state/view-token"), &old).unwrap();
        }
    }

    /// W6 (Codex on its security review): a new token that cannot be staged refuses the port save
    /// before anything is written, so the old port keeps serving and its token stays the one that
    /// works: no move leaves the old token behind a free port.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_port_save_whose_token_cannot_be_written_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no arguments and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return; // root writes into a read-only folder
        }
        let (home, port, token, _v) = w6p_resident();
        let (status, shown) = w6p_call(port, "GET", "/api/settings", &token, None);
        assert_eq!(status, 200, "{shown}");
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        body["view"] = json!({"port": free_port()});
        let state = home.path().join("state");
        let config = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        // The lock file is there already, as after any earlier save; only a new file is refused.
        drop(crate::settings::config_lock(home.path()).unwrap());
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
        let (status, refusal) = w6p_call(port, "POST", "/api/settings", &token, Some(&body));
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            (status, refusal["code"].as_str()),
            (500, Some("write_failed"))
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            config
        );
        assert_eq!(file_token(home.path()).unwrap(), token);
        assert_eq!(w6p_call(port, "GET", "/api/settings", &token, None).0, 200);
    }

    /// W6 (CodeRabbit): a port save's settings are written only once its new token is in place.
    /// One the settings refuse makes no token, and one whose token cannot be put in place writes
    /// none of its settings, the port and every other key alike.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_port_save_writes_its_settings_only_after_its_token() {
        let (home, port, token, v) = w6p_resident();
        let (status, shown) = w6p_call(port, "GET", "/api/settings", &token, None);
        assert_eq!(status, 200, "{shown}");
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        let to = free_port();
        body["view"] = json!({"port": to});
        body["capture"]["store_prompts"] =
            json!(!shown["capture"]["store_prompts"].as_bool().unwrap());
        let config = || std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        let before = config();
        let mut stale = body.clone();
        stale["version"] = json!("another");
        let (status, refusal) = w6p_post(&v, &token, "/api/settings", &stale);
        assert_eq!((status, refusal["code"].as_str()), (409, Some("stale")));
        assert_eq!(file_token(home.path()).unwrap(), token);
        assert_eq!(w6p_call(port, "GET", "/api/settings", &token, None).0, 200);
        // The token's place taken by a folder: the staged token cannot be renamed there.
        let place = home.path().join("state/view-token");
        std::fs::remove_file(&place).unwrap();
        std::fs::create_dir(&place).unwrap();
        let r = v.save_with_new_token(&serde_json::to_vec(&body).unwrap(), to);
        let answer: Value = serde_json::from_slice(&r.body).unwrap();
        assert!(r.status == 500 && answer["code"] == "write_failed");
        assert_eq!(config(), before);
        assert!(place.is_dir());
        assert_eq!(v.port(), port);
        let left = |dir: &Path| {
            std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with("tmp"))
                .collect::<Vec<_>>()
        };
        assert_eq!(left(home.path()), Vec::<String>::new());
        assert_eq!(left(&home.path().join("state")), Vec::<String>::new());
    }

    /// W6 (Codex on its security review): a saved port is bound, written and moved onto under
    /// the hold a new token takes too, so a save that waited cannot move the page back after a new
    /// token has answered with its own port.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_port_save_waits_while_another_move_is_made() {
        let (home, port, token, v) = w6p_resident();
        let (status, shown) = w6p_call(port, "GET", "/api/settings", &token, None);
        assert_eq!(status, 200, "{shown}");
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        let to = free_port();
        body["view"] = json!({"port": to});
        let file = || std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        let config = file();
        let held = v.moving.lock().unwrap();
        let saver = {
            let v = Arc::clone(&v);
            std::thread::spawn(move || w6p_post(&v, &token, "/api/settings", &body))
        };
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            file(),
            config,
            "the save wrote while another move was being made"
        );
        assert_eq!(view_outcome(home.path()), format!("listening {port}"));
        drop(held);
        let (status, saved) = saver.join().unwrap();
        assert_eq!(status, 200, "{saved}");
        assert_eq!(saved["view_runtime"]["port"], to);
        assert_eq!(view_outcome(home.path()), format!("listening {to}"));
    }

    /// W6: a foreground run's token is its own and ends with it: there is none to replace. A port
    /// saved on its page is the resident viewer's, so the run stays where it is, and a home that is
    /// not resident starts no resident viewer.
    #[test]
    fn w6p_a_foreground_run_keeps_its_token_and_its_port() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "[view]\nport = 17399\n").unwrap();
        let token = "f".repeat(32);
        let v = Viewer::new(home.path(), None, 4323, Token::Run(token.clone()));
        let host = "127.0.0.1:4323";
        let got = request(
            &v,
            "GET",
            "/api/settings",
            &[("Host", host), ("X-Oboete-Token", &token)],
            &[],
        );
        let shown: Value = serde_json::from_slice(&got.body).unwrap();
        assert_eq!(
            shown["view_runtime"],
            json!({"port": 4323, "mode": "foreground"})
        );
        let (status, refusal) = w6p_post(&v, &token, "/api/view/token", &json!({}));
        assert_eq!(
            (status, refusal["code"].as_str()),
            (409, Some("foreground"))
        );
        assert!(!home.path().join("state/view-token").exists());
        let (status, refusal) = w6p_post(&v, &token, "/api/view/resident", &json!({}));
        assert_eq!(
            (status, refusal["code"].as_str()),
            (409, Some("not_resident"))
        );
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        body["view"] = json!({"port": 17400});
        let (status, saved) = w6p_post(&v, &token, "/api/settings", &body);
        assert_eq!(status, 200, "{saved}");
        assert_eq!(
            saved["view_runtime"],
            json!({"port": 4323, "mode": "foreground"})
        );
        assert_eq!(crate::config::view(home.path()).unwrap().port.get(), 17400);
        assert_eq!(v.port(), 4323);
        let guarded = Viewer::new(home.path(), None, 4321, Token::Run("t0k".into()));
        for path in ["/api/view/token", "/api/view/resident"] {
            save_guards(&guarded, path, MAX_BODY, b"{}");
        }
    }

    /// W6 (the commit security review): a resident port saved on a foreground run's page replaces
    /// the token file too, since the resident viewer comes back on that port at its tick and the
    /// old bookmark's port is free then; the run keeps its own token and port, and its answer
    /// holds no address.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_resident_port_saved_in_the_foreground_replaces_the_token_file() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "[view]\nport = 17398\n").unwrap();
        std::fs::create_dir_all(home.path().join("state")).unwrap();
        write_token(home.path()).unwrap();
        let old = file_token(home.path()).unwrap();
        let token = "e".repeat(32);
        let v = Viewer::new(home.path(), None, 4324, Token::Run(token.clone()));
        let shown = crate::settings::show(home.path());
        let mut body: Value = serde_json::from_slice(&save_body(&shown)).unwrap();
        body["view"] = json!({"port": 17399});
        let (status, saved) = w6p_post(&v, &token, "/api/settings", &body);
        assert_eq!(status, 200, "{saved}");
        assert_eq!(
            saved["view_runtime"],
            json!({"port": 4324, "mode": "foreground"})
        );
        assert_eq!(crate::config::view(home.path()).unwrap().port.get(), 17399);
        assert_ne!(file_token(home.path()).unwrap(), old);
        // The same port again changes nothing more.
        let new = file_token(home.path()).unwrap();
        let mut body: Value =
            serde_json::from_slice(&save_body(&crate::settings::show(home.path()))).unwrap();
        body["view"] = json!({"port": 17399});
        assert_eq!(w6p_post(&v, &token, "/api/settings", &body).0, 200);
        assert_eq!(file_token(home.path()).unwrap(), new);
    }

    /// W6: R7 from a foreground run's page once its home is saved resident: the answer is the
    /// address of the resident viewer that is up (here one this test started), with its file's
    /// token, and the run takes no more connections and returns once that answer is sent.
    #[cfg(target_os = "linux")]
    #[test]
    fn w6p_a_foreground_run_hands_its_page_to_the_resident_viewer_and_ends() {
        let (home, port, token, _) = w6p_resident();
        // Held here, so the bring-up starts no worker from a test.
        let _worker = crate::worker::lock(home.path()).unwrap().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let run = listener.local_addr().unwrap().port();
        let mine = "r".repeat(32);
        let v = Arc::new(Viewer::new(
            home.path(),
            None,
            run,
            Token::Run(mine.clone()),
        ));
        let ended = std::thread::spawn(move || accept(listener, &v));
        let on = |port: u16, token: &str| {
            w6p_call(port, "POST", "/api/view/resident", token, Some(&json!({})))
        };
        let (status, refusal) = on(port, &token);
        assert_eq!((status, refusal["code"].as_str()), (409, Some("resident")));
        let (status, answer) = on(run, &mine);
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["url"], format!("http://127.0.0.1:{port}/#t={token}"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ended.is_finished() {
            assert!(Instant::now() < deadline, "the run did not end");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(w6p_closed(run), "the run's address still answers");
    }
}
