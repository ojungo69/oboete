//! Workers AI's bge-m3 (PR-D, docs/pr-d.md), as Design B's embedding phase and search call it
//! (milestone 4 D8): the request, its limits, and the sign bits the vector index holds.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::config;

pub const DIM: usize = 1024;
/// The model, recorded with every vector: one vector space per store (proposal §2.3). Workers AI
/// and a local copy of the same weights share it (decision 5), so this names the model, not where
/// it ran.
pub const EMBEDDER: &str = "bge-m3";
/// `provider_calls` name, for the daily cap.
pub(crate) const CALLS: &str = "workers-ai-embed";
/// Workers AI takes at most 100 texts per request and counts every text as long as the longest
/// (texts × longest ≤ 60,000 tokens, PR-A2): texts of similar length go together, count × longest
/// ≤ 50,000 characters.
pub(crate) const BATCH: usize = 100;
const BATCH_CHARS: usize = 50_000;
/// The model cuts beyond 8,192 tokens (`truncate_inputs`); sending more is wasted bytes.
pub(crate) const MAX_CHARS: usize = 12_000;
/// A prompt is embedded by its opening (the spike's texts).
pub(crate) const PROMPT_CHARS: usize = 1_000;
/// A batch of up to 100 texts; a search query waits for its vector (MCP budget p95 1.5 s).
pub(crate) const BATCH_TIMEOUT: Duration = Duration::from_secs(180);
/// One answer holds up to 100 × 1,024 floats as JSON (about 2 MB).
const MAX_RESPONSE_BYTES: u64 = 8 << 20;

/// Where `local` keeps bge-m3's files and its runtime (`model_fetch`): no store operation touches
/// it.
pub fn local_dir(home: &Path) -> PathBuf {
    home.join("models").join(EMBEDDER)
}

/// Why this build cannot run `local` here, if it cannot.
pub fn local_unavailable() -> Option<&'static str> {
    if !cfg!(feature = "local-embed") {
        Some("this oboete was built without local embeddings (cargo feature local-embed)")
    } else if crate::model_fetch::RUNTIME.is_none() {
        Some("Microsoft releases no ONNX Runtime 1.28.0 for this machine")
    } else {
        None
    }
}

/// Whether `local`'s files are ready, and doctor's line on them, from their sizes and the
/// `verified` marker: nothing is hashed.
pub fn local_state(home: &Path) -> (bool, String) {
    match (local_unavailable(), crate::model_fetch::local_files()) {
        (None, Some(files)) => files_state(&local_dir(home), &files),
        (why, _) => (
            false,
            format!("local model: {}", why.unwrap_or("unavailable")),
        ),
    }
}

fn files_state(dir: &Path, files: &[crate::model_fetch::Artifact]) -> (bool, String) {
    if crate::model_fetch::marker_ok(dir, files) {
        return (true, format!("local model: ready in {}", dir.display()));
    }
    let setup = "`oboete setup --embeddings local`";
    let short: Vec<&str> = (files.iter())
        .filter(|f| std::fs::metadata(dir.join(f.name)).map_or(true, |m| m.len() != f.size))
        .map(|f| f.name)
        .collect();
    let line = if short.len() == files.len() {
        format!(
            "not downloaded; {setup} downloads it into {}",
            dir.display()
        )
    } else if !short.is_empty() {
        format!(
            "incomplete in {} ({} missing); {setup} downloads the rest",
            dir.display(),
            short.join(", ")
        )
    } else {
        format!(
            "not verified since its files changed in {}; {setup} checks them again",
            dir.display()
        )
    };
    (false, format!("local model: {line}"))
}

/// The model's URL and the token.
fn endpoint(cfg: &config::Embedding) -> Result<(String, String)> {
    let account = cfg
        .account_id
        .as_deref()
        .ok_or_else(|| anyhow!("[embedding] account_id is not set"))?;
    let url = cfg.url.clone().unwrap_or_else(|| {
        format!(
            "{}client/v4/accounts/{account}/ai/run/@cf/baai/bge-m3",
            config::WORKERS_AI
        )
    });
    Ok((url, config::read_key(&cfg.key_file)?))
}

/// The embedder `[embedding]` configures (milestone 4 D8): the model's id, where it runs, and the
/// token. No `Debug`: it holds the token.
pub struct Embedder {
    pub id: String,
    pub url: String,
    key: String,
}

/// Why a request gave no vectors: its status and Retry-After, and whether the request may have
/// left the machine; never the answer's body, which can quote the text sent (#91).
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub status: Option<u16>,
    pub retry_after_s: Option<f64>,
    pub sent: bool,
    pub message: String,
}

