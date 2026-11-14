    use super::*;

    fn home(text: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        home
    }

    fn send(home: &Path, action: Value) -> Result<Value, Refusal> {
        let body = serde_json::to_vec(&json!({"version": show(home)["version"], "action": action}))
            .unwrap();
        save(home, &Mutex::new(()), &body)
    }

    fn http(name: &str, endpoint: &str) -> Value {
        json!({"kind": "openai", "name": name, "enabled": true, "base_url": endpoint,
            "model": "synthetic", "timeout_s": 30, "daily_budget": 20, "subscription": false,
            "limits": {"max_request_tokens": null, "daily_tokens": null,
                "usd_per_mtok_in": 0.0, "usd_per_mtok_out": 0.0, "max_output_tokens": 4000}})
    }

    fn cli(name: &str, model: Option<&str>, enabled: bool) -> Value {
        json!({"kind": "cli", "name": name, "enabled": enabled, "cli": "claude", "model": model,
            "timeout_s": 90, "limits": {"max_request_tokens": null, "daily_tokens": null,
                "usd_per_mtok_in": 0.0, "usd_per_mtok_out": 0.0, "max_output_tokens": 4000}})
    }

    /// Decision 78: an entry's model list is answered for the version the page read and the entry it
    /// names, ids only, nothing of its key or URL; kept, then read again on `refresh`; refused by its
    /// cause for a stale version, a subscription's program and a key file that holds no key.
    #[test]
    fn an_entrys_model_list_answers_ids_only_and_refuses_by_cause() {
        let owner = tempfile::tempdir().unwrap();
        let key = owner.path().join("LIST_KEY.md");
        let secret = ["List", "Synthetic", "Key999"].concat();
        std::fs::write(&key, format!("key\n{secret}\n")).unwrap();
        let empty = owner.path().join("EMPTY_KEY.md");
        std::fs::write(&empty, "key\n\n").unwrap();
        let list = json!({"data": [{"id": "model-a"}, {"id": "model-b"}]})
            .to_string()
            .into_bytes();
        let (url, requests) = crate::provider::tests::serve_each(
            vec![("200 OK", list.clone()), ("200 OK", list)],
            "",
        );
        let home = home(&format!(
            "[[providers]]\nkind = \"openai\"\nname = \"listed\"\nbase_url = '{url}'\n\
            model = \"synthetic\"\nkey_file = '{}'\n\
            [[providers]]\nkind = \"openai\"\nname = \"empty\"\nbase_url = '{url}'\n\
            model = \"synthetic\"\nkey_file = '{}'\n\
            [[providers]]\nkind = \"cli\"\nname = \"sub\"\ncli = \"claude\"\n",
            key.display(),
            empty.display()
        ));
        let shown = show(home.path());
        let ask = |name: &str, version: &Value, refresh: bool| {
            let at = shown["providers"].as_array().unwrap().iter();
            let selector = at.clone().find(|p| p["name"] == name).unwrap()["selector"].clone();
            let body = json!({"version": version, "selector": selector, "refresh": refresh});
            models(home.path(), &serde_json::to_vec(&body).unwrap())
        };
        let wait = std::time::Duration::from_secs(5);
        let first = ask("listed", &shown["version"], false).unwrap();
        assert_eq!(
            first,
            json!({"models": ["model-a", "model-b"], "from": "service"})
        );
        assert!(!first.to_string().contains(&secret) && !first.to_string().contains(&url));
        assert!(requests.recv_timeout(wait).is_ok());
        assert_eq!(
            ask("listed", &shown["version"], false).unwrap()["from"],
            "cache"
        );
        assert_eq!(
            ask("listed", &shown["version"], true).unwrap()["from"],
            "service"
        );
        assert!(requests.recv_timeout(wait).is_ok());
        let refusal = |r: Result<Value, Refusal>| {
            let r = r.unwrap_err();
            (r.status, r.code)
        };
        assert_eq!(
            refusal(ask("listed", &json!("stale"), false)),
            (409, "stale")
        );
        assert_eq!(
            refusal(ask("sub", &shown["version"], false)),
            (422, "unsupported")
        );
        assert_eq!(
            refusal(ask("empty", &shown["version"], false)),
            (502, "no_key")
        );
        assert!(
            requests
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
    }

    /// Decision 78: a list is asked, as a connection test is, only of a destination the page shows
    /// and with headers that leave its transport alone: plain HTTP to another host, or a header that
    /// would carry or redirect a credential, is refused before anything is sent.
    #[test]
    fn an_entrys_model_list_keeps_the_connection_tests_rules() {
        let owner = tempfile::tempdir().unwrap();
        let key = owner.path().join("LIST_KEY.md");
        std::fs::write(&key, "key\nSynthetic999\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let local = format!("http://{}/v1", listener.local_addr().unwrap());
        let home = home(&format!(
            "[[providers]]\nkind = \"openai\"\nname = \"plain\"\nbase_url = 'http://192.0.2.10/v1'\n\
            model = \"synthetic\"\nkey_file = '{0}'\n\
            [[providers]]\nkind = \"openai\"\nname = \"hosted\"\nbase_url = '{local}'\n\
            model = \"synthetic\"\nkey_file = '{0}'\nheaders = {{ Host = \"elsewhere.invalid\" }}\n",
            key.display()
        ));
        let shown = show(home.path());
        for (name, code) in [("plain", "bad_endpoint"), ("hosted", "unsafe_headers")] {
            let selector = shown["providers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["name"] == name)
                .unwrap()["selector"]
                .clone();
            let body = json!({"version": shown["version"], "selector": selector});
            let r = models(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!((r.status, r.code), (422, code), "{name}");
        }
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn catalog_creation_keeps_builtin_transport_but_uses_managed_keys_and_posted_limits() {
        for name in ["opencode-go", "nim", "anthropic", "groq"] {
            let home = home("providers = []\n");
            let path = home.path().join("config.toml");
            let builtin = config::from_text(&path, "")
                .unwrap()
                .providers
                .into_iter()
                .find(|provider| provider.name() == name)
                .unwrap();
            let Provider::Openai {
                base_url,
                api,
                model,
                headers,
                extra,
                retry_429,
                ..
            } = &builtin
            else {
                panic!("an HTTP preset");
            };
            let mut entry = http(name, base_url);
            entry["api"] = serde_json::to_value(api).unwrap();
            entry["model"] = json!(model);
            entry["enabled"] = json!(false);
            entry["subscription"] = json!(name == "opencode-go");
            if name == "opencode-go" {
                entry["daily_budget"] = Value::Null;
            }
            entry["limits"]["usd_per_mtok_in"] = json!(0.25);
            for changed in [
                None,
                Some("name"),
                Some("base_url"),
                Some("api"),
                Some("model"),
            ] {
                let mut candidate = entry.clone();
                if let Some(field) = changed {
                    candidate[field] = match field {
                        "base_url" => json!("https://example.invalid/v1"),
                        "api" => json!(if *api == config::Api::Anthropic {
                            "openai"
                        } else {
                            "anthropic"
                        }),
                        _ => json!("synthetic-other"),
                    };
                }
                send(home.path(), json!({"op": "create", "entry": candidate})).unwrap();
                let loaded =
                    config::from_text(&path, &std::fs::read_to_string(&path).unwrap()).unwrap();
                let Provider::Openai {
                    key_file,
                    headers: actual_headers,
                    extra: actual_extra,
                    retry_429: actual_retry,
                    enabled,
                    limits,
                    timeout_s,
                    daily_budget,
                    subscription,
                    ..
                } = loaded.providers.last().unwrap()
                else {
                    panic!("created HTTP entry");
                };
                if changed.is_none() {
                    assert_eq!(actual_headers, headers, "{name}");
                    assert_eq!(actual_extra, extra, "{name}");
                    assert_eq!(actual_retry, retry_429, "{name}");
                } else {
                    assert!(actual_headers.is_empty(), "{name}: {changed:?}");
                    assert!(actual_extra.is_empty());
                    assert!(!actual_retry);
                }
                assert!(
                    key_file.is_none(),
                    "keys still require managed registration"
                );
                assert!(!enabled);
                assert_eq!(*timeout_s, 30);
                assert_eq!(
                    *daily_budget,
                    if name == "opencode-go" {
                        None
                    } else {
                        Some(20)
                    }
                );
                assert_eq!(*subscription, name == "opencode-go");
                assert_eq!(limits.usd_per_mtok_in, 0.25);
                assert!(!home.path().join("providers.db").exists());
            }
        }
    }

    #[test]
    fn an_unsupported_legacy_entry_can_be_disabled_without_rewriting_its_private_fields() {
        let text = "[[providers]]\nkind = \"cli\"\nname = \"legacy\"\ncli = \"gemini\"\n\
            daily_budget = 7 # old cap\nfuture = \"retained\"\n";
        let home = home(text);
        let shown = show(home.path());
        let saved = send(home.path(), json!({"op": "enabled", "selector": shown["providers"][0]["selector"], "enabled": false})).unwrap();
        assert_eq!(saved["providers"][0]["effective"]["on"], false);
        assert!(
            config::load_chain(home.path())
                .unwrap()
                .providers
                .is_empty()
        );
        let disabled = bytes(home.path()).unwrap();
        let refused = send(home.path(), json!({"op": "enabled", "selector": saved["providers"][0]["selector"], "enabled": true})).unwrap_err();
        assert_eq!((refused.status, refused.code), (422, "unsupported_cli"));
        assert_eq!(bytes(home.path()).unwrap(), disabled);
        let after = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(after.contains("daily_budget = 7 # old cap"));
        assert!(after.contains("future = \"retained\""));
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn invalid_native_limits_and_cli_arguments_leave_the_usable_file_unchanged() {
        let home = home("providers = [] # still usable\n");
        let before = bytes(home.path()).unwrap();
        for (field, value) in [
            ("name", json!(" ")),
            ("model", json!("")),
            ("timeout_s", json!(0)),
            ("daily_budget", json!(0)),
        ] {
            let mut entry = http("local", "https://example.invalid/v1");
            entry[field] = value;
            assert!(
                send(home.path(), json!({"op": "create", "entry": entry})).is_err(),
                "{field}"
            );
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        for field in ["max_request_tokens", "daily_tokens", "max_output_tokens"] {
            let mut entry = http("local", "https://example.invalid/v1");
            entry["limits"][field] = json!(0);
            assert_eq!(
                send(home.path(), json!({"op": "create", "entry": entry}))
                    .unwrap_err()
                    .code,
                "range",
                "{field}"
            );
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        for model in ["--config=untrusted", "unsafe\nargument"] {
            assert!(
                send(
                    home.path(),
                    json!({"op": "create", "entry": cli("a", Some(model), true)})
                )
                .is_err()
            );
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        assert!(!home.path().join("providers.db").exists());
    }

    /// The judge's ledger rows are under `judge:` names: no chain entry is created or renamed so.
    #[test]
    fn a_chain_entry_cannot_take_a_judge_name() {
        let home = home("providers = [] # still usable\n");
        let before = bytes(home.path()).unwrap();
        for name in ["judge:jev", "judge:"] {
            let entry = http(name, "https://example.invalid/v1");
            assert_eq!(
                send(home.path(), json!({"op": "create", "entry": entry}))
                    .unwrap_err()
                    .code,
                "bad_entry",
                "{name}"
            );
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
    }

    /// A `judge:` entry written by hand is never tested: no request, no row of the judge's.
    #[test]
    fn a_hand_written_judge_name_is_never_tested() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let home = home(&format!(
            "[[providers]]\nkind = \"openai\"\nname = \"judge:jev\"\n\
            base_url = {endpoint:?}\nmodel = \"synthetic\"\n"
        ));
        let shown = show(home.path());
        let mut body =
            json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]});
        let refusal = preview(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap_err();
        assert_eq!((refusal.status, refusal.code), (422, "bad_entry"));
        body["confirmed"] = json!(true);
        let refusal = test(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap_err();
        assert_eq!((refusal.status, refusal.code), (422, "bad_entry"));
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn a_provider_save_rejects_unsafe_destinations_without_traffic_or_config_changes() {
        let home = home("providers = [] # preserved\n");
        let before = bytes(home.path()).unwrap();
        for endpoint in [
            "http://example.invalid/v1",
            "http://localhost:8080/v1",
            "file:///tmp/key",
            "ftp://example.invalid/v1",
            "https://user:password@example.invalid/v1",
            "https://example.invalid/v1?token=private",
            "https://example.invalid/v1#private",
            "https://example.invalid:99999/v1",
            "https://example.invalid:/v1",
            "https://example.invalid:abc/v1",
            "https://example.invalid:0/v1",
            "https://example.invalid/\nprivate",
            "https://example.invalid\\private",
        ] {
            let r = send(
                home.path(),
                json!({"op": "create", "entry": http("local", endpoint)}),
            )
            .expect_err("unsafe endpoint must be refused");
            assert_eq!((r.status, r.code), (422, "bad_endpoint"), "{endpoint}");
            assert_eq!(bytes(home.path()).unwrap(), before);
        }
        for endpoint in [
            "https://example.invalid/v1",
            "http://127.0.0.1:12345/v1",
            "http://[::1]:12345/v1",
        ] {
            let saved = send(
                home.path(),
                json!({"op": "create", "entry": http("local", endpoint)}),
            )
            .unwrap();
            assert_eq!(saved["providers"][0]["saved"]["base_url"], endpoint);
            send(
                home.path(),
                json!({"op": "remove", "selector": saved["providers"][0]["selector"]}),
            )
            .unwrap();
        }
        assert!(!home.path().join("providers.db").exists());
        assert!(!home.path().join("worker.lock").exists());
    }

    /// Decision 78: a model chosen on the automatic Gemini's line is an edit of it as of any entry:
    /// written into the file at its place with its model and prices, its key file kept, and no
    /// second Gemini left beside it.
    #[test]
    fn the_automatic_geminis_model_and_prices_are_edited_in_place() {
        let home = home("gemini = \"after-subscriptions\"\nproviders = []\n");
        let shown = show(home.path());
        let at = &shown["providers"][0];
        assert_eq!(at["selector"]["source"], "gemini");
        let s = &at["saved"];
        let mut limits = s["limits"].clone();
        limits["usd_per_mtok_in"] = json!(0.3);
        limits["usd_per_mtok_out"] = json!(2.5);
        let entry = json!({"kind": "openai", "name": "gemini", "enabled": s["enabled"],
            "base_url": s["base_url"], "api": s["api"], "model": "gemini-3.5-flash-lite",
            "timeout_s": s["timeout_s"], "subscription": s["subscription"],
            "daily_budget": s["daily_budget"], "limits": limits});
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": at["selector"], "entry": entry}),
        )
        .unwrap();
        let geminis: Vec<&Value> = saved["providers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["name"] == "gemini")
            .collect();
        assert_eq!(geminis.len(), 1, "{saved}");
        assert_eq!(geminis[0]["saved"]["model"], "gemini-3.5-flash-lite");
        assert_eq!(geminis[0]["saved"]["limits"]["usd_per_mtok_in"], 0.3);
        assert_eq!(geminis[0]["saved"]["limits"]["usd_per_mtok_out"], 2.5);
        let text = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(text.contains("GEMINI_API_KEY"), "{text}");
    }

    #[test]
    fn creating_a_native_gemini_keeps_the_existing_virtual_entry_until_explicit_removal() {
        let home = home("gemini = \"after-subscriptions\" # placement note\nproviders = []\n");
        let shown = show(home.path());
        assert_eq!(shown["providers"].as_array().unwrap().len(), 1);
        assert_eq!(shown["providers"][0]["selector"]["source"], "gemini");
        let saved = send(
            home.path(),
            json!({"op": "create", "entry": http("gemini", "https://example.invalid/v1")}),
        )
        .unwrap();
        assert_eq!(
            saved["providers"].as_array().unwrap().len(),
            2,
            "creating an entry must not silently replace the pre-existing virtual provider"
        );
        assert_eq!(
            saved["providers"][0]["saved"]["model"],
            shown["providers"][0]["saved"]["model"]
        );
        let saved = send(
            home.path(),
            json!({"op": "remove", "selector": saved["providers"][1]["selector"]}),
        )
        .unwrap();
        assert_eq!(saved["gemini"], "after-subscriptions");
        assert_eq!(saved["providers"].as_array().unwrap().len(), 1);
        let saved = send(
            home.path(),
            json!({"op": "remove", "selector": saved["providers"][0]["selector"]}),
        )
        .unwrap();
        assert_eq!(saved["gemini"], Value::Null);
        assert!(saved["providers"].as_array().unwrap().is_empty());
        assert!(
            std::fs::read_to_string(home.path().join("config.toml"))
                .unwrap()
                .contains("# placement note")
        );
        assert!(!home.path().join("providers.db").exists());
    }

    /// Parity row 11: a key saved on the providers page ends its entry's rest, and no other's.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_key_saved_on_the_providers_page_ends_its_entrys_rest() {
        let owner = tempfile::tempdir().unwrap();
        let home = home(
            "[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
             base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\n",
        );
        let db = crate::providers_db::open(home.path()).unwrap();
        let rest = crate::providers_db::State {
            down_until: 5_000_000_000_000,
            ..Default::default()
        };
        for p in ["a", "b"] {
            crate::providers_db::set_state(&db, p, rest).unwrap();
        }
        let shown = show(home.path());
        let body = serde_json::to_vec(&json!({"version": shown["version"],
            "selector": shown["providers"][0]["selector"], "key": "NewSyntheticKey111"}))
        .unwrap();
        save_key_at(home.path(), &Mutex::new(()), &body, owner.path(), None).unwrap();
        let rests = ["a", "b"].map(|p| crate::providers_db::state(&db, p).unwrap());
        assert_eq!(rests, [Default::default(), rest]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn managed_key_registration_binds_a_physical_entry_without_revealing_or_replacing_old_keys() {
        use std::os::unix::fs::PermissionsExt;
        let owner = tempfile::tempdir().unwrap();
        let legacy = owner.path().join("LEGACY_KEY.md");
        std::fs::write(&legacy, "old key\nOldSyntheticKey000\nnotes unchanged\n").unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
        let text = format!(
            "# original\n[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\nkey_file = {legacy:?}\n\
            [[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\nfuture = 7\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let canary = "NewSyntheticKeyCanary999";
        let body = serde_json::to_vec(&json!({"version": shown["version"],
            "selector": shown["providers"][1]["selector"], "key": canary}))
        .unwrap();
        let answer = save_key_at(home.path(), &Mutex::new(()), &body, owner.path(), None).unwrap();
        assert!(!answer.to_string().contains(canary));
        let shown = show(home.path());
        assert!(!shown.to_string().contains(canary));
        assert_eq!(
            shown["providers"][0]["saved"]["key_file"],
            legacy.to_str().unwrap()
        );
        let key_path =
            std::path::PathBuf::from(shown["providers"][1]["saved"]["key_file"].as_str().unwrap());
        assert!(key_path.starts_with(owner.path()) && !key_path.starts_with(home.path()));
        assert_eq!(key_path.extension().unwrap(), "md");
        assert_eq!(
            std::fs::read_to_string(&key_path).unwrap().lines().nth(1),
            Some(canary)
        );
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_to_string(&legacy).unwrap(),
            "old key\nOldSyntheticKey000\nnotes unchanged\n"
        );
        let config = bytes(home.path()).unwrap();
        assert!(
            !String::from_utf8(config.clone().unwrap())
                .unwrap()
                .contains(canary)
        );
        assert!(
            String::from_utf8(config.clone().unwrap())
                .unwrap()
                .contains("future = 7")
        );
        let r = save_key_at(home.path(), &Mutex::new(()), &body, owner.path(), None).unwrap_err();
        assert_eq!((r.status, r.code), (409, "stale"));
        assert_eq!(bytes(home.path()).unwrap(), config);
        assert!(!home.path().join("providers.db").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_config_save_discards_only_the_new_managed_key() {
        use std::os::unix::fs::PermissionsExt;
        let owner = tempfile::tempdir().unwrap();
        let old = owner.path().join("EXISTING_KEY.md");
        std::fs::write(&old, "key\nOldSyntheticKey999\nretained\n").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o600)).unwrap();
        let text = "[[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\n";
        let home = home(text);
        let config = home.path().join("config.toml");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o444)).unwrap();
        let shown = show(home.path());
        let body = serde_json::to_vec(&json!({"version": shown["version"],
            "selector": shown["providers"][0]["selector"], "key": "UnusedSyntheticKey999"}))
        .unwrap();
        let refused =
            save_key_at(home.path(), &Mutex::new(()), &body, owner.path(), None).unwrap_err();
        assert_eq!((refused.status, refused.code), (500, "write_failed"));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
        let managed = owner.path().join(".local/share/oboete/keys");
        assert!(
            managed.exists(),
            "the managed registration reached the subsequent config stage"
        );
        assert_eq!(std::fs::read_dir(managed).unwrap().count(), 0);
        assert_eq!(
            std::fs::read_to_string(old).unwrap(),
            "key\nOldSyntheticKey999\nretained\n"
        );
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn provider_edits_keep_native_noops_inline_fields_and_legacy_subscription_caps() {
        let text = "# kept\nproviders = [\n\
            { kind = \"cli\", name = \"a\", cli = \"claude\", model = \"first\", timeout_s = 90, daily_budget = 7, future = \"kept\" },\n\
            { kind = \"cli\", name = \"b\", cli = \"claude\", model = \"second\", timeout_s = 90 }\n\
            ] # list note\n[chain]\norder = [\"b\", \"a\"] # overlay note\n";
        let home = home(text);
        let shown = show(home.path());
        send(
            home.path(),
            json!({"op": "edit", "selector": shown["providers"][0]["selector"],
            "entry": cli("a", Some("first"), true)}),
        )
        .unwrap();
        assert_eq!(bytes(home.path()).unwrap().unwrap(), text.as_bytes());
        let moved = send(
            home.path(),
            json!({"op": "move", "selector": shown["providers"][0]["selector"], "to": 1}),
        )
        .unwrap();
        assert_eq!(moved["providers"][0]["name"], "b");
        assert_eq!(moved["providers"][1]["name"], "a");
        assert_eq!(moved["providers"][1]["effective"]["order"], 1);
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": moved["providers"][1]["selector"],
            "entry": cli("日本語の名前", Some("changed"), true)}),
        )
        .unwrap();
        assert_eq!(saved["providers"][1]["name"], "日本語の名前");
        let cfg = config::load(home.path()).unwrap();
        assert_eq!(
            cfg.providers
                .iter()
                .find(|p| p.name() == "日本語の名前")
                .unwrap()
                .daily_budget(),
            7
        );
        let after = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        for kept in [
            "# kept",
            "future = \"kept\"",
            "# list note",
            "# overlay note",
            "order = [\"b\", \"a\"]",
        ] {
            assert!(after.contains(kept), "{kept}: {after}");
        }
        let mut wrong = cli("x", None, true);
        wrong["daily_budget"] = json!(5);
        assert!(send(home.path(), json!({"op": "create", "entry": wrong})).is_err());
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            after
        );
    }

    /// Owner decision 41 (#164): a codex entry's `credits` is a switch on the page, saved as
    /// `credits = true` and left out when off; a page that sends none keeps it; claude has none.
    #[test]
    fn a_codex_entrys_credits_are_set_on_the_page() {
        let home = home("providers = []\n");
        let mut entry = cli("c", None, true);
        entry["cli"] = json!("codex");
        entry["credits"] = json!(true);
        let saved = send(home.path(), json!({"op": "create", "entry": entry})).unwrap();
        assert_eq!(saved["providers"][0]["saved"]["credits"], true);
        let text = || std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(text().contains("credits = true"), "{}", text());
        let selector = saved["providers"][0]["selector"].clone();
        let mut older = cli("c", Some("gpt-6-luna"), true);
        older["cli"] = json!("codex");
        send(
            home.path(),
            json!({"op": "edit", "selector": selector, "entry": older.clone()}),
        )
        .unwrap();
        assert!(text().contains("credits = true"), "{}", text());
        older["credits"] = json!(false);
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": selector, "entry": older}),
        )
        .unwrap();
        assert_eq!(saved["providers"][0]["saved"]["credits"], false);
        assert!(!text().contains("credits"), "{}", text());
        let before = text();
        let mut claude = cli("d", None, true);
        claude["credits"] = json!(true);
        let refused = send(home.path(), json!({"op": "create", "entry": claude})).unwrap_err();
        assert_eq!((refused.status, refused.code), (422, "range"));
        assert_eq!(text(), before);
    }

    /// Decision 64: an entry made or edited on the page may have no cap of calls a day, written
    /// 4294967295, and a subscription still takes none.
    #[test]
    fn an_http_entry_may_be_given_no_cap_of_calls_a_day() {
        let home = home("providers = []\n");
        let mut entry = http("open", "https://example.invalid/v1");
        entry["daily_budget"] = json!(u32::MAX);
        send(home.path(), json!({"op": "create", "entry": entry})).unwrap();
        let text = || String::from_utf8(bytes(home.path()).unwrap().unwrap()).unwrap();
        assert!(text().contains("daily_budget = 4294967295"), "{}", text());
        let path = home.path().join("config.toml");
        let loaded = config::from_text(&path, &text()).unwrap();
        assert_eq!(loaded.providers.last().unwrap().daily_budget(), u32::MAX);
        let mut sub = http("sub", "https://example.invalid/v1");
        sub["daily_budget"] = json!(u32::MAX);
        sub["subscription"] = json!(true);
        let before = text();
        let r = send(home.path(), json!({"op": "create", "entry": sub})).unwrap_err();
        assert_eq!((r.status, r.code), (422, "range"));
        assert_eq!(text(), before);
    }

    #[test]
    fn editing_other_native_fields_keeps_legacy_http_caps_but_rejects_new_invalid_caps() {
        for budget in [0_u32, u32::MAX] {
            for subscription in [false, true] {
                let text = format!(
                    "# original\n[[providers]]\nkind = \"openai\"\nname = \"legacy\"\n\
                    base_url = \"https://example.invalid/v1\"\nmodel = \"before\"\n\
                    daily_budget = {budget} # legacy cap\nsubscription = {subscription}\nfuture = \"kept\"\n"
                );
                let home = home(&text);
                let shown = show(home.path());
                let mut entry = http("legacy", "https://example.invalid/v1");
                entry["model"] = json!("after");
                entry["enabled"] = json!(false);
                entry["daily_budget"] = json!(budget);
                entry["subscription"] = json!(subscription);
                let selector = shown["providers"][0]["selector"].clone();
                let saved = send(
                    home.path(),
                    json!({"op": "edit", "selector": selector, "entry": entry}),
                )
                .unwrap();
                let native = &saved["providers"][0]["saved"];
                assert_eq!(native["daily_budget"], budget);
                assert_eq!(native["subscription"], subscription);
                assert_eq!(native["model"], "after");
                assert_eq!(native["enabled"], false);
                let after = bytes(home.path()).unwrap();
                let text = String::from_utf8(after.clone().unwrap()).unwrap();
                for kept in ["# original", "# legacy cap", "future = \"kept\""] {
                    assert!(text.contains(kept), "{text}");
                }
                // Out of range, and not the no cap (decision 64), which a subscription takes no
                // more than any other new cap.
                entry["daily_budget"] = json!(if budget == 0 { 100_001 } else { 0 });
                let r = send(
                    home.path(),
                    json!({"op": "edit", "selector": selector, "entry": entry}),
                )
                .unwrap_err();
                assert_eq!(
                    (r.status, r.code, r.field.as_str()),
                    (422, "range", "providers.daily_budget")
                );
                assert_eq!(bytes(home.path()).unwrap(), after);
                let r = send(home.path(), json!({"op": "create", "entry": entry})).unwrap_err();
                assert_eq!((r.status, r.code), (422, "range"));
                assert_eq!(bytes(home.path()).unwrap(), after);
            }
        }
    }

    #[test]
    fn moving_native_tables_keeps_their_nested_fields_and_unrelated_comments() {
        let text = "# root note\n[summary]\ncurate = false # summary note\n\
            [[providers]]\nkind = \"openai\"\nname = \"a\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"first\"\n\
            [providers.extra]\nmarker = \"belongs-to-a\" # a note\n\
            [backup]\ndir = \"retained-backup\" # unrelated note\n\
            [[providers]]\nkind = \"openai\"\nname = \"b\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"second\"\n\
            [providers.headers]\nx-marker = \"belongs-to-b\" # b note\n\
            [[providers]]\nkind = \"cli\"\nname = \"c\"\ncli = \"claude\"\n\
            [providers.limits]\ndaily_tokens = 3000 # c note\n";
        let home = home(text);
        let shown = show(home.path());
        let saved = send(
            home.path(),
            json!({"op": "move", "selector": shown["providers"][2]["selector"], "to": 0}),
        )
        .unwrap();
        let cfg = config::load(home.path()).unwrap();
        let names: Vec<_> = cfg.providers.iter().map(Provider::name).collect();
        assert_eq!(names, ["c", "a", "b"]);
        assert_eq!(saved["providers"][0]["name"], "c");
        assert_eq!(cfg.providers[0].limits().daily_tokens, Some(3000));
        let Provider::Openai { extra, .. } = &cfg.providers[1] else {
            panic!("a's kind changed")
        };
        assert_eq!(extra["marker"], "belongs-to-a");
        let Provider::Openai { headers, .. } = &cfg.providers[2] else {
            panic!("b's kind changed")
        };
        assert_eq!(headers["x-marker"], "belongs-to-b");
        let after = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        for kept in [
            "# root note",
            "# summary note",
            "# unrelated note",
            "# a note",
            "# b note",
            "# c note",
            "dir = \"retained-backup\"",
        ] {
            assert!(after.contains(kept), "{kept}: {after}");
        }
        send(
            home.path(),
            json!({"op": "remove", "selector": saved["providers"][0]["selector"]}),
        )
        .unwrap();
        assert_eq!(
            config::load(home.path())
                .unwrap()
                .providers
                .iter()
                .map(Provider::name)
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn builtin_edits_materialize_the_native_base_without_losing_effective_overlays() {
        let home = home("[chain]\norder = [\"claude\"]\ntimeout_s = { claude = 150 }\n");
        let shown = show(home.path());
        let row = shown["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "claude")
            .unwrap();
        let old = config::load(home.path()).unwrap();
        let mut draft = cli("claude", Some("haiku"), false);
        draft["timeout_s"] = row["saved"]["timeout_s"].clone();
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": row["selector"], "entry": draft}),
        )
        .unwrap();
        let new = config::load(home.path()).unwrap();
        assert_eq!(new.providers.len(), old.providers.len());
        for (a, b) in old.providers.iter().zip(&new.providers) {
            assert_eq!(a.name(), b.name());
            if a.name() != "claude" {
                assert_eq!(toml::to_string(a).unwrap(), toml::to_string(b).unwrap());
            }
        }
        assert_eq!(saved["chain"][0]["name"], "claude");
        assert_eq!(saved["chain"][0]["effective_timeout_s"], 150);
        assert!(
            saved["providers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["selector"]["source"] == "file")
        );
        assert!(
            config::load_chain(home.path())
                .unwrap()
                .providers
                .iter()
                .all(|p| p.name() != "claude")
        );
        assert!(!home.path().join("providers.db").exists());
    }

    #[test]
    fn a_probe_preview_is_bound_to_saved_selection_and_makes_no_files_or_requests() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let text = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\n\
            base_url = {endpoint:?}\nmodel = \"synthetic\"\n\
            limits = {{ usd_per_mtok_in = 1.0, usd_per_mtok_out = 1.0 }}\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let mut body =
            json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]});
        let ready = preview(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(ready["version"], shown["version"]);
        assert_eq!(ready["selector"], body["selector"]);
        assert_eq!(ready["fixture"], crate::provider::PROBE_PROMPT);
        assert_eq!(ready["destination"], format!("{endpoint}/chat/completions"));
        assert_eq!(ready["ready"], true);
        assert_eq!(ready["max_output_tokens"], 128);
        assert!(ready["estimated_usd"].as_f64().unwrap() > 0.0);
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            text
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
        body["version"] = json!("stale");
        assert_eq!(
            preview(home.path(), &serde_json::to_vec(&body).unwrap())
                .unwrap_err()
                .status,
            409
        );
        body["version"] = shown["version"].clone();
        body["prompt"] = json!("private history must not be accepted");
        assert!(preview(home.path(), &serde_json::to_vec(&body).unwrap()).is_err());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_cli_probe_preview_is_truthfully_unavailable_without_ledger_or_process_work() {
        for cli in ["claude", "codex"] {
            let text =
                format!("[[providers]]\nkind = \"cli\"\nname = \"synthetic\"\ncli = {cli:?}\n");
            let home = home(&text);
            let shown = show(home.path());
            let body = serde_json::to_vec(&json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]})).unwrap();
            let answer = preview(home.path(), &body).unwrap();
            assert_eq!(answer["ready"], false);
            assert_eq!(answer["code"], "cli_probe_unbounded");
            assert_eq!(answer["max_output_tokens"], Value::Null);
            assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
            let mut confirmed: Value = serde_json::from_slice(&body).unwrap();
            confirmed["confirmed"] = json!(true);
            let r = test(home.path(), &serde_json::to_vec(&confirmed).unwrap()).unwrap_err();
            assert_eq!((r.status, r.code), (422, "cli_probe_unbounded"));
            assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
            assert_eq!(
                std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
                text
            );
        }
    }

    #[test]
    fn an_unreadable_ledger_reports_a_safe_cause_without_sending_or_rewriting_it() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let home = home(&format!(
            "[[providers]]\nkind = \"openai\"\nname = \"synthetic\"\nbase_url = {endpoint:?}\nmodel = \"synthetic\"\n"
        ));
        let shown = show(home.path());
        let body =
            json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]});
        let ledger = home.path().join("providers.db");
        let canary = b"not a database: PrivateSyntheticCanary999";
        std::fs::write(&ledger, canary).unwrap();
        let r = preview(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap_err();
        assert_eq!((r.status, r.code), (503, "ledger_invalid"));
        let mut confirmed = body.clone();
        confirmed["confirmed"] = json!(true);
        let r = test(home.path(), &serde_json::to_vec(&confirmed).unwrap()).unwrap_err();
        assert_eq!((r.status, r.code), (503, "ledger_invalid"));
        assert!(!format!("{r:?}").contains("PrivateSyntheticCanary999"));
        assert!(!format!("{r:?}").contains(home.path().to_str().unwrap()));
        assert_eq!(std::fs::read(&ledger).unwrap(), canary);
        assert!(!home.path().join("dispatch.lock").exists());
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn a_busy_ledger_preview_is_retryable_and_does_not_send_or_create_files() {
        let home = home(
            "[[providers]]\nkind = \"openai\"\nname = \"synthetic\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"synthetic\"\n",
        );
        let db = crate::providers_db::open(home.path()).unwrap();
        db.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;")
            .unwrap();
        let shown = show(home.path());
        let body = serde_json::to_vec(
            &json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]}),
        )
        .unwrap();
        let r = preview(home.path(), &body).unwrap_err();
        assert_eq!((r.status, r.code), (503, "provider_busy"));
        assert!(!home.path().join("dispatch.lock").exists());
        assert!(!home.path().join("raw.db").exists());
        db.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(preview(home.path(), &body).unwrap()["ready"], true);
    }

    #[test]
    fn a_probe_does_not_shrink_the_normal_fallback_for_legacy_unmetered_calls() {
        let home = home(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\n\
            base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\n\
            [providers.limits]\ndaily_tokens = 4100\nmax_output_tokens = 4000\nusd_per_mtok_in = 1.0\n",
        );
        let db = crate::providers_db::open(home.path()).unwrap();
        crate::providers_db::record(
            &db,
            &crate::providers_db::Call {
                provider: "local",
                role: "curator",
                span: "legacy",
                outcome: "error",
                ms: 1,
                detail: Some("transport"),
                bytes_out: 100,
                est_tokens: Some(100),
                usage: crate::providers_db::Usage::default(),
                usd: None,
            },
        )
        .unwrap();
        let shown = show(home.path());
        let body = serde_json::to_vec(
            &json!({"version": shown["version"], "selector": shown["providers"][0]["selector"]}),
        )
        .unwrap();
        let answer = preview(home.path(), &body).unwrap();
        assert_eq!(
            answer["ready"], false,
            "the current normal fallback already consumes the daily token cap: {answer}"
        );
        assert_eq!(answer["code"], "budget");
        assert_eq!(
            db.query_row("SELECT count(*) FROM provider_calls", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT detail FROM provider_calls", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "transport"
        );
        assert!(!home.path().join("dispatch.lock").exists());
    }

    #[test]
    fn the_explicit_test_requires_consent_and_current_selection_then_sends_one_fixed_request() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let text = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"local\"\n\
            base_url = {endpoint:?}\nmodel = \"synthetic\"\n\
            extra = {{ messages = [\"private history canary\"], models = [\"fallback\"], stream = true }}\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let mut body = json!({"version": shown["version"], "selector": shown["providers"][0]["selector"], "confirmed": false});
        assert_eq!(
            test(home.path(), &serde_json::to_vec(&body).unwrap())
                .unwrap_err()
                .code,
            "confirmation"
        );
        body["confirmed"] = json!(true);
        body["version"] = json!("stale");
        assert_eq!(
            test(home.path(), &serde_json::to_vec(&body).unwrap())
                .unwrap_err()
                .code,
            "stale"
        );
        body["version"] = shown["version"].clone();
        assert!(!home.path().join("providers.db").exists());
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        let server = std::thread::spawn(move || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < until =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    result => panic!("private test connection: {result:?}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                assert!(request.len() <= 64 * 1024);
                if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&request[..at]).unwrap();
                    let size: usize = header
                        .lines()
                        .filter_map(|l| l.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .unwrap()
                        .1
                        .trim()
                        .parse()
                        .unwrap();
                    if request.len() >= at + 4 + size {
                        break;
                    }
                }
            }
            let data = json!({"choices": [{"message": {"content": "{\"ok\":true}"}}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 4}})
            .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len()).unwrap();
            drop(stream);
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
            request
        });
        let result = test(home.path(), &serde_json::to_vec(&body).unwrap()).unwrap();
        let request = server.join().unwrap();
        assert_eq!(result["status"], "ok");
        assert!(!result.to_string().contains("private history canary"));
        let at = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let payload: Value = serde_json::from_slice(&request[at..]).unwrap();
        assert_eq!(
            payload["messages"],
            json!([{"role": "user", "content": crate::provider::PROBE_PROMPT}])
        );
        assert_eq!(payload["stream"], false);
        assert!(payload.get("models").is_none());
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            text
        );
        assert!(!home.path().join("raw.db").exists() && !home.path().join("knowledge.db").exists());
        let db = crate::providers_db::read_only(home.path())
            .unwrap()
            .unwrap();
        assert_eq!(crate::providers_db::last_calls(&db, 10).unwrap().len(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn legacy_key_saves_wait_for_the_config_writer_and_cannot_retarget_a_stale_group() {
        use std::os::unix::fs::PermissionsExt;
        let owner = tempfile::tempdir().unwrap();
        let key = owner.path().join("EXISTING_KEY.md");
        std::fs::write(&key, "key\nUnchangedSynthetic999\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let text = format!(
            "[[providers]]\nkind = \"openai\"\nname = \"a\"\nbase_url = \"https://example.invalid/v1\"\n\
            model = \"synthetic\"\nkey_file = {key:?}\n"
        );
        let home = home(&text);
        let shown = show(home.path());
        let body = serde_json::to_vec(
            &json!({"version": shown["version"], "entry": "a", "key": "ReplacementSynthetic999"}),
        )
        .unwrap();
        let held = config_lock(home.path()).unwrap();
        let path = home.path().to_path_buf();
        let (started, waiting) = std::sync::mpsc::channel();
        let (finished, result) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            started.send(()).unwrap();
            finished
                .send(crate::settings::save_key(&path, &Mutex::new(()), &body))
                .unwrap();
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            matches!(
                result.recv_timeout(std::time::Duration::from_millis(200)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "legacy key save escaped the shared writer lock"
        );
        let changed = text.replace("name = \"a\"", "name = \"renamed\"");
        std::fs::write(home.path().join("config.toml"), &changed).unwrap();
        std::fs::File::unlock(&held).unwrap();
        let r = result
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap_err();
        writer.join().unwrap();
        assert_eq!((r.status, r.code), (409, "stale"));
        assert_eq!(
            std::fs::read_to_string(key).unwrap(),
            "key\nUnchangedSynthetic999\n"
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
            changed
        );
    }

    /// Codex on #409: an entry switched to the other API keeps none of its `extra`, which speaks
    /// the API it had; an edit that keeps the API keeps it.
    #[test]
    fn switching_an_entrys_api_drops_its_extra() {
        let home = home(
            "[[providers]]\nkind = \"openai\"\nname = \"proxy\"\n\
             base_url = \"https://example.invalid/v1\"\nmodel = \"synthetic\"\n\
             extra = { chat_template_kwargs = { enable_thinking = false } }\n",
        );
        let selector = show(home.path())["providers"][0]["selector"].clone();
        let text = || std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        let entry = http("proxy", "https://example.invalid/v1");
        send(
            home.path(),
            json!({"op": "edit", "selector": selector, "entry": entry}),
        )
        .unwrap();
        assert!(text().contains("chat_template_kwargs"), "{}", text());
        let selector = show(home.path())["providers"][0]["selector"].clone();
        let mut entry = http("proxy", "https://example.invalid/v1");
        entry["api"] = json!("anthropic");
        send(
            home.path(),
            json!({"op": "edit", "selector": selector, "entry": entry}),
        )
        .unwrap();
        assert!(
            text().contains("api = \"anthropic\"") && !text().contains("chat_template_kwargs"),
            "{}",
            text()
        );
    }

    /// The owner, 2026-10-09: an HTTP entry's API is chosen on the page and saved as `api`, left
    /// out for the OpenAI-compatible default; its test previews the Messages endpoint.
    #[test]
    fn an_entrys_api_is_saved_shown_and_previewed() {
        let home = home("providers = []\n");
        let mut entry = http("claude", "https://api.anthropic.com/v1");
        entry["api"] = json!("anthropic");
        let saved = send(home.path(), json!({"op": "create", "entry": entry})).unwrap();
        assert_eq!(saved["providers"][0]["saved"]["api"], "anthropic");
        let text = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(text.contains("api = \"anthropic\""), "{text}");
        let body = serde_json::to_vec(
            &json!({"version": saved["version"], "selector": saved["providers"][0]["selector"]}),
        )
        .unwrap();
        let shown = preview(home.path(), &body).unwrap();
        assert_eq!(
            shown["destination"],
            "https://api.anthropic.com/v1/messages"
        );
        let mut entry = http("claude", "https://api.anthropic.com/v1");
        entry["api"] = json!("openai");
        let selector = &saved["providers"][0]["selector"];
        let saved = send(
            home.path(),
            json!({"op": "edit", "selector": selector, "entry": entry}),
        )
        .unwrap();
        assert_eq!(saved["providers"][0]["saved"]["api"], "openai");
        let text = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(!text.contains("api = "), "{text}");
        // An unknown API is refused with the file kept; an older page's entry, without one, is
        // OpenAI-compatible.
        let before = bytes(home.path()).unwrap();
        let mut entry = http("other", "https://example.invalid/v1");
        entry["api"] = json!("anthropic-ish");
        let refused = send(home.path(), json!({"op": "create", "entry": entry})).unwrap_err();
        assert_eq!((refused.status, refused.code), (400, "bad_request"));
        assert_eq!(bytes(home.path()).unwrap(), before);
        let entry = http("old", "https://example.invalid/v1");
        let saved = send(home.path(), json!({"op": "create", "entry": entry})).unwrap();
        assert_eq!(saved["providers"][1]["saved"]["api"], "openai");
    }
