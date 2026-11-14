use super::*;
use crate::embed::stub::Stub;
use crate::search::b::fixture::Store;
use serde_json::json;
use std::time::{Duration, Instant};

const R: &str = "github.com/x/r";
const MIN: i64 = 60_000;
const NOW: i64 = 1_000 * MIN;

fn per_prompt(s: &Store, on: bool) {
    std::fs::write(
        s.home.path().join("config.toml"),
        format!("[inject]\nper_prompt = {on}\n"),
    )
    .unwrap();
}

fn keys_built(k: &Connection) -> Vec<(String, String, String, String)> {
    k.prepare("SELECT agent, session, repo, branch FROM shortlists ORDER BY session, branch")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn key(session: &str, branch: &str) -> (String, String, String, String) {
    ("claude".into(), session.into(), R.into(), branch.into())
}

#[test]
fn a_cached_shortlist_filters_mute_and_restores_unmute() {
    let mut s = Store::new();
    let uid = s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    s.event(
        "prompt",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "Parser errors go to stderr."}),
    );
    s.run();
    per_prompt(&s, true);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    Builder::new(s.home.path())
        .run(&s.raw, &mut k, None, NOW)
        .unwrap();
    let cached = of(&k, ("claude", "live", R, "main")).unwrap().unwrap();
    assert_eq!(cached, std::slice::from_ref(&uid));
    for (muted, bodies) in [(true, vec![]), (false, vec!["Parser errors go to stderr."])] {
        crate::claims::mute(s.home.path(), &uid, muted).unwrap();
        let picked = pick(
            &s.raw,
            &k,
            &cached,
            &["Parser errors go to stderr."],
            THRESHOLD,
        )
        .unwrap();
        let actual: Vec<&str> = picked.iter().flatten().map(|c| c.body.as_str()).collect();
        assert_eq!(actual, bodies);
        let printed = report(
            s.home.path(),
            R,
            None,
            "Parser errors go to stderr.",
            THRESHOLD,
        )
        .unwrap();
        assert_eq!(
            printed,
            if muted {
                String::new()
            } else {
                format!("{uid} 1.000\n")
            }
        );
    }
}

#[test]
fn the_builder_leaves_muted_claims_out_and_unmute_refreshes_its_cache() {
    let mut s = Store::new();
    let uid = s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    s.event(
        "prompt",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "Parser errors go to stderr."}),
    );
    s.run();
    per_prompt(&s, true);
    crate::claims::mute(s.home.path(), &uid, true).unwrap();
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut builder = Builder::new(s.home.path());
    assert_eq!(
        builder.run(&s.raw, &mut k, None, NOW).unwrap(),
        Phase::Covered
    );
    assert_eq!(of(&k, ("claude", "live", R, "main")).unwrap(), Some(vec![]));
    crate::claims::mute(s.home.path(), &uid, false).unwrap();
    assert_eq!(of(&k, ("claude", "live", R, "main")).unwrap(), None);
    assert_eq!(
        builder.run(&s.raw, &mut k, None, NOW).unwrap(),
        Phase::Covered
    );
    assert_eq!(
        of(&k, ("claude", "live", R, "main")).unwrap(),
        Some(vec![uid])
    );
}

/// D9, A100: only this device's sessions with an event in the 30 minutes before now and no
/// end get a shortlist, and only while `[inject] per_prompt` is on: off, no table is made.
#[test]
fn only_live_sessions_get_a_shortlist_with_per_prompt_on() {
    let mut s = Store::new();
    let parser = s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let ask = json!({"prompt": "Where do the parser errors go?"});
    s.event("prompt", "live", (R, "main"), NOW - MIN, ask.clone());
    s.event("prompt", "idle", (R, "main"), NOW - 31 * MIN, ask.clone());
    s.event("prompt", "ended", (R, "main"), NOW - MIN, ask);
    s.event("end", "ended", (R, "main"), NOW - MIN + 1, json!({}));
    s.run();
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut b = Builder::new(s.home.path());
    assert_eq!(b.run(&s.raw, &mut k, None, NOW).unwrap(), Phase::Idle);
    assert!(!exists(&k, "table", "shortlists").unwrap());
    per_prompt(&s, true);
    assert_eq!(b.run(&s.raw, &mut k, None, NOW).unwrap(), Phase::Covered);
    assert_eq!(keys_built(&k), [key("live", "main")]);
    let live = of(&k, ("claude", "live", R, "main")).unwrap();
    assert_eq!(live, Some(vec![parser]));
    assert_eq!(of(&k, ("claude", "idle", R, "main")).unwrap(), None);
    // Nothing new: nothing built.
    assert_eq!(b.run(&s.raw, &mut k, None, NOW).unwrap(), Phase::Idle);
    // Idle past 30 minutes: its rows go.
    assert_eq!(
        b.run(&s.raw, &mut k, None, NOW + 30 * MIN).unwrap(),
        Phase::Covered
    );
    assert!(keys_built(&k).is_empty());
}

