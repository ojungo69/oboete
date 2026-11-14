use serde_json::{Value, json};
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Default)]
struct State {
    texts: Vec<Vec<String>>,
    script: std::collections::VecDeque<(u16, Option<u32>)>,
    held: bool,
    /// A text every request that holds it is answered 400 for.
    refused: Option<String>,
    /// Requests answered so far: a held one is answered after its client gave up.
    answered: usize,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

pub(crate) struct Stub {
    pub(crate) url: String,
    state: Shared,
}

/// While it lives, requests wait before they are answered (after they are recorded).
pub(crate) struct Hold(Shared);

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.0.lock().unwrap().held = false;
        self.0.1.notify_all();
    }
}

impl Stub {
    /// bge-m3 at a free loopback port.
    pub(crate) fn start() -> Stub {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/run/bge-m3", listener.local_addr().unwrap());
        let state: Shared = Arc::default();
        let shared = state.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let shared = shared.clone();
                std::thread::spawn(move || answer(conn.unwrap(), &shared, super::EMBEDDER));
            }
        });
        Stub { url, state }
    }

    /// The texts of each request so far, in order.
    pub(crate) fn texts(&self) -> Vec<Vec<String>> {
        self.state.0.lock().unwrap().texts.clone()
    }

    pub(crate) fn requests(&self) -> usize {
        self.state.0.lock().unwrap().texts.len()
    }

    pub(crate) fn answered(&self) -> usize {
        self.state.0.lock().unwrap().answered
    }

    /// The next request is answered `status`, with Retry-After `retry` seconds.
    pub(crate) fn fail_next(&self, status: u16, retry: Option<u32>) {
        self.state
            .0
            .lock()
            .unwrap()
            .script
            .push_back((status, retry));
    }

    /// Every request that holds `text` is answered 400.
    pub(crate) fn refuse(&self, text: &str) {
        self.state.0.lock().unwrap().refused = Some(text.to_owned());
    }

    pub(crate) fn hold(&self) -> Hold {
        self.state.0.lock().unwrap().held = true;
        Hold(self.state.clone())
    }
}

/// A test home's local model (`local_model`): `models/bge-m3/stub` holds its load's delay
/// in ms, `fail`, `nan`, or `gate` (the load waits until a file `go` is put beside it); each
/// load adds a line to `stub-loads` beside it.
pub(crate) fn local_model(home: &std::path::Path) -> Option<crate::resident::Load> {
    let dir = super::local_dir(home);
    let how = std::fs::read_to_string(dir.join("stub")).ok()?;
    Some(Box::new(move || {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(dir.join("stub-loads"))?
            .write_all(b"load\n")?;
        match how.trim() {
            "fail" => anyhow::bail!("the stub model does not load"),
            "nan" => {
                Ok(Box::new(|_: &str| Ok(vec![f32::NAN; super::DIM])) as crate::resident::Model)
            }
            "gate" => {
                while !dir.join("go").exists() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(Box::new(|text: &str| Ok(vector(super::EMBEDDER, text)))
                    as crate::resident::Model)
            }
            ms => {
                std::thread::sleep(std::time::Duration::from_millis(ms.parse().unwrap_or(0)));
                Ok(Box::new(|text: &str| Ok(vector(super::EMBEDDER, text)))
                    as crate::resident::Model)
            }
        }
    }))
}

/// A test home with a local model that loads as `how` says (`local_model`).
pub(crate) fn local(home: &std::path::Path, how: &str) {
    let dir = super::local_dir(home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stub"), how).unwrap();
}

/// How many times a test home's local model loaded.
pub(crate) fn local_loads(home: &std::path::Path) -> usize {
    let loads = super::local_dir(home).join("stub-loads");
    std::fs::read_to_string(loads).map_or(0, |s| s.lines().count())
}

/// The stub's vector for `text` under model `id`: each word adds one to the dimension its
/// hash picks, then the vector is scaled to unit length.
pub(crate) fn vector(id: &str, text: &str) -> Vec<f32> {
    use sha2::{Digest, Sha256};
    let mut v = vec![0.0f32; super::DIM];
    let words = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty());
    for w in words {
        let h = Sha256::digest(format!("{id}\n{}", w.to_lowercase()).as_bytes());
        v[usize::from(u16::from_le_bytes([h[0], h[1]])) % super::DIM] += 1.0;
    }
    if v.iter().all(|x| *x == 0.0) {
        v[0] = 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn answer(mut conn: std::net::TcpStream, shared: &Shared, id: &str) {
    let mut req = Vec::new();
    let mut buf = [0u8; 65536];
    let body = loop {
        let n = conn.read(&mut buf).unwrap_or(0);
        req.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&req).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text
                .to_lowercase()
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").map(str::to_string))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if req.len() >= end + 4 + len {
                break req[end + 4..end + 4 + len].to_vec();
            }
        }
        if n == 0 {
            return;
        }
    };
    let texts: Vec<String> = serde_json::from_slice::<Value>(&body).unwrap()["text"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_owned())
        .collect();
    let (lock, wake) = &**shared;
    let scripted = {
        let mut state = lock.lock().unwrap();
        state.texts.push(texts.clone());
        while state.held {
            state = wake.wait(state).unwrap();
        }
        state.answered += 1;
        match &state.refused {
            Some(r) if texts.contains(r) => Some((400, None)),
            _ => state.script.pop_front(),
        }
    };
    let (status, extra, out) = match scripted {
        Some((status, retry)) => (
            status,
            retry.map_or(String::new(), |s| format!("Retry-After: {s}\r\n")),
            json!({"success": false, "errors": [{"code": status, "message": "stub"}]}),
        ),
        None => {
            let data: Vec<Vec<f32>> = texts.iter().map(|t| vector(id, t)).collect();
            let shape = [texts.len(), super::DIM];
            (
                200,
                String::new(),
                json!({"result": {"shape": shape, "data": data}, "success": true}),
            )
        }
    };
    let out = out.to_string();
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        out.len()
    );
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(out.as_bytes());
}
