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
           PRIMARY KEY (agent, session, repo, branch)
         );
         -- When a session last asked for a query vector (unix ms), kept apart from its keys, whose
         -- rows go when it ends or moves, until `VECTOR_EVERY` has passed.
         CREATE TABLE IF NOT EXISTS shortlist_asks(
           agent TEXT NOT NULL, session TEXT NOT NULL, at INTEGER NOT NULL,
           PRIMARY KEY (agent, session)
         );
         -- The last query vector, kept with its gated text's identity and embedder.
         CREATE TABLE IF NOT EXISTS shortlist_vectors(
           agent TEXT NOT NULL, session TEXT NOT NULL, repo TEXT NOT NULL, branch TEXT NOT NULL,
           sha TEXT NOT NULL, embedder TEXT NOT NULL, vec TEXT NOT NULL,
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
        // A query refused at its send releases its key's cooldown, whatever this call does next.
        if let Some(id) = embed
            .as_deref_mut()
            .and_then(crate::embed_phase::Phase::unasked)
            && let Some((agent, rest)) = id.split_once('\0')
            && let Some((session, _)) = rest.split_once('\0')
        {
            schema(k)?;
            k.execute(
                "DELETE FROM shortlist_asks WHERE agent = ?1 AND session = ?2",
                params![agent, session],
            )?;
        }
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
        let mut live = keys(raw, k, now)?;
        // The answered key first: `KEYS` newer keys due meanwhile never leave its vector unused.
        if let Some(i) = answer
            .as_ref()
            .and_then(|a| live.iter().position(|key| key.id() == a.key))
        {
            let key = live.remove(i);
            live.insert(0, key);
        }
        // The exclusion list and the sessions it holds (every repository they touched), once.
        let reading = crate::curate::Reading::now(raw, crate::curate::Reads::Live)?;
        let tombstones = raw.tombstones()?;
        let active: Option<String> = k
            .query_row(
                "SELECT embedder FROM vec_generation WHERE state = 'active'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let mut built = Vec::new();
        let mut asks = Vec::new();
        for key in &live {
            if built.len() == KEYS {
                break;
            }
            let id = key.id();
            let answered = answer.as_ref().filter(|a| a.key == id);
            let is_due = due(k, device, key)?;
            let cooled =
                embed.is_some() && active.is_some() && now - asked_at(k, key)? >= VECTOR_EVERY;
            // A key last built without a vector for its text asks again once cooled, due or not.
            let retry = cooled && !vectored(k, key, active.as_deref())?;
            if answered.is_none() && !is_due && !retry {
                continue;
            }
            let texts = parts(raw, k, key, &rules)?;
            let text = texts.join("\n");
            let sha = crate::embed_phase::sha(&text);
            // The parts are read by session (the facts have no agent): a session of this id that
            // any agent ran in an excluded repository excludes the key.
            let excluded = reading
                .excluded
                .iter()
                .any(|e| e.split_once('\0').is_some_and(|(_, s)| s == key.session));
            // A vector is used only for the text the key has now, from the active embedder.
            let fresh = answered
                .filter(|a| a.text == text && !excluded && active.as_ref() == Some(&a.embedder));
            let cached: Option<String> = if excluded || fresh.is_some() {
                None
            } else {
                k.query_row(
                    "SELECT vec FROM shortlist_vectors
                     WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4
                       AND sha = ?5 AND embedder = ?6",
                    params![
                        key.agent,
                        key.session,
                        key.repo,
                        key.branch,
                        sha,
                        active.as_deref().unwrap_or("")
                    ],
                    |r| r.get(0),
                )
                .optional()?
            };
            let cached: Option<Vec<f32>> = cached.map(|v| serde_json::from_str(&v)).transpose()?;
            let vector = fresh.map(|a| a.vector.as_slice()).or(cached.as_deref());
            if let Some(e) = embed.as_deref_mut()
                && vector.is_none()
                && !excluded
                && !text.is_empty()
                && cooled
                && e.ask(k, &id, &text, &reading, &rules, tombstones)?
            {
                asks.push(key);
            }
            if fresh.is_none() && !is_due {
                continue;
            }
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let claims =
                crate::search::b::delivered_ranked(raw, k, &refs, vector, &key.repo, SHORT)?;
            built.push((key, claims, fresh, sha));
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
        tx.execute(
            "DELETE FROM shortlist_asks WHERE at <= ?1",
            [now - VECTOR_EVERY],
        )?;
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
                    for table in ["shortlists", "shortlist", "shortlist_vectors"] {
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
        for key in &asks {
            tx.execute(
                "INSERT OR REPLACE INTO shortlist_asks(agent, session, at) VALUES(?1, ?2, ?3)",
                params![key.agent, key.session, now],
            )?;
        }
        for (key, claims, fresh, sha) in &built {
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
                 ON CONFLICT(agent, session, repo, branch) DO UPDATE SET
                   built_seq = excluded.built_seq",
                params![key.agent, key.session, key.repo, key.branch, key.last],
            )?;
            if let Some(a) = fresh {
                tx.execute(
                    "INSERT OR REPLACE INTO shortlist_vectors
                     (agent, session, repo, branch, sha, embedder, vec)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        key.agent,
                        key.session,
                        key.repo,
                        key.branch,
                        sha,
                        a.embedder,
                        serde_json::to_string(&a.vector)?
                    ],
                )?;
            } else {
                // A vector kept for a text the key no longer has is of no more use.
                tx.execute(
                    "DELETE FROM shortlist_vectors
                     WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4 AND sha != ?5",
                    params![key.agent, key.session, key.repo, key.branch, sha],
                )?;
            }
        }
        tx.commit()?;
        Ok(if built.is_empty() && asks.is_empty() && dropped == 0 {
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

/// When `key`'s session last asked for a query vector in the last `VECTOR_EVERY`, on any of its
/// checkouts and across an end and a resume, 0 never.
fn asked_at(k: &Connection, key: &Key) -> Result<i64> {
    Ok(k.query_row(
        "SELECT at FROM shortlist_asks WHERE agent = ?1 AND session = ?2",
        params![key.agent, key.session],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or(0))
}

/// Whether `key` keeps a vector from `active`: one for the text of its last build, since a build
/// drops one kept for another text.
fn vectored(k: &Connection, key: &Key, active: Option<&str>) -> Result<bool> {
    Ok(k.query_row(
        "SELECT 1 FROM shortlist_vectors
         WHERE agent = ?1 AND session = ?2 AND repo = ?3 AND branch = ?4 AND embedder = ?5",
        params![key.agent, key.session, key.repo, key.branch, active],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
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
    let file_facts: Vec<(String, i64)> = k
        .prepare(
            "SELECT label, MAX(seq) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND branch = ?3 AND fact = 'file' AND session = ?4
             GROUP BY label ORDER BY MAX(seq) DESC LIMIT ?5",
        )?
        .query_map(
            params![device, key.repo, key.branch, key.session, FILES],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    let mut files = Vec::new();
    for (label, seq) in file_facts {
        let Some(e) = event(raw, device, seq)? else {
            continue;
        };
        let b: Value = serde_json::from_str(&e.body).unwrap_or(Value::Null);
        let input = field(&b, "input");
        let Some(input) = untouched(&input, rules) else {
            continue;
        };
        let input: Value = serde_json::from_str(input).unwrap_or(Value::Null);
        if crate::consumer::manifest::paths(&input, e.cwd.as_deref()).contains(&label) {
            files.push(label);
        }
    }
    // The last failure not followed by a success of the same call, as the manifest pairs them.
    let failing: Option<i64> = k
        .query_row(
            "SELECT f.seq FROM manifest_facts f
             WHERE f.device = ?1 AND f.repo = ?2 AND f.branch = ?3 AND f.fact = 'fail'
               AND f.session = ?4
               AND NOT EXISTS (SELECT 1 FROM manifest_facts x
                 WHERE x.device = f.device AND x.repo = f.repo AND x.fact = 'fixed'
                   AND x.branch = f.branch AND x.session = f.session
                   AND x.label = f.label AND x.seq > f.seq)
             ORDER BY f.seq DESC LIMIT 1",
            at,
            |r| r.get(0),
        )
        .optional()?;
    let failed = match failing.map(body).transpose()?.flatten() {
        Some(b) => {
            let input = field(&b, "input");
            let ran = untouched(&input, rules).map(what_ran).unwrap_or_default();
            vec![field(&b, "tool"), ran]
        }
        None => Vec::new(),
    };
    Ok([(prompts, "\n"), (files, "\n"), (failed, " ")]
        .iter()
        .map(|(fields, sep)| joined(fields, sep, rules))
        .filter(|t| !t.trim().is_empty())
        .collect())
}

/// A stored input the rules leave as it is, else none: what a call ran or the files it named are
/// read from it only then, so no rule written against the input is undone by reading a part of it
/// out (Codex's reviews of #333). The part read is gated again with the rest of the query.
fn untouched<'a>(input: &'a str, rules: &crate::redact::Rules) -> Option<&'a str> {
    (joined(&[input.to_owned()], "", rules) == input).then_some(input)
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
/// from every delivered claim of the repository. Ignores `[inject]` and keeps no hook state.
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
}
