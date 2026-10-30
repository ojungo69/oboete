//! The judge on the settings page (docs/my-work-model.md J1, slice 11a-2): its state, its key typed
//! on the page, and one connection test of a made-up question. Answers carry codes, never the key.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Refusal, invalid, put, refused};
use crate::judge::{Asked, NAME};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Test {
    confirmed: bool,
}

/// The judge as the page shows it: `set` when `[judge]` is there (a key was given), its switch,
/// whether its key file has a key, and its calls in the last 24 hours and spend this month.
/// A ledger that does not read gives no numbers (null), as the page's other totals.
pub(crate) fn section(
    judge: Option<&crate::config::Judge>,
    ledger: &anyhow::Result<Option<Connection>>,
) -> Value {
    let (calls, usd) = match ledger {
        Ok(Some(db)) => (
            crate::providers_db::calls_in_a_day(db, NAME)
                .ok()
                .map(|(calls, _)| calls),
            crate::providers_db::usd_this_month_of(db, NAME).ok(),
        ),
        Ok(None) => (Some(0), Some(0.0)),
        Err(_) => (None, None),
    };
    let key = judge.is_some_and(|j| crate::config::read_key(&j.key_file).is_ok());
    json!({
        "set": judge.is_some(),
        "enabled": judge.is_some_and(|j| j.enabled),
        "key": if key { "ok" } else { "missing" },
        "key_file": judge.map(|j| j.key_file.display().to_string()),
        "daily_budget": judge.map(|j| j.daily_budget),
        "calls_today": calls,
        "usd_this_month": usd,
    })
}

/// `POST /api/judge/key {"key", "version"}`: the key typed on the page, written as a provider's is
/// (`keyfile::managed`: a new owner-only file outside the store), `[judge] key_file` pointed at it
/// and the judge turned on, against the version the page showed. The old file is left as it is.
/// `authorized` is the request's token checked again under config.lock.
pub(crate) fn save_key(
    home: &Path,
    saving: &Mutex<()>,
    authorized: &dyn Fn() -> bool,
    body: &[u8],
) -> Result<Value, Refusal> {
    super::managed_key(home, (saving, authorized), body, None, "judge", turned_on)
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
    Ok(json!({"outcome": match asked {
        Asked::Answered(_) => "ok",
        Asked::Skipped(why) => why,
    }}))
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
            super::super::managed_key(
                home.path(),
                (&Mutex::new(()), &|| ok),
                body,
                homes,
                "judge",
                turned_on,
            )
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
        assert_eq!(crate::config::read_key(&judge.key_file).unwrap(), key);
        assert!(
            !judge.key_file.starts_with(home.path()),
            "outside the store"
        );
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
            key_file: dir.path().join("none"),
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
