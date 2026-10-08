//! Fixed invalid-config recovery. Never returns the original text or accepts a path.
use super::{Refusal, refused};
use crate::executable::{CommandCaller, CommandHome};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::Path, sync::Mutex};

// No AI, spending, automatic worker, prompt text or old-memory delivery after recovery.
// Other capture uses built-in redaction; the preview discloses the loss of custom settings.
#[cfg(target_os = "linux")]
const SAFE: &str = "providers = []\npaid_usd_per_month = 0\n\
    [summary]\ncurate = false\n[embedding]\nprovider = 'none'\n\
    [worker]\nresident = false\n[capture]\nstore_prompts = false\n\
    [inject]\nsession_start = false\nsession_start_note = false\nper_prompt = false\ncorrection = false\n";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    preview_key: String,
    confirmed: bool,
}

struct Consent {
    nonce: String,
    fingerprint: String,
}

/// One random confirmation per viewer; recovery's witness stays in this bounded cache.
#[derive(Default)]
pub(crate) struct Recovery {
    prepared: Mutex<Option<Consent>>,
}

fn unavailable() -> Refusal {
    refused(422, "recovery_unavailable", "")
}
fn stale() -> Refusal {
    refused(409, "stale", "")
}

#[cfg(all(test, target_os = "linux"))]
thread_local! {
    pub(crate) static BEFORE_COPY_WRITE: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    pub(crate) static AFTER_COMMIT: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
}

impl Recovery {
    pub(crate) fn preview(&self, home: &Path, body: &[u8]) -> Result<Value, Refusal> {
        if serde_json::from_slice::<Value>(body).ok() != Some(json!({})) {
            return Err(refused(400, "bad_request", ""));
        }
        let input = snapshot(home)?;
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
        let nonce: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        *self
            .prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Consent {
            nonce: nonce.clone(),
            fingerprint: input.key,
        });
        Ok(json!({"preview_key":nonce,
        "replaces":"all_settings", "copy":"current_bytes", "ai":"off",
        "prompt_text":"off", "injection":"off", "other_capture":"builtin_redaction"}))
    }

    pub(crate) fn start(
        &self,
        caller: Option<CommandCaller>,
        home: &Path,
        saving: &Mutex<()>,
        body: &[u8],
    ) -> Result<Value, Refusal> {
        if body.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
            return Err(refused(400, "bad_request", ""));
        }
        let posted: Start =
            serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
        if !posted.confirmed {
            return Err(refused(422, "recovery_confirmation", ""));
        }
        let _saving = saving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let input = snapshot(home).map_err(|_| stale())?;
        let mut prepared = self
            .prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !prepared.as_ref().is_some_and(|consent| {
            consent.nonce == posted.preview_key && consent.fingerprint == input.key
        }) {
            return Err(stale());
        }
        // A consumed confirmation cannot replay after partial work or an uncertain response.
        prepared.take();
        drop(prepared);
        let command =
            CommandHome::new(home, caller.ok_or_else(unavailable)?).map_err(|_| unavailable())?;
        apply(home, &input, &command)
    }
}

#[cfg(not(target_os = "linux"))]
struct Input {
    key: String,
}
#[cfg(not(target_os = "linux"))]
fn snapshot(_home: &Path) -> Result<Input, Refusal> {
    Err(unavailable())
}
#[cfg(not(target_os = "linux"))]
fn apply(_home: &Path, _input: &Input, _command: &CommandHome) -> Result<Value, Refusal> {
    Err(unavailable())
}

#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
};

#[cfg(target_os = "linux")]
struct Input {
    key: String,
    bytes: Vec<u8>,
    // Held descriptors prevent inode reuse while consent is being checked.
    _file: File,
    folder: File,
}

#[cfg(target_os = "linux")]
fn snapshot(home: &Path) -> Result<Input, Refusal> {
    const LIMIT: u64 = 1024 * 1024;
    let home = std::path::absolute(home).map_err(|_| unavailable())?;
    let folder = crate::keyfile::private_recovery_dir(&home).map_err(|_| unavailable())?;
    let path = home.join("config.toml");
    let mut file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|_| unavailable())?;
    let stamp = |file: &File| -> Result<String, Refusal> {
        let m = file.metadata().map_err(|_| unavailable())?;
        // SAFETY: geteuid takes no arguments and cannot fail.
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o200 == 0
            || m.len() > LIMIT
            || !crate::keyfile::private_fs(file)
        {
            return Err(unavailable());
        }
        Ok(format!(
            "{}:{}:{}:{}:{}:{}:{}:{}",
            m.dev(),
            m.ino(),
            m.mode(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec()
        ))
    };
    let before = stamp(&file)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() as u64 > LIMIT || stamp(&file)? != before {
        return Err(unavailable());
    }
    if std::str::from_utf8(&bytes).ok().is_some_and(|text| {
        super::parsed(&path, text).is_some() && text.parse::<toml_edit::DocumentMut>().is_ok()
    }) {
        return Err(unavailable());
    }
    let m = folder.metadata().map_err(|_| unavailable())?;
    let mut hash = Sha256::new();
    hash.update(format!(
        "recovery-v1:{}:{}:{}:{before}:",
        m.dev(),
        m.ino(),
        m.mode()
    ));
    hash.update(&bytes);
    Ok(Input {
        key: format!("{:x}", hash.finalize()),
        bytes,
        _file: file,
        folder,
    })
}

