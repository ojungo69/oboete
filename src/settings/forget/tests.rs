    use super::*;
    use crate::search::b::fixture::Store;

    fn ask(home: &Path, kind: &str, id: &str) -> Result<Value, Refusal> {
        preview(
            home,
            &serde_json::to_vec(&json!({"item": {"kind": kind, "id": id}})).unwrap(),
        )
    }

    fn confirm(
        home: &Path,
        kind: &str,
        id: &str,
        key: &str,
        confirmed: bool,
    ) -> Result<Value, Refusal> {
        let body = json!({"item": {"kind": kind, "id": id}, "preview_key": key,
            "confirmed": confirmed});
        start(home, &Mutex::new(()), &serde_json::to_vec(&body).unwrap())
    }

    fn code(r: Result<Value, Refusal>) -> (u16, &'static str) {
        let r = r.unwrap_err();
        (r.status, r.code)
    }

    /// A prompt is forgotten from the page as the CLI forgets it: the preview counts it and the
    /// claim quoting it, a wrong key or no confirmation registers nothing, and the right key hides
    /// the record and forgets the claim; the same start again is stale.
    #[test]
    fn a_prompt_is_forgotten_with_its_previews_key_and_once() {
        let mut s = Store::new();
        let seq = s.said("s1", "r", 1_000, "Ship on Fridays.");
        let kept = s.said("s1", "r", 2_000, "Keep the tests green.");
        let uid = s.claim(
            seq,
            "Ship on Fridays.",
            ("decision", "decided", "user"),
            &[],
        );
        s.run();
        let id = s.key(seq);
        // The purge's rebuild needs raw.db open nowhere else.
        let Store {
            home: dir,
            raw: held,
        } = s;
        drop(held);
        let home = dir.path();
        let shown = ask(home, "prompt", &id).unwrap();
        assert_eq!(
            (shown["records"].as_u64(), shown["claims"].as_u64()),
            (Some(1), Some(1))
        );
        let key = shown["preview_key"].as_str().unwrap().to_owned();
        assert_eq!(
            code(confirm(home, "prompt", &id, &"0".repeat(64), true)),
            (409, "forget_stale")
        );
        assert_eq!(
            code(confirm(home, "prompt", &id, &key, false)),
            (422, "forget_confirmation")
        );
        assert_eq!(
            code(confirm(home, "prompt", &id, "not hex", true)),
            (422, "forget_key")
        );
        let forgotten = || crate::raw::open(home).unwrap().forgotten(&uid).unwrap();
        assert!(!forgotten());
        std::fs::create_dir_all(home.join("state")).unwrap();
        let continued = forget::CONTINUED.get();
        let started = confirm(home, "prompt", &id, &key, true).unwrap();
        assert_eq!(
            started,
            json!({"started": true, "records": 1, "logged": true, "cleaning": true,
                "after": false})
        );
        let at = |seq: i64| {
            let raw = crate::raw::open(home).unwrap();
            (raw.after(raw.device(), seq - 1, 1).unwrap().first())
                .is_some_and(|r| r.seq == seq && matches!(r.item, crate::raw::Item::Event(_)))
        };
        assert!(
            !at(seq) && at(kept),
            "the forgotten record is hidden at once, the other kept"
        );
        // The purge that `forget --continue` runs takes the claim quoting it, as the CLI's does.
        assert_eq!(forget::CONTINUED.get(), continued + 1);
        forget::purge(home).unwrap();
        assert!(forgotten());
        assert_eq!(
            code(confirm(home, "prompt", &id, &key, true)),
            (409, "forget_stale")
        );
    }

    /// A claim and a note go by their uid; a session by its page key, every record of it and none
    /// of another session's.
    #[test]
    fn claims_notes_and_sessions_are_forgotten_by_their_page_ids() {
        let mut s = Store::new();
        let gone = s.said("s1", "r", 1_000, "Use tabs.");
        let other = s.said("s2", "r", 1_500, "Use spaces.");
        let also = s.said("s1", "r", 2_000, "Wrap at 100.");
        let claim = s.claim(gone, "Use tabs.", ("preference", "decided", "user"), &[]);
        let note = s.note("n1", "r", 3_000, "Deploys", "Deploys wait for the canary.");
        s.run();
        let home = s.home.path();
        let forget = |kind: &str, id: &str| {
            let key = ask(home, kind, id).unwrap()["preview_key"]
                .as_str()
                .unwrap()
                .to_owned();
            confirm(home, kind, id, &key, true).unwrap()
        };
        forget("claim", &claim);
        forget("note", &note);
        let raw = crate::raw::open(home).unwrap();
        assert!(raw.forgotten(&claim).unwrap() && raw.forgotten(&note).unwrap());
        let sessions = raw.live_sessions().unwrap();
        let key = |label: &str| {
            let s = sessions.iter().find(|s| s.session == label).unwrap();
            crate::view::session_key(&s.agent, &s.session)
        };
        let s1 = key("s1");
        let shown = ask(home, "session", &s1).unwrap();
        assert_eq!(shown["records"].as_u64(), Some(2));
        forget("session", &s1);
        let left: Vec<String> = (raw.live_sessions().unwrap().into_iter())
            .map(|s| s.session)
            .collect();
        assert_eq!(left, ["s2"]);
        let at = |seq: i64| {
            (raw.after(raw.device(), seq - 1, 1).unwrap().first())
                .is_some_and(|r| r.seq == seq && matches!(r.item, crate::raw::Item::Event(_)))
        };
        assert_eq!((at(gone), at(also), at(other)), (false, false, true));
        assert_eq!(code(ask(home, "session", &s1)), (422, "forget_item"));
    }

    /// A claim's preview counts the claims it superseded, current again once it is gone, as the
    /// CLI lists them; one that superseded none counts none.
    #[test]
    fn a_claims_preview_counts_the_claims_that_come_back() {
        let mut s = Store::new();
        let old = s.decided("r", 1_000, "Ship on Fridays.", &[]);
        let new = s.decided("r", 2_000, "Ship on Mondays.", &[&old]);
        s.run();
        let home = s.home.path();
        assert_eq!(ask(home, "claim", &new).unwrap()["again"], json!(1));
        assert_eq!(ask(home, "claim", &old).unwrap()["again"], json!(0));
    }

    /// A session asked of a home with no store makes none: the store is not opened to look.
    #[test]
    fn a_session_asked_of_a_home_with_no_store_makes_none() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            code(ask(home.path(), "session", "0123456789abcdef")),
            (422, "forget_item")
        );
        assert!(!crate::raw::exists(home.path()));
    }

    /// A start while a purge runs starts its purge after that one and says so, and a start whose
    /// request log could not be written says so, as the CLI does; both are registered.
    #[test]
    fn a_start_says_when_its_purge_waits_and_when_its_log_was_not_written() {
        let mut s = Store::new();
        let one = s.said("s1", "r", 1_000, "Use tabs.");
        let two = s.said("s1", "r", 2_000, "Wrap at 100.");
        s.run();
        let (first, second) = (s.key(one), s.key(two));
        let Store {
            home: dir,
            raw: held,
        } = s;
        drop(held);
        let home = dir.path();
        std::fs::create_dir_all(home.join("state")).unwrap();
        let go = |id: &str| {
            let key = ask(home, "prompt", id).unwrap()["preview_key"]
                .as_str()
                .unwrap()
                .to_owned();
            confirm(home, "prompt", id, &key, true).unwrap()
        };
        let running = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.join("state").join("purge.lock"))
            .unwrap();
        running.lock().unwrap();
        let continued = forget::CONTINUED.get();
        let started = go(&first);
        assert_eq!(
            (&started["cleaning"], &started["after"], &started["logged"]),
            (&json!(true), &json!(true), &json!(true))
        );
        assert_eq!(forget::CONTINUED.get(), continued + 1);
        drop(running);
        std::fs::write(home.join("config.toml"), "[backup]\ndir = 42\n").unwrap();
        let started = go(&second);
        assert_eq!(
            (&started["cleaning"], &started["after"], &started["logged"]),
            (&json!(true), &json!(false), &json!(false))
        );
        let raw = crate::raw::open(home).unwrap();
        let shown = |seq: i64| {
            (raw.after(raw.device(), seq - 1, 1).unwrap().first())
                .is_some_and(|r| r.seq == seq && matches!(r.item, crate::raw::Item::Event(_)))
        };
        assert_eq!((shown(one), shown(two)), (false, false));
    }

    /// A preview that only the item's session can take is refused by name, so the page can say
    /// so: past the cap with its copies, or a record of another device's.
    #[test]
    fn what_only_its_session_can_take_is_refused_by_name() {
        let named = |e: anyhow::Error| {
            let r = preview_failed(&e);
            (r.status, r.code)
        };
        assert_eq!(
            named(anyhow::Error::new(forget::TooLarge).context("previewing")),
            (422, "forget_too_large")
        );
        assert_eq!(
            named(anyhow::Error::new(forget::OtherDevice)),
            (422, "forget_other_device")
        );
        assert_eq!(
            named(anyhow::anyhow!("no raw records matched")),
            (422, "forget_preview_failed")
        );
        let mut s = Store::new();
        s.said("s1", "r", 1_000, "Use tabs.");
        s.run();
        let other = format!("{}0:1", s.raw.device());
        assert_eq!(
            code(ask(s.home.path(), "prompt", &other)),
            (422, "forget_other_device")
        );
        assert_eq!(
            code(confirm(
                s.home.path(),
                "prompt",
                &other,
                &"0".repeat(64),
                true
            )),
            (422, "forget_other_device")
        );
    }

    /// What the page cannot forget is refused before anything is read or written: an unknown
    /// kind, a card or a summary, an id of another kind than the one named, a session key of no
    /// session, a record handle that is not one, a body with an unknown field.
    #[test]
    fn what_is_no_item_is_refused() {
        let mut s = Store::new();
        let seq = s.said("s1", "r", 1_000, "Use tabs.");
        let claim = s.claim(seq, "Use tabs.", ("preference", "decided", "user"), &[]);
        let note = s.note("n1", "r", 3_000, "Deploys", "Deploys wait for the canary.");
        s.run();
        let home = s.home.path();
        for (kind, id) in [
            ("card", "12.0"),
            ("summary", "S12"),
            ("claim", "12.0"),
            ("claim", note.as_str()),
            ("note", claim.as_str()),
            ("prompt", "not a handle"),
            ("prompt", &format!("{}:0", s.raw.device())),
            ("session", "0123456789abcdef"),
            ("session", ""),
        ] {
            assert_eq!(
                code(ask(home, kind, id)),
                (422, "forget_item"),
                "{kind} {id}"
            );
        }
        let long = "a".repeat(MAX_ID + 1);
        assert_eq!(code(ask(home, "claim", &long)), (422, "forget_item"));
        let extra = json!({"item": {"kind": "claim", "id": claim, "more": 1}});
        assert_eq!(
            code(preview(home, &serde_json::to_vec(&extra).unwrap())),
            (400, "bad_request")
        );
        assert_eq!(code(preview(home, b"not json")), (400, "bad_request"));
        let raw = crate::raw::open(home).unwrap();
        assert!(!raw.forgotten(&claim).unwrap() && !raw.forgotten(&note).unwrap());
    }
