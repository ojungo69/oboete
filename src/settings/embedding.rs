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

fn unauthorized() -> Refusal {
    refused(401, "unauthorized", "")
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
    /// checks every file, and the answer waits for it. `authorized` is the request's token checked
    /// again under config.lock, under which a new token is made: a choice that waited out one
    /// writes nothing, the provider after a download included (as `/api/settings`).
    pub(crate) fn start(
        &self,
        home: &Path,
        saving: &Mutex<()>,
        authorized: &dyn Fn() -> bool,
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
            if !authorized() {
                return Err(unauthorized());
            }
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
                let _saving = saving.lock().unwrap_or_else(PoisonError::into_inner);
                match config_lock(home) {
                    Err(_) => Some("embedding_write_failed"),
                    Ok(_config) if !authorized() => Some("embedding_token"),
                    Ok(_config) => (super::set_embedding_provider_held(home, consent.choice()))
                        .err()
                        .map(|_| "embedding_write_failed"),
                }
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
/// `authorized` is the request's token checked again under config.lock, as for a choice.
pub(crate) fn save_key(
    home: &Path,
    saving: &Mutex<()>,
    authorized: &dyn Fn() -> bool,
    body: &[u8],
) -> Result<Value, Refusal> {
    super::managed_key(home, (saving, authorized), body, None, "embedding", |_| {})
}

#[cfg(test)]
mod tests;
