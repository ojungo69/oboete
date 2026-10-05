//! Per-home ordering between forget registration and provider dispatch.
//! The file is coordination only; raw.db remains the sole deletion authority.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ureq::config::Config;
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{
    Buffers, ConnectProxyConnector, ConnectionDetails, Connector, Either, NextTimeout,
    RustlsConnector, TcpConnector, Transport,
};

pub(crate) type Guard = Arc<Admission>;

#[derive(Debug)]
pub(crate) struct Admission {
    held: Mutex<Option<File>>,
    body_read: AtomicBool,
    write_failed: AtomicBool,
    #[cfg(all(test, unix))]
    partial: Mutex<Option<tests::Checkpoint>>,
    #[cfg(all(test, unix))]
    receiving: Mutex<Option<tests::Checkpoint>>,
}

impl Admission {
    pub(crate) fn shared(home: &Path) -> Result<Guard> {
        Ok(Arc::new(Self {
            held: Mutex::new(Some(lock(home, false, crate::db::OPEN_WRITE_WAIT)?)),
            body_read: AtomicBool::new(false),
            write_failed: AtomicBool::new(false),
            #[cfg(all(test, unix))]
            partial: Mutex::new(None),
            #[cfg(all(test, unix))]
            receiving: Mutex::new(None),
        }))
    }

    pub(crate) fn release(&self) {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// Taken before a live raw write transaction, never by hook opens or appends.
pub(crate) fn exclusive(home: &Path) -> Result<File> {
    lock(home, true, crate::db::OPEN_WRITE_WAIT)
}

fn lock(home: &Path, exclusive: bool, wait: Duration) -> Result<File> {
    let path = home.join("dispatch.lock");
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "dispatch coordination is not a regular file"
    );
    let deadline = Instant::now() + wait;
    loop {
        let result = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match result {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "provider dispatch or forget is busy: try again",
                )
                .into());
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).context("lock provider dispatch");
            }
        }
    }
}

struct TrackedBody {
    bytes: Cursor<Vec<u8>>,
    admission: Guard,
}

impl Read for TrackedBody {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.bytes.read(out)?;
        if self.bytes.position() == self.bytes.get_ref().len() as u64 {
            self.admission.body_read.store(true, Ordering::Release);
        }
        Ok(n)
    }
}

#[derive(Debug)]
struct ObservedTcp {
    admission: Guard,
}

#[derive(Debug)]
struct Observed<T: Transport> {
    inner: T,
    admission: Guard,
}

fn failed_write() -> ureq::Error {
    ureq::Error::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "the physical dispatch write failed",
    ))
}

impl<T: Transport> Transport for Observed<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        if self.admission.write_failed.load(Ordering::Acquire) {
            #[cfg(all(test, unix))]
            if let Some(receiving) = self.admission.receiving.lock().unwrap().take() {
                receiving.stop();
            }
            return Err(failed_write());
        }
        #[cfg(all(test, unix))]
        if self.admission.body_read.load(Ordering::Acquire)
            && let Some(partial) = self.admission.partial.lock().unwrap().take()
        {
            self.inner.transmit_output(amount / 2, timeout)?;
            partial.stop();
            self.admission.write_failed.store(true, Ordering::Release);
            return Err(failed_write());
        }
        let sent = self.inner.transmit_output(amount, timeout);
        if sent.is_err() {
            self.admission.write_failed.store(true, Ordering::Release);
        }
        sent
    }

    fn maybe_await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        if self.admission.write_failed.load(Ordering::Acquire) {
            #[cfg(all(test, unix))]
            if let Some(receiving) = self.admission.receiving.lock().unwrap().take() {
                receiving.stop();
            }
            return Err(failed_write());
        }
        // At physical TCP below TLS, this read follows successful pending TLS writes. A body
        // reader EOF or an outer TLS write alone cannot prove that. CONNECT/100/TLS handshake
        // reads precede body consumption and keep the hold; failed physical writes never unlock.
        if self.admission.body_read.load(Ordering::Acquire) {
            self.admission.release();
            #[cfg(all(test, unix))]
            if let Some(receiving) = self.admission.receiving.lock().unwrap().take() {
                receiving.stop();
            }
        }
        self.inner.maybe_await_input(timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.maybe_await_input(timeout)
    }
    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }
    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

