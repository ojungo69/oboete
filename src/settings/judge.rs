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
/// `ok`, or why there is none (`judge::Asked::Skipped`), or `refused` with the reason d1 on this
/// PC's check gave (#791).
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
    Ok(match asked {
        Asked::Answered(_) => json!({"outcome": "ok"}),
        // #791: nothing went, and why: said apart from a request that had no good answer.
        Asked::Refused(why) => json!({"outcome": "refused", "reason": why}),
        Asked::Skipped(why) => json!({ "outcome": why }),
    })
}

#[cfg(test)]
mod tests;
