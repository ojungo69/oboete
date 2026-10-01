//! `oboete view`: the memory in a browser, on 127.0.0.1. std `TcpListener` + `httparse`, one
//! thread per connection, one bundled page and a small JSON API over `search` (reads) plus two
//! delete endpoints and the settings page's save (#94), the one request with a body. Every
//! `/api` request carries the per-launch token, which the page reads from the URL fragment and
//! sends as a header. docs/m1.md decisions 11 and 14 have the reasons (tiny_http's open CVEs) and
//! the threat model; spec 6.6 has the save's.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// caller (`checkout`).
    cwd: PathBuf,
    port: u16,
    token: String,
    /// Settings saves, one at a time.
    saving: Mutex<()>,
    /// The page `--open` gave the browser opener, removed by the first request with the token.
    opener: Mutex<Option<PathBuf>>,
    /// Connections being served: at most `MAX_CONNECTIONS`.
    live: AtomicUsize,
}

/// One of `MAX_CONNECTIONS`, given back when its connection's thread ends, however it ends.
struct Slot(Arc<Viewer>);

impl Slot {
    fn take(v: &Arc<Viewer>) -> Option<Slot> {
        if v.live.fetch_add(1, Ordering::SeqCst) < MAX_CONNECTIONS {
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

/// A write that takes a request's body: the settings, or a key.
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

/// Serve until interrupted.
pub fn run(home: &Path, port: u16, open: bool) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| anyhow!("random token: {e}"))?;
    let viewer = Arc::new(Viewer {
        home: home.to_path_buf(),
        cwd: std::env::current_dir()?,
        port,
        token: raw.iter().map(|b| format!("{b:02x}")).collect(),
        saving: Mutex::new(()),
        opener: Mutex::new(None),
        live: AtomicUsize::new(0),
    });
    let url = format!("http://127.0.0.1:{port}/#t={}", viewer.token);
    println!("{url}\n(open it in a browser; Ctrl-C stops the viewer)");
    if open {
        viewer.open(home, &url, open_browser);
    }
    accept(&listener, &viewer);
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
    // One per port: another viewer of this home neither replaces nor removes it.
    let page = opener_dir(home).join(format!("view-open-{port}.html"));
    // Absolute: the opener, and a browser already running, resolve a relative path in their own
    // working directory.
    let page = std::path::absolute(&page).unwrap_or(page);
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
    fn serve(&self, mut stream: TcpStream) {
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

    /// The two saves go through `save_gate`; every other request is answered by `route`.
    fn head(&self, method: &str, target: &str, headers: &[(&str, &str)]) -> Head {
        let (cap, save): (usize, Save) = match (method, target) {
            ("POST", "/api/settings") => (MAX_BODY, Self::save),
            ("POST", "/api/key") => (MAX_KEY_BODY, Self::save_key),
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

    /// DNS rebinding: a page on another name that resolves to 127.0.0.1 sends its own Host.
    /// Browsers leave port 80 out of Host.
    fn host_ok(&self, host: Option<&str>) -> bool {
        host.is_some_and(|h| {
            h == format!("127.0.0.1:{}", self.port)
                || h == format!("localhost:{}", self.port)
                || (self.port == 80 && (h == "127.0.0.1" || h == "localhost"))
        })
    }

    fn token_ok(&self, given: Option<&str>) -> bool {
        Sha256::digest(given.unwrap_or("").as_bytes()) == Sha256::digest(self.token.as_bytes())
    }

    /// A request brought the token, on any path and whatever it asks: the browser has it, and
    /// the page `--open` took it there through has done its work.
    fn token_arrived(&self) {
        let page = self
            .opener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
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
    /// runs applies to its next search (D11).
    fn checkout(&self) -> Result<(String, String)> {
        let settings = crate::capture::Settings::load(&self.home)?;
        let cwd = self.cwd.to_string_lossy();
        let (_, repo, branch) = crate::capture::checkout(&json!({ "cwd": cwd }), &settings);
        Ok((repo, branch.unwrap_or_default()))
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
                query.caller = Some(self.checkout()?.0);
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
            "timeline" => {
                let (all, limit, before) = match (flag(q, "all"), limit(q, 50), page(q)) {
                    (Ok(all), Ok(limit), Ok(before)) => (all, limit, before),
                    (Err(bad), _, _) | (_, Err(bad), _) | (_, _, Err(bad)) => return Ok(bad),
                };
                let repo = match (all, arg("repo")) {
                    (true, _) => None,
                    (false, Some(r)) => Some(r.to_owned()),
                    (false, None) => Some(self.checkout()?.0),
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
            // Its text is SessionStart's, which `hook::start_text` gated itself.
            "context" => return Ok(Response::json(&self.context(arg("repo"))?)),
            "repos" => {
                let (current, branch) = self.checkout()?;
                json!({ "current": current, "branch": branch, "repos": repos(&self.home)? })
            }
            "version" => json!({ "v": version(&self.home)? }),
            "stats" => stats(&self.home)?,
            _ => return Ok(Response::text(404, "not found")),
        };
        Ok(Response::json(&gated(answer)))
    }

    /// The Context page: what SessionStart shows a new session in `repo`'s checkout (the viewer's
    /// own by default; another repository's at the branch of its newest manifest), as `oboete
    /// inject` joins it (D4).
    fn context(&self, repo: Option<&str>) -> Result<Value> {
        let on = crate::config::inject(&self.home).is_ok_and(|i| i.session_start);
        let (repo, branch) = match repo {
            None => self.checkout()?,
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
/// an import), an op the worker applied, a session started or a prompt typed. Other records (tool
/// calls, replies) do not move it, so a working agent does not redraw the page between prompts.
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
    Ok(format!("{}:{}:{}", raw.max_op_seq()?, applied.0, applied.1))
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
        let v = Viewer {
            home: dir.clone(),
            cwd: dir.clone(),
            port: 4321,
            token: "t0k".into(),
            saving: Mutex::new(()),
            opener: Mutex::new(None),
            live: AtomicUsize::new(0),
        };
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
        let v = Viewer {
            home: s.home.path().to_owned(),
            cwd: dir,
            port: 4321,
            token: "t0k".into(),
            saving: Mutex::new(()),
            opener: Mutex::new(None),
            live: AtomicUsize::new(0),
        };
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
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let c = crate::claims::active_one(&k, uid).unwrap().unwrap();
        let op = crate::claims::CorrectionOp {
            uid: uid.into(),
            anchor: crate::claims::Anchor {
                device: c.device,
                seq: c.seq,
            },
            status: status.map(Into::into),
            body: body.map(Into::into),
        };
        let op = serde_json::to_value(op).unwrap();
        s.raw
            .append_ops(&[(crate::raw::OpKind::Correction, op)])
            .unwrap();
        s.run();
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
        assert_ne!(get(&v, "/api/version")["v"], v0);
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
        let shown = crate::hook::inject_text(&v.home, &v.cwd, None);
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
            "capture": shown["capture"], "chain": chain}))
        .unwrap()
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

    #[test]
    fn query_params_decode() {
        let p = params("q=a+b%20c&repo=%2Fhome%2Fx&x");
        assert_eq!((p["q"].as_str(), p["repo"].as_str()), ("a b c", "/home/x"));
        assert!(!p.contains_key("x"));
    }
}
