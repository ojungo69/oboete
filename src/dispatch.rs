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
    exclusive_within(home, crate::db::OPEN_WRITE_WAIT)
}

/// `exclusive`, waiting `wait` at most.
pub(crate) fn exclusive_within(home: &Path, wait: Duration) -> Result<File> {
    lock(home, true, wait)
}

/// Whether `e` is a hold that did not come within its wait.
pub(crate) fn busy(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::WouldBlock)
}

/// Milestone 5 D5 (#439): a reader's shared hold, from its first read until its answer is built,
/// so no forget commits while a read is in flight. Taken after raw.db is open: a restore and a
/// registration take raw.lock first too. Waits `wait` at most for an exclusive holder.
pub(crate) fn reading(home: &Path, wait: Duration) -> Result<File> {
    lock(home, false, wait)
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
                #[cfg(test)]
                if exclusive {
                    BLOCKED.lock().unwrap().insert(path.clone());
                }
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

/// Tests (#439): the lock files an exclusive hold has waited on, so a test knows a registration
/// has met a reader's fence before it checks that the registration waits (Greptile on #446).
#[cfg(test)]
static BLOCKED: std::sync::Mutex<std::collections::BTreeSet<std::path::PathBuf>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Tests: whether an exclusive hold has waited on `home`'s lock since the last time this asked.
#[cfg(test)]
pub(crate) fn blocked(home: &Path) -> bool {
    BLOCKED.lock().unwrap().remove(&home.join("dispatch.lock"))
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
mod tests;
