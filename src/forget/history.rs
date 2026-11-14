//! Decision 109 (docs/milestone-5-plan.md, Slice 5c): a repository's forget reaches the history
//! imported for it, claude-mem's documents and work state and the notes agents saved. From its
//! registration on, no reader shows that history: knowledge.db's imported documents are read only
//! while knowledge.db has applied every such forget raw.db holds, and a reader takes the proof of
//! it, `Applied`, under `dispatch::reading`, which no registration passes. Until the request that
//! registers one is built (5a, 5b), raw.db holds none, and every reader reads as before.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::BTreeSet;

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

/// What a reader says where it left knowledge.db's imported documents out (`imported_open` gave no
/// proof): they are not read until a forget's deletion is applied, which is no answer of "none".
pub(crate) const WITHHELD: &str =
    "Imported history is withheld until a forget's deletion reaches knowledge.db; try again shortly.";

#[cfg(test)]
pub(crate) mod fixture;

#[cfg(test)]
mod tests;
