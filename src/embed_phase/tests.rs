use super::fixture::*;
use super::*;
use crate::embed::stub::Stub;
use crate::search::b::fixture::Store;

const R: &str = "github.com/o/r";

/// A current native record's registration waits for a batch already through its final
/// check to send. Hooks still append, and registration completes before model headers.
#[test]
fn a_native_forget_orders_with_dispatch_without_waiting_for_embedding_headers() {
    use std::sync::mpsc::{RecvTimeoutError, sync_channel};
    const CANARY: &str = "native-dispatch-forget-canary-901";
    let mut s = Store::new();
    let stub = Stub::start();
    config(&s, &stub);
    let response = stub.hold();
    let mut event = crate::raw::test_event(&serde_json::json!({"prompt": CANARY}).to_string());
    event.source = "transcript".into();
    event.kind = "prompt".into();
    event.repo = Some(R.into());
    let identity = crate::raw::ImportIdentity {
        origin: crate::forget::origin("synthetic-dispatch", "native-901"),
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
    // Imported raw is held for recuration, but an ordinary claim anchored in it is an
    // active embedding document. Forget still targets the genuine native raw record.
    derive(&mut s, seq, CANARY, CANARY);
    s.run();
    let home = s.home.path().to_owned();
    let preview = crate::forget::preview(
        &home,
        crate::forget::Target::Record {
            device: s.raw.device().into(),
            seq,
        },
    )
    .unwrap();
    assert_eq!(
        preview.count(),
        1,
        "the fixture is a genuinely eligible native record"
    );
    let request = crate::forget::previous_transcript_request(&s.raw, seq, &identity);
    let k = crate::knowledge::open(&home).unwrap();
    let loaded = crate::config::load(&home).unwrap();
    let embedder = Embedder::from_config(&loaded.embedding).unwrap().unwrap();
    let reading = Reading::now(&s.raw, Reads::Live).unwrap();
    let rules = crate::redact::Rules::load(&home).unwrap();
    let batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
        .unwrap()
        .unwrap();
    assert!(batch.texts.iter().any(|t| t.contains(CANARY)));
    let (checked_at, checked) = sync_channel(1);
    let (transmit, begin) = sync_channel(1);
    let sending_home = home.clone();
    let sender = std::thread::spawn(move || {
        AFTER_SEND_CHECK.with(|at| {
            *at.borrow_mut() = Some(Box::new(move || {
                checked_at.send(()).unwrap();
                begin.recv_timeout(Duration::from_secs(3)).unwrap();
            }));
        });
        send(&sending_home, &batch, &embedder, Duration::from_secs(3))
    });
    checked.recv_timeout(Duration::from_secs(3)).unwrap();
    let (started_at, started) = sync_channel(1);
    let (registered_at, registered) = sync_channel(1);
    let registering_home = home.clone();
    let forget = std::thread::spawn(move || {
        started_at.send(()).unwrap();
        let result =
            crate::raw::open(&registering_home).and_then(|mut raw| raw.forget_apply(&[request]));
        registered_at.send(result.is_ok()).unwrap();
        result
    });
    started.recv_timeout(Duration::from_secs(3)).unwrap();
    // The dispatch hold is independent of raw's writer transaction.
    crate::raw::open(&home)
        .unwrap()
        .append(&crate::raw::test_event(
            "an unrelated hook while native dispatch is paused",
        ))
        .unwrap();
    let early = match registered.recv_timeout(Duration::from_millis(100)) {
        Ok(success) => {
            assert!(success);
            true
        }
        Err(RecvTimeoutError::Timeout) => false,
        Err(e) => panic!("forget thread ended: {e}"),
    };
    transmit.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while stub.requests() == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(stub.requests(), 1);
    let before_headers = early || registered.recv_timeout(Duration::from_secs(1)).unwrap();
    let result = forget.join().unwrap().unwrap();
    assert_eq!(result, 1);
    assert_eq!(
        stub.answered(),
        0,
        "the model response headers are still withheld"
    );
    drop(response);
    assert!(matches!(sender.join().unwrap().0, Sent::Vectors(_)));
    assert!(
        !early,
        "native forget committed after the send check but before transmission"
    );
    assert!(
        before_headers,
        "registration waited for the embedding response"
    );
}

/// A decided decision of `R` that quotes a new record (`quote`, whole), with its own `body`.
fn claim(s: &mut Store, quote: &str, body: &str) -> String {
    let seq = s.said("s", R, 1_000, quote);
    derive(s, seq, quote, body)
}

/// A claim op that quotes record `seq` (`quote`, whole) with `body`: a new claim, or a
/// re-derivation of the one that quotes the same.
fn derive(s: &mut Store, seq: i64, quote: &str, body: &str) -> String {
    use crate::claims::{ClaimOp, Evidence};
    let evidence = Evidence {
        device: s.raw.device().to_owned(),
        seq,
        offset: 0,
        length: quote.len() as i64,
        sentence: 0,
        quote: quote.into(),
        claim_at: None,
    };
    let uid = crate::claims::uid("decision", &evidence);
    let op = ClaimOp {
        id: "c1".into(),
        kind: "decision".into(),
        status: "decided".into(),
        speaker: "user".into(),
        scope: "repo".into(),
        body: body.into(),
        evidence: vec![evidence],
        supersedes: Vec::new(),
        recipe: "test".into(),
        tier: 1,
        why: String::new(),
        tainted: false,
        support: Vec::new(),
    };
    let op = serde_json::to_value(op).unwrap();
    let ops = [(crate::raw::OpKind::Claim, op)];
    s.raw.append_ops(&ops).unwrap();
    uid
}

/// The day's embedding requests spent, as earlier calls would have.
fn spend_the_day(home: &Path) {
    use crate::providers_db as pdb;
    let db = pdb::open(home).unwrap();
    for _ in 0..200 {
        let call = pdb::Call {
            provider: crate::embed::CALLS,
            role: ROLE,
            span: "",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: Some(1),
            usage: pdb::Usage::default(),
            usd: None,
        };
        pdb::record(&db, &call).unwrap();
    }
}

fn keys(s: &Store) -> Vec<(String, String, Option<String>)> {
    let k = crate::knowledge::open(s.home.path()).unwrap();
    k.prepare("SELECT kind, key, skipped FROM vector_keys ORDER BY kind, key")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn skipped(s: &Store, key: &str) -> Option<String> {
    keys(s)
        .into_iter()
        .find(|(_, k, _)| k == key)
        .and_then(|(.., why)| why)
}

/// docs/retention.md: a record whose text expired is sent to no embedder; its mark keeps it
/// out of the queue and the backlog, and a carry's list of records it takes no vector of names
/// it.
#[test]
fn an_expired_record_is_marked_and_never_sent() {
    let mut s = Store::new();
    let stub = Stub::start();
    config(&s, &stub);
    let gone = s.said("s", R, 1_000, "the expired lantern words");
    let kept = s.said("s", R, 2_000, "the kept harbor words");
    s.raw.expire_for_test(&[gone]);
    s.run();
    embed_all(&s);
    let key = |seq: i64| format!("{}:{seq}", s.raw.device());
    assert_eq!(skipped(&s, &key(gone)).as_deref(), Some("expired"));
    assert_eq!(skipped(&s, &key(kept)), None);
    let sent: Vec<String> = stub.texts().into_iter().flatten().collect();
    assert!(sent.iter().any(|t| t.contains("harbor")), "{sent:?}");
    assert!(sent.iter().all(|t| !t.contains("lantern")), "{sent:?}");
    let k = crate::knowledge::open(s.home.path()).unwrap();
    assert_eq!(
        doctor_facts_in(&k, crate::embed::EMBEDDER).records.unwrap(),
        0
    );
    assert!(s.raw.forgotten_record_keys().unwrap().contains(&key(gone)));
}

/// Row 30-1 (D13): nothing of an excluded repository reaches the embedder: its claims, a claim
/// elsewhere that quotes a session that touched it, its claude-mem project and that project's
/// worktree sessions, and that session's records in any repository. The rest is sent.
#[test]
fn an_excluded_repositorys_documents_reach_no_embedder() {
    const X: &str = "github.com/o/secret";
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let there = s.said("sx", X, 1_000, "Secret words there.");
    let here = s.said("sx", R, 2_000, "Secret words said here.");
    let secret = [
        derive(&mut s, there, "Secret words there.", "Secret words there."),
        derive(
            &mut s,
            here,
            "Secret words said here.",
            "Secret words said here.",
        ),
        s.imported(
            "o1",
            "secret",
            3_000,
            "Secret notes",
            "Secret project notes.",
        ),
        s.imported(
            "o2",
            "secret/wt",
            3_000,
            "Secret tree",
            "Secret worktree notes.",
        ),
        s.key(there),
        s.key(here),
    ];
    let open = s.said("so", R, 4_000, "Open words.");
    derive(&mut s, open, "Open words.", "Open words.");
    s.imported("o3", "r", 4_000, "Open notes", "Open project notes.");
    s.raw.exclude(X, false).unwrap();
    s.run();
    embed_all(&s);
    let sent = stub.texts().concat();
    assert!(sent.iter().any(|t| t == "Open words."), "{sent:?}");
    assert!(sent.iter().all(|t| !t.contains("Secret")), "{sent:?}");
    for key in &secret {
        assert_eq!(skipped(&s, key).as_deref(), Some("excluded"), "{key}");
    }
}

/// Tools slice 3 (V1, V3): cards and summaries are embedded after the claims, a skipped summary
/// is not, and each vector follows its row: a recuration's replaced card loses its vector and
/// the new card gets one, a rewind takes them off and maps the card it brings back with no
/// call, and a rebuild sends only the texts whose vectors no key of the old file mapped any more
/// (milestone 5 D5).
#[test]
fn a_cards_and_a_summarys_vectors_follow_their_rows() {
    use crate::consumer::{cards::Cards, turns::Turns};
    use crate::worker::Consumer;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let quote = "Parser caches stay in Redis.";
    let seq = s.said("s", R, 1_000, quote);
    derive(&mut s, seq, quote, quote);
    let first = s.cards(
        seq,
        seq,
        serde_json::json!([{"type": "bugfix", "title": "Azimuth"}]),
        false,
    );
    let turn = |skipped: bool| {
        serde_json::json!({"agent": "claude", "session": "s", "repo": R, "ts": 2_000,
                "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
                "fields": if skipped { serde_json::json!({}) }
                          else { serde_json::json!({"request": "Fix the parser."}) },
                "skipped": skipped})
    };
    let before_turns = s.raw.max_op_seq().unwrap();
    let summary = s.turn(turn(false));
    s.turn(turn(true));
    s.run();
    embed_all(&s);
    let device = s.raw.device().to_owned();
    let kept = |s: &Store, kind: &str| -> Vec<String> {
        keys(s)
            .into_iter()
            .filter(|(k, _, why)| k == kind && why.is_none())
            .map(|(_, key, _)| key)
            .collect()
    };
    assert_eq!(stub.texts()[0], [quote], "the claims' batch first");
    assert!(
        stub.texts()
            .concat()
            .iter()
            .any(|t| t.starts_with("Azimuth"))
    );
    assert_eq!(kept(&s, "o"), [format!("{device}.{}", first[0])]);
    let summary_key = format!("S{device}.{}", &summary[1..]);
    assert_eq!(kept(&s, "s"), std::slice::from_ref(&summary_key));
    // A recuration replaces the card.
    let before = s.raw.max_op_seq().unwrap();
    let second = s.cards(
        seq,
        seq,
        serde_json::json!([{"type": "feature", "title": "Nebula"}]),
        true,
    );
    s.run();
    embed_all(&s);
    assert_eq!(kept(&s, "o"), [format!("{device}.{}", second[0])]);
    // A restore that lost the recuration: the card it replaced is current again, mapped to the
    // vector it had, and a lost summary's vector goes.
    let sent = stub.requests();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    Cards.rewind(&k, &device, before).unwrap();
    Turns.rewind(&k, &device, before_turns).unwrap();
    assert!(kept(&s, "o").is_empty() && kept(&s, "s").is_empty());
    embed_all(&s);
    assert_eq!(kept(&s, "o"), [format!("{device}.{}", first[0])]);
    assert_eq!(stub.requests(), sent);
    // A rebuild, with no store open as `oboete rebuild` runs, makes the rows again from the op
    // log, which still holds the recuration and the summary: their vectors, which no key of the
    // old file mapped after the rewind, are not carried and are made again.
    drop(k);
    let Store { home, raw } = s;
    drop(raw);
    crate::worker::rebuild(home.path()).unwrap();
    let s = Store {
        raw: crate::raw::open(home.path()).unwrap(),
        home,
    };
    embed_all(&s);
    assert_eq!(kept(&s, "o"), [format!("{device}.{}", second[0])]);
    assert_eq!(kept(&s, "s"), [summary_key]);
    let again = stub.texts()[sent..].concat();
    assert_eq!(again.len(), 2, "{again:?}");
    assert!(again.iter().any(|t| t.starts_with("Nebula")), "{again:?}");
    assert!(
        again.iter().any(|t| t.contains("Fix the parser.")),
        "{again:?}"
    );
}

/// Tools slice 3 (V2, D13): an excluded repository's card and summary, those of a session
/// that touched it, and those made from one of its records (a window across sessions, a goal,
/// a window a summary read) are not sent; each is marked `excluded`. The rest is sent.
/// Milestone 5 D5: a claim and an imported document forgotten before their vectors reach no
/// embedder, though knowledge.db still holds them: each is marked `forgotten`, and the rest are
/// sent.
#[test]
fn a_forgotten_claim_or_document_reaches_no_embedder() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let uid = claim(&mut s, "Parser caches stay in Redis.", "Secret claim body.");
    let doc = s.imported("o1", "r", 2_000, "Secret title", "Secret document body.");
    s.said("s", R, 3_000, "Open words.");
    s.run();
    let home = s.home.path().to_owned();
    for u in [&uid, &doc] {
        let target = crate::forget::Target::parse_uid(u).unwrap();
        let p = crate::forget::preview(&home, target).unwrap();
        crate::forget::start(&home, &p).unwrap();
    }
    embed_all(&s);
    let sent = stub.texts().concat();
    assert!(sent.iter().any(|t| t.contains("Open words")), "{sent:?}");
    assert!(sent.iter().all(|t| !t.contains("Secret")), "{sent:?}");
    assert_eq!(skipped(&s, &uid).as_deref(), Some("forgotten"));
    assert_eq!(skipped(&s, &doc).as_deref(), Some("forgotten"));
}

#[test]
fn an_excluded_repositorys_cards_and_summaries_reach_no_embedder() {
    const X: &str = "github.com/o/secret";
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let there = s.said("sx", X, 1_000, "Secret words there.");
    let elsewhere = s.said("sx", R, 2_000, "Words said here.");
    let open = s.said("so", R, 3_000, "Open words.");
    let card = |s: &mut Store, seq, title: &str| {
        s.cards(
            seq,
            seq,
            serde_json::json!([{"type": "bugfix", "title": title}]),
            false,
        )[0]
        .clone()
    };
    let turn = |s: &mut Store, session: &str, repo: &str, seq, request: &str| {
        s.turn(
            serde_json::json!({"agent": "claude", "session": session, "repo": repo,
                "ts": 4_000, "from": seq, "through": seq, "read": [], "goals": [],
                "removed": [], "fields": {"request": request}, "skipped": false}),
        )
    };
    let device = s.raw.device().to_owned();
    // K4's records: a window over two sessions has no session or repository label of its own,
    // and a goal is a record its curator was shown.
    let mixed = s.cards(
        elsewhere,
        open,
        serde_json::json!([{"type": "bugfix", "title": "Secret card across sessions"}]),
        false,
    )[0]
    .clone();
    let goal = s
        .raw
        .append_ops(&[(
            crate::raw::OpKind::Window,
            serde_json::json!({"outcome": "curated", "summary": "", "from_seq": open,
                "to_seq": open, "from_offset": null, "to_offset": null, "elided": [],
                "removed": [], "goals": [there], "recurate": false,
                "observations": [{"type": "bugfix", "title": "Secret card of a goal"}]}),
        )])
        .unwrap()[0];
    // T7's: the windows whose cards a summary read, and their goals.
    let read = |s: &mut Store, read: serde_json::Value, goals: serde_json::Value, request| {
        s.turn(
            serde_json::json!({"agent": "claude", "session": "so", "repo": R,
                "ts": 4_000, "from": open, "through": open, "read": read, "goals": goals,
                "removed": [], "fields": {"request": request}, "skipped": false}),
        )
    };
    let read_there = read(
        &mut s,
        serde_json::json!([[there, there]]),
        serde_json::json!([]),
        "Secret summary of what it read",
    );
    let goal_there = read(
        &mut s,
        serde_json::json!([]),
        serde_json::json!([there]),
        "Secret summary of a goal",
    );
    let not_sent = [
        format!("{device}.{}", card(&mut s, there, "Secret card there")),
        format!("{device}.{}", card(&mut s, elsewhere, "Secret card here")),
        format!("{device}.{mixed}"),
        format!("{device}.{goal}.0"),
        format!(
            "S{device}.{}",
            &turn(&mut s, "sx", X, there, "Secret summary")[1..]
        ),
        format!(
            "S{device}.{}",
            &turn(&mut s, "sx", R, elsewhere, "Secret summary here")[1..]
        ),
        format!("S{device}.{}", &read_there[1..]),
        format!("S{device}.{}", &goal_there[1..]),
    ];
    card(&mut s, open, "Open card");
    turn(&mut s, "so", R, open, "Open summary");
    s.raw.exclude(X, false).unwrap();
    s.run();
    embed_all(&s);
    let sent = stub.texts().concat();
    assert!(sent.iter().any(|t| t.starts_with("Open card")), "{sent:?}");
    assert!(sent.iter().any(|t| t == "Open summary"), "{sent:?}");
    assert!(sent.iter().all(|t| !t.contains("Secret")), "{sent:?}");
    for key in &not_sent {
        assert_eq!(skipped(&s, key).as_deref(), Some("excluded"), "{key}");
    }
}

/// Tools slice 3 (V2, K4, T7): a card and a summary a removal hides before they are embedded
/// are not sent; each is marked `empty`, as a removed record is.
#[test]
fn a_card_and_a_summary_a_removal_hides_reach_no_embedder() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let gone = s.said("s", R, 1_000, "Gone words.");
    let card = s.cards(
        gone,
        gone,
        serde_json::json!([{"type": "bugfix", "title": "Hidden card"}]),
        false,
    )[0]
    .clone();
    let summary = s.turn(
        serde_json::json!({"agent": "claude", "session": "s", "repo": R,
            "ts": 2_000, "from": gone, "through": gone, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Hidden summary"}, "skipped": false}),
    );
    let device = s.raw.device().to_owned();
    s.raw
        .append_tombstone(crate::raw::Target::Record {
            device: device.clone(),
            seq: gone,
        })
        .unwrap();
    s.run();
    embed_all(&s);
    let sent = stub.texts().concat();
    assert!(sent.iter().all(|t| !t.contains("Hidden")), "{sent:?}");
    for key in [
        format!("{device}.{card}"),
        format!("S{device}.{}", &summary[1..]),
    ] {
        assert_eq!(skipped(&s, &key).as_deref(), Some("empty"), "{key}");
    }
}

/// Parity row 3 (docs/card-hide.md): a card and a summary removed from the page lose their
/// vectors and are sent to no embedder while removed; brought back, they are embedded again.
#[test]
fn a_card_and_a_summary_removed_from_the_page_lose_their_vectors_until_brought_back() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let seq = s.said("s", R, 1_000, "Ship the parser.");
    let card = s.cards(
        seq,
        seq,
        serde_json::json!([{"type": "bugfix", "title": "Azimuth"}]),
        false,
    )[0]
    .clone();
    let summary = s.turn(
        serde_json::json!({"agent": "claude", "session": "s", "repo": R,
            "ts": 2_000, "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {"request": "Fix the parser."}, "skipped": false}),
    );
    s.run();
    embed_all(&s);
    let device = s.raw.device().to_owned();
    let both = vec![
        format!("{device}.{card}"),
        format!("S{device}.{}", &summary[1..]),
    ];
    let kept = |s: &Store| -> Vec<String> {
        (keys(s).into_iter())
            .filter(|(kind, _, why)| ["o", "s"].contains(&kind.as_str()) && why.is_none())
            .map(|(_, key, _)| key)
            .collect()
    };
    assert_eq!(kept(&s), both);
    let hide = |kind: &str, id: &str, hidden: bool| {
        let body = serde_json::json!({"item": {"kind": kind, "id": id}, "hidden": hidden});
        let body = serde_json::to_vec(&body).unwrap();
        crate::settings::hide::save(s.home.path(), &std::sync::Mutex::new(()), &body).unwrap();
    };
    hide("card", &card, true);
    hide("summary", &summary, true);
    assert!(kept(&s).is_empty(), "their vectors go");
    let sent = stub.requests();
    embed_all(&s);
    assert_eq!(stub.requests(), sent, "nothing removed is sent");
    hide("card", &card, false);
    hide("summary", &summary, false);
    embed_all(&s);
    assert_eq!(kept(&s), both, "brought back, they are embedded again");
}

/// Tools slice 3 (V1, K6; Codex): each value of a card or a summary is gated alone, as its
/// reader gates it, so a rule on a whole value that spans its lines holds in the text sent.
#[test]
fn a_rule_on_a_whole_value_of_a_card_or_summary_holds_in_the_text_sent() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let path = s.home.path().join("config.toml");
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = '^ACME\\ncode=[0-9]{6}$' }]\n";
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + rule).unwrap();
    let seq = s.said("s", R, 1_000, "Open words.");
    s.cards(
        seq,
        seq,
        serde_json::json!([{"type": "bugfix", "title": "Card title",
                "facts": ["ACME\ncode=123456"]}]),
        false,
    );
    s.turn(
        serde_json::json!({"agent": "claude", "session": "s", "repo": R, "ts": 2_000,
            "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {"request": "Summary request", "learned": "ACME\ncode=654321"},
            "skipped": false}),
    );
    s.run();
    embed_all(&s);
    let sent = stub.texts().concat();
    assert!(sent.iter().any(|t| t.starts_with("Card title")), "{sent:?}");
    assert!(
        sent.iter().any(|t| t.starts_with("Summary request")),
        "{sent:?}"
    );
    assert!(
        sent.iter()
            .all(|t| !t.contains("123456") && !t.contains("654321")),
        "{sent:?}"
    );
}

