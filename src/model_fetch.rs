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
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread::{self, JoinHandle},
        time::Duration,
    };

    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn artifact(url: &'static str) -> Artifact {
        Artifact {
            name: "onnx/model.onnx_data",
            url,
            size: 5,
            sha256: HELLO,
        }
    }

    struct Stub {
        url: &'static str,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Stub {
        fn new(reply: impl Fn(usize, &str) -> Vec<u8> + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = Box::leak(
                format!("http://{}/model", listener.local_addr().unwrap()).into_boxed_str(),
            );
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let seen = Arc::clone(&requests);
            let done = Arc::clone(&stop);
            let thread = thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(error) => panic!("stub accept: {error}"),
                    };
                    // Windows and BSD give an accepted socket its listener's non-blocking mode.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        let count = reader.read_line(&mut line).unwrap();
                        request.push_str(&line);
                        if count == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    let index = {
                        let mut seen = seen.lock().unwrap();
                        let index = seen.len();
                        seen.push(request.clone());
                        index
                    };
                    // A refused reply can close the connection before the stub has written it.
                    let _ = stream.write_all(&reply(index, &request));
                }
            });
            Self {
                url,
                requests,
                stop,
                thread: Some(thread),
            }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let stub = self.thread.take().unwrap().join();
            // A second panic while a failed test unwinds would abort every test in the binary.
            if !thread::panicking() {
                stub.unwrap();
            }
        }
    }

    fn reply(status: u16, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = format!(
            "HTTP/1.1 {status} Stub\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn a_download_resumes_exactly() {
        // gzip of "hello": a ranged reply in that encoding counts its range in these 25 bytes.
        const GZIP: [u8; 25] = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 203, 72, 205, 201, 201, 7, 0, 134, 166, 16, 54, 5, 0,
            0, 0,
        ];
        for case in ["redirect", "restart", "encoded"] {
            let stub = Stub::new(move |index, _| match (case, index) {
                ("redirect", 0) => reply(302, "Location: /storage\r\n", b""),
                ("redirect", _) => reply(206, "Content-Range: bytes 2-4/5\r\n", b"llo"),
                ("restart", _) => reply(200, "", b"hello"),
                ("encoded", _) => reply(
                    206,
                    "Content-Encoding: gzip\r\nContent-Range: bytes 2-24/25\r\n",
                    &GZIP[2..],
                ),
                _ => unreachable!(),
            });
            let dir = tempfile::tempdir().unwrap();
            let artifacts = [artifact(stub.url)];
            let file = dir.path().join(artifacts[0].name);
            let part = dir.path().join(format!("{}.part", artifacts[0].name));
            fs::create_dir(part.parent().unwrap()).unwrap();
            fs::write(&part, b"he").unwrap();
            let result = fetch(dir.path(), &artifacts);
            if case == "encoded" {
                assert!(result.is_err());
                assert_eq!(fs::read(&part).unwrap(), b"he");
                assert!(!file.exists());
            } else {
                result.unwrap();
                assert_eq!(fs::read(&file).unwrap(), b"hello");
                assert!(!part.exists());
                verify(dir.path(), &artifacts).unwrap();
                assert!(marker_ok(dir.path(), &artifacts));
            }
            let requests = stub.requests();
            assert_eq!(requests.len(), if case == "redirect" { 2 } else { 1 });
            for request in &requests {
                let lower = request.to_ascii_lowercase();
                assert!(lower.contains("range: bytes=2-\r\n"), "{request}");
                assert!(lower.contains("accept-encoding: identity\r\n"), "{request}");
                assert!(
                    lower.contains(concat!(
                        "user-agent: oboete/",
                        env!("CARGO_PKG_VERSION"),
                        "\r\n"
                    )),
                    "{request}"
                );
                assert!(!lower.contains("authorization:") && !lower.contains("cookie:"));
            }
            if case == "redirect" {
                assert!(requests[1].starts_with("GET /storage "));
            }
        }
    }

    #[test]
    fn a_redirect_off_the_rule_or_in_a_loop_is_refused() {
        for (location, error, requests) in [
            ("http://example.com/model", "HTTPS", 1),
            ("//example.com/model", "HTTPS", 1),
            ("/model", "more than 10 redirects", 11),
        ] {
            let stub = Stub::new(move |_, _| reply(302, &format!("Location: {location}\r\n"), b""));
            let dir = tempfile::tempdir().unwrap();
            let artifacts = [artifact(stub.url)];
            let part = dir.path().join(format!("{}.part", artifacts[0].name));
            fs::create_dir(part.parent().unwrap()).unwrap();
            fs::write(&part, b"he").unwrap();
            let result = fetch(dir.path(), &artifacts).unwrap_err();
            assert!(
                format!("{result:#}").contains(error),
                "{location}: {result:#}"
            );
            assert_eq!(stub.requests().len(), requests, "{location}");
            assert_eq!(fs::read(&part).unwrap(), b"he");
        }
    }

    #[test]
    fn one_fetch_or_verify_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = [artifact("http://127.0.0.1:1/model")];
        let file = dir.path().join(artifacts[0].name);
        fs::create_dir(file.parent().unwrap()).unwrap();
        fs::write(&file, b"hello").unwrap();
        let held = lock(dir.path()).unwrap();
        for result in [
            fetch_with(dir.path(), &artifacts, |_| None),
            verify(dir.path(), &artifacts),
        ] {
            let error = result.unwrap_err();
            assert!(format!("{error:#}").contains("another oboete"));
            assert!(matches!(error.downcast_ref(), Some(Stopped::Busy(_))));
        }
        drop(held);
        fetch_with(dir.path(), &artifacts, |_| None).unwrap();
        verify(dir.path(), &artifacts).unwrap();
        assert!(marker_ok(dir.path(), &artifacts));
    }

    #[test]
    fn a_download_off_the_pin_is_refused() {
        let stub = Stub::new(|_, _| reply(200, "", b"jello"));
        let dir = tempfile::tempdir().unwrap();
        let artifacts = [artifact(stub.url)];
        let file = dir.path().join(artifacts[0].name);
        let part = dir.path().join(format!("{}.part", artifacts[0].name));
        let error = fetch(dir.path(), &artifacts).unwrap_err();
        assert!(error.to_string().contains("SHA-256"), "{error:#}");
        assert_eq!(stub.requests().len(), 1);
        assert!(!file.exists());
        assert!(!part.exists());

        fs::write(&file, b"hello").unwrap();
        verify(dir.path(), &artifacts).unwrap();
        assert!(marker_ok(dir.path(), &artifacts));
        fs::write(&file, b"jello").unwrap();
        assert!(verify(dir.path(), &artifacts).is_err());
        assert!(!marker_ok(dir.path(), &artifacts));
    }

    #[test]
    fn free_space_is_checked_before_any_request() {
        let stub = Stub::new(|_, _| reply(200, "", b"hello"));
        let dir = tempfile::tempdir().unwrap();
        let artifacts = [artifact(stub.url)];
        let part = dir.path().join(format!("{}.part", artifacts[0].name));
        fs::create_dir(part.parent().unwrap()).unwrap();
        fs::write(&part, b"he").unwrap();
        let error = fetch_with(dir.path(), &artifacts, |_| Some(MARGIN + 2)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "model download needs 67108867 bytes; 67108866 bytes available"
        );
        // The settings page answers it with a code (W4).
        assert!(matches!(
            error.downcast_ref::<Stopped>(),
            Some(Stopped::NoSpace { .. })
        ));
        assert!(stub.requests().is_empty());
        assert_eq!(fs::read(&part).unwrap(), b"he");
        // A file that verifies needs no space beyond the margin, and no request.
        fs::write(dir.path().join(artifacts[0].name), b"hello").unwrap();
        fetch_with(dir.path(), &artifacts, |_| Some(MARGIN)).unwrap();
        assert!(stub.requests().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn free_bytes_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        assert!(crate::failure::free_bytes(dir.path()).is_some_and(|bytes| bytes > 0));
    }

    #[test]
    fn invalid_ranges_and_statuses_keep_the_partial_file() {
        for (status, range) in [
            (206, ""),
            (206, "Content-Range: bytes 1-4/5\r\n"),
            (206, "Content-Range: bytes 2-3/5\r\n"),
            (206, "Content-Range: bytes 2-4/6\r\n"),
            (
                206,
                "Content-Range: bytes 2-4/5\r\nContent-Range: bytes 2-4/5\r\n",
            ),
            (416, ""),
            (500, ""),
        ] {
            let stub = Stub::new(move |_, _| reply(status, range, b"llo"));
            let dir = tempfile::tempdir().unwrap();
            let artifacts = [artifact(stub.url)];
            let part = dir.path().join(format!("{}.part", artifacts[0].name));
            fs::create_dir(part.parent().unwrap()).unwrap();
            fs::write(&part, b"he").unwrap();
            assert!(fetch(dir.path(), &artifacts).is_err(), "{status} {range}");
            assert_eq!(fs::read(part).unwrap(), b"he");
            assert!(!dir.path().join(artifacts[0].name).exists());
        }
    }

    #[test]
    fn an_oversized_body_is_removed_and_a_short_body_can_resume() {
        for (body, kept) in [(b"hello!".as_slice(), false), (b"he".as_slice(), true)] {
            let stub = Stub::new(move |_, _| reply(200, "", body));
            let dir = tempfile::tempdir().unwrap();
            let artifacts = [artifact(stub.url)];
            assert!(fetch(dir.path(), &artifacts).is_err());
            let part = dir.path().join(format!("{}.part", artifacts[0].name));
            assert_eq!(part.exists(), kept);
            if kept {
                assert_eq!(fs::read(part).unwrap(), b"he");
            }
            assert!(!dir.path().join(artifacts[0].name).exists());
        }
    }

    #[test]
    fn a_verified_file_and_a_complete_part_need_no_request() {
        for name in ["onnx/model.onnx_data", "onnx/model.onnx_data.part"] {
            let dir = tempfile::tempdir().unwrap();
            let artifacts = [artifact("http://127.0.0.1:1/model")];
            let file = dir.path().join(name);
            fs::create_dir(file.parent().unwrap()).unwrap();
            fs::write(&file, b"hello").unwrap();
            fetch_with(dir.path(), &artifacts, |_| None).unwrap();
            verify(dir.path(), &artifacts).unwrap();
            assert!(marker_ok(dir.path(), &artifacts));
            assert!(
                !dir.path()
                    .join(format!("{}.part", artifacts[0].name))
                    .exists()
            );
        }
    }

    #[test]
    fn an_insecure_or_credentialed_url_is_refused_before_io() {
        for url in [
            "http://example.com/model",
            "http://127.0.0.2/model",
            "http://localhost.example.com/model",
            "ftp://127.0.0.1/model",
            "https://user:pass@example.com/model",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let artifacts = [artifact(url)];
            let error = fetch_with(dir.path(), &artifacts, |_| {
                panic!("URL checked before free space")
            })
            .unwrap_err();
            assert!(error.to_string().contains("HTTPS"), "{url}: {error:#}");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        }
        for url in [
            "https://example.com/model",
            "http://127.0.0.1/model",
            "http://localhost/model",
            "http://[::1]/model",
        ] {
            assert!(allow_url(&url.parse().unwrap()).is_ok(), "{url}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn model_files_and_their_parents_must_not_be_symbolic_links() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("model.onnx_data"), b"hello").unwrap();
        let artifacts = [artifact("http://127.0.0.1:1/model")];
        symlink(outside.path(), dir.path().join("onnx")).unwrap();
        assert!(fetch_with(dir.path(), &artifacts, |_| None).is_err());
        assert!(verify(dir.path(), &artifacts).is_err());
        assert!(!marker_ok(dir.path(), &artifacts));
        fs::remove_file(dir.path().join("onnx")).unwrap();
        fs::create_dir(dir.path().join("onnx")).unwrap();
        symlink(
            outside.path().join("model.onnx_data"),
            dir.path().join(artifacts[0].name),
        )
        .unwrap();
        assert!(fetch_with(dir.path(), &artifacts, |_| None).is_err());
        assert!(verify(dir.path(), &artifacts).is_err());
        assert_eq!(
            fs::read(outside.path().join("model.onnx_data")).unwrap(),
            b"hello"
        );
    }

    /// Line H with `model_fetch` itself (docs/spike/local-embeddings.md): the five files from
    /// huggingface.co and this target's runtime from github.com into the directory
    /// `OBOETE_MODEL_FETCH` names, resuming any `.part` there, then verified. By hand only: it
    /// downloads up to 2.4 GB.
    #[test]
    #[ignore]
    fn fetches_bge_m3_and_its_runtime() {
        let dir =
            std::env::var_os("OBOETE_MODEL_FETCH").expect("OBOETE_MODEL_FETCH names a directory");
        let dir = Path::new(&dir);
        fetch(dir, BGE_M3).unwrap();
        fetch_runtime(dir, RUNTIME.as_ref().unwrap()).unwrap();
        let all = local_files().unwrap();
        verify(dir, &all).unwrap();
        assert!(marker_ok(dir, &all));
    }

    #[test]
    fn the_runtime_comes_out_of_its_archive() {
        let src = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("x/lib")).unwrap();
        fs::write(src.path().join("x/lib/libfake"), b"runtime").unwrap();
        let tgz = src.path().join("a.tgz");
        let made = Command::new(tar())
            .arg("-czf")
            .arg(&tgz)
            .arg("-C")
            .arg(src.path())
            .arg("x/lib/libfake")
            .status()
            .unwrap();
        assert!(made.success());
        let bytes = fs::read(&tgz).unwrap();
        let sha = |b: &[u8]| -> &'static str {
            Box::leak(format!("{:x}", Sha256::digest(b)).into_boxed_str())
        };
        let served = bytes.clone();
        let stub = Stub::new(move |_, _| reply(200, "", &served));
        let runtime = |library_sha: &'static str| Runtime {
            archive: Artifact {
                name: "onnxruntime/a.tgz",
                url: stub.url,
                size: bytes.len() as u64,
                sha256: sha(&bytes),
            },
            member: "x/lib/libfake",
            library: Artifact {
                name: "onnxruntime/libfake",
                url: "",
                size: 7,
                sha256: library_sha,
            },
        };
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("onnxruntime/libfake");
        let part = dir.path().join("onnxruntime/libfake.part");

        let off_pin = install(dir.path(), &runtime(HELLO), &tar()).unwrap_err();
        assert!(format!("{off_pin:#}").contains("SHA-256"), "{off_pin:#}");
        let no_tar = install(
            dir.path(),
            &runtime(sha(b"runtime")),
            Path::new("/no/such/tar"),
        );
        assert!(format!("{:#}", no_tar.unwrap_err()).contains("place x/lib/libfake in"));
        assert!(!library.exists() && !part.exists());

        install(dir.path(), &runtime(sha(b"runtime")), &tar()).unwrap();
        assert_eq!(fs::read(&library).unwrap(), b"runtime");
        assert!(!dir.path().join("onnxruntime/a.tgz").exists());
        install(dir.path(), &runtime(sha(b"runtime")), &tar()).unwrap();
        // One download in all: the archive verified the second time, the library the last.
        assert_eq!(stub.requests().len(), 1);
    }

    /// Codex on #426: the lock is held while `tar` writes the library's `.part`, not only while
    /// the archive downloads, so a second setup cannot write the same file meanwhile. The fake
    /// `tar` waits for the test to look first (CodeRabbit on #426: a sleep did not promise it).
    #[cfg(unix)]
    #[test]
    fn the_runtime_is_taken_out_under_the_lock() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let slow_tar = bin.path().join("tar");
        let go = bin.path().join("go");
        // At most 30 s: a test that fails before it opens the gate ends instead of hanging.
        let script = format!(
            "#!/bin/sh\ni=0\nwhile [ ! -e '{}' ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done\nprintf runtime\n",
            go.display()
        );
        fs::write(&slow_tar, script).unwrap();
        fs::set_permissions(&slow_tar, fs::Permissions::from_mode(0o755)).unwrap();
        let sha = |b: &[u8]| -> &'static str {
            Box::leak(format!("{:x}", Sha256::digest(b)).into_boxed_str())
        };
        let stub = Stub::new(|_, _| reply(200, "", b"archive"));
        let runtime = Runtime {
            archive: Artifact {
                name: "onnxruntime/a.tgz",
                url: stub.url,
                size: 7,
                sha256: sha(b"archive"),
            },
            member: "x/lib/libfake",
            library: Artifact {
                name: "onnxruntime/libfake",
                url: "",
                size: 7,
                sha256: sha(b"runtime"),
            },
        };
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("onnxruntime/libfake.part");
        thread::scope(|scope| {
            let installing = scope.spawn(|| install(dir.path(), &runtime, &slow_tar));
            let started = std::time::Instant::now();
            while !part.exists() {
                assert!(started.elapsed() < Duration::from_secs(10), "tar never ran");
                thread::sleep(Duration::from_millis(10));
            }
            let second = lock(dir.path()).map(drop);
            fs::write(&go, "").unwrap();
            let refused = second.unwrap_err();
            assert!(
                format!("{refused:#}").contains("another oboete"),
                "{refused:#}"
            );
            installing.join().unwrap().unwrap();
        });
        assert_eq!(
            fs::read(dir.path().join("onnxruntime/libfake")).unwrap(),
            b"runtime"
        );
    }

    #[test]
    fn marker_ok_follows_the_pins_and_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = [artifact("https://example.com/model")];
        let file = dir.path().join(artifacts[0].name);
        fs::create_dir(file.parent().unwrap()).unwrap();
        fs::write(&file, b"hello").unwrap();
        assert!(!marker_ok(dir.path(), &artifacts));
        verify(dir.path(), &artifacts).unwrap();
        assert!(marker_ok(dir.path(), &artifacts));

        let changed = [Artifact {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            ..artifact(artifacts[0].url)
        }];
        assert!(!marker_ok(dir.path(), &changed));
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        // Windows sets a time only through a handle opened for writing.
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(modified + Duration::from_secs(10))
            .unwrap();
        assert!(!marker_ok(dir.path(), &artifacts));
        verify(dir.path(), &artifacts).unwrap();
        assert!(marker_ok(dir.path(), &artifacts));
        fs::remove_file(dir.path().join("verified")).unwrap();
        assert!(!marker_ok(dir.path(), &artifacts));
    }
}
