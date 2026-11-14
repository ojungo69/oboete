//! The local model's files (milestone 4, Task 10): fetched once into the home, resumed after an
//! interruption, and each held to the size and SHA-256 pinned here (spec: an optional download
//! with its hash pinned in the binary, resume and a free-space check). The pin is the trust
//! anchor: nothing off it is ever left in place.

use std::{
    fs::{self, File, Metadata},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use ureq::unversioned::{
    resolver::DefaultResolver,
    transport::{Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport},
};

const MARGIN: u64 = 64 * 1024 * 1024;

/// The longest one step waits for the server: resolving, connecting (TLS included), sending the
/// request, and each read. The whole download has no limit, as 2.3 GB on a slow line outlasts any.
const IDLE: Duration = Duration::from_secs(60);

/// Why a fetch did not start, for a caller that answers with a code (the settings page, W4).
#[derive(Debug, Clone, PartialEq)]
pub enum Stopped {
    /// Another oboete holds the model directory's lock.
    Busy(PathBuf),
    /// The disk holds less than the download needs.
    NoSpace { needed: u64, available: u64 },
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stopped::Busy(dir) => write!(
                f,
                "another oboete is fetching or verifying the model in {}: run it again when it ends",
                dir.display()
            ),
            Stopped::NoSpace { needed, available } => write!(
                f,
                "model download needs {needed} bytes; {available} bytes available"
            ),
        }
    }
}

impl std::error::Error for Stopped {}