/// D9: a key is built again after a reply (each Stop) or `EVERY_EVENTS` events, not after one
/// event; a session that moves to another branch is a new key, and the old key's rows go;
/// imported records are no session's events (D6).
#[test]
fn a_stop_twenty_events_or_a_new_branch_rebuild_a_key() {
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "live",
        main,
        NOW - 10 * MIN,
        json!({"prompt": "the parser"}),
    );
    s.run();
    per_prompt(&s, true);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut b = Builder::new(s.home.path());
    let mut run = |s: &Store| {
        s.run();
        b.run(&s.raw, &mut k, None, NOW).unwrap()
    };
    assert_eq!(run(&s), Phase::Covered);
    let lexer = s.decided(R, 2 * MIN, "Lexer errors go to stderr too.", &[]);
    s.event(
        "prompt",
        "live",
        main,
        NOW - 9 * MIN,
        json!({"prompt": "the lexer"}),
    );
    assert_eq!(run(&s), Phase::Idle);
    s.event(
        "reply",
        "live",
        main,
        NOW - 9 * MIN + 1,
        json!({"assistant": "ok"}),
    );
    assert_eq!(run(&s), Phase::Covered);
    let k2 = crate::knowledge::open(s.home.path()).unwrap();
    let uids = of(&k2, ("claude", "live", R, "main")).unwrap().unwrap();
    assert!(uids.contains(&lexer));
    let tool = json!({"tool": "Bash", "input": "{\"command\": \"ls\"}", "output": ""});
    for i in 0..EVERY_EVENTS - 1 {
        s.event("tool", "live", main, NOW - 8 * MIN + i, tool.clone());
    }
    assert_eq!(run(&s), Phase::Idle);
    s.event("tool", "live", main, NOW - 7 * MIN, tool.clone());
    assert_eq!(run(&s), Phase::Covered);
    // Imported records are history, not a session's events.
    for i in 0..EVERY_EVENTS {
        let e = crate::raw::Event {
            source: "transcript".into(),
            kind: "reply".into(),
            session: "live".into(),
            repo: Some(R.into()),
            branch: Some("main".into()),
            ts: NOW - 6 * MIN + i,
            ..crate::raw::test_event(&json!({"assistant": "old"}).to_string())
        };
        s.raw.append(&e).unwrap();
    }
    assert_eq!(run(&s), Phase::Idle);
    s.event(
        "prompt",
        "live",
        (R, "feature"),
        NOW - 5 * MIN,
        json!({"prompt": "x"}),
    );
    assert_eq!(run(&s), Phase::Covered);
    assert_eq!(keys_built(&k2), [key("live", "feature")]);
}

/// MUST-M21 (D9): the failing command and the files touched are parts of their own, so a
/// lesson about the command, and a claim about a file, are shortlisted even after a long prompt
/// whose words fill the prompts' list with other claims.
#[test]
fn a_failing_command_after_a_long_prompt_finds_its_lesson() {
    let mut s = Store::new();
    let text = "Run cargo test with --features full.";
    let seq = s.said("s", R, MIN, text);
    let lesson = s.claim(seq, text, ("lesson", "decided", "user"), &[]);
    let config = s.decided(R, 2 * MIN, "The config loader lives in src/config.rs.", &[]);
    for i in 0..2 * SHORT as i64 {
        s.decided(
            R,
            3 * MIN + i,
            &format!("Rule {i:03} about the parser."),
            &[],
        );
    }
    let main = (R, "main");
    // Past `search::trigrams`' 64 on its own: a query of the joined parts would hold the
    // prompt's pieces only.
    let words: Vec<String> = (0..60).map(|i| format!("z{i:02}q")).collect();
    let long = format!("Look at the parser rules. {}", words.join(" "));
    s.event(
        "prompt",
        "live",
        main,
        NOW - 3 * MIN,
        json!({"prompt": long}),
    );
    let edit = json!({"tool": "Edit", "input": "{\"file_path\": \"src/config.rs\"}",
            "output": ""});
    s.event("tool", "live", main, NOW - 2 * MIN, edit);
    let run = json!({"tool": "Bash", "input": "{\"command\": \"cargo test --features full\"}",
            "output": "error", "failed": true});
    s.event("tool", "live", main, NOW - MIN, run);
    s.run();
    per_prompt(&s, true);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    assert_eq!(
        Builder::new(s.home.path())
            .run(&s.raw, &mut k, None, NOW)
            .unwrap(),
        Phase::Covered
    );
    let uids = of(&k, ("claude", "live", R, "main")).unwrap().unwrap();
    assert_eq!(uids.len(), SHORT);
    assert!(uids.contains(&lesson) && uids.contains(&config));
}

/// A success clears only its own session's failing command.
#[test]
fn another_sessions_success_does_not_hide_a_failure() {
    let mut s = Store::new();
    let lesson = s.decided(R, MIN, "Run cargo test with --features full.", &[]);
    let input = "{\"command\":\"cargo test --features full\"}";
    s.event(
        "tool",
        "failing",
        (R, "main"),
        NOW - 2 * MIN,
        json!({"tool": "Bash", "input": input, "output": "error", "failed": true}),
    );
    s.event(
        "tool",
        "passing",
        (R, "main"),
        NOW - MIN,
        json!({"tool": "Bash", "input": input, "output": "ok"}),
    );
    s.run();
    per_prompt(&s, true);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    Builder::new(s.home.path())
        .run(&s.raw, &mut k, None, NOW)
        .unwrap();
    assert_eq!(
        of(&k, ("claude", "failing", R, "main")).unwrap(),
        Some(vec![lesson])
    );
    assert_eq!(
        of(&k, ("claude", "passing", R, "main")).unwrap(),
        Some(Vec::new())
    );
}

