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
    let (conn, query) = match raw {
        RawRepos::Main(conn) => (
            conn,
            "SELECT DISTINCT repo FROM main.records
             WHERE type = 'event' AND repo IS NOT NULL AND repo <> ''",
        ),
        RawRepos::Attached(conn) => (
            conn,
            "SELECT DISTINCT repo FROM privacy_raw.records
             WHERE type = 'event' AND repo IS NOT NULL AND repo <> ''",
        ),
    };
    let mut records = conn.prepare(query)?;
    for label in records.query_map([], |row| row.get::<_, String>(0))? {
        labels.insert(label?);
    }
    if let Some(k) = k {
        let tables: i64 = k.query_row(
            "SELECT COUNT(*) FROM main.sqlite_master WHERE type = 'table'
             AND name IN ('derivations', 'imported', 'raw_docs')",
            [],
            |row| row.get(0),
        )?;
        if tables == 3 {
            for row in crate::view::repos_in(k)? {
                labels.insert(
                    row["repo"]
                        .as_str()
                        .context("invalid repository row")?
                        .to_owned(),
                );
            }
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
mod tests {
    use super::*;
    use crate::search::b::fixture::Store;

    #[test]
    fn a_fresh_privacy_get_is_empty_and_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("unconfigured");

        let shown = show(&home);

        assert_eq!(shown["available"], true);
        assert_eq!(shown["repositories"], json!([]));
        assert_eq!(
            shown["rescan"],
            json!({"state": "empty", "processed": 0, "total": 0})
        );
        assert!(!home.exists(), "a GET initialized the absent home");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_new_raw_repository_can_be_excluded_before_any_worker_index_exists() {
        let mut store = Store::new();
        store.event(
            "prompt",
            "new-session",
            ("new-repo", "main"),
            1_000,
            json!({"prompt": "New work.", "repo": "body-only-repo"}),
        );
        let before = (
            store.raw.max_seq().unwrap(),
            store.raw.max_op_seq().unwrap(),
        );
        let shown = show(store.home.path());
        assert_eq!(shown["available"], true);
        assert_eq!(
            shown["repositories"],
            json!([{"selector": selector("new-repo"), "label": "new-repo", "excluded": false,
                     "never_recorded": false}])
        );
        assert_eq!(
            (
                store.raw.max_seq().unwrap(),
                store.raw.max_op_seq().unwrap()
            ),
            before
        );
        for file in ["knowledge.db", "providers.db", "state"] {
            assert!(!store.home.path().join(file).exists(), "GET created {file}");
        }
        let saved = exclude(
            store.home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({
                "selector": shown["repositories"][0]["selector"], "undo": false
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved["recorded"], true);
        assert_eq!(saved["excluded"], true);
        assert_eq!(store.raw.exclusions().unwrap(), ["new-repo"]);
        for file in ["knowledge.db", "providers.db", "state"] {
            assert!(
                !store.home.path().join(file).exists(),
                "POST created {file}"
            );
        }
    }

    #[test]
    fn raw_repository_labels_join_existing_derived_and_excluded_labels_before_indexing() {
        let mut store = Store::new();
        store.said("mixed", "shared", 1_000, "Indexed work.");
        store.imported("derived", "derived-only", 1_000, "A note", "Imported work.");
        store.raw.exclude("excluded-only", false).unwrap();
        store.run();
        store.said("mixed", "shared", 2_000, "Duplicate label.");
        store.said("mixed", "new-repo", 3_000, "Unindexed work.");
        store.said("remote", "remote-repo", 4_000, "Another device's work.");
        store.said("empty", "", 5_000, "No repository label.");
        store
            .raw
            .append(&crate::raw::test_event("Unlabelled work."))
            .unwrap();
        let raw = Connection::open(crate::raw::path(store.home.path())).unwrap();
        raw.execute(
            "UPDATE records SET device = 'remote-device' WHERE repo = 'remote-repo'",
            [],
        )
        .unwrap();
        let shown = show(store.home.path());
        assert_eq!(shown["available"], true);
        assert_eq!(shown["rescan"]["state"], "pending");
        let rows = shown["repositories"].as_array().unwrap();
        let labels: Vec<_> = rows
            .iter()
            .map(|row| row["label"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                "claude-mem:derived-only",
                "excluded-only",
                "new-repo",
                "remote-repo",
                "shared"
            ]
        );
        let selected = rows.iter().find(|row| row["label"] == "new-repo").unwrap();
        let saved = exclude(
            store.home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({"selector": selected["selector"], "undo": false})).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["recorded"], true);
        assert_eq!(
            store.raw.exclusions().unwrap(),
            ["excluded-only", "new-repo"]
        );
        let k = knowledge(store.home.path()).unwrap().unwrap().0;
        assert_eq!(
            k.query_row("SELECT count(*) FROM raw_docs", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "privacy lookup or exclusion indexed the new raw records"
        );
        assert!(!store.home.path().join("providers.db").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_home_reads_the_same_privacy_state_without_rebinding_the_store() {
        let mut store = Store::new();
        store.said("s", "repo", 1_000, "A privacy read through a home alias.");
        store.run();
        let links = tempfile::tempdir().unwrap();
        let alias = links.path().join("home");
        std::os::unix::fs::symlink(store.home.path(), &alias).unwrap();
        let conn = Connection::open_with_flags(
            store.home.path().join("raw.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let before: Vec<(String, String)> = conn
            .prepare("SELECT key, value FROM meta ORDER BY key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let expected = show(store.home.path());
        assert_eq!(expected["rescan"]["state"], "complete");
        assert_eq!(
            show(&alias),
            expected,
            "an ancestor alias is not a symlinked store file"
        );
        let after: Vec<(String, String)> = conn
            .prepare("SELECT key, value FROM meta ORDER BY key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(after, before, "a read changed store identity or metadata");
    }

    #[cfg(unix)]
    #[test]
    fn privacy_reads_reject_leaf_symlinks_even_when_they_name_the_same_store() {
        let mut store = Store::new();
        store.said("s", "repo", 1_000, "A private leaf-symlink control.");
        store.run();
        let home = store.home.path();
        for name in ["raw.db", "knowledge.db", "raw.lock"] {
            let path = home.join(name);
            let moved = home.join(format!("{name}.control"));
            std::fs::rename(&path, &moved).unwrap();
            std::os::unix::fs::symlink(&moved, &path).unwrap();
            assert_eq!(show(home)["available"], false, "{name} symlink accepted");
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(&moved, &path).unwrap();
            assert_eq!(show(home)["rescan"]["state"], "complete");
        }
        let opened = crate::raw::read_only(home).unwrap().unwrap();
        let path = home.join("raw.db");
        let moved = home.join("raw.db.control");
        std::fs::rename(&path, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &path).unwrap();
        assert!(
            opened.current().is_err(),
            "a late leaf symlink was accepted"
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&moved, &path).unwrap();
        opened.current().unwrap();
    }

    #[test]
    fn an_exclusion_can_be_undone_after_open_recovers_the_same_restored_file() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.exclude("repo-without-history", false).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        drop(raw);
        std::fs::rename(
            home.path().join("raw.db"),
            home.path().join("raw.db.restored"),
        )
        .unwrap();
        let shown = show(home.path());
        assert_eq!(shown["available"], true);
        let selected = shown["repositories"][0]["selector"].as_str().unwrap();
        let result = exclude(
            home.path(),
            &Mutex::new(()),
            &serde_json::to_vec(&json!({"selector": selected, "undo": true})).unwrap(),
        );
        assert!(
            result.is_ok(),
            "same-file recovery refused exclusion: {result:?}"
        );
        assert_eq!(result.unwrap()["excluded"], false);
        assert!(home.path().join("raw.db").is_file());
        assert!(!home.path().join("raw.db.restored").exists());
        assert!(
            crate::raw::open(home.path())
                .unwrap()
                .exclusions()
                .unwrap()
                .is_empty()
        );
    }

    /// Parity row 14: the page stops recording by a folder name or claude-mem pattern, lists it,
    /// refuses an empty one or a request that names two things, and takes it back by its selector.
    #[test]
    fn the_page_stops_recording_by_a_pattern_and_takes_it_back() {
        let mut store = Store::new();
        store.said("s", "github.com/o/open", 1_000, "Pattern page journey.");
        store.run();
        let home = store.home.path().to_owned();
        let saving = Mutex::new(());
        let post =
            |body: serde_json::Value| capture(&home, &saving, &serde_json::to_vec(&body).unwrap());
        let saved = post(json!({"pattern": " secret-* ", "undo": false})).unwrap();
        assert_eq!(saved["never_recorded"], true);
        let id = saved["pattern_selector"].as_str().unwrap().to_owned();
        assert_eq!(
            show(&home)["patterns"],
            json!([{"selector": id, "label": "secret-*"}])
        );
        for (body, code) in [
            (json!({"pattern": "  ", "undo": false}), "capture_pattern"),
            (
                json!({"pattern_selector": "0".repeat(64), "undo": true}),
                "pattern_not_found",
            ),
            (json!({"pattern": "x", "undo": true}), "bad_request"),
            (
                json!({"pattern": "x", "folder": "/x", "undo": false}),
                "bad_request",
            ),
        ] {
            let before = store.raw.max_op_seq().unwrap();
            assert_eq!(post(body.clone()).unwrap_err().code, code, "{body}");
            assert_eq!(store.raw.max_op_seq().unwrap(), before, "{body}");
        }
        assert_eq!(
            post(json!({"pattern_selector": id, "undo": true})).unwrap()["never_recorded"],
            false
        );
        assert_eq!(show(&home)["patterns"], json!([]));
    }

    /// Slice 4b: the page stops recording a listed repository by its selector and a folder by its
    /// absolute path, lists both, and records them again; a repository never recorded shows as
    /// never sent, a relative path or an unknown selector is refused before any write, and the
    /// folder's selector names its label as stored.
    #[test]
    fn the_page_stops_recording_a_repository_or_a_folder_and_takes_it_back() {
        const REPO: &str = "github.com/o/private";
        let mut store = Store::new();
        store.said("s", REPO, 1_000, "Capture page journey.");
        store.run();
        let home = store.home.path().to_owned();
        let saving = Mutex::new(());
        let post =
            |body: serde_json::Value| capture(&home, &saving, &serde_json::to_vec(&body).unwrap());
        let wanted = selector(REPO);
        let saved = post(json!({"selector": wanted, "undo": false})).unwrap();
        assert_eq!(
            (saved["never_recorded"].clone(), saved["recorded"].clone()),
            (json!(true), json!(true))
        );
        assert!(store.raw.capture_exclusions().unwrap().repos.contains(REPO));
        let folder = home.join("private-folder").to_string_lossy().into_owned();
        let written = home.join("x").join("..").join("private-folder");
        let saved = post(json!({"folder": written.to_string_lossy(), "undo": false})).unwrap();
        let folder_id = saved["folder_selector"].as_str().unwrap().to_owned();
        let shown = show(&home);
        let row = shown["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["selector"] == wanted.as_str())
            .unwrap()
            .clone();
        assert_eq!(
            (row["never_recorded"].clone(), row["excluded"].clone()),
            (json!(true), json!(true))
        );
        assert_eq!(
            shown["folders"],
            json!([{"selector": folder_id, "label": folder}])
        );
        for (body, code) in [
            (
                json!({"folder": "relative/path", "undo": false}),
                "capture_folder",
            ),
            (
                json!({"selector": "0".repeat(64), "undo": false}),
                "repo_not_found",
            ),
            (
                json!({"folder_selector": "0".repeat(64), "undo": true}),
                "folder_not_found",
            ),
            (
                json!({"selector": "nope", "undo": false}),
                "privacy_selector",
            ),
            (json!({"folder": folder, "undo": true}), "bad_request"),
            (
                json!({"selector": wanted, "folder": folder, "undo": false}),
                "bad_request",
            ),
        ] {
            let before = store.raw.max_op_seq().unwrap();
            assert_eq!(post(body.clone()).unwrap_err().code, code, "{body}");
            assert_eq!(store.raw.max_op_seq().unwrap(), before, "{body}");
        }
        assert_eq!(
            post(json!({"folder_selector": folder_id, "undo": true})).unwrap()["never_recorded"],
            false
        );
        assert_eq!(
            post(json!({"selector": wanted, "undo": true})).unwrap()["never_recorded"],
            false
        );
        assert_eq!(
            store.raw.capture_exclusions().unwrap(),
            crate::raw::CaptureExclusions::default()
        );
        assert_eq!(show(&home)["folders"], json!([]));
    }

    /// Codex on #491: a rule added after an exclusion that reads another listed repository as the
    /// excluded one shows that one as never recorded too, as a hook reads its events now. Its undo
    /// does not take back the exclusion stored for the other repository, which would record that
    /// one too (Codex Security Review on #491): the other's own row does; refused too when several
    /// read as it.
    #[test]
    fn a_repository_a_rule_reads_as_an_excluded_one_is_never_recorded_until_its_undo() {
        const A: &str = "github.com/o/secret-a1";
        const B: &str = "github.com/o/secret-b2";
        const C: &str = "github.com/o/secret-c3";
        let mut store = Store::new();
        for (ts, session, repo) in [(1_000, "sa", A), (2_000, "sb", B), (3_000, "sc", C)] {
            store.said(session, repo, ts, "Labels read alike journey.");
        }
        store.run();
        let home = store.home.path().to_owned();
        let saving = Mutex::new(());
        let post =
            |body: serde_json::Value| capture(&home, &saving, &serde_json::to_vec(&body).unwrap());
        let never = |repo: &str| {
            show(&home)["repositories"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["selector"] == selector(repo).as_str())
                .unwrap()["never_recorded"]
                .clone()
        };
        let rule = |on: bool| {
            let rules =
                "[redaction]\nextra_rules = [{ id = 'secret', regex = 'secret-[a-z][0-9]' }]\n";
            let text = format!("[summary]\ncurate = false\n{}", if on { rules } else { "" });
            std::fs::write(home.join("config.toml"), text).unwrap();
        };
        post(json!({"selector": selector(A), "undo": false})).unwrap();
        assert_eq!(never(B), json!(false));
        rule(true);
        // A hook reads B's events as A's exclusion now.
        let settings = crate::capture::Settings::load(&home).unwrap();
        let list = store.raw.capture_exclusions().unwrap();
        let b_now = crate::capture::exclusion_label("repo", B, &settings);
        assert!(crate::capture::never_recorded(&list, &settings).covers(Some(&b_now), None));
        assert_eq!(never(B), json!(true));
        // Nor is its session sent, as its row says (Cursor on #491: #489's `exclusions_now`).
        let reading = crate::curate::Reading::now(&store.raw, crate::curate::Reads::Live).unwrap();
        assert!(reading.excluded.contains("claude\0sb"), "{reading:?}");
        let before = store.raw.max_op_seq().unwrap();
        assert_eq!(
            post(json!({"selector": selector(B), "undo": true}))
                .unwrap_err()
                .code,
            "never_other"
        );
        assert_eq!(store.raw.max_op_seq().unwrap(), before);
        assert_eq!((never(A), never(B)), (json!(true), json!(true)));
        post(json!({"selector": selector(A), "undo": true})).unwrap();
        assert_eq!(
            store.raw.capture_exclusions().unwrap(),
            crate::raw::CaptureExclusions::default()
        );
        assert_eq!((never(A), never(B)), (json!(false), json!(false)));
        // A and C excluded before the rule, each under its own label, both read as B now.
        rule(false);
        post(json!({"selector": selector(A), "undo": false})).unwrap();
        post(json!({"selector": selector(C), "undo": false})).unwrap();
        rule(true);
        let before = store.raw.max_op_seq().unwrap();
        assert_eq!(
            post(json!({"selector": selector(B), "undo": true}))
                .unwrap_err()
                .code,
            "never_ambiguous"
        );
        assert_eq!(store.raw.max_op_seq().unwrap(), before);
        // A taken back from its own row stays unrecorded while C reads as it, and the answer says
        // so, as its row does (CodeRabbit on #491); C taken back too, both are recorded.
        let answer = post(json!({"selector": selector(A), "undo": true})).unwrap();
        assert_eq!(
            (answer["never_recorded"].clone(), never(A)),
            (json!(true), json!(true))
        );
        let answer = post(json!({"selector": selector(C), "undo": true})).unwrap();
        assert_eq!(
            (answer["never_recorded"].clone(), never(A)),
            (json!(false), json!(false))
        );
    }

    /// Codex on #491: an entry stored as given whose records carry only the label a rule made of
    /// it is listed under that label alone, and that row's undo takes it back; once another listed
    /// repository reads as it too, it keeps a row of its own, and only that row takes it back.
    #[test]
    fn a_stored_entry_listed_under_its_masked_label_is_taken_back_from_that_row() {
        const A: &str = "github.com/o/secret-a1";
        const B: &str = "github.com/o/secret-b2";
        let mut store = Store::new();
        let home = store.home.path().to_owned();
        std::fs::write(
            home.join("config.toml"),
            "[summary]\ncurate = false\n[redaction]\nextra_rules = [{ id = 'secret', regex = 'secret-[a-z][0-9]' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(&home).unwrap();
        let masked = crate::capture::exclusion_label("repo", A, &settings);
        assert_ne!(masked, A);
        // A's records carry the label the rule made of it; its exclusion is stored as given.
        store.said("sa", &masked, 1_000, "Masked label journey.");
        store.run();
        store.raw.exclude_capture(("repo", A), false).unwrap();
        let saving = Mutex::new(());
        let post =
            |body: serde_json::Value| capture(&home, &saving, &serde_json::to_vec(&body).unwrap());
        let labels = || {
            show(&home)["repositories"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["selector"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(), [selector(&masked)]);
        post(json!({"selector": selector(&masked), "undo": true})).unwrap();
        assert!(store.raw.capture_exclusions().unwrap().repos.is_empty());
        // B's records read as A now too: A's entry keeps a row of its own, which alone takes it
        // back; the masked label's row is refused, nothing written.
        store.raw.exclude_capture(("repo", A), false).unwrap();
        store.said("sb", B, 2_000, "Another repository journey.");
        store.run();
        assert!(labels().contains(&selector(A)));
        let before = store.raw.max_op_seq().unwrap();
        assert_eq!(
            post(json!({"selector": selector(&masked), "undo": true}))
                .unwrap_err()
                .code,
            "never_other"
        );
        assert_eq!(store.raw.max_op_seq().unwrap(), before);
        post(json!({"selector": selector(A), "undo": true})).unwrap();
        assert!(store.raw.capture_exclusions().unwrap().repos.is_empty());
    }

    /// Codex on #491, owner 2026-10-13: the page stores a repository as its records name it, the
    /// one name it has for it, and an event a hook labelled under a rule that masks the name, while
    /// the exclusion committed, is refused by its append, which reads the list as the rules now
    /// read it.
    #[test]
    fn the_page_stores_a_repository_as_its_records_name_it() {
        const OLD: &str = "github.com/o/acme-482915";
        const NEW: &str = "github.com/o/acme-731904";
        let mut store = Store::new();
        store.said("s", OLD, 1_000, "Before the rule.");
        store.run();
        let home = store.home.path().to_owned();
        std::fs::write(
            home.join("config.toml"),
            "[summary]\ncurate = false\n[redaction]\nextra_rules = [{ id = 'acme', \
             regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let settings = crate::capture::Settings::load(&home).unwrap();
        let now = crate::capture::exclusion_label("repo", OLD, &settings);
        assert_ne!(now, OLD);
        let saving = Mutex::new(());
        let body = serde_json::to_vec(&json!({"selector": selector(OLD), "undo": false})).unwrap();
        capture(&home, &saving, &body).unwrap();
        let stored = store.raw.capture_exclusions().unwrap().repos;
        assert_eq!(Vec::from_iter(stored), [OLD]);
        // One row, the repository's, not a second for the label it is stored under (Codex and
        // Cursor on #491).
        let shown = show(&home);
        let rows: Vec<_> = shown["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["selector"].clone(), r["never_recorded"].clone()))
            .collect();
        assert_eq!(rows, [(json!(selector(OLD)), json!(true))]);
        // And an entry the CLI stored as given, which the records name by the label the rule
        // made: the records' row, never recorded, and, as the rule reads it as the entry before
        // too, a row of its own to take it back from (Cursor on #491).
        let masked = crate::raw::Event {
            repo: Some(crate::capture::exclusion_label("repo", NEW, &settings)),
            session: "t".into(),
            ..crate::raw::test_event("Under the rule.")
        };
        store.raw.append(&masked).unwrap();
        store.raw.exclude_capture(("repo", NEW), false).unwrap();
        let shown = show(&home);
        let rows: Vec<_> = shown["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["selector"] != json!(selector(OLD)))
            .map(|r| (r["selector"].clone(), r["never_recorded"].clone()))
            .collect();
        let label = crate::capture::exclusion_label("repo", NEW, &settings);
        assert_eq!(
            rows,
            [
                (json!(selector(&label)), json!(true)),
                (json!(selector(NEW)), json!(true))
            ]
        );
        // A folder is stored as typed, though the rule masks part of it.
        let folder = home.join("acme-482915").to_string_lossy().into_owned();
        let body = serde_json::to_vec(&json!({"folder": folder, "undo": false})).unwrap();
        capture(&home, &saving, &body).unwrap();
        let folders = store.raw.capture_exclusions().unwrap().folders;
        assert_eq!(Vec::from_iter(folders), [folder]);
        let event = crate::raw::Event {
            repo: Some(now),
            ..crate::raw::test_event("After the rule.")
        };
        let elsewhere = ("github.com/o/open", None);
        let appended = store
            .raw
            .append_captured(&event, &[], &settings, elsewhere)
            .unwrap();
        assert_eq!(appended, None);
    }

    /// Codex on #491: a folder taken off the list while a folder around it is listed is answered
    /// as still never recorded, with no path (the list shows the folders masked); the outer one's
    /// undo records again.
    #[test]
    fn the_undo_of_a_folder_inside_a_listed_folder_is_answered_as_still_not_recorded() {
        let store = Store::new();
        let home = store.home.path().to_owned();
        let saving = Mutex::new(());
        let post =
            |body: serde_json::Value| capture(&home, &saving, &serde_json::to_vec(&body).unwrap());
        let around = home.join("private").to_string_lossy().into_owned();
        let inside = home.join("private").join("project");
        post(json!({"folder": around, "undo": false})).unwrap();
        let added = post(json!({"folder": inside.to_string_lossy(), "undo": false})).unwrap();
        let undone =
            post(json!({"folder_selector": added["folder_selector"], "undo": true})).unwrap();
        assert_eq!(undone["never_recorded"], true);
        assert!(!undone.to_string().contains("private"), "{undone}");
        let outer = json!({"folder_selector": folder_selector(&around), "undo": true});
        let undone = post(outer).unwrap();
        assert_eq!(undone["never_recorded"], false);
    }

    /// Codex on #491: a folder undo whose settings do not load is refused before its write, rather
    /// than committed and then answered as failed.
    #[test]
    fn a_folder_undo_under_settings_that_do_not_load_writes_nothing() {
        let store = Store::new();
        let home = store.home.path().to_owned();
        let saving = Mutex::new(());
        let post =
            |body: serde_json::Value| capture(&home, &saving, &serde_json::to_vec(&body).unwrap());
        let folder = home.join("private").to_string_lossy().into_owned();
        let added = post(json!({"folder": folder, "undo": false})).unwrap();
        std::fs::write(home.join("config.toml"), "[redaction\n").unwrap();
        let before = store.raw.max_op_seq().unwrap();
        let undo = json!({"folder_selector": added["folder_selector"], "undo": true});
        assert_eq!(post(undo).unwrap_err().code, "privacy_unavailable");
        assert_eq!(store.raw.max_op_seq().unwrap(), before);
        assert!(
            store
                .raw
                .capture_exclusions()
                .unwrap()
                .folders
                .contains(&folder)
        );
    }

    /// Cursor on #491: capture entries a rule now reads as one label keep rows of their own, so
    /// each is taken back from its row (that label's own undo is refused as ambiguous). One taken
    /// back stays unrecorded while the other reads as it, and the answer says so (CodeRabbit on
    /// #491).
    #[test]
    fn entries_a_rule_reads_alike_keep_their_own_rows() {
        const ONE: &str = "github.com/o/acme-111111";
        const TWO: &str = "github.com/o/acme-222222";
        let mut store = Store::new();
        store.said("s", ONE, 1_000, "Alike rows journey.");
        store.run();
        for repo in [ONE, TWO] {
            store.raw.exclude_capture(("repo", repo), false).unwrap();
        }
        std::fs::write(
            store.home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = 'acme', regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let shown = show(store.home.path());
        let rows: Vec<_> = shown["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["selector"].clone())
            .collect();
        for repo in [ONE, TWO] {
            assert!(rows.contains(&json!(selector(repo))), "{repo}: {rows:?}");
        }
        let saving = Mutex::new(());
        let body = json!({"selector": selector(TWO), "undo": true});
        let undone = capture(
            store.home.path(),
            &saving,
            &serde_json::to_vec(&body).unwrap(),
        );
        assert_eq!(undone.unwrap()["never_recorded"], true);
        assert!(!store.raw.capture_exclusions().unwrap().repos.contains(TWO));
    }

    /// Codex on #489: a folder posted again in another spelling of letter case, where that is one
    /// folder, keeps the one entry stored for it, so the undo of its row leaves none.
    #[test]
    fn the_page_keeps_one_entry_for_a_folder_in_either_spelling_where_case_is_ignored() {
        let store = Store::new();
        let home = store.home.path().to_owned();
        let saving = Mutex::new(());
        let post =
            |body: serde_json::Value| capture(&home, &saving, &serde_json::to_vec(&body).unwrap());
        let folder = home.join("Private").to_string_lossy().into_owned();
        let other = home.join("private").to_string_lossy().into_owned();
        post(json!({"folder": folder, "undo": false})).unwrap();
        let saved = post(json!({"folder": other, "undo": false})).unwrap();
        let folders = store.raw.capture_exclusions().unwrap().folders;
        if !cfg!(any(windows, target_os = "macos")) {
            assert_eq!(folders.len(), 2);
            return;
        }
        assert_eq!(Vec::from_iter(folders), [folder.as_str()]);
        assert_eq!(saved["folder_selector"], json!(folder_selector(&folder)));
        post(json!({"folder_selector": saved["folder_selector"], "undo": true})).unwrap();
        assert!(store.raw.capture_exclusions().unwrap().folders.is_empty());
    }

    #[test]
    fn stored_repo_selectors_survive_rules_and_exclusions_without_history_can_be_undone() {
        const PRIVATE: &str = "github.com/o/private";
        const PRIVATE_ID: &str = "3bade42df09fc0272eb1a2e5c5e7d81828634a9a471a06930d83d6901d74dc03";
        const NO_HISTORY: &str = "github.com/o/no-history";
        const NO_HISTORY_ID: &str =
            "ef20c28ddc01394f6ab3414dfcc16bcbbda52a2ab295ffb433c5982fcd1f8a90";
        let mut store = Store::new();
        store.said("s", PRIVATE, 1_000, "Privacy selector journey.");
        store.raw.exclude(NO_HISTORY, false).unwrap();
        store.run();
        std::fs::write(
            store.home.path().join("config.toml"),
            "[summary]\ncurate = false\n[redaction]\nextra_rules = [{ id = 'labels', \
             regex = '^github[.]com/o/private$|^[0-9a-f]{64}$' }]\n",
        )
        .unwrap();
        let shown = show(store.home.path());
        assert_eq!(shown["available"], true);
        let rows = shown["repositories"].as_array().unwrap();
        let private = rows
            .iter()
            .find(|row| row["selector"] == PRIVATE_ID)
            .unwrap();
        assert_eq!(private["label"], "[REDACTED]");
        assert_eq!(private["excluded"], false);
        assert!(
            rows.iter()
                .any(|row| row["selector"] == NO_HISTORY_ID && row["excluded"] == true)
        );
        assert!(!serde_json::to_string(&shown).unwrap().contains(PRIVATE));

        let saving = Mutex::new(());
        let home = store.home.path().to_owned();
        let post = |selector: &str, undo: bool| {
            exclude(
                &home,
                &saving,
                &serde_json::to_vec(&json!({"selector": selector, "undo": undo})).unwrap(),
            )
            .unwrap()
        };
        let saved = post(PRIVATE_ID, false);
        assert_eq!(saved["selector"], PRIVATE_ID);
        assert_eq!(saved["excluded"], true);
        assert_eq!(saved["recorded"], true);
        assert_eq!(store.raw.exclusions().unwrap(), [NO_HISTORY, PRIVATE]);
        assert!(
            !crate::search::raw(&home, "Privacy selector journey", None, 5)
                .unwrap()
                .is_empty(),
            "send exclusions hid recorded search results"
        );
        store.said("s", PRIVATE, 2_000, "Recorded while excluded.");
        store.run();
        assert!(
            !crate::search::raw(&home, "Recorded while excluded", None, 5)
                .unwrap()
                .is_empty(),
            "send exclusions stopped new recording or full-text indexing"
        );
        assert_eq!(post(NO_HISTORY_ID, true)["excluded"], false);
        assert_eq!(store.raw.exclusions().unwrap(), [PRIVATE]);
        assert_eq!(post(PRIVATE_ID, true)["excluded"], false);
        assert!(store.raw.exclusions().unwrap().is_empty());
    }

    #[test]
    fn rescan_status_tracks_real_batches_rules_and_a_rewind_without_unmasking() {
        use crate::worker::Consumer;

        let mut store = Store::new();
        for i in 1..=501 {
            store.said("s", "repo", i, "An old acme-123456 value.");
        }
        store.run();
        assert_eq!(
            show(store.home.path())["rescan"],
            json!({"state": "complete", "processed": 501, "total": 501})
        );
        let config = store.home.path().join("config.toml");
        let rule = "[summary]\ncurate = false\n[redaction]\n\
            extra_rules = [{ id = 'acme', regex = 'acme-[0-9]{6}' }]\n";
        std::fs::write(&config, rule).unwrap();
        assert_eq!(
            show(store.home.path())["rescan"],
            json!({"state": "pending", "processed": 0, "total": 501})
        );

        let mut k = crate::knowledge::open(store.home.path()).unwrap();
        let tx = k
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let at = crate::knowledge::checkpoint::get(&tx, "rescan", store.raw.device()).unwrap();
        let next = crate::consumer::rescan::Rescan::new(store.home.path())
            .step(&store.raw, &tx, store.raw.device(), at)
            .unwrap();
        assert_eq!(next, 500);
        crate::knowledge::checkpoint::set_in(
            &tx,
            crate::knowledge::checkpoint::SEQS,
            "rescan",
            store.raw.device(),
            next,
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            show(store.home.path())["rescan"],
            json!({"state": "pending", "processed": 500, "total": 1001})
        );
        store.run();
        assert_eq!(
            show(store.home.path())["rescan"],
            json!({"state": "complete", "processed": 1002, "total": 1002})
        );

        let allow =
            "allowlist = ['afb9e7cc01de1058f27960e30cb305c557dd8b2ef3b446fd147f1356efa6fa6b']\n";
        std::fs::write(&config, format!("{rule}{allow}")).unwrap();
        assert_eq!(
            show(store.home.path())["rescan"],
            json!({"state": "pending", "processed": 0, "total": 1002})
        );
        store.run();
        assert_eq!(show(store.home.path())["rescan"]["state"], "complete");
        let records = store.raw.after(store.raw.device(), 0, 1).unwrap();
        let crate::raw::Item::Event(event) = &records[0].item else {
            panic!("the original record was not retained");
        };
        assert!(
            !event.body.contains("acme-123456"),
            "an allow exception unmasked a tombstone"
        );

        store.said("s", "repo", 2_000, "Safe trailing rewind fixture.");
        store.run();
        let top = store.raw.max_seq().unwrap();
        assert_eq!(top, 1003);
        Connection::open(store.home.path().join("raw.db"))
            .unwrap()
            .execute(
                "DELETE FROM records WHERE device = ?1 AND seq = ?2",
                rusqlite::params![store.raw.device(), top],
            )
            .unwrap();
        assert_eq!(show(store.home.path())["rescan"]["state"], "pending");
        store.run();
        assert_eq!(
            show(store.home.path())["rescan"],
            json!({"state": "complete", "processed": 1002, "total": 1002})
        );
    }

    #[test]
    fn a_privacy_get_never_treats_a_store_mid_swap_as_empty() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append(&crate::raw::test_event(
            r#"{"prompt":"Restore admission fixture."}"#,
        ))
        .unwrap();
        drop(raw);
        let held = crate::raw::lock_for_swap(home.path()).unwrap();
        let saved = home.path().join("raw.db.swap-fixture");
        std::fs::rename(home.path().join("raw.db"), &saved).unwrap();

        let shown = show(home.path());

        assert_eq!(shown["available"], false);
        assert_eq!(shown["rescan"]["state"], "unavailable");
        assert!(!home.path().join("raw.db").exists());
        assert!(!home.path().join("raw.db.restored").exists());
        assert!(!home.path().join("knowledge.db").exists());
        assert!(!home.path().join("state").exists());
        std::fs::rename(saved, home.path().join("raw.db")).unwrap();
        drop(held);
        assert_eq!(show(home.path())["rescan"]["state"], "pending");
    }

    #[test]
    fn missing_scan_rows_stay_pending_and_corrupt_stores_are_unavailable_without_initialization() {
        // Windows disallows '?' in filenames; space, '#' and Unicode still exercise URI escaping.
        let prefix = if cfg!(windows) {
            "privacy # café-"
        } else {
            "privacy #? café-"
        };
        let home = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append(&crate::raw::test_event(
            r#"{"prompt":"One stored record."}"#,
        ))
        .unwrap();
        let before = std::fs::read(home.path().join("raw.db")).unwrap();
        assert_eq!(
            show(home.path())["rescan"],
            json!({"state": "pending", "processed": null, "total": 1})
        );
        assert!(!home.path().join("knowledge.db").exists());
        assert!(!home.path().join("state").exists());
        assert_eq!(std::fs::read(home.path().join("raw.db")).unwrap(), before);

        let k = Connection::open(home.path().join("knowledge.db")).unwrap();
        k.execute_batch(
            "CREATE TABLE checkpoints(consumer TEXT, device TEXT, seq INTEGER);
             CREATE TABLE fixture_canary(value TEXT);",
        )
        .unwrap();
        assert_eq!(show(home.path())["rescan"]["state"], "pending");
        let tables: i64 = k
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "the GET initialized derived tables");
        k.execute_batch("CREATE TABLE rescan(device TEXT PRIMARY KEY, version TEXT NOT NULL);")
            .unwrap();
        let version = crate::capture::Settings::load(home.path())
            .unwrap()
            .rules
            .version()
            .to_owned();
        k.execute(
            "INSERT INTO rescan VALUES(?1, ?2)",
            [raw.device(), &version],
        )
        .unwrap();
        assert_eq!(
            show(home.path())["rescan"],
            json!({"state": "pending", "processed": null, "total": 1})
        );
        drop(k);
        std::fs::write(home.path().join("knowledge.db"), "corrupt-fixture-canary").unwrap();
        let shown = show(home.path());
        assert_eq!(shown["available"], false);
        assert_eq!(shown["rescan"]["state"], "unavailable");
        assert!(!serde_json::to_string(&shown).unwrap().contains("canary"));
        assert_eq!(
            std::fs::read(home.path().join("knowledge.db")).unwrap(),
            b"corrupt-fixture-canary"
        );
        assert!(!home.path().join("state").exists());
    }

    #[test]
    fn a_copied_store_is_not_reported_complete_or_given_an_identity_by_a_get() {
        let mut store = Store::new();
        store.said("s", "repo", 1_000, "Copy identity fixture.");
        store.run();
        assert_eq!(show(store.home.path())["rescan"]["state"], "complete");
        let conn = Connection::open(store.home.path().join("raw.db")).unwrap();
        conn.execute(
            "UPDATE meta SET value = 'copied-file-fixture' WHERE key = 'store_file'",
            [],
        )
        .unwrap();
        let before: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'device_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(show(store.home.path())["rescan"]["state"], "pending");

        let after: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'device_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, before);
        let binding: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'store_file'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(binding, "copied-file-fixture");
    }

    #[test]
    fn invalid_or_unknown_exclusion_selectors_create_no_store_or_echo_input() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("absent");
        let saving = Mutex::new(());
        for (body, status, code) in [
            (
                json!({"selector": "input-canary", "undo": false}),
                422,
                "privacy_selector",
            ),
            (
                json!({"selector": "0".repeat(64), "undo": false}),
                404,
                "repo_not_found",
            ),
            (
                json!({"selector": "0".repeat(64), "undo": "false"}),
                400,
                "bad_request",
            ),
            (
                json!({"selector": "0".repeat(64), "undo": false, "repo": "input-canary"}),
                400,
                "bad_request",
            ),
        ] {
            let refused = exclude(&home, &saving, &serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!((refused.status, refused.code), (status, code));
            assert!(!refused.field.contains("canary"));
            assert!(!home.exists());
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn nonregular_or_missing_raw_coordination_is_unavailable_and_is_not_created() {
        let home = tempfile::tempdir().unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        drop(raw);
        std::fs::remove_file(home.path().join("raw.lock")).unwrap();
        assert_eq!(show(home.path())["rescan"]["state"], "unavailable");
        assert!(!home.path().join("raw.lock").exists());
        std::fs::create_dir(home.path().join("raw.lock")).unwrap();
        assert_eq!(show(home.path())["rescan"]["state"], "unavailable");
        assert!(home.path().join("raw.lock").is_dir());
        assert!(!home.path().join("knowledge.db").exists());
    }
}