impl<In: Transport> Connector<In> for ObservedTcp {
    type Out = Either<In, Observed<<TcpConnector as Connector<()>>::Out>>;
    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        // Match the current dependency's missing-feature refusal. The application selects
        // Rustls only. Its unsupported environment-SOCKS fallback still uses native TCP;
        // DefaultConnector's private log warning is not a transport capability.
        if chained.is_none()
            && let Some(proxy) = details.config.proxy()
            && matches!(
                proxy.protocol(),
                ureq::ProxyProtocol::Socks4
                    | ureq::ProxyProtocol::Socks4A
                    | ureq::ProxyProtocol::Socks5
                    | ureq::ProxyProtocol::Socks5h
            )
            && !proxy.is_from_env()
        {
            panic!("Enable feature socks-proxy to use manually configured proxy");
        }
        if details.needs_tls()
            && details.config.tls_config().provider() != ureq::tls::TlsProvider::Rustls
            && !chained.as_ref().is_some_and(Transport::is_tls)
        {
            panic!("uri scheme is https but the selected TLS provider feature is not enabled");
        }
        // A CONNECT tunnel already contains the observed physical TCP connection. Passing it
        // through retains proxy TLS below target TLS; an extra observer above it unlocks early.
        if let Some(transport) = chained {
            return Ok(Some(Either::A(transport)));
        }
        let connected = TcpConnector::default().connect(details, None::<()>)?;
        Ok(connected.map(|inner| {
            Either::B(Observed {
                inner,
                admission: Arc::clone(&self.admission),
            })
        }))
    }
}

pub(crate) fn agent(config: Config, admission: Guard) -> ureq::Agent {
    let connector =
        ().chain(ConnectProxyConnector::default())
            .chain(ObservedTcp { admission })
            .chain(RustlsConnector::default());
    ureq::Agent::with_parts(config, connector, DefaultResolver::default())
}

/// Same JSON bytes and default framing as ureq's from_json, with consumption tracked separately
/// from the physical transport. All return paths release a refusal/failed or already-sent call.
pub(crate) fn json(
    mut request: ureq::RequestBuilder<ureq::typestate::WithBody>,
    value: &impl serde::Serialize,
    admission: &Guard,
) -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let result = (|| {
        let bytes = serde_json::to_vec_pretty(value)?;
        if let Some(headers) = request.headers_mut() {
            if !headers.contains_key("content-length") && !headers.contains_key("transfer-encoding")
            {
                headers.insert("content-length", bytes.len().into());
            }
            if !headers.contains_key("content-type") {
                headers.insert(
                    "content-type",
                    "application/json; charset=utf-8".parse().unwrap(),
                );
            }
        }
        let body = ureq::SendBody::from_owned_reader(TrackedBody {
            bytes: Cursor::new(bytes),
            admission: Arc::clone(admission),
        });
        request.send(body)
    })();
    admission.release();
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::agent as observed_agent;
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

    #[derive(Debug)]
    pub(super) struct Checkpoint {
        reached: SyncSender<()>,
        resume: Mutex<Receiver<()>>,
    }
    impl Checkpoint {
        pub(super) fn stop(&self) {
            self.reached.send(()).unwrap();
            self.resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
        }
    }
    struct Server {
        child: Child,
        lines: Receiver<String>,
        reader: Option<std::thread::JoinHandle<()>>,
        url: String,
    }

    impl Server {
        fn line(&self) -> String {
            self.lines
                .recv_timeout(Duration::from_secs(6))
                .expect("the synthetic server did not emit its bounded checkpoint")
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                reader.join().unwrap();
            }
        }
    }

    fn server(home: &Path, tls: bool, proxy: &str) -> Server {
        if tls {
            let generated = Command::new("openssl")
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-days",
                    "1",
                    "-subj",
                    "/CN=localhost",
                    "-addext",
                    "subjectAltName=IP:127.0.0.1",
                    "-addext",
                    "basicConstraints=critical,CA:FALSE",
                    "-addext",
                    "extendedKeyUsage=serverAuth",
                    "-keyout",
                ])
                .arg(home.join("key.pem"))
                .arg("-out")
                .arg(home.join("cert.pem"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                generated.success(),
                "synthetic TLS certificate generation failed"
            );
        }
        let script = r#"
