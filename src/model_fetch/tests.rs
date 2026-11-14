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
        let url =
            Box::leak(format!("http://{}/model", listener.local_addr().unwrap()).into_boxed_str());
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
        31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 203, 72, 205, 201, 201, 7, 0, 134, 166, 16, 54, 5, 0, 0, 0,
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
    let dir = std::env::var_os("OBOETE_MODEL_FETCH").expect("OBOETE_MODEL_FETCH names a directory");
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
