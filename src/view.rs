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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    /// Where `oboete view` was started: its checkout is the page's default scope and its search's
    /// caller (`checkout`). The resident viewer has none, and every repository is its default
    /// scope (docs/resident.md R8).
    cwd: Option<PathBuf>,
    port: u16,
    token: Token,
    /// Settings saves, one at a time.
    saving: Mutex<()>,
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
/// first and handed to it.
enum Head {
    Answer(Response),
    Body(usize, Save),
}

/// A typed write that takes a request's body.
type Save = fn(&Viewer, &[u8]) -> Response;

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
    accept(&listener, &viewer);
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
    let token = file_token(home).ok_or_else(|| anyhow!("the resident viewer's token file"))?;
    let url = format!("http://127.0.0.1:{port}/#t={token}");
    println!("{url}\n(the resident viewer: bookmark this address; it stays up)");
    if open {
        match opener_page(home, port, &url) {
            Ok(page) => open_browser(&page),
            Err(e) => eprintln!("(could not write the page for the browser: {e})"),
        }
    }
    Ok(())
}

/// `oboete view --new-token` (R6): a new token file and, in a resident home, the next free port
/// in `[view] port`, so the viewer comes back on a new address: the old one's tick sees the port
/// change and it leaves, and the worker starts it again. Whether the port moved, and the address
/// to bookmark. The move comes first: one that fails changes nothing, and the old bookmark keeps
/// working; a token write that fails after it is mended by running the command again.
pub fn new_token(home: &Path) -> Result<(bool, String)> {
    std::fs::create_dir_all(home.join("state"))?;
    anyhow::ensure!(owner_only(home), NOT_PRIVATE);
    let from = crate::config::view(home)?.port.get();
    let moved = resident_home(home);
    let port = if moved {
        let port = (from..=u16::MAX)
            .skip(1)
            .find(|&p| TcpListener::bind(("127.0.0.1", p)).is_ok())
            .ok_or_else(|| anyhow!("no free port after {from}"))?;
        crate::settings::set_view_port(home, port)?;
        port
    } else {
        from
    };
    write_token(home)?;
    let token = file_token(home).ok_or_else(|| anyhow!("the new token file"))?;
    Ok((moved, format!("http://127.0.0.1:{port}/#t={token}")))
}

/// 16 bytes of the OS generator, in lower-case hex.
fn fresh_token() -> Result<String> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| anyhow!("random token: {e}"))?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// The outcome of a resident start on a filesystem whose modes keep no file its owner's alone.
const NOT_PRIVATE: &str = "this home's filesystem cannot keep the page's token to its owner";

/// The outcome of a resident start whose port another program or home holds.
const PORT_IN_USE: &str = "port in use";

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

/// What starts the resident viewer for a resident worker (R4): at the worker's start and then at
/// most once a minute, where the worker looks at its backup deadline, when the home's files can
/// be its owner's and nothing holds `state/view.lock`; after "port in use", only every 10
/// minutes. It keeps the viewer it started and reaps it before it starts another.
pub struct Starter {
    every: Duration,
    after_busy: Duration,
    next: Instant,
    started: Option<Instant>,
    child: Option<std::process::Child>,
    spawn: Spawn,
}

impl Starter {
    pub fn new() -> Self {
        Self::with(
            MINUTE,
            AFTER_PORT_IN_USE,
            Box::new(|home| crate::hook::spawn_detached(home, &["view", "--resident"])),
        )
    }

    fn with(every: Duration, after_busy: Duration, spawn: Spawn) -> Self {
        Self {
            every,
            after_busy,
            next: Instant::now(),
            started: None,
            child: None,
            spawn,
        }
    }

