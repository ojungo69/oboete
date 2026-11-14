//! d1 on this PC (#786): whether the program listening where the judge sends may be asked. On
//! Linux its owner is read from /proc/net's tables; elsewhere nothing here reads a socket's owner,
//! and the settings page says that whatever listens there receives what the judge is shown.

// Off Linux only the tests read the tables (#786).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why nothing goes, a call's detail: no program listens at the port, one that may not be asked
/// does, or the tables do not read.
pub(super) const NONE: &str = "no listener";
pub(super) const UNTRUSTED: &str = "untrusted listener";
pub(super) const UNREAD: &str = "listeners unread";

/// Whether the judge may send to `port` on 127.0.0.1: a program listens there, and each one such a
/// connection can reach is this user's, root's or the `ollama` account's, which Ollama's Linux
/// installer runs it as (only root makes an account).
#[cfg(target_os = "linux")]
pub(super) fn ours(port: u16) -> Result<(), &'static str> {
    // As text where bytes that are not UTF-8 (a program's name cut short, a full name in
    // /etc/passwd) do not keep the numbers around them from reading.
    let read = |path: &str| match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        // A kernel without IPv6 has no tcp6, and no listener in it.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && path.ends_with('6') => {
            Ok(String::new())
        }
        Err(_) => Err(UNREAD),
    };
    let (tcp, tcp6) = (read("/proc/net/tcp")?, read("/proc/net/tcp6")?);
    let euid = euid_of(&read("/proc/self/status")?).ok_or(UNREAD)?;
    let passwd = read("/etc/passwd").unwrap_or_default();
    listening(&tcp, &tcp6, port, &trusted(euid, &passwd))
}

/// Off Linux nothing reads a socket's owner: the settings page says what this leaves open.
#[cfg(not(target_os = "linux"))]
pub(super) fn ours(_port: u16) -> Result<(), &'static str> {
    Ok(())
}

/// Over `tcp` and `tcp6`, /proc/net's tables: the sockets listening at `port` where a connection
/// to 127.0.0.1 reaches them. None, or one whose owner is not of `trusted`, is a reason not to send.
fn listening(tcp: &str, tcp6: &str, port: u16, trusted: &[u32]) -> Result<(), &'static str> {
    let mut found = false;
    for line in tcp.lines().skip(1).chain(tcp6.lines().skip(1)) {
        let row = row(line).ok_or(UNREAD)?;
        if row.listens && row.port == port && reached(row.address) {
            if !trusted.contains(&row.uid) {
                return Err(UNTRUSTED);
            }
            found = true;
        }
    }
    found.then_some(()).ok_or(NONE)
}

/// A socket of a table: its local address and port, whether it listens, and its owner's uid.
struct Row {
    address: IpAddr,
    port: u16,
    listens: bool,
    uid: u32,
}

/// `line` of a table, as the kernel prints one: `sl local_address rem_address st … uid …`.
fn row(line: &str) -> Option<Row> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let (address, port) = fields.get(1)?.split_once(':')?;
    Some(Row {
        address: address_of(address)?,
        port: u16::from_str_radix(port, 16).ok()?,
        listens: *fields.get(3)? == "0A",
        uid: fields.get(7)?.parse().ok()?,
    })
}

/// An address as a table prints it: 8 hex digits for IPv4, 32 for IPv6, each 8 the next four of
/// its bytes read as a u32 in the machine's own order (`%08X`).
fn address_of(hex: &str) -> Option<IpAddr> {
    let words = (0..hex.len())
        .step_by(8)
        .map(|at| u32::from_str_radix(hex.get(at..at + 8)?, 16).ok())
        .collect::<Option<Vec<u32>>>()?;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
    match words.len() {
        1 => Some(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).ok()?).into()),
        4 => Some(Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?).into()),
        _ => None,
    }
}

/// Whether a connection to 127.0.0.1 reaches a socket at `a`: 127.0.0.1 or 0.0.0.0, or on a socket
/// for both families `::` or either of those mapped. Not ::1, which only IPv6 reaches.
fn reached(a: IpAddr) -> bool {
    let v4 = |a: Ipv4Addr| a == Ipv4Addr::LOCALHOST || a.is_unspecified();
    match a {
        IpAddr::V4(a) => v4(a),
        IpAddr::V6(a) => a.is_unspecified() || a.to_ipv4_mapped().is_some_and(v4),
    }
}

/// The effective uid in `status`, /proc/self/status's text: its `Uid:` line is real, effective,
/// saved and filesystem uid.
fn euid_of(status: &str) -> Option<u32> {
    let uids = status.lines().find_map(|line| line.strip_prefix("Uid:"))?;
    uids.split_whitespace().nth(1)?.parse().ok()
}

/// The owners a listener may have: this user (`euid`), root, and the `ollama` account of `passwd`,
/// /etc/passwd's text.
fn trusted(euid: u32, passwd: &str) -> Vec<u32> {
    let mut trusted = vec![euid, 0];
    trusted.extend(uid_of(passwd, "ollama"));
    trusted
}

/// The uid of the account `name` in `passwd`, /etc/passwd's text.
fn uid_of(passwd: &str, name: &str) -> Option<u32> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        fields.nth(1)?.parse().ok()
    })
}

// The tables' hex as a little-endian machine prints it, as the three CI hosts are.
#[cfg(all(test, target_endian = "little"))]
mod tests;