#[cfg(target_os = "linux")]
fn state_directory(path: &Path) -> Result<File, Refusal> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map_err(|_| unavailable())?;
    let m = file.metadata().map_err(|_| unavailable())?;
    // This fixed child of the already-private home holds a lock, not the recovery copy.
    // Older saves created it as 0755; read permission does not permit replacing lock names.
    if !crate::keyfile::private_fs(&file) || m.mode() & 0o022 != 0
        // SAFETY: geteuid takes no arguments and cannot fail.
        || m.uid() != unsafe { libc::geteuid() }
    {
        return Err(unavailable());
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn apply(home: &Path, input: &Input, command: &CommandHome) -> Result<Value, Refusal> {
    let failed = || refused(500, "recovery_failed", "");
    command.check(home).map_err(|_| stale())?;
    if snapshot(home).map_err(|_| stale())?.key != input.key {
        return Err(stale());
    }
    // A new state directory is private from its first instant; never follow a planted alias.
    let state = home.join("state");
    match std::fs::DirBuilder::new().mode(0o700).create(&state) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(failed()),
    }
    let _state = state_directory(&state)?;
    let lock = state.join("config.lock");
    match std::fs::symlink_metadata(&lock) {
        Ok(m) if m.is_file() && m.nlink() == 1 => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(unavailable()),
    }
    let _config = super::config_lock(home).map_err(|_| failed())?;
    let same_file = |a: &File, b: &File| -> Result<bool, Refusal> {
        let a = a.metadata().map_err(|_| stale())?;
        let b = b.metadata().map_err(|_| stale())?;
        Ok(a.dev() == b.dev() && a.ino() == b.ino())
    };
    let check = || -> Result<(), Refusal> {
        command.check(home).map_err(|_| stale())?;
        let state_now = state_directory(&state).map_err(|_| stale())?;
        let lock_now = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&lock)
            .map_err(|_| stale())?;
        if !lock_now.metadata().map_err(|_| stale())?.is_file()
            || !same_file(&_state, &state_now)?
            || !same_file(&_config, &lock_now)?
        {
            return Err(stale());
        }
        if snapshot(home).map_err(|_| stale())?.key != input.key {
            return Err(stale());
        }
        Ok(())
    };
    check()?;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| failed())?;
    let name = format!(
        "config.toml.recovery-{}.bak",
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let path = home.join(&name);
    let mut copy = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| failed())?;
    #[cfg(test)]
    if let Some(before) = BEFORE_COPY_WRITE.take() {
        before(home);
    }
    let created = copy.metadata().map_err(|_| failed())?;
    // As for key registration, verify the opened destination before copying any secret bytes.
    // A safe earlier pathname observation is not proof of this descriptor's filesystem.
    if !created.is_file() || created.nlink() != 1 || created.mode() & 0o777 != 0o600
        || !crate::keyfile::private_fs(&copy)
        // SAFETY: geteuid takes no arguments and cannot fail.
        || created.uid() != unsafe { libc::geteuid() }
    {
        return Err(unavailable());
    }
    check()?;
    copy.write_all(&input.bytes)
        .and_then(|_| copy.sync_all())
        .map_err(|_| failed())?;
    let verify = || -> Result<(), Refusal> {
        let current = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| failed())?;
        let m = current.metadata().map_err(|_| failed())?;
        let original = copy.metadata().map_err(|_| failed())?;
        if !m.is_file()
            || m.dev() != original.dev()
            || m.ino() != original.ino()
            || m.nlink() != 1
            || m.mode() & 0o777 != 0o600
            || m.len() != input.bytes.len() as u64
        {
            return Err(failed());
        }
        let mut bytes = Vec::new();
        current
            .take(input.bytes.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| failed())?;
        if bytes != input.bytes {
            return Err(failed());
        }
        Ok(())
    };
    verify()?;
    input.folder.sync_all().map_err(|_| failed())?;
    let target = home.join("config.toml");
    if super::parsed(&target, SAFE).is_none() {
        return Err(failed());
    }
    let staged = crate::setup::stage(&target, SAFE).map_err(|_| failed())?;
    // Existing stager semantics: shared writers serialize; external edits after this last
    // check and before rename are not an atomic compare-and-swap.
    check()?;
    verify()?;
    staged.commit().map_err(|_| failed())?;
    #[cfg(test)]
    if let Some(after) = AFTER_COMMIT.take() {
        after(home);
    }
    // A post-commit readback failure is reported as unknown; the private copy is retained.
    let readback = (|| -> std::io::Result<Vec<u8>> {
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&target)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let mut bytes = Vec::new();
        file.take(SAFE.len() as u64 + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    })();
    if command.check(home).is_err() || readback.ok().as_deref() != Some(SAFE.as_bytes()) {
        return Ok(json!({"phase":"unknown", "backup":name}));
    }
    Ok(json!({"phase":"complete", "backup":name, "settings":super::show(home)}))
}
