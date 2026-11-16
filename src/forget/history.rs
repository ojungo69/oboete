//! Decision 109 (docs/milestone-5-plan.md, Slice 5c): a repository's forget reaches the history
//! imported for it, claude-mem's documents and work state and the notes agents saved. From its
//! registration on, no reader shows that history: knowledge.db's imported documents are read only
//! while knowledge.db has applied every such forget raw.db holds, and a reader takes the proof of
//! it, `Applied`, under `dispatch::reading`, which no registration passes. Until the request that
//! registers one is built (5a, 5b), raw.db holds none, and every reader reads as before.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;

/// Proof that knowledge.db had applied every history forget raw.db held when it was taken: what a
/// function that reads knowledge.db's imported documents asks of its caller. Good while the caller
/// holds the `dispatch::reading` it was taken under.
#[derive(Debug)]
pub(crate) struct Applied(());

/// A history forget as a store holds it: its job, and the token its insertion into raw.db wrote.
pub(crate) type Pair = (String, String);

/// raw.db's history forgets. None in a raw.db this build has not opened to write yet (its table is
/// made then), as a read-only reader may find one.
pub(crate) fn registered(raw: &Connection) -> Result<BTreeSet<Pair>> {
    pairs(raw, "forget_history")
}

/// The history forgets knowledge.db has applied. None in one made before decision 109, which a
/// read-only reader may open before anything made its table.
pub(crate) fn applied(k: &Connection) -> Result<BTreeSet<Pair>> {
    pairs(k, "history_applied")
}

fn pairs(conn: &Connection, table: &str) -> Result<BTreeSet<Pair>> {
    if !crate::consumer::manifest::exists(conn, "table", table)? {
        return Ok(BTreeSet::new());
    }
    let mut st = conn.prepare_cached(&format!("SELECT job, token FROM {table}"))?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Whether knowledge.db has applied every history forget raw.db holds, by job and token (a job
/// raw.db lost and a log applied again has another token, so the readers close until knowledge.db
/// applies it again); the proof when it has. raw.db alone when it holds none, as before any
/// registration.
pub(crate) fn imported_open(raw: &Connection, k: &Connection) -> Result<Option<Applied>> {
    let registered = registered(raw)?;
    if registered.is_empty() {
        return Ok(Some(Applied(())));
    }
    Ok(registered.is_subset(&applied(k)?).then_some(Applied(())))
}

/// Doctor's line (decision 109): how many of the history forgets raw.db holds knowledge.db has
/// not applied, the relation `imported_open` reads; none when it has applied them all.
pub(crate) fn doctor_line(registered: &BTreeSet<Pair>, applied: &BTreeSet<Pair>) -> Option<String> {
    let waiting = registered.difference(applied).count();
    (waiting > 0).then(|| {
        format!(
            "imported history is withheld: {waiting} forget(s) of a repository's history not \
             yet applied in knowledge.db (`oboete forget --continue` applies them; the worker \
             does too)"
        )
    })
}

/// Whether a history forget covers the imported document, its files or the note `uid` names
/// (`denied_imports`, kind 'uid'): what knowledge.db is not given (5c), and no import appends
/// (5d). Asked of a writer's raw.db, whose open made the table: once per Import op a consumer
/// projects.
pub(crate) fn covered(raw: &Connection, uid: &str) -> Result<bool> {
    covered_key(raw, "uid", uid)
}

/// Whether a history forget covers the identity `key` of `kind` (`denied_imports`): an Import op's
/// uid ('uid'), imported work state's `ws_key` ('ws'). Asked of a writer's raw.db.
fn covered_key(raw: &Connection, kind: &str, key: &str) -> Result<bool> {
    Ok(raw
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM denied_imports WHERE kind = ?1 AND key = ?2)",
        )?
        .query_row([kind, key], |r| r.get(0))?)
}

/// Imported work state's identity in the refusal table (kind 'ws'): `source:source_id`, of an op
/// body that names both. A list written here names neither.
pub(crate) fn ws_key(body: &serde_json::Value) -> Option<String> {
    Some(format!(
        "{}:{}",
        body["source"].as_str()?,
        body["source_id"].as_str()?
    ))
}

