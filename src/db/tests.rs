use super::*;

type OpenStore = fn(&Path) -> Result<Connection>;

#[test]
fn derived_store_schema_opens_retry_past_the_busy_timeout() {
    let cases: [(OpenStore, &str); 5] = [
        (crate::knowledge::open, "DROP TABLE rewinds"),
        (crate::knowledge::open, "DROP TABLE vec_index"),
        (crate::providers_db::open, "DROP TABLE key_limits"),
        (crate::providers_db::open, "DROP INDEX provider_calls_day"),
        (
            crate::providers_db::open,
            "ALTER TABLE provider_state DROP COLUMN tokens_left",
        ),
    ];
    let homes: Vec<_> = cases.iter().map(|_| tempfile::tempdir().unwrap()).collect();
    let writers: Vec<_> = cases
        .iter()
        .zip(&homes)
        .map(|((open, change), home)| {
            let writer = open(home.path()).unwrap();
            writer.execute_batch(change).unwrap();
            writer.execute_batch("BEGIN IMMEDIATE").unwrap();
            writer
        })
        .collect();
    std::thread::scope(|scope| {
        let (sent, received) = std::sync::mpsc::channel();
        let threads: Vec<_> = cases
            .iter()
            .zip(&homes)
            .map(|((open, _), home)| {
                let sent = sent.clone();
                scope.spawn(move || {
                    BUSY_RETRY_NOTICE.with(|notice| notice.set(Some(sent)));
                    let opened = open(home.path());
                    let retried = BUSY_RETRY_NOTICE.with(|notice| notice.take().is_none());
                    (opened, retried)
                })
            })
            .collect();
        drop(sent);
        let retries: std::result::Result<Vec<_>, _> = threads
            .iter()
            .map(|_| received.recv_timeout(Duration::from_secs(5)))
            .collect();
        if retries.is_ok() {
            // The real SQLite busy handler has already exhausted its 2 s wait.
            std::thread::sleep(Duration::from_millis(500));
        }
        for writer in &writers {
            writer.execute_batch("ROLLBACK").unwrap();
        }
        for ((_, change), thread) in cases.iter().zip(threads) {
            let (opened, retried) = thread.join().unwrap();
            let opened = opened.unwrap_or_else(|e| panic!("{change}: {e:#}"));
            assert!(retried, "{change}: no busy retry was observed");
            let timeout: u32 = opened
                .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                .unwrap();
            assert_eq!(timeout, 2_000);
            let synchronous: u32 = opened
                .query_row("PRAGMA synchronous", [], |r| r.get(0))
                .unwrap();
            assert_eq!(synchronous, 1);
        }
        retries.expect("every missing schema must retry before its writer is released");
    });
}

#[test]
fn initialized_derived_stores_open_without_a_write_lock_or_commit() {
    for open in [
        crate::knowledge::open as OpenStore,
        crate::providers_db::open,
    ] {
        let home = tempfile::tempdir().unwrap();
        let writer = open(home.path()).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        std::thread::scope(|scope| {
            let (sent, received) = std::sync::mpsc::channel();
            let path = home.path();
            scope.spawn(move || {
                crate::crash::off();
                let opened = open(path);
                sent.send((opened, crate::crash::count())).unwrap();
            });
            let opened = received.recv_timeout(Duration::from_secs(2));
            writer.execute_batch("ROLLBACK").unwrap();
            let (opened, commits) = opened.expect("a warm open must not wait for the writer");
            opened.unwrap();
            assert_eq!(commits, 0);
        });
    }
}

