//! The embedder chosen on the settings page (W4, docs/settings-embeddings.md). What a choice does
//! is computed as `oboete setup --embeddings` computes it (`setup::consent`); the page previews
//! it, and takes it with the preview's one-use key while nothing it depends on has changed.
//! `local` downloads and checks its files holding only the model folder's lock, so other saves go
//! on meanwhile, and then `[embedding] provider` is written. One choice runs at a time and the
//! last one's outcome is kept for the page, as maintenance keeps its receipt. Answers carry codes:
//! no error's text, and never the Workers AI token.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Refusal, bytes, config_lock, invalid, parsed, put, refused, utf8, version};
use crate::model_fetch::{Artifact, Runtime, Stopped};
use crate::setup::Consent;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    choice: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    choice: String,
    preview_key: String,
    confirmed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeySave {
    key: String,
    version: String,
}

/// The page's Workers AI values in its settings save; `account_id` absent keeps the saved one,
/// which the page cannot remove (a `workers-ai` choice needs it).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Values {
    account_id: Option<String>,
    daily_requests: u32,
    monthly_usd: f64,
}

/// A preview the page may take once, while config.toml and the files are as they were.
struct Prepared {
    nonce: String,
    fingerprint: String,
}

/// A choice taken: running, or how it ended.
#[derive(Clone, Serialize)]
struct Run {
    choice: &'static str,
    /// `running`, `done`, `failed`, or `unknown` when it stopped in a panic.
    phase: &'static str,
    code: Option<&'static str>,
    /// The bytes the model folder held, and those still to download, when it started.
    held: u64,
    get: u64,
}

#[derive(Default)]
struct State {
    prepared: Option<Prepared>,
    active: Option<Run>,
    last: Option<Run>,
}

pub(crate) struct Embedding {
    state: Mutex<State>,
    model: &'static [Artifact],
    runtime: Option<&'static Runtime>,
}

impl Default for Embedding {
    fn default() -> Self {
        Self::with_files(
            crate::model_fetch::BGE_M3,
            crate::model_fetch::RUNTIME.as_ref(),
        )
    }
}

/// A choice that stops in a panic is kept as `unknown`, as maintenance keeps its runs.
struct Active<'a>(&'a Embedding);

impl Drop for Active<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut state = self.0.lock();
        if let Some(mut run) = state.active.take() {
            run.phase = "unknown";
            run.code = Some("embedding_unknown");
            state.last = Some(run);
        }
    }
}

fn stale() -> Refusal {
    refused(409, "stale", "")
}

fn busy() -> Refusal {
    refused(409, "embedding_busy", "")
}

fn known(choice: &str) -> Result<(), Refusal> {
    match choice {
        "none" | "local" | "workers-ai" => Ok(()),
        _ => Err(refused(400, "bad_request", "")),
    }
}

/// What a preview binds: config.toml's version and what the choice does with it.
fn fingerprint(version: &str, consent: &Consent) -> String {
    let mut hash = Sha256::new();
    hash.update(b"oboete:embedding-choice:v1\0");
    hash.update(version.as_bytes());
    hash.update([0]);
    hash.update(serde_json::to_vec(consent).expect("typed consent serializes"));
    format!("{:x}", hash.finalize())
}

fn snapshot(home: &Path, state: &State) -> Value {
    json!({"active": state.active, "last": state.last,
        "held": crate::setup::bytes_in(&crate::embed::local_dir(home))})
}