#[derive(Clone, Copy)]
pub struct Artifact {
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// BAAI/bge-m3 at commit 5617a9f61b028005a4858fdac845db406aefb181: the five files oboete's fused graph reads.
pub const BGE_M3: &[Artifact] = &[
    Artifact {
        name: "config.json",
        url: "https://huggingface.co/BAAI/bge-m3/resolve/5617a9f61b028005a4858fdac845db406aefb181/config.json",
        size: 687,
        sha256: "26159e7ad065073448460117eb24b7a4572f6f4e78eadff65dc0a11c052449fa",
    },
    Artifact {
        name: "special_tokens_map.json",
        url: "https://huggingface.co/BAAI/bge-m3/resolve/5617a9f61b028005a4858fdac845db406aefb181/special_tokens_map.json",
        size: 964,
        sha256: "8c785abebea9ae3257b61681b4e6fd8365ceafde980c21970d001e834cf10835",
    },
    Artifact {
        name: "tokenizer_config.json",
        url: "https://huggingface.co/BAAI/bge-m3/resolve/5617a9f61b028005a4858fdac845db406aefb181/tokenizer_config.json",
        size: 444,
        sha256: "a62b2b6784f990259fddef5f16388693a8043be4f69179e6a5257eeb3f9abac4",
    },
    Artifact {
        name: "tokenizer.json",
        url: "https://huggingface.co/BAAI/bge-m3/resolve/5617a9f61b028005a4858fdac845db406aefb181/tokenizer.json",
        size: 17_098_108,
        sha256: "21106b6d7dab2952c1d496fb21d5dc9db75c28ed361a05f5020bbba27810dd08",
    },
    Artifact {
        name: "onnx/model.onnx_data",
        url: "https://huggingface.co/BAAI/bge-m3/resolve/5617a9f61b028005a4858fdac845db406aefb181/onnx/model.onnx_data",
        size: 2_266_820_608,
        sha256: "1eebfb28493f67bba03ce0ef64bfdc7fc5a3bd9d7493f818bb1d78cd798416b4",
    },
];

/// Microsoft's ONNX Runtime 1.28.0 for one target (docs/spike/local-embeddings.md, the runtime
/// loaded at run time): its release archive, the library's path inside it, and the library, which
/// has no URL of its own as it comes out of the archive.
pub struct Runtime {
    pub archive: Artifact,
    pub member: &'static str,
    pub library: Artifact,
}

/// This target's runtime; `None` where Microsoft releases none (macOS x64), which gets no `local`.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    archive: Artifact {
        name: "onnxruntime/onnxruntime-linux-x64-1.28.0.tgz",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-1.28.0.tgz",
        size: 9_125_960,
        sha256: "a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407",
    },
    member: "onnxruntime-linux-x64-1.28.0/lib/libonnxruntime.so.1.28.0",
    library: Artifact {
        name: "onnxruntime/libonnxruntime.so.1.28.0",
        url: "",
        size: 24_268_848,
        sha256: "1461ef7cc3d9e49982591721683cc3e3a55580aeca9a5254e7aac47b75ee4bab",
    },
});
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    archive: Artifact {
        name: "onnxruntime/onnxruntime-linux-aarch64-1.28.0.tgz",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-aarch64-1.28.0.tgz",
        size: 8_116_278,
        sha256: "e15ff8b5d85afe6c144d97c6fd432254bf76a219daaf17658087d6ecb3e8f0bb",
    },
    member: "onnxruntime-linux-aarch64-1.28.0/lib/libonnxruntime.so.1.28.0",
    library: Artifact {
        name: "onnxruntime/libonnxruntime.so.1.28.0",
        url: "",
        size: 20_591_712,
        sha256: "f1ec1a08eb99bd6e5401340f0a2b101381bf4694415480291dc13bcaa30f9ec7",
    },
});
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    archive: Artifact {
        name: "onnxruntime/onnxruntime-osx-arm64-1.28.0.tgz",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-osx-arm64-1.28.0.tgz",
        size: 32_396_562,
        sha256: "1268b359718099bde2cedb55787f182a130067bc4f31e8c88478c445b850d3d8",
    },
    member: "onnxruntime-osx-arm64-1.28.0/lib/libonnxruntime.1.28.0.dylib",
    library: Artifact {
        name: "onnxruntime/libonnxruntime.1.28.0.dylib",
        url: "",
        size: 39_312_136,
        sha256: "dc19bbcb2f5c9fb3c68b4f9248aa0a35065ff702c5dbeae75eac54a74da97b6d",
    },
});
#[cfg(all(windows, target_arch = "x86_64"))]
pub const RUNTIME: Option<Runtime> = Some(Runtime {
    archive: Artifact {
        name: "onnxruntime/onnxruntime-win-x64-1.28.0.zip",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-win-x64-1.28.0.zip",
        size: 78_796_801,
        sha256: "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc",
    },
    member: "onnxruntime-win-x64-1.28.0/lib/onnxruntime.dll",
    library: Artifact {
        name: "onnxruntime/onnxruntime.dll",
        url: "",
        size: 15_809_848,
        sha256: "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257",
    },
});
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(target_os = "macos", target_arch = "aarch64"),
    all(windows, target_arch = "x86_64")
)))]
pub const RUNTIME: Option<Runtime> = None;

/// `local`'s files as installed: bge-m3's and this target's runtime library, each at its pin.
pub fn local_files() -> Option<Vec<Artifact>> {
    let runtime = RUNTIME.as_ref()?;
    Some(BGE_M3.iter().copied().chain([runtime.library]).collect())
}

/// The runtime library (`RUNTIME`) into `dir/onnxruntime/`, unless it already holds its pin.
pub fn fetch_runtime(dir: &Path, runtime: &Runtime) -> Result<()> {
    install(dir, runtime, &tar())
}

/// The system's `tar`: GNU tar reads a `.tgz` on Linux, and bsdtar reads both a `.tgz` and a
/// `.zip` on macOS and Windows. Windows' own is named by path, as Git's GNU tar can come first on
/// the PATH and reads no `.zip`.
fn tar() -> PathBuf {
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        return Path::new(&root).join("System32").join("tar.exe");
    }
    PathBuf::from("tar")
}

