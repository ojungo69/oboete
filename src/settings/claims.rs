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
        Ok(OwnerReceipt::Replaced { uid }) => Ok(replaced(Some(&uid))),
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
///
/// #1131 items 3, 4 and 7: read and written on one raw handle, whose shared hold on raw.lock keeps
/// a restore from swapping raw.db, under config.lock (the redaction rules it gates with stay those
/// in force; the token is asked again once it is held, `authorized`) and the dispatch hold (a purge
/// rewrites ops in place, which no count of the fence shows), and written against the rule fence
/// taken before the read, as the work view writes. The append refuses it, with nothing written (409
/// `rule_moved`: the page reads again), when its raw.db is not the fence's file, checked before its
/// transaction, or, inside it, when anything `fence_moved` reads moved since the fence
/// (docs/my-work-model.md lists it): a decision, an exclusion, a forget, a tombstone, an expiry, an
/// op about the suggestion or the preference an undo retracts, and, while a repository is excluded,
/// a session's first event in one or under a label that may name one, or a 65th event read.
pub(crate) fn rule(
    home: &Path,
    saving: &Mutex<()>,
    authorized: &dyn Fn() -> bool,
    body: &[u8],
) -> Result<Value, Refusal> {
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
    // What the read would refuse as pending, waited for first within the receipts' time, the
    // worker's pass run here where this process may run it, as the work view waits (#1131 item 34):
    // a decision or a learning op the worker appended just before is applied, not refused.
    crate::work_view::caught_up(home, true).map_err(|_| refused(503, "rule_unavailable", ""))?;
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unavailable = |_| refused(503, "rule_unavailable", "");
    let not_found = || refused(404, "rule_not_found", "suggestion");
    let pending = || refused(409, "rule_pending", "suggestion");
    // What the read showed is not what the write would act on (the fence moved): nothing is
    // pending, unlike `pending`'s refusal of a decision not applied yet.
    let changed = || refused(409, "rule_moved", "suggestion");
    // A home with no store answers as it did before the locks (Codex on the plan of #1131 C1).
    if !crate::raw::exists(home) {
        return Err(not_found());
    }
    // config.lock before raw.lock, as the work view takes them.
    let wait = crate::db::OPEN_WRITE_WAIT;
    let config = crate::settings::config_lock_within(home, wait)
        .map_err(|e| unavailable(anyhow::Error::from(e)))?
        .ok_or_else(|| refused(503, "rule_unavailable", ""))?;
    if !authorized() {
        return Err(refused(401, "unauthorized", ""));
    }
    let Some((mut raw, k)) = crate::search::b::stores(home).map_err(unavailable)? else {
        return Err(not_found());
    };
    let reading = crate::dispatch::reading(home, wait).map_err(unavailable)?;
    let fence = raw.rule_fence().map_err(unavailable)?;
    #[cfg(test)]
    if let Some(hook) = FENCED.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    // One read of knowledge.db (#1083's R2b): a decision the my_work consumer applies between two
    // reads would read as neither shown nor waiting, and a retry would approve again.
    let read = (k.unchecked_transaction().map_err(anyhow::Error::from)).map_err(unavailable)?;
    let shown = crate::my_work_rules::shown(&raw, &read).map_err(unavailable)?;
    #[cfg(test)]
    if let Some(hook) = BETWEEN_READS.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let unapplied = crate::my_work_rules::unapplied(&raw, &read, id).map_err(unavailable)?;
    drop(read);
    let Some(suggestion) = shown.suggestions.iter().find(|sg| sg.id == id) else {
        return Err(not_found());
    };
    let decision = shown.decided.get(id);
    let write = match (posted.action.as_str(), decision) {
        // #739: a decision recorded but not applied yet is waited for, not made again.
        _ if unapplied => return Err(pending()),
        ("approve" | "reject", Some(_)) => return Err(refused(409, "rule_decided", "suggestion")),
        ("undo", None) => return Err(refused(409, "rule_not_decided", "suggestion")),
        ("approve", None) => {
            Write::Approve(posted.statement.as_deref().unwrap_or(&suggestion.statement))
        }
        (_, None) => Write::Decision(json!({"action": "reject"})),
        (_, Some(d)) => match &d.claim {
            Some(uid) => Write::Undo(uid, d.seq),
            None => Write::Decision(json!({"action": "undo"})),
        },
    };
    // The settings once the read's refusals are made, as each action answered a failed load before
    // (Codex on the plan of #1131 C1): an approval and an approval's undo as a claim's, the rest as
    // a rule's.
    let settings = match crate::capture::Settings::load(home) {
        Ok(settings) => settings,
        Err(e) => {
            return match write {
                Write::Decision(_) => Err(unavailable(e)),
                Write::Approve(_) | Write::Undo(..) => answer(Err(OwnerRefusal::unavailable(e))),
            };
        }
    };
    // What the write's window is checked against: the repositories excluded now, who was excluded
    // at the fence already, and the test of a label first recorded after this read.
    let (repos, label) = raw.exclusions_read().map_err(unavailable)?;
    let excluded = raw
        .excluded_through(&repos, fence.rows)
        .map_err(unavailable)?;
    let sources = crate::raw::Sources {
        repos,
        excluded,
        label,
    };
    let subject = crate::raw::Subject {
        uid: match write {
            Write::Undo(uid, _) => Some(uid),
            Write::Approve(_) | Write::Decision(_) => None,
        },
        suggestion: Some(id),
        sources: Some(&sources),
    };
    let ruled = (&fence, &subject);
    let device = raw.device().to_owned();
    let rule = crate::my_work_rules::RULE;
    let moved = |e: &anyhow::Error| e.is::<crate::raw::OpsMoved>();
    let wrote = match write {
        Write::Approve(text) => {
            let checked = crate::claims::directive_text(&settings, text, repo).and_then(|c| {
                crate::claims::repo_checked(&raw, &k, &settings, &c.0, repo).map(|()| c)
            });
            let (c, quote) = match checked {
                Ok(checked) => checked,
                Err(e) => return answer(Err(e)),
            };
            checked_hook();
            // The directive, its preference and the decision in one transaction (#1083's R2b):
            // recorded whole or not at all, so a retry never makes a second preference.
            match crate::claims::approve_append(&mut raw, &settings, (&c, &quote), id, Some(ruled))
            {
                Ok(approved) => Wrote::Approval(approved),
                Err(e) if moved(&e.cause) => return Err(changed()),
                Err(e) => return answer(Err(e)),
            }
        }
        Write::Decision(fields) => {
            checked_hook();
            raw.gates_with(&settings.rules);
            let written = fields.clone();
            let appended = crate::my_work_ops::bodies(rule, vec![(id.to_owned(), fields)])
                .and_then(|ops| raw.append_ops_ruled(&ops, &fence, &subject));
            match appended {
                Ok(seqs) => Wrote::Decision(seqs, written),
                Err(e) if moved(&e) => return Err(changed()),
                Err(e) => return Err(unavailable(e)),
            }
        }
        // Its preference retracted and the decision undone in one append (#1083's R2b).
        Write::Undo(uid, seq) => {
            let ops = match crate::claims::undo_ops(&raw, &k, id, uid) {
                Ok(ops) => ops,
                Err(e) => return answer(Err(OwnerRefusal::unavailable(e))),
            };
            checked_hook();
            match crate::claims::undo_append(&mut raw, &settings.rules, (id, uid, seq), ops, ruled)
            {
                Ok(Some(seqs)) => Wrote::Undo(uid, seqs),
                Ok(None) => return Err(pending()),
                Err(e) if moved(&e.cause) => return Err(changed()),
                Err(e) => return answer(Err(e)),
            }
        }
    };
    // The receipts' waits, which can run the worker, hold none of the store's locks: config.lock,
    // the dispatch hold and raw.db. The page's own save lock stays held to the end, as before.
    drop((reading, raw, config));
    // #1131 item 2: each receipt is applied only while its decision is the suggestion's decision
    // now; one a later decision replaced answers `replaced`, which the page says and reads again.
    let decided = |d: crate::claims::Decided| match d {
        crate::claims::Decided::Applied => json!({"state": "applied"}),
        crate::claims::Decided::Pending => json!({"state": "pending"}),
        crate::claims::Decided::Replaced => replaced(None),
    };
    match wrote {
        Wrote::Approval(approved) => {
            match crate::claims::approve_receipt(home, &k, &device, id, &approved) {
                OwnerReceipt::Applied { uid } => Ok(json!({"state": "applied", "uid": uid})),
                OwnerReceipt::Pending { uid, .. } => Ok(json!({"state": "pending", "uid": uid})),
                receipt => answer(Ok(receipt)),
            }
        }
        Wrote::Decision(seqs, fields) => Ok(match seqs.last() {
            Some(&seq) => decided(crate::claims::decision_receipt(
                home,
                &k,
                &device,
                (id, seq),
                &fields,
            )),
            None => json!({"state": "pending"}),
        }),
        Wrote::Undo(uid, seqs) => Ok(decided(crate::claims::undo_receipt(
            home,
            &k,
            &device,
            (id, uid),
            &seqs,
        ))),
    }
}