/// D8, D9: the search runs outside every transaction (it asserts so), and a write that fails
/// part way keeps the rows the last build wrote, and its build, which the next call makes
/// again.
#[test]
fn the_phase_searches_outside_a_transaction() {
    let mut s = Store::new();
    let parser = s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "live",
        main,
        NOW - MIN,
        json!({"prompt": "the parser"}),
    );
    s.run();
    per_prompt(&s, true);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let inside = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _tx = k.unchecked_transaction().unwrap();
        crate::search::b::delivered_ranked(&s.raw, &k, &["parser"], None, R, SHORT)
    }));
    assert!(inside.is_err(), "a search inside a transaction is a bug");
    let mut b = Builder::new(s.home.path());
    assert_eq!(b.run(&s.raw, &mut k, None, NOW).unwrap(), Phase::Covered);
    s.decided(R, 2 * MIN, "Parser warnings go to stderr.", &[]);
    s.event(
        "reply",
        "live",
        main,
        NOW - MIN + 1,
        json!({"assistant": "ok"}),
    );
    s.run();
    // The write fails part way: nothing of it lands.
    k.execute_batch(
        "CREATE TEMP TRIGGER no_rank BEFORE INSERT ON main.shortlist
             BEGIN SELECT RAISE(ABORT, 'a failed write'); END;",
    )
    .unwrap();
    assert!(b.run(&s.raw, &mut k, None, NOW).is_err());
    k.execute_batch("DROP TRIGGER no_rank").unwrap();
    assert_eq!(
        of(&k, ("claude", "live", R, "main")).unwrap(),
        Some(vec![parser.clone()])
    );
    assert_eq!(b.run(&s.raw, &mut k, None, NOW).unwrap(), Phase::Covered);
    let uids = of(&k, ("claude", "live", R, "main")).unwrap().unwrap();
    assert_eq!(uids.len(), 2);
}

/// `per_prompt` on, with `stub` as the embedder, and what is stored embedded.
fn embedded(s: &Store, stub: &Stub) {
    crate::embed_phase::fixture::config(s, stub);
    let path = s.home.path().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap() + "[inject]\nper_prompt = true\n";
    std::fs::write(path, text).unwrap();
    crate::embed_phase::fixture::embed_all(s);
}

/// The query embeddings counted in providers.db.
fn queries(s: &Store) -> i64 {
    crate::providers_db::open(s.home.path())
        .unwrap()
        .query_row(
            "SELECT count(*) FROM provider_calls WHERE role = 'query'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn wait(until: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !until() {
        assert!(Instant::now() < deadline, "waited 20 s");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// D9: a key built from full text asks for its query vector, never waiting for it, and is
/// built again with it when it comes back for the text the key still has; a key asks at most
/// once every `VECTOR_EVERY`, and never for a text it keeps a vector for.
#[test]
fn one_query_vector_per_key_every_fifteen_minutes() {
    let stub = Stub::start();
    let mut s = Store::new();
    let parser = s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    // Words of two letters: no full-text list finds it (trigrams), the vector list does.
    let near = s.decided(R, MIN, "Db ok.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "live",
        main,
        NOW - 2 * MIN,
        json!({"prompt": "parser db ok"}),
    );
    s.run();
    embedded(&s, &stub);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let mut b = Builder::new(s.home.path());
    let mut run = |s: &Store, k: &mut Connection, phase: &mut _, now| {
        s.run();
        b.run(&s.raw, k, Some(phase), now).unwrap()
    };
    let shortlist = |k: &Connection| of(k, ("claude", "live", R, "main")).unwrap().unwrap();
    let sent = stub.requests();
    assert_eq!(run(&s, &mut k, &mut phase, NOW), Phase::Covered);
    let uids = shortlist(&k);
    assert!(uids.contains(&parser) && !uids.contains(&near));
    assert_eq!(queries(&s), 1);
    wait(|| phase.done());
    assert_eq!(stub.texts()[sent..], [["parser db ok"]]);
    // The embedding phase settles the answer; the next call builds the key with it.
    phase.poll(&s.raw, &k).unwrap();
    assert_eq!(run(&s, &mut k, &mut phase, NOW), Phase::Covered);
    let uids = shortlist(&k);
    assert!(uids.contains(&parser) && uids.contains(&near));
    // A reply within 15 minutes: built again, nothing asked.
    s.event("reply", "live", main, NOW - MIN, json!({"assistant": "ok"}));
    assert_eq!(run(&s, &mut k, &mut phase, NOW + MIN), Phase::Covered);
    assert!(shortlist(&k).contains(&near));
    assert_eq!(queries(&s), 1);
    // 15 minutes on, the same text: the vector kept for it serves, nothing asked.
    s.event(
        "reply",
        "live",
        main,
        NOW + 15 * MIN,
        json!({"assistant": "ok"}),
    );
    assert_eq!(run(&s, &mut k, &mut phase, NOW + 16 * MIN), Phase::Covered);
    assert!(shortlist(&k).contains(&near));
    assert_eq!(queries(&s), 1);
    // A new prompt changes the text the next reply builds: asked.
    s.event(
        "prompt",
        "live",
        main,
        NOW + 16 * MIN,
        json!({"prompt": "lexer db ok"}),
    );
    s.event(
        "reply",
        "live",
        main,
        NOW + 16 * MIN + 1,
        json!({"assistant": "ok"}),
    );
    assert_eq!(run(&s, &mut k, &mut phase, NOW + 17 * MIN), Phase::Covered);
    assert_eq!(queries(&s), 2);
}

/// D1 holds only new asks while a replayed native request is ahead of consumers.
#[test]
fn a_native_forget_holds_new_query_asks_until_the_consumers_drain() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    s.event(
        "prompt",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "parser errors"}),
    );
    let mut event = crate::raw::test_event(r#"{"prompt":"native-policy-canary-902"}"#);
    event.source = "transcript".into();
    event.session = "native-history".into();
    let identity = crate::raw::ImportIdentity {
        origin: crate::forget::origin("synthetic-dispatch", "policy-902"),
        session: crate::forget::session(&event.agent, &event.session),
        ambiguous: None,
        unverified: false,
    };
    let seq = s
        .raw
        .append_imported_origins(
            &[crate::capture::Captured {
                event,
                ledger: Vec::new(),
            }],
            std::slice::from_ref(&identity),
            "",
            None,
        )
        .unwrap()[0];
    s.run();
    embedded(&s, &stub);
    let home = s.home.path();
    let request = crate::forget::previous_transcript_request(&s.raw, seq, &identity);
    let mut replay = crate::raw::open(home).unwrap();
    assert_eq!(replay.forget_apply(&[request]).unwrap(), 1);
    crate::forget::reconcile(home, &mut replay).unwrap();
    let mut k = crate::knowledge::open(home).unwrap();
    assert!(crate::curate::lagging(&s.raw, &k).unwrap());
    let mut phase = crate::embed_phase::Phase::new(home);
    phase.poll(&s.raw, &k).unwrap();
    let mut builder = Builder::new(home);
    builder.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap();
    assert_eq!(
        queries(&s),
        0,
        "a new ask bypassed the actual native forget lag"
    );
    assert!(!phase.busy());
    s.run();
    assert!(!crate::curate::lagging(&s.raw, &k).unwrap());
    builder.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap();
    assert_eq!(
        queries(&s),
        1,
        "draining the tombstone did not resume the query"
    );
    wait(|| phase.done());
    phase.poll(&s.raw, &k).unwrap();
}

