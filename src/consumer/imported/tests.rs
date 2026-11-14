    use super::*;

    fn doc(source_id: &str, title: &str, body: &str) -> ImportDoc {
        ImportDoc {
            uid: format!("claude-mem:abc:{source_id}"),
            source: "claude-mem:abc".into(),
            source_id: source_id.into(),
            kind: "decision".into(),
            repo: "claude-mem:free-mem".into(),
            session: "s1".into(),
            ts: 1_000,
            title: title.into(),
            body: body.into(),
        }
    }

    fn count(k: &Connection, table: &str) -> i64 {
        k.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn imported_fts_finds_an_imported_title() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_imports(vec![
            doc("o1", "Chose SQLite", "Because it is one file."),
            doc("o2", "Tabs", "Indent with tabs."),
        ])
        .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(Imported.step(&raw, &k, &device, 0).unwrap(), 2);
        let uid: String = k
            .query_row(
                "SELECT i.uid FROM imported_fts f JOIN imported i ON i.rowid = f.rowid
                 WHERE imported_fts MATCH 'SQLite'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uid, "claude-mem:abc:o1");
    }

    /// D5: a restore that lost ops takes their documents out with them, and only theirs. The ops
    /// that take their seqs read in again under the freed rowids, which the contentless index
    /// takes back (OpenCodeReview on #305).
    #[test]
    fn a_rewind_removes_the_rows_of_the_lost_ops_only() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        raw.append_imports(vec![doc("o1", "Kept", "One.")]).unwrap();
        raw.append_imports(vec![doc("o2", "Lost", "Two.")]).unwrap();
        raw.append_imports(vec![doc("o3", "Lost", "Three.")])
            .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        assert_eq!(Imported.step(&raw, &k, &device, 0).unwrap(), 3);
        Imported.rewind(&k, &device, 1).unwrap();
        assert_eq!((count(&k, "imported"), count(&k, "imported_fts")), (1, 1));
        let left: String = k
            .query_row(
                "SELECT i.title FROM imported_fts f JOIN imported i ON i.rowid = f.rowid",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, "Kept");
        assert_eq!(Imported.step(&raw, &k, &device, 1).unwrap(), 3);
        let found: Vec<String> = k
            .prepare(
                "SELECT i.source_id FROM imported_fts f JOIN imported i ON i.rowid = f.rowid
                 WHERE imported_fts MATCH 'Three' ORDER BY i.rowid",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(found, ["o3"]);
        k.execute(
            "INSERT INTO imported_fts(imported_fts) VALUES('integrity-check')",
            [],
        )
        .unwrap();
    }

    /// #506: how a prompt's block names an imported document, by its newest row: an observation
    /// by its title and its whole body, its narrative and its facts (parity row 31), a title that
    /// only begins a word of it kept, labelled imported with its status unknown; a summary by its
    /// body's first sentence (its request); a note saved without a title by its whole text once.
    /// A prompt, and a forgotten document, take no place.
    #[test]
    fn a_prompt_names_an_imported_document_by_its_newest_row() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let kind = |id: &str, kind: &str, title: &str, body: &str| ImportDoc {
            kind: kind.into(),
            ..doc(id, title, body)
        };
        let untitled = "Use tabs in Makefiles. Make needs them, and spaces break the build where \
                        nobody looks.";
        // `mcp::untitled`: a note saved without a title is titled with its first 60 characters.
        let title = format!("{}...", &untitled[..60]);
        raw.append_imports(vec![
            doc("o1", "Old title", "Old text."),
            doc(
                "o1",
                "Gateway retries",
                "Payments retry on a timeout. The gateway answers late at night.\n- three times",
            ),
            kind(
                "s1",
                "summary",
                "",
                "Request: Fix the retry.\nLearned: It was the clock.",
            ),
            kind("p1", "prompt", "", "Why does the payment fail?"),
            ImportDoc {
                source: crate::raw::NOTE_SOURCE.into(),
                ..kind("n1", "note", &title, untitled)
            },
            doc("o2", "Forgotten", "Gone."),
            doc(
                "o3",
                "Plan",
                "Planet-scale storage was picked:\n- one region\n- two copies",
            ),
        ])
        .unwrap();
        raw.append_ops(&[(
            OpKind::Forget,
            serde_json::json!({"uid": "claude-mem:abc:o2"}),
        )])
        .unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let device = raw.device().to_owned();
        Imported.step(&raw, &k, &device, 0).unwrap();
        let line = |uid: &str| prompt_line(&k, &raw, uid).unwrap();
        let at = |what: &str, words: &str| Some((1_000, what.to_owned(), words.to_owned()));
        assert_eq!(
            line("claude-mem:abc:o1"),
            at(
                "imported decision (status unknown)",
                "Gateway retries: Payments retry on a timeout. The gateway answers late at night.\n\
                 - three times"
            )
        );
        assert_eq!(
            line("claude-mem:abc:o3"),
            at(
                "imported decision (status unknown)",
                "Plan: Planet-scale storage was picked:\n- one region\n- two copies"
            )
        );
        assert_eq!(
            line("claude-mem:abc:s1"),
            at(
                "imported summary (status unknown)",
                "Request: Fix the retry."
            )
        );
        assert_eq!(line("claude-mem:abc:n1"), at("note", untitled));
        assert_eq!(line("claude-mem:abc:p1"), None);
        assert_eq!(line("claude-mem:abc:o2"), None);
        assert_eq!(line("claude-mem:abc:none"), None);
    }

    /// docs/file-note.md F10, F12 (tests 17, 18): a files op is no document, and `imported_files`
    /// keeps a row per path of each list, `\\` read as `/`, each with its op's number of paths; a
    /// rewind drops the rows of the ops it takes back, and a rebuild makes the same rows again.
    #[test]
    fn a_files_op_is_indexed_and_is_no_document() {
        let mut s = crate::search::b::fixture::Store::new();
        let uid = s.imported_with_files(
            "o1",
            ("p", "cm"),
            "bugfix",
            3_000,
            "Fixed the reader",
            &["src/a.rs", r"src\b.rs"],
            &["src/a.rs"],
        );
        s.run();
        let home = s.home.path().to_owned();
        let rows = || -> Vec<(String, String, bool, i64)> {
            let k = crate::knowledge::open(&home).unwrap();
            k.prepare("SELECT name, path, modified, named FROM imported_files WHERE uid = ?1 ORDER BY rowid")
                .unwrap()
                .query_map([&uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let expected = vec![
            ("a.rs".to_owned(), "src/a.rs".to_owned(), false, 3),
            ("b.rs".to_owned(), "src/b.rs".to_owned(), false, 3),
            ("a.rs".to_owned(), "src/a.rs".to_owned(), true, 3),
        ];
        assert_eq!(rows(), expected);
        let k = crate::knowledge::open(&home).unwrap();
        assert_eq!(count(&k, "imported"), 1, "the files op is no document");
        let got = crate::search::b::get(&home, &uid).unwrap().unwrap();
        assert!(
            got.contains("Fixed the reader") && !got.contains("src/b.rs"),
            "{got}"
        );
        let device = s.raw.device().to_owned();
        let files_seq = s
            .raw
            .ops_after(&device, 0, 100)
            .unwrap()
            .into_iter()
            .find(|o| crate::raw::is_import_files(&o.body))
            .unwrap()
            .op_seq;
        ImportedFiles.rewind(&k, &device, files_seq - 1).unwrap();
        assert!(rows().is_empty());
        assert_eq!(count(&k, "imported"), 1);
        drop(k);
        let crate::search::b::fixture::Store { home: _kept, raw } = s;
        drop(raw);
        crate::worker::rebuild(&home).unwrap();
        assert_eq!(rows(), expected);
    }
