//! Forget from the page (parity row 3, milestone 5 slice 5): the forget `oboete forget` runs, of
//! an item the page shows. The preview's token is the key the confirmation brings back, as the
//! CLI's start reads the preview again and refuses a stale one; the purge goes on in the
//! background, `oboete forget --continue` from the worker's round (D5 item 8). A card or a session
//! summary is no item here: it goes with its session or the prompt it came from, as
//! `forget::Target::parse_uid` says.

use std::path::Path;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Refusal, refused};
use crate::forget::{self, Target};

/// An item as the page names it: its kind and the id the page has for it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    kind: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    item: Item,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    item: Item,
    preview_key: String,
    confirmed: bool,
}

/// The longest id the page sends: a record's handle, a uid or a session's key.
const MAX_ID: usize = 512;

/// Whether `s` is `n` lowercase hex digits, as a uid (64) or a session's page key (16) is.
fn hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `item` is one the page may name, by its form alone: a known kind and an id of its form.
fn well_formed(item: &Item) -> bool {
    let id = item.id.as_str();
    id.len() <= MAX_ID
        && match item.kind.as_str() {
            "claim" => hex(id, 64),
            "note" => id
                .strip_prefix(crate::raw::NOTE_SOURCE)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| !rest.is_empty()),
            "prompt" => Target::parse_handle(id).is_ok(),
            "session" => hex(id, 16),
            _ => false,
        }
}

/// The forget target of a well-formed `item`: a session by its page key (its first prompt's record
/// names it, as `oboete forget --session` takes a record), a prompt by its record, a claim or a
/// note by its uid. None when the item is not there.
fn target(home: &Path, item: &Item) -> Result<Target, Refusal> {
    let bad = || refused(422, "forget_item", "item");
    let id = item.id.as_str();
    match item.kind.as_str() {
        "claim" | "note" => Target::parse_uid(id).map_err(|_| bad()),
        "prompt" => {
            let (device, seq) = Target::parse_handle(id).map_err(|_| bad())?;
            Ok(Target::Record { device, seq })
        }
        "session" => {
            // Opening raw.db makes one where there is none.
            if !crate::raw::exists(home) {
                return Err(bad());
            }
            let raw = crate::raw::open(home).map_err(|_| bad())?;
            let sessions = raw.live_sessions().map_err(|_| bad())?;
            let s = (sessions.into_iter())
                .find(|s| crate::view::session_key(&s.agent, &s.session) == id)
                .ok_or_else(bad)?;
            drop(raw);
            forget::session_target(home, &format!("{}:{}", s.device, s.seq)).map_err(|_| bad())
        }
        _ => Err(bad()),
    }
}

/// The refusal of a preview that failed: by name where the page can say what to do instead, as
/// a prompt only its session's forget can take.
fn preview_failed(e: &anyhow::Error) -> Refusal {
    if e.is::<forget::TooLarge>() {
        refused(422, "forget_too_large", "item")
    } else if e.is::<forget::OtherDevice>() {
        refused(422, "forget_other_device", "item")
    } else {
        refused(422, "forget_preview_failed", "")
    }
}

/// `item`'s preview, as the CLI shows it before it asks.
fn previewed(home: &Path, item: &Item) -> Result<forget::Preview, Refusal> {
    if !well_formed(item) {
        return Err(refused(422, "forget_item", "item"));
    }
    let target = target(home, item)?;
    let p = forget::preview(home, target).map_err(|e| preview_failed(&e))?;
    p.validate().map_err(|e| preview_failed(&e))?;
    Ok(p)
}

/// What forgetting the item takes, by count (the page shows the item itself), and the key its
/// start brings back: with a claim, the claims it superseded, current again once it is gone
/// (`again`), as the CLI lists them.
pub(crate) fn preview(home: &Path, body: &[u8]) -> Result<Value, Refusal> {
    let asked: Asked = serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let p = previewed(home, &asked.item)?;
    let key = p
        .token()
        .map_err(|_| refused(422, "forget_preview_failed", ""))?;
    let again = p.uid.as_ref().map_or(0, |u| u.again.len());
    Ok(json!({
        "preview_key": key, "records": p.count(), "copies": p.copies(), "claims": p.claims,
        "cards": p.cards, "summaries": p.summaries, "written": p.written, "again": again,
    }))
}

/// The forget the owner confirmed, registered only while the item's preview is the one shown:
/// a preview that changed since is stale (409), as the CLI's start refuses one. The records are
/// hidden at once; the purge goes on in the background: `cleaning` when it was started, `after`
/// when it waits for a purge that runs (one reads its jobs when it begins); with no `cleaning` the
/// worker's next look starts it. `logged` is false when a request log was not written, which the
/// CLI says too.
pub(crate) fn start(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Start =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if !well_formed(&posted.item) {
        return Err(refused(422, "forget_item", "item"));
    }
    if !posted.confirmed {
        return Err(refused(422, "forget_confirmation", "confirmed"));
    }
    if !hex(&posted.preview_key, 64) {
        return Err(refused(422, "forget_key", "preview_key"));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let stale = || refused(409, "forget_stale", "preview_key");
    let current = |item: &Item| -> Result<(forget::Preview, String), Refusal> {
        let p = previewed(home, item)?;
        let key = p
            .token()
            .map_err(|_| refused(422, "forget_preview_failed", ""))?;
        Ok((p, key))
    };
    // The item gone, nothing of it left, or a forget registered since: the preview shown is gone.
    // What only its session's forget can take is said as its preview says it.
    let (p, key) = current(&posted.item).map_err(|r| match r.code {
        "forget_too_large" | "forget_other_device" => r,
        _ => stale(),
    })?;
    if key != posted.preview_key {
        return Err(stale());
    }
    let report = match forget::start(home, &p) {
        Ok((_, report)) => report,
        // Its own check under raw's lock found the records changed after this read.
        Err(e) => {
            return Err(match current(&posted.item) {
                Ok((_, again)) if again == key => {
                    eprintln!("oboete: a forget from the page was not registered: {e:#}");
                    refused(500, "forget_failed", "")
                }
                _ => stale(),
            });
        }
    };
    for problem in &report.problems {
        eprintln!("oboete: a forget from the page is registered, not logged: {problem}");
    }
    // The CLI purges at once: so does this, in the background (`forget --continue`).
    let cleaning = forget::continue_now(home, true);
    Ok(json!({
        "started": true, "records": p.count(), "logged": report.problems.is_empty(),
        "cleaning": cleaning.is_some(), "after": cleaning == Some(true),
    }))
}

#[cfg(test)]
mod tests;