import json, socket, ssl, sys
listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen(1)
print(listener.getsockname()[1], flush=True)
connection, _ = listener.accept()
connection.settimeout(5)
if sys.argv[2] == 'tls':
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(sys.argv[1] + '/cert.pem', sys.argv[1] + '/key.pem')
    if sys.argv[3] in ('direct', 'https'):
        connection = context.wrap_socket(connection, server_side=True)
if sys.argv[3] != 'direct':
    connect = b''
    while b'\r\n\r\n' not in connect:
        connect += connection.recv(1)
    if not connect.startswith(b'CONNECT '):
        raise SystemExit('not a CONNECT request')
    print('CONNECT received', flush=True)
    if sys.stdin.readline().strip() != 'tunnel':
        raise SystemExit('tunnel was not requested')
    connection.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
    if sys.argv[3] == 'http':
        connection = context.wrap_socket(connection, server_side=True)
    else:
        # Standard-library TLS-in-TLS fixture: the production client still uses native ureq.
        class TargetTLS:
            def __init__(self, lower):
                self.lower = lower
                self.incoming, self.outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
                self.session = context.wrap_bio(self.incoming, self.outgoing, server_side=True)
                self.drive(self.session.do_handshake)
            def flush(self):
                while self.outgoing.pending:
                    self.lower.sendall(self.outgoing.read())
            def drive(self, action):
                while True:
                    try:
                        result = action()
                        self.flush()
                        return result
                    except ssl.SSLWantReadError:
                        self.flush()
                        data = self.lower.recv(65536)
                        if not data:
                            self.incoming.write_eof()
                        else:
                            self.incoming.write(data)
                    except ssl.SSLWantWriteError:
                        self.flush()
            def recv(self, count):
                return self.drive(lambda: self.session.read(count))
            def sendall(self, data):
                while data:
                    sent = self.drive(lambda: self.session.write(data))
                    data = data[sent:]
            def close(self):
                self.lower.close()
        connection = TargetTLS(connection)
head = b''
while b'\r\n\r\n' not in head:
    piece = connection.recv(1)
    if not piece:
        raise SystemExit('request headers ended early')
    head += piece
length = next(int(line.split(b':', 1)[1]) for line in head.split(b'\r\n')
              if line.lower().startswith(b'content-length:'))
if b'expect: 100-continue' in head.lower():
    print('continue awaited', flush=True)
    if sys.stdin.readline().strip() != 'continue':
        raise SystemExit('continue was not requested')
    connection.sendall(b'HTTP/1.1 100 Continue\r\n\r\n')
body = b''
while len(body) < length:
    piece = connection.recv(length - len(body))
    if not piece:
        raise SystemExit('request body ended early')
    body += piece
if json.loads(body) != {'prompt':'synthetic-dispatch-canary'}:
    raise SystemExit('unexpected synthetic request body')
print('body received', flush=True)
if sys.stdin.readline().strip() != 'respond':
    raise SystemExit('response was not requested')