/// The archive at its pin (`fetch`), the one member written out by `tar -xOf`, which writes no
/// path of the archive's own, held to the library's pin, and the archive removed, all under one
/// hold of the directory's lock: a second setup must not write the same `.part`, or remove the
/// archive while this one reads it (Codex on #426).
fn install(dir: &Path, runtime: &Runtime, tar: &Path) -> Result<()> {
    allow_url(&runtime.archive.url.parse()?)?;
    fs::create_dir_all(dir)?;
    let _lock = lock(dir)?;
    let library = plain_path(dir, runtime.library.name)?;
    if check(&library, &runtime.library).is_ok() {
        return Ok(());
    }
    fetch_held(
        dir,
        std::slice::from_ref(&runtime.archive),
        crate::failure::free_bytes,
    )?;
    let archive = plain_path(dir, runtime.archive.name)?;
    let part = plain_path(dir, &format!("{}.part", runtime.library.name))?;
    let ran = Command::new(tar)
        .arg("-xOf")
        .arg(&archive)
        .arg(runtime.member)
        .stdin(Stdio::null())
        .stdout(File::create(&part)?)
        .stderr(Stdio::null())
        .status();
    if !matches!(ran, Ok(status) if status.success()) {
        remove(&part)?;
        return Err(match ran {
            Err(error) => anyhow::Error::new(error).context(format!(
                "running {} to take the runtime out of its archive; without it, place {} in {}",
                tar.display(),
                runtime.member,
                library.parent().unwrap_or(dir).display()
            )),
            Ok(_) => anyhow::anyhow!(
                "{} could not take {} out of {}",
                tar.display(),
                runtime.member,
                archive.display()
            ),
        });
    }
    finish(&part, &library, &runtime.library)?;
    remove(&archive)
}

pub fn fetch(dir: &Path, artifacts: &[Artifact]) -> Result<()> {
    fetch_with(dir, artifacts, crate::failure::free_bytes)
}

fn fetch_with(
    dir: &Path,
    artifacts: &[Artifact],
    free: impl Fn(&Path) -> Option<u64>,
) -> Result<()> {
    for artifact in artifacts {
        allow_url(&artifact.url.parse()?)?;
    }
    fs::create_dir_all(dir)?;
    let _lock = lock(dir)?;
    fetch_held(dir, artifacts, free)
}