/// An unchanged query keeps its semantic candidates across replies and a worker restart.
#[test]
fn an_unchanged_query_keeps_its_vector_after_a_reply_and_restart() {
    let mut s = Store::new();
    let parser = s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let near = s.decided(R, MIN, "Db ok.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "live",
        main,
        NOW - 2 * MIN,
        json!({"prompt": "parser db ok"}),
    );
    s.run();
    per_prompt(&s, true);
    crate::embed_phase::fixture::vectors(&s);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut b = Builder::new(s.home.path());
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    b.run(&s.raw, &mut k, None, NOW).unwrap();
    let shortlist = |k: &Connection| of(k, ("claude", "live", R, "main")).unwrap().unwrap();
    assert_eq!(shortlist(&k), [parser]);
    k.execute(
        "INSERT INTO shortlist_asks VALUES('claude', 'live', ?1)",
        [NOW],
    )
    .unwrap();
    crate::embed_phase::fixture::answer(
        &mut phase,
        "claude\0live\0github.com/x/r\0main",
        "parser db ok",
    );
    b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap();
    assert!(shortlist(&k).contains(&near));
    s.event("reply", "live", main, NOW - MIN, json!({"assistant": "ok"}));
    s.run();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW + MIN).unwrap();
    assert!(
        shortlist(&k).contains(&near),
        "the unchanged reply lost its semantic candidate"
    );
    drop((b, phase, k));
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut b = Builder::new(s.home.path());
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    s.event("reply", "live", main, NOW, json!({"assistant": "ok"}));
    s.run();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW + 2 * MIN)
        .unwrap();
    assert!(
        shortlist(&k).contains(&near),
        "the restart lost its semantic candidate"
    );
    assert_eq!(
        asked_at(&k, &keys(&s.raw, &k, NOW).unwrap()[0]).unwrap(),
        NOW
    );
    k.execute(
        "UPDATE vec_generation SET embedder = 'other' WHERE state = 'active'",
        [],
    )
    .unwrap();
    s.event("reply", "live", main, NOW + 1, json!({"assistant": "ok"}));
    s.run();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW + 2 * MIN)
        .unwrap();
    assert!(!shortlist(&k).contains(&near));
    k.execute(
        "UPDATE vec_generation SET embedder = ?1 WHERE state = 'active'",
        [crate::embed::EMBEDDER],
    )
    .unwrap();
    s.event("reply", "live", main, NOW + 2, json!({"assistant": "ok"}));
    s.run();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW + 2 * MIN)
        .unwrap();
    assert!(shortlist(&k).contains(&near));
    // The same text from an excluded session cannot reuse the cached vector.
    s.exclude("github.com/x/secret");
    s.event(
        "tool",
        "live",
        ("github.com/x/secret", "main"),
        NOW + 3,
        json!({}),
    );
    s.event("reply", "live", main, NOW + 4, json!({"assistant": "ok"}));
    s.run();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW + 3 * MIN)
        .unwrap();
    assert!(!shortlist(&k).contains(&near));
    s.raw.exclude("github.com/x/secret", true).unwrap();
    s.event(
        "prompt",
        "live",
        main,
        NOW + 5,
        json!({"prompt": "the lexer"}),
    );
    s.event("reply", "live", main, NOW + 6, json!({"assistant": "ok"}));
    s.run();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW + 4 * MIN)
        .unwrap();
    assert!(!shortlist(&k).contains(&near));
}

/// A full-text build while another key's query waits does not consume query eligibility.
#[test]
fn a_second_session_asks_after_the_first_query_is_released() {
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let near = s.decided(R, MIN, "Db ok.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "second",
        main,
        NOW - 2 * MIN,
        json!({"prompt": "parser db ok"}),
    );
    s.event(
        "prompt",
        "first",
        main,
        NOW - MIN,
        json!({"prompt": "parser db ok"}),
    );
    s.run();
    crate::embed_phase::fixture::config_at(&s, "http://127.0.0.1:1/run/bge-m3");
    let path = s.home.path().join("config.toml");
    std::fs::write(
        &path,
        std::fs::read_to_string(&path).unwrap() + "[inject]\nper_prompt = true\n",
    )
    .unwrap();
    crate::embed_phase::fixture::vectors(&s);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let mut b = Builder::new(s.home.path());
    let sent = crate::embed_phase::Sent::Vectors(vec![crate::embed::stub::vector(
        crate::embed::EMBEDDER,
        "parser db ok",
    )]);
    let release = crate::embed_phase::fixture::hold_query(
        &mut phase,
        "claude\0first\0github.com/x/r\0main",
        "parser db ok",
        sent,
    );
    b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap();
    k.execute(
        "INSERT INTO shortlist_asks VALUES('claude', 'first', ?1)",
        [NOW],
    )
    .unwrap();
    assert_eq!(
        keys_built(&k),
        [key("first", "main"), key("second", "main")]
    );
    assert_eq!(queries(&s), 1);
    assert!(
        !of(&k, ("claude", "second", R, "main"))
            .unwrap()
            .unwrap()
            .contains(&near)
    );
    release.send(()).unwrap();
    wait(|| phase.done());
    phase.poll(&s.raw, &k).unwrap();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap();
    assert_eq!(queries(&s), 2, "the second key never retried its query");
    assert_eq!(
        asked_at(&k, &keys(&s.raw, &k, NOW).unwrap()[1]).unwrap(),
        NOW
    );
}

