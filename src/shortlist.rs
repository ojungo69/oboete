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
use std::collections::HashSet;
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

/// The claims one prompt's injection takes at most, a pair counting two (D2).
pub const PLACES: usize = 3;
/// The share of one text's trigrams a claim's body holds to be injected with it (D9): set by Task 8
/// Step 11 on milestone 3's dev prompts (docs/milestone-4.md), tuned by Task 12b.
pub const THRESHOLD: f64 = 0.53;
/// A text's trigrams count as at least this many: a short prompt (`進めて`) tells no topic, and
/// its one or two trigrams would match any claim holding them (Step 11).
pub const MIN_GRAMS: usize = 10;
/// The trigrams read of each text (D9): `search::trigrams`' 64 would leave a long prompt's end out.
const GRAMS: usize = 256;

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
        schema(k)?;
        let rules = crate::capture::Settings::load(&self.home)
            .map(|s| s.rules)
            .unwrap_or_default();
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
            let excluded = reading
                .excluded
                .contains(&format!("{}\0{}", key.agent, key.session));
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
            let mut asked = 0;
            if let Some(e) = embed.as_deref_mut()
                && vector.is_none()
                && !excluded
                && !text.is_empty()
                && now - vector_at(k, key)? >= VECTOR_EVERY
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

/// When `key`'s query vector was last asked for, 0 never.
fn vector_at(k: &Connection, key: &Key) -> Result<i64> {
    Ok(k.query_row(
        "SELECT vector_at FROM shortlists
         WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4",
        params![key.agent, key.session, key.repo, key.branch],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or(0))
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

/// Each run's trigrams of `text`, case folded as the index folds them, at most `cap`.
fn grams(text: &str, cap: usize) -> HashSet<String> {
    crate::search::trigrams_upto(text, cap)
        .into_iter()
        .map(|g| g.to_ascii_lowercase())
        .collect()
}

/// The share of `of` (one text's trigrams, at least `MIN_GRAMS`) that `body` holds.
fn share(of: &HashSet<String>, body: &str) -> f64 {
    let held = grams(body, usize::MAX);
    of.iter().filter(|g| held.contains(*g)).count() as f64 / of.len().max(MIN_GRAMS) as f64
}

/// Spec 4.2 and D9: of `candidates` (uids, the best first), the units (`placed`) of the claims
/// whose body holds at least `threshold` of one text's trigrams. Read-only.
pub fn pick(
    raw: &Raw,
    k: &Connection,
    candidates: &[String],
    texts: &[&str],
    threshold: f64,
) -> Result<Vec<Vec<crate::claims::Claim>>> {
    let of: Vec<HashSet<String>> = texts
        .iter()
        .map(|t| grams(t, GRAMS))
        .filter(|g| !g.is_empty())
        .collect();
    if of.is_empty() {
        return Ok(Vec::new());
    }
    placed(raw, k, candidates, |c| {
        of.iter().any(|g| share(g, &c.body) >= threshold)
    })
}

/// The units (D2) of the claims of `candidates` (uids, the best first) that `keep` keeps, each
/// still delivered under `claims::DECIDED`, a unit left out whole when an owner's change the
/// worker has not applied touches one of its claims (D3), in `PLACES` places, newest first.
/// Read-only.
pub fn placed(
    raw: &Raw,
    k: &Connection,
    candidates: &[String],
    keep: impl Fn(&crate::claims::Claim) -> bool,
) -> Result<Vec<Vec<crate::claims::Claim>>> {
    use crate::claims;
    if !exists(k, "view", "active")? {
        return Ok(Vec::new());
    }
    let pending = claims::Pending::read(raw, k)?;
    let hidden = |uid: &str| pending.touches(k, uid);
    let decided = format!(
        "SELECT 1 FROM active a WHERE a.uid = ?1 AND {}",
        claims::DECIDED_WHERE
    );
    let mut ranked = Vec::new();
    for uid in candidates {
        let Some(c) = claims::delivered_one(k, uid)? else {
            continue;
        };
        let still = k
            .query_row(&decided, [uid], |_| Ok(()))
            .optional()?
            .is_some();
        if still && keep(&c) {
            ranked.push(c);
        }
    }
    let (mut fit, _) = claims::place(claims::units(k, &ranked, hidden)?, PLACES);
    claims::newest_first(&mut fit);
    Ok(fit)
}

/// `oboete inject --prompt` (Task 8 Step 11): each claim `prompt` would get in `repo`, with the
/// share of the prompt's trigrams its body holds, from `session`'s shortlists there, or, with none,
/// from every delivered claim of the repository. Ignores `[inject]` and writes nothing.
pub fn report(
    home: &Path,
    repo: &str,
    session: Option<&str>,
    prompt: &str,
    threshold: f64,
) -> Result<String> {
    let path = home.join("knowledge.db");
    if !path.exists() {
        return Ok(String::new());
    }
    let raw = crate::raw::open(home)?;
    let k = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !exists(&k, "view", "active")? {
        return Ok(String::new());
    }
    let uids = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<Vec<String>> {
        Ok(k.prepare(sql)?
            .query_map(args, |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    };
    let mut candidates = match session {
        Some(s) if exists(&k, "table", "shortlist")? => uids(
            "SELECT uid FROM shortlist WHERE session = ?1 AND repo = ?2
             GROUP BY uid ORDER BY MIN(rank)",
            &[&s, &repo],
        )?,
        _ => Vec::new(),
    };
    if candidates.is_empty() {
        let every = format!(
            "SELECT a.uid FROM active a WHERE a.repo = ?1 AND {}
             ORDER BY a.valid_from DESC, a.uid",
            crate::claims::DECIDED_WHERE
        );
        candidates = uids(&every, &[&repo])?;
    }
    let text = crate::hook::strip_blocks(prompt, true);
    let of = grams(&text, GRAMS);
    let units = pick(&raw, &k, &candidates, &[&text], threshold)?;
    Ok(units
        .iter()
        .flatten()
        .map(|c| format!("{} {:.3}\n", c.uid, share(&of, &c.body)))
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
        assert_eq!(picked, [vec![stderr.clone(), stdout.clone()]], "{picked:?}");
        // At 0 every delivered one passes: the pair and one more fill the 3 places.
        let picked = uids(pick(&s.raw, &k, &all, &prompt, 0.0).unwrap());
        assert_eq!(
            picked,
            [vec![lexer.clone()], vec![stderr.clone(), stdout.clone()]],
            "{picked:?}"
        );
        assert!(
            pick(&s.raw, &k, &all, &["zzz qqq"], THRESHOLD)
                .unwrap()
                .is_empty()
        );
        // A short text counts as `MIN_GRAMS` trigrams: three held tell no topic.
        let short = pick(&s.raw, &k, &all, &["Lexer"], THRESHOLD).unwrap();
        assert!(short.is_empty(), "{:?}", uids(short));
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
        assert_eq!(lines.len(), 2, "{printed}");
        assert!(lines[1].starts_with(&format!("{stdout} 0.")), "{printed}");
    }
}
