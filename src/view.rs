//! `oboete view`: a read-only viewer on 127.0.0.1 for a browser. std `TcpListener` + `httparse`,
//! one thread per connection, one bundled page and a small JSON API over `search`. Every `/api`
//! request carries the per-launch token, which the page reads from the URL fragment and sends as a
//! header. docs/m1.md decision 11 has the reasons (tiny_http's open CVEs) and the threat model.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{db, repo, search};

const INDEX: &str = include_str!("../assets/viewer/index.html");
const APP_JS: &str = include_str!("../assets/viewer/app.js");
const APP_CSS: &str = include_str!("../assets/viewer/app.css");
/// Request line and headers; a browser's GET is well under 2 KB.
const MAX_HEAD: usize = 16 * 1024;
const MAX_LIMIT: usize = 200;
const SECURITY_HEADERS: &str = "Content-Security-Policy: default-src 'none'; script-src 'self'; \
    style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; \
    frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
    Cache-Control: no-store\r\n";

struct Viewer {
    home: PathBuf,
    /// The repository `oboete view` was started in: the page's default scope.
    cwd_repo: String,
    port: u16,
    token: String,
}

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
pub fn run(home: &Path, port: u16) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| anyhow!("random token: {e}"))?;
    let viewer = Arc::new(Viewer {
        home: home.to_path_buf(),
        cwd_repo: repo::key(&std::env::current_dir()?),
        port,
        token: raw.iter().map(|b| format!("{b:02x}")).collect(),
    });
    println!(
        "http://127.0.0.1:{port}/#t={}\n(open it in a browser; Ctrl-C stops the viewer)",
        viewer.token
    );
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let v = Arc::clone(&viewer);
        // A browser keeps idle pre-connected sockets open; one thread each keeps them from
        // stalling the rest.
        std::thread::spawn(move || v.serve(stream));
    }
    Ok(())
}

impl Viewer {
    fn serve(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = Vec::with_capacity(2048);
        let mut chunk = [0u8; 4096];
        let (resp, head_only) = loop {
            let n = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            let mut headers = [httparse::EMPTY_HEADER; 64];
            let mut req = httparse::Request::new(&mut headers);
            match req.parse(&buf) {
                Ok(httparse::Status::Complete(_)) => {
                    let method = req.method.unwrap_or("");
                    let pairs: Vec<(&str, &str)> = req
                        .headers
                        .iter()
                        .map(|h| (h.name, std::str::from_utf8(h.value).unwrap_or("")))
                        .collect();
                    break (
                        self.route(method, req.path.unwrap_or(""), &pairs),
                        method == "HEAD",
                    );
                }
                Ok(httparse::Status::Partial) if buf.len() < MAX_HEAD => continue,
                // Malformed, too many headers, or a head over the cap.
                _ => break (Response::text(400, "bad request"), false),
            }
        };
        let _ = stream.write_all(&resp.bytes(head_only));
    }

    fn route(&self, method: &str, target: &str, headers: &[(&str, &str)]) -> Response {
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
        };
        if method != "GET" && method != "HEAD" {
            return Response::text(405, "read-only viewer");
        }
        // No request here has a body; refusing framing headers outright leaves nothing to smuggle.
        if header("content-length").is_some() || header("transfer-encoding").is_some() {
            return Response::text(400, "requests carry no body");
        }
        // DNS rebinding: a page on another name that resolves to 127.0.0.1 sends its own Host.
        // Browsers leave port 80 out of Host.
        let host_ok = header("host").is_some_and(|h| {
            h == format!("127.0.0.1:{}", self.port)
                || h == format!("localhost:{}", self.port)
                || (self.port == 80 && (h == "127.0.0.1" || h == "localhost"))
        });
        if !host_ok {
            return Response::text(403, "open the viewer through 127.0.0.1 or localhost");
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match path {
            "/" => return Response::new(200, "text/html; charset=utf-8", INDEX),
            "/app.js" => return Response::new(200, "text/javascript; charset=utf-8", APP_JS),
            "/app.css" => return Response::new(200, "text/css; charset=utf-8", APP_CSS),
            "/favicon.ico" => return Response::new(204, "text/plain", ""),
            p if !p.starts_with("/api/") => return Response::text(404, "not found"),
            _ => {}
        }
        let given = header("x-oboete-token").unwrap_or("");
        if Sha256::digest(given.as_bytes()) != Sha256::digest(self.token.as_bytes()) {
            return Response::text(401, "missing or wrong token");
        }
        let q = params(query);
        match self.api(&path["/api/".len()..], &q) {
            Ok(r) => r,
            Err(e) => Response::text(500, &format!("{e:#}")),
        }
    }

    fn api(&self, name: &str, q: &HashMap<String, String>) -> Result<Response> {
        let conn = db::open(&self.home)?;
        let repo = q.get("repo").map(String::as_str).filter(|r| !r.is_empty());
        let limit = |default: usize| {
            q.get("limit")
                .and_then(|l| l.parse().ok())
                .unwrap_or(default)
                .min(MAX_LIMIT)
        };
        let id = q.get("id").map(String::as_str).unwrap_or("");
        let doc = |h: &search::Hit, text: &str| json!({"doc": h.doc, "kind": h.kind, "repo": h.repo, "when": h.when, "title": h.title, "text": text});
        Ok(Response::json(&match name {
            "repos" => {
                let repos: Vec<Value> = search::repos(&conn)?
                    .iter()
                    .map(|r| json!({"repo": r.repo, "sessions": r.sessions, "last": r.last}))
                    .collect();
                json!({"current": self.cwd_repo, "repos": repos})
            }
            "timeline" => {
                let rows: Vec<Value> = search::timeline(&conn, repo, limit(50))?
                    .iter()
                    .map(|r| json!({"id": r.id, "agent": r.agent, "repo": r.repo, "when": r.when, "summary": r.summary}))
                    .collect();
                json!(rows)
            }
            "session" => {
                let docs: Vec<Value> = search::session_docs(&conn, id)?
                    .iter()
                    .map(|h| doc(h, &h.body))
                    .collect();
                json!(docs)
            }
            "search" => {
                let query = q.get("q").map(String::as_str).unwrap_or("");
                let terms: Vec<&str> = query.split_whitespace().collect();
                let hits: Vec<Value> = search::search(&conn, query, repo, limit(50))?
                    .iter()
                    .map(|h| doc(h, &search::snippet(&h.body, &terms, 160)))
                    .collect();
                json!(hits)
            }
            "doc" => match search::get(&conn, id)? {
                Some(h) => doc(&h, &h.body),
                None => return Ok(Response::text(404, "no such document")),
            },
            _ => return Ok(Response::text(404, "not found")),
        }))
    }
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

