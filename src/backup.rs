//! Milestone 2 Task 8 (MUST-M15, spec 2.6, plan D11): raw.db's records as sealed, checksummed
//! zstd segments in the backup directory, and the quarantine and restore that use them.
//!
//! A segment is `<device>-<first seq>-<last seq>.seg.zst` with a `.sha256` beside it: the
//! records' backup lines (`Raw::export_lines`: as `Raw::after` returns them, masked, with their
//! ledger rows). The last backed-up seq is read from the names, so it survives a lost
//! knowledge.db.

use crate::raw::{self, Raw};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// D11: the most record bytes one segment holds (Task 12 may halve it).
pub const SEGMENT_BYTES: usize = 8 << 20;
/// D11: how often a running worker backs up besides at its idle exit.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// The most a segment may decompress to: its cap plus one record or one append of ops past it
/// (capture keeps a body far below this, `raw::MAX_BATCH_OPS` and `MAX_BATCH_BYTES` an append).
const MAX_SEGMENT_BYTES: u64 = (SEGMENT_BYTES as u64) + (128 << 20);

/// `[backup]` in config.toml (spec 1.5: the location is a user setting).
#[derive(Debug, Default, serde::Deserialize)]
struct Settings {
    #[serde(default)]
    backup: Location,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    dir: Option<PathBuf>,
}

/// The backup directory: `[backup] dir` (relative to the home), else `<home>/backups`. Only
/// `[backup]` is read, so a mistake elsewhere in config.toml does not stop backups.
pub fn dir(home: &Path) -> Result<PathBuf> {
    let path = home.join("config.toml");
    let dir = match std::fs::read_to_string(&path) {
        Ok(text) => location(&text)
            .with_context(|| format!("{}: [backup]", path.display()))?
            .map(|d| home.join(d)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    Ok(dir.unwrap_or_else(|| home.join("backups")))
}

/// `[backup] dir` as config.toml's text has it, as `dir` reads it.
pub(crate) fn location(text: &str) -> Result<Option<PathBuf>> {
    Ok(toml::from_str::<Settings>(text)?.backup.dir)
}

/// Records and ops are backed up alike, each in its own segments with its own cursor (milestone 3
/// D1): an op is often appended when no record is, and the records' cursor would never reach it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Records,
    Ops,
}

impl Kind {
    fn suffix(self) -> &'static str {
        match self {
            Kind::Records => ".seg.zst",
            Kind::Ops => ".ops.zst",
        }
    }
    /// This device's highest seq (records) or op seq (ops) in raw.
    fn top(self, raw: &Raw) -> Result<i64> {
        match self {
            Kind::Records => raw.max_seq(),
            Kind::Ops => raw.max_op_seq(),
        }
    }
    /// The first one raw holds after `at`.
    fn next(self, raw: &Raw, at: i64) -> Result<Option<i64>> {
        Ok(match self {
            Kind::Records => raw.after(raw.device(), at, 1)?.first().map(|r| r.seq),
            Kind::Ops => raw
                .ops_after(raw.device(), at, 1)?
                .first()
                .map(|o| o.op_seq),
        })
    }
    fn lines(self, raw: &Raw, at: i64) -> Result<Vec<(i64, String)>> {
        match self {
            Kind::Records => raw.export_lines(at, SEGMENT_BYTES),
            Kind::Ops => raw.export_op_lines(at, SEGMENT_BYTES),
        }
    }
}

/// One segment file, by the seqs its name gives.
struct Segment {
    device: String,
    first: i64,
    last: i64,
    path: PathBuf,
}

fn name(device: &str, first: i64, last: i64, kind: Kind) -> String {
    format!("{device}-{first:012}-{last:012}{}", kind.suffix())
}

/// The segments of `kind` in `dir`, in (device, first seq) order; files of other names are not
/// ours.
fn segments(dir: &Path, kind: Kind) -> Result<Vec<Segment>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(kind.suffix()))
        else {
            continue;
        };
        let mut parts = stem.rsplitn(3, '-');
        let (Some(last), Some(first), Some(device)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if let (Ok(first), Ok(last)) = (first.parse(), last.parse()) {
            out.push(Segment {
                device: device.to_owned(),
                first,
                last,
                path,
            });
        }
    }
    out.sort_by(|a, b| (&a.device, a.first).cmp(&(&b.device, b.first)));
    Ok(out)
}

/// The records of this home's raw.db above the last backed-up seq, then its ops above the last
/// backed-up op seq, as sealed segments (records first: a restore then never holds a window op
/// whose records it lacks).
/// Returns the last segment written, `None` when there was nothing new. (The worker calls `run`.)
#[cfg(test)]
pub fn export(home: &Path) -> Result<Option<PathBuf>> {
    Ok(export_from(&raw::open(home)?, &dir(home)?)?
        .pop()
        .map(|(p, _)| p))
}

/// `export`, with the time each segment took (Task 12 measures D11's segment cap with it).
pub fn export_timed(home: &Path) -> Result<Vec<(PathBuf, std::time::Duration)>> {
    export_from(&raw::open(home)?, &dir(home)?)
}

