//! A hook's time (docs/hook-time.md). An agent cancels a hook at its timeout (Claude Code's prompt
//! hook at 5 s where an older `setup` wrote it, at 15 s since decision 108), and what the hook
//! has not done by then is lost: its answer, the shown set, the counts doctor reads. Two things
//! here keep a hook inside its time, and say where the time went when it was not:
//! - One deadline for what a hook injects (`Clock::by`): `setup::hook_timeout`, the least `setup`
//!   has written for the agent's event (it writes a longer one now, and an older settings file
//!   still holds this one), less `MARGIN`, counted from the process's start. Recording is never
//!   cut short by it
//!   (MUST-M16, docs/unwritten.md); the waits of what it injects take what remains: the read
//!   fence's, the prompt point's for the viewer, a file's note's and a memory lookup's. A local
//!   read is not cut.
//! - A record of slow hooks (`state/hook-time.jsonl`). A cancelled hook writes nothing at its
//!   end, so a thread beside the hook writes a line while it still runs: at `SLOW`, and once more
//!   past its deadline. A hook that took `SLOW` or more writes a line at its end too. A line
//!   holds the phase in flight and when each phase began: names and numbers, never text, a path
//!   or a session.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A hook still running this long after its start is written down.
pub(crate) const SLOW: Duration = Duration::from_secs(2);
/// What a hook keeps of the timeout its time is counted from (`setup::hook_timeout`) for its own
/// end (the marker, the worker's start, its answer on stdout) and for the time before `main`.
pub(crate) const MARGIN: Duration = Duration::from_secs(1);
/// The record's size at which it starts again, the one before it kept beside it.
const CAP: u64 = 256 * 1024;
const FILE: &str = "hook-time.jsonl";
/// Days doctor's line counts.
const DAYS: i64 = 7;

/// What a hook does, in the order it does it; `Clock::begin` marks each as it begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The payload from stdin, and whose it is.
    Parse,
    /// A memory lookup's search (docs/memory-lookup.md).
    Lookup,
    /// The note on a file about to be read (docs/file-note.md).
    Note,
    Settings,
    /// raw.db, waited for.
    Open,
    Record,
    /// The read fence, waited for.
    Fence,
    Manifest,
    /// The prompt point (docs/prompt-context.md).
    Prompt,
    /// The marker and the worker's start.
    After,
    /// What the agent is given, assembled and written.
    Output,
}

const NAMES: [&str; 11] = [
    "parse", "lookup", "note", "settings", "open", "record", "fence", "manifest", "prompt",
    "after", "output",
];

static PROCESS: OnceLock<(Instant, i64)> = OnceLock::new();

/// Stamps the process's start: `main`'s first line. A process that never called it (a test)
/// starts at its first use.
pub(crate) fn init() {
    process();
}

fn process() -> (Instant, i64) {
    *PROCESS.get_or_init(|| (Instant::now(), crate::db::now_ms()))
}

/// One hook's clock: its start, when each phase began, and the end of its time for what it
/// injects. Marks are atomics, read by the thread `watch` starts: a hook inside its time does no
/// I/O for them.
pub(crate) struct Clock {
    start: Instant,
    wall_ms: i64,
    /// Whether `start` is the process's, so the time before it can be read.
    process: bool,
    slow: Duration,
    /// ms after `start` at which the time for what the hook injects is over, plus one; 0 is none.
    by: AtomicU32,
    /// ms after `start` at which each phase began, plus one; 0 is a phase not begun.
    began: [AtomicU32; NAMES.len()],
    doing: AtomicU8,
}

impl Clock {
    /// The clock of a hook that is its process: it started at `main`'s first line.
    pub(crate) fn of_process() -> Arc<Self> {
        let (start, wall_ms) = process();
        Arc::new(Self::at(start, wall_ms, true, SLOW))
    }

    /// The clock of a hook run inside a test: it starts now, with no agent's timeout to keep.
    #[cfg(test)]
    pub(crate) fn starting_now() -> Arc<Self> {
        Arc::new(Self::at(Instant::now(), crate::db::now_ms(), false, SLOW))
    }

