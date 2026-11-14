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
/// The brand kit's favicons (assets/brand, DESIGN.md "Brand"): the ink symbol on a white tile, so it
/// shows on a light or a dark tab alike.
const FAVICON_ICO: &[u8] = include_bytes!("../assets/brand/favicon/favicon.ico");
const FAVICON_SVG: &str = include_str!("../assets/brand/favicon/favicon.svg");
const TOUCH_ICON: &[u8] = include_bytes!("../assets/brand/favicon/apple-touch-icon.png");
/// Request line and headers; a browser's GET is well under 2 KB.
const MAX_HEAD: usize = 16 * 1024;
/// A settings save's body; the page's is under 2 KB.
const MAX_BODY: usize = 16 * 1024;
/// The prompt hook's body: the prompt (at most twice `embed::PROMPT_CHARS` characters), the
/// session's agent and name, and the keys of a manifest shown at the prompt (at most
/// `search::b::PROMPT_SHOWN` of at most 256 bytes each, a claim's 64), with room for JSON's quotes
/// and escapes (Codex on #521).
const PROMPT_BODY: usize = 1024 * 1024;
/// Decision 52: the claude-mem names a prompt's checkout may send (claude-mem gives three at most).
const PROMPT_NAMES: usize = 16;
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
    /// When the resident viewer took that lock, unix ms: a request to leave written before it was
    /// another viewer's (`LEAVE`). 0 for a foreground viewer.
    started: i64,
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
    /// The embedder's preview, the choice running and the last one (W4).
    embedding: crate::settings::embedding::Embedding,
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
    /// Decision 71 (Codex on #830): the lookups' query vectors still being made, at most
    /// `LOOKUP_VECTORS`; one a lookup stopped waiting for runs on, holding its reads, after the
    /// lookup's connection is gone.
    vectors: Arc<AtomicUsize>,
    /// The secret the links to copies are made with (docs/deliverables.md), made when first
    /// needed: a link opens its copy until this viewer stops.
    files: std::sync::OnceLock<[u8; 32]>,
}

/// The query vectors lookups leave being made past their wait, at most; past them a lookup takes
/// the words side alone, so a held embedder cannot gather threads beyond the connections' cap.
const LOOKUP_VECTORS: usize = 2;

/// One of the `LOOKUP_VECTORS` a lookup's query vector holds while it is made; given back however
/// it ends, its thread's panic or a thread that could not start too (Codex on #830).
struct Making(Arc<AtomicUsize>);

impl Making {
    fn take(vectors: &Arc<AtomicUsize>) -> Option<Self> {
        vectors
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < LOOKUP_VECTORS).then_some(n + 1)
            })
            .ok()?;
        Some(Making(Arc::clone(vectors)))
    }
}

impl Drop for Making {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A lookup's query vector, made by `make` on a thread `spawn` starts while one of
/// `LOOKUP_VECTORS` is free, and waited for no longer than `wait`: none when it came later, made
/// none, or no permit or thread could be had (the permit, moved into the work, goes back with it).
fn vector_within(
    vectors: &Arc<AtomicUsize>,
    wait: Duration,
    make: impl FnOnce() -> Option<Vec<f32>> + Send + 'static,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) -> Option<Vec<f32>> {
    let making = Making::take(vectors)?;
    let deadline = Instant::now() + wait;
    let (made, vector) = std::sync::mpsc::channel();
    spawn(Box::new(move || {
        let _making = making;
        let _ = made.send(make());
    }))
    .ok()?;
    let heard = vector
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()
        .flatten();
    // One heard past the wait, as by a request held up meanwhile, is not taken (Codex on #830).
    heard.filter(|_| Instant::now() <= deadline)
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
    /// A copy asked for by its link (`send_file`).
    File(FileAsk),
}

/// A copy asked for by its link, `/files/<tag>/<name>` (docs/deliverables.md, slice 2): its tag,
/// the one `Range` it asked for, and the deadline of the whole answer, from when its head came.
struct FileAsk {
    tag: [u8; 16],
    range: Option<String>,
    deadline: Instant,
}

/// The whole answer to a link to a copy, the head's lookup and the hash included: a reader slower
/// than this is cut, and a player asks again from where it stopped (`Range`).
const FILE_TIME: Duration = Duration::from_secs(60);

/// A copy's headers: its page's script runs in an opaque origin, so it can read neither the
/// viewer's token nor its answers; and no policy of the viewer's own page.
const COPY_HEADERS: &str = "Content-Security-Policy: sandbox allow-scripts\r\n\
    X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n\
    Accept-Ranges: bytes\r\n";

/// A copy's type by its extension, claude-mem's table (`WorkViewRoutes.ts` 79-83).
fn copy_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "html" => "text/html; charset=utf-8",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "csv" => "text/csv; charset=utf-8",
        "md" | "markdown" | "txt" | "rst" | "json" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// One `bytes=` range of `len` bytes (RFC 9110 14.1.2): `Some(Ok((first, last)))` answers 206,
/// `Some(Err(()))` 416; none, several, or one that is no range answer the whole file.
fn byte_range(header: Option<&str>, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header?.trim().strip_prefix("bytes=")?;
    let (first, last) = spec.split_once('-')?;
    let (first, last) = (first.trim(), last.trim());
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u64>().ok())
            .flatten()
    };
    if first.is_empty() {
        // The last `n` bytes.
        let n = number(last)?;
        return Some(if n == 0 || len == 0 {
            Err(())
        } else {
            Ok((len.saturating_sub(n), len - 1))
        });
    }
    let first = number(first)?;
    let last = if last.is_empty() {
        u64::MAX
    } else {
        number(last)?
    };
    if last < first {
        return None;
    }
    Some(if first >= len {
        Err(())
    } else {
        Ok((first, last.min(len - 1)))
    })
}

/// The head of a copy's answer: 200 with the whole length, 206 with its range, or 416.
fn copy_head(status: u16, name: &str, first: u64, count: u64, len: u64) -> Vec<u8> {
    let (reason, ctype) = match status {
        200 => ("OK", copy_type(name)),
        206 => ("Partial Content", copy_type(name)),
        _ => ("Range Not Satisfiable", "text/plain; charset=utf-8"),
    };
    let range = match status {
        206 => format!(
            "Content-Range: bytes {first}-{}/{len}\r\n",
            first + count - 1
        ),
        416 => format!("Content-Range: bytes */{len}\r\n"),
        _ => String::new(),
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {count}\r\n\
         {range}{COPY_HEADERS}Connection: close\r\n\r\n"
    )
    .into_bytes()
}

