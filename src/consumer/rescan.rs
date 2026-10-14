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
}

impl Rescan {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
        }
    }
}

/// Records per step, as the other consumers.
const BATCH: usize = 500;

/// How long a step waits for the dispatch hold before it leaves its batch to the next pass: short,
/// as the consumers after it in the same pass (full text first) wait for it.
const HOLD_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

#[cfg(test)]
thread_local! {
    /// What happens to the home between a step's read of its batch and its tombstones.
    static BEFORE_TOMBSTONES: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// What happens to the home once a step has checked the rules again, before its tombstones.
    static WHILE_TOMBSTONES: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

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
    fn step(&mut self, raw: &Raw, k: &Connection, device: &str, after: i64) -> Result<i64> {
        self.step_report(raw, k, device, after, &mut || {})
    }

    fn step_report(
        &mut self,
        raw: &Raw,
        k: &Connection,
        _device: &str,
        after: i64,
        committed: &mut dyn FnMut(),
    ) -> Result<i64> {
        schema(k)?;
        // Settings that do not load stop capture too, and doctor names them. The batch is passed
        // over (a checkpoint that never moves keeps the worker from exiting), and the version is
        // forgotten, so the first pass that loads them scans raw again from seq 1.
        let Ok(settings) = crate::capture::Settings::load(&self.home) else {
            k.execute("DELETE FROM rescan WHERE device = ?1", [raw.device()])?;
            let recs = raw.after(raw.device(), after, BATCH)?;
            return Ok(recs.last().map_or(after, |r| r.seq));
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
            // Rules turned off find nothing in what is stored: no pass from seq 1 for them, and
            // turning them on again starts one (owner decision 43).
            if settings.rules.is_off() { after } else { 0 }
        };
        let recs = raw.after(device, from, BATCH)?;
        let found: Vec<(i64, usize, usize)> = recs
            .iter()
            .filter_map(|r| match &r.item {
                Item::Event(e) => Some((r.seq, e)),
                _ => None,
            })
            .flat_map(|(seq, e)| {
                crate::redact::field_ranges(&e.body, &settings.rules)
                    .into_iter()
                    .map(move |(start, end)| (seq, start, end))
            })
            .collect();
        #[cfg(test)]
        if let Some(meanwhile) = BEFORE_TOMBSTONES.take() {
            meanwhile();
        }
        if found.is_empty() {
            return Ok(recs.last().map_or(from, |r| r.seq));
        }
        // A second handle on raw.db for the tombstones (the worker's own is read-only here),
        // closed with the step: a handle kept open would hold raw.lock, and a restore waits for
        // every open store to close.
        let mut writer = crate::raw::open_report(&self.home, None, &mut |_| {
            committed();
            Ok(())
        })?;
        // Rules changed while the batch was read (redaction turned off, say) write no tombstone:
        // one would mask for good a record a hook appended under the rules now. The version is
        // forgotten and the batch read again. The check and the tombstones are under the hold a
        // save that changes the rules takes too, so none changes them in between (owner decision
        // 43; Codex and CodeRabbit on #502).
        // A hold that does not come within its wait (readers holding it one after another, a
        // forget's registration) writes nothing and fails nothing: the batch is read again at the
        // next pass (Windows CI on #491).
        let _dispatch = match crate::dispatch::exclusive_within(&self.home, HOLD_WAIT) {
            Ok(held) => held,
            Err(e) if crate::dispatch::busy(&e) => return Ok(from),
            Err(e) => return Err(e.context("the rescan's tombstones were not written")),
        };
        if crate::capture::Settings::load(&self.home)
            .ok()
            .map(|s| s.rules.version().to_owned())
            .as_deref()
            != Some(version)
        {
            k.execute("DELETE FROM rescan WHERE device = ?1", [device])?;
            return Ok(after);
        }
        #[cfg(test)]
        if let Some(meanwhile) = WHILE_TOMBSTONES.take() {
            meanwhile();
        }
        for (seq, start, end) in found {
            writer.append_tombstone(Target::Range {
                device: device.to_owned(),
                seq,
                offset: start as i64,
                length: (end - start) as i64,
            })?;
            committed();
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

    /// Owner decision 43: a record stored while redaction is off, and read by the rescan then, is
    /// masked once redaction is on again.
    #[test]
    fn turning_redaction_on_again_masks_what_was_stored_while_it_was_off() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        worker::run_once(p).unwrap(); // the rescan's version is the bundled rules'
        std::fs::write(p.join("config.toml"), "[redaction]\nenabled = false\n").unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        // As capture stores it while off: as typed.
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 0);
        assert!(body(&raw, seq).contains(&key));
        std::fs::write(p.join("config.toml"), "[redaction]\nenabled = true\n").unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, seq).contains("ghp_"), "{}", body(&raw, seq));
    }

    /// Owner decision 43 (Codex on #502): rules turned off while a batch is read write no
    /// tombstone for it, so a record stored as typed under the switch off stays so.
    #[test]
    fn rules_turned_off_while_a_batch_is_read_write_no_tombstone() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        let config = p.join("config.toml");
        BEFORE_TOMBSTONES.set(Some(Box::new(move || {
            std::fs::write(&config, "[redaction]\nenabled = false\n").unwrap();
        })));
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 0);
        assert!(body(&raw, seq).contains(&key));
    }

    /// Owner decision 43 (CodeRabbit on #502): from its check of the rules to its last tombstone, a
    /// step holds what a save that changes them takes, so a save made in between waits until the
    /// tombstones of the rules it replaces are written.
    #[test]
    fn a_save_made_while_tombstones_are_written_waits_for_them() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_owned();
        let mut raw = raw::open(&p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        let saving = std::rc::Rc::new(std::cell::RefCell::new(None));
        let (started, at) = (std::rc::Rc::clone(&saving), p.clone());
        WHILE_TOMBSTONES.set(Some(Box::new(move || {
            crate::dispatch::blocked(&at); // no earlier wait is this save's
            let home = at.clone();
            // A save of the page turning redaction off: the hold, then config.toml.
            *started.borrow_mut() = Some(std::thread::spawn(move || {
                let _held = crate::dispatch::exclusive(&home).unwrap();
                std::fs::write(home.join("config.toml"), "[redaction]\nenabled = false\n").unwrap();
            }));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !crate::dispatch::blocked(&at) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the save did not wait"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })));
        worker::run_once(&p).unwrap();
        saving.borrow_mut().take().unwrap().join().unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, seq).contains("ghp_"), "{}", body(&raw, seq));
        let config = std::fs::read_to_string(p.join("config.toml")).unwrap();
        assert!(config.contains("enabled = false"), "{config}");
    }

    /// Windows CI on #491: a hold that does not come within its wait (readers holding dispatch one
    /// after another) writes no tombstone and fails no worker run; the next pass masks the record.
    #[test]
    fn a_busy_hold_leaves_the_batch_to_the_next_pass() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let seq = raw
            .append(&raw::test_event(&format!(
                r#"{{"prompt":"deploy {key} now"}}"#
            )))
            .unwrap();
        // A reader holds dispatch until the step has waited for it past its wait, then lets it go:
        // the first attempt is busy whatever the machine's speed (Codex on #507).
        let reader = crate::dispatch::Admission::shared(p).unwrap();
        crate::dispatch::blocked(p); // no earlier wait is this step's
        let at = p.to_owned();
        let released = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !crate::dispatch::blocked(&at) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the step never waited"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            std::thread::sleep(HOLD_WAIT * 2);
            drop(reader);
        });
        worker::run_once(p).unwrap();
        released.join().unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(tombstones(&raw), 1);
        assert!(!body(&raw, seq).contains("ghp_"), "{}", body(&raw, seq));
    }

    #[test]
    fn settings_that_do_not_load_neither_hold_the_worker_nor_skip_a_record() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
        std::fs::write(p.join("config.toml"), rule).unwrap();
        worker::run_once(p).unwrap(); // the rescan's version is this rule's
        // Stored under other rules (appended here without capture's gate).
        let seq = raw
            .append(&raw::test_event(r#"{"prompt":"deploy acme-123456 now"}"#))
            .unwrap();
        std::fs::write(p.join("config.toml"), "[redaction\n").unwrap();
        worker::run_once(p).unwrap(); // returns: the rescan passes over the batch
        assert_eq!(tombstones(&raw), 0);
        // The same rule back: the same version, and still the record passed over is scanned.
        std::fs::write(p.join("config.toml"), rule).unwrap();
        worker::run_once(p).unwrap();
        assert_eq!(body(&raw, seq), r#"{"prompt":"deploy *********** now"}"#);
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
