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
    let (shown, unapplied) = match crate::search::b::stores(home).map_err(unavailable)? {
        Some((raw, k)) => (
            crate::my_work_rules::shown(&raw, &k).map_err(unavailable)?,
            crate::my_work_rules::unapplied(&raw, &k, id).map_err(unavailable)?,
        ),
        None => return Err(refused(404, "rule_not_found", "suggestion")),
    };
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
            let receipt = crate::claims::pref_add_recorded(home, text, repo);
            let (uid, kept) = match &receipt {
                Ok(OwnerReceipt::Applied { uid }) => (uid.clone(), true),
                Ok(OwnerReceipt::Pending { uid, .. }) => (uid.clone(), false),
                // No preference to name: the suggestion waits as it was.
                _ => return answer(receipt),
            };
            // The preference is the owner's now: a retry would make a second one.
            let Ok(applied) = record(json!({"action": "approve", "claim": uid})) else {
                return Ok(json!({"state": "pending", "uid": uid, "code": "rule_partly_recorded"}));
            };
            Ok(json!({"state": state(kept && applied), "uid": uid}))
        }
        (_, None) => Ok(json!({"state": state(record(json!({"action": "reject"}))?)})),
        (_, Some(d)) => {
            if let Some(uid) = &d.claim {
                match crate::claims::correct_recorded(home, uid, Some("retracted"), None) {
                    // A preference forgotten since has nothing left to retract.
                    Ok(_) => {}
                    Err(e) if matches!(e.code, OwnerRefusalCode::ClaimMissing) => {}
                    Err(e) => return answer(Err(e)),
                }
            }
            Ok(json!({"state": state(record(json!({"action": "undo"}))?)}))
        }
    }
}

#[cfg(test)]
mod tests;
