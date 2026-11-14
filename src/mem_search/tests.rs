    use super::*;

    fn state(scope: &str, expires_at: i64) -> State {
        State {
            scope: scope.into(),
            expires_at,
            step: 2,
            options: Options {
                query: "q".into(),
                project: None,
                repo: None,
                limit: 20,
                max_details: 3,
                depth_before: 2,
                depth_after: 2,
                dumb: false,
            },
            eligible: Vec::new(),
            trace: vec!["index"],
        }
    }

    fn row(id: &str, kind: &'static str, title: &str) -> Row {
        Row {
            id: id.into(),
            kind,
            project: "github.com/o/r".into(),
            title: title.into(),
            created_at: None,
            kind_of: None,
        }
    }

    /// claude-mem's store: at most 1024 continuations, the oldest given up first; an expired one
    /// gone at the next put; one of another scope not found.
    #[test]
    fn the_store_keeps_1024_continuations_and_drops_expired_ones() {
        let mut c = Cursors::default();
        for i in 0..1030 {
            c.put(format!("k{i}"), state("s", 100), 0).unwrap();
        }
        assert_eq!(c.entries.len(), 1024);
        assert!(c.get("k5", "s").is_none() && c.get("k1029", "s").is_some());
        assert!(c.get("k1029", "other").is_none());
        c.put("new".into(), state("s", 200), 150).unwrap();
        assert_eq!((c.entries.len(), c.order.len()), (1, 1));
        let one = serde_json::to_string(&state("s", 200)).unwrap().len();
        assert_eq!(c.bytes, one);
    }

    /// A continuation is `ms_` and 24 base64url characters, a new one each time.
    #[test]
    fn a_continuation_is_27_characters_of_base64url() {
        let (a, b) = (cursor().unwrap(), cursor().unwrap());
        assert!(is_cursor(&a) && is_cursor(&b) && a != b, "{a} {b}");
        assert!(!is_cursor("ms_short") && !is_cursor(&format!("ms_{}", "+".repeat(24))));
    }

    /// claude-mem's `ranked`: the most title words first, a word's first five characters enough
    /// for words of five or more, stop words and prompts left out, ties in index order.
    #[test]
    fn auto_ranks_by_title_words_as_claude_mem_does() {
        let rows = [
            row("1", "observation", "Parser errors"),
            row("2", "prompt", "parser errors parsers"),
            row("3", "summary", "Parsers and their errors"),
            row("6", "observation", "The parsing"),
            row("4", "observation", "Unrelated"),
            row("5", "observation", "errors of the parser and the lexer"),
        ];
        let got: Vec<String> = ranked(&rows, "the parser errors", 5)
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(got, ["1", "3", "5"]);
        assert_eq!(terms("What is THE memory of Parser? parser"), ["parser"]);
    }

    /// An answer over 64 KiB keeps its index while it can and halves its longest detail, each
    /// cut marked; a detail of more than 8 KiB comes cut.
    /// claude-mem #4865's ranking: by how many of the query's words a title holds, each counted
    /// once and exactly (stop words too), then by the search's rank.
    #[test]
    fn dumb_ranks_titles_by_how_many_query_words_they_hold() {
        let rows = vec![
            row("d", "observation", "Parsers of nothing"),
            row("a", "observation", "Nothing here"),
            row("e", "observation", "Quartz notes"),
            row("b", "observation", "The parser"),
            row("c", "observation", "The quartz parser parser"),
        ];
        let ids: Vec<String> = by_title(rows, "the quartz parser")
            .into_iter()
            .map(|r| r.id)
            .collect();
        // "the" counts though it is a stop word; "Parsers" is not "parser".
        assert_eq!(ids, ["c", "b", "e", "d", "a"]);
    }

    #[test]
    fn an_answer_is_bounded_as_claude_mems_is() {
        let home = tempfile::tempdir().unwrap();
        let cursors = std::sync::Mutex::new(Cursors::default());
        let scope: Scope = &|_| Ok(Ok(None));
        let search = Search {
            home: home.path(),
            scope: "s".into(),
            cursors: &cursors,
            now: 0,
            repo: scope,
            rules: &crate::redact::Rules::default(),
            hook: false,
            vector: None,
        };
        let detail = |id: &str| Detail {
            row: row(id, "observation", "t"),
            content: "x".repeat(DETAIL_BYTES),
            truncated: false,
        };
        let answer = |details: Vec<Detail>| Answer {
            auto: false,
            step: 3,
            project: None,
            reason: None,
            index: Vec::new(),
            details,
            continuation: None,
            max_details: 5,
            trace: Vec::new(),
            dumb: false,
        };
        let bounded = search
            .bounded(answer((0..10).map(|i| detail(&i.to_string())).collect()))
            .unwrap();
        assert!(render(&bounded).len() <= RESPONSE_BYTES);
        assert!(bounded.details.iter().any(|d| d.truncated));
        assert!(render(&bounded).contains("[Detail shortened to fit the memory budget.]"));
        // What the fence quotes grows past the bound though the text does not (Codex on #527).
        let closing = "</oboete-memory".repeat(DETAIL_BYTES / 15);
        let quoted = answer(
            (0..5)
                .map(|i| Detail {
                    content: closing.clone(),
                    ..detail(&i.to_string())
                })
                .collect(),
        );
        assert!(render(&quoted).len() <= RESPONSE_BYTES);
        assert!(delivered(&quoted) > RESPONSE_BYTES);
        assert!(delivered(&search.bounded(quoted).unwrap()) <= RESPONSE_BYTES);
        assert_eq!(short_text("aé", 2), "a");
        // dumb_search's index, with no continuation, gives up rows while it shows no details
        // (claude-mem 15.0), where mem_search's would refuse.
        let wide = |dumb: bool| Answer {
            auto: true,
            step: 1,
            reason: Some("no_relevant_candidates"),
            index: (0..400)
                .map(|i| row(&i.to_string(), "observation", &"t".repeat(240)))
                .collect(),
            details: Vec::new(),
            dumb,
            ..answer(Vec::new())
        };
        let kept = search.bounded(wide(true)).unwrap();
        assert!(delivered(&kept) <= RESPONSE_BYTES && kept.index.len() > 1);
        assert_eq!(kept.reason, Some("index_truncated"));
        assert!(search.bounded(wide(false)).is_err());
    }
