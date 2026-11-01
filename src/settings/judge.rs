//! The judge on the settings page (docs/my-work-model.md J1, J7; slices 11a-2 and 11d): where it
//! sends, its state, a key typed on the page for each service, and one connection test of a
//! made-up question. Answers carry codes, never a key.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ManagedKey, Refusal, invalid, put, refused};
use crate::config::Destination;
use crate::judge::Asked;
#[cfg(test)]
use crate::judge::NAME;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Test {
    confirmed: bool,
}

/// The body of a key typed for a destination (J7). An older page sends Typesafe's alone (Jev's).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JudgeKey {
    key: String,
    version: String,
    #[serde(default)]
    destination: Destination,
    /// Cloudflare's account id, with its token: the address names it.
    #[serde(default)]
    account_id: Option<String>,
}

/// The judge as the page shows it: `set` when `[judge]` is there, its switch, where it sends and the
/// list it chooses from (each destination's key: `ok`, `missing`, or `none` for d1 on this PC, its
/// price, and whether it stays on this PC), Cloudflare's account id, the calls in the last 24 hours
/// of the destination in use (as its cap counts them), and the judge's spend this month wherever it
/// was sent. A ledger that does not read gives no numbers (null), as the page's other totals.
pub(crate) fn section(
    judge: Option<&crate::config::Judge>,
    ledger: &anyhow::Result<Option<Connection>>,
) -> Value {
    let to = judge.map_or(Destination::Jev, |j| j.destination);
    let name = crate::judge::fixed(to).name;
    let (calls, usd) = match ledger {
        Ok(Some(db)) => (
            crate::providers_db::calls_in_a_day(db, name)
                .ok()
                .map(|(calls, _)| calls),
            // The judge's spend, wherever it was sent.
            (Destination::ALL.iter())
                .try_fold(0.0, |sum, d| {
                    let name = crate::judge::fixed(*d).name;
                    crate::providers_db::usd_this_month_of(db, name).map(|usd| sum + usd)
                })
                .ok(),
        ),
        Ok(None) => (Some(0), Some(0.0)),
        Err(_) => (None, None),
    };
    // A key's state is its file's; Cloudflare's account id has its own field on the page.
    let key = |d: Destination| match (crate::judge::fixed(d).key, judge) {
        (None, _) => "none",
        (Some(_), Some(j))
            if crate::judge::key_file_of(j, d)
                .is_some_and(|f| crate::config::read_key(f).is_ok()) =>
        {
            "ok"
        }
        (Some(_), _) => "missing",
    };
    let destinations: Vec<Value> = (Destination::ALL.iter())
        .map(|&d| {
            let fixed = crate::judge::fixed(d);
            json!({"id": d.id(), "key": key(d), "usd_per_mtok_in": fixed.usd_per_mtok_in,
                "local": fixed.key.is_none()})
        })
        .collect();
    let key_file = (judge.and_then(crate::judge::key_file)).map(|f| f.display().to_string());
    json!({
        "set": judge.is_some(),
        "enabled": judge.is_some_and(|j| j.enabled),
        "destination": to.id(),
        "destinations": destinations,
        "key": key(to),
        "key_file": key_file,
        "cloudflare_account_id": judge.and_then(|j| j.cloudflare_account_id.clone()),
        "daily_budget": judge.map(|j| j.daily_budget),
        "calls_today": calls,
        "usd_this_month": usd,
    })
}

/// `POST /api/judge/key {"key", "version", "destination", "account_id"}`: the key typed on the page
/// for a destination, written as a provider's is (`keyfile::managed`: a new owner-only file outside
/// the store), its service's `[judge]` field pointed at it, Cloudflare's account id beside it, that
/// destination chosen and the judge turned on, against the version the page showed (J7). The old
/// file is left as it is. `authorized` is the request's token checked again under config.lock.
pub(crate) fn save_key(
    home: &Path,
    saving: &Mutex<()>,
    authorized: &dyn Fn() -> bool,
    body: &[u8],
) -> Result<Value, Refusal> {
    save_key_with(home, (saving, authorized), body, None)
}