    /// A clock that started `ago`, slow at `slow`.
    #[cfg(test)]
    pub(crate) fn started(ago: Duration, slow: Duration) -> Arc<Self> {
        let wall_ms = crate::db::now_ms() - i64::try_from(ago.as_millis()).unwrap_or(0);
        Arc::new(Self::at(Instant::now() - ago, wall_ms, false, slow))
    }

    /// A clock that started `ago` and keeps its agent's timeout, as a hook that is its process.
    #[cfg(test)]
    pub(crate) fn started_process(ago: Duration) -> Arc<Self> {
        let wall_ms = crate::db::now_ms() - i64::try_from(ago.as_millis()).unwrap_or(0);
        Arc::new(Self::at(Instant::now() - ago, wall_ms, true, SLOW))
    }

    fn at(start: Instant, wall_ms: i64, process: bool, slow: Duration) -> Self {
        let clock = Self {
            start,
            wall_ms,
            process,
            slow,
            by: AtomicU32::new(0),
            began: Default::default(),
            doing: AtomicU8::new(0),
        };
        clock.began[Phase::Parse as usize].store(1, Ordering::Relaxed);
        clock
    }

    /// Whether this hook is its process, which an agent's timeout ends: `run_stdin` sets its time
    /// for what it injects from `setup::hook_timeout`.
    pub(crate) fn keeps_timeouts(&self) -> bool {
        self.process
    }

    fn ms(&self) -> u32 {
        u32::try_from(self.start.elapsed().as_millis())
            .unwrap_or(u32::MAX)
            .min(u32::MAX - 1)
    }

    /// `phase` begins now, and the one before it has ended.
    pub(crate) fn begin(&self, phase: Phase) {
        self.began[phase as usize].store(self.ms() + 1, Ordering::Relaxed);
        self.doing.store(phase as u8, Ordering::Relaxed);
    }

    /// The hook's time for what it injects ends `timeout` less `MARGIN` after its start: `timeout`
    /// is `setup::hook_timeout`'s, which the agent's own may be longer than. An agent whose
    /// timeout oboete does not write has none: `None` leaves the hook's waits as they are.
    pub(crate) fn inject_within(&self, timeout: Option<Duration>) {
        let by = timeout.map_or(0, |t| {
            u32::try_from(t.saturating_sub(MARGIN).as_millis())
                .unwrap_or(u32::MAX)
                .min(u32::MAX - 1)
                + 1
        });
        self.by.store(by, Ordering::Relaxed);
    }

