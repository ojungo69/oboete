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
/// The most a segment may decompress to: its cap plus one record past it (capture keeps a
/// body far below this).
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
        Ok(text) => toml::from_str::<Settings>(&text)
            .with_context(|| format!("{}: [backup]", path.display()))?
            .backup
            .dir
            .map(|d| home.join(d)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    Ok(dir.unwrap_or_else(|| home.join("backups")))
}

/// One segment file, by the seqs its name gives.
struct Segment {
    device: String,
    first: i64,
    last: i64,
    path: PathBuf,
}

fn name(device: &str, first: i64, last: i64) -> String {
    format!("{device}-{first:012}-{last:012}.seg.zst")
}

/// The segments in `dir`, in (device, first seq) order; files of other names are not ours.
fn segments(dir: &Path) -> Result<Vec<Segment>> {
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
            .and_then(|n| n.strip_suffix(".seg.zst"))
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

/// The records of this home's raw.db above the last backed-up seq, as sealed segments.
/// Returns the last segment written, `None` when there was nothing new. (The worker calls `run`.)
#[cfg(test)]
pub fn export(home: &Path) -> Result<Option<PathBuf>> {
    export_from(&raw::open(home)?, &dir(home)?)
}

fn export_from(raw: &Raw, dir: &Path) -> Result<Option<PathBuf>> {
    let device = raw.device();
    anyhow::ensure!(
        !device.is_empty() && device.chars().all(|c| c.is_ascii_alphanumeric()),
        "device id {device:?} cannot name a segment"
    );
    let mut last = segments(dir)?
        .iter()
        .filter(|s| s.device == device)
        .map(|s| s.last)
        .max()
        .unwrap_or(0);
    if raw.max_seq()? <= last {
        return Ok(None);
    }
    if !dir.exists() {
        // Private when oboete makes it; a directory the user chose keeps its permissions.
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        crate::db::private(dir, 0o700);
    }
    let mut wrote = None;
    loop {
        let lines = raw.export_lines(last, SEGMENT_BYTES)?;
        let (Some(first), Some(end)) = (lines.first(), lines.last()) else {
            return Ok(wrote);
        };
        let (first, end) = (first.0, end.0);
        let mut text = String::new();
        for (_, l) in &lines {
            text.push_str(l);
            text.push('\n');
        }
        wrote = Some(seal(dir, &name(device, first, end), text.as_bytes())?);
        last = end;
    }
}

/// Sealing: the checksum, then the segment, each written to a temporary name, synced and
/// renamed, then the directory synced. A segment never exists without its checksum; a checksum
/// left without its segment is written again by the next export.
fn seal(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf> {
    let z = zstd::bulk::compress(data, 3)?;
    let sum = format!("{}  {name}\n", hex(&Sha256::digest(&z)));
    write_synced(dir, &format!("{name}.sha256"), sum.as_bytes())?;
    let path = write_synced(dir, name, &z)?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(path)
}

fn write_synced(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    crate::db::private(&tmp, 0o600);
    f.write_all(data)?;
    f.sync_all()?;
    let path = dir.join(name);
    std::fs::rename(&tmp, &path)?;
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
    for s in segments(dir)? {
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
/// `<name>.quarantined-<ms>`. Returns the new name of the main file.
fn quarantine(home: &Path, name: &str) -> Result<PathBuf> {
    let suffix = format!("quarantined-{}", crate::db::now_ms());
    let main = home.join(format!("{name}.{suffix}"));
    for ext in ["", "-wal", "-shm"] {
        let from = home.join(format!("{name}{ext}"));
        if from.exists() {
            std::fs::rename(&from, home.join(format!("{name}{ext}.{suffix}")))
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
    let dir = dir(home)?;
    let all = segments(&dir)?;
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
    let tmp = home.join("raw.db.restoring");
    for ext in ["", "-journal"] {
        let _ = std::fs::remove_file(home.join(format!("raw.db.restoring{ext}")));
    }
    let mut rebuild = raw::Rebuild::new(&tmp, &device)?;
    let mut records = 0;
    for s in &ok {
        let mut text = String::new();
        zstd::Decoder::new(std::fs::File::open(&s.path)?)?
            .take(MAX_SEGMENT_BYTES)
            .read_to_string(&mut text)
            .with_context(|| format!("read {}", s.path.display()))?;
        for line in text.lines().filter(|l| !l.is_empty()) {
            rebuild.add(line)?;
            records += 1;
        }
    }
    rebuild.finish()?;
    let kept = quarantine(home, "raw.db")?;
    std::fs::rename(&tmp, home.join("raw.db"))?;
    #[cfg(unix)]
    std::fs::File::open(home)?.sync_all()?;
    let skipped: Vec<String> = bad
        .iter()
        .filter_map(|s| s.path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let note = format!(
        "raw.db restored at {} from {} segment(s), {records} record(s); the damaged file is kept as {}{}",
        rusqlite::Connection::open_in_memory()?.query_row(
            "SELECT strftime('%Y-%m-%d %H:%M UTC', ?1 / 1000, 'unixepoch')",
            [crate::db::now_ms()],
            |r| r.get::<_, String>(0)
        )?,
        ok.len(),
        kept.file_name().unwrap_or_default().to_string_lossy(),
        if skipped.is_empty() {
            String::new()
        } else {
            format!("; skipped damaged segment(s): {}", skipped.join(", "))
        }
    );
    let state = home.join("state");
    std::fs::create_dir_all(&state)?;
    std::fs::write(state.join("restored"), format!("{note}\n"))?;
    Ok(note)
}

/// Whether an error says the database file is damaged (not busy, not missing).
fn corrupt(e: &anyhow::Error) -> bool {
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
    let checked = crate::knowledge::open(home)
        .and_then(|k| crate::db::quick_check(&k, "knowledge.db").map(|()| k));
    match checked {
        Ok(k) => Ok(k),
        Err(e) if damaged(&e) => {
            let kept = quarantine(home, "knowledge.db")?;
            eprintln!(
                "oboete: knowledge.db: {e:#}; kept as {} and rebuilt from raw.db",
                kept.display()
            );
            crate::knowledge::open(home)
        }
        Err(e) => Err(e),
    }
}

/// An open or `quick_check` error that says the file is damaged: SQLite's corrupt or not a
/// database, or a check that ran and reported a problem (the one error that is no
/// `rusqlite::Error`). Busy, permission and the like are not.
fn damaged(e: &anyhow::Error) -> bool {
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
    let segs = segments(&dir).unwrap_or_default();
    if home.join("raw.db").exists() {
        match raw::open(home) {
            Ok(raw) => {
                let through = segs
                    .iter()
                    .filter(|s| s.device == raw.device())
                    .map(|s| s.last)
                    .max()
                    .unwrap_or(0);
                lines.push(format!(
                    "backup: {} segment(s) in {}, through seq {through} of {}",
                    segs.len(),
                    dir.display(),
                    raw.max_seq().unwrap_or(0)
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
        let seg = segments(&p.join("backups")).unwrap().remove(0).path;
        overwrite(&seg, 10, &[0xA5; 16]);
        assert_eq!(verify(&p.join("backups")).unwrap().len(), 1); // the checksum names it
    }

    #[test]
    fn tombstones_ledger_and_removed_records_survive_a_restore() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let mut raw = raw::open(p).unwrap();
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let (masked, found) = crate::redact::scan(&format!("alpha {key} zqx-private-words tail"));
        let finding = found.into_iter().next().unwrap();
        let a = raw
            .append_with_ledger(&raw::test_event(&masked), &[("/prompt".into(), finding)])
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
        let seg = segments(&p.join("backups")).unwrap().remove(0).path;
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
        let seg = segments(&p.join("backups")).unwrap().remove(0).path;
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
}
