//! Milestone 4 Task 8 (D9): the shortlist phase. While `[inject] per_prompt` is on, the worker
//! keeps, for each of this device's live sessions, the delivered claims the prompt hook picks
//! from: at most `SHORT` per key (agent, session and the checkout of its last event), ranked by
//! `search::b::delivered_ranked` over the session's last prompts, the files it touched and its
//! failing command, each gated with the rules as they are now. A key is built when it has no row,
//! after a reply, every `EVERY_EVENTS` events and after a rewind. The search runs outside every
//! transaction, and only its result is written, in one short one. A key's query vector is asked
//! of the embedding phase, which never waits for it, at most every `VECTOR_EVERY`: the key is
//! built from full text meanwhile, and again with the vector once it comes back for the text the
//! key still has.

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
/// A key's query vector is asked at most this often (D9, pending the owner's item 1).
// ponytail: up to `KEYS` keys asking 4 an hour each can spend a day's `daily_requests` on a busy
// day; one hourly budget across keys if that shows.
pub const VECTOR_EVERY: i64 = 15 * 60_000;

/// Made when `per_prompt` is first on: a home that never asks for it gets no table.
fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS shortlists(
           agent TEXT NOT NULL, session TEXT NOT NULL, repo TEXT NOT NULL, branch TEXT NOT NULL,
           -- The session's last event the build read.
           built_seq INTEGER NOT NULL,
           -- When its query vector was last asked for (unix ms), 0 never.
           vector_at INTEGER NOT NULL DEFAULT 0,
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

impl Key {
    /// What an ask is filed under.
    fn id(&self) -> String {
        [&self.agent, &self.session, &self.repo, &self.branch]
            .map(String::as_str)
            .join("\0")
    }
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
    /// due, or whose vector came back, built; `Covered` when one was. Idle while `per_prompt` is
    /// off, or settings that do not load turn it off, and before the worker has made the claims
    /// and the manifest's facts. With `embed`, a key built from full text asks for its vector.
    pub fn run(
        &mut self,
        raw: &Raw,
        k: &mut Connection,
        mut embed: Option<&mut crate::embed_phase::Phase>,
        now: i64,
    ) -> Result<Phase> {
        let answer = embed
            .as_deref_mut()
            .and_then(crate::embed_phase::Phase::answer);
        let on = crate::config::inject(&self.home)
            .inspect_err(|e| eprintln!("oboete: no shortlist for now: {e:#}"))
            .is_ok_and(|i| i.per_prompt);
        if !on || !exists(k, "table", "manifest_facts")? || !exists(k, "view", "active")? {
            return Ok(Phase::Idle);
        }
        // Rules that do not load stop the phase as they stop a hook's capture: the bundled rules
        // alone never gate a query in place of the user's (Codex's security review of Step 4).
        let rules = match crate::redact::Rules::load(&self.home) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("oboete: no shortlist for now: {e:#}");
                return Ok(Phase::Idle);
            }
        };
        schema(k)?;
        let device = raw.device();
        let live = keys(raw, k, now)?;
        // The exclusion list and the sessions it holds (every repository they touched), once.
        let reading = crate::curate::Reading::now(raw, crate::curate::Reads::Live)?;
        let active: Option<String> = k
            .query_row(
                "SELECT embedder FROM vec_generation WHERE state = 'active'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let mut built = Vec::new();
        for key in &live {
            if built.len() == KEYS {
                break;
            }
            let id = key.id();
            let answered = answer.as_ref().filter(|a| a.key == id);
            let is_due = due(k, device, key)?;
            if answered.is_none() && !is_due {
                continue;
            }
            let texts = parts(raw, k, key, &rules)?;
            let text = texts.join("\n");
            // The parts are read by session (the facts have no agent): a session of this id that
            // any agent ran in an excluded repository excludes the key.
            let excluded = reading
                .excluded
                .iter()
                .any(|e| e.split_once('\0').is_some_and(|(_, s)| s == key.session));
            // A vector is used only for the text the key has now, from the active embedder.
            let vector = answered
                .filter(|a| a.text == text && !excluded && active.as_ref() == Some(&a.embedder))
                .map(|a| a.vector.as_slice());
            if vector.is_none() && !is_due {
                continue;
            }
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let claims =
                crate::search::b::delivered_ranked(raw, k, &refs, vector, &key.repo, SHORT)?;
            let mut asked = vector_at(k, key)?;
            if let Some(e) = embed.as_deref_mut()
                && vector.is_none()
                && !excluded
                && !text.is_empty()
                && now - asked >= VECTOR_EVERY
                && e.ask(k, &id, &text, &reading)?
            {
                asked = now;
            }
            built.push((key, claims, asked));
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
        for (key, claims, asked) in &built {
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
                "INSERT INTO shortlists(agent, session, repo, branch, built_seq, vector_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(agent, session, repo, branch) DO UPDATE SET
                   built_seq = excluded.built_seq,
                   vector_at = MAX(vector_at, excluded.vector_at)",
                params![
                    key.agent,
                    key.session,
                    key.repo,
                    key.branch,
                    key.last,
                    asked
                ],
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

/// When `key`'s session last asked for a query vector, on any of its checkouts, 0 never: a key
/// built keeps it, so a session that moves between branches asks no more often.
fn vector_at(k: &Connection, key: &Key) -> Result<i64> {
    Ok(k.query_row(
        "SELECT COALESCE(MAX(vector_at), 0) FROM shortlists WHERE agent = ?1 AND session = ?2",
        params![key.agent, key.session],
        |r| r.get(0),
    )?)
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

/// The key's query in three parts, each gated with `rules` as it is read, whole and in each of
/// its fields alone (`joined`): the session's last `PROMPTS` prompts on the checkout, the files it
/// touched there, and its failing command (its tool and what it ran, never its output). Empty
/// parts are left out.
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
        Some(b) => vec![field(&b, "tool"), what_ran(&field(&b, "input"))],
        None => Vec::new(),
    };
    Ok([(prompts, "\n"), (files, "\n"), (failed, " ")]
        .iter()
        .map(|(fields, sep)| joined(fields, sep, rules))
        .filter(|t| !t.trim().is_empty())
        .collect())
}

/// `fields` joined by `sep`, gated whole, line by line and in each field alone
/// (`redact::joined_with`): a rule anchored to a field (`^...$`) still holds once another field
/// is joined before it (Codex's security review of Step 4).
fn joined(fields: &[String], sep: &str, rules: &crate::redact::Rules) -> String {
    let mut at = 0;
    let parts: Vec<_> = fields
        .iter()
        .map(|f| {
            let part = at..at + f.len();
            at = part.end + sep.len();
            part
        })
        .collect();
    crate::redact::joined_with(&fields.join(sep), &parts, rules)
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
                .run(&s.raw, &mut k, None, NOW)
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
        assert_eq!(uids.len(), 2, "{uids:?}");
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
    /// once every `VECTOR_EVERY`.
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
        assert!(uids.contains(&parser) && !uids.contains(&near), "{uids:?}");
        assert_eq!(queries(&s), 1);
        wait(|| phase.done());
        assert_eq!(stub.texts()[sent..], [["parser db ok"]]);
        // The embedding phase settles the answer; the next call builds the key with it.
        phase.poll(&s.raw, &k).unwrap();
        assert_eq!(run(&s, &mut k, &mut phase, NOW), Phase::Covered);
        let uids = shortlist(&k);
        assert!(uids.contains(&parser) && uids.contains(&near), "{uids:?}");
        // A reply within 15 minutes: built again, nothing asked.
        s.event("reply", "live", main, NOW - MIN, json!({"assistant": "ok"}));
        assert_eq!(run(&s, &mut k, &mut phase, NOW + MIN), Phase::Covered);
        assert_eq!(queries(&s), 1);
        s.event(
            "reply",
            "live",
            main,
            NOW + 15 * MIN,
            json!({"assistant": "ok"}),
        );
        assert_eq!(run(&s, &mut k, &mut phase, NOW + 16 * MIN), Phase::Covered);
        assert_eq!(queries(&s), 2);
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
        let rule =
            "[redaction]\nextra_rules = [{ id = \"deploy\", regex = '^deploy-key-[0-9]{6}' }]\n";
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

    /// D9: a session that moves to another branch and back asks for its query vector no more
    /// often than every `VECTOR_EVERY`: each key built keeps the session's last ask (Codex's
    /// security review of Step 4).
    #[test]
    fn a_session_moving_between_branches_asks_no_sooner() {
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
                };
                let consumers = crate::worker::consumers(&home);
                crate::worker::run_holding(&home, 200, consumers, || {}, None, phases)
            });
            wait(|| stub.requests() > sent && shortlist().is_some());
            wait(|| curated.load(Ordering::SeqCst) > 0);
            let uids = shortlist().unwrap();
            assert!(uids.contains(&parser) && !uids.contains(&near), "{uids:?}");
            // Past the worker's idle wait: it stays up for the answer.
            std::thread::sleep(Duration::from_millis(1_000));
            assert!(!worker.is_finished(), "the worker left before the answer");
            drop(held);
            worker.join().unwrap().unwrap();
        });
        let uids = shortlist().unwrap();
        assert!(uids.contains(&parser) && uids.contains(&near), "{uids:?}");
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
        assert!(!uids.contains(&near), "{uids:?}");
    }
}
