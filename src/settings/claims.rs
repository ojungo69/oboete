//! Typed owner-claim actions for the guarded local viewer.

use std::path::Path;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Refusal, refused};
use crate::claims::{OwnerReceipt, OwnerRefusal, OwnerRefusalCode, PendingReason};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Correction {
    uid: String,
    status: Option<String>,
    body: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mute {
    uid: String,
    muted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    text: String,
    apply_to_all_repos: bool,
    /// Slice 8b: one repository's (a key as records name it), with `apply_to_all_repos` false.
    repo: Option<String>,
}

fn full_uid(uid: &str) -> Result<(), Refusal> {
    if uid.len() == 64
        && uid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(refused(422, "claim_uid", "uid"))
    }
}

fn answer(result: Result<OwnerReceipt, OwnerRefusal>) -> Result<Value, Refusal> {
    match result {
        Ok(OwnerReceipt::Applied { uid }) => Ok(json!({"state": "applied", "uid": uid})),
        Ok(OwnerReceipt::Pending { uid, reason, .. }) => {
            let code = match reason {
                PendingReason::Application => "claim_pending",
                PendingReason::NotKept => "claim_not_applied",
            };
            Ok(json!({"state": "pending", "uid": uid, "code": code}))
        }
        Ok(OwnerReceipt::DirectiveOnly { .. }) => {
            Ok(json!({"state": "directive_only", "code": "preference_partly_recorded"}))
        }
        Err(e) => Err(match e.code {
            OwnerRefusalCode::ClaimMissing => refused(404, "claim_not_found", "uid"),
            OwnerRefusalCode::CorrectionInvalid => refused(422, "claim_invalid", ""),
            OwnerRefusalCode::Unavailable => refused(503, "claim_unavailable", ""),
            OwnerRefusalCode::PreferenceEmpty => refused(422, "preference_empty", "text"),
            OwnerRefusalCode::PreferenceTooLong => refused(422, "preference_too_long", "text"),
            OwnerRefusalCode::PreferenceRepoUnknown => {
                refused(422, "preference_repo_unknown", "repo")
            }
            OwnerRefusalCode::PreferenceRepoExcluded => {
                refused(422, "preference_repo_excluded", "repo")
            }
            OwnerRefusalCode::PreferenceRepoMasked => {
                refused(422, "preference_repo_masked", "repo")
            }
        }),
    }
}

pub(crate) fn correct(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Correction =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    full_uid(&posted.uid)?;
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    answer(crate::claims::correct_recorded(
        home,
        &posted.uid,
        posted.status.as_deref(),
        posted.body.as_deref(),
    ))
}

pub(crate) fn mute(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Mute = serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    full_uid(&posted.uid)?;
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    answer(crate::claims::mute_recorded(
        home,
        &posted.uid,
        posted.muted,
    ))
}

pub(crate) fn pref_add(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Preference =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    // Every repository's only when the owner confirmed it (spec 3.3), else the one named.
    if posted.apply_to_all_repos == posted.repo.is_some() {
        return Err(refused(
            422,
            "preference_confirmation",
            "apply_to_all_repos",
        ));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    answer(crate::claims::pref_add_recorded(
        home,
        &posted.text,
        posted.repo.as_deref(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    suggestion: String,
    action: String,
    statement: Option<String>,
    scope: Option<String>,
    repo: Option<String>,
}

/// V3, R3: the owner's action on a rule suggestion of My Rules: approve it as a preference of every
/// repository or, slice 8b, of one (`scope: "repo"` with its `repo`), as `pref add` records one
/// (its text the suggestion's, or the one posted), reject it, or undo either; an approve is undone
/// by retracting the preference it made.
pub(crate) fn rule(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Rule = serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let id = posted.suggestion.as_str();
    let digits = id.strip_prefix('s').unwrap_or_default();
    if id.len() > 21 || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(refused(400, "bad_request", "suggestion"));
    }
    let approve = posted.action == "approve";
    let known = ["approve", "reject", "undo"].contains(&posted.action.as_str());
    let given = posted.statement.is_some() || posted.scope.is_some() || posted.repo.is_some();
    if !known || (!approve && given) {
        return Err(refused(400, "bad_request", "action"));
    }
    // A rule of every repository, as only the viewer and `pref add` make one (spec 3.3), or of
    // the one named (slice 8b).
    let repo = match (posted.scope.as_deref(), posted.repo.as_deref()) {
        _ if !approve => None,
        (Some("global"), None) => None,
        (Some("repo"), Some(repo)) => Some(repo),
        _ => return Err(refused(422, "rule_scope", "scope")),
    };
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unavailable = |_| refused(503, "rule_unavailable", "");
    let Some((raw, k)) = crate::search::b::stores(home).map_err(unavailable)? else {
        return Err(refused(404, "rule_not_found", "suggestion"));
    };
    // One read of knowledge.db (#1083's R2b): a decision the my_work consumer applies between two
    // reads would read as neither shown nor waiting, and a retry would approve again.
    let read = (k.unchecked_transaction().map_err(anyhow::Error::from)).map_err(unavailable)?;
    let shown = crate::my_work_rules::shown(&raw, &read).map_err(unavailable)?;
    #[cfg(test)]
    if let Some(hook) = BETWEEN_READS.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let unapplied = crate::my_work_rules::unapplied(&raw, &read, id).map_err(unavailable)?;
    drop((read, raw));
    let Some(suggestion) = shown.suggestions.iter().find(|sg| sg.id == id) else {
        return Err(refused(404, "rule_not_found", "suggestion"));
    };
    let decision = shown.decided.get(id);
    let state = |applied: bool| if applied { "applied" } else { "pending" };
    let record =
        |fields: Value| crate::my_work_rules::record(home, id, fields).map_err(unavailable);
    match (posted.action.as_str(), decision) {
        // #739: a decision recorded but not applied yet is waited for, not made again.
        _ if unapplied => Err(refused(409, "rule_pending", "suggestion")),
        ("approve" | "reject", Some(_)) => Err(refused(409, "rule_decided", "suggestion")),
        ("undo", None) => Err(refused(409, "rule_not_decided", "suggestion")),
        ("approve", None) => {
            let text = posted.statement.as_deref().unwrap_or(&suggestion.statement);
            // The directive, its preference and the decision in one transaction (#1083's R2b):
            // recorded whole or not at all, so a retry never makes a second preference.
            match crate::claims::approve_recorded(home, id, text, repo) {
                Ok(OwnerReceipt::Applied { uid }) => Ok(json!({"state": "applied", "uid": uid})),
                Ok(OwnerReceipt::Pending { uid, .. }) => {
                    Ok(json!({"state": "pending", "uid": uid}))
                }
                receipt => answer(receipt),
            }
        }
        (_, None) => Ok(json!({"state": state(record(json!({"action": "reject"}))?)})),
        (_, Some(d)) => match &d.claim {
            // Its preference retracted and the decision undone in one append (#1083's R2b).
            Some(uid) => match crate::claims::undo_approval_recorded(home, id, uid, d.seq) {
                Ok(crate::claims::Undone::Recorded(applied)) => {
                    Ok(json!({"state": state(applied)}))
                }
                Ok(crate::claims::Undone::Waiting) => {
                    Err(refused(409, "rule_pending", "suggestion"))
                }
                Err(e) => answer(Err(e)),
            },
            None => Ok(json!({"state": state(record(json!({"action": "undo"}))?)})),
        },
    }
}

#[cfg(test)]
thread_local! {
    /// A test's hook, run between a rule action's read of what My Rules shows and of what is not
    /// applied yet.
    pub(crate) static BETWEEN_READS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests;
