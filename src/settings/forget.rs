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
/// hidden at once; the purge goes on in the background. `cleaning` is false when a purge already
/// runs, after which this forget's purge starts; `logged` is false when a request log was not
/// written, which the CLI says too.
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
    let cleaning = forget::continue_now(home);
    Ok(
        json!({"started": true, "records": p.count(), "logged": report.problems.is_empty(),
        "cleaning": cleaning}),
    )
}

#[cfg(test)]
mod tests {
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
            json!({"started": true, "records": 1, "logged": true, "cleaning": true})
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

    /// A start while a purge runs says its purge waits for that one, and a start whose request
    /// log could not be written says so, as the CLI does; both are registered.
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
            (&started["cleaning"], &started["logged"]),
            (&json!(false), &json!(true))
        );
        assert_eq!(forget::CONTINUED.get(), continued);
        drop(running);
        std::fs::write(home.join("config.toml"), "[backup]\ndir = 42\n").unwrap();
        let started = go(&second);
        assert_eq!(
            (&started["cleaning"], &started["logged"]),
            (&json!(true), &json!(false))
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
}