impl Embedding {
    pub(crate) fn with_files(
        model: &'static [Artifact],
        runtime: Option<&'static Runtime>,
    ) -> Self {
        Self {
            state: Mutex::new(State::default()),
            model,
            runtime,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `GET /api/embedding`: the choice running and the last one, and the bytes the model folder
    /// holds, which grow while it downloads.
    pub(crate) fn status(&self, home: &Path) -> Value {
        snapshot(home, &self.lock())
    }

    /// The choice's consent and config.toml's version, from a file every reader loads.
    fn consent(&self, home: &Path, choice: &str) -> Result<(Consent, String), Refusal> {
        let was = bytes(home).map_err(|_| invalid())?;
        let text = utf8(was.as_deref()).ok_or_else(invalid)?;
        parsed(&home.join("config.toml"), text).ok_or_else(invalid)?;
        let consent =
            crate::setup::consent(home, choice, self.model, self.runtime).map_err(|error| {
                match error.downcast_ref::<crate::setup::Refused>() {
                    Some(why) => refused(422, why.code(), "embedding.choice"),
                    None => invalid(),
                }
            })?;
        Ok((consent, version(was.as_deref())))
    }

    /// `POST /api/embedding/preview {"choice"}`: what the choice does, and the key that takes it.
    /// It reads only: no request goes out, and no file or folder is made.
    pub(crate) fn preview(&self, home: &Path, body: &[u8]) -> Result<Value, Refusal> {
        let posted: Preview =
            serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
        known(&posted.choice)?;
        if self.lock().active.is_some() {
            return Err(busy());
        }
        let (consent, version) = self.consent(home, &posted.choice)?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| refused(503, "unavailable", ""))?;
        let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
        self.lock().prepared = Some(Prepared {
            nonce: nonce.clone(),
            fingerprint: fingerprint(&version, &consent),
        });
        Ok(json!({"preview_key": nonce, "consent": consent}))
    }

    /// `POST /api/embedding {"choice", "preview_key", "confirmed"}`: the preview taken, once,
    /// while config.toml and the files are as it saw them. `none` and `workers-ai` are written
    /// under the holds that check was made under; `local` first downloads what is missing and
    /// checks every file, and the answer waits for it.
    pub(crate) fn start(
        &self,
        home: &Path,
        saving: &Mutex<()>,
        body: &[u8],
    ) -> Result<Value, Refusal> {
        let posted: Start =
            serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
        known(&posted.choice)?;
        if !posted.confirmed {
            return Err(refused(422, "embedding_confirmation", "confirmed"));
        }
        let consent = {
            let _saving = saving.lock().unwrap_or_else(PoisonError::into_inner);
            let _config = config_lock(home).map_err(|_| refused(500, "write_failed", ""))?;
            let (consent, version) = self.consent(home, &posted.choice)?;
            let mut state = self.lock();
            if state.active.is_some() {
                return Err(busy());
            }
            let fresh = fingerprint(&version, &consent);
            if !(state.prepared.as_ref())
                .is_some_and(|p| p.nonce == posted.preview_key && p.fingerprint == fresh)
            {
                return Err(stale());
            }
            // Used once, whatever happens next.
            state.prepared = None;
            let get = match &consent {
                Consent::Local { get, .. } => get.iter().map(|d| d.bytes).sum(),
                _ => 0,
            };
            let run = Run {
                choice: consent.choice(),
                phase: "running",
                code: None,
                held: crate::setup::bytes_in(&crate::embed::local_dir(home)),
                get,
            };
            if !consent.needs_files() {
                let written = super::set_embedding_provider_held(home, consent.choice());
                let code = written.err().map(|_| "embedding_write_failed");
                state.last = Some(Run {
                    phase: if code.is_none() { "done" } else { "failed" },
                    code,
                    ..run
                });
                return Ok(snapshot(home, &state));
            }
            state.active = Some(run);
            consent
        };
        let _active = Active(self);
        let code = match crate::setup::ready(&consent, self.model, self.runtime) {
            Ok(()) => {
                let write = || -> anyhow::Result<bool> {
                    let _saving = saving.lock().unwrap_or_else(PoisonError::into_inner);
                    let _config = config_lock(home)?;
                    super::set_embedding_provider_held(home, consent.choice())
                };
                write().err().map(|_| "embedding_write_failed")
            }
            Err(error) => Some(
                match error.chain().find_map(|c| c.downcast_ref::<Stopped>()) {
                    Some(Stopped::Busy(_)) => "embedding_busy",
                    Some(Stopped::NoSpace { .. }) => "embedding_no_space",
                    None => "embedding_failed",
                },
            ),
        };
        let mut state = self.lock();
        if let Some(mut run) = state.active.take() {
            run.phase = if code.is_none() { "done" } else { "failed" };
            run.code = code;
            state.last = Some(run);
        }
        Ok(snapshot(home, &state))
    }
}

/// `GET /api/settings`'s `embedding`: the saved values, Workers AI's use as doctor counts it,
/// and the local model's files from their sizes and marker (nothing hashed, nothing sent).
pub(super) fn section(
    home: &Path,
    cfg: &crate::config::Embedding,
    usd: Option<f64>,
    requests: Option<u32>,
    model: &[Artifact],
    runtime: Option<&Runtime>,
) -> Value {
    let dir = crate::embed::local_dir(home);
    let unavailable = crate::embed::local_unavailable().map(|_| {
        if cfg!(feature = "local-embed") {
            "no_runtime"
        } else {
            "no_feature"
        }
    });
    let files = match (unavailable, runtime) {
        (None, Some(runtime)) => {
            let all: Vec<Artifact> = (model.iter().copied()).chain([runtime.library]).collect();
            Some(crate::embed::files_in(&dir, &all).0.code())
        }
        _ => None,
    };
    json!({
        "provider": cfg.provider,
        "workers_ai": {
            "account_id": cfg.account_id,
            "key_file": cfg.key_file,
            "key": if cfg.key_file.exists() { "ok" } else { "missing" },
            "daily_requests": cfg.daily_requests,
            "monthly_usd": cfg.monthly_usd,
            "usd_this_month": usd,
            "requests_today": requests,
        },
        "local": {
            "unavailable": unavailable,
            "files": files,
            "dir": dir,
            "held": crate::setup::bytes_in(&dir),
        },
    })
}

/// The page's Workers AI values into `doc`, each only when it changes, and only then checked: an
/// account id is Cloudflare's 32 lowercase hexadecimal characters, `daily_requests` within the
/// provider budget's range, and `monthly_usd` a number of USD above 0, as config.toml requires. A
/// value config.toml has already is the user's, in range or not, so it does not block the rest of
/// a save (as `put_entry`).
pub(super) fn write(
    doc: &mut toml_edit::DocumentMut,
    posted: &Values,
    now: &crate::config::Embedding,
) -> Result<(), Refusal> {
    if let Some(account) = &posted.account_id
        && now.account_id.as_ref() != Some(account)
    {
        let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
        if account.len() != 32 || !account.bytes().all(hex) {
            return Err(refused(422, "range", "embedding.account_id"));
        }
        put(doc, "embedding", "account_id", account.as_str().into());
    }
    if posted.daily_requests != now.daily_requests {
        if !super::BUDGET.contains(&posted.daily_requests) {
            return Err(refused(422, "range", "embedding.daily_requests"));
        }
        let requests = i64::from(posted.daily_requests);
        put(doc, "embedding", "daily_requests", requests.into());
    }
    if posted.monthly_usd != now.monthly_usd {
        if !(posted.monthly_usd.is_finite() && posted.monthly_usd > 0.0) {
            return Err(refused(422, "range", "embedding.monthly_usd"));
        }
        put(doc, "embedding", "monthly_usd", posted.monthly_usd.into());
    }
    Ok(())
}

/// `POST /api/embedding/key {"key", "version"}`: the Workers AI token typed on the page, written
/// as a provider's key is (`keyfile::managed`: a new owner-only file outside the store), and
/// `[embedding] key_file` pointed at it, against the version the page showed. The old file is
/// left as it is. The answer is the settings with the key's state: the key is in no answer.
pub(crate) fn save_key(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let owner = crate::config::home_dir();
    let data = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from);
    save_key_at(home, saving, body, &owner, data.as_deref())
}