/// Codex on #429: a text the rules leave blank is passed over under those rules only: when
/// they change, it is read again, and sent when the new rules leave it words.
#[test]
fn a_text_the_rules_blank_is_read_again_when_they_change() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let path = s.home.path().join("config.toml");
    let unruled = std::fs::read_to_string(&path).unwrap();
    let rule = "[redaction]\nextra_rules = [{ id = \"plan\", regex = 'Secret plan' }]\n";
    std::fs::write(&path, unruled.clone() + rule).unwrap();
    let seq = s.said("s", R, 1_000, "Open words.");
    s.cards(
        seq,
        seq,
        serde_json::json!([{"type": "decision", "title": "Secret plan"}]),
        false,
    );
    s.turn(
        serde_json::json!({"agent": "claude", "session": "s", "repo": R, "ts": 2_000,
            "from": seq, "through": seq, "read": [], "goals": [], "removed": [],
            "fields": {"request": "Secret plan"}, "skipped": false}),
    );
    s.run();
    embed_all(&s);
    let blanked = |t: &String| t.contains("Secret plan") || t.trim() == "[REDACTED]";
    assert!(!stub.texts().concat().iter().any(blanked));
    // doctor counts them as texts with nothing to send.
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let facts = doctor_facts_in(&k, crate::embed::EMBEDDER).skipped.unwrap();
    assert!(facts.contains(&("empty".to_owned(), 2)), "{facts:?}");
    let before = stub.requests();
    std::fs::write(&path, unruled).unwrap();
    embed_all(&s);
    let later = stub.texts()[before..].concat();
    assert_eq!(
        later
            .iter()
            .filter(|t| t.starts_with("Secret plan"))
            .count(),
        2,
        "{later:?}"
    );
}

/// CodeRabbit on #429: a joined text's key is never another text's plain hash, whatever bytes
/// that text holds (a claim's body may hold a NUL).
#[test]
fn a_joined_key_is_never_a_plain_texts_key() {
    let one = 0..1;
    assert_ne!(
        joined_sha("A", std::slice::from_ref(&one)),
        sha("A\u{0}0-1")
    );
}

/// Codex and CodeRabbit on #429: a card's or a summary's vector is reused only by a row whose
/// values join to the same text at the same places. Values split differently gate differently
/// (`joined_with`), so the second row's own gated text is sent, not mapped to the first's.
#[test]
fn values_joined_to_one_text_at_other_places_share_no_vector() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let path = s.home.path().join("config.toml");
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = '^ACME\\ncode=[0-9]{6}$' }]\n";
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + rule).unwrap();
    let summary = |s: &mut Store, session: &str, fields: serde_json::Value| {
        let seq = s.said(session, R, 1_000, "Open words.");
        s.turn(
            serde_json::json!({"agent": "claude", "session": session, "repo": R,
                "ts": 2_000, "from": seq, "through": seq, "read": [], "goals": [],
                "removed": [], "fields": fields, "skipped": false}),
        );
    };
    let card = |s: &mut Store, session: &str, facts: serde_json::Value| {
        let seq = s.said(session, R, 1_000, "Open words.");
        s.cards(
            seq,
            seq,
            serde_json::json!([{"type": "bugfix", "title": "Card", "facts": facts}]),
            false,
        );
    };
    // Neither value is the rule's whole: the code goes out with the rest.
    summary(
        &mut s,
        "a",
        serde_json::json!({"request": "Prefix\nACME\ncode=123456"}),
    );
    card(&mut s, "b", serde_json::json!(["ACME", "code=654321"]));
    s.run();
    embed_all(&s);
    let first = stub.texts().concat();
    assert!(first.iter().any(|t| t.contains("123456")), "{first:?}");
    assert!(first.iter().any(|t| t.contains("654321")), "{first:?}");
    let before = stub.requests();
    // The same texts, one value the rule's whole in each.
    summary(
        &mut s,
        "c",
        serde_json::json!({"request": "Prefix", "learned": "ACME\ncode=123456"}),
    );
    card(&mut s, "d", serde_json::json!(["ACME\ncode=654321"]));
    s.run();
    embed_all(&s);
    let later = stub.texts()[before..].concat();
    assert!(later.iter().any(|t| t.starts_with("Prefix")), "{later:?}");
    assert!(later.iter().any(|t| t.starts_with("Card")), "{later:?}");
    assert!(
        later
            .iter()
            .all(|t| !t.contains("123456") && !t.contains("654321")),
        "{later:?}"
    );
}

