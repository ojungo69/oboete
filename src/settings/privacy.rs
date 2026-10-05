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
    rescan: RescanState,
}

#[derive(Serialize)]
struct Repository {
    selector: String,
    label: String,
    excluded: bool,
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
    let mut hash = Sha256::new();
    hash.update(b"oboete:privacy-repo:v1");
    hash.update([0]);
    hash.update(label.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Current rules only touch display text. Selectors and state are validated protocol metadata.
pub fn show(home: &Path) -> Value {
    let state = read(home).unwrap_or_else(|_| PrivacyState {
        available: false,
        repositories: Vec::new(),
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
            rescan: RescanState {
                state: RescanPhase::Empty,
                processed: Some(0),
                total: Some(0),
            },
        });
    };
    let settings = crate::capture::Settings::load(home)?;
    let excluded = crate::raw::exclusions_in(&raw.conn)?;
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
        let labels = labels(Some(&tx), &excluded)?;
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
            labels(None, &excluded)?,
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
    let repositories = labels
        .into_iter()
        .map(|label| Repository {
            selector: selector(&label),
            label: crate::redact::outbound_with(&label, &settings.rules),
            excluded: excluded.contains(&label),
        })
        .collect();
    Ok(PrivacyState {
        available: true,
        repositories,
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

/// Reuse the viewer's existing repository query after its derived tables exist, then include
/// exclusions whose history is absent. A GET never creates those derived tables.
fn labels(k: Option<&Connection>, excluded: &[String]) -> anyhow::Result<BTreeSet<String>> {
    let mut labels: BTreeSet<String> = excluded.iter().cloned().collect();
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
    if posted.selector.len() != 64
        || !posted
            .selector
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(refused(422, "privacy_selector", "selector"));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unavailable = || refused(503, "privacy_unavailable", "");
    let raw = crate::raw::read_only(home)
        .map_err(|_| unavailable())?
        .ok_or_else(|| refused(404, "repo_not_found", "selector"))?;
    let excluded = crate::raw::exclusions_in(&raw.conn).map_err(|_| unavailable())?;
    let k = knowledge(home).map_err(|_| unavailable())?;
    let labels = labels(k.as_ref().map(|(conn, _)| conn), &excluded).map_err(|_| unavailable())?;
    let mut selected = labels
        .iter()
        .filter(|label| selector(label) == posted.selector);
    let label = selected
        .next()
        .ok_or_else(|| refused(404, "repo_not_found", "selector"))?;
    if selected.next().is_some() {
        return Err(refused(409, "repo_changed", "selector"));
    }
    let mut writer = raw.into_writer(home).map_err(|_| unavailable())?;
    let op_seq = writer
        .exclude(label, posted.undo)
        .map_err(|_| unavailable())?;
    Ok(json!({
        "selector": posted.selector,
        "excluded": !posted.undo,
        "recorded": true,
        "op_seq": op_seq,
    }))
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