    /// When the hook's time for what it injects is over, if it has one.
    pub(crate) fn by(&self) -> Option<Instant> {
        match self.by.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(self.start + Duration::from_millis(u64::from(ms - 1))),
        }
    }

    /// Whether the hook's time for what it injects is over: it injects nothing more.
    pub(crate) fn over(&self) -> bool {
        self.by().is_some_and(|by| Instant::now() >= by)
    }

    /// `wait`, or what remains of the hook's time for what it injects when that is less.
    pub(crate) fn left(&self, wait: Duration) -> Duration {
        match self.by() {
            Some(by) => wait.min(by.saturating_duration_since(Instant::now())),
            None => wait,
        }
    }

    /// The earlier of `at` and the end of the hook's time for what it injects.
    pub(crate) fn no_later(&self, at: Instant) -> Instant {
        self.by().map_or(at, |by| by.min(at))
    }

    /// Starts the thread beside a hook that is its process. It writes a line when the hook still
    /// runs at `slow`, and one more half of `MARGIN` past its deadline: the last look, half of
    /// `MARGIN` before the timeout the hook's time is counted from, which is where an older
    /// settings file cancels it. The thread ends with the process: a hook inside its time never
    /// hears it.
    pub(crate) fn watch(self: &Arc<Self>, home: &Path, agent: &str, event: &str) {
        let clock = Arc::clone(self);
        let (home, agent, event) = (home.to_owned(), agent.to_owned(), event.to_owned());
        let spawned = std::thread::Builder::new()
            .name("hook-time".into())
            .spawn(move || {
                sleep_until(clock.start + clock.slow);
                clock.write(&home, &agent, &event, "slow");
                if let Some(by) = clock.by() {
                    sleep_until(by + MARGIN / 2);
                    clock.write(&home, &agent, &event, "late");
                }
            });
        if let Err(e) = spawned {
            eprintln!("oboete: the hook's time is not watched: {e}");
        }
    }

    /// The hook's end: a line when it took `slow` or more, as every hook the thread wrote of did.
    pub(crate) fn finish(&self, home: &Path, agent: &str, event: &str) {
        if self.start.elapsed() >= self.slow {
            self.write(home, agent, event, "end");
        }
    }

    /// The phases begun so far, in order.
    #[cfg(test)]
    pub(crate) fn begun(&self) -> Vec<&'static str> {
        (NAMES.iter().zip(&self.began))
            .filter(|(_, at)| at.load(Ordering::Relaxed) > 0)
            .map(|(name, _)| *name)
            .collect()
    }

    fn write(&self, home: &Path, agent: &str, event: &str, at: &str) {
        if let Err(e) = append(home, &self.line(agent, event, at)) {
            eprintln!("oboete: the hook's time is not written: {e}");
        }
    }

    fn line(&self, agent: &str, event: &str, at: &str) -> Value {
        let began: serde_json::Map<String, Value> = (NAMES.iter().zip(&self.began))
            .filter_map(|(name, at)| {
                let ms = at.load(Ordering::Relaxed);
                (ms > 0).then(|| ((*name).to_owned(), json!(ms - 1)))
            })
            .collect();
        json!({
            "ts": self.wall_ms,
            "pid": std::process::id(),
            "agent": name(agent),
            "event": name(event),
            "at": at,
            "ms": self.ms(),
            "before_ms": self.process.then(|| before_ms(self.start)).flatten(),
            "phase": NAMES[usize::from(self.doing.load(Ordering::Relaxed)).min(NAMES.len() - 1)],
            "began": began,
        })
    }
}

/// An agent's or an event's name as the record keeps it: its letters, digits, `-` and `_`, 32 at
/// most. They are the command line's, so nothing else is taken from them.
fn name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(32)
        .collect()
}

fn sleep_until(at: Instant) {
    let left = at.saturating_duration_since(Instant::now());
    if !left.is_zero() {
        std::thread::sleep(left);
    }
}

/// How long this process existed before `start`, its creation to `main`'s first line (the loader,
/// a binary read back from disk): Linux only.
#[cfg(target_os = "linux")]
fn before_ms(start: Instant) -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let up = std::fs::read_to_string("/proc/uptime").ok()?;
    // SAFETY: sysconf reads a constant of the system and takes no pointer.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    before_from(&stat, &up, hz as f64, start.elapsed().as_secs_f64())
}

/// `before_ms` of `/proc/self/stat`'s and `/proc/uptime`'s text, the clock ticks a second, and the
/// seconds since `main`'s first line.
#[cfg(target_os = "linux")]
fn before_from(stat: &str, uptime: &str, hz: f64, since_start: f64) -> Option<u64> {
    // Past the command's name, which may hold spaces and parentheses, the 20th field is the
    // process's start in clock ticks since boot (proc(5): starttime, field 22).
    let fields = stat.rsplit_once(')')?.1;
    let ticks: f64 = fields.split_whitespace().nth(19)?.parse().ok()?;
    let up: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    let before = up - ticks / hz - since_start;
    // Both clocks tick in hundredths of a second: a small negative is zero.
    (hz > 0.0).then(|| (before.max(0.0) * 1000.0).round() as u64)
}

#[cfg(not(target_os = "linux"))]
fn before_ms(_: Instant) -> Option<u64> {
    None
}

