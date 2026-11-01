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
mod tests {
    use super::*;

    const PORT: u16 = 11434;

    /// A table of /proc/net with `rows`: each socket's local address, its state (`0A` listens) and
    /// its owner's uid, as the kernel prints them.
    fn table(rows: &[(&str, &str, u32)]) -> String {
        let mut text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when \
                        retrnsmt   uid  timeout inode\n"
            .to_owned();
        for (n, (local, state, uid)) in rows.iter().enumerate() {
            text.push_str(&format!(
                "   {n}: {local} 00000000:0000 {state} 00000000:00000000 00:00000000 00000000 \
                 {uid:>5}        0 {} 1 0000000000000000 100 0 0 10 0\n",
                1000 + n
            ));
        }
        text
    }

    /// A listener of a trusted owner where a connection to 127.0.0.1 reaches it may be asked, at
    /// 127.0.0.1 or 0.0.0.0, or for both families at :: (as Ollama's service listens, owned by its
    /// own account).
    #[test]
    fn a_trusted_listener_the_connection_reaches_is_asked() {
        let at = |local: &str| table(&[(local, "0A", 1000)]);
        assert_eq!(
            listening(&at("0100007F:2CAA"), "", PORT, &[1000, 0]),
            Ok(())
        );
        assert_eq!(
            listening(&at("00000000:2CAA"), "", PORT, &[1000, 0]),
            Ok(())
        );
        let ollama = table(&[("00000000000000000000000000000000:2CAA", "0A", 999)]);
        assert_eq!(
            listening(&table(&[]), &ollama, PORT, &[1000, 0, 999]),
            Ok(())
        );
    }

    /// A listener of an owner not trusted where the connection reaches it refuses, beside a trusted
    /// one too; so does one mapped from IPv4 on a socket of both families.
    #[test]
    fn an_untrusted_listener_the_connection_reaches_refuses() {
        let other = table(&[("00000000:2CAA", "0A", 1001)]);
        assert_eq!(listening(&other, "", PORT, &[1000, 0]), Err(UNTRUSTED));
        let both = table(&[("0100007F:2CAA", "0A", 1000), ("00000000:2CAA", "0A", 1001)]);
        assert_eq!(listening(&both, "", PORT, &[1000, 0]), Err(UNTRUSTED));
        let mapped = table(&[("0000000000000000FFFF00000100007F:2CAA", "0A", 1001)]);
        assert_eq!(listening("", &mapped, PORT, &[1000, 0]), Err(UNTRUSTED));
        let any = table(&[("0000000000000000FFFF000000000000:2CAA", "0A", 1001)]);
        assert_eq!(listening("", &any, PORT, &[1000, 0]), Err(UNTRUSTED));
    }

    /// What the connection cannot reach is not counted: another port, a socket that does not
    /// listen, 127.0.0.2 and ::1. With nothing else at the port there is no listener.
    #[test]
    fn what_the_connection_cannot_reach_is_not_counted() {
        let elsewhere = table(&[
            ("0100007F:207D", "0A", 1001),
            ("0100007F:2CAA", "01", 1001),
            ("0200007F:2CAA", "0A", 1001),
        ]);
        let v6 = table(&[("00000000000000000000000001000000:2CAA", "0A", 1001)]);
        assert_eq!(listening(&elsewhere, &v6, PORT, &[1000, 0]), Err(NONE));
        let ours = table(&[("0100007F:2CAA", "0A", 1000)]);
        let with_ours = format!("{elsewhere}{}", ours.lines().nth(1).unwrap());
        assert_eq!(listening(&with_ours, &v6, PORT, &[1000, 0]), Ok(()));
        assert_eq!(listening("", "", PORT, &[1000, 0]), Err(NONE));
    }

    /// A row that does not read as the kernel prints one refuses: nothing is sent on a guess.
    #[test]
    fn a_row_that_does_not_read_refuses() {
        let ours = table(&[("0100007F:2CAA", "0A", 1000)]);
        for bad in [
            "   9: 0100007F:2CAA",
            "   9: 0100007G:2CAA 0 0A 0 0 0 1000",
            "   9: x",
        ] {
            let text = format!("{ours}{bad}\n");
            assert_eq!(listening(&text, "", PORT, &[1000, 0]), Err(UNREAD), "{bad}");
        }
        let short = table(&[("100007F:2CAA", "0A", 1000)]);
        assert_eq!(listening(&short, "", PORT, &[1000, 0]), Err(UNREAD));
    }

    /// This user is the effective uid of /proc/self/status, the second of its `Uid:` line.
    #[test]
    fn this_user_is_the_effective_uid() {
        let status = "Name:\toboete\nUmask:\t0022\nUid:\t1000\t1001\t1002\t1003\n\
                      Gid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(euid_of(status), Some(1001));
        assert_eq!(euid_of("Name:\toboete\n"), None);
        assert_eq!(euid_of("Uid:\t1000\n"), None);
    }

    /// The owners trusted are this user, root and the `ollama` account, read by its name alone.
    #[test]
    fn this_user_root_and_the_ollama_account_are_trusted() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nollamax:x:5:5::/:/bin/false\n\
                      ollama:x:999:988::/usr/share/ollama:/bin/false\n";
        assert_eq!(trusted(1000, passwd), [1000, 0, 999]);
        assert_eq!(trusted(1000, ""), [1000, 0]);
        assert_eq!(uid_of(passwd, "ollama"), Some(999));
        assert_eq!(uid_of("root:x:0:0::/root:/bin/sh\n", "ollama"), None);
        assert_eq!(
            uid_of("ollama:x:not-a-number:1::/:/bin/false\n", "ollama"),
            None
        );
    }
}
