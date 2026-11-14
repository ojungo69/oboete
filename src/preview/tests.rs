use super::*;

#[test]
fn w6d_preview_releases_original_descriptors_before_its_callback() {
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("source.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE fixture(value INTEGER); INSERT INTO fixture VALUES(1);")
        .unwrap();
    drop(conn);
    let identity = crate::worker::file_id(std::fs::metadata(&path));
    assert!(identity.is_some());
    with_preview_v1(&path, |copy, _| {
        let held = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| crate::worker::file_id(std::fs::metadata(entry.path())) == identity)
            .count();
        assert_eq!(
            held, 0,
            "preview still held a source descriptor during its callback"
        );
        assert_eq!(
            copy.query_row("SELECT value FROM fixture", [], |row| row.get::<_, i64>(0))?,
            1
        );
        Ok(())
    })
    .unwrap();
}