/// Every key of `kind` a history forget covers: the uids ('uid'), for what keeps no row of them
/// though it reads none by uid (the carry of vectors into a new knowledge.db), and imported work
/// state's `ws_key`s ('ws'), for raw.db's readers of work state. A raw.db without the table (one a
/// hook reads before this version's writer opened it) holds none.
pub(crate) fn covered_keys(raw: &Connection, kind: &str) -> Result<HashSet<String>> {
    if !crate::consumer::manifest::exists(raw, "table", "denied_imports")? {
        return Ok(HashSet::new());
    }
    let mut st = raw.prepare_cached("SELECT DISTINCT key FROM denied_imports WHERE kind = ?1")?;
    let rows = st.query_map([kind], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The source ids of `source`'s documents (kind 'uid', an import's uid being `source:source_id`)
/// or imported work state (kind 'ws') a history forget covers, which an import takes as already
/// imported (5c), their bodies rewritten by a purge (5e) or not.
pub(crate) fn covered_ids(raw: &Connection, kind: &str, source: &str) -> Result<HashSet<String>> {
    let prefix = format!("{source}:");
    Ok((covered_keys(raw, kind)?.into_iter())
        .filter_map(|key| key.strip_prefix(&prefix).map(str::to_owned))
        .collect())
}

/// Whether a history forget covers what op `body` of `kind` appends (5d): an imported document,
/// its files or a note by its uid, imported work state by its source and source id.
pub(crate) fn covered_op(
    raw: &Connection,
    kind: crate::raw::OpKind,
    body: &serde_json::Value,
) -> Result<bool> {
    let (kind, key) = match kind {
        crate::raw::OpKind::Import => ("uid", body["uid"].as_str().map(str::to_owned)),
        crate::raw::OpKind::WorkState => ("ws", ws_key(body)),
        _ => return Ok(false),
    };
    key.map_or(Ok(false), |key| covered_key(raw, kind, &key))
}

/// Decision 109 (5c, "先に消す"): applies in knowledge.db `k` each history forget raw.db holds that
/// it has not: every row made from what the forget covers goes (`remove`), and the job is recorded
/// with the token read here, in one knowledge.db transaction for each. A job raw.db holds with
/// another token (lost with raw.db, applied again from its log) is applied again. A pass that
/// stops leaves its job unapplied and the readers closed; the next pass does it again. How many it
/// applied.
pub(crate) fn apply(raw: &Connection, k: &mut Connection) -> Result<usize> {
    let registered = registered(raw)?;
    if registered.is_empty() {
        return Ok(0);
    }
    let done = applied(k)?;
    let mut count = 0;
    for (job, token) in registered.difference(&done) {
        let uids = job_uids(raw, job)?;
        let tx = k.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        remove(&tx, &uids)?;
        tx.execute(
            "INSERT OR REPLACE INTO history_applied(job, token) VALUES (?1, ?2)",
            [job, token],
        )?;
        tx.commit()?;
        count += 1;
    }
    Ok(count)
}

/// `oboete forget --continue` (5c): applies the history forgets knowledge.db has not, asking a
/// resident worker aside only when one waits. How many it applied.
pub(crate) fn apply_due(home: &Path) -> Result<usize> {
    {
        let Some(raw) = crate::raw::read_only(home)? else {
            return Ok(0);
        };
        if registered(&raw.conn)?.is_empty()
            || imported_open(&raw.conn, &crate::knowledge::open(home)?)?.is_some()
        {
            return Ok(0);
        }
    }
    let _worker = crate::worker::lock_asking(home)?;
    let raw = crate::raw::open(home)?;
    raw.apply_history(&mut crate::knowledge::open(home)?)
}

/// The uids history forget `job` covers.
fn job_uids(raw: &Connection, job: &str) -> Result<Vec<String>> {
    let mut st = raw.prepare_cached(
        "SELECT key FROM denied_imports WHERE kind = 'uid' AND job = ?1 ORDER BY key",
    )?;
    let rows = st.query_map([job], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every row knowledge.db made from the imported documents, files and notes `uids` name, whichever
/// repository or op each is filed under: their full-text rows by rowid first, then the rows, the
/// files, the vectors' index and keys, and last the embedding queue, which deleting a key fills
/// again (`vector_todo_unkeyed`). A table not made yet holds none.
fn remove(k: &Connection, uids: &[String]) -> Result<()> {
    if uids.is_empty() {
        return Ok(());
    }
    k.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS history_covered(uid TEXT PRIMARY KEY);
         DELETE FROM temp.history_covered;",
    )?;
    {
        let mut add = k.prepare("INSERT OR IGNORE INTO temp.history_covered(uid) VALUES (?1)")?;
        for uid in uids {
            add.execute([uid])?;
        }
    }
    let has = |table: &str| crate::consumer::manifest::exists(k, "table", table);
    if has("imported")? {
        k.execute(
            "DELETE FROM imported_fts WHERE rowid IN
               (SELECT rowid FROM imported WHERE uid IN (SELECT uid FROM temp.history_covered))",
            [],
        )?;
        k.execute(
            "DELETE FROM imported WHERE uid IN (SELECT uid FROM temp.history_covered)",
            [],
        )?;
    }
    if has("imported_files")? {
        k.execute(
            "DELETE FROM imported_files WHERE uid IN (SELECT uid FROM temp.history_covered)",
            [],
        )?;
    }
    let ids: Vec<i64> = k
        .prepare(
            "SELECT id FROM vector_keys
             WHERE kind IN ('k', 'p') AND key IN (SELECT uid FROM temp.history_covered)",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    // One row at a time, as `embed_phase::touched` removes them from the vec0 index.
    for id in ids {
        k.prepare_cached("DELETE FROM vec_index WHERE rowid = ?1")?
            .execute([id])?;
        k.prepare_cached("DELETE FROM vector_keys WHERE id = ?1")?
            .execute([id])?;
    }
    if has("vector_todo")? {
        k.execute(
            "DELETE FROM vector_todo
             WHERE family = 'i' AND key IN (SELECT uid FROM temp.history_covered)",
            [],
        )?;
    }
    k.execute("DELETE FROM temp.history_covered", [])?;
    Ok(())
}

/// What a reader says where it left knowledge.db's imported documents out (`imported_open` gave no
/// proof): they are not read until a forget's deletion is applied, which is no answer of "none".
pub(crate) const WITHHELD: &str = "Imported history is withheld until a forget's deletion reaches knowledge.db; try again shortly.";

#[cfg(test)]
pub(crate) mod fixture;

#[cfg(test)]
mod tests;
