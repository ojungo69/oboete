    use super::*;

    /// Milestone 5 D5 (2b): a directory the sweep cannot list or search fails it, since a value in
    /// it may hold what must go (Codex on #444); a value that is not text is offered empty and
    /// replaced. Root reads every directory: the failures are checked only where they happen.
    #[cfg(unix)]
    #[test]
    fn a_sweep_fails_on_a_directory_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        update(home.path(), "claude", "s", "shown", |_| Some("{}".into())).unwrap();
        let agent = root(home.path()).join("claude");
        let shown = dir(home.path(), "claude", "s").join("shown");
        // Not listed, then listed but not searched.
        for mode in [0o000, 0o400] {
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(mode)).unwrap();
            let refused = std::fs::read_dir(&agent).is_err() || std::fs::metadata(&shown).is_err();
            let swept = update_all(home.path(), "shown", |_| None);
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
            if refused {
                assert!(swept.is_err(), "mode {mode:o}");
            }
        }
        std::fs::write(dir(home.path(), "claude", "s").join("shown"), [0xff, 0xfe]).unwrap();
        let swept = update_all(home.path(), "shown", |v| v.is_empty().then(|| "{}".into()));
        assert_eq!(swept.unwrap(), 1);
        assert_eq!(
            value(home.path(), "claude", "s", "shown").as_deref(),
            Some("{}")
        );
    }

    #[test]
    fn a_claim_is_won_once_across_threads_and_a_take_clears_it_once() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        let won: usize = (0..8)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || claim(&p, "grok", "s/1", "injected"))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap() as usize)
            .sum();
        assert_eq!(won, 1);
        assert!(!claim(&p, "grok", "s/1", "injected"));
        assert!(claim(&p, "grok", "s/2", "injected")); // another session
        set(&p, "cursor", "s/1", "compacted").unwrap();
        set(&p, "cursor", "s/1", "compacted").unwrap(); // already set
        assert!(take(&p, "cursor", "s/1", "compacted"));
        assert!(!take(&p, "cursor", "s/1", "compacted"));
    }

    #[test]
    fn a_flag_set_once_is_taken_by_one_of_concurrent_takes() {
        let home = tempfile::tempdir().unwrap();
        for _ in 0..200 {
            set(home.path(), "cursor", "s", "compacted").unwrap();
            let start = std::sync::Arc::new(std::sync::Barrier::new(4));
            let won: usize = (0..4)
                .map(|_| {
                    let (p, start) = (home.path().to_path_buf(), start.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        take(&p, "cursor", "s", "compacted")
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|t| t.join().unwrap() as usize)
                .sum();
            assert_eq!(won, 1);
        }
    }

    /// Task 8 Step 5: concurrent updates of one session's value each see the others' changes.
    #[test]
    fn concurrent_updates_keep_every_change() {
        let home = tempfile::tempdir().unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let (p, start) = (home.path().to_path_buf(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    update(&p, "claude", "s", "shown", |v| {
                        Some(v.unwrap_or_default() + &format!("{i}\n"))
                    })
                    .unwrap();
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        let mut seen: Vec<String> = value(home.path(), "claude", "s", "shown")
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        seen.sort();
        assert_eq!(seen, (0..8).map(|i| i.to_string()).collect::<Vec<_>>());
        update(home.path(), "claude", "s", "shown", |_| None).unwrap();
        assert_eq!(value(home.path(), "claude", "s", "shown"), None);
    }

    #[cfg(unix)] // setting a directory's time needs another open on Windows
    #[test]
    fn prune_removes_only_sessions_older_than_the_limit() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        set(p, "agy", "old", "injected").unwrap();
        set(p, "agy", "new", "injected").unwrap();
        let old = dir(p, "agy", "old");
        let past = SystemTime::now() - KEEP - Duration::from_secs(60);
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        prune(p, KEEP);
        assert!(!old.exists());
        assert!(dir(p, "agy", "new").exists());
    }