/// Tools slice 3 (Codex): cards held for an answer (Step 6) come back as cards, each sent once
/// more alone after another document's answer, not read again as one batch.
#[test]
fn held_cards_are_sent_again_each_alone() {
    use crate::providers_db as pdb;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let seq = s.said("s", R, 1_000, "Open words.");
    s.run();
    embed_all(&s);
    let db = pdb::open(s.home.path()).unwrap();
    let mut held = Vec::new();
    for title in ["First card", "Second card"] {
        let card = s.cards(
            seq,
            seq,
            serde_json::json!([{"type": "bugfix", "title": title}]),
            false,
        )[0]
        .clone();
        stub.fail_next(400, None);
        s.run();
        embed_all(&s);
        let key = format!("{}.{card}", s.raw.device());
        assert_eq!(skipped(&s, &key).as_deref(), Some("held"));
        // A 400 alone is one failure and no rest: cleared, so the next card is alone too.
        pdb::set_state(&db, crate::embed::CALLS, pdb::State::default()).unwrap();
        held.push(key);
    }
    let sent = stub.requests();
    s.said("s", R, 2_000, "Later words.");
    s.run();
    embed_all(&s);
    let texts = &stub.texts()[sent..];
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert!(texts.iter().any(|t| *t == ["First card"]), "{texts:?}");
    assert!(texts.iter().any(|t| *t == ["Second card"]), "{texts:?}");
    for key in &held {
        assert!(
            keys(&s).iter().any(|(_, k, why)| k == key && why.is_none()),
            "{key}"
        );
    }
}

/// D13: a batch read under one exclusion list is not sent once the list changed: `send` makes
/// no request and the phase keeps nothing of it, its count included; the next poll reads again
/// and sends what may go.
#[test]
fn a_batch_read_before_an_exclusion_is_never_sent() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.said("s", R, 1_000, "Open words.");
    s.run();
    let home = s.home.path();
    let k = crate::knowledge::open(home).unwrap();
    let config = crate::config::load(home).unwrap();
    let embedder = Embedder::from_config(&config.embedding).unwrap().unwrap();
    let reading = Reading::now(&s.raw, Reads::Live).unwrap();
    let rules = crate::redact::Rules::load(home).unwrap();
    let batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
        .unwrap()
        .unwrap();
    let db = crate::providers_db::open(home).unwrap();
    let call = reserve(&db, ROLE, "1 documents", "Open words.", 160, 1.0)
        .unwrap()
        .unwrap();
    s.raw.exclude("github.com/o/elsewhere", false).unwrap();
    let (sent, _) = send(home, &batch, &embedder, Duration::from_secs(5));
    assert!(matches!(&sent, Sent::Unsent(e) if e.is::<crate::curate::ListChanged>()));
    Phase::new(home)
        .finish(&s.raw, &k, &batch, Some(call), sent, 0)
        .unwrap();
    assert_eq!(stub.requests(), 0);
    let rows: i64 = db
        .query_row(
            "SELECT (SELECT count(*) FROM provider_calls) + (SELECT count(*) FROM provider_state)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
    embed_all(&s);
    assert_eq!(stub.requests(), 1);
}

/// Spec 6.4: the sending thread checks the rules the batch was composed under.
#[test]
fn a_paused_send_rejects_changed_or_invalid_rules() {
    for invalid in [false, true] {
        let mut s = Store::new();
        config_at(&s, "http://127.0.0.1:1/run/bge-m3");
        s.said("s", R, 1_000, "opaque_canary");
        s.run();
        let home = s.home.path();
        let k = crate::knowledge::open(home).unwrap();
        let config = crate::config::load(home).unwrap();
        let embedder = Embedder::from_config(&config.embedding).unwrap().unwrap();
        let reading = Reading::now(&s.raw, Reads::Live).unwrap();
        let rules = crate::redact::Rules::load(home).unwrap();
        let batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
            .unwrap()
            .unwrap();
        let gate = std::sync::Barrier::new(2);
        let (sent, ms) = std::thread::scope(|scope| {
            let sending = scope.spawn(|| {
                gate.wait();
                send(home, &batch, &embedder, Duration::from_millis(100))
            });
            let path = home.join("config.toml");
            let regex = if invalid { "(" } else { "^opaque_canary$" };
            let rules =
                format!("[redaction]\nextra_rules = [{{ id = \"canary\", regex = '{regex}' }}]\n");
            std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + &rules).unwrap();
            gate.wait();
            sending.join().unwrap()
        });
        assert!(
            matches!(sent, Sent::Unsent(_)),
            "invalid={invalid}: a stale batch reached the embedder"
        );
        assert_eq!(ms, 0);
    }
}

/// A tombstone written while the sender waits invalidates document and query text alike.
#[test]
fn a_paused_send_rejects_a_new_tombstone() {
    for query in [false, true] {
        let mut s = Store::new();
        config_at(&s, "http://127.0.0.1:1/run/bge-m3");
        let seq = s.said("s", R, 1_000, "opaque_canary");
        s.run();
        let home = s.home.path().to_owned();
        let k = crate::knowledge::open(&home).unwrap();
        let config = crate::config::load(&home).unwrap();
        let embedder = Embedder::from_config(&config.embedding).unwrap().unwrap();
        let reading = Reading::now(&s.raw, Reads::Live).unwrap();
        let rules = crate::redact::Rules::load(&home).unwrap();
        let mut batch = pending(&s.raw, &k, &embedder.id, &reading, &rules, false)
            .unwrap()
            .unwrap();
        if query {
            batch.docs.clear();
        }
        let gate = std::sync::Barrier::new(2);
        let (sent, ms) = std::thread::scope(|scope| {
            let sending = scope.spawn(|| {
                gate.wait();
                send(&home, &batch, &embedder, Duration::from_millis(100))
            });
            s.raw
                .append_tombstone(crate::raw::Target::Record {
                    device: s.raw.device().to_owned(),
                    seq,
                })
                .unwrap();
            gate.wait();
            sending.join().unwrap()
        });
        assert!(
            matches!(sent, Sent::Unsent(_)),
            "query={query}: tombstoned text reached the embedder"
        );
        assert_eq!(ms, 0);
    }
}

/// A tombstone after consumer drain also keeps stale claim and raw indexes from egress.
#[test]
fn pending_tombstones_keep_raw_and_claim_text_out_of_batches() {
    for whole in [true, false] {
        let mut s = Store::new();
        let text = "opaque_canary parser";
        let seq = s.said("s", R, 1_000, text);
        s.claim(seq, text, ("decision", "decided", "user"), &[]);
        s.run();
        let device = s.raw.device().to_owned();
        let target = if whole {
            crate::raw::Target::Record { device, seq }
        } else {
            let e = crate::consumer::manifest::event(&s.raw, &device, seq)
                .unwrap()
                .unwrap();
            crate::raw::Target::Range {
                device,
                seq,
                offset: e.body.find("opaque_canary").unwrap() as i64,
                length: "opaque_canary".len() as i64,
            }
        };
        s.raw.append_tombstone(target).unwrap();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let reading = Reading::now(&s.raw, Reads::Live).unwrap();
        let rules = crate::redact::Rules::default();
        let batch = pending(&s.raw, &k, crate::embed::EMBEDDER, &reading, &rules, false).unwrap();
        assert!(
            batch
                .as_ref()
                .is_none_or(|b| b.texts.iter().all(|t| !t.contains("opaque_canary"))),
            "whole={whole}: pending tombstoned text was composed"
        );
    }
}

/// A pending status change holds a claim only until the consumer applies it.
#[test]
fn a_pending_status_correction_does_not_permanently_skip_a_claim() {
    let mut s = Store::new();
    let text = "Parser errors go to stderr.";
    let seq = s.said("s", R, 1_000, text);
    let uid = s.claim(seq, text, ("decision", "decided", "user"), &[]);
    s.run();
    let correction = serde_json::json!({
        "uid": uid, "anchor": {"device": s.raw.device(), "seq": seq}, "status": "done"
    });
    s.raw
        .append_ops(&[(crate::raw::OpKind::Correction, correction)])
        .unwrap();
    vectors(&s);
    s.run();
    vectors(&s);
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let embedded: bool = k.query_row(
            "SELECT EXISTS(SELECT 1 FROM vector_keys WHERE kind = 'c' AND key = ?1 AND skipped IS NULL)",
            [&uid], |r| r.get(0),
        ).unwrap();
    assert!(
        embedded,
        "the applied status change left the claim permanently skipped"
    );
}

/// D13: an exclusion undone queues its documents again: the marks made under the old list go.
#[test]
fn undoing_an_exclusion_queues_its_documents() {
    const X: &str = "github.com/o/secret";
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let seq = s.said("sx", X, 1_000, "Secret words there.");
    s.raw.exclude(X, false).unwrap();
    s.run();
    embed_all(&s);
    assert_eq!(stub.requests(), 0);
    assert_eq!(skipped(&s, &s.key(seq)).as_deref(), Some("excluded"));
    s.raw.exclude(X, true).unwrap();
    s.run();
    embed_all(&s);
    assert_eq!(stub.texts().concat(), ["Secret words there."]);
    assert!(
        keys(&s)
            .iter()
            .any(|(_, k, why)| *k == s.key(seq) && why.is_none())
    );
}