/// A send rejected before egress is retried under current rules without consuming cadence.
#[test]
fn an_unsent_query_is_recomposed_without_waiting_fifteen_minutes() {
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    s.event(
        "prompt",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "opaque_canary parser"}),
    );
    s.run();
    crate::embed_phase::fixture::config_at(&s, "http://127.0.0.1:1/run/bge-m3");
    let path = s.home.path().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap() + "[inject]\nper_prompt = true\n";
    std::fs::write(&path, &config).unwrap();
    crate::embed_phase::fixture::vectors(&s);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut b = Builder::new(s.home.path());
    b.run(&s.raw, &mut k, None, NOW).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let sent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("the redaction rules changed"));
    let release = crate::embed_phase::fixture::hold_query(
        &mut phase,
        "claude\0live\0github.com/x/r\0main",
        "opaque_canary parser",
        sent,
    );
    k.execute(
        "INSERT INTO shortlist_asks VALUES('claude', 'live', ?1)",
        [NOW],
    )
    .unwrap();
    let rule = "[redaction]\nextra_rules = [{ id = \"canary\", regex = 'opaque_canary' }]\n";
    std::fs::write(&path, config + rule).unwrap();
    release.send(()).unwrap();
    wait(|| phase.done());
    phase.poll(&s.raw, &k).unwrap();
    b.run(&s.raw, &mut k, Some(&mut phase), NOW + 1).unwrap();
    assert_eq!(queries(&s), 1, "the unsent query left its cooldown behind");
    let text = crate::embed_phase::fixture::query_text(&phase).unwrap();
    assert!(!text.contains("opaque_canary"), "{text}");
    assert!(text.ends_with(" parser"), "{text}");
}

/// A query refused at its send releases its cooldown even when the rules then do not load:
/// a worker that stops there keeps nothing in memory to release it later.
#[test]
fn an_unsent_query_releases_its_cooldown_while_the_rules_do_not_load() {
    let mut s = Store::new();
    s.event(
        "prompt",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "parser"}),
    );
    s.run();
    per_prompt(&s, true);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut b = Builder::new(s.home.path());
    b.run(&s.raw, &mut k, None, NOW).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let sent = crate::embed_phase::Sent::Unsent(anyhow::anyhow!("the rules changed"));
    let release = crate::embed_phase::fixture::hold_query(
        &mut phase,
        "claude\0live\0github.com/x/r\0main",
        "parser",
        sent,
    );
    k.execute(
        "INSERT INTO shortlist_asks VALUES('claude', 'live', ?1)",
        [NOW],
    )
    .unwrap();
    release.send(()).unwrap();
    wait(|| phase.done());
    phase.poll(&s.raw, &k).unwrap();
    std::fs::write(
        s.home.path().join("config.toml"),
        "[inject]\nper_prompt = true\n[redaction]\nextra_rules = [{ id = \"bad\", regex = '(' }]\n",
    )
    .unwrap();
    let phase_now = b.run(&s.raw, &mut k, Some(&mut phase), NOW + 1).unwrap();
    assert_eq!(phase_now, Phase::Idle);
    let asks: i64 = k
        .query_row("SELECT count(*) FROM shortlist_asks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(asks, 0);
}

/// Rows 30-1 and 30-2, spec 5.5 (D9): a session with an event in an excluded repository asks
/// for no query vector, whatever checkout its key is on; another session's words are sent,
/// gated: never its token or a `<private>` block.
#[test]
fn an_excluded_session_sends_no_query() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let token = ["gh", "p_q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"].concat();
    let open = format!("the parser {token} <private>acme plan</private> again");
    s.event(
        "prompt",
        "open",
        (R, "main"),
        NOW - 3 * MIN,
        json!({"prompt": open}),
    );
    // Newer, so it would be asked first, and alone, were its exclusion missed.
    s.event(
        "prompt",
        "mixed",
        ("github.com/x/secret", "main"),
        NOW - 2 * MIN,
        json!({"prompt": "zebra words"}),
    );
    s.event(
        "prompt",
        "mixed",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "zebra parser"}),
    );
    s.exclude("github.com/x/secret");
    s.run();
    embedded(&s, &stub);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let sent = stub.requests();
    let mut b = Builder::new(s.home.path());
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Covered
    );
    assert_eq!(keys_built(&k), [key("mixed", "main"), key("open", "main")]);
    wait(|| phase.done());
    let asked = stub.texts()[sent..].concat();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].starts_with("the parser ") && asked[0].ends_with(" again"),
        "{asked:?}"
    );
    for hidden in [&token[..12], "acme", "zebra"] {
        assert!(!asked[0].contains(hidden), "{asked:?}");
    }
    assert_eq!(queries(&s), 1);
}

