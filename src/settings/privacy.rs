//! Typed viewer privacy operations; recording and search remain available after exclusion.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Refusal, refused};

#[derive(Serialize)]
struct PrivacyState {
    available: bool,
    repositories: Vec<Repository>,
    /// Slice 4b: this device's folders never recorded (spec 6.1).
    folders: Vec<Folder>,
    /// Parity row 14: the folder names and claude-mem patterns never recorded.
    patterns: Vec<Folder>,
    rescan: RescanState,
}

#[derive(Serialize)]
struct Repository {
    selector: String,
    label: String,
    excluded: bool,
    /// Slice 4b: never recorded (spec 6.1), and so never sent either.
    never_recorded: bool,
}

#[derive(Serialize)]
struct Folder {
    selector: String,
    label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum RescanPhase {
    Empty,
    Pending,
    Complete,
    Unavailable,
}

#[derive(Serialize)]
struct RescanState {
    state: RescanPhase,
    processed: Option<i64>,
    total: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusion {
    selector: String,
    undo: bool,
}

fn selector(label: &str) -> String {
    hashed(b"oboete:privacy-repo:v1", label)
}

/// Slice 4b: a folder never recorded, as the page names it back.
fn folder_selector(label: &str) -> String {
    hashed(b"oboete:privacy-folder:v1", label)
}

/// Parity row 14: a pattern never recorded, as the page names it back.
fn pattern_selector(label: &str) -> String {
    hashed(b"oboete:privacy-pattern:v1", label)
}

fn hashed(domain: &[u8], label: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update([0]);
    hash.update(label.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn is_selector(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Current rules only touch display text. Selectors and state are validated protocol metadata.
pub fn show(home: &Path) -> Value {
    let state = read(home).unwrap_or_else(|_| PrivacyState {
        available: false,
        repositories: Vec::new(),
        folders: Vec::new(),
        patterns: Vec::new(),
        rescan: RescanState {
            state: RescanPhase::Unavailable,
            processed: None,
            total: None,
        },
    });
    serde_json::to_value(state).expect("typed privacy state serializes")
}

fn read(home: &Path) -> anyhow::Result<PrivacyState> {
    let Some(raw) = crate::raw::read_only(home)? else {
        return Ok(PrivacyState {
            available: true,
            repositories: Vec::new(),
            folders: Vec::new(),
            patterns: Vec::new(),
            rescan: RescanState {
                state: RescanPhase::Empty,
                processed: Some(0),
                total: Some(0),
            },
        });
    };
    let settings = crate::capture::Settings::load(home)?;
    let excluded = crate::raw::exclusions_in(&raw.conn)?;
    let never = crate::raw::capture_exclusions_in(&raw.conn)?;
    let (labels, rescan) = if let Some((mut k, identity)) = knowledge(home)? {
        let path = &raw.sqlite_path;
        let uri = format!(
            "file:{}?mode=ro",
            percent_encoding::percent_encode(
                path.as_os_str().as_encoded_bytes(),
                percent_encoding::NON_ALPHANUMERIC,
            )
        );
        // The fixed alias is always raw; main is always the existing knowledge file.
        // READ_ONLY | URI applies to both opens; neither can create a missing database.
        k.execute("ATTACH DATABASE ?1 AS privacy_raw", [uri])?;
        let tx = k.transaction()?;
        let state = rescan_in(&tx, settings.rules.version(), &crate::db::store_file(path))?;
        let labels = labels(Some(&tx), RawRepos::Attached(&tx), &[])?;
        tx.commit()?;
        anyhow::ensure!(
            std::fs::symlink_metadata(home.join("knowledge.db")).is_ok_and(|m| m.is_file())
                && crate::db::store_file(&home.join("knowledge.db")) == identity,
            "knowledge changed during a read"
        );
        (labels, state)
    } else {
        let (_, total): (String, i64) = raw.conn.query_row(
            "SELECT value, (SELECT COALESCE(MAX(seq), 0) FROM main.records
                            WHERE device = main.meta.value)
             FROM main.meta WHERE key = 'device_id'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        (
            labels(None, RawRepos::Main(&raw.conn), &[])?,
            RescanState {
                state: RescanPhase::Pending,
                processed: None,
                total: Some(total),
            },
        )
    };
    raw.current()?;
    anyhow::ensure!(
        crate::capture::Settings::load(home)?.rules.version() == settings.rules.version(),
        "rules changed during a read"
    );
    // As the hook reads an event's label now: a rule added since an exclusion may read another
    // repository as one listed (Codex on #491).
    let gated = crate::capture::never_recorded(&never, &settings);
    let repositories = rows(labels, &excluded, &never, &settings)
        .into_iter()
        .map(|label| {
            let now = crate::capture::exclusion_label("repo", &label, &settings);
            Repository {
                selector: selector(&label),
                label: crate::redact::outbound_with(&label, &settings.rules),
                excluded: excluded.contains(&label),
                never_recorded: [&label, &now]
                    .into_iter()
                    .any(|l| never.repos.contains(l) || gated.repos.contains(l)),
            }
        })
        .collect();
    let folders = never
        .folders
        .iter()
        .map(|label| Folder {
            selector: folder_selector(label),
            label: crate::redact::outbound_with(label, &settings.rules),
        })
        .collect();
    let patterns = never
        .patterns
        .iter()
        .map(|label| Folder {
            selector: pattern_selector(label),
            label: crate::redact::outbound_with(label, &settings.rules),
        })
        .collect();
    Ok(PrivacyState {
        available: true,
        repositories,
        folders,
        patterns,
        rescan,
    })
}

/// Both database snapshots start in the same read transaction. Version and checkpoint also
/// come from one knowledge snapshot: Rescan writes its version at START, not completion.
fn rescan_in(k: &Connection, version: &str, identity: &str) -> anyhow::Result<RescanState> {
    let (device, bound, total, tables): (String, Option<String>, i64, i64) = k.query_row(
        "SELECT value,
                (SELECT value FROM privacy_raw.meta WHERE key = 'store_file'),
                (SELECT COALESCE(MAX(seq), 0) FROM privacy_raw.records
                 WHERE device = m.value),
                (SELECT COUNT(*) FROM main.sqlite_master WHERE type = 'table'
                 AND name IN ('rescan', 'checkpoints'))
         FROM privacy_raw.meta AS m WHERE key = 'device_id'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    anyhow::ensure!(total >= 0, "invalid raw checkpoint");
    let mut state = RescanState {
        state: RescanPhase::Pending,
        processed: None,
        total: Some(total),
    };
    if tables != 2 || bound.as_deref() != Some(identity) || identity.is_empty() {
        return Ok(state);
    }
    let (scanned, checkpoint): (Option<String>, Option<i64>) = k.query_row(
        "SELECT (SELECT version FROM main.rescan WHERE device = ?1),
                (SELECT seq FROM main.checkpoints
                 WHERE consumer = 'rescan' AND device = ?1)",
        [device],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if scanned.as_deref() != Some(version) {
        state.processed = Some(0);
    } else if let Some(checkpoint) = checkpoint {
        anyhow::ensure!(checkpoint >= 0, "invalid rescan checkpoint");
        // A checkpoint above raw's tip is a rewind to perform, never a completed scan.
        if checkpoint <= total {
            state.processed = Some(checkpoint);
            if checkpoint == total {
                state.state = RescanPhase::Complete;
            }
        }
    }
    Ok(state)
}

fn knowledge(home: &Path) -> anyhow::Result<Option<(Connection, String)>> {
    let path = home.join("knowledge.db");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => anyhow::ensure!(metadata.is_file(), "knowledge is not a regular file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("read privacy knowledge"),
    }
    let identity = crate::db::store_file(&path);
    let sqlite_path = home.canonicalize()?.join("knowledge.db");
    anyhow::ensure!(
        !identity.is_empty() && crate::db::store_file(&sqlite_path) == identity,
        "knowledge changed before a read"
    );
    let conn = Connection::open_with_flags(
        &sqlite_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    anyhow::ensure!(
        std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file())
            && crate::db::store_file(&path) == identity
            && crate::db::store_file(&sqlite_path) == identity,
        "knowledge changed during a read"
    );
    Ok(Some((conn, identity)))
}

enum RawRepos<'a> {
    Main(&'a Connection),
    Attached(&'a Connection),
}

/// The page's repositories: those the records and knowledge.db name (`history`), and each
/// exclusion besides, but a capture entry that reads now as one of them and as no other (stored as
/// given while the records hold the label a rule made), which is that one's row, not a second
/// (Codex and Cursor on #491); that row's undo takes it back (`stands_for`). Entries that read now
/// as one label together, or an entry several listed labels read as, keep rows of their own: an
/// undo from a row they only read alike is refused, and each is taken back from its own (Cursor
/// and Codex on #491).
fn rows(
    mut history: BTreeSet<String>,
    excluded: &[String],
    never: &crate::raw::CaptureExclusions,
    settings: &crate::capture::Settings,
) -> BTreeSet<String> {
    let count = |labels: &mut dyn Iterator<Item = &String>| {
        let mut n = std::collections::BTreeMap::<String, usize>::new();
        for label in labels {
            *n.entry(crate::capture::exclusion_label("repo", label, settings))
                .or_default() += 1;
        }
        n
    };
    let (listed, alike) = (count(&mut history.iter()), count(&mut never.repos.iter()));
    for label in excluded {
        let now = crate::capture::exclusion_label("repo", label, settings);
        let merged = never.repos.contains(label)
            && listed.get(&now) == Some(&1)
            && alike.get(&now) == Some(&1);
        if !merged {
            history.insert(label.clone());
        }
    }
    history
}

/// Whether the listed row `row` stands for the capture entry `entry` (`rows`): the entry has no
/// label of its own among `history`, and `row` is the one listed label that reads as it now (Codex
/// on #491).
fn stands_for(
    history: &BTreeSet<String>,
    settings: &crate::capture::Settings,
    row: &str,
    entry: &str,
) -> bool {
    let now = crate::capture::exclusion_label("repo", entry, settings);
    let mut read = history
        .iter()
        .filter(|label| crate::capture::exclusion_label("repo", label, settings) == now);
    !history.contains(entry)
        && read.next().map(String::as_str) == Some(row)
        && read.next().is_none()
}

/// Include newly captured repository metadata before indexing, the viewer's derived labels,
/// and exclusions without history. A GET never creates derived tables.
fn labels(
    k: Option<&Connection>,
    raw: RawRepos<'_>,
    excluded: &[String],
) -> anyhow::Result<BTreeSet<String>> {
    let mut labels: BTreeSet<String> = excluded.iter().cloned().collect();
    // Every device's and every kind's: a rule's record and another device's records are in no
    // table of knowledge.db. By the records' index where one is built (`raw::event_repos`).
    let (conn, schema) = match raw {
        RawRepos::Main(conn) => (conn, "main"),
        RawRepos::Attached(conn) => (conn, "privacy_raw"),
    };
    labels.extend(crate::raw::event_repos(conn, schema)?);
    if let Some(k) = k {
        let tables: i64 = k.query_row(
            "SELECT COUNT(*) FROM main.sqlite_master WHERE type = 'table'
             AND name IN ('derivations', 'imported', 'raw_docs')",
            [],
            |row| row.get(0),
        )?;
        if tables == 3 {
            // Their names alone: the list's counts are not shown here, and take seconds on a
            // large store (#899 W2).
            labels.extend(crate::view::repo_names_in(k)?);
        }
    }
    Ok(labels)
}

pub fn exclude(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Exclusion =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    if !is_selector(&posted.selector) {
        return Err(refused(422, "privacy_selector", "selector"));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (raw, label, _) = listed_repository(home, &posted.selector)?;
    let mut writer = raw
        .into_writer(home)
        .map_err(|_| refused(503, "privacy_unavailable", ""))?;
    let op_seq = writer
        .exclude(&label, posted.undo)
        .map_err(|_| refused(503, "privacy_unavailable", ""))?;
    Ok(json!({
        "selector": posted.selector,
        "excluded": !posted.undo,
        "recorded": true,
        "op_seq": op_seq,
    }))
}

/// The repository `selector` names in the list a GET shows, read fresh, with the read-only store
/// the write goes on with.
/// The listed repository `wanted` selects, with the labels the records and knowledge.db name
/// (`rows`'s `history`).
fn listed_repository(
    home: &Path,
    wanted: &str,
) -> Result<(crate::raw::ReadOnly, String, BTreeSet<String>), Refusal> {
    let unavailable = || refused(503, "privacy_unavailable", "");
    let raw = crate::raw::read_only(home)
        .map_err(|_| unavailable())?
        .ok_or_else(|| refused(404, "repo_not_found", "selector"))?;
    let excluded = crate::raw::exclusions_in(&raw.conn).map_err(|_| unavailable())?;
    let k = knowledge(home).map_err(|_| unavailable())?;
    let history = labels(
        k.as_ref().map(|(conn, _)| conn),
        RawRepos::Main(&raw.conn),
        &[],
    )
    .map_err(|_| unavailable())?;
    let selected: BTreeSet<String> = history
        .iter()
        .chain(&excluded)
        .filter(|label| selector(label) == wanted)
        .cloned()
        .collect();
    let mut selected = selected.into_iter();
    let label = selected
        .next()
        .ok_or_else(|| refused(404, "repo_not_found", "selector"))?;
    if selected.next().is_some() {
        return Err(refused(409, "repo_changed", "selector"));
    }
    Ok((raw, label, history))
}

/// Slice 4b's request: a listed repository by its `selector`, a folder to stop recording in by its
/// absolute path, or a listed folder by its `folder_selector` to record in again; a folder name or
/// claude-mem pattern (`pattern`) to stop recording by, or a listed one by its `pattern_selector`
/// to record by again (parity row 14).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    folder_selector: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    pattern_selector: Option<String>,
    undo: bool,
}

/// Slice 4b (spec 6.1, `oboete capture exclude`): stop recording a listed repository or a folder
/// of this device, or record it again. A folder is an absolute path, stored through the gate as an
/// event's `cwd` label; what was recorded stays.
pub fn capture(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Capture =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let unavailable = || refused(503, "privacy_unavailable", "");
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (key, label, row, mut writer, answer) = match (
        posted.selector,
        posted.folder,
        posted.folder_selector,
        posted.pattern,
        posted.pattern_selector,
    ) {
        (Some(wanted), None, None, None, None) => {
            if !is_selector(&wanted) {
                return Err(refused(422, "privacy_selector", "selector"));
            }
            let (raw, row, history) = listed_repository(home, &wanted)?;
            // Stored as its records name it, the one name the page has for it (owner,
            // 2026-10-13: what is never recorded is stored as given); an undo takes back the
            // entry stored for it, as the CLI's does (Codex on #491), or the one its row
            // stands for (`rows`), and never another repository's entry that only reads as it
            // now (Codex Security Review and Codex on #491).
            let label = if posted.undo {
                let settings = crate::capture::Settings::load(home).map_err(|_| unavailable())?;
                let stored = crate::raw::capture_exclusions_in(&raw.conn)
                    .map_err(|_| unavailable())?
                    .repos;
                match crate::capture::undone("repo", &row, &stored, &settings) {
                    Ok(entry) => entry,
                    Err(e) => match e
                        .downcast_ref::<crate::capture::ReadAlike>()
                        .map(|alike| alike.entries.as_slice())
                    {
                        Some([entry]) if stands_for(&history, &settings, &row, entry) => {
                            entry.clone()
                        }
                        Some([_]) => return Err(refused(409, "never_other", "selector")),
                        _ => return Err(refused(409, "never_ambiguous", "selector")),
                    },
                }
            } else {
                row.clone()
            };
            let writer = raw.into_writer(home).map_err(|_| unavailable())?;
            ("repo", label, row, writer, json!({ "selector": wanted }))
        }
        (None, Some(folder), None, None, None) if !posted.undo => {
            if folder.len() > 4096 || folder.contains('\0') || !Path::new(&folder).is_absolute() {
                return Err(refused(422, "capture_folder", "folder"));
            }
            // As the CLI stores it: as given, `.` and `..` by name (slice 4a's `folder_path`;
            // owner 2026-10-13: what is never recorded is stored as given), and as the entry
            // already stored for it in another spelling of letter case where that is one
            // folder (Codex on #489).
            let given = crate::capture::folder_path(&folder);
            let writer = crate::raw::open(home).map_err(|_| unavailable())?;
            let stored = writer
                .capture_exclusions()
                .map_err(|_| unavailable())?
                .folders;
            let label = crate::capture::stored_entry("cwd", &given, &stored)
                .cloned()
                .unwrap_or(given);
            let answer = json!({ "folder_selector": folder_selector(&label) });
            ("folder", label.clone(), label, writer, answer)
        }
        (None, None, Some(wanted), None, None) if posted.undo => {
            if !is_selector(&wanted) {
                return Err(refused(422, "privacy_selector", "folder_selector"));
            }
            let raw = crate::raw::read_only(home)
                .map_err(|_| unavailable())?
                .ok_or_else(|| refused(404, "folder_not_found", "folder_selector"))?;
            let folders = crate::raw::capture_exclusions_in(&raw.conn)
                .map_err(|_| unavailable())?
                .folders;
            let label = folders
                .into_iter()
                .find(|f| folder_selector(f) == wanted)
                .ok_or_else(|| refused(404, "folder_not_found", "folder_selector"))?;
            let writer = raw.into_writer(home).map_err(|_| unavailable())?;
            (
                "folder",
                label.clone(),
                label,
                writer,
                json!({ "folder_selector": wanted }),
            )
        }
        // Parity row 14: as the CLI stores one (`capture::pattern`), and as the entry already
        // stored for it in another spelling of letter case where the platform ignores it.
        (None, None, None, Some(pattern), None) if !posted.undo => {
            let given = crate::capture::pattern(&pattern)
                .map_err(|_| refused(422, "capture_pattern", "pattern"))?;
            let writer = crate::raw::open(home).map_err(|_| unavailable())?;
            let stored = writer
                .capture_exclusions()
                .map_err(|_| unavailable())?
                .patterns;
            let label = crate::capture::stored_entry("cwd", &given, &stored)
                .cloned()
                .unwrap_or(given);
            let answer = json!({ "pattern_selector": pattern_selector(&label) });
            ("pattern", label.clone(), label, writer, answer)
        }
        (None, None, None, None, Some(wanted)) if posted.undo => {
            if !is_selector(&wanted) {
                return Err(refused(422, "privacy_selector", "pattern_selector"));
            }
            let raw = crate::raw::read_only(home)
                .map_err(|_| unavailable())?
                .ok_or_else(|| refused(404, "pattern_not_found", "pattern_selector"))?;
            let patterns = crate::raw::capture_exclusions_in(&raw.conn)
                .map_err(|_| unavailable())?
                .patterns;
            let label = patterns
                .into_iter()
                .find(|p| pattern_selector(p) == wanted)
                .ok_or_else(|| refused(404, "pattern_not_found", "pattern_selector"))?;
            let writer = raw.into_writer(home).map_err(|_| unavailable())?;
            (
                "pattern",
                label.clone(),
                label,
                writer,
                json!({ "pattern_selector": wanted }),
            )
        }
        _ => return Err(refused(400, "bad_request", "")),
    };
    // What is taken off the list stays unrecorded while another entry still covers it: a folder,
    // one around it or its repository (Codex on #491, CodeRabbit on #489), and a repository,
    // another entry that reads as it now, as its row reads (CodeRabbit on #491). The answer says
    // so, with no path, which the list shows masked (Codex and Cursor on #491). Read before the
    // write, so nothing fails once it is committed (Codex on #491).
    let still = if posted.undo {
        let mut list = writer.capture_exclusions().map_err(|_| unavailable())?;
        let settings = crate::capture::Settings::load(home).map_err(|_| unavailable())?;
        if key == "pattern" {
            // A pattern names no one place to say this of.
            false
        } else if key == "folder" {
            list.folders.remove(&label);
            crate::capture::not_recorded_why(&label, &list, &settings).is_some()
        } else {
            list.repos.remove(&label);
            let gated = crate::capture::never_recorded(&list, &settings);
            let now = crate::capture::exclusion_label("repo", &row, &settings);
            [&row, &now]
                .into_iter()
                .any(|l| list.repos.contains(l) || gated.repos.contains(l))
        }
    } else {
        false
    };
    let op_seq = writer
        .exclude_capture((key, &label), posted.undo)
        .map_err(|_| unavailable())?;
    let mut answer = answer;
    answer["never_recorded"] = json!(!posted.undo || still);
    answer["recorded"] = json!(true);
    answer["op_seq"] = json!(op_seq);
    Ok(answer)
}

#[cfg(test)]
mod tests;