/// Step 13: a poll reads a page of `vector_todo`, never the whole store. The queue holds the
/// imported documents and records with no key row, those a home held before its first poll
/// included, and nothing once they are embedded; a new record, a text changed and an exclusion
/// undone each put their document back.
#[test]
fn the_queue_holds_only_what_waits_for_a_vector() {
    const X: &str = "github.com/o/secret";
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.imported("o1", "r", 1_000, "Deploy notes", "Notes.");
    let seq = s.said("s", R, 2_000, "Words to mask.");
    let secret = s.said("sx", X, 3_000, "Secret words there.");
    s.raw.exclude(X, false).unwrap();
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let queued = || -> Vec<String> {
        k.prepare("SELECT family || ' ' || key FROM vector_todo ORDER BY family, key")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    embed_all(&s);
    assert!(queued().is_empty(), "{:?}", queued());
    let more = s.said("s", R, 4_000, "More words.");
    s.run();
    assert_eq!(queued(), [format!("r {}", s.key(more))]);
    embed_all(&s);
    assert!(queued().is_empty(), "{:?}", queued());
    let body = serde_json::json!({ "prompt": "Words to mask." }).to_string();
    s.raw
        .append_tombstone(crate::raw::Target::Range {
            device: s.raw.device().to_owned(),
            seq,
            offset: body.find("mask").unwrap() as i64,
            length: 4,
        })
        .unwrap();
    s.run();
    assert_eq!(queued(), [format!("r {}", s.key(seq))]);
    embed_all(&s);
    s.raw.exclude(X, true).unwrap();
    s.run();
    embed_all(&s);
    assert!(queued().is_empty(), "{:?}", queued());
    assert!(
        stub.texts()
            .concat()
            .iter()
            .any(|t| t == "Secret words there.")
    );
    assert!(
        keys(&s)
            .iter()
            .any(|(_, key, why)| *key == s.key(secret) && why.is_none())
    );
}

/// Settle the current call (starting one if needed), then return the next poll's step.
fn call(s: &Store, k: &Connection, phase: &mut Phase) -> Step {
    if phase.flight.is_none() {
        assert!(matches!(
            phase.poll(&s.raw, k).unwrap(),
            Step::Waiting { .. }
        ));
    }
    assert!(phase.flight.is_some(), "embedding call did not start");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !phase.done() {
        assert!(
            std::time::Instant::now() < deadline,
            "embedding call did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(phase.poll(&s.raw, k).unwrap(), Step::Covered);
    phase.poll(&s.raw, k).unwrap()
}

/// Rows 55-5 and 7.6 (Step 6): a 429, a 500 and a call past its timeout each rest the
/// embedder, and the documents wait for a later call. A text the model will not take alone is
/// `refused` once another part of its batch was answered, and the rest of the batch goes; with
/// no part answered, the embedder rests instead. Each request counts in the day's.
#[test]
fn a_refused_or_timed_out_embedding_waits_and_loses_nothing() {
    use crate::providers_db as pdb;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let words = s.said("s", R, 1_000, "Words to embed.");
    s.run();
    let home = s.home.path();
    let k = crate::knowledge::open(home).unwrap();
    let db = pdb::open(home).unwrap();
    let mut phase = Phase::new(home);
    phase.timeout = Duration::from_millis(300);
    let rested = |step: Step, at_least: i64| {
        let Step::Waiting { until, up } = step else {
            panic!("{step:?}")
        };
        assert!(until >= crate::db::now_ms() + at_least, "{step:?}");
        assert!(up);
        assert_eq!(skipped(&s, &s.key(words)), None);
        assert!(!keys(&s).iter().any(|(_, key, _)| *key == s.key(words)));
        pdb::set_state(&db, crate::embed::CALLS, pdb::State::default()).unwrap();
    };
    stub.fail_next(429, Some(2));
    rested(call(&s, &k, &mut phase), 40_000);
    stub.fail_next(500, None);
    rested(call(&s, &k, &mut phase), 500_000);
    let held = stub.hold();
    rested(call(&s, &k, &mut phase), 500_000);
    drop(held);
    while stub.answered() < stub.requests() {
        std::thread::sleep(Duration::from_millis(5));
    }
    // A 404 sets no rest at first (the breaker counts it), and the next answer clears the count.
    stub.fail_next(404, None);
    assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
    assert_eq!(pdb::state(&db, crate::embed::CALLS).unwrap().fails, 1);
    assert_eq!(call(&s, &k, &mut phase), Step::Idle);
    assert_eq!(
        pdb::state(&db, crate::embed::CALLS).unwrap(),
        pdb::State::default()
    );
    assert!(
        keys(&s)
            .iter()
            .any(|(_, key, why)| *key == s.key(words) && why.is_none())
    );
    assert_eq!(pdb::calls_in_a_day(&db, crate::embed::CALLS).unwrap().0, 5);

    // Split down to the text alone, which fails before the other half is answered: held, then
    // sent once more alone after that answer, and refused; the rest embedded.
    stub.refuse("Poison words.");
    let poison = s.said("s", R, 2_000, "Poison words.");
    let fine = s.said("s", R, 3_000, "Fine words.");
    s.run();
    until_idle(&s.raw, &k, &mut phase);
    assert_eq!(stub.requests(), 9);
    assert_eq!(skipped(&s, &s.key(poison)).as_deref(), Some("refused"));
    assert!(
        keys(&s)
            .iter()
            .any(|(_, key, why)| *key == s.key(fine) && why.is_none())
    );
    assert_eq!(
        pdb::state(&db, crate::embed::CALLS).unwrap(),
        pdb::State::default()
    );

    // Alone from the start with nothing answered: not refused, as the embedder may be what
    // fails, and not rested on or in the way of the rest: held, a mark, so not sent again by
    // this worker or the next, until another document's answer; then sent once more alone and
    // refused.
    let alone = s.said("s", R, 4_000, "Poison words.");
    s.run();
    let sent = stub.requests();
    assert!(matches!(
        phase.poll(&s.raw, &k).unwrap(),
        Step::Waiting { .. }
    ));
    while !phase.done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
    assert_eq!(stub.requests(), sent + 1);
    assert_eq!(skipped(&s, &s.key(alone)).as_deref(), Some("held"));
    // Counted as the breaker counts a 400 (one, no rest), so failing alone again and again
    // with nothing answered would rest it.
    assert_eq!(
        pdb::state(&db, crate::embed::CALLS).unwrap(),
        pdb::State {
            fails: 1,
            ..pdb::State::default()
        }
    );
    let mut phase = Phase::new(s.home.path());
    phase.timeout = Duration::from_millis(300);
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
    assert_eq!(stub.requests(), sent + 1);
    let later = s.said("s", R, 5_000, "Later words.");
    s.run();
    until_idle(&s.raw, &k, &mut phase);
    assert_eq!(stub.requests(), sent + 3);
    assert_eq!(stub.texts()[sent + 1], ["Later words."]);
    assert_eq!(skipped(&s, &s.key(alone)).as_deref(), Some("refused"));
    assert!(
        keys(&s)
            .iter()
            .any(|(_, key, why)| *key == s.key(later) && why.is_none())
    );
    assert_eq!(
        pdb::calls_in_a_day(&db, crate::embed::CALLS).unwrap().0 as usize,
        stub.requests()
    );
}

/// Steps 4 and 6: a text whose stored text changed while its split went on is not refused:
/// the mark would hold the old text's hash, and the new text would never be read again.
#[test]
fn a_text_changed_during_its_split_is_not_refused() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    stub.refuse("Poison words.");
    let poison = s.said("s", R, 1_000, "Poison words.");
    s.said("s", R, 2_000, "Fine words.");
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = Phase::new(s.home.path());
    // The batch, then the poison alone: both refused, the other half still to go.
    for _ in 0..2 {
        assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
    }
    let body = serde_json::json!({ "prompt": "Poison words." }).to_string();
    s.raw
        .append_tombstone(crate::raw::Target::Range {
            device: s.raw.device().to_owned(),
            seq: poison,
            offset: body.find("words").unwrap() as i64,
            length: 5,
        })
        .unwrap();
    s.run();
    until_idle(&s.raw, &k, &mut phase);
    assert!(stub.texts().concat().iter().any(|t| t == "Poison *****."));
    assert!(
        keys(&s)
            .iter()
            .any(|(_, key, why)| *key == s.key(poison) && why.is_none())
    );
}

/// Step 6 (D8): a split's half that waited out a rest is read again before it goes: a text
/// masked meanwhile never leaves as it was, and goes as it is now.
#[test]
fn a_split_half_is_read_again_before_it_is_sent() {
    use crate::providers_db as pdb;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    stub.refuse("Poison words.");
    let poison = s.said("s", R, 1_000, "Poison words.");
    s.said("s", R, 2_000, "Fine words, longer.");
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = Phase::new(s.home.path());
    let answered = |phase: &mut Phase| {
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    };
    // The batch is refused and split; its other half meets a 429: the poison's half waits.
    answered(&mut phase);
    stub.fail_next(429, Some(60));
    answered(&mut phase);
    assert_eq!(stub.requests(), 2);
    let body = serde_json::json!({ "prompt": "Poison words." }).to_string();
    s.raw
        .append_tombstone(crate::raw::Target::Range {
            device: s.raw.device().to_owned(),
            seq: poison,
            offset: body.find("words").unwrap() as i64,
            length: 5,
        })
        .unwrap();
    s.run();
    let db = pdb::open(s.home.path()).unwrap();
    pdb::set_state(&db, crate::embed::CALLS, pdb::State::default()).unwrap();
    until_idle(&s.raw, &k, &mut phase);
    let after = stub.texts()[2..].concat();
    assert!(!after.iter().any(|t| t == "Poison words."), "{after:?}");
    assert!(after.iter().any(|t| t == "Poison *****."), "{after:?}");
}

/// Step 6: a split that nothing answers stops after `SPLIT_FAILS` requests: the embedder is
/// failing, not a text, so nothing is refused; the texts that failed alone are held for an
/// answer.
#[test]
fn a_split_nothing_answers_stops_at_its_bound() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    for i in 0..16 {
        s.said("s", R, 1_000 + i, &format!("Words {i:02} to embed."));
    }
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = Phase::new(s.home.path());
    for _ in 0..SPLIT_FAILS {
        stub.fail_next(400, None);
        assert!(phase.flight.is_none());
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    }
    assert!(phase.split.is_none());
    assert_eq!(stub.requests(), SPLIT_FAILS as usize);
    let marks: Vec<_> = keys(&s).into_iter().map(|(_, _, why)| why).collect();
    assert!(!marks.is_empty());
    assert!(
        marks.iter().all(|why| why.as_deref() == Some("held")),
        "{marks:?}"
    );
}

/// The Global Constraints (Step 7): batches stop 40 requests short of `daily_requests`, kept
/// for query vectors, which pass until the day's are spent; batches stop at `monthly_usd` too;
/// embedding spend and the paid curators' spend are counted apart, so neither stops the other.
#[test]
fn the_embedding_cap_is_its_own_and_keeps_room_for_queries() {
    use crate::providers_db as pdb;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.said("s", R, 1_000, "Words to embed.");
    s.run();
    let home = s.home.path().to_owned();
    let home = home.as_path();
    let k = crate::knowledge::open(home).unwrap();
    let db = pdb::open(home).unwrap();
    let row = |role: &'static str, usd: Option<f64>, provider: &'static str| pdb::Call {
        provider,
        role,
        span: "",
        outcome: "ok",
        ms: 1,
        detail: None,
        bytes_out: 1,
        est_tokens: Some(1),
        usage: pdb::Usage::default(),
        usd,
    };
    for _ in 0..159 {
        pdb::record(&db, &row("embed", None, crate::embed::CALLS)).unwrap();
    }
    // A paid curator spent past the USD 5: the embedder is not stopped.
    pdb::record(&db, &row("curator", Some(9.0), "paid")).unwrap();
    let mut phase = Phase::new(home);
    assert_eq!(call(&s, &k, &mut phase), Step::Idle);
    assert_eq!(stub.requests(), 1);
    s.said("s", R, 2_000, "More words to embed.");
    s.run();
    let Step::Waiting { until, up } = phase.poll(&s.raw, &k).unwrap() else {
        panic!("160 requests in the day, and a batch went")
    };
    assert!(until > crate::db::now_ms() + 3_600_000 && !up);
    assert_eq!(stub.requests(), 1);
    // A query passes, from the 40 kept for queries, until the day's 200 are spent.
    use crate::search::b::{Query, Vector, VectorSkip, query};
    let q = Query {
        text: "Words".into(),
        caller: Some(R.into()),
        limit: 5,
        ..Default::default()
    };
    assert_eq!(query(home, &q).unwrap().vector, Vector::Used);
    assert_eq!(stub.requests(), 2);
    for _ in 0..39 {
        pdb::record(&db, &row("query", None, crate::embed::CALLS)).unwrap();
    }
    let spent = Vector::Skipped(VectorSkip::Waiting);
    assert_eq!(query(home, &q).unwrap().vector, spent);
    assert_eq!(stub.requests(), 2);
    // The month's embedding USD spent: no batch until next month, and the curators' spend
    // leaves it out.
    db.execute("DELETE FROM provider_calls WHERE role = 'embed'", [])
        .unwrap();
    pdb::record(&db, &row("embed", Some(1.0), crate::embed::CALLS)).unwrap();
    assert_eq!(pdb::usd_this_month(&db).unwrap(), 9.0);
    assert_eq!(pdb::embed_usd_this_month(&db).unwrap(), 1.0);
    let Step::Waiting { until, up } = phase.poll(&s.raw, &k).unwrap() else {
        panic!("the month's USD spent, and a batch went")
    };
    assert_eq!((until, up), (pdb::next_month(), false));
    assert_eq!(stub.requests(), 2);
    // Short of it, a request whose cost would pass it waits for the next UTC day, when the
    // free neurons come back.
    db.execute(
        "UPDATE provider_calls SET usd = 1.0 - 1e-9 WHERE role = 'embed'",
        [],
    )
    .unwrap();
    let used = pdb::Call {
        est_tokens: Some(10_000_000),
        usd: Some(0.0),
        ..row("elsewhere", None, crate::embed::CALLS)
    };
    pdb::record(&db, &used).unwrap();
    let now = crate::db::now_ms();
    let Step::Waiting { until, .. } = phase.poll(&s.raw, &k).unwrap() else {
        panic!("a batch past the month's USD went")
    };
    assert!(until > now && until <= now + pdb::DAY_MS && until % pdb::DAY_MS == 0);
    assert_eq!(stub.requests(), 2);
    // The estimate: free inside the day's 10,000 neurons, USD 0.011 per 1,000 past them.
    let tokens = |neurons: f64| (neurons * 1e6 / NEURONS_PER_M / COUNTED_PER_ESTIMATED) as i64;
    assert_eq!(usd(0, tokens(9_000.0)), 0.0);
    let past = usd(tokens(9_000.0), tokens(2_000.0));
    assert!((past - 0.011).abs() < 1e-4, "{past}");
}

/// Step 7: a request is counted, with what it may cost, before it is sent: a call that never
/// comes back, a process that dies, or a write that fails after it leaves it counted. An
/// answer whose body cannot be used was run, so it is billed as an answer is.
#[test]
fn a_request_is_counted_before_it_is_sent() {
    use crate::providers_db as pdb;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.said("s", R, 1_000, "Words to embed.");
    s.run();
    let home = s.home.path();
    let k = crate::knowledge::open(home).unwrap();
    let db = pdb::open(home).unwrap();
    let rows = || -> Vec<(String, Option<f64>)> {
        db.prepare("SELECT outcome, usd FROM provider_calls WHERE role = 'embed' ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let mut phase = Phase::new(home);
    let held = stub.hold();
    assert!(matches!(
        phase.poll(&s.raw, &k).unwrap(),
        Step::Waiting { .. }
    ));
    while stub.requests() == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(pdb::calls_in_a_day(&db, crate::embed::CALLS).unwrap().0, 1);
    assert_eq!(rows(), [("sent".to_owned(), Some(0.0))]);
    drop(held);
    while !phase.done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    assert_eq!(rows(), [("ok".to_owned(), Some(0.0))]);
    // Past the day's free neurons, an answer of 200 that holds no vectors is billed.
    pdb::record(
        &db,
        &pdb::Call {
            provider: crate::embed::CALLS,
            role: "elsewhere",
            span: "",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: Some(10_000_000),
            usage: pdb::Usage::default(),
            usd: Some(0.0),
        },
    )
    .unwrap();
    s.said("s", R, 2_000, "More words to embed.");
    s.run();
    stub.fail_next(200, None);
    assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
    let last = rows().pop().unwrap();
    assert_eq!(last.0, "error");
    assert!(last.1.is_some_and(|usd| usd > 0.0), "{last:?}");
}

/// Spec 1.7: a restore sets aside a knowledge.db that is not damaged, and the next worker's
/// first poll carries its vectors into the empty cache, those its WAL still held included (a
/// process that died left them there): nothing is embedded again.
#[test]
fn a_restore_keeps_the_vectors_its_wal_held() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let home = s.home.path().to_owned();
    drop(crate::knowledge::open(&home).unwrap());
    // Open across what follows, so no close copies the WAL into the file (Windows renames no
    // open file).
    #[cfg(unix)]
    let held = {
        let held = Connection::open(home.join("knowledge.db")).unwrap();
        held.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
        held
    };
    claim(
        &mut s,
        "Parser caches stay in Redis.",
        "Parser caches go to Redis.",
    );
    s.said("s", R, 3_000, "Words.");
    s.run();
    embed_all(&s);
    let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
    let (sent, before) = (stub.requests(), indexed(&s));
    assert!(sent >= 1 && before >= 2, "{sent} {before}");
    #[cfg(unix)]
    std::mem::forget(held);
    crate::backup::quarantine(&home, "knowledge.db").unwrap();
    s.run();
    embed_all(&s);
    assert_eq!((stub.requests(), indexed(&s)), (sent, before));
}

/// Spec 1.7: a worker that sets knowledge.db aside and opens a new one carries into it too,
/// before the answer of a call in flight is written there, whatever the cap: the same phase
/// serves both files.
#[test]
fn a_worker_carries_into_each_knowledge_db_it_opens() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    claim(
        &mut s,
        "Parser caches stay in Redis.",
        "Parser caches go to Redis.",
    );
    s.said("s", R, 3_000, "Words.");
    s.run();
    let home = s.home.path().to_owned();
    let mut phase = Phase::new(&home);
    let k = crate::knowledge::open(&home).unwrap();
    until_idle(&s.raw, &k, &mut phase);
    let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
    let before = indexed(&s);
    let held = stub.hold();
    s.said("s", R, 4_000, "More words.");
    s.run();
    assert!(matches!(
        phase.poll(&s.raw, &k).unwrap(),
        Step::Waiting { .. }
    ));
    drop(k);
    crate::backup::quarantine(&home, "knowledge.db").unwrap();
    s.run();
    spend_the_day(&home);
    let sent = stub.requests();
    drop(held);
    while !phase.done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let k = crate::knowledge::open(&home).unwrap();
    while phase.poll(&s.raw, &k).unwrap() == Step::Covered {}
    assert_eq!((stub.requests(), indexed(&s)), (sent, before + 1));
}

/// Step 6: while a call waits, each kind's cached vectors are still mapped: one claim to send
/// keeps no imported document or record from the vector its text already has.
#[test]
fn cached_vectors_are_mapped_while_a_call_waits() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.imported("o1", "r", 1_000, "Notes", "Project notes.");
    s.said("s", R, 2_000, "Words.");
    s.run();
    embed_all(&s);
    let sent = stub.requests();
    // A new index over the cache, as after a rebuild, and a claim with no vector yet.
    let k = crate::knowledge::open(s.home.path()).unwrap();
    k.execute_batch("DELETE FROM vector_keys; DELETE FROM vec_index;")
        .unwrap();
    claim(
        &mut s,
        "Parser caches stay in Redis.",
        "Parser caches go to Redis.",
    );
    s.run();
    spend_the_day(s.home.path());
    let mut phase = Phase::new(s.home.path());
    assert!(matches!(
        phase.poll(&s.raw, &k).unwrap(),
        Step::Waiting { .. }
    ));
    assert_eq!(stub.requests(), sent);
    let mapped: Vec<String> = keys(&s)
        .into_iter()
        .filter(|(kind, _, why)| kind != "c" && why.is_none())
        .map(|(kind, ..)| kind)
        .collect();
    assert_eq!(mapped, ["k", "r"]);
}