/// Spec 5.5 (D9): the parts are read by session, so a session of the same id that another
/// agent ran in an excluded repository excludes the key too (Codex's security review of Step
/// 4).
#[test]
fn another_agents_excluded_session_of_the_id_sends_no_query() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let codex = |kind: &str, repo: &str, ts: i64, body: Value| crate::raw::Event {
        agent: "codex".into(),
        kind: kind.into(),
        session: "s".into(),
        repo: Some(repo.into()),
        branch: Some("main".into()),
        ts,
        ..crate::raw::test_event(&body.to_string())
    };
    let secret = "github.com/x/secret";
    let words = json!({"prompt": "zebra words"});
    s.raw
        .append(&codex("prompt", secret, NOW - 3 * MIN, words.clone()))
        .unwrap();
    s.raw
        .append(&codex("prompt", R, NOW - 2 * MIN, words))
        .unwrap();
    // Newer, so the key is Claude's.
    s.event(
        "prompt",
        "s",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "the parser"}),
    );
    s.exclude(secret);
    s.run();
    embedded(&s, &stub);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let mut b = Builder::new(s.home.path());
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Covered
    );
    assert_eq!(keys_built(&k), [key("s", "main")]);
    assert_eq!(queries(&s), 0);
}

/// Each field of a part is gated alone as well as joined (`joined`): a rule anchored to the
/// failing command (`^...`) holds once its tool's name is before it (Codex's security review
/// of Step 4).
#[test]
fn each_field_of_a_query_is_gated_alone_too() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "live",
        main,
        NOW - 2 * MIN,
        json!({"prompt": "the parser"}),
    );
    let input = json!({"command": "deploy-key-123456 --push"}).to_string();
    let run = json!({"tool": "Bash", "input": input, "output": "error", "failed": true});
    s.event("tool", "live", main, NOW - MIN, run);
    s.run();
    embedded(&s, &stub);
    let path = s.home.path().join("config.toml");
    let rule = "[redaction]\nextra_rules = [{ id = \"deploy\", regex = '^deploy-key-[0-9]{6}' }]\n";
    let text = std::fs::read_to_string(&path).unwrap() + rule;
    std::fs::write(path, text).unwrap();
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let sent = stub.requests();
    let mut b = Builder::new(s.home.path());
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Covered
    );
    wait(|| phase.done());
    let asked = stub.texts()[sent..].concat();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].ends_with(" --push") && !asked[0].contains("123456"),
        "{asked:?}"
    );
}

/// Spec 6.4: a rule on the stored input still holds after the command is extracted.
#[test]
fn the_stored_failing_input_is_gated_before_command_extraction() {
    let mut s = Store::new();
    s.event(
        "tool",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"tool": "Bash", "input": "{\"command\":\"echo opaque_canary\"}",
                "output": "error", "failed": true}),
    );
    s.run();
    let path = s.home.path().join("config.toml");
    let rule = "[redaction]\nextra_rules = [{ id = \"input\", \
                    regex = '^\\{\"command\":\"echo (opaque_canary)\"\\}$', \
                    secret_group = 1 }]\n";
    std::fs::write(path, rule).unwrap();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let key = keys(&s.raw, &k, NOW).unwrap().pop().unwrap();
    let rules = crate::redact::Rules::load(s.home.path()).unwrap();
    let asked = parts(&s.raw, &k, &key, &rules).unwrap();
    assert_eq!(asked.len(), 1, "{asked:?}");
    // An input the rules change gives no command: only its tool is asked with.
    assert_eq!(asked[0].trim(), "Bash", "{asked:?}");
}

/// The query of the session's first key, with these rules written after its records.
fn asked_with(s: &Store, rules: &str) -> Vec<String> {
    std::fs::write(s.home.path().join("config.toml"), rules).unwrap();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let key = keys(&s.raw, &k, NOW).unwrap().pop().unwrap();
    let rules = crate::redact::Rules::load(s.home.path()).unwrap();
    parts(&s.raw, &k, &key, &rules).unwrap()
}

/// Spec 6.4: a rule on the stored input still holds for the files read out of it.
#[test]
fn a_rule_on_the_stored_input_keeps_its_files_out_of_the_query() {
    let mut s = Store::new();
    let main = (R, "main");
    s.event(
        "tool",
        "live",
        main,
        NOW - 2 * MIN,
        json!({"tool": "Edit", "input": "{\"file_path\":\"proprietary_canary.txt\"}"}),
    );
    s.event(
        "prompt",
        "live",
        main,
        NOW - MIN,
        json!({"prompt": "the parser"}),
    );
    s.run();
    let asked = asked_with(
        &s,
        "[redaction]\nextra_rules = [{ id = \"input\", \
             regex = '^\\{\"file_path\":\"([a-z_.]+)\"\\}$', secret_group = 1 }]\n",
    );
    assert_eq!(asked, ["the parser"]);
}

/// Spec 6.4: a rule on the input and one on its command each hold, though the first one's
/// mask would hide the second one's context.
#[test]
fn rules_on_the_input_and_on_its_command_both_hold() {
    let mut s = Store::new();
    s.event(
        "tool",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"tool": "Bash", "input": "{\"command\":\"echo opaque_canary\"}",
                "output": "error", "failed": true}),
    );
    s.run();
    let asked = asked_with(
        &s,
        "[redaction]\nextra_rules = [\
             { id = \"input\", regex = '^\\{\"command\":\"(echo) opaque_canary\"\\}$', \
               secret_group = 1 }, \
             { id = \"command\", regex = '^echo (opaque_canary)$', secret_group = 1 }]\n",
    );
    assert!(
        asked.iter().all(|p| !p.contains("opaque_canary")),
        "{asked:?}"
    );
}

