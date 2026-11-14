use super::*;
use std::time::{Duration, Instant};

#[test]
fn startup_config_uses_defaults_or_checks_capture_redaction_and_resident_settings() {
    assert!(startup_text_ready(None));
    assert!(startup_text_ready(Some("")));
    assert!(startup_text_ready(Some(
        "[worker]\nresident = true\n[view]\nport = 17374\n[capture]\nstore_prompts = false\ntool_output = 'head-tail'\n"
    )));
    assert!(startup_text_ready(Some(
        "[[redaction.extra_rules]]\nid = 'synthetic'\nregex = 'canary-[a-z]+'\n"
    )));
    for invalid in [
        "[worker\nresident = true",
        "[worker]\nresident = 'editing'",
        "[worker]\nresident = true\nresdient = true",
        "[view]\nport = 0",
        "[view]\nport = 65536",
        "[view]\nport = 17374\nextra = true",
        "[capture]\ntool_output = 'unknown'",
        "[redactions]\nallowlist = []",
        "[redaction]\nallowlist = ['synthetic-not-a-hash']",
        "[[redaction.extra_rules]]\nid = 'synthetic'\nregex = '['",
        "[[redaction.extra_rules]]\nid = 'synthetic'\nregex = '(canary)'\nsecret_group = 2",
    ] {
        assert!(!startup_text_ready(Some(invalid)));
    }
}

#[test]
fn unreadable_config_defers_replacement_and_a_repaired_file_is_rechecked() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    assert!(startup_ready(home.path()));
    std::fs::create_dir(&path).unwrap();
    assert!(!startup_ready(home.path()));
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, "[worker\n").unwrap();
    assert!(!startup_ready(home.path()));
    std::fs::write(&path, "[worker]\nresident = true\n").unwrap();
    assert!(startup_ready(home.path()));
}

#[test]
fn only_the_same_process_and_known_lock_roles_can_restore_a_home_handoff() {
    use std::os::unix::ffi::OsStringExt;
    for (value, role) in [
        ("11:worker.lock:2:3", Role::Worker),
        ("11:view.lock:2:3", Role::Viewer),
    ] {
        let home = parse_home(std::ffi::OsStr::new(value), 11)
            .unwrap()
            .unwrap();
        assert!(home.role == role);
        assert_eq!(home.identity, (2, 3));
    }
    assert!(
        parse_home(std::ffi::OsStr::new("12:unknown"), 11)
            .unwrap()
            .is_none()
    );
    for value in [
        "",
        "not-a-pid:worker.lock:2:3",
        "11:unknown:2:3",
        "11:worker.lock:2",
        "11:worker.lock:2:3:extra",
        "11:worker.lock:-2:3",
        "11:worker.lock:2:18446744073709551616",
    ] {
        assert!(parse_home(std::ffi::OsStr::new(value), 11).is_err());
    }
    let invalid = std::ffi::OsString::from_vec(b"11:worker.lock:\xff".to_vec());
    assert!(parse_home(&invalid, 11).is_err());
}

#[test]
fn a_viewer_handoff_accepts_only_a_positive_specific_child_of_its_process() {
    assert_eq!(parse_viewer("11:12", 11), Some(12));
    for value in [
        "12:13",
        "11:11",
        "11:0",
        "11:2147483648",
        "11:4294967296",
        "11",
        "x:12",
        "11:x",
    ] {
        assert_eq!(parse_viewer(value, 11), None);
    }
}

#[test]
fn rejected_exec_inputs_and_native_failure_keep_the_old_process_resources() {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let lock_path = scratch.path().join("worker.lock");
    std::fs::write(&lock_path, "synthetic held lock").unwrap();
    let lock = std::fs::File::open(&lock_path).unwrap();
    lock.lock().unwrap();
    let identity = crate::worker::file_id(lock.metadata());
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let image = scratch.path().join("invalid-image");
    std::fs::write(&image, "synthetic invalid ELF image").unwrap();
    std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut binary = Binary {
        path: image,
        identity: None,
        arguments: vec![
            "original argv[0]".into(),
            "--home".into(),
            scratch.path().into(),
        ],
        viewer: std::sync::Mutex::new(None),
    };
    let absent = binary.exec(Role::Worker, None, None).unwrap_err();
    assert_eq!(absent.kind(), std::io::ErrorKind::Other);
    binary
        .arguments
        .push(std::ffi::OsString::from_vec(b"bad\0argument".to_vec()));
    assert_eq!(
        binary
            .exec(Role::Worker, identity, None)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    binary.arguments.pop();
    let path = binary.path.clone();
    binary.path = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"bad\0path".to_vec()));
    assert_eq!(
        binary
            .exec(Role::Worker, identity, None)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    binary.path = path;
    fn sigpipe() -> libc::sighandler_t {
        let mut state = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        // SAFETY: null new action only reads the disposition; initialized on success.
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGPIPE, std::ptr::null(), state.as_mut_ptr()) },
            0
        );
        unsafe { state.assume_init().sa_sigaction }
    }
    let signal = sigpipe();
    // The optional child metadata stays private; this failed image never receives it.
    assert_eq!(
        binary
            .exec(Role::Worker, identity, Some(12))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOEXEC)
    );
    assert_eq!(sigpipe(), signal);
    assert_eq!(std::env::current_dir().unwrap(), cwd);
    assert_eq!(crate::worker::file_id(lock.metadata()), identity);
    assert!(matches!(
        std::fs::File::open(lock_path).unwrap().try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    assert_eq!(
        std::net::TcpListener::bind(address).unwrap_err().kind(),
        std::io::ErrorKind::AddrInUse
    );
    let connection = std::net::TcpStream::connect(address).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    drop((connection, accepted));
}

#[test]
fn missing_requires_the_next_minute_and_recovery_or_metadata_errors_keep_serving() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("bin");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("oboete");
    std::fs::write(&path, "synthetic executable").unwrap();
    let _loaded = std::fs::File::open(&path).unwrap();
    let identity = crate::worker::file_id(std::fs::metadata(&path));
    let start = Instant::now();
    let mut watch = Watch::default();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(watch.at(&path, identity, start), Change::Stay);
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(59)),
        Change::Stay
    );
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(60)),
        Change::Missing,
        "a continuously absent executable did not end at its next minute check"
    );
    let mut watch = Watch::default();
    assert_eq!(watch.at(&path, identity, start), Change::Stay);
    std::fs::write(&path, "replacement").unwrap();
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(10)),
        Change::Replaced
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(20)),
        Change::Stay
    );
    std::fs::remove_dir(&directory).unwrap();
    std::fs::write(&directory, "not a directory").unwrap();
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(90)),
        Change::Stay,
        "ENOTDIR is not proof of a continuously missing path"
    );
    std::fs::remove_file(&directory).unwrap();
    std::fs::create_dir(&directory).unwrap();
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(100)),
        Change::Stay
    );
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(159)),
        Change::Stay
    );
    assert_eq!(
        watch.at(&path, identity, start + Duration::from_secs(160)),
        Change::Missing
    );
}
