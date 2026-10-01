//! Milestone 4 Task 8 (D9): the shortlist phase. While `[inject] per_prompt` is on, the worker
//! keeps, for each of this device's live sessions, the delivered claims the prompt hook picks
//! from: at most `SHORT` per key (agent, session and the checkout of its last event), ranked by
//! `search::b::delivered_ranked` over the session's last prompts, the files it touched and its
//! failing command, each gated with the rules as they are now. A key is built when it has no row,
//! after a reply, every `EVERY_EVENTS` events and after a rewind. The search runs outside every
//! transaction, and only its result is written, in one short one.

use crate::consumer::manifest::{ACTIVE_MS, event, exists, what_ran};
use crate::curate::Phase;
use crate::raw::Raw;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The claims a key keeps (D9).
pub const SHORT: usize = 50;
/// A key is built again once its session has this many events since its build (D9).
pub const EVERY_EVENTS: i64 = 20;
/// Keys built per call (D9).
const KEYS: usize = 8;
/// The session's prompts a key's query reads.
const PROMPTS: i64 = 3;
/// The session's files a key's query reads, the newest first (the manifest's `FILES`).
const FILES: i64 = 10;

/// Made when `per_prompt` is first on: a home that never asks for it gets no table.
fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS shortlists(
           agent TEXT NOT NULL, session TEXT NOT NULL, repo TEXT NOT NULL, branch TEXT NOT NULL,
           -- The session's last event the build read.
           built_seq INTEGER NOT NULL,
           PRIMARY KEY (agent, session, repo, branch)
         );
         CREATE TABLE IF NOT EXISTS shortlist(
           agent TEXT NOT NULL, session TEXT NOT NULL, repo TEXT NOT NULL, branch TEXT NOT NULL,
           rank INTEGER NOT NULL, uid TEXT NOT NULL,
           PRIMARY KEY (agent, session, repo, branch, rank)
         );
         -- The live sessions, by time across the device's repositories.
         CREATE INDEX IF NOT EXISTS manifest_facts_live ON manifest_facts(device, fact, ts);",
    )?;
    Ok(())
}

/// A live session on the checkout of its last event, and that event's seq.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    agent: String,
    session: String,
    repo: String,
    branch: String,
    last: i64,
}

pub struct Builder {
    home: PathBuf,
}