impl Failure {
    /// Whether the request may have been run, and so billed: sent, and not answered with an
    /// error status (an answer whose body could not be read or used was run).
    pub fn billed(&self) -> bool {
        self.sent && self.status.is_none_or(|s| (200..300).contains(&s))
    }
}

impl Embedder {
    /// The embedder `[embedding]` names, or `None` for `provider = "none"`. The token is read
    /// here, so a missing key file stops the phase before it reads anything to send.
    pub fn from_config(cfg: &config::Embedding) -> Result<Option<Embedder>> {
        if cfg.provider != "workers-ai" {
            return Ok(None);
        }
        let (url, key) = endpoint(cfg)?;
        Ok(Some(Embedder {
            id: EMBEDDER.to_owned(),
            url,
            key,
        }))
    }

    /// One request: the texts' unit vectors, in order.
    pub fn run(
        &self,
        texts: &[&str],
        timeout: Duration,
    ) -> std::result::Result<Vec<Vec<f32>>, Failure> {
        self.run_admitted(texts, timeout, None)
    }

    pub(crate) fn run_admitted(
        &self,
        texts: &[&str],
        timeout: Duration,
        admission: Option<crate::dispatch::Guard>,
    ) -> std::result::Result<Vec<Vec<f32>>, Failure> {
        let failed = |status, retry_after_s, message: String| Failure {
            status,
            retry_after_s,
            sent: true,
            message,
        };
        // A loopback url (a stub) is never reached through the environment's proxy, which would
        // get the key and the texts; no redirect is followed.
        let request = crate::provider::admitted_agent(&self.url, timeout, 0, admission.as_ref())
            .post(&self.url)
            .header("Authorization", &format!("Bearer {}", self.key));
        let body = json!({"text": texts, "truncate_inputs": true});
        let mut resp = match &admission {
            Some(admission) => crate::dispatch::json(request, &body, admission),
            None => request.send_json(&body),
        }
        .map_err(|e| {
            let why = crate::provider::transport(&e);
            failed(None, None, format!("workers ai: {why}"))
        })?;
        let status = resp.status().as_u16();
        let retry_after_s = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0);
        let failed = |message: String| failed(Some(status), retry_after_s, message);
        let mut raw = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(resp.body_mut().as_reader(), MAX_RESPONSE_BYTES + 1),
            &mut raw,
        )
        .map_err(|e| {
            let why = crate::provider::read_error(&e);
            failed(format!("workers ai: read body: {why}"))
        })?;
        if raw.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(failed(format!(
                "workers ai: response larger than {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        let text = String::from_utf8_lossy(&raw);
        // The body is not kept: a 400 can quote the stored text sent for embedding (issue #91).
        if status != 200 {
            let code = crate::provider::error_code(&text)
                .map(|c| format!(": {c}"))
                .unwrap_or_default();
            return Err(failed(format!("workers ai: http {status}{code}")));
        }
        serde_json::from_str::<Value>(&text)
            .context("workers ai: response is not JSON")
            .and_then(|v| vectors(&v, texts.len()))
            .map_err(|e| failed(format!("{e:#}")))
    }
}

/// Requests of at most 100 texts whose count × longest stays under `BATCH_CHARS` (`todo` sorted by
/// length, so each batch holds texts of similar length). A text longer than that goes alone.
pub(crate) fn batches(todo: &[(String, String)]) -> Vec<&[(String, String)]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..todo.len() {
        let longest = todo[i].1.chars().count();
        if i > start && (i - start == BATCH || (i - start + 1) * longest > BATCH_CHARS) {
            out.push(&todo[start..i]);
            start = i;
        }
    }
    if start < todo.len() {
        out.push(&todo[start..]);
    }
    out
}

/// `result.data` of a Workers AI answer as `n` unit vectors of finite numbers.
fn vectors(v: &Value, n: usize) -> Result<Vec<Vec<f32>>> {
    let data = v["result"]["data"]
        .as_array()
        .ok_or_else(|| anyhow!("workers ai: no result.data"))?;
    anyhow::ensure!(
        data.len() == n,
        "workers ai: {} vectors for {n} texts",
        data.len()
    );
    data.iter()
        .map(|row| {
            let row = row
                .as_array()
                .filter(|r| r.len() == DIM)
                .ok_or_else(|| anyhow!("workers ai: a vector is not {DIM} numbers"))?;
            let mut vec = row
                .iter()
                .map(|x| {
                    x.as_f64()
                        .map(|x| x as f32)
                        .filter(|x| x.is_finite())
                        .ok_or_else(|| anyhow!("workers ai: a coordinate is not a finite number"))
                })
                .collect::<Result<Vec<f32>>>()?;
            let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            anyhow::ensure!(
                norm.is_finite() && norm > 0.0,
                "workers ai: a zero or overflowing vector"
            );
            vec.iter_mut().for_each(|x| *x /= norm);
            Ok(vec)
        })
        .collect()
}