/// Each segment written, with the time it took to read, compress and seal.
fn export_from(raw: &Raw, dir: &Path) -> Result<Vec<(PathBuf, std::time::Duration)>> {
    let device = raw.device();
    anyhow::ensure!(
        !device.is_empty() && device.chars().all(|c| c.is_ascii_alphanumeric()),
        "device id {device:?} cannot name a segment"
    );
    let mut wrote = Vec::new();
    for kind in [Kind::Records, Kind::Ops] {
        let mut last = cursor(raw, dir, kind)?;
        if kind.top(raw)? <= last {
            continue;
        }
        if !dir.exists() {
            // Private when oboete makes it; a directory the user chose keeps its permissions.
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            crate::db::private(dir, 0o700);
        }
        loop {
            let started = std::time::Instant::now();
            let lines = kind.lines(raw, last)?;
            let (Some(first), Some(end)) = (lines.first(), lines.last()) else {
                break;
            };
            let (first, end) = (first.0, end.0);
            let mut text = String::new();
            for (_, l) in &lines {
                text.push_str(l);
                text.push('\n');
            }
            let path = seal(dir, &name(device, first, end, kind), text.as_bytes())?;
            wrote.push((path, started.elapsed()));
            last = end;
        }
    }
    Ok(wrote)
}

/// Where this device's backups end. MUST-M14: raw lost commits the backups hold, and its next
/// records reuse those seqs. The segments past raw's end are set aside, so that range is backed
/// up again from raw as it is reused (a loss that new records have already covered again goes
/// unseen here, as it does for the consumers' rewind: #83).
fn cursor(raw: &Raw, dir: &Path, kind: Kind) -> Result<i64> {
    let max = kind.top(raw)?;
    let mut mine: Vec<Segment> = segments(dir, kind)?
        .into_iter()
        .filter(|s| s.device == raw.device())
        .collect();
    mine.sort_by_key(|s| s.first);
    set_aside(mine.iter().filter(|s| s.last > max))?;
    mine.retain(|s| s.last <= max);
    // A range raw holds that no segment covers (a segment file was removed): the segments after
    // it are set aside too, so the next export writes that range and the rest again. A range raw
    // does not hold either (a segment a restore skipped) stays a gap.
    let mut end = 0;
    for (i, s) in mine.iter().enumerate() {
        if s.first > end + 1 && kind.next(raw, end)?.is_some_and(|seq| seq < s.first) {
            set_aside(&mine[i..])?;
            break;
        }
        end = end.max(s.last);
    }
    Ok(end)
}

/// Segments no longer trusted, renamed (the segment before its checksum: a checksum left alone
/// is written again by the next export). Their files stay on disk.
fn set_aside<'a>(segs: impl IntoIterator<Item = &'a Segment>) -> Result<()> {
    let stamp = crate::db::now_ms();
    for s in segs {
        for path in [
            s.path.clone(),
            PathBuf::from(format!("{}.sha256", s.path.display())),
        ] {
            if path.exists() {
                let aside = PathBuf::from(format!("{}.quarantined-{stamp}", path.display()));
                std::fs::rename(&path, aside)?;
            }
        }
    }
    Ok(())
}

/// At a worker's start, when raw may have lost commits: the backups past its end set aside
/// before new records reuse their seqs.
pub fn check(home: &Path, raw: &Raw) {
    for kind in [Kind::Records, Kind::Ops] {
        if let Err(e) = dir(home).and_then(|d| cursor(raw, &d, kind)) {
            eprintln!("oboete: backup: {e:#}");
        }
    }
}