/// `save_key` with where the key's file may go (`managed_key`'s `homes`). Everything the body names
/// is checked before a file is written: d1 on this PC takes no key, and Cloudflare's token comes
/// with its account id, which no other's does.
fn save_key_with(
    home: &Path,
    auth: (&Mutex<()>, &dyn Fn() -> bool),
    body: &[u8],
    homes: Option<(&Path, Option<&Path>)>,
) -> Result<Value, Refusal> {
    let posted: JudgeKey =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let to = posted.destination;
    let Some(field) = crate::judge::fixed(to).key.map(|key| key.field) else {
        return Err(refused(422, "no_key", "judge.key"));
    };
    let cloudflare = crate::judge::cloudflare(to);
    let account = match posted.account_id {
        Some(a) if cloudflare && crate::config::account_id(&a) => Some(a),
        _ if cloudflare => return Err(refused(422, "bad_account", "judge.account_id")),
        Some(_) => return Err(refused(400, "bad_request", "")),
        None => None,
    };
    let key = ManagedKey {
        key: posted.key,
        version: posted.version,
    };
    super::managed_key_at(home, auth, &key, homes, ("judge", field), |doc| {
        put(doc, "judge", "destination", to.id().into());
        if let Some(a) = account {
            put(doc, "judge", "cloudflare_account_id", a.into());
        }
        turned_on(doc);
    })
}

/// J1: off until a key is set; setting one is the owner turning it on.
fn turned_on(doc: &mut toml_edit::DocumentMut) {
    put(doc, "judge", "enabled", true.into());
}

