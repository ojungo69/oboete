    use super::*;
    use crate::provider::tests::{request_body, serve_each};
    use std::sync::mpsc::Receiver;

    const KEY: &str = "TESTKEY_for_the_stub";
    const CF_KEY: &str = "CFTOKEN_for_the_stub";
    const LIQUID_KEY: &str = "liquid_KEY_for_the_stub";
    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

    struct Home {
        dir: tempfile::TempDir,
        raw: Raw,
        db: Connection,
        judge: config::Judge,
    }

    fn home() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("TYPESAFE_KEY.md");
        std::fs::write(&key_file, format!("# a test key\n{KEY}\n")).unwrap();
        Home {
            raw: crate::raw::open(dir.path()).unwrap(),
            db: providers_db::open(dir.path()).unwrap(),
            judge: config::Judge {
                enabled: true,
                destination: config::Destination::Jev,
                key_file: Some(key_file),
                cloudflare_key_file: None,
                cloudflare_account_id: None,
                liquid_key_file: None,
                model: "jev-latest".into(),
                timeout_s: 5,
                daily_budget: 300,
            },
            dir,
        }
    }

    /// `h`'s judge sending to `d`, each service's own key in a file of its own.
    fn sending_to(h: &mut Home, d: config::Destination) {
        let file = |name: &str, key: &str| {
            let file = h.dir.path().join(name);
            std::fs::write(&file, format!("# a test key\n{key}\n")).unwrap();
            file
        };
        let (cloudflare, liquid) = (file("CF_KEY.md", CF_KEY), file("LIQUID_KEY.md", LIQUID_KEY));
        h.judge.cloudflare_key_file = Some(cloudflare);
        h.judge.cloudflare_account_id = Some(ACCOUNT.into());
        h.judge.liquid_key_file = Some(liquid);
        h.judge.destination = d;
    }

    /// `answer` in Cloudflare's envelope, as its REST API answered on 2026-10-31.
    fn enveloped(answer: &[u8]) -> Vec<u8> {
        let answer: Value = serde_json::from_slice(answer).unwrap();
        json!({"result": answer, "success": true, "errors": [], "messages": []})
            .to_string()
            .into_bytes()
    }

    fn rules(toml: &str) -> Rules {
        Rules::new(&config::parse_capture(Some(toml)).unwrap().redaction).unwrap()
    }

    /// A conversation and a choice among two requests, `g7` and `g8`, or none.
    fn asked(repos: &[String]) -> Ask<'_> {
        Ask {
            span: "c1",
            state: json!({
                "conversation": {"worktree": "github.com/o/r", "asks": ["fix codename-blue's save"]},
                "requests": {"g7": {"title": "Fix the save"}, "g8": {"title": "Tidy the logs"}},
            }),
            questions: json!({"pick": {"type": "choice",
                "instructions": "Which request does `conversation` continue?",
                "criteria": {"g7": "`requests.g7`: Fix the save", "g8": "`requests.g8`: Tidy the logs",
                    "none": "None of these"}}}),
            repos,
        }
    }

    fn picked(choice: &str) -> Vec<u8> {
        json!({"model": "jev-1.14.0",
            "answers": {"pick": {"type": "choice", "choice": choice, "confidence": 0.7,
                "probabilities": {"g7": 0.8, "g8": 0.15, "none": 0.05}}},
            "usage": {"input_tokens": 1_000_000, "output_tokens": 12}})
        .to_string()
        .into_bytes()
    }

    fn run(h: &Home, rules: &Rules, cap: f64, ask_: Ask) -> Result<Asked> {
        ask(&h.db, &h.raw, rules, (&h.judge, cap), ask_, &|| Ok(None))
    }

    /// The stub at `answers`, called in the endpoint's place.
    fn stub(answers: Vec<(&'static str, Vec<u8>)>, headers: &'static str) -> Receiver<String> {
        let (url, rx) = serve_each(answers, headers);
        AT.set(Some(url));
        rx
    }

    fn requests(rx: &Receiver<String>) -> usize {
        std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(300)).ok()).count()
    }

    /// Every row of the ledger, each column as text.
    fn ledger(db: &Connection) -> Vec<String> {
        let mut st = db
            .prepare(
                "SELECT provider, role, span, outcome, COALESCE(detail, ''), prompt_tokens,
                        completion_tokens, usd FROM provider_calls ORDER BY id",
            )
            .unwrap();
        st.query_map([], |r| {
            Ok(format!(
                "{} {} {} {} {} {:?} {:?} {:?}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<f64>>(7)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    /// J1: one request to the endpoint with the key, every recorded string gated (the ids are keys,
    /// kept as they are), the answer checked, its input priced at USD 0.042 a million, and one
    /// settled row in the ledger.
    #[test]
    fn an_answer_is_gated_sent_once_priced_and_recorded() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let rules = rules(
            "[redaction]\nextra_rules = [{ id = \"codename\", regex = \"codename-[a-z]+\" }]\n",
        );
        let answered = run(&h, &rules, 5.0, asked(&[])).unwrap();
        let Asked::Answered(answers) = answered else {
            panic!("{answered:?}")
        };
        assert_eq!(answers["pick"]["choice"], "g7");
        let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(request.starts_with("POST "), "{request}");
        assert!(
            request.contains(&format!("authorization: Bearer {KEY}"))
                || request.contains(&format!("Authorization: Bearer {KEY}"))
        );
        let sent = request_body(&request);
        assert_eq!(sent["model"], "jev-latest");
        let text = sent.to_string();
        assert!(!text.contains("codename-blue"), "{text}");
        assert!(sent["state"]["requests"].get("g7").is_some(), "{text}");
        assert!(sent["questions"]["pick"]["criteria"].get("g7").is_some());
        assert_eq!(requests(&rx), 0);
        assert_eq!(
            ledger(&h.db),
            ["judge:jev judge c1 ok  Some(1000000) Some(12) Some(0.042)"]
        );
    }

    /// Spec 5.5: a repository kept from being sent, or never recorded, is not sent: no request,
    /// no row.
    #[test]
    fn an_excluded_repository_is_never_sent() {
        for capture in [false, true] {
            let mut h = home();
            let rx = stub(vec![("200 OK", picked("g7"))], "");
            if capture {
                h.raw
                    .exclude_capture(("repo", "github.com/o/r"), false)
                    .unwrap();
            } else {
                h.raw.exclude("github.com/o/r", false).unwrap();
            }
            let repos = ["github.com/o/r".to_owned()];
            let answered = run(&h, &rules(""), 5.0, asked(&repos)).unwrap();
            assert_eq!(answered, Asked::Skipped("excluded"), "capture {capture}");
            assert_eq!(requests(&rx), 0, "capture {capture}");
            assert!(ledger(&h.db).is_empty(), "capture {capture}");
        }
    }

    /// A key the gate would change (a rule that matches an id, a question's name or a criterion)
    /// is never sent: no request, no row.
    #[test]
    fn a_key_the_gate_would_mask_is_never_sent() {
        for rule in ["g7", "pick", "none"] {
            let h = home();
            let rx = stub(vec![("200 OK", picked("g7"))], "");
            let rules = rules(&format!(
                "[redaction]\nextra_rules = [{{ id = \"ids\", regex = \"^{rule}$\" }}]\n"
            ));
            assert_eq!(
                run(&h, &rules, 5.0, asked(&[])).unwrap(),
                Asked::Skipped("masked"),
                "{rule}"
            );
            assert_eq!(requests(&rx), 0, "{rule}");
            assert!(ledger(&h.db).is_empty(), "{rule}");
        }
    }

    /// An exclusion written after the first look, before the request, still holds: the second look
    /// is under the gate's admission. Nothing is sent and the reservation is gone.
    #[test]
    fn an_exclusion_written_while_the_call_waits_holds() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let gate = || {
            let mut raw = crate::raw::open(h.dir.path())?;
            raw.exclude("github.com/o/r", false)?;
            Ok(None)
        };
        let repos = ["github.com/o/r".to_owned()];
        let answered = ask(
            &h.db,
            &h.raw,
            &rules(""),
            (&h.judge, 5.0),
            asked(&repos),
            &gate,
        )
        .unwrap();
        assert_eq!(answered, Asked::Skipped("excluded"));
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
    }

    /// #763: a second look at the exclusions that fails returns its error, sends nothing and
    /// leaves no reservation.
    #[test]
    fn a_second_exclusion_read_that_fails_leaves_no_reservation() {
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        // A repository never recorded makes the read load the capture settings from config.toml.
        h.raw
            .exclude_capture(("repo", "github.com/o/other"), false)
            .unwrap();
        let config = h.dir.path().join("config.toml");
        let gate = || {
            std::fs::write(&config, "[capture\n")?;
            Ok(None)
        };
        let repos = ["github.com/o/r".to_owned()];
        let answered = ask(
            &h.db,
            &h.raw,
            &rules(""),
            (&h.judge, 5.0),
            asked(&repos),
            &gate,
        );
        assert!(answered.is_err(), "{answered:?}");
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
    }

    /// The daily calls and the month's paid cap refuse before anything is sent, each with a row.
    #[test]
    fn the_daily_calls_and_the_paid_cap_refuse_with_a_row_and_no_request() {
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        assert_eq!(
            run(&h, &rules(""), 0.0, asked(&[])).unwrap(),
            Asked::Skipped("budget")
        );
        h.judge.daily_budget = 0;
        assert_eq!(
            run(&h, &rules(""), 5.0, asked(&[])).unwrap(),
            Asked::Skipped("budget")
        );
        assert_eq!(requests(&rx), 0);
        let rows = ledger(&h.db);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows.iter()
                .all(|r| r.starts_with("judge:jev judge c1 budget ")),
            "{rows:?}"
        );
    }

    /// Decision 64: with no cap of calls a day and no monthly cap, the judge asks.
    #[test]
    fn with_its_caps_off_the_judge_asks() {
        let mut h = home();
        h.judge.daily_budget = config::no_daily_cap();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let answered = run(&h, &rules(""), config::no_monthly_cap(), asked(&[])).unwrap();
        assert!(matches!(answered, Asked::Answered(_)), "{answered:?}");
        assert_eq!(requests(&rx), 1);
    }

    /// No retry and no redirect: an error status, a 429 and a redirect are one request each, its
    /// body never kept; a redirect's target receives nothing.
    #[test]
    fn a_failed_call_is_one_request_with_no_body_kept() {
        let (target, seen) = serve_each(vec![("200 OK", picked("g7"))], "");
        let location: &'static str = Box::leak(format!("Location: {target}/\r\n").into_boxed_str());
        for (status, headers) in [
            ("500 Internal Server Error", ""),
            ("429 Too Many Requests", "Retry-After: 0\r\n"),
            ("307 Temporary Redirect", location),
        ] {
            let h = home();
            let body = br#"{"error": "echo: fix codename-blue's save"}"#.to_vec();
            let rx = stub(vec![(status, body), ("200 OK", picked("g7"))], headers);
            let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(answered, Asked::Skipped("error"), "{status}");
            assert_eq!(requests(&rx), 1, "{status}");
            let code = &status[..3];
            assert_eq!(
                ledger(&h.db),
                [format!(
                    "judge:jev judge c1 error http {code} None None None"
                )],
                "{status}"
            );
        }
        assert_eq!(requests(&seen), 0);
    }

    /// An answer that does not fit its question is `shape`, still billed at the reported input.
    #[test]
    fn an_answer_that_does_not_fit_is_shape() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g9"))], "");
        let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
        assert_eq!(answered, Asked::Skipped("shape"));
        assert_eq!(requests(&rx), 1);
        assert_eq!(
            ledger(&h.db),
            ["judge:jev judge c1 shape answers Some(1000000) Some(12) Some(0.042)"]
        );
    }

    /// Typesafe's schema requires both token counts: without either, or with one negative or not
    /// a whole number, the answer is `shape`, its input priced as reported, else at the
    /// reservation's estimate; and what it cost counts in the month's cap.
    #[test]
    fn an_answer_without_its_token_counts_is_shape() {
        let estimated = "None None Some(4.662e-6)";
        for (usage, kept) in [
            (json!(null), estimated),
            (
                json!({"input_tokens": 1_000_000}),
                "Some(1000000) None Some(0.042)",
            ),
            (json!({"output_tokens": 12}), "None Some(12) Some(4.662e-6)"),
            (
                json!({"input_tokens": -1, "output_tokens": 12}),
                "None Some(12) Some(4.662e-6)",
            ),
            (
                json!({"input_tokens": 10.5, "output_tokens": 12}),
                "None Some(12) Some(4.662e-6)",
            ),
        ] {
            let h = home();
            let mut answer: Value = serde_json::from_slice(&picked("g7")).unwrap();
            answer["usage"] = usage.clone();
            let rx = stub(vec![("200 OK", answer.to_string().into_bytes())], "");
            let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(answered, Asked::Skipped("shape"), "{usage}");
            assert_eq!(requests(&rx), 1);
            let rows = ledger(&h.db);
            assert_eq!(rows.len(), 1, "{rows:?}");
            // The reservation's bounds stay with a call of unknown usage (`freeze_unmetered`).
            let row = &rows[0];
            assert!(row.starts_with("judge:jev judge c1 shape "), "{row}");
            assert!(row.contains("usage") && row.ends_with(kept), "{row}");
            if kept.ends_with("Some(0.042)") {
                // USD 0.042 spent: a cap of 0.04 takes no further call.
                assert_eq!(
                    run(&h, &rules(""), 0.04, asked(&[])).unwrap(),
                    Asked::Skipped("budget")
                );
            }
        }
    }

    /// The egress gate is asked right before the request: its error is returned, nothing is sent
    /// and the reservation is gone.
    #[test]
    fn a_gate_error_sends_nothing_and_leaves_no_row() {
        let h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        let gate = || Err(anyhow::anyhow!("the list changed"));
        let e = ask(
            &h.db,
            &h.raw,
            &rules(""),
            (&h.judge, 5.0),
            asked(&[]),
            &gate,
        )
        .unwrap_err();
        assert_eq!(e.to_string(), "the list changed");
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
    }

    /// Off, or no key in its file: nothing is reserved or sent.
    #[test]
    fn off_or_without_a_key_nothing_goes() {
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        h.judge.enabled = false;
        assert_eq!(
            run(&h, &rules(""), 5.0, asked(&[])).unwrap(),
            Asked::Skipped("off")
        );
        h.judge.enabled = true;
        std::fs::write(h.judge.key_file.as_ref().unwrap(), "only a note\n").unwrap();
        assert_eq!(
            run(&h, &rules(""), 5.0, asked(&[])).unwrap(),
            Asked::Skipped("key")
        );
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
        drop(h.dir);
    }

    /// J7 (decision 63): each destination's address is fixed here. Cloudflare's names the account
    /// id in its path only when it is 32 lowercase hex digits, so no value of `[judge]` reaches
    /// another host or path, and without one the judge is not ready.
    #[test]
    fn each_destination_is_a_fixed_address() {
        use config::Destination::*;
        let mut h = home();
        let cloudflare = |model: &str| {
            format!(
                "https://api.cloudflare.com/client/v4/accounts/{ACCOUNT}/ai/run/@cf/cloudflare/{model}"
            )
        };
        for (d, at) in [
            (Jev, "https://api.typesafe.ai/v1/systemone".to_owned()),
            (Clef, cloudflare("clef")),
            (ClefFlash, cloudflare("clef-flash")),
            (
                D1,
                "https://api.liquid.ai/decisions/v1/systemone".to_owned(),
            ),
            (D1Local, "http://127.0.0.1:11434/v1/systemone".to_owned()),
        ] {
            sending_to(&mut h, d);
            assert_eq!(url(&h.judge).as_deref(), Some(at.as_str()), "{d:?}");
            assert!(ready(&h.judge), "{d:?}");
        }
        sending_to(&mut h, ClefFlash);
        for bad in [
            None,
            Some("0123456789ABCDEF0123456789abcdef"),
            Some("0123456789abcdef0123456789abcde"),
            Some("0123456789abcdef0123456789abcdef/../../zones"),
        ] {
            h.judge.cloudflare_account_id = bad.map(Into::into);
            assert_eq!(url(&h.judge), None, "{bad:?}");
            assert!(!ready(&h.judge), "{bad:?}");
        }
    }

    /// J7: each destination is asked with its own model selector and its own service's key (none
    /// for d1 on this PC), its answer read where its API puts it (Cloudflare's inside its
    /// envelope), and its call kept in the ledger under its own name at its own price.
    #[test]
    fn each_destination_sends_its_model_and_key_and_keeps_its_own_row() {
        use config::Destination::*;
        let row =
            |name: &str, usd: &str| format!("{name} judge c1 ok  Some(1000000) Some(12) {usd}");
        for (d, model, key, kept) in [
            (
                Jev,
                "jev-latest",
                Some(KEY),
                row("judge:jev", "Some(0.042)"),
            ),
            (Clef, "clef", Some(CF_KEY), row("judge:clef", "Some(0.24)")),
            (
                ClefFlash,
                "clef-flash",
                Some(CF_KEY),
                row("judge:clef-flash", "Some(0.038)"),
            ),
            (D1, "d1.1", Some(LIQUID_KEY), row("judge:d1", "Some(0.02)")),
            (D1Local, "d1", None, row("judge:d1-local", "None")),
        ] {
            let mut h = home();
            sending_to(&mut h, d);
            let answer = match d {
                Clef | ClefFlash => enveloped(&picked("g7")),
                _ => picked("g7"),
            };
            let rx = stub(vec![("200 OK", answer)], "");
            let answered = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert!(
                matches!(answered, Asked::Answered(_)),
                "{d:?}: {answered:?}"
            );
            let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(request_body(&request)["model"], model, "{d:?}");
            let lower = request.to_lowercase();
            match key {
                Some(key) => {
                    let header = format!("authorization: bearer {}", key.to_lowercase());
                    assert!(lower.contains(&header), "{d:?}");
                }
                None => assert!(!lower.contains("authorization"), "{d:?}"),
            }
            assert_eq!(ledger(&h.db), [kept], "{d:?}");
        }
    }

    /// J7: Cloudflare's answer is the `result` of its envelope when `success` is true; another
    /// envelope, or an answer outside one, is not its own (`shape`), and is paid for as sent.
    #[test]
    fn cloudflare_s_envelope_is_read_or_refused() {
        let answer: Value = serde_json::from_slice(&picked("g7")).unwrap();
        for body in [
            json!({"result": answer, "success": false, "errors": [{"code": 3040}], "messages": []}),
            json!({"success": true, "errors": [], "messages": []}),
            answer.clone(),
        ] {
            let mut h = home();
            sending_to(&mut h, config::Destination::ClefFlash);
            let _rx = stub(vec![("200 OK", body.to_string().into_bytes())], "");
            let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(asked_, Asked::Skipped("shape"), "{body}");
            let kept = ledger(&h.db);
            assert_eq!(kept.len(), 1, "{kept:?}");
            let row = &kept[0];
            assert!(row.starts_with("judge:clef-flash judge c1 shape "), "{row}");
            assert!(row.contains("envelope"), "{row}");
        }
        // A body that is not JSON is `json`, from Cloudflare or any other.
        for d in [config::Destination::ClefFlash, config::Destination::Jev] {
            let mut h = home();
            sending_to(&mut h, d);
            let _rx = stub(vec![("200 OK", b"<html>busy</html>".to_vec())], "");
            let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(asked_, Asked::Skipped("shape"), "{d:?}");
            let row = &ledger(&h.db)[0];
            assert!(row.contains("json") && !row.contains("envelope"), "{row}");
        }
    }

    /// J7: a destination without what it needs sends nothing and keeps no row: Cloudflare's without
    /// its token or its account id, Liquid's without its key. d1 on this PC needs neither.
    #[test]
    fn a_destination_without_its_key_sends_nothing() {
        use config::Destination::*;
        let mut h = home();
        let rx = stub(vec![("200 OK", picked("g7"))], "");
        for (d, unset) in [(Clef, "token"), (ClefFlash, "account"), (D1, "key")] {
            sending_to(&mut h, d);
            match unset {
                "token" => h.judge.cloudflare_key_file = None,
                "account" => h.judge.cloudflare_account_id = None,
                _ => h.judge.liquid_key_file = None,
            }
            assert!(!ready(&h.judge), "{d:?}");
            let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
            assert_eq!(asked_, Asked::Skipped("key"), "{d:?}");
        }
        assert_eq!(requests(&rx), 0);
        assert!(ledger(&h.db).is_empty());
        sending_to(&mut h, D1Local);
        (
            h.judge.key_file,
            h.judge.cloudflare_key_file,
            h.judge.liquid_key_file,
        ) = (None, None, None);
        assert!(ready(&h.judge));
    }

    /// #786: d1 on this PC is asked only when a program that may be asked listens at its port. With
    /// none there, nothing is sent: the call is an `error` that says why, and no byte went out.
    #[cfg(target_os = "linux")]
    #[test]
    fn d1_on_this_pc_with_no_listener_sends_nothing() {
        let mut h = home();
        sending_to(&mut h, config::Destination::D1Local);
        // Port 0: no socket ever listens there, so no other test's stub can take it meanwhile.
        AT.set(Some("http://127.0.0.1:0/v1/systemone".to_owned()));
        let asked_ = run(&h, &rules(""), 5.0, asked(&[])).unwrap();
        assert_eq!(asked_, Asked::Refused("no listener"));
        // #791: the judges' ops keep it as any call that failed.
        assert_eq!(asked_.unanswered(), "error");
        assert_eq!(Asked::Skipped("budget").unanswered(), "budget");
        assert_eq!(
            ledger(&h.db),
            ["judge:d1-local judge c1 error no listener None None None"]
        );
        let out: i64 = (h.db)
            .query_row("SELECT bytes_out FROM provider_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(out, 0);
    }

    /// J7: the round's judge is not the one `[judge]` names once its destination, or the key file
    /// or account id that destination reads, changed since the round read the settings.
    #[test]
    fn a_destination_switched_since_the_round_read_it_is_not_still_on() {
        use config::Destination::*;
        let mut h = home();
        sending_to(&mut h, ClefFlash);
        let path = |f: &Option<std::path::PathBuf>| f.as_ref().unwrap().display().to_string();
        let (config, jev) = (h.dir.path().join("config.toml"), path(&h.judge.key_file));
        let table = |destination: &str, account: &str, cloudflare: &str| {
            let text = format!(
                "providers = []\n[judge]\ndestination = \"{destination}\"\nkey_file = '{jev}'\n\
                 cloudflare_key_file = '{cloudflare}'\ncloudflare_account_id = \"{account}\"\n"
            );
            std::fs::write(&config, text).unwrap();
        };
        let cloudflare = path(&h.judge.cloudflare_key_file);
        table("clef-flash", ACCOUNT, &cloudflare);
        assert!(still_on(h.dir.path(), &h.judge));
        table("clef", ACCOUNT, &cloudflare);
        assert!(!still_on(h.dir.path(), &h.judge));
        table("clef-flash", &"f".repeat(32), &cloudflare);
        assert!(!still_on(h.dir.path(), &h.judge));
        table("clef-flash", ACCOUNT, &path(&h.judge.liquid_key_file));
        assert!(!still_on(h.dir.path(), &h.judge));
        // Jev's key file is not Clef-flash's: Jev's alone changed, it is still the same judge.
        table("clef-flash", ACCOUNT, &cloudflare);
        h.judge.key_file = None;
        assert!(still_on(h.dir.path(), &h.judge));
        sending_to(&mut h, D1Local);
        table("d1-local", ACCOUNT, &cloudflare);
        assert!(still_on(h.dir.path(), &h.judge));
    }

    /// Doctor's line names each state without the key.
    #[test]
    fn doctor_names_the_judges_state() {
        let mut h = home();
        assert!(doctor_line(h.dir.path(), None).contains("off, no key set"));
        assert_eq!(
            doctor_line(h.dir.path(), Some(&h.judge)),
            "judge (Jev): on, 0 of 300 calls in the last 24 hours"
        );
        // Decision 64: no cap is said, not counted against 4294967295.
        h.judge.daily_budget = config::no_daily_cap();
        assert_eq!(
            doctor_line(h.dir.path(), Some(&h.judge)),
            "judge (Jev): on, 0 calls in the last 24 hours, no cap of calls a day"
        );
        h.judge.enabled = false;
        assert!(
            doctor_line(h.dir.path(), Some(&h.judge)).contains("off ([judge] enabled = false)")
        );
        h.judge.enabled = true;
        std::fs::write(h.judge.key_file.as_ref().unwrap(), "a note only\n").unwrap();
        let line = doctor_line(h.dir.path(), Some(&h.judge));
        assert!(line.contains("has no key on line 2"), "{line}");
        assert!(!line.contains(KEY));
        // J7: the destination is named, d1 on this PC with the address it is sent to; one without
        // what it needs says so, its account id not repeated.
        h.judge.daily_budget = 300;
        sending_to(&mut h, config::Destination::D1Local);
        assert_eq!(
            doctor_line(h.dir.path(), Some(&h.judge)),
            "judge (d1 on this PC, 127.0.0.1:11434): on, 0 of 300 calls in the last 24 hours"
        );
        sending_to(&mut h, config::Destination::ClefFlash);
        h.judge.cloudflare_account_id = None;
        let line = doctor_line(h.dir.path(), Some(&h.judge));
        assert_eq!(
            line,
            "judge (Clef-flash): on, but no Cloudflare account id is set: nothing is sent"
        );
        h.judge.cloudflare_account_id = Some(ACCOUNT.into());
        std::fs::write(h.judge.cloudflare_key_file.as_ref().unwrap(), "a note\n").unwrap();
        let line = doctor_line(h.dir.path(), Some(&h.judge));
        assert!(line.starts_with("judge (Clef-flash): on, but "), "{line}");
        assert!(
            line.contains("has no key on line 2") && !line.contains(ACCOUNT),
            "{line}"
        );
    }

    #[test]
    fn an_answer_fits_its_question_or_is_refused() {
        let questions = asked(&[]).questions;
        let mut noul = json!({"same": {"type": "noul", "instructions": "Same work?",
            "criteria": {"true": "Same", "false": "Different"}}});
        let fits = |q: &Value, a: Value| answered(q, &json!({"answers": a})).is_some();
        let choice = |choice: &str, p: f64| {
            json!({"pick": {"type": "choice", "choice": choice,
                "probabilities": {"g7": p, "none": 0.1}}})
        };
        assert!(fits(&questions, choice("g7", 0.9)));
        assert!(fits(&questions, choice("none", 0.0)));
        assert!(!fits(&questions, choice("g9", 0.9)));
        assert!(!fits(&questions, choice("g7", 1.5)));
        assert!(
            !fits(&questions, choice("g8", 0.9)),
            "no probability of its own"
        );
        assert!(!fits(
            &questions,
            json!({"pick": {"type": "choice", "choice": "g7", "probabilities": {}}})
        ));
        assert!(!fits(
            &questions,
            json!({"pick": {"type": "choice", "choice": "g7"}})
        ));
        assert!(!fits(
            &questions,
            json!({"pick": {"type": "noul", "noul": 0.5}})
        ));
        assert!(!fits(&questions, json!({})));
        assert!(fits(&noul, json!({"same": {"type": "noul", "noul": 0.96}})));
        assert!(!fits(
            &noul,
            json!({"same": {"type": "noul", "noul": -0.1}})
        ));
        noul["same"]["type"] = "score".into();
        assert!(!fits(
            &noul,
            json!({"same": {"type": "score", "score": 1.0}})
        ));
    }
