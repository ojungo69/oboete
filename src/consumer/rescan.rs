//! Milestone 2 Task 7, part b (spec 2.2, D8): when the redaction rules change (a user rule added,
//! a kept value removed), the records stored under the old ones are scanned again, and each new
//! finding gets a range tombstone. `Raw::after` then masks it on every read, and the consumers
//! that index a record read it again when they reach its tombstone.

use crate::raw::{Item, Raw, Target};
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub struct Rescan {
    pub home: PathBuf,
    /// A second handle on raw.db, for the tombstones (the worker's own is read-only here).
    writer: Option<Raw>,
}

impl Rescan {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            writer: None,
        }
    }
}

/// Records per step, as the other consumers.
const BATCH: usize = 500;

fn schema(k: &Connection) -> Result<()> {
    // The ruleset each device's records were last scanned with.
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS rescan(device TEXT PRIMARY KEY, version TEXT NOT NULL);",
    )?;
    Ok(())
}

impl Consumer for Rescan {
    fn name(&self) -> &'static str {
        "rescan"
    }

    /// With the rules the records were last scanned with, it only follows new records (capture
    /// scanned them). With other rules, it starts again from seq 1: its checkpoint moves back,
    /// which the worker takes from any consumer.
    fn step(&mut self, raw: &Raw, k: &Connection, after: i64) -> Result<i64> {
        schema(k)?;
        // Settings that do not load stop capture too, and doctor names them: nothing to do.
        let Ok(settings) = crate::capture::Settings::load(&self.home) else {
            return Ok(after);
        };
        let version = settings.rules.version();
        let device = raw.device();
        let scanned: Option<String> = k
            .query_row(
                "SELECT version FROM rescan WHERE device = ?1",
                [device],
                |r| r.get(0),
            )
            .optional()?;
        let from = if scanned.as_deref() == Some(version) {
            after
        } else {
            k.execute(
                "INSERT INTO rescan(device, version) VALUES(?1, ?2)
                 ON CONFLICT(device) DO UPDATE SET version = excluded.version",
                params![device, version],
            )?;
            0
        };
        let recs = raw.after(device, from, BATCH)?;
        for r in &recs {
            let Item::Event(e) = &r.item else { continue };
            for (start, end) in crate::redact::field_ranges(&e.body, &settings.rules) {
                if self.writer.is_none() {
                    self.writer = Some(crate::raw::open(&self.home)?);
                }
                let w = self.writer.as_mut().expect("opened");
                w.append_tombstone(Target::Range {
                    device: device.to_owned(),
                    seq: r.seq,
                    offset: start as i64,
                    length: (end - start) as i64,
                })?;
            }
        }
        Ok(recs.last().map_or(from, |r| r.seq))
    }

    /// Tombstones it wrote may be among the lost commits: the next step scans from seq 1 again.
    fn rewind(&mut self, k: &Connection, device: &str, _to: i64) -> Result<()> {
        schema(k)?;
        k.execute("DELETE FROM rescan WHERE device = ?1", [device])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{raw, worker};

    fn tombstones(raw: &Raw) -> usize {
        raw.after(raw.device(), 0, 1000)
            .unwrap()
            .iter()
            .filter(|r| matches!(r.item, Item::Tombstone(_)))
            .count()
    }

    fn body(raw: &Raw, seq: i64) -> String {
        match &raw.after(raw.device(), seq - 1, 1).unwrap()[0].item {
            Item::Event(e) => e.body.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_new_rule_tombstones_old_records_once() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let old = raw
            .append(&raw::test_event(r#"{"prompt":"deploy acme-123456 now"}"#))
            .unwrap();
        // Masked at capture: no finding of its own again.
        let masked = raw
            .append(&raw::test_event(r#"{"prompt":"token=[REDACTED] here"}"#))
            .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 0); // the rules the records were stored under
        std::fs::write(
            p.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert_eq!(body(&raw, old), r#"{"prompt":"deploy *********** now"}"#);
        assert_eq!(body(&raw, masked), r#"{"prompt":"token=[REDACTED] here"}"#);
        assert!(
            crate::search::raw(p, "acme-123", None, 5)
                .unwrap()
                .is_empty()
        );
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1); // once
        // Lost commits took the tombstone (MUST-M14): the rewind scans again from seq 1.
        let top = raw.max_seq().unwrap();
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq = ?1", [top])
            .unwrap();
        assert!(body(&raw, old).contains("acme-123456"));
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, old).contains("acme"));
    }

    #[test]
    fn a_rule_anchored_to_a_field_matches_as_capture_would() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let seq = raw
            .append(&raw::test_event(
                r#"{"prompt":"deploy acme-123456","cwd":"/r"}"#,
            ))
            .unwrap();
        worker::run_once(p).unwrap();
        std::fs::write(
            p.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}$' }]\n",
        )
        .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(
            body(&raw, seq),
            r#"{"prompt":"deploy ***********","cwd":"/r"}"#
        );
    }

    #[test]
    fn a_restore_that_skips_the_tombstone_masks_its_target_again() {
        // #83, raised on #110: the tombstone is sealed in a later segment than its target.
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event(r#"{"prompt":"deploy acme-123456 now"}"#))
            .unwrap();
        crate::backup::export(p).unwrap();
        std::fs::write(
            p.join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        worker::run_once(p).unwrap();
        crate::backup::export(p).unwrap();
        drop(raw);
        let later = std::fs::read_dir(p.join("backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|f| f.to_string_lossy().ends_with("-000000000002.seg.zst"))
            .unwrap();
        std::fs::write(&later, b"damaged").unwrap();
        std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        worker::run_once(p).unwrap(); // restores seq 1 only, then the rescan runs from seq 1
        let raw = raw::open(p).unwrap();
        assert!(!body(&raw, 1).contains("acme"), "{}", body(&raw, 1));
        assert!(
            crate::search::raw(p, "acme-123", None, 5)
                .unwrap()
                .is_empty()
        );
    }
}