impl Builder {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
        }
    }

    /// One call at `now`: the rows of keys no longer live dropped, and up to `KEYS` keys that are
    /// due built; `Covered` when one was. Idle while `per_prompt` is off, or settings that do not
    /// load turn it off, and before the worker has made the claims and the manifest's facts.
    pub fn run(&mut self, raw: &Raw, k: &mut Connection, now: i64) -> Result<Phase> {
        let on = crate::config::inject(&self.home)
            .inspect_err(|e| eprintln!("oboete: no shortlist for now: {e:#}"))
            .is_ok_and(|i| i.per_prompt);
        if !on || !exists(k, "table", "manifest_facts")? || !exists(k, "view", "active")? {
            return Ok(Phase::Idle);
        }
        schema(k)?;
        let rules = crate::capture::Settings::load(&self.home)
            .map(|s| s.rules)
            .unwrap_or_default();
        let device = raw.device();
        let live = keys(raw, k, now)?;
        let mut built = Vec::new();
        for key in &live {
            if built.len() == KEYS {
                break;
            }
            if due(k, device, key)? {
                let texts = parts(raw, k, key, &rules)?;
                let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
                let claims =
                    crate::search::b::delivered_ranked(raw, k, &texts, None, &key.repo, SHORT)?;
                built.push((key, claims));
            }
        }
        // A half-rebuilt `manifest_facts` (after a rewind) looks as if every session ended.
        let rebuilding = k
            .query_row(
                "SELECT 1 FROM manifest_rebuild WHERE device = ?1",
                [device],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let tx = k.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut dropped = 0;
        if !rebuilding {
            let rows: Vec<(String, String, String, String)> = tx
                .prepare("SELECT agent, session, repo, branch FROM shortlists")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (agent, session, repo, branch) in rows {
                let gone = !live.iter().any(|l| {
                    l.agent == agent && l.session == session && l.repo == repo && l.branch == branch
                });
                if gone {
                    for table in ["shortlists", "shortlist"] {
                        tx.execute(
                            &format!(
                                "DELETE FROM {table}
                                 WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4"
                            ),
                            params![agent, session, repo, branch],
                        )?;
                    }
                    dropped += 1;
                }
            }
        }
        for (key, claims) in &built {
            let at = params![key.agent, key.session, key.repo, key.branch];
            tx.execute(
                "DELETE FROM shortlist
                 WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4",
                at,
            )?;
            for (rank, c) in claims.iter().enumerate() {
                tx.execute(
                    "INSERT INTO shortlist(agent, session, repo, branch, rank, uid)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        key.agent,
                        key.session,
                        key.repo,
                        key.branch,
                        rank as i64,
                        c.uid
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO shortlists(agent, session, repo, branch, built_seq)
                 VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(agent, session, repo, branch) DO UPDATE SET built_seq = excluded.built_seq",
                params![key.agent, key.session, key.repo, key.branch, key.last],
            )?;
        }
        tx.commit()?;
        Ok(if built.is_empty() && dropped == 0 {
            Phase::Idle
        } else {
            Phase::Covered
        })
    }
}

/// This device's sessions with an event in the `ACTIVE_MS` before `now` whose last event is not an
/// end, each on the checkout of that event, with its agent (the facts have none; a session whose
/// last record a tombstone removed is left out), the newest first.
fn keys(raw: &Raw, k: &Connection, now: i64) -> Result<Vec<Key>> {
    let device = raw.device();
    // One aggregate, so SQLite takes the checkout from each session's last event.
    let sessions: Vec<(String, String, String, i64)> = k
        .prepare(
            "SELECT session, repo, branch, MAX(seq) FROM manifest_facts
             WHERE device = ?1 AND fact = 'event' AND ts >= ?2
             GROUP BY session ORDER BY MAX(seq) DESC",
        )?
        .query_map(params![device, now - ACTIVE_MS], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for (session, repo, branch, last) in sessions {
        let ended = k
            .query_row(
                "SELECT 1 FROM manifest_facts WHERE device = ?1 AND fact = 'end' AND seq = ?2",
                params![device, last],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if ended {
            continue;
        }
        let Some(e) = event(raw, device, last)? else {
            continue;
        };
        out.push(Key {
            agent: e.agent,
            session,
            repo,
            branch,
            last,
        });
    }
    Ok(out)
}

/// Whether `key` is built now: it has no row; its session's records went back past its build (a
/// rewind); or its session has, since the build, a reply (each Stop) or `EVERY_EVENTS` events.
fn due(k: &Connection, device: &str, key: &Key) -> Result<bool> {
    let built: Option<i64> = k
        .query_row(
            "SELECT built_seq FROM shortlists
             WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4",
            params![key.agent, key.session, key.repo, key.branch],
            |r| r.get(0),
        )
        .optional()?;
    let Some(built) = built else {
        return Ok(true);
    };
    if built >= key.last {
        return Ok(built > key.last);
    }
    let (events, replies): (i64, i64) = k.query_row(
        "SELECT COALESCE(SUM(fact = 'event'), 0), COALESCE(SUM(fact = 'reply'), 0)
         FROM manifest_facts
         WHERE device = ?1 AND repo = ?2 AND branch = ?3 AND fact IN ('event', 'reply')
           AND seq > ?4 AND session = ?5",
        params![device, key.repo, key.branch, built, key.session],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(replies > 0 || events >= EVERY_EVENTS)
}

/// The key's query in three parts, each gated with `rules` as it is read: the session's last
/// `PROMPTS` prompts on the checkout, the files it touched there, and its failing command (its
/// tool and what it ran, never its output). Empty parts are left out.
fn parts(
    raw: &Raw,
    k: &Connection,
    key: &Key,
    rules: &crate::redact::Rules,
) -> Result<Vec<String>> {
    let device = raw.device();
    let at = params![device, key.repo, key.branch, key.session];
    let body = |seq: i64| -> Result<Option<Value>> {
        Ok(event(raw, device, seq)?.map(|e| serde_json::from_str(&e.body).unwrap_or(Value::Null)))
    };
    let field = |v: &Value, name: &str| v[name].as_str().unwrap_or("").to_owned();
    let seqs: Vec<i64> = k
        .prepare(
            "SELECT seq FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND branch = ?3 AND fact = 'prompt' AND session = ?4
             ORDER BY seq DESC LIMIT ?5",
        )?
        .query_map(
            params![device, key.repo, key.branch, key.session, PROMPTS],
            |r| r.get(0),
        )?
        .collect::<rusqlite::Result<_>>()?;
    let mut prompts = Vec::new();
    for seq in seqs {
        if let Some(b) = body(seq)? {
            prompts.push(field(&b, "prompt"));
        }
    }
    let files: Vec<String> = k
        .prepare(
            "SELECT label FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND branch = ?3 AND fact = 'file' AND session = ?4
             GROUP BY label ORDER BY MAX(seq) DESC LIMIT ?5",
        )?
        .query_map(
            params![device, key.repo, key.branch, key.session, FILES],
            |r| r.get(0),
        )?
        .collect::<rusqlite::Result<_>>()?;
    // The last failure not followed by a success of the same call, as the manifest pairs them.
    let failing: Option<i64> = k
        .query_row(
            "SELECT f.seq FROM manifest_facts f
             WHERE f.device = ?1 AND f.repo = ?2 AND f.branch = ?3 AND f.fact = 'fail'
               AND f.session = ?4
               AND NOT EXISTS (SELECT 1 FROM manifest_facts x
                 WHERE x.device = f.device AND x.repo = f.repo AND x.fact = 'fixed'
                   AND x.branch = f.branch AND x.label = f.label AND x.seq > f.seq)
             ORDER BY f.seq DESC LIMIT 1",
            at,
            |r| r.get(0),
        )
        .optional()?;
    let failed = match failing.map(body).transpose()?.flatten() {
        Some(b) => format!("{} {}", field(&b, "tool"), what_ran(&field(&b, "input"))),
        None => String::new(),
    };
    Ok([prompts.join("\n"), files.join("\n"), failed]
        .iter()
        .map(|t| crate::redact::lines_with(t, rules))
        .filter(|t| !t.trim().is_empty())
        .collect())
}

/// The shortlist the worker keeps for `key`'s session on a checkout, the best first: none when
/// it keeps none (`per_prompt` was off, or the key is not built yet).
// The prompt point (Step 6) reads it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn of(
    k: &Connection,
    (agent, session, repo, branch): (&str, &str, &str, &str),
) -> Result<Option<Vec<String>>> {
    if !exists(k, "table", "shortlists")? {
        return Ok(None);
    }
    let built = k
        .query_row(
            "SELECT 1 FROM shortlists
             WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4",
            params![agent, session, repo, branch],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !built {
        return Ok(None);
    }
    let uids = k
        .prepare(
            "SELECT uid FROM shortlist
             WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4 ORDER BY rank",
        )?
        .query_map(params![agent, session, repo, branch], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(uids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::b::fixture::Store;
    use serde_json::json;

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
        assert_eq!(b.run(&s.raw, &mut k, NOW).unwrap(), Phase::Idle);
        assert!(!exists(&k, "table", "shortlists").unwrap());
        per_prompt(&s, true);
        assert_eq!(b.run(&s.raw, &mut k, NOW).unwrap(), Phase::Covered);
        assert_eq!(keys_built(&k), [key("live", "main")]);
        let live = of(&k, ("claude", "live", R, "main")).unwrap();
        assert_eq!(live, Some(vec![parser]));
        assert_eq!(of(&k, ("claude", "idle", R, "main")).unwrap(), None);
        // Nothing new: nothing built.
        assert_eq!(b.run(&s.raw, &mut k, NOW).unwrap(), Phase::Idle);
        // Idle past 30 minutes: its rows go.
        assert_eq!(
            b.run(&s.raw, &mut k, NOW + 30 * MIN).unwrap(),
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
            b.run(&s.raw, &mut k, NOW).unwrap()
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
        assert!(uids.contains(&lexer), "{uids:?}");
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
                .run(&s.raw, &mut k, NOW)
                .unwrap(),
            Phase::Covered
        );
        let uids = of(&k, ("claude", "live", R, "main")).unwrap().unwrap();
        assert_eq!(uids.len(), SHORT);
        assert!(uids.contains(&lesson) && uids.contains(&config), "{uids:?}");
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
        assert_eq!(b.run(&s.raw, &mut k, NOW).unwrap(), Phase::Covered);
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
        assert!(b.run(&s.raw, &mut k, NOW).is_err());
        k.execute_batch("DROP TRIGGER no_rank").unwrap();
        assert_eq!(
            of(&k, ("claude", "live", R, "main")).unwrap(),
            Some(vec![parser.clone()])
        );
        assert_eq!(b.run(&s.raw, &mut k, NOW).unwrap(), Phase::Covered);
        let uids = of(&k, ("claude", "live", R, "main")).unwrap().unwrap();
        assert_eq!(uids.len(), 2, "{uids:?}");
    }
}
