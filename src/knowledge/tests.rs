    use super::*;

    #[test]
    fn schema_upgrade_is_atomic_with_vec_shadows_and_keeps_existing_data() {
        let home = tempfile::tempdir().unwrap();
        let writer = open(home.path()).unwrap();
        checkpoint::set_in(&writer, checkpoint::SEQS, "kept", "device", 17).unwrap();
        let vector = vec![0u8; crate::embed::DIM * 4];
        writer
            .execute("INSERT INTO vectors VALUES ('kept', 'sha', ?1)", [&vector])
            .unwrap();
        let missing = "DROP TABLE rewinds; DROP TABLE vec_index;";
        writer.execute_batch(missing).unwrap();
        crate::crash::off();
        let reopened = open(home.path()).unwrap();
        assert_eq!(crate::crash::count(), 1);
        reopened.execute_batch(missing).unwrap();
        crate::crash::at(1);
        let failed = open(home.path());
        let commits = crate::crash::count();
        crate::crash::off();
        assert!(failed.is_err());
        assert_eq!(commits, 1);
        let remaining: i64 = writer
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'rewinds'
             OR name = 'vec_index' OR name GLOB 'vec_index_*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        let recovered = open(home.path()).unwrap();
        assert_eq!(checkpoint::get(&recovered, "kept", "device").unwrap(), 17);
        assert_eq!(
            recovered
                .query_row("SELECT vec FROM vectors WHERE embedder='kept'", [], |r| r
                    .get::<_, Vec<
                    u8,
                >>(
                    0
                ))
                .unwrap(),
            vector
        );
        let bits = vec![0u8; crate::embed::DIM / 8];
        recovered
            .execute(
                "INSERT INTO vec_index(rowid, embedder, kind, repo, ts, session, embedding)
             VALUES(7, 'kept', 'c', '', 0, '', vec_bit(?1))",
                [&bits],
            )
            .unwrap();
        let nearest: i64 = recovered
            .query_row(
                "SELECT rowid FROM vec_index WHERE embedding MATCH vec_bit(?1) AND k=1
             AND embedder='kept' AND kind='c'",
                [&bits],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nearest, 7);
    }

    /// Milestone 4 D8: a process whose first store is knowledge.db reads `vec_index`, so `open`
    /// registers sqlite-vec itself. Run again in a child process of its own, since another test's
    /// `db::open` registers the module for every connection of this one.
    #[test]
    fn a_knowledge_db_opened_alone_reads_its_vector_index() {
        const HOME: &str = "OBOETE_TEST_KNOWLEDGE_ALONE";
        let zero = vec![0u8; crate::embed::DIM / 8];
        let nearest = |k: &rusqlite::Connection| -> i64 {
            k.query_row(
                "SELECT rowid FROM vec_index WHERE embedding MATCH vec_bit(?1) AND k = 1
                   AND embedder = 'bge-m3' AND kind = 'c'",
                [&zero],
                |r| r.get(0),
            )
            .unwrap()
        };
        if let Ok(home) = std::env::var(HOME) {
            let k = super::open(std::path::Path::new(&home)).unwrap();
            assert_eq!(nearest(&k), 7);
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let k = super::open(home.path()).unwrap();
        k.execute(
            "INSERT INTO vec_index(rowid, embedder, kind, repo, ts, session, embedding)
             VALUES (7, 'bge-m3', 'c', '', 0, '', vec_bit(?1))",
            [&zero],
        )
        .unwrap();
        assert_eq!(nearest(&k), 7);
        drop(k);
        let name = "knowledge::tests::a_knowledge_db_opened_alone_reads_its_vector_index";
        // The test binary, as the harness started it.
        let out = std::process::Command::new(std::env::args_os().next().unwrap())
            .args(["--exact", name])
            .env(HOME, home.path())
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success() && said.contains("1 passed"), "{said}");
    }