/// Sign bits, most significant bit first in each byte (as the spike's `np.packbits`).
pub fn bits(vec: &[f32]) -> Vec<u8> {
    vec.chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |b, (i, x)| if *x > 0.0 { b | (0x80 >> i) } else { b })
        })
        .collect()
}

/// A loopback Workers AI for tests (milestone 4 Task 5). Each request is answered with one unit
/// vector per text, whose dimensions come from the text's words (`vector`), so a text is near the
/// texts that share its words. It records
/// every request's texts, answers the next ones with a scripted status and Retry-After when told,
/// and holds requests while a `Hold` lives.
#[cfg(test)]
pub(crate) mod stub {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_stay_under_the_request_limits() {
        let doc = |n: usize| (String::new(), "x".repeat(n));
        let mut todo: Vec<_> = (0..250).map(|_| doc(10)).collect();
        todo.extend((0..30).map(|_| doc(2_000)));
        todo.push(doc(60_000));
        let got = batches(&todo);
        for b in &got {
            let longest = b.iter().map(|(_, t)| t.len()).max().unwrap();
            assert!(b.len() <= BATCH);
            assert!(
                b.len() == 1 || b.len() * longest <= BATCH_CHARS,
                "{}",
                b.len()
            );
        }
        assert_eq!(got.iter().map(|b| b.len()).sum::<usize>(), todo.len());
        assert_eq!(got.last().unwrap().len(), 1);
    }

    /// Task 10: doctor names `local`'s files from their sizes and the marker, hashing nothing:
    /// not downloaded, incomplete (which), changed since verified, ready; and a build that cannot
    /// run them.
    #[test]
    fn doctor_names_the_local_model_state() {
        use crate::model_fetch::Artifact;
        let pin = |b: &[u8]| -> &'static str {
            Box::leak(format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b)).into_boxed_str())
        };
        let files = [
            Artifact {
                name: "a.json",
                url: "",
                size: 1,
                sha256: pin(b"a"),
            },
            Artifact {
                name: "onnx/b",
                url: "",
                size: 2,
                sha256: pin(b"bb"),
            },
        ];
        let home = tempfile::tempdir().unwrap();
        let dir = local_dir(home.path());
        let state = || files_state(&dir, &files);
        assert!(
            !state().0 && state().1.contains("not downloaded"),
            "{:?}",
            state()
        );
        std::fs::create_dir_all(dir.join("onnx")).unwrap();
        std::fs::write(dir.join("a.json"), "a").unwrap();
        assert!(state().1.contains("incomplete") && state().1.contains("onnx/b missing"));
        std::fs::write(dir.join("onnx/b"), "bb").unwrap();
        assert!(
            !state().0 && state().1.contains("not verified"),
            "{:?}",
            state()
        );
        crate::model_fetch::verify(&dir, &files).unwrap();
        assert!(state().0 && state().1.contains("ready"), "{:?}", state());
        // Written again, same size: the marker no longer matches, and nothing was hashed to say so.
        std::fs::remove_file(dir.join("a.json")).unwrap();
        std::fs::write(dir.join("a.json"), "x").unwrap();
        assert!(
            !state().0 && state().1.contains("not verified"),
            "{:?}",
            state()
        );
        let (ready, line) = local_state(home.path());
        if let Some(why) = local_unavailable() {
            assert_eq!((ready, line), (false, format!("local model: {why}")));
        } else {
            assert!(!ready && line.contains("not downloaded"), "{line}");
        }
    }

    #[test]
    fn answers_with_bad_coordinates_are_refused() {
        let row = |x: Value| {
            let mut r: Vec<Value> = vec![json!(0.5); DIM];
            r[3] = x;
            json!({"result": {"data": [r]}})
        };
        assert!(vectors(&row(json!(0.1)), 1).is_ok());
        assert!(vectors(&row(json!("x")), 1).is_err());
        assert!(vectors(&row(json!(1e300)), 1).is_err());
        assert!(vectors(&json!({"result": {"data": [vec![0.0; DIM]]}}), 1).is_err());
        assert!(vectors(&row(json!(0.1)), 2).is_err());
    }

    #[test]
    fn bits_are_signs_msb_first() {
        let mut v = vec![-1.0f32; 16];
        v[0] = 0.5;
        v[9] = 0.1;
        assert_eq!(bits(&v), [0x80, 0x40]);
    }
}