connection.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}')
connection.close()
"#;
        let mut child = Command::new("python3")
            .args(["-u", "-c", script])
            .arg(home)
            .arg(if tls { "tls" } else { "plain" })
            .arg(proxy)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let (emitted, lines) = sync_channel(4);
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if emitted.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Server {
            child,
            lines,
            reader: Some(reader),
            url: String::new(),
        };
        let port = server.line();
        let port = port
            .trim()
            .parse::<u16>()
            .expect("the synthetic server's port");
        server.url = format!("{}://127.0.0.1:{port}/", if tls { "https" } else { "http" });
        server
    }

    fn released_before_response(tls: bool, proxy: &str, expect: bool) {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("admission.lock");
        let held = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        held.lock_shared().unwrap();
        let admission = Arc::new(Admission {
            held: Mutex::new(Some(held)),
            body_read: AtomicBool::new(false),
            write_failed: AtomicBool::new(false),
            partial: Mutex::new(None),
            receiving: Mutex::new(None),
        });
        let mut server = server(home.path(), tls, proxy);
        let mut config = Config::builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(5)));
        if tls {
            let cert = std::fs::read(home.path().join("cert.pem")).unwrap();
            let roots = ureq::tls::RootCerts::Specific(
                vec![ureq::tls::Certificate::from_pem(&cert).unwrap()].into(),
            );
            config = config.tls_config(ureq::tls::TlsConfig::builder().root_certs(roots).build());
        }
        if proxy != "direct" {
            let address = server.url.split_once("://").unwrap().1;
            config = config.proxy(Some(
                ureq::Proxy::new(&format!("{proxy}://{address}")).unwrap(),
            ));
        }
        let url = server.url.clone();
        let sending = Arc::clone(&admission);
        let agent = observed_agent(config.build(), Arc::clone(&admission));
        let sender = std::thread::spawn(move || {
            let mut request = agent.post(&url);
            if expect {
                request = request.header("Expect", "100-continue");
            }
            let answer = super::json(
                request,
                &serde_json::json!({
                    "prompt":"synthetic-dispatch-canary",
                }),
                &sending,
            );
            answer.map(|r| r.status().as_u16())
        });
        if proxy != "direct" {
            let connect = server.line();
            assert_eq!(connect.trim(), "CONNECT received");
            let registration = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            assert!(
                matches!(
                    registration.try_lock(),
                    Err(std::fs::TryLockError::WouldBlock)
                ),
                "CONNECT must not release admission before the target request body"
            );
            writeln!(server.child.stdin.as_mut().unwrap(), "tunnel").unwrap();
        }
        if expect {
            assert_eq!(server.line(), "continue awaited");
            let registration = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            assert!(
                matches!(
                    registration.try_lock(),
                    Err(std::fs::TryLockError::WouldBlock)
                ),
                "100-continue wait released admission before the body"
            );
            writeln!(server.child.stdin.as_mut().unwrap(), "continue").unwrap();
        }
        let received = server.line();
        assert_eq!(received.trim(), "body received");
        assert!(admission.body_read.load(Ordering::Acquire));
        let registration = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let until = Instant::now() + Duration::from_millis(300);
        let mut acquired = false;
        while Instant::now() < until {
            if registration.try_lock().is_ok() {
                acquired = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // Always settle the fixture before reporting failure: no detached server or sender.
        writeln!(server.child.stdin.as_mut().unwrap(), "respond").unwrap();
        assert_eq!(sender.join().unwrap().unwrap(), 200);
        assert!(
            acquired,
            "registration remained blocked while response headers were withheld"
        );
    }

    #[test]
    fn native_http_releases_admission_after_body_before_response_headers() {
        released_before_response(false, "direct", false);
    }

    #[test]
    fn native_tls_releases_admission_after_body_before_response_headers() {
        released_before_response(true, "direct", false);
    }

    #[test]
    fn native_connect_keeps_admission_through_tunnel_then_releases_after_body() {
        released_before_response(true, "http", false);
    }

    #[test]
    fn native_https_proxy_keeps_admission_through_both_tls_layers() {
        released_before_response(true, "https", false);
    }

    #[test]
    fn native_tls_keeps_admission_through_expect_continue() {
        released_before_response(true, "direct", true);
    }

    fn serve_redirects(
        listener: std::net::TcpListener,
        waiting: Arc<AtomicBool>,
        status: u16,
    ) -> Vec<(String, Vec<u8>)> {
        let mut seen = Vec::new();
        'requests: for n in 0..2 {
            let until = Instant::now() + Duration::from_secs(2);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if waiting.load(Ordering::Acquire) {
                            break 'requests;
                        }
                        assert!(Instant::now() < until, "redirect did not connect");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("redirect accept: {e}"),
                }
            };
            // Accepted sockets can inherit nonblocking mode from the listener on BSD.
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).unwrap(), 1);
                head.push(byte[0]);
            }
            // The short probe detects a consumed redirect body, after headers arrive.
            socket
                .set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let header = String::from_utf8(head).unwrap();
            let method = header.split_whitespace().next().unwrap().to_owned();
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|len| len.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            let mut read = 0;
            while read < length {
                match socket.read(&mut body[read..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => read += n,
                }
            }
            body.truncate(read);
            seen.push((method, body));
            let reply = if n == 0 {
                format!(
                    "HTTP/1.1 {status} Redirect\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".into()
            };
            let _ = socket.write_all(reply.as_bytes());
        }
        seen
    }

    /// Existing ureq redirects switch 302 to a bodyless GET; 307 retains POST but does not
    /// rewind an already-consumed JSON body. The admitted reader must keep both behaviours.
    #[test]
    fn native_redirect_method_and_body_semantics_match_original_json() {
        fn trace(guarded: bool, status: u16) -> (bool, Vec<(String, Vec<u8>)>) {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let finished = Arc::new(AtomicBool::new(false));
            let waiting = Arc::clone(&finished);
            let server = std::thread::spawn(move || serve_redirects(listener, waiting, status));
            let config = Config::builder()
                .proxy(None)
                .max_redirects(10)
                .timeout_global(Some(Duration::from_secs(2)))
                .build();
            let value = serde_json::json!({"prompt":"synthetic-redirect-canary"});
            let answer = if guarded {
                let home = tempfile::tempdir().unwrap();
                let admission = Admission::shared(home.path()).unwrap();
                let agent = observed_agent(config, Arc::clone(&admission));
                super::json(agent.post(&url), &value, &admission)
            } else {
                let agent: ureq::Agent = config.into();
                agent.post(&url).send_json(&value)
            };
            finished.store(true, Ordering::Release);
            (answer.is_ok(), server.join().unwrap())
        }
        for status in [302, 307] {
            let native = trace(false, status);
            let admitted = trace(true, status);
            assert_eq!(
                admitted, native,
                "redirect {status} changed native semantics"
            );
            assert_eq!(admitted.1[0].0, "POST");
            if let Some(second) = admitted.1.get(1) {
                assert_eq!(second.0, if status == 302 { "GET" } else { "POST" });
                assert!(
                    second.1.is_empty(),
                    "redirect replayed content after release"
                );
            } else {
                assert_eq!(status, 307);
                assert!(
                    !admitted.0,
                    "a retained-body redirect unexpectedly succeeded"
                );
            }
        }
    }

    #[test]
    fn a_native_tls_partial_write_cannot_release_admission_at_a_receive_wait() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("admission.lock");
        let held = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        held.lock_shared().unwrap();
        let (partial_at, partial) = sync_channel(1);
        let (continue_partial, resumed_partial) = sync_channel(1);
        let (receive_at, receiving) = sync_channel(1);
        let (continue_receive, resumed_receive) = sync_channel(1);
        let admission = Arc::new(Admission {
            held: Mutex::new(Some(held)),
            body_read: AtomicBool::new(false),
            write_failed: AtomicBool::new(false),
            partial: Mutex::new(Some(Checkpoint {
                reached: partial_at,
                resume: Mutex::new(resumed_partial),
            })),
            receiving: Mutex::new(Some(Checkpoint {
                reached: receive_at,
                resume: Mutex::new(resumed_receive),
            })),
        });
        let server = server(home.path(), true, "direct");
        let cert = std::fs::read(home.path().join("cert.pem")).unwrap();
        let roots = ureq::tls::RootCerts::Specific(
            vec![ureq::tls::Certificate::from_pem(&cert).unwrap()].into(),
        );
        let config = Config::builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(2)))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(roots).build())
            .build();
        let agent = observed_agent(config, Arc::clone(&admission));
        let url = server.url.clone();
        let sending = Arc::clone(&admission);
        let sender = std::thread::spawn(move || {
            let bytes = br#"{"prompt":"synthetic-dispatch-canary"}"#.to_vec();
            let length = bytes.len();
            let body = ureq::SendBody::from_owned_reader(TrackedBody {
                bytes: Cursor::new(bytes),
                admission: Arc::clone(&sending),
            });
            let answer = agent
                .post(&url)
                .header("Content-Length", length.to_string())
                .send(body);
            if let Some(receiving) = sending.receiving.lock().unwrap().take() {
                receiving.stop();
            }
            sending.release();
            answer.is_err()
        });
        partial.recv_timeout(Duration::from_secs(3)).unwrap();
        let registration = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(
            matches!(
                registration.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ),
            "reading all body bytes alone released admission"
        );
        continue_partial.send(()).unwrap();
        receiving.recv_timeout(Duration::from_secs(3)).unwrap();
        let premature = registration.try_lock().is_ok();
        if premature {
            registration.unlock().unwrap();
        }
        continue_receive.send(()).unwrap();
        assert!(
            sender.join().unwrap(),
            "the deliberately damaged TLS request succeeded"
        );
        assert!(
            !premature,
            "a failed physical TLS write released admission before the call failed"
        );
        assert!(
            admission.held.lock().unwrap().is_none(),
            "failed calls must release their own admission descriptor"
        );
        crate::worker::try_lock(&registration).expect(
            "failed calls must permit registration after transient inherited descriptors close",
        );
    }
}