/// Owner decision 43 (Codex's security review of the switch): a batch composed while
/// redaction is off is not sent once it is on again while the send waits for raw.db or the
/// dispatch guard: the rules are compared after the waits.
#[test]
fn a_batch_composed_under_other_rules_is_not_sent_after_the_waits() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let path = s.home.path().join("config.toml");
    let plain = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{plain}[redaction]\nenabled = false\n")).unwrap();
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
    s.said("s", "github.com/o/r", 1_000, &format!("deploy with {key}"));
    s.run();
    let home = s.home.path().to_owned();
    let k = crate::knowledge::open(&home).unwrap();
    let loaded = crate::config::load(&home).unwrap();
    let embedder = Embedder::from_config(&loaded.embedding).unwrap().unwrap();
    let reading = Reading::now(&s.raw, Reads::Live).unwrap();
    let off = crate::redact::Rules::load(&home).unwrap();
    let batch = pending(&s.raw, &k, &embedder.id, &reading, &off, false)
        .unwrap()
        .unwrap();
    assert!(batch.texts.iter().any(|t| t.contains(&key)));
    let config_home = home.clone();
    WHILE_STILL_WAITS.set(Some(Box::new(move || {
        std::fs::write(config_home.join("config.toml"), &plain).unwrap();
    })));
    let e = still(&home, &batch, true).unwrap_err();
    assert!(e.to_string().contains("redaction rules changed"), "{e:#}");
    assert_eq!(stub.requests(), 0);
}

/// Codex on #489: a claude-mem project imported before a rule that masks its name sends none
/// of its documents once its repository is never recorded under the label the rule makes of
/// it: the document's project is read as the rules now read it, as a hook reads a label.
#[test]
fn an_import_from_before_a_rule_is_not_sent_once_its_repository_is_never_recorded() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let uid = s.imported("o1", "acme-482915", 1_000, "Notes", "Project notes.");
    let path = s.home.path().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
    std::fs::write(&path, format!("{text}{rule}")).unwrap();
    let settings = crate::capture::Settings::load(s.home.path()).unwrap();
    let now = crate::capture::exclusion_label("repo", "github.com/o/acme-482915", &settings);
    s.raw.exclude_capture(("repo", &now), false).unwrap();
    s.run();
    embed_all(&s);
    assert_eq!(skipped(&s, &uid).as_deref(), Some("excluded"));
    assert_eq!(stub.requests(), 0);
}

/// CodeRabbit on #489: an imported document excluded under a rule that masks its project's
/// name is judged again once the rule is taken away, though the list of exclusions reads the
/// same: it gets its vector.
#[test]
fn an_import_excluded_under_a_rule_is_judged_again_when_the_rule_goes() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let uid = s.imported("o1", "acme-482915", 1_000, "Notes", "Project notes.");
    let path = s.home.path().join("config.toml");
    let plain = std::fs::read_to_string(&path).unwrap();
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
    std::fs::write(&path, format!("{plain}{rule}")).unwrap();
    let settings = crate::capture::Settings::load(s.home.path()).unwrap();
    let now = crate::capture::exclusion_label("repo", "github.com/o/acme-482915", &settings);
    s.raw.exclude_capture(("repo", &now), false).unwrap();
    s.run();
    embed_all(&s);
    assert_eq!(skipped(&s, &uid).as_deref(), Some("excluded"));
    std::fs::write(&path, &plain).unwrap();
    s.run();
    embed_all(&s);
    assert_eq!(skipped(&s, &uid), None);
}

/// #496 (owner, 2026-10-13): with the repository stored as given, a rule that needs more than
/// a project's name (`o/acme-…`) keeps the project's imported documents back too: the stored
/// key and the import's project are compared as they are, by D13's name.
#[test]
fn an_import_is_not_sent_under_a_rule_that_reads_a_repository_across_its_owner() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let uid = s.imported("o1", "acme-482915", 1_000, "Notes", "Project notes.");
    let path = s.home.path().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'o/acme-[0-9]{6}' }]\n";
    std::fs::write(&path, format!("{text}{rule}")).unwrap();
    s.raw
        .exclude_capture(("repo", "github.com/o/acme-482915"), false)
        .unwrap();
    s.run();
    embed_all(&s);
    assert_eq!(skipped(&s, &uid).as_deref(), Some("excluded"));
    assert_eq!(stub.requests(), 0);
}

/// Steps 4 and 5 (D13): an `excluded` mark goes whenever its document is written again, its
/// text the same or not, since what excluded it can change under the same text: a uid
/// imported again under another project.
#[test]
fn a_document_written_again_is_judged_again() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let uid = s.imported("o1", "secret", 1_000, "Notes", "Project notes.");
    s.raw.exclude("github.com/o/secret", false).unwrap();
    s.run();
    embed_all(&s);
    assert_eq!(skipped(&s, &uid).as_deref(), Some("excluded"));
    assert_eq!(stub.requests(), 0);
    s.imported("o1", "r", 1_000, "Notes", "Project notes.");
    s.run();
    embed_all(&s);
    assert_eq!(skipped(&s, &uid), None);
    assert_eq!(stub.requests(), 1);
}

/// Rows 55-1 and 55-7: a providers.db that will not open holds back the vectors only: the
/// phase sends nothing and is idle, so the worker goes on with curation and its backups.
#[test]
fn a_providers_db_that_will_not_open_holds_back_only_the_vectors() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.said("s", R, 1_000, "Words to embed.");
    s.run();
    let home = s.home.path();
    let k = crate::knowledge::open(home).unwrap();
    let db = home.join("providers.db");
    if db.exists() {
        std::fs::remove_file(&db).unwrap();
    }
    std::fs::create_dir(&db).unwrap();
    let mut phase = Phase::new(home);
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
    assert_eq!(stub.requests(), 0);
}

/// Milestone 5 D5 (2b): carry takes only the vectors a key of the old file maps, and none that
/// only a forgotten claim's or document's key maps, nor (slice 3c) a forgotten record's; a key
/// that kept no vector maps none, and a file from before the keys carries nothing.
#[test]
fn carry_takes_only_what_a_kept_key_maps() {
    let home = tempfile::tempdir().unwrap();
    let from = home.path().join("old");
    let unit = |i: usize| -> Vec<u8> {
        (0..crate::embed::DIM)
            .flat_map(|d| if d == i { 1.0f32 } else { 0.0 }.to_le_bytes())
            .collect()
    };
    let old = Connection::open(&from).unwrap();
    old.execute_batch(
        "CREATE TABLE vectors(embedder TEXT NOT NULL, src_sha TEXT NOT NULL, vec BLOB NOT NULL,
               PRIMARY KEY (embedder, src_sha)) WITHOUT ROWID;
             CREATE TABLE vector_keys(id INTEGER PRIMARY KEY, embedder TEXT NOT NULL,
               kind TEXT NOT NULL, key TEXT NOT NULL, src_sha TEXT, skipped TEXT,
               UNIQUE (kind, key, embedder));
             INSERT INTO vector_keys(embedder, kind, key, src_sha, skipped) VALUES
               ('e', 'c', 'g', 'gone', NULL), ('e', 'c', 'k', 'kept', NULL),
               ('e', 'r', 'd:1', 'record', NULL), ('e', 'k', 'g', 'shared', NULL),
               ('e', 'p', 'k2', 'shared', NULL), ('e', 'c', 'skipped', NULL, 'too long');",
    )
    .unwrap();
    for (i, sha) in ["gone", "kept", "loose", "record", "shared"]
        .iter()
        .enumerate()
    {
        old.execute(
            "INSERT INTO vectors VALUES ('e', ?1, ?2)",
            params![sha, unit(i)],
        )
        .unwrap();
    }
    drop(old);
    let shas = |k: &Connection| -> Vec<String> {
        k.prepare("SELECT src_sha FROM vectors ORDER BY src_sha")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let mut gone = crate::forget::Forgotten {
        uids: std::collections::HashSet::from(["g".to_owned()]),
        ..Default::default()
    };
    let k = crate::knowledge::open(home.path()).unwrap();
    assert_eq!(carry(&k, &from, &gone).unwrap(), 3);
    assert_eq!(shas(&k), ["kept", "record", "shared"]);
    // A forgotten record's key maps no vector carried.
    let other = tempfile::tempdir().unwrap();
    let fresh = crate::knowledge::open(other.path()).unwrap();
    gone.records.insert("d:1".to_owned());
    assert_eq!(carry(&fresh, &from, &gone).unwrap(), 2);
    assert_eq!(shas(&fresh), ["kept", "shared"]);
    let older = home.path().join("older");
    Connection::open(&older)
        .unwrap()
        .execute_batch("CREATE TABLE vectors(embedder TEXT, src_sha TEXT, vec BLOB)")
        .unwrap();
    assert_eq!(carry(&k, &older, &gone).unwrap(), 0);
}