/// A link's tag: 32 lowercase hex digits.
fn parse_tag(s: &str) -> Option<[u8; 16]> {
    let mut tag = [0u8; 16];
    if s.len() != 32 {
        return None;
    }
    for (i, out) in tag.iter_mut().enumerate() {
        let pair = s.get(i * 2..i * 2 + 2)?;
        if !pair
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        *out = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(tag)
}

type FilesMac = hmac::Hmac<Sha256>;

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
    // This run serves the page: its searches use its own model (D14).
    crate::search::b::serving();
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
/// How often the resident viewer looks for new hook records to keep its model warm (row 31).
const WARM_EVERY: Duration = Duration::from_secs(2);

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

/// G23: the resident viewer's port and token while it listens, for the prompt hook to ask it:
/// it holds its lock, the outcome it wrote says it listens on the configured port, and its token
/// file reads. A read-only status, as `resident_line`'s.
pub(crate) fn resident_answering(home: &Path) -> Option<(u16, String)> {
    let port = crate::config::view(home).ok()?.port.get();
    let held = crate::worker::lock_held(&home.join("state/view.lock"));
    resident_up(outcome(home).as_deref(), port, held).then_some(())?;
    Some((port, file_token(home)?))
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
    // Parity row 31: every model this viewer loads, a search's before the first hook record's
    // too, stays as long as the hooks keep it (Codex and CodeRabbit on #560).
    crate::search::b::keep_warm();
    crate::search::b::serving();
    let Some(Resident {
        lock,
        listener,
        viewer,
    }) = listen(home)?
    else {
        return Ok(());
    };
    let id = viewer.home_lock;
    // Parity row 31: while the hooks record, the local model is loaded and kept, so a prompt after
    // a session's start or the agent's work finds it warm instead of the words alone.
    let warming = viewer.home.clone();
    std::thread::spawn(move || {
        let mut seen = None;
        let mut pages = crate::search::b::Warmth::default();
        loop {
            let newest = crate::raw::newest_hook(&warming).ok().flatten();
            if let Some((seq, ts)) = newest {
                if crate::search::b::warms(seen, (seq, ts), crate::db::now_ms()) {
                    crate::search::b::warm(&warming);
                }
                seen = Some(seq);
            }
            // Decision 71: what an agent's memory lookup reads, kept in the page cache while the
            // hooks record, so a lookup does not wait on the disk.
            if pages.due(crate::db::now_ms(), newest.map(|(_, ts)| ts)) {
                match crate::search::b::warm_pages(&warming) {
                    Ok(Some(took)) => pages.read(crate::db::now_ms(), Some(took)),
                    // No store to read (a rebuild set knowledge.db aside): looked at again at the
                    // next tick, so the rebuilt one is read as soon as it is there (cubic on #830).
                    Ok(None) => {}
                    Err(_) => pages.read(crate::db::now_ms(), None),
                }
            }
            std::thread::sleep(WARM_EVERY);
        }
    });
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
                    if matches!(why, Leaving::Asked) {
                        let _ = std::fs::remove_file(looking.home.join("state").join(LEAVE));
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
    /// `setup all --remove` asked it to (`LEAVE`), whatever requests came.
    Asked,
    Moved,
    NotResident,
}

/// Parity row 50: what `setup all --remove` writes in `state` to ask a resident viewer to leave at
/// its next look, as a page a tab polls every few seconds is never quiet for a minute (Codex on
/// #618): the time it was written, unix ms.
pub(crate) const LEAVE: &str = "view-leave";

impl Leaving {
    fn text(self) -> &'static str {
        match self {
            Leaving::Gone => "its home is gone",
            Leaving::Asked => "setup removed every agent",
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
    let started = crate::db::now_ms();
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
            started,
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
            started: 0,
            cwd,
            port: AtomicU16::new(port),
            next: Mutex::new(None),
            token,
            saving: Mutex::new(()),
            moving: Mutex::new(()),
            recovery: crate::settings::recovery::Recovery::default(),
            maintenance: crate::settings::maintenance::Maintenance::default(),
            embedding: crate::settings::embedding::Embedding::default(),
            agents: crate::setup::agents::Agents::default(),
            opener: Mutex::new(None),
            live: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            vectors: Arc::new(AtomicUsize::new(0)),
            files: std::sync::OnceLock::new(),
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
    /// is another than `lock`, or none), setup asks it to (`LEAVE`), `[view] port` names another
    /// port, or config.toml loads, does not say `resident = true`, and no request came since the
    /// last look (`quiet`). A file that does not load leaves it as it is.
    fn leaving(&self, lock: crate::worker::FileId, quiet: bool) -> Option<Leaving> {
        let state = self.home.join("state");
        if crate::worker::file_id(std::fs::metadata(state.join("view.lock"))) != lock {
            return Some(Leaving::Gone);
        }
        // A request written before this viewer took its lock was another's, or one a viewer did
        // not take: it is taken away, not followed (CodeRabbit on #618). One with no time in it is
        // as old.
        if let Ok(text) = std::fs::read_to_string(state.join(LEAVE)) {
            if text
                .trim()
                .parse::<i64>()
                .is_ok_and(|at| at >= self.started)
            {
                return Some(Leaving::Asked);
            }
            let _ = std::fs::remove_file(state.join(LEAVE));
        }
        if crate::config::view(&self.home).is_ok_and(|v| v.port.get() != self.port()) {
            return Some(Leaving::Moved);
        }
        (quiet && crate::config::worker(&self.home).is_ok_and(|w| !w.resident))
            .then_some(Leaving::NotResident)
    }

    fn serve(&self, mut stream: TcpStream) {
        // W1: this request's texts are checked with one clone of each rule's regex, let go when
        // the request is answered (`redact::Warm`).
        let _warm = crate::redact::warm();
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
            Head::Answer(_) | Head::File(_) => 0,
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
            Head::File(ask) => return self.send_file(&mut stream, &ask, head_only),
        };
        send(&mut stream, &resp.bytes(head_only), ANSWER_TIME);
    }

    /// The secret of this viewer's links to copies, made once.
    fn files_key(&self) -> Result<&[u8; 32]> {
        if let Some(key) = self.files.get() {
            return Ok(key);
        }
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("{e}"))?;
        // Two first uses at once both make one; the first set is the one both read.
        let _ = self.files.set(key);
        self.files
            .get()
            .ok_or_else(|| anyhow::anyhow!("the links' secret"))
    }

    /// A copy's link tag: HMAC-SHA256 of its provenance under this viewer's secret, cut to 16
    /// bytes (docs/deliverables.md, slice 2).
    fn file_mac(&self, row: &crate::deliverables::Row) -> Result<FilesMac> {
        use hmac::Mac;
        let mut mac =
            FilesMac::new_from_slice(self.files_key()?).map_err(|e| anyhow::anyhow!("{e}"))?;
        mac.update(&crate::deliverables::provenance(row));
        Ok(mac)
    }

    /// The tag a copy's link carries, hex.
    fn file_tag(&self, row: &crate::deliverables::Row) -> Result<String> {
        use hmac::Mac;
        let tag = self.file_mac(row)?.finalize().into_bytes();
        Ok(tag[..16].iter().map(|b| format!("{b:02x}")).collect())
    }

    /// This viewer's links' instance: what the page compares to know its links still open. Made
    /// from the secret, so it changes with it and tells nothing of it.
    fn files_instance(&self) -> Result<String> {
        let hash = Sha256::new()
            .chain_update(b"oboete files instance\0")
            .chain_update(self.files_key()?)
            .finalize();
        Ok(hash[..8].iter().map(|b| format!("{b:02x}")).collect())
    }

    /// A link to a copy: the same refusals as `route`'s reads, a cross-site fetch refused too (as
    /// claude-mem's), and no token, the link being its own capability. The name after the tag is
    /// for the browser alone.
    fn file_head(&self, method: &str, rest: &str, headers: &[(&str, &str)]) -> Head {
        let deadline = Instant::now() + FILE_TIME;
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
        };
        let refuse = |status, msg| Head::Answer(Response::text(status, msg));
        if method != "GET" && method != "HEAD" {
            return refuse(405, "method not allowed");
        }
        if header("transfer-encoding").is_some() || header("content-length").is_some() {
            return refuse(400, "requests carry no body");
        }
        if !self.host_ok(header("host")) {
            return refuse(403, "open the viewer through 127.0.0.1 or localhost");
        }
        if header("sec-fetch-site").is_some_and(|s| s.eq_ignore_ascii_case("cross-site")) {
            return refuse(403, "a copy opens from this computer's viewer only");
        }
        let tag = rest.split(['/', '?']).next().unwrap_or("");
        let Some(tag) = parse_tag(tag) else {
            return refuse(404, "not found");
        };
        Head::File(FileAsk {
            tag,
            range: header("range").map(str::to_owned),
            deadline,
        })
    }

    /// The copy a link names, opened and checked (`deliverables::open_copy`); none for a tag no
    /// row has, or a copy that is not as its row says.
    fn copy_for(&self, ask: &FileAsk) -> Result<Option<(std::fs::File, crate::deliverables::Row)>> {
        use hmac::Mac;
        let Some(db) = crate::deliverables::open_read(&self.home)? else {
            return Ok(None);
        };
        let left = ask.deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!left.is_zero(), "out of time");
        db.busy_timeout(left.min(Duration::from_secs(2)))?;
        let rows = crate::deliverables::rows(&db)?;
        drop(db);
        for row in rows {
            anyhow::ensure!(Instant::now() < ask.deadline, "out of time");
            if self.file_mac(&row)?.verify_truncated_left(&ask.tag).is_ok() {
                // Not as its row says: none (404); not read in time or at all: an error (503).
                let file = crate::deliverables::open_copy(&self.home, &row, ask.deadline)?;
                if file.is_none() {
                    eprintln!("oboete: copy {} is not as its row says: not served", row.id);
                }
                return Ok(file.map(|file| (file, row)));
            }
        }
        Ok(None)
    }

    /// Sends a copy (docs/deliverables.md, slice 2), all of it within the ask's deadline. Its
    /// records' denials are read last, with raw.db open and the readers' hold taken (the order a
    /// restore and a forget's registration take them), right before the head goes: a forget
    /// registered before then refuses the link; one after waits for the hold, and the stream it
    /// let in may finish, reading the bytes it was let in with.
    fn send_file(&self, stream: &mut TcpStream, ask: &FileAsk, head_only: bool) {
        use std::io::{Seek, SeekFrom};
        let left = || ask.deadline.saturating_duration_since(Instant::now());
        // A refusal is still written when the deadline made it: a second at least.
        let refuse = |stream: &mut TcpStream, status: u16, msg: &str| {
            send(
                stream,
                &Response::text(status, msg).bytes(head_only),
                left().clamp(Duration::from_secs(1), ANSWER_TIME),
            );
        };
        let (mut file, row) = match self.copy_for(ask) {
            Ok(Some(found)) => found,
            Ok(None) => return refuse(stream, 404, "not found"),
            Err(e) => {
                eprintln!("oboete: a copy could not be looked up: {e:#}");
                return refuse(stream, 503, "the copy cannot be served now");
            }
        };
        let len = row.bytes;
        let range = if head_only {
            None
        } else {
            byte_range(ask.range.as_deref(), len)
        };
        let (status, first, count) = match range {
            None => (200, 0, len),
            Some(Ok((first, last))) => (206, first, last - first + 1),
            Some(Err(())) => (416, 0, 0),
        };
        let admitted = match self.admit(&row, ask.deadline) {
            Ok(Some(admitted)) => admitted,
            Ok(None) => return refuse(stream, 404, "not found"),
            Err(_) => return refuse(stream, 503, "the copy cannot be served now"),
        };
        let sent = send(
            stream,
            &copy_head(status, &row.copy, first, count, len),
            left(),
        );
        drop(admitted);
        if !sent || head_only || status == 416 || file.seek(SeekFrom::Start(first)).is_err() {
            return;
        }
        let mut buf = vec![0u8; 64 << 10];
        let mut todo = count;
        while todo > 0 {
            let want = buf.len().min(usize::try_from(todo).unwrap_or(usize::MAX));
            let n = match file.read(&mut buf[..want]) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            if !send(stream, &buf[..n], left()) {
                return;
            }
            todo -= n as u64;
        }
    }

    /// A copy's admission (docs/deliverables.md, slice 2): raw.db open, then the readers' hold
    /// (the order a restore and a forget's registration take them), and its records' denials read
    /// in that hold, each wait within what is left before `deadline`; none when they deny it, or
    /// when there are no records to read them from. The caller sends the head while it keeps
    /// both, and lets them go before the body.
    fn admit(
        &self,
        row: &crate::deliverables::Row,
        deadline: Instant,
    ) -> Result<Option<(crate::raw::ReadOnly, std::fs::File)>> {
        let left = || {
            let left = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(!left.is_zero(), "out of time");
            Ok(left)
        };
        let Some(raw) = crate::raw::read_only_until(&self.home, Some(deadline))? else {
            return Ok(None);
        };
        let hold = crate::dispatch::reading(&self.home, left()?)?;
        let denied = crate::deliverables::denied_by(row, |seq, ts| {
            raw.conn.busy_timeout(left()?)?;
            let (device, agent, session) = (&row.device, &row.agent, &row.session);
            crate::raw::forgets_in(
                &raw.conn,
                device,
                agent,
                session,
                row.repo.as_deref(),
                seq,
                ts,
            )
        })?;
        Ok((!denied).then_some((raw, hold)))
    }

    /// Typed operations with a body go through `save_gate`; other requests use `route`.
    fn head(&self, method: &str, target: &str, headers: &[(&str, &str)]) -> Head {
        if let Some(rest) = target.strip_prefix("/files/") {
            return self.file_head(method, rest, headers);
        }
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
            ("POST", "/api/providers/models") => (MAX_BODY, |v, b, _| v.provider_models(b)),
            ("POST", "/api/key") => (MAX_KEY_BODY, |v, b, _| v.save_key(b)),
            ("POST", "/api/embedding/preview") => (MAX_BODY, |v, b, _| v.embedding_preview(b)),
            ("POST", "/api/embedding") => (MAX_BODY, Self::embedding_start),
            ("POST", "/api/embedding/key") => (MAX_KEY_BODY, Self::save_embedding_key),
            ("POST", "/api/judge/key") => (MAX_KEY_BODY, Self::save_judge_key),
            ("POST", "/api/judge/test") => (MAX_BODY, |v, b, _| v.test_judge(b)),
            ("POST", "/api/resume") => (MAX_BODY, |v, b, _| v.resume(b)),
            ("POST", "/api/privacy/exclude") => (MAX_BODY, |v, b, _| v.exclude(b)),
            ("POST", "/api/privacy/capture") => (MAX_BODY, |v, b, _| v.capture(b)),
            ("POST", "/api/claims/correct") => (MAX_BODY, |v, b, _| v.claim_correct(b)),
            ("POST", "/api/claims/mute") => (MAX_BODY, |v, b, _| v.claim_mute(b)),
            ("POST", "/api/forget/preview") => (MAX_BODY, |v, b, _| v.forget_preview(b)),
            ("POST", "/api/forget/start") => (MAX_BODY, |v, b, _| v.forget_start(b)),
            ("POST", "/api/hide") => (MAX_BODY, |v, b, _| v.hide(b)),
            ("POST", "/api/preferences") => (MAX_BODY, |v, b, _| v.preference(b)),
            ("POST", "/api/my-work/review") => (MAX_BODY, |v, b, _| v.my_work_review(b)),
            ("POST", "/api/my-work/finished") => (MAX_BODY, |v, b, _| v.my_work_finished(b)),
            ("POST", "/api/my-work/refresh") => (MAX_BODY, |v, b, _| v.my_work_refresh(b)),
            ("POST", "/api/my-work/suggestion") => (MAX_BODY, |v, b, _| v.my_work_suggestion(b)),
            ("POST", "/api/my-rules") => (MAX_BODY, |v, b, _| v.my_rules(b)),
            ("POST", "/api/my-focus") => (MAX_BODY, |v, b, _| v.my_focus(b)),
            ("POST", "/api/maintenance/preview") => (MAX_BODY, |v, b, _| v.maintenance_preview(b)),
            ("POST", "/api/setup/preview") => (MAX_BODY, |v, b, _| v.agent_preview(b)),
            ("POST", "/api/setup/start") => (MAX_BODY, |v, b, _| v.agent_start(b)),
            ("POST", "/api/maintenance/start") => (MAX_BODY, |v, b, _| v.maintenance_start(b)),
            ("POST", "/api/view/resident") => (MAX_BODY, |v, b, _| v.view_resident(b)),
            ("POST", "/api/view/token") => (MAX_BODY, Self::view_token),
            ("POST", "/api/prompt-context") => (PROMPT_BODY, |v, b, _| v.prompt_context(b)),
            ("POST", "/api/query-vector") => (PROMPT_BODY, |v, b, _| v.query_vector(b)),
            ("POST", "/api/lookup") => (PROMPT_BODY, |v, b, _| v.lookup(b)),
            _ => return Head::Answer(self.route(method, target, headers)),
        };
        // `save_gate` passes a request with exactly one, the viewer's.
        let given = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("x-oboete-token"))
            .map_or("", |(_, t)| t);
        // The prompt hook is no browser and sends no Origin: its request reads as the page's
        // search does, which needs none either, and the token header it carries is what a page of
        // another site cannot send (a custom header needs a preflight, and OPTIONS is 405).
        let browser = !matches!(
            target,
            "/api/prompt-context" | "/api/query-vector" | "/api/lookup"
        );
        match self.save_gate(headers, cap, browser) {
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
        browser: bool,
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
        // The prompt hook's request says nothing of the page: the opener page stays for it.
        if browser {
            self.token_arrived();
        }
        let origin = only("origin").and_then(|o| o.strip_prefix("http://"));
        if browser && !self.host_ok(origin) {
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

    /// G23 (docs/prompt-context.md): the meaning side of a prompt's block, for the prompt hook:
    /// the keys near the prompt in its checkout's repository, with their cosines, and whether a
    /// vector was made. The hook reads, gates and shows the documents itself.
    fn prompt_context(&self, body: &[u8]) -> Response {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Asked {
            repo: String,
            prompt: String,
            /// The agent and session whose stored shown set takes no place in the answer.
            #[serde(default)]
            agent: Option<String>,
            #[serde(default)]
            session: Option<String>,
            /// The keys of a manifest the hook shows in this call, before it stores them.
            #[serde(default)]
            shown: Vec<String>,
            /// Decision 52: the checkout's claude-mem names (`import::names_of`).
            #[serde(default)]
            projects: Vec<String>,
        }
        let asked = match serde_json::from_slice::<Asked>(body) {
            Ok(a)
                if !a.repo.is_empty()
                    && !a.prompt.trim().is_empty()
                    && a.agent
                        .as_deref()
                        .is_none_or(|g| crate::setup::AGENTS.contains(&g))
                    && a.session
                        .as_deref()
                        .is_none_or(|s| (1..=512).contains(&s.len()))
                    && a.shown.len() <= search::b::PROMPT_SHOWN
                    && a.shown.iter().all(|k| k.len() <= 256)
                    && a.projects.len() <= PROMPT_NAMES
                    && a.projects
                        .iter()
                        .all(|p| (1..=crate::import::NAME_BYTES).contains(&p.len())) =>
            {
                a
            }
            _ => return Response::text(400, "a prompt context takes a repository and a prompt"),
        };
        let mut shown: std::collections::HashSet<String> = asked.shown.into_iter().collect();
        if let (Some(agent), Some(session)) = (&asked.agent, &asked.session) {
            let stored = crate::hook::shown_set(&self.home, agent, session);
            shown.extend(
                stored
                    .into_iter()
                    .filter(|(_, e)| e["body"] == true)
                    .map(|(key, _)| key),
            );
        }
        match search::b::prompt_near(
            &self.home,
            (&asked.repo, &asked.projects),
            &asked.prompt,
            &shown,
        ) {
            Ok(near) => Response::json(&json!({
                "vector": match near.vector {
                    search::b::Vector::Used => json!("used"),
                    search::b::Vector::Skipped(s) => json!(s),
                },
                "hits": near.hits,
            })),
            Err(e) => failed(&e),
        }
    }

    /// N5 (docs/memory-lookup.md): the query vector of a memory lookup's words, which the hook
    /// searches with itself; or the reason for none (`search::b::VectorSkip`).
    fn query_vector(&self, body: &[u8]) -> Response {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Asked {
            repo: String,
            text: String,
        }
        let asked = match serde_json::from_slice::<Asked>(body) {
            Ok(a) if !a.repo.is_empty() && !a.text.trim().is_empty() => a,
            _ => return Response::text(400, "a query vector takes a repository and a text"),
        };
        match search::b::query_vector(&self.home, &asked.repo, &asked.text) {
            Ok(Ok(vector)) => Response::json(&json!({ "vector": vector })),
            Ok(Err(skip)) => Response::json(&json!({ "skip": skip })),
            Err(e) => failed(&e),
        }
    }

    /// Decision 71 (docs/memory-lookup.md): an agent's own memory lookup answered here, as
    /// claude-mem's hook asks its worker: the answer the hook would give
    /// (`memory_lookup::index_answer`), with this viewer's vector for its words when it makes one,
    /// gated and within `room`; `{"text": null}` when nothing is found or fits. The hook decides
    /// first that the call is a lookup it may answer.
    fn lookup(&self, body: &[u8]) -> Response {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Asked {
            repo: String,
            query: String,
            room: usize,
        }
        let asked = match serde_json::from_slice::<Asked>(body) {
            // The hook's query is at most 1,024 bytes (`memory_lookup::bound`), its room the
            // hook's note room.
            Ok(a)
                if !a.repo.is_empty()
                    && !a.query.trim().is_empty()
                    && a.query.len() <= 1024
                    && a.room <= crate::hook::NOTE_ROOM =>
            {
                a
            }
            _ => return Response::text(400, "a lookup takes a repository, its words and a room"),
        };
        let rules = match crate::capture::Settings::load(&self.home) {
            Ok(settings) => settings.rules,
            Err(e) => return failed(&e),
        };
        let wait = match crate::config::inject(&self.home) {
            Ok(inject) => Duration::from_millis(inject.per_prompt_wait_ms as u64),
            Err(e) => return failed(&e),
        };
        // The meaning side within `[inject] per_prompt_wait_ms`, as the hook waits for the vector
        // it asks (G23, Codex on #830); a vector later than that is not waited for, and the words
        // side answers alone, as it does while `LOOKUP_VECTORS` are still being made.
        let (home, repo, query) = (self.home.clone(), asked.repo.clone(), asked.query.clone());
        let vector = vector_within(
            &self.vectors,
            wait,
            move || search::b::query_vector(&home, &repo, &query).ok()?.ok(),
            |work| std::thread::Builder::new().spawn(work).map(drop),
        );
        match crate::memory_lookup::index_answer(
            &self.home,
            &asked.repo,
            &asked.query,
            vector.as_deref(),
            &rules,
            asked.room,
        ) {
            Ok(text) => Response::json(&json!({ "text": text })),
            Err(e) => failed(&e),
        }
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

    /// Decision 78: an entry's model list, ids only.
    fn provider_models(&self, body: &[u8]) -> Response {
        saved(crate::settings::provider_models(&self.home, body))
    }

    /// A key written to its entry's key file (#94 part 3); the answer never holds it.
    fn save_key(&self, body: &[u8]) -> Response {
        saved(crate::settings::save_key(&self.home, &self.saving, body))
    }

    fn embedding_preview(&self, body: &[u8]) -> Response {
        saved(self.embedding.preview(&self.home, body))
    }

    /// The choice taken; for `local` the answer waits for the download, while the page reads
    /// its progress from `GET /api/embedding` on another connection. Its token is checked again
    /// under config.lock, as a settings save's is.
    fn embedding_start(&self, body: &[u8], given: &str) -> Response {
        let authorized = || self.token_ok(Some(given));
        saved(
            self.embedding
                .start(&self.home, &self.saving, &authorized, body),
        )
    }

    /// The Workers AI token written to a new key file (W4); the answer never holds it. The
    /// request's token is checked again under config.lock.
    fn save_embedding_key(&self, body: &[u8], given: &str) -> Response {
        let authorized = || self.token_ok(Some(given));
        saved(crate::settings::embedding::save_key(
            &self.home,
            &self.saving,
            &authorized,
            body,
        ))
    }

    /// The judge's key typed on the page (docs/my-work-model.md J1); the answer never holds it.
    fn save_judge_key(&self, body: &[u8], given: &str) -> Response {
        let authorized = || self.token_ok(Some(given));
        saved(crate::settings::judge::save_key(
            &self.home,
            &self.saving,
            &authorized,
            body,
        ))
    }

    fn test_judge(&self, body: &[u8]) -> Response {
        saved(crate::settings::judge::test(&self.home, body))
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

    fn capture(&self, body: &[u8]) -> Response {
        saved(crate::settings::privacy::capture(
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

    /// V3, R3: an approve, reject or undo of a rule suggestion.
    fn my_rules(&self, body: &[u8]) -> Response {
        saved(crate::settings::claims::rule(
            &self.home,
            &self.saving,
            body,
        ))
    }

    /// F6: the owner's action on My Focus.
    fn my_focus(&self, body: &[u8]) -> Response {
        saved(crate::my_work_focus::act(&self.home, &self.saving, body))
    }

    /// J4: the owner's Join or Not this on a judge's suggestion.
    fn my_work_suggestion(&self, body: &[u8]) -> Response {
        saved(crate::my_work_judge::act(&self.home, &self.saving, body))
    }

    /// Parity row 3: what forgetting an item the page shows takes, and the key its start brings.
    fn forget_preview(&self, body: &[u8]) -> Response {
        saved(crate::settings::forget::preview(&self.home, body))
    }

    fn forget_start(&self, body: &[u8]) -> Response {
        saved(crate::settings::forget::start(
            &self.home,
            &self.saving,
            body,
        ))
    }

    /// Parity row 3: a card or a summary removed from the page, or brought back.
    fn hide(&self, body: &[u8]) -> Response {
        saved(crate::settings::hide::save(&self.home, &self.saving, body))
    }

    fn preference(&self, body: &[u8]) -> Response {
        saved(crate::settings::claims::pref_add(
            &self.home,
            &self.saving,
            body,
        ))
    }

    /// docs/my-work.md rule 6: `{request, version, checked}`, a check of the version of a request
    /// as the page saw it, or its Undo. The request is found by its id as this route shows it; a
    /// version that is not its current one, or a check of one not reported complete, is refused
    /// with 409, and the page reads My Work again.
    /// Decision 70 (parity row X14): Refresh now. The body is `{}`; the answer is the status
    /// line's state when nothing can run, else `nothing_due`, or `started` with how many subjects
    /// the worker takes at its next round.
    fn my_work_refresh(&self, body: &[u8]) -> Response {
        if !empty_object(body) {
            return refused(400, "bad_request", "");
        }
        match crate::my_work_run::request_refresh(&self.home, crate::db::now_ms()) {
            Ok(answer) => Response::json(&answered("my-work-refresh", answer)),
            Err(e) => failed(&e),
        }
    }

    /// Slice 2 of docs/subagents.md: Mark finished. The body is `{"id"}`, a subagent's id as the
    /// page has it; under its own lock and the readers' hold, from a fresh read to the write:
    /// 404 when it names no listed subagent, 409 `finished` when it is Finished already, 409
    /// `not_recorded` when its conversation's folder is gone or masked or records nothing now,
    /// 503 `busy` when another Mark finished holds the lock past the wait.
    /// The page reads My Work again after it, which counts the new stop at once.
    fn my_work_finished(&self, body: &[u8]) -> Response {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Posted {
            id: String,
        }
        let posted = match serde_json::from_slice::<Posted>(body) {
            Ok(p) if p.id.starts_with("s:") && p.id.len() <= 64 => p,
            _ => return refused(400, "bad_request", ""),
        };
        let _lock = match crate::my_work::finished_lock(&self.home, crate::db::OPEN_WRITE_WAIT) {
            Ok(Some(lock)) => lock,
            Ok(None) => return refused(503, "busy", ""),
            Err(e) => return failed(&e),
        };
        // raw.db first, then the readers' hold, the order a restore takes them in (Codex on
        // slice 2); the same handle reads and appends.
        let (mut raw, k) = match crate::search::b::stores(&self.home) {
            Ok(Some(stores)) => stores,
            Ok(None) => return refused(404, "not_found", ""),
            Err(e) => return failed(&e),
        };
        let _read = match crate::dispatch::reading(&self.home, crate::db::OPEN_WRITE_WAIT) {
            Ok(hold) => hold,
            Err(e) => return failed(&e),
        };
        let (found, folder) = match crate::my_work::subagent(&self.home, (&raw, &k), &posted.id) {
            Ok(Some(found)) => found,
            Ok(None) => return refused(404, "not_found", ""),
            Err(e) => return failed(&e),
        };
        if found.finished.is_some() {
            return refused(409, "finished", "");
        }
        let Some(folder) = folder else {
            return refused(409, "not_recorded", "");
        };
        match crate::my_work_subagents::mark(
            &self.home,
            &mut raw,
            &found,
            &folder,
            crate::db::now_ms(),
        ) {
            Ok(true) => {
                // As a hook starts one after it writes (Codex and cubic on #850), reaped when it
                // leaves, or it sits as a zombie for as long as the viewer runs (cubic on #850).
                match crate::hook::start_worker(&self.home) {
                    Ok(Some(mut child)) => {
                        std::thread::spawn(move || child.wait());
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("oboete: worker not started: {e:#}"),
                }
                Response::json(&json!({}))
            }
            Ok(false) => refused(409, "not_recorded", ""),
            Err(e) => failed(&e),
        }
    }

    fn my_work_review(&self, body: &[u8]) -> Response {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Posted {
            request: String,
            version: String,
            checked: bool,
        }
        let hex =
            |v: &str| v.len() == 64 && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        let posted = match serde_json::from_slice::<Posted>(body) {
            Ok(p)
                if !p.request.is_empty()
                    && p.request.chars().count() <= 2_000
                    && hex(&p.version) =>
            {
                p
            }
            _ => return refused(400, "bad_request", ""),
        };
        let now = crate::db::now_ms();
        // Held from the read the check is tested against to its write (`ReviewsLock`).
        let lock = match crate::my_work::reviews_lock(&self.home) {
            Ok(lock) => lock,
            Err(e) => return failed(&e),
        };
        let (answer, held) = match crate::my_work::reviewing(&self.home, now) {
            Ok(read) => read,
            Err(e) => return failed(&e),
        };
        let answer = answered("my-work", answer);
        let requests = answer["requests"].as_array().map_or(&[][..], Vec::as_slice);
        let Some(i) = requests
            .iter()
            .position(|r| r["id"] == posted.request.as_str())
        else {
            return refused(404, "not_found", "");
        };
        if requests[i]["version"] != posted.version.as_str() {
            return refused(409, "stale", "");
        }
        if posted.checked && requests[i]["status"] != "ready" {
            return refused(409, "not_ready", "");
        }
        match crate::my_work::set_review(
            (&self.home, &lock),
            (&held[i], &held),
            &posted.version,
            posted.checked,
            now,
        ) {
            Ok(()) => Response::json(&json!({})),
            Err(e) => failed(&e),
        }
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
            "/favicon.ico" => return Response::new(200, "image/x-icon", FAVICON_ICO),
            "/favicon.svg" => return Response::new(200, "image/svg+xml", FAVICON_SVG),
            "/apple-touch-icon.png" => return Response::new(200, "image/png", TOUCH_ICON),
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
        // config.toml's names, not stored text: answered as the settings are, not through the
        // stores' gate, whose masking rule could change a name that matches one (69b-3c-1b).
        if name == "rests" {
            return if query.is_empty() {
                Response::json(&crate::settings::rests(&self.home))
            } else {
                Response::text(400, "rests carry no query")
            };
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
        if name == "embedding" {
            return if query.is_empty() {
                Response::json(&self.embedding.status(&self.home))
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
            // Parity row 3: the cards and summaries removed from the page, to bring back.
            "hidden" => {
                let after = match hidden_page(q) {
                    Ok(after) => after,
                    Err(bad) => return Ok(bad),
                };
                match crate::settings::hide::list(&self.home, after) {
                    Ok(listed) => listed,
                    Err(refused) => return Ok(saved(Err(refused))),
                }
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
                // docs/page.md S4: a session's page, by its key (`session_key`).
                let session = arg("session");
                if session.is_some_and(|key| {
                    key.len() != 16 || !key.bytes().all(|b| b.is_ascii_hexdigit())
                }) {
                    return Ok(bad("session"));
                }
                let repo = match (all || session.is_some(), arg("repo")) {
                    (true, _) => None,
                    (false, Some(r)) => Some(r.to_owned()),
                    // No checkout (the resident viewer, R8): every repository.
                    (false, None) => self.checkout()?.map(|(repo, _)| repo),
                };
                feed(&self.home, (repo.as_deref(), session), cursor, limit)?
            }
            // docs/page.md S2: the sessions hooks recorded, as claude-mem's Sessions tab lists them.
            "sessions" => {
                let (all, limit, before) = match (flag(q, "all"), limit(q, 50), session_page(q)) {
                    (Ok(all), Ok(limit), Ok(before)) => (all, limit.min(100), before),
                    (Err(bad), _, _) | (_, Err(bad), _) | (_, _, Err(bad)) => return Ok(bad),
                };
                let repo = match (all, arg("repo")) {
                    (true, _) => None,
                    (false, Some(r)) => Some(r.to_owned()),
                    (false, None) => self.checkout()?.map(|(repo, _)| repo),
                };
                sessions(&self.home, repo.as_deref(), before.as_ref(), limit)?
            }
            // X10 (docs/my-work.md): every repository's requests, as claude-mem's My Work shows them.
            // Each card with its deliverables, a copy's link made by this viewer (slice 2b).
            "my-work" => match crate::my_work::Range::of(q.get("range").map(String::as_str)) {
                Some(range) => {
                    let tag = |row: &crate::deliverables::Row| self.file_tag(row);
                    let now = crate::db::now_ms();
                    let mut answer = crate::my_work::listed(&self.home, range, now, &tag)?;
                    // The links' instance: a restarted viewer's links are new (docs/deliverables.md).
                    answer["instance"] = json!(self.files_instance()?);
                    answer
                }
                None => return Ok(bad("range")),
            },
            // V1 (docs/my-work-model.md): My Work's status line, a route of its own so that the
            // requests' answer stays as the page's held updates compare it.
            "my-work/status" => crate::my_work_run::status(
                &self.home,
                &crate::my_work_run::line_jobs(),
                crate::db::now_ms(),
            )?,
            // V2: the owner's goals, as claude-mem's My Goals shows them, with the goals job's
            // status line (V1).
            "my-goals" => {
                let now = crate::db::now_ms();
                let mut answer = crate::my_work_goals::page(&self.home, now)?;
                let goals = [crate::my_work_goals::NAME];
                answer["status"] = crate::my_work_run::status(&self.home, &goals, now)?;
                answer
            }
            // V3: the rule suggestions and the owner's rules, with the rules job's status line (V1).
            "my-rules" => {
                let now = crate::db::now_ms();
                let mut answer = crate::my_work_rules::page(&self.home, now)?;
                let rules = [crate::my_work_rules::NAME];
                answer["status"] = crate::my_work_run::status(&self.home, &rules, now)?;
                answer
            }
            // Slice 4 (docs/my-work.md): what one request produced, read when its page opens, by
            // the id the page has, as a check names it (`my_work_review`).
            "my-work/product" => {
                let Some(request) = arg("request").filter(|r| r.chars().count() <= 2_000) else {
                    return Ok(bad("request"));
                };
                let (answer, held) = crate::my_work::reviewing(&self.home, crate::db::now_ms())?;
                let answer = answered("my-work", answer);
                let requests = answer["requests"].as_array().map_or(&[][..], Vec::as_slice);
                let Some(i) = requests.iter().position(|r| r["id"] == request) else {
                    return Ok(Response::text(404, "no such request"));
                };
                let tag = |row: &crate::deliverables::Row| self.file_tag(row);
                let (products, unread) = crate::my_work::products(&self.home, &held[i], &tag)?;
                let mut out = json!({"products": products});
                // The copies not read: the page shows the rest and asks again.
                if unread {
                    out["copies_unread"] = json!(true);
                }
                out
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
            // A copy made or swept, and a viewer whose links changed (docs/deliverables.md, slice
            // 2), move the page's version too.
            "version" => {
                let mut answer = polled(&self.home)?;
                // A copies' store that does not read moves no version rather than stopping the
                // page's polls (the list and the products report it, less often than this).
                let files = (|| -> Result<String> {
                    Ok(match crate::deliverables::open_read(&self.home)? {
                        Some(db) => {
                            crate::deliverables::fingerprint(&crate::deliverables::rows(&db)?)
                        }
                        None => String::new(),
                    })
                })()
                .unwrap_or_else(|_| "unread".into());
                let v = answer["v"].as_str().unwrap_or("").to_owned();
                answer["v"] = json!(format!("{v}:{files}:{}", self.files_instance()?));
                answer
            }
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
        // As `oboete inject` prints it, which agents whose files are not copied read.
        let manifest = crate::hook::start_text_read(
            &self.home,
            raw.as_ref(),
            &repo,
            &branch,
            &session,
            &settings,
            true,
            false,
        )?;
        let text = crate::hook::joined(&self.home, raw.as_ref(), manifest.as_ref());
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
pub(crate) fn answered(name: &str, mut answer: Value) -> Value {
    // A list's position, its last item's time and ids, comes back as it went, as the feed's
    // cursor does: Settings' list of what is removed too (Codex on #672).
    let next = matches!(name, "feed" | "timeline" | "sessions" | "hidden")
        .then(|| answer.get_mut("next").map(Value::take))
        .flatten();
    // A My Work request's id leaves the gate's walk and comes back under its key: a masked id's
    // fingerprint is its selector, as a claim's uid is, so the gate takes only what is before it
    // (#570), known by the answer's own field, as no id's own text can pass for it (Codex and
    // Sourcery on #576). So does the id of each of its conversations (`sessions`), which is the
    // id of that conversation's own request (decision 55), and each work item's `stream`, the
    // fingerprint of a list the gate masks (CodeRabbit on #597), and the request's `version`, which
    // a check names (rule 6), and the id of the request a judge's suggestion names (J4) and of the
    // conversation it is about, as `sessions` has it (#777).
    type Taken = (
        Option<String>,
        Vec<Option<String>>,
        Vec<Option<Value>>,
        Option<Value>,
        (Option<String>, Option<String>),
    );
    let ids: Vec<Taken> = match answer.get_mut("requests") {
        Some(Value::Array(requests)) if name == "my-work" => (requests.iter_mut())
            .map(|request| {
                let sessions = match request.get_mut("sessions") {
                    Some(Value::Array(sessions)) => sessions.iter_mut().map(request_id).collect(),
                    _ => Vec::new(),
                };
                let streams = match request.get_mut("work") {
                    Some(Value::Array(work)) => (work.iter_mut())
                        .map(|w| w.as_object_mut().and_then(|w| w.remove("stream")))
                        .collect(),
                    _ => Vec::new(),
                };
                let version = request.as_object_mut().and_then(|r| r.remove("version"));
                let about = (request.get_mut("suggestion"))
                    .and_then(|s| s.get_mut("conversation"))
                    .and_then(request_id);
                let suggested = request.get_mut("suggestion").and_then(request_id);
                (
                    request_id(request),
                    sessions,
                    streams,
                    version,
                    (suggested, about),
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    // Slice 2 of docs/subagents.md: each subagent's conversation, by the id its request's
    // `sessions` give it, and its own id, a keyed print the page posts back, leave the walk and
    // come back as theirs do.
    let subagent_ids: Vec<(Option<String>, Option<Value>)> = match answer.get_mut("subagents") {
        Some(Value::Array(items)) if name == "my-work" => (items.iter_mut())
            .map(|s| {
                let conversation = s.get_mut("conversation").and_then(request_id);
                (conversation, s.as_object_mut().and_then(|s| s.remove("id")))
            })
            .collect(),
        _ => Vec::new(),
    };
    // My Focus's ids (F1), random and code's own, are selectors as a request's id is: an area's,
    // and the area each request sits in, leave the walk and come back (Codex on #746). A value no
    // code made goes through it.
    let take = |items: Option<&mut Value>, key: &str| -> Vec<Option<Value>> {
        match items {
            Some(Value::Array(items)) if name == "my-work" => (items.iter_mut())
                .map(|i| {
                    let i = i.as_object_mut()?;
                    crate::my_work_focus::is_id(i.get(key)?).then(|| i.remove(key))?
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let area_ids = take(answer.get_mut("areas"), "id");
    let placed = take(answer.get_mut("requests"), "focus");
    // The viewer's own values (docs/deliverables.md, slice 2): the instance of its links to
    // copies, each request's copies' fingerprint and each copy's link tag are selectors, so no
    // rule's mask merges two of them or breaks a link; a copy's title and path go through it.
    let instance = (name == "my-work")
        .then(|| answer.as_object_mut().and_then(|a| a.remove("instance")))
        .flatten();
    let own = |items: Option<&mut Value>, key: &str| -> Vec<Option<Value>> {
        match items {
            Some(Value::Array(items)) => (items.iter_mut())
                .map(|i| i.as_object_mut().and_then(|i| i.remove(key)))
                .collect(),
            _ => Vec::new(),
        }
    };
    let copies = match name {
        "my-work" => own(answer.get_mut("requests"), "copies"),
        _ => Vec::new(),
    };
    let files = match name {
        "my-work/product" => own(answer.get_mut("products"), "file"),
        _ => Vec::new(),
    };
    // Each card's deliverables' tags (slice 2b), a list in each request.
    let card_files: Vec<Vec<Option<Value>>> = match answer.get_mut("requests") {
        Some(Value::Array(requests)) if name == "my-work" => (requests.iter_mut())
            .map(|r| own(r.get_mut("deliverables"), "file"))
            .collect(),
        _ => Vec::new(),
    };
    // A session's key, a hash that names it in the page's requests (docs/page.md S2), leaves the
    // gate's walk and comes back as a cursor does, so no rule of the user's masks a selector
    // (CodeRabbit on #635).
    let keys: Vec<Option<Value>> = match answer.get_mut("sessions") {
        Some(Value::Array(sessions)) if name == "sessions" => (sessions.iter_mut())
            .map(|s| s.as_object_mut().and_then(|s| s.remove("key")))
            .collect(),
        _ => Vec::new(),
    };
    // My Rules' uids go back to the claim routes as selectors (#739).
    let mut answer = if matches!(name, "claim" | "search" | "timeline" | "doc" | "my-rules") {
        gated_with_ids(answer, true)
    } else {
        gated(answer)
    };
    if let Some(Value::Array(sessions)) = answer.get_mut("sessions") {
        for (session, key) in sessions.iter_mut().zip(keys) {
            if let (Value::Object(session), Some(key)) = (session, key) {
                session.insert("key".to_owned(), key);
            }
        }
    }
    if let Some(next) = next {
        answer["next"] = next;
    }
    if let Some(instance) = instance {
        answer["instance"] = instance;
    }
    for (key, list, values) in [
        ("id", "areas", area_ids),
        ("focus", "requests", placed),
        ("copies", "requests", copies),
        ("file", "products", files),
    ] {
        if let Some(Value::Array(items)) = answer.get_mut(list) {
            for (item, value) in items.iter_mut().zip(values) {
                if let (Value::Object(item), Some(value)) = (item, value) {
                    item.insert(key.to_owned(), value);
                }
            }
        }
    }
    if let Some(Value::Array(requests)) = answer.get_mut("requests") {
        for (request, files) in requests.iter_mut().zip(card_files) {
            if let Some(Value::Array(items)) = request.get_mut("deliverables") {
                for (item, file) in items.iter_mut().zip(files) {
                    if let (Value::Object(item), Some(file)) = (item, file) {
                        item.insert("file".to_owned(), file);
                    }
                }
            }
        }
    }
    if let Some(Value::Array(items)) = answer.get_mut("subagents") {
        for (item, (conversation, own)) in items.iter_mut().zip(subagent_ids) {
            if let Some(shown) = item.get_mut("conversation") {
                put_id(shown, conversation);
            }
            if let (Value::Object(item), Some(own)) = (item, own) {
                item.insert("id".to_owned(), own);
            }
        }
    }
    if let Some(Value::Array(requests)) = answer.get_mut("requests") {
        for (request, (id, sessions, streams, version, (suggested, about))) in
            requests.iter_mut().zip(ids)
        {
            if let Some(suggestion) = request.get_mut("suggestion") {
                if let Some(conversation) = suggestion.get_mut("conversation") {
                    put_id(conversation, about);
                }
                put_id(suggestion, suggested);
            }
            if let Some(Value::Array(shown)) = request.get_mut("sessions") {
                for (session, id) in shown.iter_mut().zip(sessions) {
                    put_id(session, id);
                }
            }
            if let Some(Value::Array(work)) = request.get_mut("work") {
                for (item, stream) in work.iter_mut().zip(streams) {
                    if let (Value::Object(item), Some(stream)) = (item, stream) {
                        item.insert("stream".to_owned(), stream);
                    }
                }
            }
            if let (Value::Object(request), Some(version)) = (&mut *request, version) {
                request.insert("version".to_owned(), version);
            }
            put_id(request, id);
        }
    }
    answer
}

/// A My Work id put back under its key, after the gate's walk.
fn put_id(v: &mut Value, id: Option<String>) {
    if let (Value::Object(v), Some(id)) = (v, id) {
        v.insert("id".to_owned(), Value::String(id));
    }
}

/// A My Work request's id, or its conversation's, taken out of it with its `fingerprint`, and
/// gated but for that.
fn request_id(request: &mut Value) -> Option<String> {
    let request = request.as_object_mut()?;
    let fingerprint = match request.remove("fingerprint") {
        Some(Value::String(print)) => Some(print),
        _ => None,
    };
    let Some(Value::String(id)) = request.remove("id") else {
        return None;
    };
    // A group's id is its fingerprint alone (docs/my-work-model.md C6): nothing to gate.
    if fingerprint
        .as_deref()
        .is_some_and(|print| id.strip_prefix("g:") == Some(print))
    {
        return Some(id);
    }
    let print = fingerprint.map_or_else(String::new, |print| format!("#{print}"));
    Some(match id.strip_suffix(print.as_str()) {
        Some(shown) => format!("{}{print}", redact::outbound(shown)),
        None => redact::outbound(&id),
    })
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
    notes: Option<NotePosition>,
}

/// A saved note's place in the feed: its time, then its op's device and seq.
type NotePosition = (i64, String, i64);

/// Parity row N3: the notes agents saved (`save_memory`), the newest first after `before`, at most
/// `limit`, as claude-mem's feed shows a manual memory among its observations: each note's newest
/// row, none a forget names, its title and text gated as every reader gates them. Whether another
/// remains.
fn notes_page(
    k: &rusqlite::Connection,
    raw: &crate::raw::Raw,
    repo: Option<&str>,
    before: Option<&NotePosition>,
    limit: usize,
    rules: &redact::Rules,
) -> Result<(Vec<(NotePosition, Value)>, bool)> {
    if !crate::consumer::manifest::exists(k, "table", "imported")? {
        return Ok((Vec::new(), false));
    }
    let (ts, device, seq) = before
        .cloned()
        .unwrap_or((i64::MAX, String::new(), i64::MAX));
    let mut st = k.prepare(
        "SELECT i.uid, i.repo, i.ts, i.title, i.body, i.op_device, i.op_seq FROM imported i
         WHERE i.session = ?1 AND i.source = ?2 AND (?3 IS NULL OR i.repo = ?3)
           AND (i.ts, i.op_device, i.op_seq) < (?4, ?5, ?6)
           AND i.rowid = (SELECT MAX(j.rowid) FROM imported j WHERE j.uid = i.uid)
         ORDER BY i.ts DESC, i.op_device DESC, i.op_seq DESC",
    )?;
    let mut rows = st.query(rusqlite::params![
        crate::raw::NOTE_SESSION,
        crate::raw::NOTE_SOURCE,
        repo,
        ts,
        device,
        seq
    ])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let uid: String = r.get(0)?;
        if raw.forgotten(&uid)? {
            continue;
        }
        if out.len() == limit {
            return Ok((out, true));
        }
        let (repo, ts): (String, i64) = (r.get(1)?, r.get(2)?);
        let gate = |s: String| redact::outbound_with(&s, rules);
        let name = crate::consumer::manifest::repo_name(&repo, rules);
        out.push((
            (ts, r.get(5)?, r.get(6)?),
            json!({
                "kind": "note", "id": uid, "ts": ts, "agent": null,
                "repo_name": name, "repo": repo,
                "title": gate(r.get(3)?), "subtitle": "", "narrative": gate(r.get(4)?),
                "facts": [], "concepts": [], "files_read": [], "files_modified": [],
            }),
        ));
    }
    Ok((out, false))
}

/// #661: where Settings' list of removed cards and summaries goes on, `before` as the list's last
/// answer gave it in `next`; it takes no other parameter, and a malformed one is refused before
/// any store is opened, as the feed's is.
fn hidden_page(
    q: &HashMap<String, String>,
) -> std::result::Result<crate::settings::hide::Position, Response> {
    if q.keys().any(|k| k != "before") {
        return Err(bad("query"));
    }
    let Some(cursor) = q.get("before").filter(|s| !s.is_empty()) else {
        return Ok(Default::default());
    };
    if cursor.len() > 4096 {
        return Err(bad("before"));
    }
    serde_json::from_str(cursor).map_err(|_| bad("before"))
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

/// A session's place in the list (docs/page.md S2): its start, then its first prompt's device
/// and seq.
type SessionPosition = (i64, String, i64);

/// docs/page.md S2 and S4: what names a session in the page's requests, 16 hex digits of a hash
/// of its agent and session: no label of it, and the same whichever of its prompts a forget took.
pub(crate) fn session_key(agent: &str, session: &str) -> String {
    let digest = Sha256::digest(format!("{agent}\0{session}"));
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn session_page(
    q: &HashMap<String, String>,
) -> std::result::Result<Option<SessionPosition>, Response> {
    let Some(cursor) = q.get("before").filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if cursor.len() > 4096 {
        return Err(bad("before"));
    }
    serde_json::from_str(cursor)
        .map(Some)
        .map_err(|_| bad("before"))
}

/// docs/page.md S2: the sessions hooks recorded in `repo` (every repository's when none), the
/// newest first after `before`, at most `limit`: each by its first prompt's key, with its agent,
/// session, repository, title, start and number of items; and the next page's place.
fn sessions(
    home: &Path,
    repo: Option<&str>,
    before: Option<&SessionPosition>,
    limit: usize,
) -> Result<Value> {
    let Some((raw, k)) = search::b::stores(home)? else {
        return Ok(json!({"sessions": [], "next": null}));
    };
    let rules = redact::Rules::load(home)?;
    let mut listed: Vec<_> = (raw.live_sessions()?.into_iter())
        .filter(|s| repo.is_none() || s.repo.as_deref() == repo)
        .filter(|s| {
            before.is_none_or(|b| (s.started, s.device.as_str(), s.seq) < (b.0, b.1.as_str(), b.2))
        })
        .collect();
    listed.sort_by(|a, b| (b.started, &b.device, b.seq).cmp(&(a.started, &a.device, a.seq)));
    let more = listed.len() > limit;
    listed.truncate(limit);
    // Each kind as the feed counts it: current cards of the session's own windows (K2), and its
    // summaries that are no skip.
    // Parity row 3: a removed card or summary counts no more than the feed shows it.
    let counts = |table: &str, sql: &str| -> Result<HashMap<(String, String), i64>> {
        if !crate::consumer::manifest::exists(&k, "table", table)? {
            return Ok(HashMap::new());
        }
        let sql = sql.replace("{shown}", &crate::hide::shown(&k, table, "")?);
        let mut st = k.prepare(&sql)?;
        let rows = st.query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    let cards = counts(
        "cards",
        "SELECT agent, session, COUNT(*) FROM cards
         WHERE replaced_by IS NULL AND agent IS NOT NULL AND session IS NOT NULL {shown}
         GROUP BY agent, session",
    )?;
    let turns = counts(
        "turns",
        "SELECT agent, session, COUNT(*) FROM turns WHERE skipped = 0 {shown}
         GROUP BY agent, session",
    )?;
    let gate = |s: &str| redact::outbound_with(s, &rules);
    let mut out = Vec::new();
    for s in &listed {
        let key = (s.agent.clone(), s.session.clone());
        let items = s.prompts as i64
            + cards.get(&key).copied().unwrap_or(0)
            + turns.get(&key).copied().unwrap_or(0);
        // On one line, at most 100 characters, as a title is shown.
        let title = raw.prompt_text((&s.device, s.seq), &rules)?.map(|t| {
            let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
            match flat.char_indices().nth(100) {
                Some((at, _)) => format!("{}…", &flat[..at]),
                None => flat,
            }
        });
        out.push(json!({
            "key": session_key(&s.agent, &s.session), "agent": gate(&s.agent),
            "session": gate(&s.session), "repo": s.repo.as_deref().map(gate),
            "repo_name": crate::consumer::manifest::repo_name(s.repo.as_deref().unwrap_or(""), &rules),
            "title": title.filter(|t| !t.is_empty()), "started": s.started, "items": items,
        }));
    }
    let next = (listed.last())
        .filter(|_| more)
        .map(|s| json!([s.started, s.device, s.seq]).to_string());
    Ok(json!({"sessions": out, "next": next}))
}

/// The feed's four bounded pages, merged by own time, kind, then stored ID (newest first).
/// Every payload is already gated by its table's one reader; `api` gates the answer too.
fn feed(
    home: &Path,
    (repo, session): (Option<&str>, Option<&str>),
    mut cursor: FeedCursor,
    limit: usize,
) -> Result<Value> {
    let Some((raw, k)) = search::b::stores(home)? else {
        return Ok(json!({"items": [], "next": null}));
    };
    // A session's page (S4): the agent and session its key names; a key of no session hooks
    // recorded has no items.
    let named = match session {
        Some(key) => match (raw.live_sessions()?.into_iter())
            .find(|s| session_key(&s.agent, &s.session) == key)
        {
            Some(s) => Some((s.agent, s.session)),
            None => return Ok(json!({"items": [], "next": null})),
        },
        None => None,
    };
    let scope = (repo, named.as_ref().map(|(a, s)| (a.as_str(), s.as_str())));
    let rules = redact::Rules::load(home)?;
    let (cards, cards_more) =
        crate::cards::page(&k, &raw, scope, cursor.cards.as_ref(), limit, &rules)?;
    let (summaries, summaries_more) =
        crate::turns::page(&k, &raw, scope, cursor.summaries.as_ref(), limit, &rules)?;
    let (prompts, prompts_more) = raw.prompts(scope, cursor.prompts.as_ref(), limit, &rules)?;
    // A note is its repository's document, of no session's work: a session's page shows none.
    let (notes, notes_more) = if named.is_some() {
        (Vec::new(), false)
    } else {
        notes_page(&k, &raw, repo, cursor.notes.as_ref(), limit, &rules)?
    };
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
        // Whole, as claude-mem's feed sends it: the page clamps a long one (parity row X9).
        rows.push((2, (ts, device, seq, 0), json!({
            "kind": "prompt", "id": p.key(), "ts": p.ts,
            "agent": p.agent, "repo_name": name(p.repo.as_deref()), "repo": p.repo, "text": p.text,
        })));
    }
    for ((ts, device, seq), value) in notes {
        rows.push((3, (ts, device, seq, 0), value));
    }
    rows.sort_by(|(ak, a, _), (bk, b, _)| {
        b.0.cmp(&a.0)
            .then_with(|| ak.cmp(bk))
            .then_with(|| b.cmp(a))
    });
    let more = rows.len() > limit || cards_more || summaries_more || prompts_more || notes_more;
    rows.truncate(limit);
    let mut items = Vec::new();
    for (kind, (ts, device, seq, n), value) in rows {
        match kind {
            0 => cursor.cards = Some((ts, device, seq, n)),
            1 => cursor.summaries = Some((ts, device, seq)),
            2 => cursor.prompts = Some((ts, device, seq)),
            _ => cursor.notes = Some((ts, device, seq)),
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

/// Each repository's counts and newest time. Its records are counted from the Fts consumer's
/// index `raw_docs_repo`, not by reading each record's text (4.5 s cold on this PC's store,
/// 2026-11-01).
const REPOS: &str = "SELECT repo, SUM(claims), SUM(imported), SUM(records), MAX(last) FROM (
       SELECT repo, COUNT(DISTINCT uid) AS claims, 0 AS imported, 0 AS records,
              MAX(valid_from) AS last
       FROM derivations WHERE repo IS NOT NULL GROUP BY repo
       UNION ALL
       SELECT repo, 0, COUNT(DISTINCT uid), 0, MAX(ts) FROM imported GROUP BY repo
       UNION ALL
       SELECT repo, 0, 0, COUNT(*), MAX(ts) FROM raw_docs WHERE repo IS NOT NULL GROUP BY repo)
     GROUP BY repo ORDER BY MAX(last) DESC, repo";

#[cfg(test)]
thread_local! {
    /// How many times this thread counted the repositories: a test's look at `repos_in`.
    pub(crate) static REPOS_COUNTED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Existing repository enumeration on a caller-owned connection, without opening stores.
pub(crate) fn repos_in(k: &rusqlite::Connection) -> Result<Vec<Value>> {
    #[cfg(test)]
    REPOS_COUNTED.set(REPOS_COUNTED.get() + 1);
    let mut st = k.prepare(REPOS)?;
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

/// The names alone of the repositories `REPOS` lists: its three tables under its filters, with no
/// count, so each is read from an index of its own (#899 W2: 0.14 s where the counts took 3.1 s
/// cold on this PC's store, 2026-11-10). In no order.
const REPO_NAMES: &str = "SELECT repo FROM derivations WHERE repo IS NOT NULL
     UNION SELECT repo FROM imported
     UNION SELECT repo FROM raw_docs WHERE repo IS NOT NULL";

/// The repositories `repos_in` lists, by name only, on a caller-owned connection.
pub(crate) fn repo_names_in(k: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut st = k.prepare(REPO_NAMES)?;
    let rows = st.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// What the page asks every few seconds: the version of what it shows (`version`) and, while
/// curation is on, how many of this machine's records wait to be curated into cards and whether
/// curation waits for a provider or the owner (decision 67, row X27: claude-mem's queue beside its
/// logo). A count that does not read leaves them out: it never stops the page's poll.
fn polled(home: &Path) -> Result<Value> {
    let mut answer = json!({ "v": version(home)? });
    if let Ok(Some((waiting, held))) = waiting(home) {
        answer["waiting"] = waiting.into();
        answer["held"] = held.into();
    }
    Ok(answer)
}

/// How many of this device's live records past the curation checkpoint wait to be curated, as
/// session start's backlog counts them (`consumer::manifest`), and whether the window that starts
/// at the checkpoint is held (every provider failed it, or a budget or the owner holds it). None
/// while curation is off.
fn waiting(home: &Path) -> Result<Option<(i64, bool)>> {
    if !crate::config::load(home)?.summary.curate {
        return Ok(None);
    }
    let Some((raw, k)) = search::b::stores(home)? else {
        return Ok(None);
    };
    let device = raw.device();
    // As session start's backlog: no count while a tombstone the worker has not applied may
    // change the facts, so a forgotten record is never counted (cubic on #796).
    let manifest = crate::knowledge::checkpoint::get(&k, "manifest", device)?;
    if !crate::consumer::manifest::exists(&k, "table", "manifest_facts")?
        || !raw.tombstones_after(device, manifest)?.is_empty()
    {
        return Ok(None);
    }
    let curated = crate::curate::curated_through(&raw)?;
    let (checkpoint, part) = raw.curation_checkpoint(device)?;
    let waiting = k.query_row(
        "SELECT COUNT(*) FROM manifest_facts WHERE device = ?1 AND fact = 'event' AND seq > ?2",
        rusqlite::params![device, curated],
        |r| r.get(0),
    )?;
    let next = part.map_or((checkpoint + 1, 0), |o| (checkpoint, o));
    let held = match crate::providers_db::read_only(home)? {
        Some(db) => crate::providers_db::pending_of(&db, device)?.is_some_and(|p| {
            // As `curate::stopped` reads it: the owner's hold stands until the owner ends it.
            (p.from_seq, p.from_offset.unwrap_or(0)) == next
                && (p.hold == "owner" || p.next_attempt_at > crate::db::now_ms())
        }),
        None => false,
    };
    Ok(Some((waiting, held)))
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
    let rebuilding = crate::worker::rebuilding(home);
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
mod tests;