    pub fn due(&mut self, home: &Path) {
        // Reaped as soon as it has left, so it is no zombie for a minute.
        if self
            .child
            .as_mut()
            .is_some_and(|c| !matches!(c.try_wait(), Ok(None)))
        {
            self.child = None;
        }
        let now = Instant::now();
        if now < self.next || self.child.is_some() {
            return;
        }
        self.next = now + self.every;
        let busy = outcome(home).as_deref() == Some(PORT_IN_USE);
        if busy
            && self
                .started
                .is_some_and(|t| now.duration_since(t) < self.after_busy)
        {
            return;
        }
        if !owner_only(home) || view_held(home) {
            return;
        }
        self.started = Some(now);
        forget_outcome(home);
        self.child = (self.spawn)(home);
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
    let id = crate::worker::file_id(lock.metadata());
    let looking = Arc::clone(&viewer);
    std::thread::spawn(move || {
        let mut seen = looking.requests.load(Ordering::SeqCst);
        loop {
            std::thread::sleep(MINUTE);
            let now = looking.requests.load(Ordering::SeqCst);
            let quiet = std::mem::replace(&mut seen, now) == now;
            let Some(why) = looking.leaving(id, quiet) else {
                continue;
            };
            if looking.may_leave() {
                if !matches!(why, Leaving::Gone) {
                    let _ = say(&looking.home, &format!("left: {}", why.text()));
                }
                std::process::exit(0);
            }
        }
    });
    accept(&listener, &viewer);
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
        viewer: Arc::new(Viewer::new(home, None, port, Token::File)),
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

/// A new token file: staged with mode 0600 under a name of this process's (a viewer's start and
/// `--new-token` may write at once), synced, renamed over the old one, and its folder synced.
fn write_token(home: &Path) -> Result<()> {
    let state = home.join("state");
    let staged = state.join(format!("view-token.{}.tmp", std::process::id()));
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
    file.write_all(fresh_token()?.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&staged, state.join("view-token"))?;
    std::fs::File::open(&state)?.sync_all()?;
    Ok(())
}

fn accept(listener: &TcpListener, viewer: &Arc<Viewer>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // Over the cap, the connection is dropped here: closed before a byte is read.
        let Some(slot) = Slot::take(viewer) else {
            continue;
        };
        // A browser keeps idle pre-connected sockets open; one thread each keeps them from
        // stalling the rest.
        std::thread::spawn(move || slot.0.serve(stream));
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

impl Viewer {
    fn new(home: &Path, cwd: Option<PathBuf>, port: u16, token: Token) -> Self {
        Self {
            home: home.to_path_buf(),
            cwd,
            port,
            token,
            saving: Mutex::new(()),
            opener: Mutex::new(None),
            live: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
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
        if crate::config::view(&self.home).is_ok_and(|v| v.port.get() != self.port) {
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
        let resp = match head {
            Head::Answer(r) => r,
            // Read only once the head has passed every check (`save_gate`).
            Head::Body(len, save) => {
                while buf.len() < at + len {
                    if !more(&mut stream, &mut buf) {
                        return;
                    }
                }
                save(self, &buf[at..at + len])
            }
        };
        send(&mut stream, &resp.bytes(head_only), ANSWER_TIME);
    }

    /// Typed writes go through `save_gate`; every other request is answered by `route`.
    fn head(&self, method: &str, target: &str, headers: &[(&str, &str)]) -> Head {
        let (cap, save): (usize, Save) = match (method, target) {
            ("POST", "/api/settings") => (MAX_BODY, Self::save),
            ("POST", "/api/key") => (MAX_KEY_BODY, Self::save_key),
            ("POST", "/api/resume") => (MAX_BODY, Self::resume),
            _ => return Head::Answer(self.route(method, target, headers)),
        };
        match self.save_gate(headers, cap) {
            Ok(len) => Head::Body(len, save),
            Err(r) => Head::Answer(r),
        }
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

    /// The settings as saved.
    fn save(&self, body: &[u8]) -> Response {
        saved(crate::settings::save(&self.home, &self.saving, body))
    }

    /// A key written to its entry's key file (#94 part 3); the answer never holds it.
    fn save_key(&self, body: &[u8]) -> Response {
        saved(crate::settings::save_key(&self.home, &self.saving, body))
    }

    fn resume(&self, body: &[u8]) -> Response {
        saved(crate::settings::resume(&self.home, &self.saving, body))
    }

    /// DNS rebinding: a page on another name that resolves to 127.0.0.1 sends its own Host.
    /// Browsers leave port 80 out of Host.
    fn host_ok(&self, host: Option<&str>) -> bool {
        host.is_some_and(|h| {
            h == format!("127.0.0.1:{}", self.port)
                || h == format!("localhost:{}", self.port)
                || (self.port == 80 && (h == "127.0.0.1" || h == "localhost"))
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
            Token::File => Some(opener_path(&self.home, self.port)),
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

    /// `--open`: the page for the browser, registered before `launch` starts the opener.
    fn open(&self, home: &Path, url: &str, launch: impl FnOnce(&Path)) {
        match opener_page(home, self.port, url) {
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
            return Response::json(&crate::settings::show(&self.home));
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
        let manifest = if crate::raw::exists(&self.home) {
            let settings = crate::capture::Settings::load(&self.home)?;
            let raw = crate::raw::open(&self.home)?;
            let session = crate::hook::own_session("unknown".into(), &raw);
            crate::hook::start_text_read(&self.home, &raw, &repo, &branch, &session, &settings)?
                .map(|s| s.text)
        } else {
            None
        };
        let text = crate::hook::joined(&self.home, manifest.as_deref());
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
    let mut answer = gated(answer);
    if let Some(next) = next {
        answer["next"] = next;
    }
    answer
}

fn gated(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact::outbound(&s)),
        Value::Array(xs) => Value::Array(xs.into_iter().map(gated).collect()),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, v)| (redact::outbound(&k), gated(v)))
                .collect(),
        ),
        other => other,
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
    for mut s in summaries {
        s.fields.remove("notes");
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
    // A leftover file alone is no rebuild: one stopped by a crash stays until the next.
    let rebuilding = crate::worker::running(home)
        && std::fs::read_dir(home).is_ok_and(|d| {
            d.flatten().any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("knowledge.db.rebuilding-")
            })
        });
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
        let v = Viewer {
            home: s.home.path().to_owned(),
            cwd: Some(s.home.path().to_owned()),
            port: 4321,
            token: Token::Run("t0k".into()),
            saving: Mutex::new(()),
            opener: Mutex::new(None),
            live: AtomicUsize::new(0),
        };
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
                ("notes", "Not displayed."),
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
        for text in ["FERN", "MOSS", "REED", "Not displayed."] {
            assert!(!answer.contains(text));
        }
        assert!(page["items"][1]["fields"].get("notes").is_none());
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

    /// Rows 53-1 and 53-4: every store route asks for the token and this viewer's Host.
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
        ];
        for route in &routes {
            assert_eq!(v.route("GET", route, &[HOST]).status, 401, "{route}");
            let foreign = [("Host", "evil.example:4321"), TOKEN];
            assert_eq!(v.route("GET", route, &foreign).status, 403, "{route}");
            assert_eq!(v.route("GET", route, &[HOST, TOKEN]).status, 200, "{route}");
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
        v80.port = 80;
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
        v.port = listener.local_addr().unwrap().port();
        let port = v.port;
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

    /// D11: the page puts store text into text nodes only and deletes nothing.
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
        let js = APP_JS.to_ascii_lowercase();
        for quote in ['\'', '"', '`'] {
            assert!(
                !js.contains(&format!("{quote}delete{quote}")),
                "a DELETE request"
            );
        }
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
        v.port = 4323;
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
            Head::Body(len, save) => {
                assert_eq!(len, body.len());
                save(v, body)
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
        for (path, cap) in [("/api/settings", MAX_BODY), ("/api/key", MAX_KEY_BODY)] {
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
            "/api/key?x=1",
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
        assert!(matches!(v.head("POST", path, &at_cap), Head::Body(l, _) if l == cap));
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
        v.port = listener.local_addr().unwrap().port();
        let port = v.port;
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
        v.port = listener.local_addr().unwrap().port();
        let port = v.port;
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
        v.port = listener.local_addr().unwrap().port();
        let port = v.port;
        let v = Arc::new(v);
        let server = Arc::clone(&v);
        std::thread::spawn(move || accept(&listener, &server));
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
        v.port = listener.local_addr().unwrap().port();
        let port = v.port;
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
        let port = free_port();
        let home = resident_home(port);
        let p = home.path();
        let started = listen(p).unwrap().unwrap();
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
        lock.try_lock().unwrap();
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
}