/// `fetch_with` once its caller holds the directory's lock and has checked every URL.
fn fetch_held(
    dir: &Path,
    artifacts: &[Artifact],
    free: impl Fn(&Path) -> Option<u64>,
) -> Result<()> {
    let mut pending = Vec::new();
    let mut needed = MARGIN;
    for artifact in artifacts {
        let path = plain_path(dir, artifact.name)?;
        if check(&path, artifact).is_ok() {
            continue;
        }
        let part = plain_path(dir, &format!("{}.part", artifact.name))?;
        let held = match fs::metadata(&part) {
            Ok(metadata) if metadata.len() <= artifact.size => metadata.len(),
            Ok(_) => {
                remove(&part)?;
                0
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        needed = needed
            .checked_add(artifact.size - held)
            .context("model size overflow")?;
        pending.push(artifact);
    }
    if let Some(available) = free(dir) {
        ensure!(available >= needed, Stopped::NoSpace { needed, available });
    }
    let mut config = ureq::Agent::config_builder()
        .timeout_global(None)
        .timeout_resolve(Some(IDLE))
        .timeout_connect(Some(IDLE))
        .timeout_send_request(Some(IDLE))
        .timeout_recv_body(None)
        .http_status_as_error(false)
        // Followed in `download`, which holds each hop to `allow_url` and asks for the range again.
        .max_redirects(0)
        .accept_encoding("identity")
        .user_agent(concat!("oboete/", env!("CARGO_PKG_VERSION")));
    // As `provider::agent_config`: the environment's proxy for the internet, none for this machine.
    if artifacts
        .iter()
        .all(|artifact| crate::provider::is_loopback(artifact.url))
    {
        config = config.proxy(None);
    }
    let agent = ureq::Agent::with_parts(
        config.build(),
        ModelConnector::default(),
        DefaultResolver::default(),
    );
    for artifact in pending {
        download(&agent, dir, artifact)?;
    }
    Ok(())
}

fn download(agent: &ureq::Agent, dir: &Path, artifact: &Artifact) -> Result<()> {
    let path = plain_path(dir, artifact.name)?;
    remove(&path)?;
    fs::create_dir_all(path.parent().context("artifact has no parent")?)?;
    let part = plain_path(dir, &format!("{}.part", artifact.name))?;
    let held = match fs::metadata(&part) {
        Ok(metadata) => Some(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let offset = held.unwrap_or(0);
    if held.is_some() && offset == artifact.size {
        return finish(&part, &path, artifact);
    }
    let mut url = artifact.url.to_owned();
    let mut response = None;
    for _ in 0..=10 {
        let mut request = agent.get(&url);
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let reply = request
            .call()
            .with_context(|| format!("download {}", artifact.name))?;
        if !reply.status().is_redirection() {
            response = Some(reply);
            break;
        }
        let location = reply
            .headers()
            .get("Location")
            .and_then(|value| value.to_str().ok())
            .with_context(|| format!("{}: a redirect without a Location", artifact.name))?;
        url = redirect(&url, location)?;
    }
    let mut response =
        response.with_context(|| format!("{}: more than 10 redirects", artifact.name))?;
    let append = match response.status().as_u16() {
        200 => false,
        206 => {
            let last = artifact
                .size
                .checked_sub(1)
                .context("empty ranged artifact")?;
            let expected = format!("bytes {offset}-{last}/{}", artifact.size);
            let ranges = response.headers().get_all("Content-Range");
            ensure!(
                ranges.iter().count() == 1
                    && ranges.iter().next().and_then(|value| value.to_str().ok())
                        == Some(expected.as_str()),
                "{}: Content-Range must be {expected}",
                artifact.name
            );
            true
        }
        status => bail!("{}: download returned HTTP {status}", artifact.name),
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(&part)?;
    let mut written = if append { offset } else { 0 };
    let mut reader = response.body_mut().as_reader();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if read as u64 > artifact.size.saturating_sub(written) {
            drop(file);
            remove(&part)?;
            bail!("{}: body exceeds {} bytes", artifact.name, artifact.size);
        }
        file.write_all(&buffer[..read])?;
        written += read as u64;
    }
    drop(file);
    ensure!(
        written == artifact.size,
        "{}: download stopped at {written} of {} bytes",
        artifact.name,
        artifact.size
    );
    finish(&part, &path, artifact)
}

fn finish(part: &Path, path: &Path, artifact: &Artifact) -> Result<()> {
    if let Err(error) = check(part, artifact) {
        remove(part)?;
        return Err(error);
    }
    fs::rename(part, path)?;
    Ok(())
}

/// Where a redirect leads: an absolute URL, or a path on the same host, held to the pinned URLs'
/// rule (huggingface.co's own redirects are absolute; the tests' are paths). Any other form is
/// refused.
fn redirect(from: &str, location: &str) -> Result<String> {
    let url = match location.strip_prefix('/') {
        Some(rest) if !rest.starts_with('/') => {
            let from: ureq::http::Uri = from.parse()?;
            format!(
                "{}://{}{location}",
                from.scheme_str().context("a URL without a scheme")?,
                from.authority().context("a URL without a host")?
            )
        }
        _ => location.to_owned(),
    };
    allow_url(
        &url.parse()
            .with_context(|| format!("a redirect to {location}"))?,
    )?;
    Ok(url)
}

fn allow_url(uri: &ureq::http::Uri) -> Result<()> {
    let loopback = matches!(uri.host(), Some("127.0.0.1" | "[::1]" | "localhost"));
    ensure!(
        uri.host().is_some()
            && (uri.scheme_str() == Some("https")
                || (uri.scheme_str() == Some("http") && loopback))
            && !uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@')),
        "model URL requires HTTPS or HTTP loopback"
    );
    Ok(())
}

/// ureq's own connections, each wrapped in `ModelTransport`. It checks no URL: ureq connects to a
/// CONNECT proxy through this same connector, and `download` checks every model host itself.
#[derive(Debug, Default)]
struct ModelConnector(DefaultConnector);

impl Connector<()> for ModelConnector {
    type Out = ModelTransport;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<()>,
    ) -> std::result::Result<Option<Self::Out>, ureq::Error> {
        Ok(self
            .0
            .connect(details, chained)?
            .map(|inner| ModelTransport { inner }))
    }
}

/// The connection with a limit on each wait for the server, not on the whole body: ureq's
/// `recv_body` timeout covers the body at once, which a 2.3 GB download on a slow line outlasts.
/// No separate refusal of a `Content-Encoding`: ureq decodes gzip itself and drops the header, a
/// ranged reply in another encoding fails the exact `Content-Range` check, and a whole one
/// decodes to the pinned bytes or fails the pin.
#[derive(Debug)]
struct ModelTransport {
    inner: Box<dyn Transport>,
}

impl Transport for ModelTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> std::result::Result<bool, ureq::Error> {
        let idle = IDLE.into();
        let timeout = if timeout.after > idle {
            NextTimeout {
                after: idle,
                ..timeout
            }
        } else {
            timeout
        };
        self.inner.await_input(timeout)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

pub fn verify(dir: &Path, artifacts: &[Artifact]) -> Result<()> {
    let _lock = lock(dir)?;
    let marker = plain_path(dir, "verified")?;
    remove(&marker)?;
    let mut text = String::new();
    for artifact in artifacts {
        let path = plain_path(dir, artifact.name)?;
        text.push_str(&marker_line(artifact, &check(&path, artifact)?)?);
    }
    fs::write(marker, text)?;
    Ok(())
}

pub fn marker_ok(dir: &Path, artifacts: &[Artifact]) -> bool {
    let matches = || -> Result<bool> {
        let text = fs::read_to_string(plain_path(dir, "verified")?)?;
        let mut expected = String::new();
        for artifact in artifacts {
            let metadata = fs::symlink_metadata(plain_path(dir, artifact.name)?)?;
            ensure!(metadata.len() == artifact.size, "size changed");
            expected.push_str(&marker_line(artifact, &metadata)?);
        }
        Ok(text == expected)
    };
    matches().unwrap_or(false)
}

fn marker_line(artifact: &Artifact, metadata: &Metadata) -> Result<String> {
    let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(format!(
        "{}\t{}\t{}\t{modified}\n",
        artifact.name, artifact.size, artifact.sha256
    ))
}

fn plain_path(dir: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        fs::symlink_metadata(dir)?.is_dir(),
        "model directory must be a plain directory"
    );
    let mut path = dir.to_owned();
    let mut components = Path::new(name).components().peekable();
    ensure!(components.peek().is_some(), "empty artifact name");
    while let Some(component) = components.next() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "artifact name must stay in the model directory: {name}"
        );
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure!(
                if components.peek().is_some() {
                    metadata.is_dir()
                } else {
                    metadata.is_file()
                },
                "model path must be plain: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

pub(crate) fn check(path: &Path, artifact: &Artifact) -> Result<Metadata> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() == artifact.size,
        "{}: expected {} bytes, found {}",
        artifact.name,
        artifact.size,
        metadata.len()
    );
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    ensure!(
        format!("{:x}", hash.finalize()) == artifact.sha256,
        "{}: SHA-256 does not match the pin",
        artifact.name
    );
    Ok(metadata)
}

/// One fetch or verify at a time in a model's directory: a second fetch appending to a `.part` the
/// first has already renamed would write into the published file. Held while the file is.
fn lock(dir: &Path) -> Result<File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(plain_path(dir, ".lock")?)?;
    match crate::worker::try_lock(&file) {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => bail!(Stopped::Busy(dir.to_owned())),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests;
