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
/// The cosine to a prompt at which a document's meaning bears on it (G23's `T`,
/// docs/prompt-context.md): set without model calls from milestone 4 D18's 50 no-answer dev
/// questions against every active claim of their repositories in the dev home (5,306), the lowest
/// at which at most 1 of them gets anything; the one evaluation (decision 34) sets it again.
pub const MEANING: f32 = 0.67;
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
                // D1 applies to new query asks too; settled answers and text ranking still run.
                && !crate::curate::lagging(raw, k)?
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
    let words = Words::of(texts);
    if words.0.is_empty() {
        return Ok(Vec::new());
    }
    placed(raw, k, candidates, |c| words.share(&c.body) >= threshold)
}

/// Each text's trigrams, as `pick` reads them, for anything else a prompt's block may take (G23).
pub struct Words(Vec<HashSet<String>>);

impl Words {
    pub fn of(texts: &[&str]) -> Self {
        Self(
            texts
                .iter()
                .map(|t| grams(t, GRAMS))
                .filter(|g| !g.is_empty())
                .collect(),
        )
    }

    /// The largest share of one text's trigrams that `body` holds.
    pub fn share(&self, body: &str) -> f64 {
        self.0.iter().map(|g| share(g, body)).fold(0.0, f64::max)
    }
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
    let hidden = |uid: &str| Ok(pending.touches(k, uid)? || claims::muted(k, uid)?);
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
mod tests;