/// #1131 item 2: the answer of a rule action whose decision is saved, while a later decision of its
/// suggestion is the one now: an approval's with its preference's uid.
fn replaced(uid: Option<&str>) -> Value {
    let mut answer = json!({"state": "replaced", "code": "my_rules_replaced"});
    if let Some(uid) = uid {
        answer["uid"] = json!(uid);
    }
    answer
}

/// What a rule action writes, from its read.
enum Write<'a> {
    /// An approval of the suggestion, with its text.
    Approve(&'a str),
    /// A rejection, or the undo of one: a decision alone.
    Decision(Value),
    /// The undo of an approval whose preference is `uid`, by its rule op seq.
    Undo(&'a str, i64),
}

/// What a rule action wrote, for its receipt: a decision's op seqs with its fields as written.
enum Wrote<'a> {
    Approval(crate::claims::Approved),
    Decision(Vec<i64>, Value),
    Undo(&'a str, Vec<i64>),
}

/// The `CHECKED` hook of a test: once a rule action read and prepared what it writes.
fn checked_hook() {
    #[cfg(test)]
    if let Some(hook) = CHECKED.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

#[cfg(test)]
thread_local! {
    /// A test's hook, run once a rule action took its fence, before its first read (#1131 item 38).
    pub(crate) static FENCED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// A test's hook, run between a rule action's read of what My Rules shows and of what is not
    /// applied yet.
    pub(crate) static BETWEEN_READS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// A test's hook, run once a rule action has read and prepared what it writes, before its
    /// write, its locks held (#1131 items 3, 4 and 7).
    pub(crate) static CHECKED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests;
