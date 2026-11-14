    use super::*;
    use serde_json::json;

    fn roots(custom: &[&str]) -> Roots {
        Roots {
            home: "/h".into(),
            claude_projects: "/h/.claude/projects".into(),
            codex_memories: "/h/.codex/memories".into(),
            custom: custom.iter().map(PathBuf::from).collect(),
        }
    }

    /// cubic on #895: a hook with no time left asks nothing and is not late. The call may be no
    /// lookup at all, and a late lookup's line on stderr would name one.
    #[test]
    fn a_hook_with_no_time_left_asks_no_lookup() {
        let home = tempfile::tempdir().unwrap();
        let over =
            crate::hook_time::Clock::started(Duration::from_secs(10), Duration::from_secs(2));
        over.inject_within(Some(Duration::from_secs(5)));
        let read = json!({"session_id": "s1", "cwd": "/x", "tool_name": "Read",
            "tool_input": {"file_path": "/x/a.rs"}});
        assert!(matches!(answer(home.path(), &read, 1_000, &over), Ok(None)));
    }

    /// Codex on #830: an answer heard past the deadline is late, as one not heard by then is,
    /// so no file note follows it; one heard in time is the answer.
    #[test]
    fn an_answer_heard_past_the_deadline_is_late() {
        let deadline = Instant::now();
        let wait = Duration::from_secs(3);
        let answer = || Ok(Ok(Some("index".to_owned())));
        let late = in_time(
            answer(),
            deadline,
            deadline + Duration::from_millis(1),
            wait,
        );
        assert!(late.is_err_and(|e| e.is::<Late>()));
        assert_eq!(
            in_time(answer(), deadline, deadline, wait)
                .unwrap()
                .as_deref(),
            Some("index")
        );
        let none = in_time(Err(RecvTimeoutError::Timeout), deadline, deadline, wait);
        assert!(none.is_err_and(|e| e.is::<Late>()));
    }

    fn query(tool: &str, args: Value) -> Option<String> {
        lookup(tool, &args, Path::new("/work"), &roots(&["/notes"])).map(|l| l.query)
    }

    /// claude-mem's recognition (tests/memory/native-memory-bridge.test.ts and
    /// bash-memory-search-pattern.test.ts at v13.39.0 as the model): native searches, the memory
    /// tool, and reads of a memory folder by Read, Grep, Glob, an MCP reader or a shell reader;
    /// every other call, and claude-mem's and oboete's own tools, none.
    #[test]
    fn a_lookup_is_told_as_claude_mem_tells_one() {
        let memory = "/h/.claude/projects/-work/memory/MEMORY.md";
        for (tool, args, want) in [
            (
                "memory_search",
                json!({"query": "deploy steps"}),
                Some("deploy steps"),
            ),
            ("mcp__x__memory_search", json!({"query": "q"}), Some("q")),
            ("memory_search", json!({"query": "  "}), None),
            (
                "memory",
                json!({"command": "view", "path": memory}),
                Some(memory),
            ),
            (
                "memory",
                json!({"command": "search"}),
                Some("preferences decisions corrections"),
            ),
            ("memory", json!({"command": "create"}), None),
            (
                "Read",
                json!({"file_path": memory}),
                Some("preferences decisions corrections"),
            ),
            (
                "Read",
                json!({"file_path": "/h/.claude/projects/-work/memory/release_notes.md"}),
                Some("release notes"),
            ),
            (
                "Read",
                json!({"file_path": "/h/.claude/projects/-work/x.md"}),
                None,
            ),
            (
                "Grep",
                json!({"path": "/h/.codex/memories", "pattern": "tabs"}),
                Some("tabs"),
            ),
            (
                "Glob",
                json!({"path": "/notes/a", "pattern": "*.md"}),
                Some("*.md"),
            ),
            ("Read", json!({"file_path": "src/main.rs"}), None),
            (
                "mcp__fs__read_file",
                json!({"path": "~/.codex/memories/x.md"}),
                Some("x"),
            ),
            (
                "Bash",
                json!({"command": format!("cat '{memory}' | head -5")}),
                Some("preferences decisions corrections"),
            ),
            (
                "Bash",
                json!({"command": "rg -n --glob '*.md' tabs ~/.claude/projects/p/memory"}),
                Some("tabs"),
            ),
            (
                "Bash",
                json!({"command": "grep -e spaces -e tabs /h/.codex/memories/a.md"}),
                Some("spaces"),
            ),
            (
                "Bash",
                json!({"command": "grep -f pats.txt /h/.codex/memories/a.md"}),
                Some("a"),
            ),
            (
                "Bash",
                json!({"command": "grep -A 3 retry /h/.codex/memories/a.md"}),
                Some("retry"),
            ),
            ("Bash", json!({"command": "ls /h/.codex/memories"}), None),
            ("Bash", json!({"command": "cat src/main.rs"}), None),
            (
                "Bash",
                json!({"command": "# cat /h/.codex/memories/a.md"}),
                None,
            ),
            // shell-quote's glob is no string, so no path (claude-mem's filter).
            (
                "Bash",
                json!({"command": "cat /h/.codex/memories/*.md"}),
                None,
            ),
            (
                "Bash",
                json!({"command": "cat /h/.claude/projects/*/memory/MEMORY.md"}),
                None,
            ),
            (
                "mcp__plugin_claude-mem_mcp-search__memory_search",
                json!({"query": "q"}),
                None,
            ),
            ("mcp__oboete__memory_search", json!({"query": "q"}), None),
            ("Edit", json!({"file_path": memory}), None),
        ] {
            assert_eq!(query(tool, args.clone()).as_deref(), want, "{tool} {args}");
        }
        let long = "x".repeat(600);
        assert_eq!(
            query("memory_search", json!({"query": long}))
                .unwrap()
                .len(),
            500
        );
        let wide = "あ".repeat(400);
        assert!(
            query("memory_search", json!({"query": wide}))
                .unwrap()
                .len()
                <= 1024
        );
        let read = json!({"file_path": "../.claude/projects/p/memory/a.md"});
        let at = lookup("Read", &read, Path::new("/h/x"), &roots(&[])).unwrap();
        assert_eq!(
            at.memory_path.as_deref(),
            Some(Path::new("/h/.claude/projects/p/memory/a.md"))
        );
    }

    /// shell-quote's strings: quotes and escapes taken, operators between words, an unquoted
    /// glob left out.
    #[test]
    fn a_command_splits_into_its_words() {
        assert_eq!(
            words(r#"grep -e "a b" 'c d'|head>out; x\ y && z *.md 'q*' # w"#),
            ["grep", "-e", "a b", "c d", "head", "out", "x y", "z", "q*"]
        );
    }

    /// claude-mem's Codex adapter takes an MCP read's `paths` that are files.
    #[test]
    fn an_mcp_reads_paths_are_its_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let memories = dir.path().join("memories");
        std::fs::create_dir_all(&memories).unwrap();
        std::fs::write(memories.join("prefs.md"), "Tabs.").unwrap();
        let roots = Roots {
            home: dir.path().into(),
            claude_projects: dir.path().join("claude/projects"),
            codex_memories: memories.clone(),
            custom: Vec::new(),
        };
        let file = memories.join("prefs.md").to_string_lossy().into_owned();
        let gone = memories.join("gone.md").to_string_lossy().into_owned();
        let read = |paths: Vec<&str>| {
            lookup(
                "mcp__fs__read_files",
                &json!({"paths": paths}),
                dir.path(),
                &roots,
            )
            .map(|l| l.query)
        };
        assert_eq!(read(vec![&file]).as_deref(), Some("prefs"));
        assert_eq!(read(vec![&gone]), None);
    }
