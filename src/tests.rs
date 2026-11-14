    use super::*;

    /// Whether `version` is `base`, or a trial of it: `<base>-trial.<N>+<its commit's first 8 hex
    /// digits>`, N from 1 and written without a leading zero (docs/spec.md 7.7).
    fn base_or_its_trial(version: &str, base: &str) -> bool {
        version == base
            || version
                .strip_prefix(base)
                .and_then(|rest| rest.strip_prefix("-trial."))
                .and_then(|trial| trial.split_once('+'))
                .is_some_and(|(number, commit)| {
                    !number.starts_with('0')
                        && number.parse::<u32>().is_ok()
                        && commit.len() == 8
                        && commit
                            .bytes()
                            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                })
    }

    /// A trial build's version is Cargo.toml's with its trial and its commit after it
    /// (docs/spec.md 7.7): whatever this build was given is Cargo.toml's or a trial of it, and
    /// `--version` prints it. The workflow that builds a trial runs this with its version set, so
    /// one of another shape, or of another version than Cargo.toml's, stops there.
    #[test]
    fn a_build_says_the_crates_version_or_a_trial_of_it() {
        use clap::CommandFactory;
        assert!(
            base_or_its_trial(VERSION, env!("CARGO_PKG_VERSION")),
            "{VERSION}"
        );
        assert_eq!(
            Cli::command().render_version().trim(),
            format!("oboete {VERSION}")
        );
        for version in ["1.2.3", "1.2.3-trial.1+0123abcd", "1.2.3-trial.12+89abcdef"] {
            assert!(base_or_its_trial(version, "1.2.3"), "{version}");
        }
        for version in [
            "1.2.4",
            "1.2.4-trial.1+0123abcd",
            "1.2.3-trial.0+0123abcd",
            "1.2.3-trial.01+0123abcd",
            "1.2.3-trial.1",
            "1.2.3-trial.1+0123abc",
            "1.2.3-trial.1+0123abcde",
            "1.2.3-trial.1+0123abcg",
            "1.2.3-trial.1+0123ABCD",
            "1.2.3-trial.+0123abcd",
            "1.2.3-beta.1+0123abcd",
            "1.2.3+0123abcd",
        ] {
            assert!(!base_or_its_trial(version, "1.2.3"), "{version}");
        }
    }

    thread_local! {
        /// Run after `oboete claims` built its listing (#439).
        pub(super) static LISTED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Milestone 5 D5 (#439): a forget registered while `oboete claims` lists waits for the
    /// listing: it is the one read before the commit, and the next leaves the claim out.
    #[test]
    fn a_forget_registered_while_claims_are_listed_waits_for_the_listing() {
        let mut s = search::b::fixture::Store::new();
        let gone = s.decided("r", 5, "Ship on Fridays.", &[]);
        s.run();
        let home = s.home.path().to_owned();
        let (h, u) = (home.clone(), gone.clone());
        let (sent, started) = std::sync::mpsc::channel();
        LISTED.set(Some(Box::new(move || {
            sent.send(forget::registering(&h, &u)).unwrap();
        })));
        assert!(claims_listed(&home, "r").unwrap().contains(&gone));
        started.recv().unwrap().join().unwrap();
        assert!(!claims_listed(&home, "r").unwrap().contains(&gone));
    }

    /// Milestone 5 D5: `oboete claims` lists no forgotten claim, and an owner's mute of one is
    /// refused as of a claim that is not there.
    #[test]
    fn claims_list_no_forgotten_claim_and_refuse_its_mute() {
        let mut s = search::b::fixture::Store::new();
        let gone = s.decided("r", 5, "Ship on Fridays.", &[]);
        let kept = s.decided("r", 6, "Ship on Mondays.", &[]);
        s.run();
        let home = s.home.path();
        let target = forget::Target::parse_uid(&gone).unwrap();
        forget::start(home, &forget::preview(home, target).unwrap()).unwrap();
        let listed = claims_listed(home, "r").unwrap();
        assert!(
            listed.contains(&kept) && !listed.contains(&gone),
            "{listed}"
        );
        let refused = claims::mute(home, &gone, true).unwrap_err();
        assert!(
            refused.to_string().contains("no claim has the uid"),
            "{refused:#}"
        );
    }

    #[test]
    fn mute_and_unmute_commands_wait_for_the_claims_consumer() {
        let mut s = search::b::fixture::Store::new();
        let uid = s.decided("r", 5, "Ship on Fridays.", &[]);
        s.run();
        for (name, muted) in [("mute", true), ("unmute", false)] {
            let cli = Cli::try_parse_from(["oboete", name, &uid]).unwrap();
            run(cli.cmd, s.home.path().to_owned()).unwrap();
            let k = knowledge::open(s.home.path()).unwrap();
            assert_eq!(
                k.query_row("SELECT muted FROM active WHERE uid = ?1", [&uid], |r| r
                    .get::<_, bool>(0))
                    .unwrap(),
                muted
            );
        }
        let ops = s.raw.ops_after(s.raw.device(), 0, 10).unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[1].kind, raw::OpKind::Correction);
        assert_eq!(ops[1].body["muted"], true);
        assert_eq!(ops[2].body["muted"], false);
        let k = knowledge::open(s.home.path()).unwrap();
        assert_eq!(
            knowledge::checkpoint::get_in(&k, knowledge::checkpoint::OPS, "claims", s.raw.device())
                .unwrap(),
            3
        );
    }

    /// A restore that fails still runs the consumers, as one that succeeds does: a hook that
    /// appended while it held the worker lock started no worker (Codex on #359).
    #[test]
    fn a_restore_that_fails_runs_the_consumers() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let seq = raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let cli = Cli::try_parse_from(["oboete", "restore"]).unwrap();
        let why = run(cli.cmd, p.to_owned()).unwrap_err().to_string();
        assert!(why.contains("no usable backup segment"), "{why}");
        let k = knowledge::open(p).unwrap();
        let read: Option<i64> = k
            .query_row("SELECT MIN(seq) FROM checkpoints", [], |r| r.get(0))
            .unwrap();
        assert_eq!(read, Some(seq));
    }

    /// `setup` takes an agent or `--embeddings`, never both; `--yes` only with `--embeddings`,
    /// `--remove` only with an agent, and only the three embedders.
    #[test]
    fn setup_takes_an_agent_or_an_embedder() {
        let parses =
            |args: &[&str]| Cli::try_parse_from([&["oboete", "setup"], args].concat()).is_ok();
        assert!(parses(&["claude"]));
        assert!(parses(&["all", "--remove"]));
        assert!(parses(&["--embeddings", "local"]));
        assert!(parses(&["--embeddings", "workers-ai", "--yes"]));
        assert!(!parses(&[]));
        assert!(!parses(&["--yes"]));
        assert!(!parses(&["--remove"]));
        assert!(!parses(&["claude", "--embeddings", "none"]));
        assert!(!parses(&["--embeddings", "fastembed"]));
        assert!(!parses(&["--embeddings", "none", "--remove"]));
    }