    fn viewer(name: &str) -> (PathBuf, Viewer) {
        let dir = std::env::temp_dir().join(format!("oboete-view-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = db::open(&dir).unwrap();
        db::upsert_session(&conn, "s1", "claude", "/r", "/r", 1_700_000_000_000).unwrap();
        db::apply_batch(
            &mut conn,
            &db::PendingSession {
                id: "s1".into(),
                agent: "claude".into(),
                repo: "/r".into(),
                last_event_at: 1_700_000_000_000,
            },
            "test",
            "要約: 検索を実装した",
            &[db::Observation {
                kind: "decision".into(),
                title: "use the trigram tokenizer".into(),
                body: "FTS5 trigram <b>indexes</b> CJK by character".into(),
            }],
            i64::MAX,
        )
        .unwrap();
        let v = Viewer {
            home: dir.clone(),
            cwd_repo: "/r".into(),
            port: 4321,
            token: "t0k".into(),
        };
        (dir, v)
    }

    const HOST: (&str, &str) = ("Host", "127.0.0.1:4321");
    const TOKEN: (&str, &str) = ("X-Oboete-Token", "t0k");

    fn json_of(r: &Response) -> Value {
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        serde_json::from_slice(&r.body).unwrap()
    }

    #[test]
    fn guards_come_before_any_data() {
        let (dir, v) = viewer("guards");
        let status = |m: &str, t: &str, h: &[(&str, &str)]| v.route(m, t, h).status;
        assert_eq!(status("GET", "/", &[HOST]), 200);
        assert_eq!(status("GET", "/", &[("host", "localhost:4321")]), 200);
        assert_eq!(status("POST", "/api/repos", &[HOST, TOKEN]), 405);
        assert_eq!(
            status(
                "GET",
                "/api/repos",
                &[HOST, TOKEN, ("Transfer-Encoding", "chunked")]
            ),
            400
        );
        assert_eq!(
            status("GET", "/api/repos", &[HOST, TOKEN, ("Content-Length", "0")]),
            400
        );
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

    #[test]
    fn api_answers_from_the_store() {
        let (dir, v) = viewer("api");
        let get = |t: &str| json_of(&v.route("GET", t, &[HOST, TOKEN]));
        let repos = get("/api/repos");
        assert_eq!(
            (
                repos["current"].as_str(),
                repos["repos"][0]["sessions"].as_i64()
            ),
            (Some("/r"), Some(1))
        );
        let tl = get("/api/timeline?repo=&limit=5");
        assert!(tl[0]["summary"].as_str().unwrap().starts_with("要約"));
        assert_eq!(
            get("/api/timeline?repo=%2Fother").as_array().unwrap().len(),
            0
        );
        let docs = get("/api/session?id=s1");
        assert_eq!(
            (docs[0]["doc"].as_str(), docs[1]["doc"].as_str()),
            (Some("s1"), Some("o1"))
        );
        // Japanese through percent-encoding; `+` is a space.
        let hits = get("/api/search?q=%E6%A4%9C%E7%B4%A2");
        assert_eq!(hits[0]["doc"], "s1");
        let hits = get("/api/search?q=trigram+CJK&repo=%2Fr");
        assert_eq!(hits[0]["doc"], "o1");
        // Bodies go out as data; the page renders them as text.
        let d = get("/api/doc?id=o1");
        assert!(d["text"].as_str().unwrap().contains("<b>indexes</b>"));
        assert_eq!(v.route("GET", "/api/doc?id=o9", &[HOST, TOKEN]).status, 404);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn socket_round_trip() {
        let (dir, mut v) = viewer("socket");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        v.port = listener.local_addr().unwrap().port();
        let port = v.port;
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (s, _) = listener.accept().unwrap();
                v.serve(s);
            }
        });
        let ask = |raw: &[u8]| {
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            c.write_all(raw).unwrap();
            let mut out = String::new();
            c.read_to_string(&mut out).unwrap();
            out
        };
        let ok = ask(format!(
            "GET /api/repos HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Oboete-Token: t0k\r\n\r\n"
        )
        .as_bytes());
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(
            ok.contains("Content-Security-Policy: default-src 'none'")
                && ok.contains("\"current\":\"/r\"")
        );
        let mut huge = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ").into_bytes();
        // Exactly the cap: every byte is read before the 400, so the close is a FIN, not a RST.
        huge.resize(MAX_HEAD, b'a');
        assert!(ask(&huge).starts_with("HTTP/1.1 400 "));
        server.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_params_decode() {
        let p = params("q=a+b%20c&repo=%2Fhome%2Fx&x");
        assert_eq!((p["q"].as_str(), p["repo"].as_str()), ("a b c", "/home/x"));
        assert!(!p.contains_key("x"));
    }
}
