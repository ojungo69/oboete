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
        // A range the record's tombstones mask already reads as stars (`Raw::after`), which a
        // rule with no entropy gate takes for a password: no second tombstone (Codex on #535).
        let found: Vec<_> = if found.is_empty() {
            found
        } else {
            let masks = raw.masks()?;
            // A record's union is made once, for every range found in it.
            let mut unions = std::collections::HashMap::new();
            found
                .into_iter()
                .filter(|&(seq, start, end)| {
                    let Some((_, ranges)) = masks.get(&(device.to_owned(), seq)) else {
                        return true;
                    };
                    let runs = unions.entry(seq).or_insert_with(|| union(ranges));
                    !covered(runs, start, end)
                })
                .collect()
        };
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

/// A record's tombstoned ranges (offset, length) as their union: runs in order, those that touch
/// or overlap made one, so two masks next to each other are one run, as they read (#539). A range
/// this build cannot address (a damaged row, a 32-bit build) is in none.
fn union(ranges: &[(i64, i64)]) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = ranges
        .iter()
        .filter_map(|&(offset, length)| {
            let from = usize::try_from(offset).ok()?;
            Some((from, from.checked_add(usize::try_from(length).ok()?)?))
        })
        .collect();
    ranges.sort_unstable();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (from, to) in ranges {
        match runs.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => runs.push((from, to)),
        }
    }
    runs
}

/// Whether one of `runs` (a record's `union`) holds all of `start..end`.
fn covered(runs: &[(usize, usize)], start: usize, end: usize) -> bool {
    runs.iter().any(|&(from, to)| from <= start && end <= to)
}

#[cfg(test)]
mod tests;