/// `POST /api/judge/test {"confirmed": true}`: one made-up question with no recorded text, through
/// `judge::ask` as any call goes (its budget, its ledger row). The answer is its outcome:
/// `ok`, or why there is none (`judge::Asked::Skipped`).
pub(crate) fn test(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Test = serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if !posted.confirmed {
        return Err(refused(422, "confirmation", "judge.test"));
    }
    let cfg = crate::config::load(home).map_err(|_| invalid())?;
    let Some(judge) = &cfg.judge else {
        return Err(refused(422, "missing_key", "judge.test"));
    };
    let internal = |_| refused(500, "ledger_invalid", "judge.test");
    let raw = crate::raw::open(home).map_err(internal)?;
    let db = crate::providers_db::open(home).map_err(internal)?;
    let rules = crate::redact::Rules::load(home).map_err(|_| invalid())?;
    let ask = crate::judge::Ask {
        span: "settings-test",
        state: json!({"text": "This is a connection test from a settings page."}),
        questions: json!({"english": {"type": "noul",
            "instructions": "Is `text` written in English?",
            "criteria": {"true": "Yes", "false": "No"}}}),
        repos: &[],
    };
    let asked = crate::judge::ask(
        &db,
        &raw,
        &rules,
        (judge, cfg.paid_usd_per_month),
        ask,
        &|| Ok(None),
    )
    .map_err(internal)?;
    let outcome = match asked {
        Asked::Answered(_) => "ok",
        // #791: nothing went: said apart from a request that had no good answer.
        Asked::Skipped("error")
            if crate::judge::refused_before_sending(&db, judge).map_err(internal)? =>
        {
            "refused"
        }
        Asked::Skipped(why) => why,
    };
    Ok(json!({ "outcome": outcome }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(text: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        home
    }

    fn file(home: &Path) -> String {
        std::fs::read_to_string(home.join("config.toml")).unwrap()
    }

    /// J1: a key typed on the page goes into a managed file (Linux only: `keyfile::managed`), `[judge]` points at it and the judge
    /// is on; the answer shows the key's state and never the key. A stale version or a refused
    /// token writes nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_key_saved_on_the_page_turns_the_judge_on_and_is_in_no_answer() {
        let home = home("providers = []\n");
        let owner = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let shown = super::super::show(home.path());
        assert_eq!(shown["judge"]["set"], false);
        assert_eq!(shown["judge"]["key"], "missing");
        let key = format!("apikey_{}_{}", "0123456789abcdef", "synthetic");
        let body =
            |version: &Value| serde_json::to_vec(&json!({"key": key, "version": version})).unwrap();
        let save = |body: &[u8], ok: bool| {
            let homes = Some((owner.path(), Some(data.path())));
            save_key_with(home.path(), (&Mutex::new(()), &|| ok), body, homes)
        };
        let before = file(home.path());
        assert_eq!(save(&body(&json!("stale")), true).unwrap_err().status, 409);
        assert_eq!(
            save(&body(&shown["version"]), false).unwrap_err().status,
            401
        );
        assert_eq!(file(home.path()), before);
        let answer = save(&body(&shown["version"]), true).unwrap();
        assert!(!answer.to_string().contains(&key), "{answer}");
        assert_eq!(answer["judge"]["set"], true);
        assert_eq!(answer["judge"]["enabled"], true);
        assert_eq!(answer["judge"]["key"], "ok");
        let cfg = crate::config::load(home.path()).unwrap();
        let judge = cfg.judge.unwrap();
        assert!(judge.enabled);
        let key_file = judge.key_file.as_ref().unwrap();
        assert_eq!(crate::config::read_key(key_file).unwrap(), key);
        assert!(!key_file.starts_with(home.path()), "outside the store");
        assert!(!file(home.path()).contains(&key));
        // A key of another shape is refused before anything is written.
        let after = file(home.path());
        let version = super::super::show(home.path())["version"].clone();
        let bad = serde_json::to_vec(&json!({"key": "has space", "version": version})).unwrap();
        assert_eq!(save(&bad, true).unwrap_err().code, "bad_key");
        assert_eq!(file(home.path()), after);
        // A judge turned off on the page is on again with a new key.
        assert!(after.contains("enabled = true"), "{after}");
        let off = after.replace("enabled = true", "enabled = false");
        std::fs::write(home.path().join("config.toml"), off).unwrap();
        assert!(
            !crate::config::load(home.path())
                .unwrap()
                .judge
                .unwrap()
                .enabled
        );
        let version = super::super::show(home.path())["version"].clone();
        save(&body(&version), true).unwrap();
        assert!(
            crate::config::load(home.path())
                .unwrap()
                .judge
                .unwrap()
                .enabled
        );
    }

    /// The connection test needs its confirmation and a key; it sends one made-up question.
    #[test]
    fn the_connection_test_needs_confirmation_and_a_key() {
        let home = home("providers = []\n");
        let refusal = |body: Value| test(home.path(), &serde_json::to_vec(&body).unwrap());
        assert_eq!(
            refusal(json!({"confirmed": false})).unwrap_err().code,
            "confirmation"
        );
        assert_eq!(
            refusal(json!({"confirmed": true})).unwrap_err().code,
            "missing_key"
        );
        assert_eq!(refusal(json!({"prompt": "x"})).unwrap_err().status, 400);
        assert!(!home.path().join("providers.db").exists());
    }

    /// #791: a test of d1 on this PC that the check refused sent nothing, and says `refused`; one
    /// that went and had no good answer is still `error`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_test_refused_before_sending_is_told_apart_from_one_unanswered() {
        let home = home("providers = []\n[judge]\nenabled = true\ndestination = \"d1-local\"\n");
        let tested = || test(home.path(), br#"{"confirmed": true}"#).unwrap();
        // Port 0: no socket ever listens there.
        crate::judge::AT.set(Some("http://127.0.0.1:0/v1/systemone".to_owned()));
        assert_eq!(tested(), json!({"outcome": "refused"}));
        let (url, _rx) = crate::provider::tests::serve_each(vec![("500 Oops", b"{}".to_vec())], "");
        crate::judge::AT.set(Some(url));
        assert_eq!(tested(), json!({"outcome": "error"}));
    }

    /// J7: a key typed for a destination goes into its service's own field (a managed file outside
    /// the store), with the destination chosen and the judge on; Cloudflare's with its account id,
    /// which must be one. d1 on this PC takes no key. A refused key writes nothing, and no answer
    /// holds a key.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_key_saved_for_a_destination_goes_into_its_own_field() {
        use crate::config::Destination;
        const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
        let home = home("providers = []\n");
        let owner = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let save = |mut body: Value| {
            body["version"] = super::super::show(home.path())["version"].clone();
            let homes = Some((owner.path(), Some(data.path())));
            let body = serde_json::to_vec(&body).unwrap();
            save_key_with(home.path(), (&Mutex::new(()), &|| true), &body, homes)
        };
        let token = format!("cfut_{}_{}", "0123456789abcdef", "synthetic");
        let before = file(home.path());
        for (body, code) in [
            (
                json!({"key": token, "destination": "clef-flash"}),
                "bad_account",
            ),
            (
                json!({"key": token, "destination": "clef", "account_id": "0123456789ABCDEF0123456789abcdef"}),
                "bad_account",
            ),
            (json!({"key": token, "destination": "d1-local"}), "no_key"),
        ] {
            assert_eq!(save(body).unwrap_err().code, code);
        }
        let other = json!({"key": token, "destination": "d1", "account_id": ACCOUNT});
        assert_eq!(save(other).unwrap_err().status, 400);
        assert_eq!(file(home.path()), before);
        let answer =
            save(json!({"key": token, "destination": "clef-flash", "account_id": ACCOUNT}));
        let answer = answer.unwrap();
        assert!(!answer.to_string().contains(&token), "{answer}");
        assert_eq!(answer["judge"]["destination"], "clef-flash");
        let j = crate::config::load(home.path()).unwrap().judge.unwrap();
        assert_eq!((j.destination, j.enabled), (Destination::ClefFlash, true));
        assert!(j.key_file.is_none(), "Typesafe's left unset");
        assert_eq!(j.cloudflare_account_id.as_deref(), Some(ACCOUNT));
        let file_ = j.cloudflare_key_file.clone().unwrap();
        assert_eq!(crate::config::read_key(&file_).unwrap(), token);
        assert!(!file_.starts_with(home.path()), "outside the store");
        assert!(crate::judge::ready(&j));
        let liquid = format!("liquid_{}_{}", "0123456789abcdef", "synthetic");
        save(json!({"key": liquid, "destination": "d1"})).unwrap();
        let j = crate::config::load(home.path()).unwrap().judge.unwrap();
        assert_eq!(j.destination, Destination::D1);
        let file_ = j.liquid_key_file.clone().unwrap();
        assert_eq!(crate::config::read_key(&file_).unwrap(), liquid);
        assert_eq!(
            j.cloudflare_account_id.as_deref(),
            Some(ACCOUNT),
            "left as it was"
        );
        assert!(!file(home.path()).contains(&token) && !file(home.path()).contains(&liquid));
    }

    /// J7: the page is shown the list (each destination, whether its key is there, its price, and
    /// whether it stays on this PC) and the destination in use, whose calls and spend are its own
    /// rows'. No key is in it.
    #[test]
    fn the_section_lists_the_destinations_and_counts_the_one_in_use() {
        const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
        let dir = tempfile::tempdir().unwrap();
        let db = crate::providers_db::open(dir.path()).unwrap();
        for (provider, usd) in [(NAME, 0.25), ("judge:clef-flash", 0.5)] {
            crate::providers_db::record(
                &db,
                &crate::providers_db::Call {
                    provider,
                    role: "judge",
                    span: "s",
                    outcome: "ok",
                    ms: 1,
                    detail: None,
                    bytes_out: 1,
                    est_tokens: None,
                    usage: Default::default(),
                    usd: Some(usd),
                },
            )
            .unwrap();
        }
        let cloudflare = dir.path().join("CF_KEY.md");
        std::fs::write(&cloudflare, "# a test key\nCFTOKEN_for_the_stub\n").unwrap();
        let judge = crate::config::Judge {
            enabled: true,
            destination: crate::config::Destination::ClefFlash,
            key_file: None,
            cloudflare_key_file: Some(cloudflare),
            cloudflare_account_id: Some(ACCOUNT.into()),
            liquid_key_file: None,
            model: "jev-latest".into(),
            timeout_s: 15,
            daily_budget: 300,
        };
        let shown = section(Some(&judge), &Ok(Some(db)));
        // The calls are the destination's in use, as the cap counts them; the spend is the judge's.
        assert_eq!(
            (
                &shown["destination"],
                &shown["calls_today"],
                &shown["usd_this_month"],
                &shown["key"]
            ),
            (&json!("clef-flash"), &json!(1), &json!(0.75), &json!("ok"))
        );
        // A key's state is its file's: Cloudflare's token is there without the account id too,
        // which the page shows in its own field.
        let no_account = crate::config::Judge {
            cloudflare_account_id: None,
            ..judge.clone()
        };
        let shown_ = section(Some(&no_account), &Ok(None));
        assert_eq!(shown_["destinations"][2]["key"], "ok");
        assert_eq!(shown_["cloudflare_account_id"], Value::Null);
        assert_eq!(shown["cloudflare_account_id"], ACCOUNT);
        let list = shown["destinations"].as_array().unwrap();
        let of = |key: &str| list.iter().map(|d| d[key].clone()).collect::<Vec<_>>();
        assert_eq!(
            of("id"),
            [
                json!("jev"),
                json!("clef"),
                json!("clef-flash"),
                json!("d1"),
                json!("d1-local")
            ]
        );
        assert_eq!(
            of("key"),
            [
                json!("missing"),
                json!("ok"),
                json!("ok"),
                json!("missing"),
                json!("none")
            ]
        );
        assert_eq!(
            of("local"),
            [
                json!(false),
                json!(false),
                json!(false),
                json!(false),
                json!(true)
            ]
        );
        assert_eq!(list[2]["usd_per_mtok_in"], 0.038);
        assert!(!shown.to_string().contains("CFTOKEN"), "{shown}");
    }

    /// The test sends one made-up question, as a call goes: its row in the ledger, its outcome in
    /// the answer, and nothing of the store in the request.
    #[test]
    fn the_connection_test_sends_one_made_up_question() {
        let keys = tempfile::tempdir().unwrap();
        let key_file = keys.path().join("TYPESAFE_KEY.md");
        std::fs::write(&key_file, "# a test key\nTESTKEY_for_the_stub\n").unwrap();
        let home = home(&format!(
            "[judge]\nkey_file = {:?}\n",
            key_file.display().to_string()
        ));
        let answer = json!({"model": "jev-1.14.0",
            "answers": {"english": {"type": "noul", "noul": 0.99}},
            "usage": {"input_tokens": 50, "output_tokens": 3}});
        let (url, rx) = crate::provider::tests::serve_each(
            vec![("200 OK", answer.to_string().into_bytes())],
            "",
        );
        crate::judge::AT.set(Some(url));
        let shown = test(home.path(), br#"{"confirmed": true}"#).unwrap();
        assert_eq!(shown, json!({"outcome": "ok"}));
        let sent = crate::provider::tests::request_body(&rx.recv().unwrap());
        assert_eq!(sent["questions"]["english"]["type"], "noul");
        let db = crate::providers_db::open(home.path()).unwrap();
        let (calls, _) = crate::providers_db::calls_in_a_day(&db, NAME).unwrap();
        assert_eq!(calls, 1);
    }

    /// The page's numbers: the judge's calls in 24 hours and its spend this month, its own rows
    /// only.
    #[test]
    fn the_section_counts_the_judges_rows_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::providers_db::open(dir.path()).unwrap();
        for (provider, usd) in [(NAME, 0.25), ("groq", 1.0)] {
            crate::providers_db::record(
                &db,
                &crate::providers_db::Call {
                    provider,
                    role: "judge",
                    span: "s",
                    outcome: "ok",
                    ms: 1,
                    detail: None,
                    bytes_out: 1,
                    est_tokens: None,
                    usage: Default::default(),
                    usd: Some(usd),
                },
            )
            .unwrap();
        }
        let judge = crate::config::Judge {
            enabled: false,
            destination: crate::config::Destination::Jev,
            key_file: Some(dir.path().join("none")),
            cloudflare_key_file: None,
            cloudflare_account_id: None,
            liquid_key_file: None,
            model: "jev-latest".into(),
            timeout_s: 15,
            daily_budget: 300,
        };
        let shown = section(Some(&judge), &Ok(Some(db)));
        assert_eq!(
            (
                &shown["calls_today"],
                &shown["usd_this_month"],
                &shown["enabled"],
                &shown["key"]
            ),
            (&json!(1), &json!(0.25), &json!(false), &json!("missing"))
        );
        let none = section(Some(&judge), &Ok(None));
        assert_eq!(
            (&none["calls_today"], &none["usd_this_month"]),
            (&json!(0), &json!(0.0))
        );
        // A ledger that does not read is not an unused allowance.
        let unread = section(Some(&judge), &Err(anyhow::anyhow!("unreadable")));
        assert_eq!(
            (&unread["calls_today"], &unread["usd_this_month"]),
            (&Value::Null, &Value::Null)
        );
    }
}