/// An answer is used though `KEYS` newer keys are due with it: its key is built first.
#[test]
fn an_answer_is_kept_behind_newer_due_keys() {
    let mut s = Store::new();
    s.decided(R, MIN, "Db ok.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "old",
        main,
        NOW - 20 * MIN,
        json!({"prompt": "parser db ok"}),
    );
    for i in 0..KEYS {
        let session = format!("new{i}");
        s.event(
            "prompt",
            &session,
            main,
            NOW - MIN,
            json!({"prompt": "lexer"}),
        );
    }
    s.run();
    per_prompt(&s, true);
    crate::embed_phase::fixture::vectors(&s);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    crate::embed_phase::fixture::answer(
        &mut phase,
        "claude\0old\0github.com/x/r\0main",
        "parser db ok",
    );
    Builder::new(s.home.path())
        .run(&s.raw, &mut k, Some(&mut phase), NOW)
        .unwrap();
    let kept: i64 = k
        .query_row(
            "SELECT count(*) FROM shortlist_vectors WHERE session = 'old'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
}

/// A file fact is read through raw even before the consumer applies its tombstone.
#[test]
fn pending_file_tombstones_stay_out_of_the_query() {
    for whole in [true, false] {
        let mut s = Store::new();
        let main = (R, "main");
        s.event(
            "tool",
            "live",
            main,
            NOW - 3 * MIN,
            json!({"tool": "Edit", "input": "{\"file_path\":\"src/public.rs\"}"}),
        );
        let seq = s.event(
            "tool",
            "live",
            main,
            NOW - 2 * MIN,
            json!({"tool": "Edit", "input": "{\"file_path\":\"opaque_canary.rs\"}"}),
        );
        s.event(
            "prompt",
            "live",
            main,
            NOW - MIN,
            json!({"prompt": "the parser"}),
        );
        s.run();
        let device = s.raw.device().to_owned();
        let target = if whole {
            crate::raw::Target::Record { device, seq }
        } else {
            let e = event(&s.raw, &device, seq).unwrap().unwrap();
            crate::raw::Target::Range {
                device,
                seq,
                offset: e.body.find("opaque_canary.rs").unwrap() as i64,
                length: "opaque_canary.rs".len() as i64,
            }
        };
        s.raw.append_tombstone(target).unwrap();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let key = keys(&s.raw, &k, NOW).unwrap().pop().unwrap();
        let text = parts(&s.raw, &k, &key, &crate::redact::Rules::default())
            .unwrap()
            .join("\n");
        assert!(!text.contains("opaque_canary.rs"), "whole={whole}: {text}");
        assert!(text.contains("src/public.rs"), "whole={whole}: {text}");
    }
}

/// Redaction rules that do not load stop the phase as they stop capture: nothing is built and
/// nothing asked under the bundled rules alone (Codex's security review of Step 4).
#[test]
fn rules_that_do_not_load_build_and_ask_nothing() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    s.event(
        "prompt",
        "live",
        (R, "main"),
        NOW - MIN,
        json!({"prompt": "the parser"}),
    );
    s.run();
    embedded(&s, &stub);
    let path = s.home.path().join("config.toml");
    let good = std::fs::read_to_string(&path).unwrap();
    let wrong = "[redaction]\nextra_rules = [{ id = \"x\", regex = '(' }]\n";
    std::fs::write(&path, good.clone() + wrong).unwrap();
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let mut b = Builder::new(s.home.path());
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Idle
    );
    assert_eq!(queries(&s), 0);
    std::fs::write(&path, good).unwrap();
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Covered
    );
    assert_eq!(queries(&s), 1);
}

/// D9: a session that moves to another branch and back, or ends and resumes, asks for its
/// query vector no more often than every `VECTOR_EVERY`: the ask is kept apart from the keys,
/// whose rows go (Codex's security review of Step 4).
#[test]
fn a_session_moving_or_resumed_asks_no_sooner() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let ask = json!({"prompt": "the parser"});
    s.event("prompt", "live", (R, "main"), NOW - 3 * MIN, ask.clone());
    s.run();
    embedded(&s, &stub);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let mut b = Builder::new(s.home.path());
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Covered
    );
    assert_eq!(queries(&s), 1);
    // The ask settled, so the phase could take another.
    wait(|| phase.done());
    phase.poll(&s.raw, &k).unwrap();
    for (i, branch) in [(1, "feature"), (2, "main")] {
        s.event(
            "prompt",
            "live",
            (R, branch),
            NOW - 3 * MIN + i,
            ask.clone(),
        );
        s.run();
        assert_eq!(
            b.run(&s.raw, &mut k, Some(&mut phase), NOW + i * MIN)
                .unwrap(),
            Phase::Covered
        );
        assert_eq!(keys_built(&k), [key("live", branch)]);
        assert_eq!(queries(&s), 1, "{branch}");
    }
    s.event("end", "live", (R, "main"), NOW - 2 * MIN, json!({}));
    s.run();
    let now = NOW + 3 * MIN;
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), now).unwrap(),
        Phase::Covered
    );
    assert!(keys_built(&k).is_empty());
    s.event("prompt", "live", (R, "main"), NOW - MIN, ask);
    s.run();
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), now + MIN).unwrap(),
        Phase::Covered
    );
    assert_eq!(keys_built(&k), [key("live", "main")]);
    assert_eq!(queries(&s), 1, "resumed");
}