/// Spec 1.7 (Steps 6 and 9): a rebuild makes no embedding call: the vectors of the
/// knowledge.db it sets aside are carried, and the next polls map them though the day's
/// requests are spent. A quarantined knowledge.db's vectors are carried too, and one whose
/// vectors cannot be read stops a rebuild before anything changes.
#[test]
fn a_rebuild_or_a_quarantine_keeps_the_vectors() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    claim(
        &mut s,
        "Parser caches stay in Redis.",
        "Parser caches go to Redis.",
    );
    s.imported("o1", "r", 2_000, "Deploy notes", "Notes.");
    s.said("s", R, 3_000, "Words.");
    s.run();
    embed_all(&s);
    let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
    let (sent, before) = (stub.requests(), indexed(&s));
    assert!(before >= 4, "{before}");
    let home = s.home.path().to_owned();
    spend_the_day(&home);
    // Blobs that are no vector: never carried.
    let k = crate::knowledge::open(&home).unwrap();
    let ones: Vec<u8> = (0..crate::embed::DIM)
        .flat_map(|_| 1.0f32.to_le_bytes())
        .collect();
    k.execute(
        "INSERT INTO vectors(embedder, src_sha, vec) VALUES ('bge-m3', 'short', x'00'),
               ('bge-m3', 'nan', ?1), ('bge-m3', 'zero', ?2), ('bge-m3', 'long', ?3)",
        params![
            vec![0xffu8; crate::embed::DIM * 4],
            vec![0u8; crate::embed::DIM * 4],
            ones
        ],
    )
    .unwrap();
    // Each mapped by a key, so only the blob's check keeps it back.
    k.execute_batch(
        "INSERT INTO vector_keys(embedder, kind, key, src_sha) VALUES
               ('bge-m3', 'r', 'bad:1', 'short'), ('bge-m3', 'r', 'bad:2', 'nan'),
               ('bge-m3', 'r', 'bad:3', 'zero'), ('bge-m3', 'r', 'bad:4', 'long')",
    )
    .unwrap();
    drop(k);
    // With no store open, as `oboete rebuild` runs.
    let Store { home: dir, raw } = s;
    drop(raw);
    crate::worker::rebuild(&home).unwrap();
    let k = crate::knowledge::open(&home).unwrap();
    let bad: i64 = k
        .query_row(
            "SELECT count(*) FROM vectors WHERE src_sha IN ('short', 'nan', 'zero', 'long')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(k);
    assert_eq!(bad, 0);
    let s = Store {
        home: dir,
        raw: crate::raw::open(&home).unwrap(),
    };
    embed_all(&s);
    assert_eq!((stub.requests(), indexed(&s)), (sent, before));
    let left = std::fs::read_dir(&home).unwrap().any(|e| {
        let name = e.unwrap().file_name();
        name.to_string_lossy()
            .starts_with("knowledge.db.rebuilding-")
    });
    assert!(!left);

    // A damaged page of another table: the worker sets the file aside and carries its vectors.
    let file = home.join("knowledge.db");
    {
        let k = Connection::open(&file).unwrap();
        k.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        let page: i64 = k
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = 'raw_docs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let size: i64 = k.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        drop(k);
        let mut bytes = std::fs::read(&file).unwrap();
        let at = ((page - 1) * size) as usize;
        bytes[at..at + size as usize].fill(0xff);
        std::fs::write(&file, bytes).unwrap();
    }
    s.run();
    let quarantined = std::fs::read_dir(&home).unwrap().any(|e| {
        let name = e.unwrap().file_name();
        name.to_string_lossy()
            .starts_with("knowledge.db.quarantined-")
    });
    assert!(quarantined);
    embed_all(&s);
    assert_eq!((stub.requests(), indexed(&s)), (sent, before));

    // Vectors that cannot be read: the rebuild stops, and the file is back as it was.
    let Store { home: dir, raw } = s;
    drop(raw);
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(home.join(format!("knowledge.db{ext}")));
    }
    std::fs::write(&file, b"not a database").unwrap();
    let err = crate::worker::rebuild(&home).unwrap_err();
    assert!(
        format!("{err:#}").contains("nothing was changed"),
        "{err:#}"
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"not a database");
    let aside = std::fs::read_dir(&home).unwrap().any(|e| {
        let name = e.unwrap().file_name();
        name.to_string_lossy()
            .starts_with("knowledge.db.rebuilding-")
    });
    assert!(!aside);
    drop(dir);
}

/// Spec 1.7: a rebuild stopped after it set knowledge.db aside, and stopped again when rerun,
/// loses no vectors: the next polls carry those of every file it left behind, though the
/// day's requests are spent.
#[test]
fn an_interrupted_rebuild_loses_no_vectors() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.imported("o1", "r", 2_000, "Deploy notes", "Notes.");
    s.said("s", R, 3_000, "Words.");
    s.run();
    embed_all(&s);
    let indexed = |s: &Store| keys(s).iter().filter(|(.., why)| why.is_none()).count();
    let (sent, before) = (stub.requests(), indexed(&s));
    assert!(before >= 2, "{before}");
    let home = s.home.path().to_owned();
    spend_the_day(&home);
    let Store { home: dir, raw } = s;
    drop(raw);
    // Each stopped once the new knowledge.db was made, before the vectors were carried.
    let at = crate::db::now_ms();
    for stamp in [at, at + 1] {
        for ext in ["-wal", "-shm", ""] {
            let from = home.join(format!("knowledge.db{ext}"));
            if from.exists() {
                let to = home.join(format!("knowledge.db.rebuilding-{stamp}{ext}"));
                std::fs::rename(&from, to).unwrap();
            }
        }
        drop(crate::knowledge::open(&home).unwrap());
    }
    crate::worker::rebuild(&home).unwrap();
    let s = Store {
        home: dir,
        raw: crate::raw::open(&home).unwrap(),
    };
    embed_all(&s);
    assert_eq!((stub.requests(), indexed(&s)), (sent, before));
}

/// Step 11: doctor names the embedder and how far it is: what waits per kind, what was passed
/// over and why, the requests and USD against the caps, and the last error.
#[test]
fn doctor_names_the_embedder_and_how_far_it_is() {
    let stub = Stub::start();
    let mut s = Store::new();
    let home = s.home.path().to_owned();
    s.said("s", R, 1_000, "Words to embed.");
    s.said("s", R, 1_500, "<private>all of it</private>");
    s.run();
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!(doctor_lines(&home, &k).unwrap(), ["embeddings: off"]);
    config(&s, &stub);
    let lines = doctor_lines(&home, &k).unwrap();
    assert!(lines[0].contains("(nothing embedded yet)"), "{lines:?}");
    assert!(
        lines[0].ends_with("0 claims, 0 cards, 0 summaries, 0 imported, 2 records"),
        "{lines:?}"
    );
    stub.fail_next(500, None);
    let mut phase = Phase::new(&home);
    assert!(matches!(call(&s, &k, &mut phase), Step::Waiting { .. }));
    let db = crate::providers_db::open(&home).unwrap();
    let none = crate::providers_db::State::default();
    crate::providers_db::set_state(&db, crate::embed::CALLS, none).unwrap();
    embed_all(&s);
    let lines = doctor_lines(&home, &k).unwrap();
    let want = [
        "embeddings: bge-m3 (active), waiting: 0 claims, 0 cards, 0 summaries, 0 imported, 0 records",
        "  passed over: 1 empty",
        "  requests in the last day: 2 of 200 (40 kept for queries); USD this month: 0.00 of 1.00",
    ];
    assert_eq!(lines[..3], want);
    assert!(lines[3].starts_with("  last error (embed, "), "{lines:?}");
    // Task 10: the same vectors on this machine; Workers AI's requests and rests are not its.
    crate::settings::set_embedding_provider(&home, "local").unwrap();
    let lines = doctor_lines(&home, &k).unwrap();
    let want = [
        "embeddings: bge-m3 on this machine (active), waiting: 0 claims, 0 cards, 0 summaries, 0 imported, 0 records",
        "  passed over: 1 empty",
    ];
    assert_eq!(lines, want);
}

#[test]
fn doctor_keeps_only_the_vetted_reason_when_a_curator_shares_the_embedder_name() {
    let stub = Stub::start();
    let s = Store::new();
    s.run();
    config(&s, &stub);
    let home = s.home.path();
    let k = crate::knowledge::open(home).unwrap();
    let db = crate::providers_db::open(home).unwrap();
    db.execute("INSERT INTO provider_calls(ts,provider,role,outcome,ms,detail,bytes_out) VALUES(?1,?2,'curator','error',0,'transport',10)", params![crate::db::now_ms(), crate::embed::CALLS]).unwrap();
    crate::providers_db::freeze_unmetered(&db, db.last_insert_rowid(), [123.0, 456.0]).unwrap();
    let shown = doctor_lines(home, &k).unwrap().join("\n");
    assert!(shown.contains("last error (curator,"), "{shown}");
    assert!(shown.ends_with(": transport"), "{shown}");
    assert!(
        !shown.contains("oboete-budget-v1") && !shown.contains("oboete-call-v1"),
        "{shown}"
    );
    assert_eq!(stub.requests(), 0);
}

/// Rows 55-1 and 55-7: a call the embedder holds delays only the vectors: the worker indexes
/// what is recorded meanwhile and full-text search finds it, and it does not spin while the
/// call is out.
#[test]
fn a_held_embedding_call_delays_only_the_vectors() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let first = s.said("s", R, 1_000, "First words.");
    let held = stub.hold();
    let home = s.home.path().to_owned();
    let mut phase = Phase::new(&home);
    let second = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let phases = crate::worker::Phases {
                embed: Some(&mut phase),
                ..crate::worker::Phases::default()
            };
            let consumers = crate::worker::consumers(&home);
            crate::worker::run_holding(&home, 200, consumers, || {}, None, phases)
        });
        while stub.requests() == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        let second = s.said("s", R, 2_000, "Second words while it waits.");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let q = crate::search::b::Query {
                text: "Second words".into(),
                caller: Some(R.into()),
                limit: 5,
                ..Default::default()
            };
            let found = crate::search::b::query(&home, &q).unwrap();
            if found.hits.iter().any(|h| h.key == s.key(second)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "not indexed while the call was out"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(1_000));
        drop(held);
        worker.join().unwrap().unwrap();
        second
    });
    assert!(phase.polls < 30, "{} polls", phase.polls);
    for seq in [first, second] {
        assert!(
            keys(&s)
                .iter()
                .any(|(_, k, why)| *k == s.key(seq) && why.is_none())
        );
    }
}

/// D6 and A92 (Step 5): records of imported sources get no vector, and however many there
/// are they never hold up a live record behind them: one poll marks them `source`, page after
/// page, and sends the live one. A text the gate leaves empty is marked `empty`.
#[test]
fn records_of_imported_sources_and_empty_texts_are_passed_over() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.said("s", R, 1_000, "Live words.");
    let empty = s.said("s", R, 1_500, "<private>all of it</private>");
    let many = 3 * PAGE + 50;
    for i in 0..many as i64 {
        let body = serde_json::json!({ "prompt": format!("Old words {i}.") }).to_string();
        let e = crate::raw::Event {
            source: "transcript".into(),
            session: "t".into(),
            repo: Some(R.into()),
            ts: 2_000 + i,
            ..crate::raw::test_event(&body)
        };
        s.raw.append(&e).unwrap();
    }
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = Phase::new(s.home.path());
    assert!(matches!(
        phase.poll(&s.raw, &k).unwrap(),
        Step::Waiting { .. }
    ));
    while !phase.done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    assert_eq!(stub.texts(), [vec!["Live words.".to_owned()]]);
    // Its words are all private: blanked by the rules, and judged again when they change.
    let why = skipped(&s, &s.key(empty)).unwrap();
    assert!(why.starts_with("masked:"), "{why}");
    let source = keys(&s)
        .iter()
        .filter(|(.., why)| why.as_deref() == Some("source"))
        .count();
    assert_eq!(source, many);
}

/// Tool output gets no vector (#317, decision 42). It is not queued, before the queue is made
/// or after; one a home made before had queued leaves the queue unsent; and it does not count
/// as waiting for one. The record beside it is embedded.
#[test]
fn tool_output_gets_no_vector() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    s.said("s", R, 1_000, "Live words.");
    let body = serde_json::json!({"tool": "Bash", "input": "cargo test", "output": "Tool words."});
    let before = s.event("tool", "s", (R, "main"), 2_000, body.clone());
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = Phase::new(s.home.path());
    assert!(matches!(
        phase.poll(&s.raw, &k).unwrap(),
        Step::Waiting { .. }
    ));
    while !phase.done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    let after = s.event("tool", "s", (R, "main"), 3_000, body);
    s.run();
    let queued = |key: &str| -> i64 {
        k.query_row(
            "SELECT count(*) FROM vector_todo WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!((queued(&s.key(before)), queued(&s.key(after))), (0, 0));
    k.execute(
        "INSERT INTO vector_todo(family, key, ord) VALUES ('r', ?1, ?2)",
        params![s.key(before), before],
    )
    .unwrap();
    // Nothing to send: the queued tool output leaves the queue.
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Idle);
    assert_eq!(queued(&s.key(before)), 0);
    assert_eq!(stub.texts(), [vec!["Live words.".to_owned()]]);
    let tools = [s.key(before), s.key(after)];
    assert!(keys(&s).iter().all(|(_, k, _)| !tools.contains(k)));
    let facts = doctor_facts_in(&k, crate::embed::EMBEDDER);
    assert_eq!(facts.records.unwrap(), 0);
}