/// Sealing: the checksum, then the segment, each written to a temporary name, synced, renamed
/// and its directory entry synced before the next. A segment never exists without its checksum,
/// even after a power loss; a checksum left without its segment is written again by the next
/// export.
fn seal(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf> {
    let z = zstd::bulk::compress(data, 3)?;
    let sum = format!("{}  {name}\n", hex(&Sha256::digest(&z)));
    write_synced(dir, &format!("{name}.sha256"), sum.as_bytes())?;
    write_synced(dir, name, &z)
}

fn write_synced(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    crate::db::private(&tmp, 0o600);
    f.write_all(data)?;
    f.sync_all()?;
    let path = dir.join(name);
    std::fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A segment that does not match its checksum, or has none.
#[derive(Debug)]
pub struct Problem {
    pub path: PathBuf,
    pub what: String,
}

pub fn verify(dir: &Path) -> Result<Vec<Problem>> {
    let mut out = Vec::new();
    for s in segments(dir, Kind::Records)?
        .into_iter()
        .chain(segments(dir, Kind::Ops)?)
    {
        if let Some(what) = damage(&s.path) {
            out.push(Problem { path: s.path, what });
        }
    }
    Ok(out)
}

/// Why a segment cannot be trusted, if it cannot.
fn damage(seg: &Path) -> Option<String> {
    let sum_path = PathBuf::from(format!("{}.sha256", seg.display()));
    let Ok(sum) = std::fs::read_to_string(&sum_path) else {
        return Some("no checksum".into());
    };
    let Ok(data) = std::fs::read(seg) else {
        return Some("unreadable".into());
    };
    (sum.split_whitespace().next() != Some(hex(&Sha256::digest(&data)).as_str()))
        .then(|| "checksum mismatch".into())
}

/// `<name>` (and its -wal and -shm, which SQLite binds to the name) moved aside as
/// `<name>.quarantined-<ms>`, its sidecars under the names SQLite looks for beside that (`...-wal`,
/// `...-shm`), so the kept file opens with its last commits. Returns the new name of the main file.
pub(crate) fn quarantine(home: &Path, name: &str) -> Result<PathBuf> {
    let suffix = format!("quarantined-{}", crate::db::now_ms());
    let main = home.join(format!("{name}.{suffix}"));
    // The sidecars before the file: stopped in between, the file is left without them (and is
    // still damaged, so it is quarantined again), never the file's name free with an old WAL
    // beside it that SQLite would replay into the next file of that name.
    for ext in ["-wal", "-shm", ""] {
        let from = home.join(format!("{name}{ext}"));
        if from.exists() {
            std::fs::rename(&from, home.join(format!("{name}.{suffix}{ext}")))
                .with_context(|| format!("quarantine {}", from.display()))?;
        }
    }
    Ok(main)
}

/// The device whose history the backups hold: the damaged file's own id when it can still be
/// read, else the one device the segments name. Never a guess between two.
fn device_of(home: &Path, segs: &[Segment]) -> Result<String> {
    let from_file = rusqlite::Connection::open_with_flags(
        home.join("raw.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .and_then(|c| crate::db::device_id(&c).map_err(|_| rusqlite::Error::InvalidQuery));
    if let Ok(id) = from_file {
        return Ok(id);
    }
    let mut devices: Vec<&str> = segs.iter().map(|s| s.device.as_str()).collect();
    devices.dedup();
    match devices.as_slice() {
        [one] => Ok((*one).to_owned()),
        [] => anyhow::bail!("no backup segments"),
        _ => anyhow::bail!(
            "raw.db's device id is unreadable and the backups hold {} devices",
            devices.len()
        ),
    }
}

/// MUST-M15: quarantine a damaged raw.db and rebuild it, with its device id, from the segments
/// that verify, in seq order. Returns what was done, for stderr and doctor
/// (`<home>/state/restored`).
pub fn restore(home: &Path) -> Result<String> {
    // No store is open while the file is read, rebuilt and swapped; a hook waits (or fails with
    // MUST-M16's marker) instead of writing into the file that is moved aside.
    let _swap = raw::lock_for_swap(home)?;
    let dir = dir(home)?;
    let all = segments(&dir, Kind::Records)?;
    let device = device_of(home, &all)?;
    let (ok, bad): (Vec<Segment>, Vec<Segment>) = all
        .into_iter()
        .filter(|s| s.device == device)
        .partition(|s| damage(&s.path).is_none());
    anyhow::ensure!(
        !ok.is_empty(),
        "no usable backup segment of device {device} in {}",
        dir.display()
    );
    // A build left by a restore that stopped is made again: `.restoring` may be partial, and a
    // `.restored` beside a raw.db still in place came from segments that may have changed since.
    let tmp = home.join("raw.db.restoring");
    for name in [
        "raw.db.restoring",
        "raw.db.restoring-journal",
        "raw.db.restored",
    ] {
        let _ = std::fs::remove_file(home.join(name));
    }
    let mut rebuild = raw::Rebuild::new(&tmp, &device)?;
    let mut records = 0;
    for s in &ok {
        for line in read_segment(&s.path)?.lines().filter(|l| !l.is_empty()) {
            rebuild.add(line)?;
            records += 1;
        }
    }
    // The ops after the records, in op order up to the first segment that is damaged or does not
    // start where the one before it ended. The op log after such a hole is set aside with it: its
    // windows would move the curation checkpoint past the lost ones, whose claims go with
    // knowledge.db, and their records would never be curated again.
    let (mut ops_ok, mut ops_bad, mut next) = (Vec::new(), Vec::new(), 1);
    for s in segments(&dir, Kind::Ops)?
        .into_iter()
        .filter(|s| s.device == device)
    {
        if ops_bad.is_empty() && s.first == next && damage(&s.path).is_none() {
            next = s.last + 1;
            ops_ok.push(s);
        } else {
            ops_bad.push(s);
        }
    }
    let mut ops = 0;
    for s in &ops_ok {
        for line in read_segment(&s.path)?.lines().filter(|l| !l.is_empty()) {
            rebuild.add_op(line)?;
            ops += 1;
        }
    }
    let dropped = rebuild.finish()?;
    let whole = home.join("raw.db.restored");
    std::fs::rename(&tmp, &whole)?;
    // Durable before the damaged file goes: an open that finds neither would make an empty store.
    #[cfg(unix)]
    std::fs::File::open(home)?.sync_all()?;
    // Everything that must not outlive the old raw.db goes before the swap, so a restore that
    // stops at any point is either redone (raw.db still damaged) or complete.
    // Derived data is rebuilt from what was restored: a skipped segment leaves a gap below raw's
    // highest seq that the old index and checkpoints would still cover.
    if home.join("knowledge.db").exists() {
        quarantine(home, "knowledge.db")?;
    }
    // A skipped segment is moved aside, so the export cursor never trusts its name and the seqs
    // it claimed are backed up again as they are reused.
    set_aside(bad.iter().chain(&ops_bad))?;
    let kept = quarantine(home, "raw.db")?;
    std::fs::rename(&whole, home.join("raw.db"))?;
    #[cfg(unix)]
    std::fs::File::open(home)?.sync_all()?;
    let skipped: Vec<String> = bad
        .iter()
        .chain(&ops_bad)
        .filter_map(|s| s.path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let note = format!(
        "raw.db restored at {} from {} segment(s), {records} record(s) and {} op(s); the damaged file is kept as {}{}{}",
        rusqlite::Connection::open_in_memory()?.query_row(
            "SELECT strftime('%Y-%m-%d %H:%M UTC', ?1 / 1000, 'unixepoch')",
            [crate::db::now_ms()],
            |r| r.get::<_, String>(0)
        )?,
        ok.len() + ops_ok.len(),
        ops - dropped,
        kept.file_name().unwrap_or_default().to_string_lossy(),
        if dropped > 0 {
            format!("; {dropped} op(s) past the restored records dropped")
        } else {
            String::new()
        },
        if skipped.is_empty() {
            String::new()
        } else {
            format!(
                "; skipped segment(s), damaged or after a hole in the op log: {}",
                skipped.join(", ")
            )
        }
    );
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    std::fs::write(state.join("restored"), format!("{note}\n"))?;
    Ok(note)
}

/// A segment's lines, decompressed up to the cap.
fn read_segment(path: &Path) -> Result<String> {
    let mut text = String::new();
    zstd::Decoder::new(std::fs::File::open(path)?)?
        .take(MAX_SEGMENT_BYTES)
        .read_to_string(&mut text)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(text)
}

/// A hook that found raw.db damaged while a worker held the lock asks that worker to restore it.
fn restore_request(home: &Path) -> PathBuf {
    home.join("state").join("restore-wanted")
}

pub fn request_restore(home: &Path) {
    let path = restore_request(home);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, "");
}

pub fn restore_requested(home: &Path) -> bool {
    restore_request(home).exists()
}

/// Removes the request and says whether there was one.
pub fn take_restore_request(home: &Path) -> bool {
    std::fs::remove_file(restore_request(home)).is_ok()
}

/// Whether an error says the database file is damaged (not busy, not missing).
pub fn corrupt(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<rusqlite::Error>().is_some_and(|r| {
            matches!(
                r.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
            )
        })
    })
}

/// raw.db for the worker: restored from the segments when opening it says it is damaged or its
/// `quick_check` fails. Any other error stays an error.
pub fn open_raw(home: &Path) -> Result<Raw> {
    match raw::open(home).and_then(|r| r.quick_check().map(|()| r)) {
        Ok(r) => Ok(r),
        Err(e) if damaged(&e) => {
            eprintln!("oboete: raw.db: {e:#}; {}", restore(home)?);
            raw::open(home)
        }
        Err(e) => Err(e),
    }
}

/// knowledge.db for the worker: a damaged one is quarantined and started empty. Every consumer
/// then rebuilds from seq 0 (spec 1.7); raw.db and the segments are not touched.
pub fn open_knowledge(home: &Path) -> Result<rusqlite::Connection> {
    let checked = crate::knowledge::open(home).and_then(|k| {
        crate::db::quick_check_without_vtabs(&home.join("knowledge.db"), "knowledge.db").map(|()| k)
    });
    match checked {
        Ok(k) => Ok(k),
        Err(e) if damaged(&e) => {
            let kept = quarantine(home, "knowledge.db")?;
            eprintln!(
                "oboete: knowledge.db: {e:#}; kept as {} and rebuilt from raw.db",
                kept.display()
            );
            // Its vectors, when they still read, come with the embedding phase's first poll.
            crate::knowledge::open(home)
        }
        Err(e) => Err(e),
    }
}

/// An open or `quick_check` error that says the file is damaged: SQLite's corrupt or not a
/// database, or a check that ran and reported a problem (the one error that is no
/// `rusqlite::Error`). Busy, permission and the like are not.
pub(crate) fn damaged(e: &anyhow::Error) -> bool {
    corrupt(e)
        || (e
            .chain()
            .all(|c| c.downcast_ref::<rusqlite::Error>().is_none())
            && e.to_string().contains("quick_check"))
}

/// The worker's backup step (D11): export what is new. A failure is reported and never stops
/// the worker; doctor shows how far the backups reach.
pub fn run(home: &Path, raw: &Raw) {
    if let Err(e) = dir(home).and_then(|d| export_from(raw, &d)) {
        eprintln!("oboete: backup: {e:#}");
    }
}

/// Spec 7 (files): a folder a sync client uploads.
fn in_cloud_folder(path: &Path) -> Option<String> {
    path.components().find_map(|c| {
        let c = c.as_os_str().to_string_lossy();
        [
            "OneDrive",
            "iCloud Drive",
            "iCloudDrive",
            "Mobile Documents",
            "Dropbox",
            "Google Drive",
            "GoogleDrive",
        ]
        .iter()
        .any(|m| c.starts_with(m))
        .then(|| c.into_owned())
    })
}

/// doctor's backup lines, and whether they are all well (false turns doctor red).
pub fn doctor(home: &Path) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut well = true;
    let dir = match dir(home) {
        Ok(d) => d,
        Err(e) => return (vec![format!("backup: {e:#}")], false),
    };
    for (what, path) in [
        ("the data directory", home),
        ("the backup directory", dir.as_path()),
    ] {
        if let Some(folder) = in_cloud_folder(path) {
            lines.push(format!(
                "warning: {what} is inside {folder}, which a sync client uploads: {}",
                path.display()
            ));
        }
    }
    let segs = segments(&dir, Kind::Records).unwrap_or_default();
    let op_segs = segments(&dir, Kind::Ops).unwrap_or_default();
    if raw::exists(home) {
        match raw::open(home) {
            Ok(raw) => {
                let through = segs
                    .iter()
                    .filter(|s| s.device == raw.device())
                    .map(|s| s.last)
                    .max()
                    .unwrap_or(0);
                let ops_through = op_segs
                    .iter()
                    .filter(|s| s.device == raw.device())
                    .map(|s| s.last)
                    .max()
                    .unwrap_or(0);
                lines.push(format!(
                    "backup: {} segment(s) in {}, through seq {through} of {} and op {ops_through} of {}",
                    segs.len() + op_segs.len(),
                    dir.display(),
                    raw.max_seq().unwrap_or(0),
                    raw.max_op_seq().unwrap_or(0)
                ));
                // The full check reads every page: doctor only, never on an automatic path.
                match raw.integrity_check() {
                    Ok(()) => lines.push("raw.db integrity_check: ok".into()),
                    Err(e) => {
                        well = false;
                        lines.push(format!("{e:#}"));
                    }
                }
            }
            Err(e) => {
                well = false;
                lines.push(format!("raw.db: {e:#}"));
            }
        }
    }
    match verify(&dir) {
        Ok(problems) => {
            for p in problems {
                well = false;
                lines.push(format!("backup segment {}: {}", p.path.display(), p.what));
            }
        }
        Err(e) => {
            well = false;
            lines.push(format!("backup: {e:#}"));
        }
    }
    if let Ok(note) = std::fs::read_to_string(home.join("state").join("restored")) {
        lines.push(note.trim_end().to_owned());
    }
    (lines, well)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{Item, Target};

    fn overwrite(path: &Path, at: u64, bytes: &[u8]) {
        use std::io::{Seek, SeekFrom};
        let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.seek(SeekFrom::Start(at)).unwrap();
        f.write_all(bytes).unwrap();
    }

    fn quarantined(home: &Path, name: &str) -> bool {
        std::fs::read_dir(home).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{name}.quarantined-"))
        })
    }

    #[test]
    fn a_damaged_raw_db_is_quarantined_and_rebuilt_from_its_segments() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        for i in 0..100 {
            raw.append(&raw::test_event(&format!("event {i}"))).unwrap();
        }
        let (before, device) = (raw.hashes().unwrap(), raw.device().to_owned());
        assert!(export(p).unwrap().is_some());
        assert!(export(p).unwrap().is_none()); // nothing new
        drop(raw); // the last close checkpoints the WAL
        overwrite(&p.join("raw.db"), 4096, &[0xA5; 4096]); // page 2
        crate::worker::run_once(p).unwrap(); // quick_check fails: quarantine, restore
        assert!(quarantined(p, "raw.db"));
        let restored = raw::open(p).unwrap();
        assert_eq!(
            (restored.hashes().unwrap(), restored.device().to_owned()),
            (before, device)
        );
        assert!(
            std::fs::read_to_string(p.join("state/restored"))
                .unwrap()
                .contains("100 record(s)")
        );
        let seg = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        overwrite(&seg, 10, &[0xA5; 16]);
        assert_eq!(verify(&p.join("backups")).unwrap().len(), 1); // the checksum names it
    }

    #[test]
    fn tombstones_ledger_and_removed_records_survive_a_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let (masked, found) = crate::redact::scan(
            &format!("alpha {key} zqx-private-words tail"),
            &crate::redact::Rules::default(),
        );
        let finding = found.into_iter().next().unwrap();
        let a = raw
            .append_with_ledger(
                &raw::test_event(&masked),
                &[("/prompt".into(), finding)],
                "test",
            )
            .unwrap();
        let b = raw.append(&raw::test_event("bravo visible")).unwrap();
        let dev = raw.device().to_owned();
        raw.append_tombstone(Target::Record {
            device: dev.clone(),
            seq: b,
        })
        .unwrap();
        let at = masked.find("zqx").unwrap() as i64;
        raw.append_tombstone(Target::Range {
            device: dev.clone(),
            seq: a,
            offset: at,
            length: 17,
        })
        .unwrap();
        let before = raw.hashes().unwrap();
        let ledger = |r: &Raw| r.export_lines(0, usize::MAX).unwrap()[0].1.clone();
        let line = ledger(&raw);
        assert!(line.contains("\"ledger\":[{") && !line.contains(&key));
        export(p).unwrap();
        let seg = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        let mut text = String::new();
        zstd::Decoder::new(std::fs::File::open(&seg).unwrap())
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(!text.contains("zqx-private-words") && !text.contains("bravo"));
        drop(raw);
        std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        // The id cannot be read from the file: it comes from the one device the segments name.
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.device(), dev);
        assert_eq!(restored.hashes().unwrap(), before); // masking twice changes nothing
        assert_eq!(ledger(&restored), line);
        let recs = restored.after(&dev, 0, 10).unwrap();
        assert!(matches!(recs[1].item, Item::Removed));
    }

    #[test]
    fn a_damaged_knowledge_db_is_rebuilt_and_raw_and_segments_stay() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        for i in 0..200 {
            raw.append(&raw::test_event(&format!("note zq{i:03}x kept")))
                .unwrap();
        }
        drop(raw);
        crate::worker::run_once(p).unwrap(); // indexes and backs up at idle exit
        let seg = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(0)
            .path;
        let (raw_bytes, seg_bytes) = (
            std::fs::read(p.join("raw.db")).unwrap(),
            std::fs::read(&seg).unwrap(),
        );
        overwrite(&p.join("knowledge.db"), 4096, &[0xA5; 4096]);
        crate::worker::run_once(p).unwrap();
        assert!(quarantined(p, "knowledge.db"));
        assert_eq!(std::fs::read(p.join("raw.db")).unwrap(), raw_bytes);
        assert_eq!(std::fs::read(&seg).unwrap(), seg_bytes);
        let hits = crate::search::raw(p, "zq007x", None, 5).unwrap();
        assert_eq!(hits.first().map(|h| h.seq), Some(8));
    }

    #[test]
    fn a_knowledge_db_opens_without_checking_its_search_indexes_against_their_text() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        raw.append(&raw::test_event("note zq001x kept")).unwrap();
        drop(raw);
        crate::worker::run_once(p).unwrap();
        let k = crate::knowledge::open(p).unwrap();
        k.execute("DELETE FROM raw_fts_content", []).unwrap(); // the index keeps its terms
        assert!(crate::db::quick_check(&k, "knowledge.db").is_err());
        drop(k);
        open_knowledge(p).unwrap();
        assert!(!quarantined(p, "knowledge.db"));
        let e = crate::setup::doctor(p).unwrap_err().to_string();
        assert!(e.contains("knowledge.db is damaged"), "{e}");
    }

    #[test]
    fn a_knowledge_db_whose_two_tables_share_a_page_is_rebuilt() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        drop(crate::knowledge::open(p).unwrap());
        rusqlite::Connection::open(p.join("knowledge.db"))
            .unwrap()
            .execute_batch(
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_schema SET rootpage =
                   (SELECT rootpage FROM sqlite_schema WHERE name = 'checkpoints')
                 WHERE name = 'rewinds';",
            )
            .unwrap();
        open_knowledge(p).unwrap();
        assert!(quarantined(p, "knowledge.db"));
    }

    #[test]
    fn doctor_finds_a_damaged_table_before_it_reads_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let (root, size): (i64, i64) = crate::knowledge::open(p)
            .unwrap()
            .query_row(
                "SELECT rootpage, (SELECT page_size FROM pragma_page_size)
                 FROM sqlite_schema WHERE name = 'rewinds'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let (root, size) = (root as u64, size as usize);
        overwrite(
            &p.join("knowledge.db"),
            (root - 1) * size as u64,
            &vec![0xA5; size],
        );
        let e = crate::setup::doctor(p).unwrap_err().to_string();
        assert!(e.contains("knowledge.db is damaged"), "{e}");
    }

    /// raw.db of `n` events, one segment exported after each `per` of them.
    fn segmented(p: &Path, n: usize, per: usize) {
        let mut raw = raw::open(p).unwrap();
        for i in 0..n {
            raw.append(&raw::test_event(&format!("rec zq{i:03}x")))
                .unwrap();
            if (i + 1) % per == 0 {
                export(p).unwrap();
            }
        }
    }

    fn damage_raw(p: &Path) {
        std::fs::write(p.join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
    }

    #[test]
    fn a_skipped_segment_leaves_neither_its_records_in_search_nor_its_seqs_unbacked() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
        crate::worker::run_once(p).unwrap(); // index all 30
        let segs = segments(&p.join("backups"), Kind::Records).unwrap();
        assert_eq!(segs.len(), 3);
        std::fs::write(&segs[1].path, b"damaged").unwrap(); // the middle one
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        // Derived data was rebuilt: nothing of 11-20 is found, 21-30 is.
        let hits = crate::search::raw(p, "zq0", None, 100).unwrap();
        assert!(!hits.is_empty() && hits.iter().all(|h| !(11..=20).contains(&h.seq)));
        assert_eq!(crate::search::raw(p, "zq024x", None, 5).unwrap()[0].seq, 25);
        assert!(verify(&p.join("backups")).unwrap().is_empty()); // set aside, not trusted
        // The newest segment damaged: its seqs are reused and backed up again.
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 20, 10);
        let last = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&last, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let mut raw = raw::open(p).unwrap();
        assert_eq!(raw.max_seq().unwrap(), 10);
        raw.append(&raw::test_event("after the restore")).unwrap();
        drop(raw);
        export(p).unwrap();
        let segs = segments(&p.join("backups"), Kind::Records).unwrap();
        assert_eq!(segs.last().map(|s| (s.first, s.last)), Some((11, 11)));
    }

    #[test]
    fn no_store_opens_while_a_restore_swaps_the_file_and_a_stopped_swap_is_finished() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 5, 5);
        let device = raw::open(p).unwrap().device().to_owned();
        let held = raw::lock_for_swap(p).unwrap();
        let t = std::time::Instant::now();
        assert!(raw::open(p).is_err()); // a hook's write waits, then fails (MUST-M16's marker)
        assert!(t.elapsed() >= std::time::Duration::from_secs(1));
        drop(held);
        // Stopped after the damaged file was moved aside: the rebuilt one is renamed in.
        std::fs::rename(p.join("raw.db"), p.join("raw.db.restored")).unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        // A search finishes it too, before it reads.
        crate::search::raw(p, "event", None, 5).unwrap();
        assert!(p.join("raw.db").exists() && !p.join("raw.db.restored").exists());
        let raw = raw::open(p).unwrap();
        assert_eq!(
            (raw.device().to_owned(), raw.max_seq().unwrap()),
            (device, 5)
        );
    }

    #[test]
    fn a_removed_segment_is_written_again_from_raw() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 30, 10); // segments 1-10, 11-20, 21-30
        let ranges = |p: &Path| {
            let mut r: Vec<(i64, i64)> = segments(&p.join("backups"), Kind::Records)
                .unwrap()
                .iter()
                .map(|s| (s.first, s.last))
                .collect();
            r.sort();
            r
        };
        let middle = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .into_iter()
            .find(|s| s.first == 11)
            .unwrap()
            .path;
        std::fs::remove_file(&middle).unwrap();
        export(p).unwrap();
        assert_eq!(ranges(p), [(1, 10), (11, 30)]);
    }

    #[test]
    fn records_raw_lost_are_backed_up_again_as_their_seqs_are_reused() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10); // one segment: 1-10
        rusqlite::Connection::open(p.join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq > 6", []) // lost commits (MUST-M14)
            .unwrap();
        let mut raw = raw::open(p).unwrap();
        for i in 0..2 {
            raw.append(&raw::test_event(&format!("new zn{i}x")))
                .unwrap();
        }
        drop(raw);
        export(p).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap(); // restored from the backups
        let raw = raw::open(p).unwrap();
        let bodies: Vec<String> = raw
            .after(raw.device(), 0, 100)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.item {
                raw::Item::Event(e) => Some(e.body),
                _ => None,
            })
            .collect();
        assert_eq!(bodies.len(), 8, "{bodies:?}");
        assert!(
            bodies[7].contains("zn1x") && !bodies.iter().any(|b| b.contains("zq009x")),
            "{bodies:?}"
        );
    }

    #[test]
    fn a_tombstoned_key_never_reaches_a_segment_through_the_ledger() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let (masked, found) = crate::redact::scan(
            &format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"),
            &crate::redact::Rules::default(),
        );
        let body = serde_json::json!({"trigger": {"zqx-private-words": masked}}).to_string();
        let finding = found.into_iter().next().unwrap();
        let seq = raw
            .append_with_ledger(
                &raw::test_event(&body),
                &[("/trigger/zqx-private-words".into(), finding)],
                "test",
            )
            .unwrap();
        let at = body.find("zqx").unwrap() as i64;
        raw.append_tombstone(raw::Target::Range {
            device: raw.device().to_owned(),
            seq,
            offset: at,
            length: 17,
        })
        .unwrap();
        let lines = raw.export_lines(0, usize::MAX).unwrap();
        assert!(!lines[0].1.contains("zqx"), "{}", lines[0].1);
        assert!(lines[0].1.contains("~tombstoned"));
    }

    #[test]
    fn a_partial_rebuild_is_never_renamed_in() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 3, 3);
        // A build that stopped before its records were committed keeps the `.restoring` name.
        std::fs::rename(p.join("raw.db"), p.join("raw.db.restoring")).unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(p.join(f));
        }
        assert_eq!(raw::open(p).unwrap().max_seq().unwrap(), 0);
        assert!(p.join("raw.db.restoring").exists());
    }

    #[test]
    fn only_damage_restores_not_a_busy_or_missing_file() {
        assert!(damaged(&anyhow::anyhow!(
            "raw.db: quick_check: *** in database main *** Page 2: btreeInitPage() returns error code 11"
        )));
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        );
        assert!(!damaged(
            &anyhow::Error::from(busy).context("raw.db: quick_check")
        ));
        let corrupt = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            None,
        );
        assert!(damaged(&anyhow::Error::from(corrupt)));
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(!damaged(&anyhow::Error::from(denied)));
    }

    #[test]
    fn a_backup_in_a_synced_folder_is_named_in_doctor() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[backup]\ndir = \"OneDrive - Contoso/oboete\"\n",
        )
        .unwrap();
        let (lines, _) = doctor(home.path());
        assert!(
            lines
                .iter()
                .any(|l| l.contains("inside OneDrive - Contoso")),
            "{lines:?}"
        );
        for p in [
            "/Users/a/Library/Mobile Documents/x",
            "/Users/a/Dropbox/x",
            "/g/Google Drive/x",
        ] {
            assert!(in_cloud_folder(Path::new(p)).is_some(), "{p}");
        }
        assert!(in_cloud_folder(Path::new("/home/a/.oboete/backups")).is_none());
    }

    fn window(to_seq: i64) -> (raw::OpKind, serde_json::Value) {
        (
            raw::OpKind::Window,
            serde_json::json!({"from_seq": 1, "to_seq": to_seq, "to_offset": null, "outcome": "curated"}),
        )
    }

    fn claim(text: &str) -> (raw::OpKind, serde_json::Value) {
        (raw::OpKind::Claim, serde_json::json!({"text": text}))
    }

    #[test]
    fn ops_survive_a_backup_and_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10);
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(10), claim("keep all timestamps in UTC")])
            .unwrap();
        let before = raw.ops_after(&dev, 0, 100).unwrap();
        drop(raw);
        export(p).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.ops_after(&dev, 0, 100).unwrap(), before);
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (10, None));
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(note.contains("10 record(s) and 2 op(s)"), "{note}");
    }

    #[test]
    fn an_op_appended_after_its_records_were_backed_up_is_exported() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 5, 5);
        let mut raw = raw::open(p).unwrap();
        raw.append_ops(&[window(5)]).unwrap();
        export(p).unwrap();
        // A correction with no new record: the records' cursor is already at the top.
        raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "fix"}))])
            .unwrap();
        let wrote = export(p).unwrap().unwrap();
        assert!(
            wrote
                .to_string_lossy()
                .ends_with("-000000000002-000000000002.ops.zst")
        );
        assert!(export(p).unwrap().is_none()); // nothing new
    }

    /// D6: a restore that lost an imported batch's records, the newest or one with later
    /// segments restored, drops its checkpoint and every later op, so the next pass imports the
    /// batch again; restored again, the checkpoint stays dropped.
    #[test]
    fn a_restore_that_lost_a_batch_drops_its_checkpoint() {
        // The segment lost, its seqs, the highest seq restored and the checkpoint left.
        for (lost, seqs, max, through) in [(3, (11, 12), 10, 5), (2, (9, 10), 12, 3)] {
            let home = tempfile::tempdir().unwrap();
            let p = home.path();
            segmented(p, 5, 5); // records 1-5
            let batch = |n: usize| -> Vec<crate::capture::Captured> {
                (0..n)
                    .map(|i| crate::capture::Captured {
                        event: raw::Event {
                            source: "oboete-v1".into(),
                            ..raw::test_event(&format!("v1 zq{i:03}x"))
                        },
                        ledger: Vec::new(),
                    })
                    .collect()
            };
            let checkpoint = |through| raw::Checkpoint {
                key: "oboete-v1:d1".into(),
                through,
                row: None,
                prefix: None,
            };
            // Records 6-8, 9-10 and 11-12, a segment each.
            for (n, through) in [(3, 3), (2, 5), (2, 7)] {
                let mut raw = raw::open(p).unwrap();
                raw.append_imported(&batch(n), "v", Some(&checkpoint(through)))
                    .unwrap();
                drop(raw);
                export(p).unwrap();
            }
            let lost = segments(&p.join("backups"), Kind::Records)
                .unwrap()
                .remove(lost);
            assert_eq!((lost.first, lost.last), seqs);
            std::fs::remove_file(&lost.path).unwrap();
            for _ in 0..2 {
                damage_raw(p);
                crate::worker::run_once(p).unwrap();
                let restored = raw::open(p).unwrap();
                assert_eq!(restored.max_seq().unwrap(), max);
                let checkpoints = restored.migration_checkpoints("oboete-v1:").unwrap();
                assert_eq!(checkpoints["oboete-v1:d1"].through, through);
            }
        }
    }

    #[test]
    fn a_window_op_past_the_restored_records_goes_with_every_op_after_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 5); // segments 1-5, 6-10
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        raw.append_ops(&[window(3), claim("a")]).unwrap();
        raw.append_ops(&[window(10), claim("b")]).unwrap();
        raw.append_ops(&[(raw::OpKind::Correction, serde_json::json!({"text": "c"}))])
            .unwrap();
        drop(raw);
        export(p).unwrap();
        // The newest record segment is damaged: seqs 6-10 are not restored.
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.max_seq().unwrap(), 5);
        let kept: Vec<i64> = restored
            .ops_after(&dev, 0, 100)
            .unwrap()
            .iter()
            .map(|o| o.op_seq)
            .collect();
        assert_eq!(kept, [1, 2]);
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(
            note.contains("3 op(s) past the restored records dropped"),
            "{note}"
        );
        // Their op seqs are reused and backed up again, as record seqs are.
        drop(restored);
        let mut raw = raw::open(p).unwrap();
        raw.append_ops(&[window(5)]).unwrap();
        drop(raw);
        export(p).unwrap();
        let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
        assert_eq!(ops.last().map(|s| (s.first, s.last)), Some((3, 3)));
    }

    /// docs/cards.md K4 after a restore that lost records: a removal made at a seq the lost
    /// records had still hides the card of a window cut before the loss, as any removal its op
    /// does not list does.
    #[test]
    fn a_removal_at_a_reused_seq_hides_the_card_of_a_window_cut_before_the_loss() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        for i in 0..10 {
            let event = raw::Event {
                kind: "tool".into(),
                repo: Some("r".into()),
                ..raw::test_event("{}")
            };
            raw.append(&event).unwrap();
            if (i + 1) % 5 == 0 {
                export(p).unwrap(); // segments 1-5, 6-10
            }
        }
        // Cut over records 1 to 3 while the store held ten.
        let op = serde_json::json!({"outcome": "curated", "summary": "Kept.", "from_seq": 1,
            "from_offset": null, "to_seq": 3, "to_offset": null});
        raw.append_ops(&[(raw::OpKind::Window, op)]).unwrap();
        drop(raw);
        export(p).unwrap();
        let newest = segments(&p.join("backups"), Kind::Records)
            .unwrap()
            .remove(1)
            .path;
        std::fs::write(&newest, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let cards = |raw: &Raw| {
            let k = crate::knowledge::open(p).unwrap();
            let rules = crate::redact::Rules::default();
            crate::cards::recent(&k, raw, "r", 10, &rules)
                .unwrap()
                .len()
        };
        let mut raw = raw::open(p).unwrap();
        assert_eq!((raw.max_seq().unwrap(), cards(&raw)), (5, 1));
        let device = raw.device().to_owned();
        let removed = raw.append_tombstone(raw::Target::Record { device, seq: 2 });
        assert_eq!(removed.unwrap(), 6);
        assert_eq!(cards(&raw), 0);
    }

    #[test]
    fn a_hole_in_the_op_log_sets_aside_every_op_after_it() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        segmented(p, 10, 10);
        let mut raw = raw::open(p).unwrap();
        let dev = raw.device().to_owned();
        // One ops segment per export: windows to 3, 6 and 9.
        for to in [3, 6, 9] {
            raw.append_ops(&[window(to), claim("x")]).unwrap();
            export(p).unwrap();
        }
        drop(raw);
        let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
        assert_eq!(ops.len(), 3);
        std::fs::write(&ops[1].path, b"damaged").unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        // The window to 9 verifies, but the one to 6 is lost: curation goes on after 3.
        assert_eq!(restored.max_op_seq().unwrap(), 2);
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
        let left = segments(&p.join("backups"), Kind::Ops).unwrap();
        assert_eq!(left.iter().map(|s| s.first).collect::<Vec<_>>(), [1]);
        let note = std::fs::read_to_string(p.join("state/restored")).unwrap();
        assert!(note.contains("after a hole in the op log"), "{note}");
        // A missing segment is a hole too.
        drop(restored);
        let mut raw = raw::open(p).unwrap();
        for to in [6, 9] {
            raw.append_ops(&[window(to)]).unwrap();
            export(p).unwrap();
        }
        drop(raw);
        let ops = segments(&p.join("backups"), Kind::Ops).unwrap();
        std::fs::remove_file(&ops[1].path).unwrap();
        damage_raw(p);
        crate::worker::run_once(p).unwrap();
        let restored = raw::open(p).unwrap();
        assert_eq!(restored.curation_checkpoint(&dev).unwrap(), (3, None));
    }
}