#[test]
fn open_writes_share_a_deadline_and_keep_the_busy_timeout_after_an_error() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("locked.db");
    let writer = Connection::open(&path).unwrap();
    wal(&writer, "FULL").unwrap();
    writer
        .execute_batch("CREATE TABLE sample(id INTEGER); BEGIN IMMEDIATE;")
        .unwrap();
    let mut opener = Connection::open(&path).unwrap();
    wal(&opener, "FULL").unwrap();
    let deadline = Instant::now() + Duration::from_millis(100);
    let schema = "CREATE TABLE IF NOT EXISTS another(id INTEGER);";
    let error = ensure_schema_until(&opener, schema, deadline).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<rusqlite::Error>()
            .unwrap()
            .sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy)
    );
    assert_eq!(
        opener
            .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2_000
    );
    // The second write gets the same, now expired deadline, rather than a fresh wait.
    for column in ["first", "second"] {
        let error =
            ensure_column_until(&mut opener, "sample", column, "TEXT", deadline).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<rusqlite::Error>()
                .unwrap()
                .sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
        assert!(Instant::now() >= deadline);
        assert!(deadline.elapsed() < Duration::from_secs(1));
        let timeout: i64 = opener
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 2_000);
    }
    writer.execute_batch("ROLLBACK").unwrap();
    ensure_schema_until(&opener, schema, Instant::now() + OPEN_WRITE_WAIT).unwrap();
    assert_eq!(
        opener
            .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2_000
    );
    ensure_column(&mut opener, "sample", "first", "TEXT").unwrap();
    assert_eq!(
        opener
            .execute("INSERT INTO sample(first) VALUES ('unlocked')", [])
            .unwrap(),
        1
    );
    let started = Instant::now();
    let error = ensure_column_until(
        &mut opener,
        "missing",
        "column",
        "TEXT",
        started + OPEN_WRITE_WAIT,
    )
    .unwrap_err();
    assert!(error.to_string().contains("no such table"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn documents_get_uids_unique_across_devices_and_imports_agree() {
    let dir = std::env::temp_dir().join(format!("oboete-db-uid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let conn = open(&dir).unwrap();
    let device = device_id(&conn).unwrap();
    upsert_session(&conn, "s", "claude", "/r", "/r", 1).unwrap();
    insert_prompt(&conn, "s", 1, "a prompt").unwrap();
    let doc = Doc {
        session_id: "s",
        repo: "/r",
        ts: 2,
        kind: "decision",
        title: "t",
        body: "imported",
    };
    assert!(import_doc(&conn, "claude-mem:abc", "o7", &doc).unwrap());
    let uid = |table: &str, id: i64| -> String {
        conn.query_row(&format!("SELECT uid FROM {table} WHERE id=?1"), [id], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(uid("prompts", 1), format!("{device}:p1"));
    assert_eq!(uid("observations", 1), "claude-mem:abc:o7");
    // Rows from before uids existed are filled once: imports from their source, the rest
    // from this device.
    insert_prompt(&conn, "s", 3, "another").unwrap();
    conn.execute_batch(
            "UPDATE prompts SET uid = NULL; UPDATE observations SET uid = NULL; PRAGMA user_version = 1;
             INSERT INTO imports(source, source_id, doc) VALUES ('x', '1', ''), ('x', '2', 'o'), ('x', '3', 'q9');",
        )
        .unwrap();
    drop(conn);
    let conn = open(&dir).unwrap();
    let uids: Vec<String> = conn
        .prepare("SELECT uid FROM prompts UNION ALL SELECT uid FROM observations")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        uids,
        [
            format!("{device}:p1"),
            format!("{device}:p2"),
            "claude-mem:abc:o7".into()
        ]
    );
    drop(conn);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn device_id_stays_with_the_file_and_changes_on_a_copy() {
    let dir = std::env::temp_dir().join(format!("oboete-db-device-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let conn = open(&dir).unwrap();
    let first = device_id(&conn).unwrap();
    assert!(
        first.len() == 8 && first.bytes().all(|b| b.is_ascii_hexdigit()),
        "{first}"
    );
    drop(conn);
    assert_eq!(device_id(&open(&dir).unwrap()).unwrap(), first);
    // The same store copied elsewhere (another machine's `~/.oboete`) is another device.
    let copy = dir.join("copy");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::copy(dir.join("oboete.db"), copy.join("oboete.db")).unwrap();
    let other = device_id(&open(&copy).unwrap()).unwrap();
    assert_ne!(other, first);
    assert_eq!(device_id(&open(&copy).unwrap()).unwrap(), other);
    std::fs::remove_dir_all(&dir).ok();
}

/// A Windows store from before #122 holds its creation time as its identity.
#[cfg(windows)]
#[test]
fn a_windows_store_with_the_old_identity_keeps_its_device_id() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path()).unwrap();
    let first = device_id(&conn).unwrap();
    let path = dir.path().join("oboete.db");
    let legacy = legacy_store_file(&path).unwrap();
    conn.execute("UPDATE meta SET value=?1 WHERE key='store_file'", [&legacy])
        .unwrap();
    drop(conn);
    let conn = open(dir.path()).unwrap();
    assert_eq!(device_id(&conn).unwrap(), first);
    let now: String = conn
        .query_row("SELECT value FROM meta WHERE key='store_file'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(now, store_file(&path));
}

#[test]
fn path_keys_move_to_the_origin_key_once() {
    let dir = std::env::temp_dir().join(format!("oboete-db-keys-{}", std::process::id()));
    let repo_dir = dir.join("r");
    std::fs::create_dir_all(repo_dir.join(".git")).unwrap();
    std::fs::write(
        repo_dir.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://github.com/o/r.git\n",
    )
    .unwrap();
    let path_key = repo_dir
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let conn = open(&dir).unwrap();
    // Rows an older build filed under paths.
    for (session, repo) in [
        ("a", path_key.as_str()),
        ("b", "/gone/repo"),
        ("c", "claude-mem:x"),
    ] {
        upsert_session(&conn, session, "claude", repo, repo, 1).unwrap();
        insert_prompt(&conn, session, 1, "a prompt about keys").unwrap();
    }
    conn.execute_batch("PRAGMA user_version = 0").unwrap();
    drop(conn);
    let mut conn = open(&dir).unwrap();
    let repos = |conn: &Connection, table: &str| -> Vec<String> {
        conn.prepare(&format!("SELECT repo FROM {table} ORDER BY repo"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    // A path still here gets its origin key; one that is gone, or no path, stays.
    let want = ["/gone/repo", "claude-mem:x", "github.com/o/r"];
    for table in ["sessions", "prompts", "fts"] {
        assert_eq!(repos(&conn, table), want, "{table}");
    }
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert!(version >= 1);

    // A repository used before it had an origin moves when a later run sees one.
    let late = dir.join("late");
    std::fs::create_dir_all(late.join(".git")).unwrap();
    let late_key = crate::repo::key(&late);
    upsert_session(&conn, "d", "claude", &late_key, &late_key, 1).unwrap();
    touch_repo(&conn, "d", &late_key).unwrap();
    insert_prompt(&conn, "d", 1, "before the remote").unwrap();
    assert_eq!(rekey_paths(&mut conn).unwrap(), 0);
    std::fs::write(
        late.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://github.com/o/late\n",
    )
    .unwrap();
    assert_eq!(rekey_paths(&mut conn).unwrap(), 1);
    for table in ["sessions", "prompts", "fts", "session_repos"] {
        assert!(
            repos(&conn, table).contains(&"github.com/o/late".to_string()),
            "{table}"
        );
        assert!(!repos(&conn, table).contains(&late_key), "{table}");
    }
    drop(conn);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn the_store_is_private_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("oboete-db-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    // An existing store created world-readable is tightened too.
    std::fs::write(dir.join("oboete.db"), b"").unwrap();
    std::fs::set_permissions(
        dir.join("oboete.db"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let conn = open(&dir).unwrap();
    upsert_session(&conn, "s1", "claude", "/r", "/r", 1).unwrap();
    drop(conn);
    let conn = open(&dir).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    for f in ["oboete.db", "oboete.db-wal", "oboete.db-shm"] {
        let path = dir.join(f);
        if path.exists() {
            assert_eq!(mode(&path), 0o600, "{f}");
        }
    }
    drop(conn);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sessions_table_from_m0_gains_injection_and_prompt_cursors() {
    let dir = std::env::temp_dir().join(format!("oboete-db-m0-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Connection::open(dir.join("oboete.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT NOT NULL, repo TEXT NOT NULL,
                 cwd TEXT, started_at INTEGER NOT NULL, ended_at INTEGER, last_event_at INTEGER NOT NULL);",
            )
            .unwrap();
    // Several agents' hooks can open the old database at the same moment.
    let openers: Vec<_> = (0..4)
        .map(|_| {
            let d = dir.clone();
            std::thread::spawn(move || open(&d).map(drop))
        })
        .collect();
    for h in openers {
        h.join().unwrap().unwrap();
    }
    let conn = open(&dir).unwrap();
    upsert_session(&conn, "s1", "claude", "/r", "/r", 1).unwrap();
    // Reopening a current database is a no-op.
    drop(conn);
    let conn = open(&dir).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions WHERE id='s1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn doc_ids_are_never_reused_and_old_tables_are_rebuilt() {
    let dir = std::env::temp_dir().join(format!("oboete-db-autoinc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A store from a build before delete existed: plain INTEGER PRIMARY KEY, rows in place.
    Connection::open(dir.join("oboete.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE observations(id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
                   repo TEXT NOT NULL, ts INTEGER NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL,
                   body TEXT NOT NULL, provider TEXT NOT NULL);
                 CREATE TABLE summaries(id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
                   repo TEXT NOT NULL, ts INTEGER NOT NULL, body TEXT NOT NULL, provider TEXT NOT NULL);
                 INSERT INTO observations VALUES(7, 's1', '/r', 1, 'change', 'seven', 'b', 'p');
                 INSERT INTO summaries VALUES(3, 's1', '/r', 1, 'three', 'p');",
            )
            .unwrap();
    let openers: Vec<_> = (0..4)
        .map(|_| {
            let d = dir.clone();
            std::thread::spawn(move || open(&d).map(drop))
        })
        .collect();
    for h in openers {
        h.join().unwrap().unwrap();
    }
    let mut conn = open(&dir).unwrap();
    let title: String = conn
        .query_row("SELECT title FROM observations WHERE id=7", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(title, "seven");
    assert!(autoincrements(&conn, "observations").unwrap());
    assert!(autoincrements(&conn, "summaries").unwrap());
    let indexes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name IN
                 ('observations_repo', 'observations_session', 'summaries_repo', 'summaries_session')",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(indexes, 4);
    // Delete the newest of each, store again: the ids move on.
    conn.execute_batch(
        "DELETE FROM observations WHERE id = 7; DELETE FROM summaries WHERE id = 3;
             DELETE FROM fts WHERE doc IN ('o7', 's3');",
    )
    .unwrap();
    let s = PendingSession {
        id: "s1".into(),
        repo: "/r".into(),
        last_event_at: 2,
    };
    upsert_session(&conn, "s1", "claude", "/r", "/r", 2).unwrap();
    let obs = Observation {
        kind: "change".into(),
        title: "eight".into(),
        body: "b".into(),
    };
    apply_batch(&mut conn, &s, "p", "four", std::slice::from_ref(&obs), 0).unwrap();
    let ids = |c: &Connection, sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(ids(&conn, "SELECT MAX(id) FROM observations"), 8);
    assert_eq!(ids(&conn, "SELECT MAX(id) FROM summaries"), 4);
    assert_eq!(
        ids(&conn, "SELECT COUNT(*) FROM fts WHERE doc IN ('o8', 's4')"),
        2
    );
    // A session deleted while its summary was being written leaves nothing behind, also
    // when the agent's next event has recreated the session in the meantime.
    conn.execute_batch(
        "DELETE FROM fts WHERE doc IN (SELECT 'o' || id FROM observations WHERE session_id = 's1'
               UNION ALL SELECT 's' || id FROM summaries WHERE session_id = 's1');
             DELETE FROM observations WHERE session_id = 's1';
             DELETE FROM summaries WHERE session_id = 's1';
             DELETE FROM events WHERE session_id = 's1';
             DELETE FROM sessions WHERE id = 's1';",
    )
    .unwrap();
    assert!(!apply_batch(&mut conn, &s, "p", "late", std::slice::from_ref(&obs), 0).unwrap());
    upsert_session(&conn, "s1", "claude", "/r", "/r", 3).unwrap();
    assert!(!apply_batch(&mut conn, &s, "p", "later", std::slice::from_ref(&obs), 0).unwrap());
    for table in ["observations", "summaries", "fts"] {
        assert_eq!(
            ids(&conn, &format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    // The recreated session is a live one of its own: a batch of its time is stored.
    let s3 = PendingSession {
        last_event_at: 3,
        ..s
    };
    assert!(apply_batch(&mut conn, &s3, "p", "own", std::slice::from_ref(&obs), 0).unwrap());
    assert_eq!(ids(&conn, "SELECT COUNT(*) FROM summaries"), 1);
    std::fs::remove_dir_all(&dir).ok();
}