/// Rows 55-1 and 55-7 (D8, D9): while the embedder holds a key's query, the key is built from
/// full text and curation runs; the worker stays up for the answer and builds the key again
/// with it.
#[test]
fn a_waiting_embedder_leaves_full_text_rows() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let stub = Stub::start();
    let mut s = Store::new();
    let parser = s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let near = s.decided(R, MIN, "Db ok.", &[]);
    let now = crate::db::now_ms();
    s.event(
        "prompt",
        "live",
        (R, "main"),
        now - MIN,
        json!({"prompt": "parser db ok"}),
    );
    s.run();
    embedded(&s, &stub);
    let sent = stub.requests();
    let held = stub.hold();
    let home = s.home.path().to_owned();
    let curated = AtomicUsize::new(0);
    let shortlist = || {
        let k = crate::knowledge::open(&home).unwrap();
        of(&k, ("claude", "live", R, "main")).unwrap()
    };
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut embed = crate::embed_phase::Phase::new(&home);
            let mut builder = Builder::new(&home);
            let mut curation = |_: &mut Raw, _: &Connection| {
                curated.fetch_add(1, Ordering::SeqCst);
                Ok(Phase::Idle)
            };
            let phases = crate::worker::Phases {
                embed: Some(&mut embed),
                shortlist: Some(&mut builder),
                curation: Some(&mut curation),
                ..crate::worker::Phases::default()
            };
            let consumers = crate::worker::consumers(&home);
            crate::worker::run_holding(&home, 200, consumers, || {}, None, phases)
        });
        wait(|| stub.requests() > sent && shortlist().is_some());
        wait(|| curated.load(Ordering::SeqCst) > 0);
        let uids = shortlist().unwrap();
        assert!(uids.contains(&parser) && !uids.contains(&near));
        // Past the worker's idle wait: it stays up for the answer.
        std::thread::sleep(Duration::from_millis(1_000));
        assert!(!worker.is_finished(), "the worker left before the answer");
        drop(held);
        worker.join().unwrap().unwrap();
    });
    let uids = shortlist().unwrap();
    assert!(uids.contains(&parser) && uids.contains(&near));
    assert_eq!(queries(&s), 1);
}

/// D9: a vector that comes back for a text the key no longer has is dropped: the key keeps
/// its full-text rows until it is due.
#[test]
fn a_vector_for_an_old_text_is_dropped() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.decided(R, MIN, "Parser errors go to stderr.", &[]);
    let near = s.decided(R, MIN, "Db ok.", &[]);
    let main = (R, "main");
    s.event(
        "prompt",
        "live",
        main,
        NOW - 2 * MIN,
        json!({"prompt": "parser db ok"}),
    );
    s.run();
    embedded(&s, &stub);
    let mut k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = crate::embed_phase::Phase::new(s.home.path());
    let mut b = Builder::new(s.home.path());
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Covered
    );
    wait(|| phase.done());
    s.event(
        "prompt",
        "live",
        main,
        NOW - MIN,
        json!({"prompt": "the lexer"}),
    );
    s.run();
    phase.poll(&s.raw, &k).unwrap();
    assert_eq!(
        b.run(&s.raw, &mut k, Some(&mut phase), NOW).unwrap(),
        Phase::Idle
    );
    let uids = of(&k, ("claude", "live", R, "main")).unwrap().unwrap();
    assert!(!uids.contains(&near));
}

/// Spec 4.2 and D9: a prompt gets the shortlisted claims whose body holds the threshold's share
/// of its trigrams, in 3 places, newest first: an earlier decision only after the later one
/// that ended it (D2, #295 row 2), and none an owner's change hides, applied or not (D3).
#[test]
fn pick_takes_whole_units_over_the_threshold_in_three_places() {
    let mut s = Store::new();
    let stdout = s.decided(R, MIN, "Parser errors go to stdout.", &[]);
    let stderr = s.decided(
        R,
        2 * MIN,
        "Parser errors go to stderr, not stdout.",
        &[&stdout],
    );
    let lexer = s.decided(R, 3 * MIN, "Lexer tokens are cached.", &[]);
    let warnings = s.decided(R, 4 * MIN, "Parser warnings go to the log.", &[]);
    let retracted = s.decided(R, 5 * MIN, "Parser errors are fatal.", &[]);
    let pending = s.decided(R, 6 * MIN, "Parser errors carry their line.", &[]);
    // Delivered, but not decided: no injection (D9).
    let text = "Parser errors may go to a file.";
    let seq = s.said("s", R, 7 * MIN, text);
    let proposal = s.claim(seq, text, ("decision", "proposed", "user"), &[]);
    s.run();
    s.correct(&retracted, Some("retracted"), None);
    s.run();
    // Appended, not applied yet.
    s.correct(&pending, Some("retracted"), None);
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let prompt = ["where do the parser errors go"];
    let all = [&proposal, &stdout, &lexer, &warnings, &retracted, &pending]
        .map(|u| u.to_string())
        .to_vec();
    let uids = |units: Vec<Vec<crate::claims::Claim>>| -> Vec<Vec<String>> {
        units
            .into_iter()
            .map(|u| u.into_iter().map(|c| c.uid).collect())
            .collect()
    };
    let picked = uids(pick(&s.raw, &k, &all, &prompt, THRESHOLD).unwrap());
    assert!(picked == [vec![stderr.clone(), stdout.clone()]]);
    // At 0 every delivered one passes: the pair and one more fill the 3 places.
    let picked = uids(pick(&s.raw, &k, &all, &prompt, 0.0).unwrap());
    assert!(picked == [vec![lexer.clone()], vec![stderr.clone(), stdout.clone()]]);
    assert!(
        pick(&s.raw, &k, &all, &["zzz qqq"], THRESHOLD)
            .unwrap()
            .is_empty()
    );
    // A short text counts as `MIN_GRAMS` trigrams: three held tell no topic.
    let short = pick(&s.raw, &k, &all, &["Lexer"], THRESHOLD).unwrap();
    assert!(short.is_empty());
    let whole = pick(&s.raw, &k, &all, &["Lexer tokens are cached"], THRESHOLD).unwrap();
    assert_eq!(uids(whole), [vec![lexer.clone()]]);
    // The CLI prints each one's share, from every delivered claim of the repository.
    let printed = report(
        s.home.path(),
        R,
        None,
        "where do the parser errors go",
        THRESHOLD,
    )
    .unwrap();
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with(&format!("{stdout} 0.")));
}