/// A home whose phase ran before tool output stayed out (#317): its trigger queued tool records
/// and its index held their vectors. The next poll takes both out and makes the trigger anew,
/// and the prompt beside them keeps its vector (Codex on #421).
#[test]
fn a_home_from_before_loses_its_queued_tool_output_and_its_vectors() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let prompt = s.said("s", R, 1_000, "Live words.");
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = Phase::new(s.home.path());
    phase.poll(&s.raw, &k).unwrap();
    while !phase.done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    k.execute_batch(
        "DROP TRIGGER vector_todo_record;
             CREATE TRIGGER vector_todo_record AFTER INSERT ON raw_docs
               WHEN NOT EXISTS (SELECT 1 FROM vector_keys
                                WHERE kind = 'r' AND key = NEW.device || ':' || NEW.seq)
             BEGIN
               INSERT OR IGNORE INTO vector_todo(family, key, ord)
                 VALUES ('r', NEW.device || ':' || NEW.seq, NEW.seq);
             END;",
    )
    .unwrap();
    let body = serde_json::json!({"tool": "Bash", "input": "cargo test", "output": "Tool words."});
    let tools = [
        s.event("tool", "s", (R, "main"), 2_000, body.clone()),
        s.event("tool", "s", (R, "main"), 3_000, body),
    ];
    s.run();
    let tools = tools.map(|seq| s.key(seq));
    let held = Doc {
        kind: "r",
        key: tools[1].clone(),
        sha: "from before".into(),
        repo: String::new(),
        ts: 3_000,
        session: "s".into(),
    };
    index(
        &k,
        crate::embed::EMBEDDER,
        &held,
        &[0u8; crate::embed::DIM * 4],
    )
    .unwrap();
    let count = |sql: &str, key: &str| -> i64 { k.query_row(sql, [key], |r| r.get(0)).unwrap() };
    let queued = |key: &str| count("SELECT count(*) FROM vector_todo WHERE key = ?1", key);
    let keyed = |key: &str| count("SELECT count(*) FROM vector_keys WHERE key = ?1", key);
    let indexed = |key: &str| {
        count(
            "SELECT count(*) FROM vec_index WHERE rowid IN
                   (SELECT id FROM vector_keys WHERE key = ?1)",
            key,
        )
    };
    assert_eq!(queued(&tools[0]), 1, "the old trigger queues tool output");
    assert_eq!((keyed(&tools[1]), indexed(&tools[1])), (1, 1));
    phase.poll(&s.raw, &k).unwrap();
    for key in &tools {
        assert_eq!((queued(key), keyed(key)), (0, 0), "{key}");
    }
    let left: i64 = k
        .query_row("SELECT count(*) FROM vec_index", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 1, "only the prompt's vector");
    assert_eq!(keyed(&s.key(prompt)), 1);
    let later = s.event(
        "tool",
        "s",
        (R, "main"),
        4_000,
        serde_json::json!({"tool": "Bash", "input": "ls", "output": "More."}),
    );
    s.run();
    assert_eq!(queued(&s.key(later)), 0, "the trigger is made anew");
}

/// Rows 30-1 and 30-14: what reaches the embedder has passed the gate: a token and a
/// `<private>` block in a claim, an import op and a record, and a secret across a prompt's
/// cut is hidden whole, not cut in two.
#[test]
fn texts_sent_to_the_embedder_pass_the_gate() {
    let token = ["gh", "p_q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"].concat();
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    // A curator's body can hold what its quote does not: the rescan masks a secret in raw,
    // so a quote of one would no longer read there.
    let uid = claim(
        &mut s,
        "Deploy with the token always.",
        &format!("Deploy with {token} <private>for acme</private> always."),
    );
    // An import is not rescanned, so its token reaches the phase, here across the cut: the
    // composed text is "decision: Deploy notes\n" and the body, and the cut keeps 12,000.
    let pad = crate::embed::MAX_CHARS - "decision: Deploy notes\n".len() - 15;
    let body = format!(
        "<private>acme</private>{} {token} tail",
        "x".repeat(pad - 23)
    );
    let imported = s.imported("o1", "r", 2_000, "Deploy notes", &body);
    let seq = s.said("s", R, 3_000, "<private>acme corp</private> Deploy words.");
    s.run();
    embed_all(&s);
    let sent: Vec<String> = stub.texts().concat();
    assert!(!sent.is_empty());
    for text in &sent {
        assert!(!text.contains(&token[..12]), "{text}");
        assert!(!text.contains("acme"), "{text}");
    }
    let got = keys(&s);
    for key in [uid, imported, s.key(seq)] {
        assert!(
            got.iter()
                .any(|(_, k, skipped)| *k == key && skipped.is_none()),
            "{key}: {got:?}"
        );
    }
}

/// The ranges `composed_out` gates alone are the title and the body in the composed text.
#[test]
fn composed_parts_name_the_title_and_the_body() {
    let (text, parts) = composed_parts("decision", "Tabs", "Use tabs.\nAlways.");
    assert_eq!(text, composed("decision", "Tabs", "Use tabs.\nAlways."));
    assert_eq!(&text[parts[0].clone()], "Tabs");
    assert_eq!(&text[parts[1].clone()], "Use tabs.\nAlways.");
    for kind in ["prompt", "summary"] {
        assert_eq!(composed_parts(kind, "T", "B"), ("B".to_owned(), Vec::new()));
    }
}

/// Codex on 52803e3: a rule anchored to an imported title (`^...$`), added after the import,
/// holds in the text sent as in search's hits: the title is gated alone before its kind is
/// prefixed. The rules are the process's, so the phase runs in a child.
#[test]
fn a_rule_anchored_to_an_imported_title_holds_in_the_text_sent() {
    const HOME: &str = "OBOETE_TEST_TITLE_RULE_HOME";
    const SECRET: &str = "INTERNAL-GAMMA-3";
    const OTHER: &str = "INTERNAL-DELTA-5";
    if let Ok(home) = std::env::var(HOME) {
        let home = std::path::PathBuf::from(home);
        crate::redact::set_home(&home).unwrap();
        let raw = crate::raw::open(&home).unwrap();
        let k = crate::knowledge::open(&home).unwrap();
        until_idle(&raw, &k, &mut Phase::new(&home));
        return;
    }
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let uid = s.imported("o1", "r", 2_000, SECRET, "deploy notes");
    // Codex on 6c19081: a rule on the whole composed text keeps its context though the
    // title's own rule masks the title.
    let other = s.imported("o2", "r", 2_000, OTHER, "private deployment value");
    s.run();
    let config = s.home.path().join("config.toml");
    let plain = std::fs::read_to_string(&config).unwrap();
    let ruled = format!(
        "{plain}[redaction]\nextra_rules = [{{ id = \"gamma\", regex = '^{SECRET}$' }}, \
             {{ id = \"delta\", regex = '^{OTHER}$' }}, \
             {{ id = \"whole\", regex = '(?s)^decision: {OTHER}\\n.*$' }}]\n"
    );
    std::fs::write(&config, ruled).unwrap();
    let name = "embed_phase::tests::a_rule_anchored_to_an_imported_title_holds_in_the_text_sent";
    let out = std::process::Command::new(std::env::args_os().next().unwrap())
        .args(["--exact", name])
        .env(HOME, s.home.path())
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{said}");
    assert!(said.contains("1 passed"), "{said}");
    let sent: Vec<String> = stub.texts().concat();
    assert!(sent.iter().any(|t| t.contains("deploy notes")), "{sent:?}");
    assert!(sent.iter().all(|t| !t.contains(SECRET)), "{sent:?}");
    assert!(sent.iter().all(|t| !t.contains("private")), "{sent:?}");
    let got = keys(&s);
    assert!(
        got.iter()
            .any(|(_, k, skipped)| *k == uid && skipped.is_none())
    );
    // Gated to the mask whole: nothing to send under these rules.
    assert!(
        got.iter().any(|(_, k, skipped)| *k == other
            && skipped.as_deref().is_some_and(|w| w.starts_with("masked:"))),
        "{got:?}"
    );
}

/// Row 55-6: a correction or a tombstone that lands while a call is out keeps the vector made
/// from the old text off its document; the next batch embeds the new text.
#[test]
fn a_vector_is_never_kept_for_text_that_changed_while_it_was_embedded() {
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let uid = s.decided(R, 1_000, "Parser caches stay in Redis.", &[]);
    let text = "Deploy words to mask.";
    let seq = s.said("s", R, 2_000, text);
    s.run();
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let mut phase = Phase::new(s.home.path());
    let mapped = |k: &Connection, key: &str| -> Option<String> {
        k.query_row(
            "SELECT src_sha FROM vector_keys WHERE key = ?1 AND skipped IS NULL",
            [key],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
    };
    let body = serde_json::json!({ "prompt": text }).to_string();
    let changes: [&dyn Fn(&mut Store); 2] = [
        &|s| {
            crate::claims::correct(s.home.path(), &uid, None, Some("Caches go to files.")).unwrap()
        },
        &|s| {
            let target = crate::raw::Target::Range {
                device: s.raw.device().to_owned(),
                seq,
                offset: body.find("mask").unwrap() as i64,
                length: 4,
            };
            s.raw.append_tombstone(target).unwrap();
        },
    ];
    for (call, (change, key)) in changes.iter().zip([uid.clone(), s.key(seq)]).enumerate() {
        let held = stub.hold();
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while stub.requests() == call {
            std::thread::sleep(Duration::from_millis(5));
        }
        change(&mut s);
        s.run();
        drop(held);
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        assert_eq!(mapped(&k, &key), None, "{key}");
    }
    embed_all(&s);
    let texts = stub.texts().concat();
    assert!(
        texts.iter().any(|t| t == "Caches go to files."),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "Deploy words to ****."),
        "{texts:?}"
    );
    assert_eq!(mapped(&k, &uid), Some(sha("Caches go to files.")));
    assert!(mapped(&k, &s.key(seq)).is_some());
}