/// One line onto `state/hook-time.jsonl`, the file turned over at `CAP`. No home is made for it:
/// a hook that records nothing writes nothing here either.
fn append(home: &Path, line: &Value) -> std::io::Result<()> {
    if !home.is_dir() {
        return Ok(());
    }
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    let path = state.join(FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= CAP) {
        turn_over(&path, &state.join(format!("{FILE}.1")))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path)?;
    // A file that was there before (restored, or copied in) is the person's alone too.
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    // One write: lines of hooks that end together do not mix. It begins on a line of its own, so
    // a line before it that was cut short (a full disk, a hook killed as it wrote) takes only
    // itself with it.
    file.write_all(format!("\n{line}\n").as_bytes())
}

/// The full record becomes the one kept beside it. Two hooks at once may both turn it over: the
/// second finds no file, which is the first's turn-over and no error (its line begins the new
/// file), or replaces what the first kept, a diagnostic's loss and not a record's. Any other
/// failure is one: nothing is written past the cap.
fn turn_over(path: &Path, kept: &Path) -> std::io::Result<()> {
    match std::fs::rename(path, kept) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Doctor's line: the hooks on record in the last 7 days that still ran at `SLOW`, those of them
/// that never ended (the agent cancelled them, or they were killed), and the phase most of them
/// were in. The record is two files of `CAP`: past that many slow hooks, the newest are counted.
pub(crate) fn doctor_line(home: &Path, now: i64) -> Option<String> {
    let state = home.join("state");
    let since = now - DAYS * 86_400_000;
    // A hook is its start and its process: the phase it was in when last seen running, and
    // whether its end came.
    let mut hooks: BTreeMap<(i64, u64), (Option<String>, bool)> = BTreeMap::new();
    for file in [format!("{FILE}.1"), FILE.to_owned()] {
        let text = std::fs::read_to_string(state.join(file)).unwrap_or_default();
        for v in (text.lines()).filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            let (Some(ts), Some(pid)) = (v["ts"].as_i64(), v["pid"].as_u64()) else {
                continue;
            };
            if ts < since {
                continue;
            }
            let hook = hooks.entry((ts, pid)).or_default();
            if v["at"] == "end" {
                hook.1 = true;
                // An end with no line before it: the phase that took longest.
                hook.0 = hook.0.take().or_else(|| longest(&v));
            } else {
                hook.0 = v["phase"].as_str().map(str::to_owned);
            }
        }
    }
    if hooks.is_empty() {
        return None;
    }
    let unended = hooks.values().filter(|h| !h.1).count();
    let mut phases: BTreeMap<&str, usize> = BTreeMap::new();
    for (phase, _) in hooks.values() {
        *phases.entry(phase.as_deref().unwrap_or("?")).or_default() += 1;
    }
    // The most frequent, the earlier phase on a tie.
    let most = (NAMES.iter().chain(&["?"]))
        .filter_map(|n| phases.get(n).map(|c| (*c, *n)))
        .rev()
        .max_by_key(|(c, _)| *c)
        .map_or("?", |(_, n)| n);
    Some(format!(
        "slow hooks: {} on record in the last {DAYS} days still ran at {} s, {unended} of them never ended (cancelled or killed); most often in: {most}",
        hooks.len(),
        SLOW.as_secs(),
    ))
}

/// The phase of an end line that took longest: each runs from its beginning to the next one's,
/// the last to the line's `ms`.
fn longest(v: &Value) -> Option<String> {
    let began = v["began"].as_object()?;
    let mut at: Vec<(u64, &str)> = (NAMES.iter())
        .filter_map(|n| Some((began.get(*n)?.as_u64()?, *n)))
        .collect();
    at.sort();
    let end = v["ms"].as_u64()?;
    (at.iter().enumerate())
        .map(|(i, (from, n))| {
            let to = at.get(i + 1).map_or(end, |next| next.0);
            (to.saturating_sub(*from), *n)
        })
        // The earlier phase on a tie.
        .rev()
        .max_by_key(|(took, _)| *took)
        .map(|(_, n)| n.to_owned())
}

#[cfg(test)]
mod tests;
