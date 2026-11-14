    use super::*;

    /// Settings' list over a store the worker has not given the `hidden` column yet, or one with no
    /// such table: nothing, not an error (#672).
    #[test]
    fn a_store_without_the_hidden_column_lists_nothing() {
        let k = Connection::open_in_memory().unwrap();
        k.execute_batch("CREATE TABLE cards (ts INTEGER); CREATE TABLE turns (ts INTEGER);")
            .unwrap();
        let read = |r: &rusqlite::Row<'_>| Ok(Some(r.get::<_, i64>(0)?));
        for table in ["cards", "turns"] {
            let sql = format!("SELECT ts FROM {table} WHERE hidden = 1");
            let page = removed_page(&k, table, &sql, [], 10, read).unwrap();
            assert_eq!(page, (Vec::new(), false), "{table}");
        }
        let none = Connection::open_in_memory().unwrap();
        let sql = "SELECT ts FROM cards WHERE hidden = 1";
        let page = removed_page(&none, "cards", sql, [], 10, read).unwrap();
        assert_eq!(page, (Vec::new(), false), "no table");
    }
