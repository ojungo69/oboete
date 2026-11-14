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