fn save_key_at(
    home: &Path,
    saving: &Mutex<()>,
    body: &[u8],
    owner: &Path,
    data: Option<&Path>,
) -> Result<Value, Refusal> {
    let posted: KeySave =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if !crate::keyfile::valid(&posted.key) {
        return Err(refused(422, "bad_key", "embedding.key"));
    }
    let _held = saving.lock().unwrap_or_else(PoisonError::into_inner);
    let _config = config_lock(home).map_err(|_| refused(500, "write_failed", ""))?;
    let was = bytes(home).map_err(|_| invalid())?;
    if version(was.as_deref()) != posted.version {
        return Err(stale());
    }
    let text = utf8(was.as_deref()).ok_or_else(invalid)?;
    parsed(&home.join("config.toml"), text).ok_or_else(invalid)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| invalid())?;
    let new_key = crate::keyfile::managed(&posted.key, home, owner, data)
        .map_err(|r| refused(r.status(), r.code(), "embedding.key"))?;
    let filename =
        (new_key.path().to_str()).ok_or_else(|| refused(422, "not_utf8", "embedding.key"))?;
    put(&mut doc, "embedding", "key_file", filename.into());
    let mut answer = super::providers::commit(home, &posted.version, doc)?;
    answer["key_saved"] = json!({"durable": new_key.durable});
    new_key.retain();
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    /// One model file and a runtime from `url`, pinned to `model` and `runtime`'s bytes, as
    /// `setup_embeddings` makes them.
    fn files(url: &'static str) -> (&'static [Artifact], &'static Runtime) {
        let pin = |b: &[u8]| leak(format!("{:x}", Sha256::digest(b)));
        let model = Box::leak(Box::new([Artifact {
            name: "tokenizer.json",
            url,
            size: 5,
            sha256: pin(b"model"),
        }]));
        let runtime = Box::leak(Box::new(Runtime {
            archive: Artifact {
                name: "onnxruntime/rt.tgz",
                url,
                size: 1,
                sha256: pin(b"a"),
            },
            member: "rt/lib",
            library: Artifact {
                name: "onnxruntime/librt",
                url: "",
                size: 7,
                sha256: pin(b"runtime"),
            },
        }));
        (model, runtime)
    }

    /// A URL nobody answers: a file still to download fails.
    fn closed() -> &'static str {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        leak(format!("http://{}/model", listener.local_addr().unwrap()))
    }

    fn put_files(home: &Path) {
        let dir = crate::embed::local_dir(home);
        std::fs::create_dir_all(dir.join("onnxruntime")).unwrap();
        std::fs::write(dir.join("tokenizer.json"), "model").unwrap();
        std::fs::write(dir.join("onnxruntime/librt"), "runtime").unwrap();
    }

    fn config(home: &Path) -> String {
        std::fs::read_to_string(home.join("config.toml")).unwrap()
    }

    fn body(value: Value) -> Vec<u8> {
        serde_json::to_vec(&value).unwrap()
    }

    /// Every path under `dir`, to see that a preview makes nothing.
    fn listing(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut all: Vec<_> = (std::fs::read_dir(dir).unwrap().flatten())
            .flat_map(|e| {
                let path = e.path();
                let mut under = if path.is_dir() {
                    listing(&path)
                } else {
                    Vec::new()
                };
                under.push(path);
                under
            })
            .collect();
        all.sort();
        all
    }

    /// The page's save of the settings as `show` gives them, with `embedding` when given.
    fn save_body(home: &Path, curate: bool, embedding: Option<Value>) -> Vec<u8> {
        let shown = super::super::show(home);
        let chain: Vec<Value> = (shown["chain"].as_array().unwrap().iter())
            .map(|e| {
                json!({"name": e["name"], "on": e["on"], "daily_budget": e["daily_budget"],
                    "timeout_s": e["timeout_s"], "model": e["model"]})
            })
            .collect();
        let mut summary = shown["summary"].clone();
        summary["curate"] = json!(curate);
        let mut save = json!({"version": shown["version"], "summary": summary,
            "paid_usd_per_month": shown["paid_usd_per_month"], "gemini": shown["gemini"],
            "inject": shown["inject"], "capture": shown["capture"], "chain": chain});
        if let Some(embedding) = embedding {
            save["embedding"] = embedding;
        }
        body(save)
    }

    /// W4: the page's `embedding` shows the saved choice, Workers AI's values and use, whether
    /// its token's file is there (never the token), and the local files' state by their sizes
    /// and marker.
    #[test]
    fn the_page_shows_the_choice_the_token_state_and_the_local_files() {
        let home = tempfile::tempdir().unwrap();
        let key = home.path().join("key.md");
        let cfg = crate::config::Embedding {
            account_id: Some(ACCOUNT.into()),
            key_file: key.clone(),
            ..Default::default()
        };
        let (model, runtime) = files(closed());
        let shown = || section(home.path(), &cfg, Some(0.25), Some(3), model, Some(runtime));
        let workers_ai = json!({"account_id": ACCOUNT, "key_file": key, "key": "missing",
            "daily_requests": 200, "monthly_usd": 1.0, "usd_this_month": 0.25,
            "requests_today": 3});
        assert_eq!(shown()["provider"], "none");
        assert_eq!(shown()["workers_ai"], workers_ai);
        std::fs::write(&key, "workers ai\nsecret-canary\n").unwrap();
        assert_eq!(shown()["workers_ai"]["key"], "ok");
        assert!(!shown().to_string().contains("secret-canary"));
        let local = &shown()["local"];
        assert_eq!(local["dir"], json!(crate::embed::local_dir(home.path())));
        if let Some(_why) = crate::embed::local_unavailable() {
            let why = if cfg!(feature = "local-embed") {
                "no_runtime"
            } else {
                "no_feature"
            };
            assert_eq!(
                (&local["unavailable"], &local["files"]),
                (&json!(why), &Value::Null)
            );
            return;
        }
        let files = || shown()["local"]["files"].clone();
        assert_eq!(files(), "not_downloaded");
        let dir = crate::embed::local_dir(home.path());
        std::fs::create_dir_all(dir.join("onnxruntime")).unwrap();
        std::fs::write(dir.join("tokenizer.json"), "model").unwrap();
        assert_eq!(files(), "incomplete");
        std::fs::write(dir.join("onnxruntime/librt"), "runtime").unwrap();
        assert_eq!(files(), "changed");
        let all: Vec<_> = model.iter().copied().chain([runtime.library]).collect();
        crate::model_fetch::verify(&dir, &all).unwrap();
        assert_eq!(files(), "ready");
        assert_eq!(
            shown()["local"]["held"],
            12 + std::fs::metadata(dir.join("verified")).unwrap().len()
        );
    }

    /// W4: a preview reads only, a choice needs its preview's key once and its confirmation,
    /// and a config changed since the preview, or a choice that cannot be taken, leaves
    /// config.toml's bytes as they were. `workers-ai` is written with the account and caps it
    /// showed.
    #[test]
    fn a_choice_is_previewed_and_then_taken_once() {
        let home = tempfile::tempdir().unwrap();
        let key = home.path().join("key.md");
        let config_toml = |account: bool| {
            format!(
                "# the owner's notes\n[embedding]\nprovider = \"none\" # kept\n{}key_file = '{}'\n",
                if account {
                    format!("account_id = \"{ACCOUNT}\"\n")
                } else {
                    String::new()
                },
                key.display()
            )
        };
        std::fs::write(home.path().join("config.toml"), config_toml(false)).unwrap();
        let (model, runtime) = files(closed());
        let e = Embedding::with_files(model, Some(runtime));
        let saving = Mutex::new(());
        let preview = |choice: &str| e.preview(home.path(), &body(json!({"choice": choice})));
        let start = |choice: &str, key: &str, confirmed: bool| {
            let posted = json!({"choice": choice, "preview_key": key, "confirmed": confirmed});
            e.start(home.path(), &saving, &body(posted))
        };
        // No account, then no readable token: refused before any key is given.
        assert_eq!(preview("workers-ai").unwrap_err().code, "no_account");
        std::fs::write(home.path().join("config.toml"), config_toml(true)).unwrap();
        assert_eq!(preview("workers-ai").unwrap_err().code, "no_key");
        assert_eq!(preview("elsewhere").unwrap_err().status, 400);
        // A key is used once, even when its choice changes nothing.
        let was = config(home.path());
        let nonce = preview("none").unwrap()["preview_key"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            start("none", &nonce, true).unwrap()["last"]["phase"],
            "done"
        );
        assert_eq!(start("none", &nonce, true).unwrap_err().code, "stale");
        assert_eq!(config(home.path()), was);
        std::fs::write(&key, "workers ai\nsecret-canary\n").unwrap();
        let before = listing(home.path());
        let was = config(home.path());
        let shown = preview("workers-ai").unwrap();
        assert_eq!(listing(home.path()), before, "a preview makes nothing");
        assert_eq!(
            shown["consent"],
            json!({"choice": "workers-ai", "account": ACCOUNT, "key_file": key,
                "daily_requests": 200, "monthly_usd": 1.0})
        );
        assert!(!shown.to_string().contains("secret-canary"));
        let nonce = shown["preview_key"].as_str().unwrap().to_owned();
        assert_eq!(
            start("workers-ai", &nonce, false).unwrap_err().code,
            "embedding_confirmation"
        );
        assert_eq!(
            start("workers-ai", &"0".repeat(64), true).unwrap_err().code,
            "stale"
        );
        assert_eq!(start("none", &nonce, true).unwrap_err().code, "stale");
        assert_eq!(config(home.path()), was);
        let taken = start("workers-ai", &nonce, true).unwrap();
        assert_eq!(taken["active"], Value::Null);
        assert_eq!(
            (&taken["last"]["phase"], &taken["last"]["choice"]),
            (&json!("done"), &json!("workers-ai"))
        );
        assert_eq!(
            config(home.path()),
            was.replace("provider = \"none\"", "provider = \"workers-ai\"")
        );
        // An edit after the preview: its key no longer takes it.
        let nonce = preview("none").unwrap()["preview_key"]
            .as_str()
            .unwrap()
            .to_owned();
        let edited = config(home.path()) + "# edited by hand\n";
        std::fs::write(home.path().join("config.toml"), &edited).unwrap();
        assert_eq!(start("none", &nonce, true).unwrap_err().code, "stale");
        assert_eq!(config(home.path()), edited);
    }

    /// W4 with the model (feature `local-embed`): `local` downloads what is missing and checks
    /// every file before it is written, holding only the model folder's lock, so a settings save
    /// goes on meanwhile and the page reads the run; a failed download, or the folder held by
    /// another oboete, writes nothing and is answered with its code.
    #[test]
    fn local_is_written_only_once_its_files_are_checked() {
        if crate::embed::local_unavailable().is_some() {
            return;
        }
        let saving = Mutex::new(());
        let take = |e: &Embedding, home: &Path| {
            let shown = e.preview(home, &body(json!({"choice": "local"}))).unwrap();
            let posted = json!({"choice": "local", "confirmed": true,
                "preview_key": shown["preview_key"]});
            (shown, e.start(home, &saving, &body(posted)).unwrap())
        };
        let start_config = "[summary]\ncurate = false\n[embedding]\nprovider = \"none\" # kept\n";

        // Files there but not checked: checked, then written.
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), start_config).unwrap();
        let (model, runtime) = files(closed());
        let e = Embedding::with_files(model, Some(runtime));
        put_files(home.path());
        let (shown, answer) = take(&e, home.path());
        assert_eq!(
            (&shown["consent"]["get"], &shown["consent"]["check"]),
            (&json!([]), &json!(true))
        );
        assert_eq!(answer["last"]["phase"], "done", "{answer}");
        assert!(config(home.path()).contains("provider = \"local\" # kept"));
        let all: Vec<_> = model.iter().copied().chain([runtime.library]).collect();
        assert!(crate::model_fetch::marker_ok(
            &crate::embed::local_dir(home.path()),
            &all
        ));

        // A download that fails, and a folder another oboete holds: nothing written.
        for held in [false, true] {
            let home = tempfile::tempdir().unwrap();
            std::fs::write(home.path().join("config.toml"), start_config).unwrap();
            let dir = crate::embed::local_dir(home.path());
            std::fs::create_dir_all(&dir).unwrap();
            let lock = std::fs::File::create(dir.join(".lock")).unwrap();
            if held {
                lock.lock().unwrap();
            }
            let (shown, answer) = take(&e, home.path());
            assert_eq!(shown["consent"]["get"][0]["source"], "model");
            let code = if held {
                "embedding_busy"
            } else {
                "embedding_failed"
            };
            assert_eq!(
                (&answer["last"]["phase"], &answer["last"]["code"]),
                (&json!("failed"), &json!(code)),
                "{answer}"
            );
            assert_eq!(config(home.path()), start_config);
        }

        // While a download waits on its server: the page sees it running, a settings save goes
        // through, and another choice waits for it.
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), start_config).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = leak(format!("http://{}/model", listener.local_addr().unwrap()));
        let (model, runtime) = files(url);
        let e = Embedding::with_files(model, Some(runtime));
        let (accepted, connected) = std::sync::mpsc::channel();
        let (go, gate) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                // Bounded, as the gate: a download that never connects still ends the test.
                listener.set_nonblocking(true).unwrap();
                let deadline = Instant::now() + Duration::from_secs(30);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "no download came");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("accept: {e}"),
                    }
                };
                accepted.send(()).unwrap();
                // Bounded: a test that fails before opening the gate still ends.
                let _ = gate.recv_timeout(Duration::from_secs(30));
                drop(stream);
            });
            let run = scope.spawn(|| take(&e, home.path()).1);
            connected.recv_timeout(Duration::from_secs(30)).unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            while e.status(home.path())["active"].is_null() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(e.status(home.path())["active"]["phase"], "running");
            assert_eq!(e.status(home.path())["active"]["get"], 6);
            // The save ends while the download still waits: it does not wait for it.
            let saved = scope.spawn(|| {
                let posted = save_body(home.path(), true, None);
                super::super::save(home.path(), &saving, &posted)
            });
            while !saved.is_finished() {
                assert!(
                    Instant::now() < deadline,
                    "a settings save waited for the download"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            saved.join().unwrap().unwrap();
            let again = e.preview(home.path(), &body(json!({"choice": "none"})));
            assert_eq!(again.unwrap_err().code, "embedding_busy");
            go.send(()).unwrap();
            let answer = run.join().unwrap();
            assert_eq!(answer["last"]["code"], "embedding_failed", "{answer}");
        });
        let now = config(home.path());
        assert!(now.contains("curate = true") && now.contains("provider = \"none\" # kept"));
    }

    /// W4: Workers AI's account and caps join the page's save, each checked at its range and
    /// written only when it changes; an older page's save leaves them as they are.
    #[test]
    fn workers_ai_values_are_checked_and_written_alone() {
        let home = tempfile::tempdir().unwrap();
        let text = "[embedding]\n# by hand\nprovider = \"none\"\ndaily_requests = 150 # mine\n";
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        let saving = Mutex::new(());
        let save = |values: Value| {
            let posted = save_body(home.path(), false, Some(values));
            super::super::save(home.path(), &saving, &posted)
        };
        for (values, field) in [
            (
                json!({"account_id": ACCOUNT.to_uppercase(), "daily_requests": 150,
                "monthly_usd": 1.0}),
                "embedding.account_id",
            ),
            (
                json!({"account_id": &ACCOUNT[1..], "daily_requests": 150, "monthly_usd": 1.0}),
                "embedding.account_id",
            ),
            (
                json!({"daily_requests": 0, "monthly_usd": 1.0}),
                "embedding.daily_requests",
            ),
            (
                json!({"daily_requests": 100_001, "monthly_usd": 1.0}),
                "embedding.daily_requests",
            ),
            (
                json!({"daily_requests": 150, "monthly_usd": 0.0}),
                "embedding.monthly_usd",
            ),
            (
                json!({"daily_requests": 150, "monthly_usd": -2.0}),
                "embedding.monthly_usd",
            ),
        ] {
            let refused = save(values.clone()).unwrap_err();
            assert_eq!(
                (refused.code, refused.field.as_str()),
                ("range", field),
                "{values}"
            );
            assert_eq!(config(home.path()), text);
        }
        let shown = save(json!({"account_id": ACCOUNT, "daily_requests": 150,
            "monthly_usd": 2.5}))
        .unwrap();
        assert_eq!(
            config(home.path()),
            format!("{text}account_id = \"{ACCOUNT}\"\nmonthly_usd = 2.5\n")
        );
        let workers_ai = &shown["embedding"]["workers_ai"];
        assert_eq!(
            (&workers_ai["account_id"], &workers_ai["monthly_usd"]),
            (&json!(ACCOUNT), &json!(2.5))
        );
        let posted = save_body(home.path(), true, None);
        super::super::save(home.path(), &saving, &posted).unwrap();
        assert!(config(home.path()).contains("monthly_usd = 2.5"));
        // Values the file has already, out of the page's ranges, are sent back and pass as they are.
        let text = "[embedding]\nprovider = \"none\"\naccount_id = \"ACCT\"\n\
                    daily_requests = 500000\nmonthly_usd = 0.0\n";
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        save(json!({"account_id": "ACCT", "daily_requests": 500_000, "monthly_usd": 0.0})).unwrap();
        assert_eq!(config(home.path()), text);
    }

    /// W4 (security scope): the Workers AI token typed on the page goes to a new owner-only file
    /// outside the store and `[embedding] key_file` to it; no answer holds it, and the file it
    /// was read from before stays as it was.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_workers_ai_token_goes_to_its_own_file_and_into_no_answer() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let owner = tempfile::tempdir().unwrap();
        let old = owner.path().join("CF_WORKERS_AI_KEY.md");
        std::fs::write(&old, "workers ai\nOldSyntheticToken000\n").unwrap();
        let text = format!(
            "# notes\n[embedding]\nprovider = \"none\"\nkey_file = {:?} # where it was\n",
            old.display().to_string()
        );
        std::fs::write(home.path().join("config.toml"), &text).unwrap();
        let saving = Mutex::new(());
        let version = super::super::show(home.path())["version"].clone();
        let canary = format!("{}-{}", "NewSyntheticToken", "7d41e0b2");
        let save = |key: &str, version: &Value| {
            let posted = body(json!({"key": key, "version": version}));
            save_key_at(home.path(), &saving, &posted, owner.path(), None)
        };
        let refused = save(&format!("{canary} x"), &version).unwrap_err();
        assert_eq!(
            (refused.code, refused.field.as_str()),
            ("bad_key", "embedding.key")
        );
        assert_eq!(save(&canary, &json!("none")).unwrap_err().code, "stale");
        assert_eq!(config(home.path()), text);
        let answer = save(&canary, &version).unwrap();
        assert!(!answer.to_string().contains("NewSyntheticToken"));
        assert_eq!(answer["embedding"]["workers_ai"]["key"], "ok");
        let file = crate::config::load(home.path()).unwrap().embedding.key_file;
        assert!(file.starts_with(owner.path()) && !file.starts_with(home.path()));
        assert_eq!(crate::config::read_key(&file).unwrap(), canary);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(config(home.path()).contains("# where it was"));
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "workers ai\nOldSyntheticToken000\n"
        );
    }
}
