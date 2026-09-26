//! Design B's worker (docs/milestone-2-plan.md D6, D10; MUST-M14): one per home, started by hooks,
//! it runs this milestone's consumers over `raw.db` in seq order and exits when idle.

use crate::knowledge::checkpoint;
use crate::raw::Raw;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::time::{Duration, Instant};

/// One derived view of raw.db. `step` processes records of this device after `after` and returns
/// its new checkpoint; `rewind` deletes its output above `to`. Both run inside the knowledge.db
/// transaction that also moves the checkpoint (D10).
pub trait Consumer {
    fn name(&self) -> &'static str;
    fn step(&mut self, raw: &Raw, k: &Connection, after: i64) -> Result<i64>;
    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()>;
}

/// This milestone's consumers, in order.
pub fn consumers(home: &Path) -> Vec<Box<dyn Consumer>> {
    // Compression last: it waits for every consumer before it.
    vec![
        // The rescan first: the tombstones it appends are in raw before the others read a record.
        Box::new(crate::consumer::rescan::Rescan::new(home)),
        Box::new(crate::consumer::fts::Fts),
        Box::new(crate::consumer::manifest::Manifest),
        Box::new(crate::consumer::compress::Compress),
    ]
}

/// Runs each consumer from its checkpoint until none moves; each step and its checkpoint move
/// share one knowledge.db transaction. A step may move its checkpoint back (the rescan starting
/// again under new rules).
#[cfg(test)] // the worker checks the backup deadline between passes
pub fn drain(raw: &Raw, k: &mut Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<()> {
    while pass(raw, k, consumers)? {}
    Ok(())
}

/// One batch for each consumer: whether any checkpoint moved.
fn pass(raw: &Raw, k: &mut Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<bool> {
    let device = raw.device().to_owned();
    let mut advanced = false;
    for c in consumers.iter_mut() {
        let tx = k.transaction()?;
        let at = checkpoint::get(&tx, c.name(), &device)?;
        let next = c.step(raw, &tx, at)?;
        if next != at {
            checkpoint::set(&tx, c.name(), &device, next)?;
            advanced = true;
        }
        tx.commit()?;
    }
    Ok(advanced)
}

/// The per-home worker lock, `<home>/state/worker.lock`; released when dropped.
pub struct Lock(#[allow(dead_code)] std::fs::File);

/// The lock, or `None` when another process holds it. Hooks try it too, and start a worker only
/// when they get it (dropping it at once).
pub fn lock(home: &Path) -> Result<Option<Lock>> {
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("worker.lock"))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(Lock(f))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// How often a worker looks for new records while it waits.
const POLL: Duration = Duration::from_millis(200);

/// Whether any consumer's checkpoint for this device is below raw's highest seq.
fn behind(raw: &Raw, k: &Connection, consumers: &[Box<dyn Consumer>]) -> Result<bool> {
    let top = raw.max_seq()?;
    for c in consumers {
        if checkpoint::get(k, c.name(), raw.device())? < top {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Take the lock (or return at once), check both stores, rewind, drain, then wait for new records
/// until `idle_ms` passes with none. At idle the lock is released first and raw is checked once
/// more (D6): a hook that appended before the release saw the lock held and started nothing, so
/// this last check is what finds its record; one that appends after the release gets the lock and
/// starts a worker itself. `before_exit` runs between the release and that check (a test seam).
pub fn run_with(
    home: &Path,
    idle_ms: u64,
    mut consumers: Vec<Box<dyn Consumer>>,
    mut before_exit: impl FnMut(),
) -> Result<()> {
    let Some(mut held) = lock(home)? else {
        return Ok(());
    };
    // Task 8: a damaged raw.db is restored from the backups, a damaged knowledge.db rebuilt.
    let raw = crate::backup::open_raw(home)?;
    let mut k = crate::backup::open_knowledge(home)?;
    checkpoint::rewind(&raw, &k, &mut consumers)?;
    crate::backup::check(home, &raw);
    // ponytail: D11's 30-minute deadline in memory; every idle exit backs up too, so a lost
    // deadline only brings the next backup forward. It is checked between batches and while
    // idle, so neither a long backlog nor a long idle wait puts it off.
    let mut next_backup = Instant::now() + crate::backup::EVERY;
    let mut due = |raw: &Raw| {
        if Instant::now() >= next_backup {
            crate::backup::run(home, raw);
            next_backup = Instant::now() + crate::backup::EVERY;
        }
    };
    loop {
        while pass(&raw, &mut k, &mut consumers)? {
            due(&raw);
        }
        due(&raw);
        let seen = raw.max_seq()?;
        let deadline = Instant::now() + Duration::from_millis(idle_ms);
        let mut more = false;
        while Instant::now() < deadline {
            std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
            if raw.max_seq()? > seen {
                more = true;
                break;
            }
            due(&raw);
        }
        if more {
            continue;
        }
        // Under the lock: a worker started after the release cannot export the same seqs.
        crate::backup::run(home, &raw);
        drop(held);
        before_exit();
        if !behind(&raw, &k, &consumers)? {
            return Ok(());
        }
        match lock(home)? {
            Some(l) => held = l,
            // Another worker took the lock after the release: the records are its now.
            None => return Ok(()),
        }
    }
}

pub fn run(home: &Path, idle_ms: u64) -> Result<()> {
    run_with(home, idle_ms, consumers(home), || {})
}

#[allow(dead_code)] // Task 12's replay drains without waiting.
pub fn run_once(home: &Path) -> Result<()> {
    run_with(home, 0, consumers(home), || {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge;
    use crate::raw;

    /// A consumer that writes each seq it sees into knowledge.db, so these tests need no index
    /// (Task 6).
    struct Seen;
    impl Consumer for Seen {
        fn name(&self) -> &'static str {
            "seen"
        }
        fn step(&mut self, raw: &Raw, k: &Connection, after: i64) -> Result<i64> {
            k.execute(
                "CREATE TABLE IF NOT EXISTS seen(device TEXT, seq INTEGER)",
                [],
            )?;
            let recs = raw.after(raw.device(), after, 100)?;
            for r in &recs {
                k.execute("INSERT INTO seen VALUES (?1, ?2)", (&r.device, r.seq))?;
            }
            Ok(recs.last().map_or(after, |r| r.seq))
        }
        fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
            k.execute(
                "DELETE FROM seen WHERE device = ?1 AND seq > ?2",
                (device, to),
            )?;
            Ok(())
        }
    }

    fn seen(k: &Connection) -> Vec<i64> {
        let mut st = k.prepare("SELECT seq FROM seen ORDER BY seq").unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// raw.db as if its commits above `seq` had never reached the disk (MUST-M14).
    fn lose_after(home: &Path, seq: i64) -> Raw {
        let c = rusqlite::Connection::open(home.join("raw.db")).unwrap();
        c.execute("DELETE FROM records WHERE seq > ?1", [seq])
            .unwrap();
        drop(c);
        raw::open(home).unwrap()
    }

    #[test]
    fn a_checkpoint_above_raw_is_rewound_with_its_output_and_later_events_are_not_skipped() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let mut k = knowledge::open(home.path()).unwrap();
        let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Seen)];
        for i in 0..8 {
            raw.append(&raw::test_event(&i.to_string())).unwrap();
        }
        drain(&raw, &mut k, &mut consumers).unwrap(); // output and checkpoint at 8
        drop(raw);
        let mut raw = lose_after(home.path(), 5);
        assert_eq!(
            checkpoint::rewind(&raw, &k, &mut consumers).unwrap(),
            vec![("seen".into(), 8, 5)]
        );
        assert_eq!(seen(&k), vec![1, 2, 3, 4, 5]); // no output left for the lost 6-8
        let logged: (String, i64, i64) = k
            .query_row("SELECT consumer, was, now FROM rewinds", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(logged, ("seen".into(), 8, 5));
        raw.append(&raw::test_event("new")).unwrap(); // seq 6 again, a different event
        drain(&raw, &mut k, &mut consumers).unwrap();
        assert_eq!(seen(&k), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn an_event_that_arrives_while_the_worker_decides_to_exit_is_still_processed() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let mut late = true;
        // `before_exit` runs after the lock is released and before the last check: a hook
        // appending here finds the lock free, but this test starts no second worker, so only the
        // last check can pick the event up.
        run_with(p, 0, vec![Box::new(Seen)], || {
            if std::mem::take(&mut late) {
                raw::open(p)
                    .unwrap()
                    .append(&raw::test_event("late"))
                    .unwrap();
            }
        })
        .unwrap();
        let device = raw::open(p).unwrap().device().to_owned();
        let k = knowledge::open(p).unwrap();
        assert_eq!(checkpoint::get(&k, "seen", &device).unwrap(), 2);
    }

    #[test]
    fn a_second_worker_exits_at_once() {
        let home = tempfile::tempdir().unwrap();
        let _held = lock(home.path()).unwrap().expect("the first lock");
        assert!(lock(home.path()).unwrap().is_none());
        let t = Instant::now();
        run(home.path(), 60_000).unwrap(); // returns at once: the lock is held
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_worker_picks_up_records_that_arrive_while_it_waits() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().to_path_buf();
        raw::open(&p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let writer = {
            let p = p.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                raw::open(&p)
                    .unwrap()
                    .append(&raw::test_event("second"))
                    .unwrap();
            })
        };
        run_with(&p, 1_500, vec![Box::new(Seen)], || {}).unwrap();
        writer.join().unwrap();
        let device = raw::open(&p).unwrap().device().to_owned();
        let k = knowledge::open(&p).unwrap();
        assert_eq!(checkpoint::get(&k, "seen", &device).unwrap(), 2);
        assert_eq!(seen(&k), vec![1, 2]);
    }
}