/// #843: a prompt an older oboete embedded with its entry point (`{"entrypoint":"cli"}`, #659)
/// as a line of its text gets the vector of its words once its row is indexed again: the
/// embedder is sent the words, never the entry point.
#[test]
fn a_prompt_embedded_with_its_entry_point_gets_the_vector_of_its_words() {
    use crate::knowledge::checkpoint;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let body = serde_json::json!({"prompt": "Fix the parser.", "entrypoint": "cli"});
    let seq = s.event("prompt", "s", (R, "main"), 1_000, body);
    s.run();
    embed_all(&s);
    let (key, device) = (s.key(seq), s.raw.device().to_owned());
    let k = crate::knowledge::open(s.home.path()).unwrap();
    let words = sha("Fix the parser.");
    // As an older oboete left it: the key's vector made of the text with the entry point, none
    // of the words alone, and no catch-up begun.
    k.execute(
        "UPDATE vector_keys SET src_sha = ?1 WHERE kind = 'r' AND key = ?2",
        params![sha("Fix the parser.\ncli"), key],
    )
    .unwrap();
    k.execute("DELETE FROM vectors WHERE src_sha = ?1", [&words])
        .unwrap();
    checkpoint::set_in(&k, checkpoint::SEQS, "fts-text", &device, 0).unwrap();
    s.run();
    embed_all(&s);
    let got: Option<String> = k
        .query_row(
            "SELECT src_sha FROM vector_keys WHERE kind = 'r' AND key = ?1 AND skipped IS NULL",
            [&key],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(got, Some(words));
    let sent = stub.texts().concat();
    let times = sent.iter().filter(|t| *t == "Fix the parser.").count();
    assert_eq!(times, 2, "{sent:?}");
    assert!(!sent.iter().any(|t| t.contains("cli")), "{sent:?}");
}

/// Row 46-2: every way a document's stored text changes takes its vector off, and the next
/// polls give it the vector of the text it has now (embedded, or mapped when that text was
/// embedded before): a re-derivation, a correction, a tombstone's mask and its removal, a
/// second import of a uid, and the three rewinds. A status-only correction sends nothing, and
/// the same text imported again only moves its index row's time.
#[test]
fn every_way_a_documents_text_changes_replaces_its_vector() {
    use crate::consumer::{claims::Claims, fts::Fts, imported::Imported};
    use crate::worker::Consumer;
    let stub = Stub::start();
    let mut s = Store::new();
    config(&s, &stub);
    let quote = "Parser caches stay in Redis.";
    let seq = s.said("s", R, 1_000, quote);
    let uid = derive(&mut s, seq, quote, quote);
    let masked = s.said("s", R, 2_000, "Deploy words to mask.");
    let removed = s.said("s", R, 3_000, "Words to forget.");
    let doc = s.imported("o1", "r", 4_000, "Deploy notes", "First notes.");
    let (home, device) = (s.home.path().to_owned(), s.raw.device().to_owned());
    let k = crate::knowledge::open(&home).unwrap();
    // The key's vector, if any, is the one of its stored text now.
    let follows = |s: &Store, kind: &str, key: &str| {
        s.run();
        embed_all(s);
        let now = current(Some(&s.raw), &k, kind, key)
            .unwrap()
            .map(|r| r.doc.sha);
        let got: Option<String> = k
            .query_row(
                "SELECT src_sha FROM vector_keys WHERE key = ?1 AND skipped IS NULL",
                [key],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(got, now, "{key}");
    };
    follows(&s, "c", &uid);
    let first = |sql: &str, key: &str| -> i64 { k.query_row(sql, [key], |r| r.get(0)).unwrap() };
    let claim_op = first("SELECT MIN(op_seq) FROM derivations WHERE uid = ?1", &uid);
    let import_op = first("SELECT MIN(op_seq) FROM imported WHERE uid = ?1", &doc);

    derive(&mut s, seq, quote, "Parser caches go to disk.");
    follows(&s, "c", &uid);
    assert!(
        current(None, &k, "c", &uid).unwrap().unwrap().doc.sha == sha("Parser caches go to disk.")
    );
    crate::claims::correct(&home, &uid, None, Some("Caches go to files.")).unwrap();
    follows(&s, "c", &uid);
    let sent = stub.requests();
    crate::claims::correct(&home, &uid, Some("done"), None).unwrap();
    follows(&s, "c", &uid);
    assert_eq!(stub.requests(), sent);

    let body = serde_json::json!({ "prompt": "Deploy words to mask." }).to_string();
    s.raw
        .append_tombstone(crate::raw::Target::Range {
            device: device.clone(),
            seq: masked,
            offset: body.find("mask").unwrap() as i64,
            length: 4,
        })
        .unwrap();
    follows(&s, "r", &s.key(masked));
    assert!(
        stub.texts()
            .concat()
            .iter()
            .any(|t| t == "Deploy words to ****.")
    );
    s.raw
        .append_tombstone(crate::raw::Target::Record {
            device: device.clone(),
            seq: removed,
        })
        .unwrap();
    follows(&s, "r", &s.key(removed));

    s.imported("o1", "r", 4_000, "Deploy notes", "Second notes.");
    follows(&s, "k", &doc);
    let sent = stub.requests();
    s.imported("o1", "r", 5_000, "Deploy notes", "Second notes.");
    follows(&s, "k", &doc);
    assert_eq!(stub.requests(), sent);
    let id = first("SELECT id FROM vector_keys WHERE key = ?1", &doc);
    let ts: i64 = k
        .query_row("SELECT ts FROM vec_index WHERE rowid = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(ts, 5_000);

    // A restore that lost the later ops and records: each consumer's rewind. The texts the
    // claim and the import had before were embedded then, so they are mapped with no call.
    Imported.rewind(&k, &device, import_op).unwrap();
    follows(&s, "k", &doc);
    Claims.rewind(&k, &device, claim_op).unwrap();
    follows(&s, "c", &uid);
    assert_eq!(stub.requests(), sent);
    Fts.rewind(&k, &device, masked - 1).unwrap();
    follows(&s, "r", &s.key(masked));
    // The record the claim quotes goes, and the claim with it.
    s.raw
        .append_tombstone(crate::raw::Target::Record { device, seq })
        .unwrap();
    follows(&s, "c", &uid);
    assert!(current(None, &k, "c", &uid).unwrap().is_none());
    let (index, kept): (i64, i64) = k
        .query_row(
            "SELECT (SELECT count(*) FROM vec_index),
                        (SELECT count(*) FROM vector_keys WHERE skipped IS NULL)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(index, kept);
}

/// Task 10: a home whose embedder is the local model, which loads as `how` says
/// (`embed::stub::local`).
fn local(s: &Store, how: &str) {
    let config = "[embedding]\nprovider = \"local\"\n";
    std::fs::write(s.home.path().join("config.toml"), config).unwrap();
    crate::embed::stub::local(s.home.path(), how);
}

fn indexed_count(k: &Connection) -> i64 {
    k.query_row("SELECT count(*) FROM vec_index", [], |r| r.get(0))
        .unwrap()
}

/// D14: the worker loads the local model once 20 documents wait, of every kind together (a
/// fresh claim's batch comes first), or one has waited 10 minutes; embeds them, and drops it
/// when none wait. A smaller, newer backlog is no work that keeps the worker up.
#[test]
fn the_worker_loads_past_the_threshold_and_drops_when_none_pend() {
    let mut s = Store::new();
    let now = crate::db::now_ms();
    for i in 0..5 {
        s.said("s", R, now, &format!("Fresh words {i}."));
    }
    s.run();
    local(&s, "0");
    let home = s.home.path().to_owned();
    let k = crate::knowledge::open(&home).unwrap();
    let mut phase = Phase::new(&home);
    let step = phase.poll(&s.raw, &k).unwrap();
    let waits = matches!(step, Step::Waiting { until, up: false } if until >= now + LOAD_AFTER_MS);
    assert!(waits, "{step:?}");
    let loads = || crate::embed::stub::local_loads(&home);
    assert_eq!((loads(), indexed_count(&k)), (0, 0));
    // The claim and the record it quotes, and 13 more records: 20 in all.
    s.decided(R, now, "A fresh claim goes first.", &[]);
    for i in 0..13 {
        s.said("s", R, now, &format!("More words {i}."));
    }
    s.run();
    until_idle(&s.raw, &k, &mut phase);
    assert_eq!((loads(), indexed_count(&k)), (1, 20));
    assert!(
        phase.local.is_none(),
        "the model stayed with nothing to embed"
    );
    s.said("s", R, now - LOAD_AFTER_MS, "Words that waited.");
    s.run();
    until_idle(&s.raw, &k, &mut phase);
    assert_eq!((loads(), indexed_count(&k)), (2, 21));
}

/// Task 10: without the model (none placed, a load that fails, a vector the index will not
/// take) nothing is indexed or marked and the documents wait. A failure rests the model in
/// providers.db, as a failed call rests Workers AI, so a fresh worker waits too; doctor says
/// so. The model back, what waited is embedded.
#[test]
fn a_missing_or_changed_model_leaves_the_phase_waiting() {
    use crate::providers_db as pdb;
    let mut s = Store::new();
    s.said("s", R, 1_000, "Words to embed.");
    s.run();
    local(&s, "0");
    let home = s.home.path().to_owned();
    std::fs::remove_file(crate::embed::local_dir(&home).join("stub")).unwrap();
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!(Phase::new(&home).poll(&s.raw, &k).unwrap(), Step::Idle);
    let waiting = doctor_lines(&home, &k).unwrap();
    assert!(waiting[0].ends_with("waiting: 0 claims, 0 cards, 0 summaries, 0 imported, 1 records"));
    for how in ["fail", "nan"] {
        crate::embed::stub::local(&home, how);
        let mut phase = Phase::new(&home);
        assert!(matches!(
            phase.poll(&s.raw, &k).unwrap(),
            Step::Waiting { .. }
        ));
        while !phase.done() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
        let fresh = Phase::new(&home).poll(&s.raw, &k).unwrap();
        let rests = matches!(fresh, Step::Waiting { until, .. } if until > crate::db::now_ms());
        assert!(rests, "{how}: {fresh:?}");
        assert_eq!(indexed_count(&k), 0, "{how}");
        let lines = doctor_lines(&home, &k).unwrap();
        assert!(
            lines[1].starts_with("  local model resting until"),
            "{lines:?}"
        );
        let db = pdb::open(&home).unwrap();
        pdb::set_state(&db, LOCAL, pdb::State::default()).unwrap();
    }
    crate::embed::stub::local(&home, "0");
    embed_all(&s);
    assert_eq!(indexed_count(&k), 1);
}

/// Codex on #426: while the local model rests after a failure, cached vectors are still
/// mapped and no model is loaded, as while a Workers AI call waits.
#[test]
fn cached_vectors_are_mapped_while_the_local_model_rests() {
    use crate::providers_db as pdb;
    let mut s = Store::new();
    s.imported("o1", "r", 1_000, "Notes", "Project notes.");
    s.said("s", R, 2_000, "Words.");
    s.run();
    local(&s, "0");
    embed_all(&s);
    let home = s.home.path().to_owned();
    let loads = crate::embed::stub::local_loads(&home);
    // A new index over the cache, as after a rebuild, and a claim with no vector yet, while
    // the model rests.
    let k = crate::knowledge::open(&home).unwrap();
    k.execute_batch("DELETE FROM vector_keys; DELETE FROM vec_index;")
        .unwrap();
    claim(
        &mut s,
        "Parser caches stay in Redis.",
        "Parser caches go to Redis.",
    );
    s.run();
    let rest = pdb::State {
        down_until: crate::db::now_ms() + 60_000,
        ..pdb::State::default()
    };
    pdb::set_state(&pdb::open(&home).unwrap(), LOCAL, rest).unwrap();
    let step = Phase::new(&home).poll(&s.raw, &k).unwrap();
    assert!(matches!(step, Step::Waiting { .. }), "{step:?}");
    assert_eq!(crate::embed::stub::local_loads(&home), loads);
    let mapped: Vec<String> = keys(&s)
        .into_iter()
        .filter(|(kind, _, why)| kind != "c" && why.is_none())
        .map(|(kind, ..)| kind)
        .collect();
    assert_eq!(mapped, ["k", "r"]);
    assert_eq!(indexed_count(&k), 2);
}

/// Task 10: the local model calls no Workers AI and reads no key, Workers AI's settings left
/// in place; with its key, Workers AI gets the next document (the control).
#[test]
fn a_local_run_calls_no_workers_ai_and_reads_no_key() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.said("s", R, 1_000, "Words to embed.");
    s.run();
    let home = s.home.path().to_owned();
    let text = format!(
        "[embedding]\nprovider = \"local\"\naccount_id = \"a\"\nkey_file = '{}'\nurl = \"{}\"\n",
        home.join("no-key.md").display(),
        stub.url
    );
    std::fs::write(home.join("config.toml"), text).unwrap();
    crate::embed::stub::local(&home, "0");
    embed_all(&s);
    let k = crate::knowledge::open(&home).unwrap();
    assert_eq!((stub.requests(), indexed_count(&k)), (0, 1));
    config(&s, &stub);
    s.said("s", R, 2_000, "More words.");
    s.run();
    embed_all(&s);
    assert_eq!((stub.requests(), indexed_count(&k)), (1, 2));
}

/// Row 46-2: both runners make the same model's vectors, so switching between them queues
/// nothing and loads nothing; each embeds only what is new.
#[test]
fn switching_runners_queues_nothing() {
    let stub = Stub::start();
    let mut s = Store::new();
    s.said("s", R, 1_000, "Parser words.");
    s.said("s", R, 2_000, "Deploy words.");
    s.run();
    config(&s, &stub);
    embed_all(&s);
    let home = s.home.path().to_owned();
    let k = crate::knowledge::open(&home).unwrap();
    local(&s, "0");
    assert_eq!(Phase::new(&home).poll(&s.raw, &k).unwrap(), Step::Idle);
    assert_eq!(crate::embed::stub::local_loads(&home), 0);
    s.said("s", R, 3_000, "Parser words again.");
    s.run();
    embed_all(&s);
    assert_eq!(
        (crate::embed::stub::local_loads(&home), indexed_count(&k)),
        (1, 3)
    );
    config(&s, &stub);
    assert_eq!(Phase::new(&home).poll(&s.raw, &k).unwrap(), Step::Idle);
    assert_eq!(stub.requests(), 1);
}

/// Row 55-6 on the local model: a text corrected while the model embeds it keeps no vector,
/// and the next batch embeds the new text.
#[test]
fn the_local_model_keeps_no_vector_of_text_that_changed() {
    let mut s = Store::new();
    let uid = s.decided(R, 1_000, "Parser caches stay in Redis.", &[]);
    s.run();
    local(&s, "300");
    let home = s.home.path().to_owned();
    let k = crate::knowledge::open(&home).unwrap();
    let mut phase = Phase::new(&home);
    assert!(matches!(
        phase.poll(&s.raw, &k).unwrap(),
        Step::Waiting { .. }
    ));
    crate::claims::correct(&home, &uid, None, Some("Caches go to files.")).unwrap();
    s.run();
    while !phase.done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(phase.poll(&s.raw, &k).unwrap(), Step::Covered);
    let kept = |k: &Connection| -> i64 {
        k.query_row(
            "SELECT count(*) FROM vector_keys WHERE kind = 'c' AND skipped IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(kept(&k), 0);
    until_idle(&s.raw, &k, &mut phase);
    assert_eq!(kept(&k), 1);
}

/// Row 30-1 on the local model: a session that touched an excluded repository reaches no
/// embedder, the local one included (D13); the rest is embedded.
#[test]
fn an_excluded_repositorys_records_reach_no_local_model() {
    let mut s = Store::new();
    let there = s.said("sx", "github.com/o/secret", 1_000, "Secret words.");
    let open = s.said("so", R, 2_000, "Open words.");
    s.raw.exclude("github.com/o/secret", false).unwrap();
    s.run();
    local(&s, "0");
    embed_all(&s);
    assert_eq!(skipped(&s, &s.key(there)).as_deref(), Some("excluded"));
    assert_eq!(skipped(&s, &s.key(open)), None);
    let k = crate::knowledge::open(s.home.path()).unwrap();
    assert_eq!(indexed_count(&k), 1);
}
