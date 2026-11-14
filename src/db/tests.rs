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
