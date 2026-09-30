//! Milestone 4 Task 3 (D5): the imported documents consumer. It reads the op log of every device
//! that has ops and keeps each `import` op's document in knowledge.db, keyed by the op that brought
//! it, with a trigram index over its title and body. Curation reads records, never these ops, and
//! nothing injects them.

use crate::raw::{ImportDoc, OpKind, Raw};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Imported;

/// Ops per step, one knowledge.db transaction each.
const BATCH: usize = 500;

/// A document per op: a restore's rewind removes exactly the rows of the ops it lost, and a
/// document two devices imported is kept once per op (readers show it once per uid). The index
/// keeps no copy of the text, which `imported` holds (a quarter of the store on claude-mem's
/// history): a hit is read back through its rowid.
pub fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS imported(
           rowid INTEGER PRIMARY KEY, op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
           uid TEXT NOT NULL, source TEXT NOT NULL, source_id TEXT NOT NULL, kind TEXT NOT NULL,
           repo TEXT NOT NULL, session TEXT NOT NULL, ts INTEGER NOT NULL, title TEXT NOT NULL,
           body TEXT NOT NULL, UNIQUE (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS imported_uid ON imported(uid);
         CREATE VIRTUAL TABLE IF NOT EXISTS imported_fts
           USING fts5(text, tokenize='trigram', content='', contentless_delete=1);",
    )?;
    Ok(())
}

impl Consumer for Imported {
    fn name(&self) -> &'static str {
        "imported"
    }

    fn reads_ops(&self) -> bool {
        true
    }

    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let ops = raw.ops_after(device, after, BATCH)?;
        let Some(last) = ops.last().map(|o| o.op_seq) else {
            return Ok(after);
        };
        for op in ops.into_iter().filter(|o| o.kind == OpKind::Import) {
            // An op this version cannot read stays in raw, unread.
            let Ok(d) = serde_json::from_value::<ImportDoc>(op.body) else {
                continue;
            };
            k.execute(
                "INSERT INTO imported(op_device, op_seq, uid, source, source_id, kind, repo,
                   session, ts, title, body)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    device,
                    op.op_seq,
                    d.uid,
                    d.source,
                    d.source_id,
                    d.kind,
                    d.repo,
                    d.session,
                    d.ts,
                    d.title,
                    d.body
                ],
            )?;
            k.execute(
                "INSERT INTO imported_fts(rowid, text) VALUES(?1, ?2)",
                params![k.last_insert_rowid(), format!("{}\n{}", d.title, d.body)],
            )?;
        }
        Ok(last)
    }

    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        schema(k)?;
        k.execute(
            "DELETE FROM imported_fts WHERE rowid IN
               (SELECT rowid FROM imported WHERE op_device = ?1 AND op_seq > ?2)",
            params![device, to],
        )?;
        k.execute(
            "DELETE FROM imported WHERE op_device = ?1 AND op_seq > ?2",
            params![device, to],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source_id: &str, title: &str, body: &str) -> ImportDoc {
        ImportDoc {
            uid: format!("claude-mem:abc:{source_id}"),
            source: "claude-mem:abc".into(),
            source_id: source_id.into(),
            kind: "decision".into(),
            repo: "claude-mem:free-mem".into(),
            session: "s1".into(),
            ts: 1_000,
            title: title.into(),
            body: body.into(),
        }
    }

    fn count(k: &Connection, table: &str) -> i64 {
        k.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn imported_fts_finds_an_imported_title() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_imports(vec![
            doc("o1", "Chose SQLite", "Because it is one file."),
            doc("o2", "Tabs", "Indent with tabs."),
        ])
        .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(Imported.step(&raw, &k, &device, 0).unwrap(), 2);
        let uid: String = k
            .query_row(
                "SELECT i.uid FROM imported_fts f JOIN imported i ON i.rowid = f.rowid
                 WHERE imported_fts MATCH 'SQLite'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uid, "claude-mem:abc:o1");
    }

    /// D5: a restore that lost ops takes their documents out with them, and only theirs. The ops
    /// that take their seqs read in again under the freed rowids, which the contentless index
    /// takes back (OpenCodeReview on #305).
    #[test]
    fn a_rewind_removes_the_rows_of_the_lost_ops_only() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_imports(vec![doc("o1", "Kept", "One.")]).unwrap();
        raw.append_imports(vec![doc("o2", "Lost", "Two.")]).unwrap();
        raw.append_imports(vec![doc("o3", "Lost", "Three.")])
            .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(Imported.step(&raw, &k, &device, 0).unwrap(), 3);
        Imported.rewind(&k, &device, 1).unwrap();
        assert_eq!((count(&k, "imported"), count(&k, "imported_fts")), (1, 1));
        let left: String = k
            .query_row(
                "SELECT i.title FROM imported_fts f JOIN imported i ON i.rowid = f.rowid",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, "Kept");
        assert_eq!(Imported.step(&raw, &k, &device, 1).unwrap(), 3);
        let found: Vec<String> = k
            .prepare(
                "SELECT i.source_id FROM imported_fts f JOIN imported i ON i.rowid = f.rowid
                 WHERE imported_fts MATCH 'Three' ORDER BY i.rowid",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(found, ["o3"]);
        k.execute(
            "INSERT INTO imported_fts(imported_fts) VALUES('integrity-check')",
            [],
        )
        .unwrap();
    }
}
