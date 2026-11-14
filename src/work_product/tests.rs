    use super::*;
    use serde_json::json;

    fn source(kind: &str, id: &str) -> Source {
        Source {
            kind: kind.to_owned(),
            id: id.to_owned(),
            index: None,
        }
    }

    fn bash_at(command: &str, response: Value, at: i64, id: &str) -> Vec<Product> {
        from_tool_use(
            "Bash",
            &json!(json!({ "command": command }).to_string()),
            &json!(response.to_string()),
            Some(at),
            Some(source("tool-use", id)),
            false,
        )
    }

    fn bash(command: &str, response: Value) -> Vec<Product> {
        bash_at(command, response, 100, "5")
    }

    fn brief(products: &[Product]) -> Vec<Value> {
        products.iter().map(|product| {
            let mut value = json!({ "kind": product.kind, "title": product.title, "branch": product.branch, "url": product.url });
            if let Some(sha) = &product.sha { value["sha"] = json!(sha); }
            value
        }).collect()
    }

    #[test]
    fn an_artifact_publish_records_its_link_and_title() {
        let products = from_tool_use(
            "Artifact",
            &json!(json!({ "file_path": "/w/deck.html", "title": "Launch deck" }).to_string()),
            &json!(
                json!({ "url": "https://claude.ai/artifact/abc", "title": "Launch deck" })
                    .to_string()
            ),
            Some(100),
            Some(source("tool-use", "5")),
            false,
        );
        assert_eq!(
            products,
            vec![Product {
                kind: "artifact".to_owned(),
                title: "Launch deck".to_owned(),
                url: Some("https://claude.ai/artifact/abc".to_owned()),
                at: Some(100),
                source: Some(source("tool-use", "5")),
                ..Product::default()
            }]
        );
    }

    #[test]
    fn artifact_lists_reads_and_assets_publish_nothing() {
        for input in [
            json!({ "action": "list" }),
            json!({ "action": "read" }),
            json!({ "action": "describe" }),
            json!({ "action": "publish", "asset": true }),
            json!({ "asset": {} }),
        ] {
            assert!(
                from_tool_use(
                    "Artifact",
                    &input,
                    &json!({ "artifacts": [{ "url": "https://claude.ai/artifact/x" }] }),
                    None,
                    None,
                    false
                )
                .is_empty()
            );
        }
        assert!(
            from_tool_use(
                "Read",
                &json!({}),
                &json!({ "url": "https://example.com/report" }),
                None,
                None,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn gh_pr_create_records_the_pull_request_url_and_title() {
        let products = bash(
            r#"git push && rtk gh pr create --title "Add work product" --body x"#,
            json!({ "stdout": "https://github.com/o/r/pull/42\n" }),
        );
        assert_eq!(products[0].kind, "pull-request");
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(products[0].title, "Add work product");
        assert!(
            bash(
                r#"sqlite3 db "select * from tool_uses where tool_input like '%gh pr create%'""#,
                json!({ "stdout": "x|https://github.com/o/r/pull/7", "gitOperation": { "pr": {
                "number": 7, "url": "https://github.com/o/r/pull/7", "action": "created" } } })
            )
            .is_empty()
        );
    }

    #[test]
    fn a_pull_request_uses_its_recorded_url_and_defaults_to_its_number() {
        let products = bash(
            "gh pr create --body x",
            json!({ "stdout": "https://github.com/o/r/pull/7\n",
            "gitOperation": { "pr": { "url": "https://github.com/o/r/pull/42" } } }),
        );
        assert_eq!(products[0].title, "PR #42");
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(
            bash(
                "gh pr create --title 'A title'",
                json!({ "stdout": "https://github.com/o/r/pull/7" })
            )[0]
            .title,
            "A title"
        );
        assert!(bash("gh pr create", json!({ "stdout": "https://github.com/o/r/pull/7", "gitOperation": { "pr": { "url": "javascript:alert(1)" } } })).is_empty());
    }

    #[test]
    fn observers_record_only_human_facing_files_with_their_positions() {
        let paths = ["/w/src/app.ts", "/w/design/pass-01.png", "/w/report.html"].map(str::to_owned);
        let products = from_files(&paths, Some(50), Some(source("observation", "9")));
        assert_eq!(
            products
                .iter()
                .map(|product| (
                    product.kind.as_str(),
                    product.title.as_str(),
                    product.source.as_ref().unwrap().index
                ))
                .collect::<Vec<_>>(),
            vec![
                ("image", "pass-01.png", Some(1)),
                ("page", "report.html", Some(2))
            ]
        );
        assert!(products.iter().all(|product| product.at == Some(50) && product.source.as_ref().unwrap().id == "9"));
        assert_eq!(file_kind("relative/x.png"), None);
        assert_eq!(file_kind("/w/../etc/x.png"), None);
    }

    #[test]
    fn an_index_file_takes_its_folders_name() {
        let products = from_files(
            &[
                "/w/launch-review/index.html".to_owned(),
                "/index.html".to_owned(),
                "/w/hero.PNG".to_owned(),
            ],
            None,
            None,
        );
        assert_eq!(
            products
                .iter()
                .map(|product| product.title.as_str())
                .collect::<Vec<_>>(),
            vec!["launch-review", "index.html", "hero.PNG"]
        );
        assert!(products.iter().all(|product| product.source.is_none()));
    }

    #[test]
    fn unicode_paths_with_trailing_separators_keep_their_file_titles() {
        let paths = [
            "/tmp/é.png//",
            "/😀.png///",
            "/資料/index.html",
            "/資料/index.html///",
        ]
        .map(str::to_owned);
        let products = from_files(&paths, None, None);
        assert_eq!(
            products
                .iter()
                .map(|product| product.title.as_str())
                .collect::<Vec<_>>(),
            vec!["é.png", "😀.png", "資料", "in"]
        );
    }

    #[test]
    fn github_patterns_use_javascripts_ascii_words_and_digits() {
        assert_eq!(github_repo("https://github.com/owner/répo.git"), None);
        assert_eq!(
            kind_for_url("https://github.com/o/r/pull/１２", "link"),
            "link"
        );
        assert!(
            bash(
                "gh pr create",
                json!({"stdout": "https://github.com/o/r/pull/１２"})
            )
            .is_empty()
        );
    }

    #[test]
    fn git_commands_keep_javascripts_word_boundaries() {
        let products = from_command(
            "git commit中文 -m 'subject'",
            &json!({}),
            "[main abc1234] subject",
            None,
            None,
            false,
        );
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].sha.as_deref(), Some("abc1234"));
    }

    #[test]
    fn git_patterns_keep_javascripts_whitespace_classes() {
        let products = from_command(
            "git\u{feff}commit -m 'subject'",
            &json!({}),
            "[main abc1234] subject",
            None,
            None,
            false,
        );
        assert_eq!(products.len(), 1);
        assert!(
            from_command(
                "git\u{85}commit -m 'subject'",
                &json!({}),
                "[main abc1234] subject",
                None,
                None,
                false
            )
            .is_empty()
        );
        let pattern = js_regex(r"^[\w.-]+\s+\S+$").unwrap();
        assert!(pattern.is_match("ascii\u{feff}name\u{85}x"));
        assert!(!pattern.is_match("répo name"));
    }

    #[test]
    fn printed_commits_and_pull_requests_accept_javascripts_line_terminators() {
        for separator in ["\r\n", "\r", "\u{2028}", "\u{2029}"] {
            let products = bash(
                "rtk git commit -m 'subject'",
                json!(format!("before{separator}ok abc1234{separator}")),
            );
            assert_eq!(products[0].title, "abc1234 subject", "{separator:?}");
            let products = bash(
                "gh pr create",
                json!({ "stdout": format!("before{separator}https://github.com/o/r/pull/42{separator}") }),
            );
            assert_eq!(products[0].title, "PR #42", "{separator:?}");
            assert!(
                bash(
                    "git checkout -b feat/x",
                    json!(format!("before{separator}fatal: branch exists"))
                )
                .is_empty()
            );
            assert!(
                bash(
                    "git push origin feat/x",
                    json!(format!("before{separator}fatal: remote failed"))
                )
                .is_empty()
            );
        }
        let products = bash(
            "git push origin feat/x",
            json!({ "stdout": "To https://github.com/o/r.git\r\n   1111111..2222222 feat/x -> feat/x\r\n" }),
        );
        assert_eq!(products[0].repo, None);
    }

    #[test]
    fn work_product_fields_attach_links_and_files_and_drop_unsafe_urls() {
        let fields = json!({
            "work_product": [{ "title": "Q4 deck", "url": "https://docs.google.com/presentation/d/1" },
                { "path": "/w/out/hero.png" }, "javascript:alert(1)"],
            "docs_url": "https://docs.example.com/guide", "status": "done"
        });
        let products = from_fields(
            fields.as_object().unwrap(),
            Some(3),
            Some(source("work-state", "11")),
        );
        assert_eq!(
            products
                .iter()
                .map(|product| (
                    product.kind.as_str(),
                    product.title.as_str(),
                    product.field.as_deref(),
                    product.source.as_ref().unwrap().index
                ))
                .collect::<Vec<_>>(),
            vec![
                ("link", "Q4 deck", Some("work_product"), Some(0)),
                ("image", "hero.png", Some("work_product"), Some(1)),
                (
                    "documentation",
                    "docs.example.com",
                    Some("docs_url"),
                    Some(3)
                )
            ]
        );
        assert_eq!(products[1].path.as_deref(), Some("/w/out/hero.png"));
        assert!(products.iter().all(|product| product.at == Some(3)));
    }

    #[test]
    fn several_work_products_with_titles_arrive_as_one_json_string() {
        let fields = json!({ "work_product": json!([
            { "title": "Q4 deck", "url": "https://docs.google.com/presentation/d/1" }, { "path": "/w/out/hero.png" }
        ]).to_string() });
        assert_eq!(
            from_fields(fields.as_object().unwrap(), None, None)
                .iter()
                .map(|product| (product.kind.as_str(), product.title.as_str()))
                .collect::<Vec<_>>(),
            vec![("link", "Q4 deck"), ("image", "hero.png")]
        );
        let fields = json!({ "work_product": "/w/out/report.pdf" });
        assert_eq!(
            from_fields(fields.as_object().unwrap(), None, None)[0].title,
            "report.pdf"
        );
    }

    #[test]
    fn local_paths_are_absolute_and_have_no_parent_part_or_nul() {
        for path in [
            r"C:\work\hero.png",
            "C:/work/report.pdf",
            r"\\server\share\hero.png",
            "/w/out/hero.png",
        ] {
            assert!(is_local_path(path), "{path}");
        }
        for path in [
            r"C:\work\..\secret.png",
            r"work\hero.png",
            "C:hero.png",
            "/w/../hero.png",
            "/w/hero\0.png",
            r"/w\..\hero.png",
        ] {
            assert!(!is_local_path(path), "{path}");
        }
        assert_eq!(file_kind(r"C:\work\hero.PNG"), Some("image"));
        assert_eq!(file_kind(r"\\server\share\deck.pptx"), Some("deck"));
    }

    #[test]
    fn merging_keeps_the_newest_entry_for_each_link_or_file() {
        let merged = merge(
            vec![
                vec![Product {
                    url: Some("https://a".to_owned()),
                    at: Some(1),
                    title: "old".to_owned(),
                    ..Product::default()
                }],
                vec![
                    Product {
                        url: Some("https://a".to_owned()),
                        at: Some(2),
                        title: "new".to_owned(),
                        ..Product::default()
                    },
                    Product {
                        path: Some("/b.png".to_owned()),
                        at: Some(0),
                        ..Product::default()
                    },
                ],
            ],
            60,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "new");
        assert_eq!(merged[1].path.as_deref(), Some("/b.png"));
    }

    #[test]
    fn a_git_commit_names_its_subject_and_has_no_link_without_a_push() {
        let products = bash(
            "/usr/bin/git -C /w/repo commit -F /w/repo/.scratch/message.txt",
            json!({
                "stdout": "[teambrain-purchase 8ade2c4] fix(teambrain): QA for self-serve purchase (#333)\n 9 files changed, 642 insertions(+), 84 deletions(-)\n create mode 100644 src/lib/teambrain/billing-card.ts",
                "stderr": "", "gitOperation": { "commit": { "sha": "8ade2c4", "kind": "committed", "branch": "teambrain-purchase" } }
            }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "8ade2c4 fix(teambrain): QA for self-serve purchase (#333)",
            "sha": "8ade2c4", "branch": "teambrain-purchase", "url": null })
            ]
        );
        let detached = bash(
            r#"git commit -m "fix(pro): the opt-out button ignores presses no person could have made""#,
            json!({ "stdout": "[detached HEAD 6edf530] fix(pro): the opt-out button ignores presses no person could have made\n 7 files changed, 567 insertions(+), 146 deletions(-)" }),
        );
        assert_eq!(detached[0].branch, None);
        assert_eq!(
            detached[0].title,
            "6edf530 fix(pro): the opt-out button ignores presses no person could have made"
        );
    }

    #[test]
    fn a_quiet_commit_takes_its_subject_from_git_log() {
        let command = [
            "cd \"/w/claude-mem 🧠/my-work-react-viewer\"; git reset -q; git add src/services/work-view/request-projection.mjs tests/worker/work-view/request-projection.test.mjs && git commit -q -F - <<'EOF'",
            "fix(work-view): English asks need an action verb before they become requests", "", "Port the prototype's action-verb rule.", "EOF",
            "git add -A src tests plugin && git commit -q -F - <<'EOF'",
            "feat(work-view): progress readings, measured ETA, goal links and the build story", "", "- record-progress keeps readings[].", "EOF",
            "git log --oneline -3; git status --short | grep -v \"^??\"",
        ].join("\n");
        let products = bash(
            &command,
            json!({ "stdout": "bc8cd096b feat(work-view): progress readings, measured ETA, goal links and the build story\naf4ba749c fix(work-view): English asks need an action verb before they become requests\n3990ea7e1 fix(work-view): port request-grouping review fixes from the prototype", "stderr": "" }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "af4ba74 fix(work-view): English asks need an action verb before they become requests", "sha": "af4ba749c", "branch": null, "url": null }),
                json!({ "kind": "commit", "title": "bc8cd09 feat(work-view): progress readings, measured ETA, goal links and the build story", "sha": "bc8cd096b", "branch": null, "url": null })
            ]
        );
    }

    #[test]
    fn an_rtk_commit_takes_its_sha_from_the_ok_line() {
        let products = bash(
            "rtk git commit -m 'Align timeline hydration assertions with expanded scope'",
            json!("ok bb07907\n"),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "bb07907 Align timeline hydration assertions with expanded scope", "sha": "bb07907", "branch": null, "url": null })
            ]
        );
    }

    #[test]
    fn a_quiet_commit_with_no_printed_subject_is_not_guessed() {
        assert!(
            bash(
                "/usr/bin/git commit -q -F .scratch/hooks/message.txt",
                json!({ "stdout": "", "stderr": "" })
            )
            .is_empty()
        );
    }

    #[test]
    fn a_command_that_only_mentions_git_commit_or_git_push_made_nothing() {
        let out =
            json!({ "stdout": "[feat/x abc1234] subject\n   1111111..2222222  feat/x -> feat/x" });
        assert!(bash(r#"cd ~/.claude-mem && sqlite3 -readonly claude-mem.db "SELECT id FROM tool_uses WHERE tool_input LIKE '%git commit%' AND tool_input LIKE '%git push%'""#,
            out.clone()).is_empty());
        assert!(bash("echo 'git commit'; printf 'git push'", out).is_empty());
    }

    #[test]
    fn a_git_push_links_its_branch_and_the_commit_made_by_the_same_command() {
        let command = [
            "W=\"/w/claude-mem 🧠/install-funnel\"; cd \"$W\"; git add src/services/agent-runner/detect.ts tests/services/agent-runner/detect.test.ts && git commit -q -F - <<'EOF'",
            "feat(agent-runner): detect which agent CLIs are installed and logged in", "", "Adds detectUsableAgents().", "EOF",
            "git log --oneline -1; git push -u origin feat/install-funnel-early 2>&1 | tail -6",
        ].join("\n");
        let products = bash(
            &command,
            json!({
                "stdout": "30d795d5a feat(agent-runner): detect which agent CLIs are installed and logged in\nremote: GitHub found 2 vulnerabilities on thedotmack/claude-mem's default branch (2 critical). To find out more, visit:\nremote:      https://github.com/thedotmack/claude-mem/security/dependabot\nremote:\nTo https://github.com/thedotmack/claude-mem.git\n * [new branch]          feat/install-funnel-early -> feat/install-funnel-early\nbranch 'feat/install-funnel-early' set up to track 'origin/feat/install-funnel-early'.",
                "stderr": "", "gitOperation": { "push": { "branch": "feat/install-funnel-early" } }
            }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "commit", "title": "30d795d feat(agent-runner): detect which agent CLIs are installed and logged in", "sha": "30d795d5a", "branch": "feat/install-funnel-early", "url": "https://github.com/thedotmack/claude-mem/commit/30d795d5a" }),
                json!({ "kind": "branch", "title": "feat/install-funnel-early", "branch": "feat/install-funnel-early", "url": "https://github.com/thedotmack/claude-mem/tree/feat/install-funnel-early" })
            ]
        );
    }

    #[test]
    fn a_push_uses_the_commands_remote_when_the_to_line_is_missing() {
        let products = bash(
            "/usr/bin/git push https://github.com/SuperMarioYL/claude-mem.git HEAD:perf/search-reads-read-worker 2>&1 | tail -1",
            json!({ "stdout": "   92aa26602..659ae0d38  HEAD -> perf/search-reads-read-worker", "stderr": "", "gitOperation": { "push": { "branch": "perf/search-reads-read-worker" } } }),
        );
        assert_eq!(
            products[0].branch.as_deref(),
            Some("perf/search-reads-read-worker")
        );
        assert_eq!(products[0].heads, vec!["659ae0d38"]);
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/SuperMarioYL/claude-mem/tree/perf/search-reads-read-worker")
        );
    }

    #[test]
    fn a_push_with_only_a_remote_name_has_no_link() {
        let products = bash(
            "cd \"/w/claude-mem 🧠/my-work-react-viewer\" && for i in 1 2 3; do git push -u origin feat/my-work-react-viewer 2>&1 | tail -2 && break; sleep 5; done",
            json!({ "stdout": "   81cc75296..3edf6fd8e  feat/my-work-react-viewer -> feat/my-work-react-viewer\nbranch 'feat/my-work-react-viewer' set up to track 'origin/feat/my-work-react-viewer'.",
                "gitOperation": { "push": { "branch": "feat/my-work-react-viewer" } } }),
        );
        assert_eq!(products[0].kind, "branch");
        assert_eq!(
            products[0].branch.as_deref(),
            Some("feat/my-work-react-viewer")
        );
        assert_eq!(products[0].url, None);
    }

    #[test]
    fn one_rtk_sha_for_two_commits_is_not_guessed() {
        let products = bash(
            "rtk git add -A && rtk git commit -m 'chore: bump version to 15.0.1' && rtk git tag -a v15.0.1 -m 'Version 15.0.1' && rtk git push origin HEAD:main && rtk git push origin v15.0.1 && rtk git add CHANGELOG.md && rtk git commit -m 'docs: update changelog for v15.0.1' && rtk git push origin HEAD:main",
            json!(
                "ok 1 file changed, 12 insertions(+)\nok 85e4ee3\nremote:\nTo https://github.com/thedotmack/claude-mem.git\n   0e7de4ee4..85e4ee31f  HEAD -> main\nok main\n"
            ),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "branch", "title": "main", "branch": "main", "url": "https://github.com/thedotmack/claude-mem/tree/main" })
            ]
        );
    }

    #[test]
    fn a_quiet_push_names_its_branch_without_a_ref_line() {
        let products = bash(
            "git push -q -u origin feat/work-view-git-products",
            json!({ "stdout": "", "stderr": "" }),
        );
        assert_eq!(products[0].kind, "branch");
        assert_eq!(
            products[0].branch.as_deref(),
            Some("feat/work-view-git-products")
        );
        assert_eq!(products[0].url, None);
        let products = bash(
            "git push -q https://github.com/o/r.git HEAD:refs/heads/feat/x",
            json!({ "stdout": "" }),
        );
        assert_eq!(
            products[0].url.as_deref(),
            Some("https://github.com/o/r/tree/feat/x")
        );
    }

    #[test]
    fn rejected_or_deleting_pushes_make_nothing_and_other_hosts_have_no_link() {
        assert!(bash("git push origin feat/x", json!({ "stdout": "", "stderr": " ! [rejected]        feat/x -> feat/x (fetch first)\nerror: failed to push some refs to 'https://github.com/o/r.git'" })).is_empty());
        for flags in ["--delete", "-d", "--tags", "--all", "--mirror"] {
            assert!(
                bash(
                    &format!("git push {flags} origin feat/x"),
                    json!({ "stdout": "" })
                )
                .is_empty()
            );
        }
        let products = bash(
            "git push origin feat/x",
            json!({ "stdout": "", "stderr": "To gitlab.com:o/r.git\n   1111111..2222222  feat/x -> feat/x" }),
        );
        assert_eq!(products[0].branch.as_deref(), Some("feat/x"));
        assert_eq!(products[0].url, None);
    }

    #[test]
    fn github_remotes_name_their_owner_and_repository() {
        for remote in [
            "https://github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r",
            " https://github.com/o/r.git/ ",
        ] {
            assert_eq!(github_repo(remote).as_deref(), Some("o/r"), "{remote}");
        }
        for remote in [
            "https://gitlab.com/o/r.git",
            "origin",
            "https://github.com/o/r/pull/1",
        ] {
            assert_eq!(github_repo(remote), None, "{remote}");
        }
    }

    #[test]
    fn git_checkout_and_git_switch_record_the_branch_they_created() {
        let products = bash(
            "/usr/bin/git fetch origin -q && /usr/bin/git checkout -b fix/launcher-bun-be-bun-reentry-4933 origin/main && /usr/bin/git log -1 --oneline",
            json!({ "stdout": "Switched to a new branch 'fix/launcher-bun-be-bun-reentry-4933'\nbranch 'fix/launcher-bun-be-bun-reentry-4933' set up to track 'origin/main'.\nf3cb32e43 fix: skip reasoning on OpenRouter field condensation requests (#4925)" }),
        );
        assert_eq!(
            brief(&products),
            vec![
                json!({ "kind": "branch", "title": "fix/launcher-bun-be-bun-reentry-4933", "branch": "fix/launcher-bun-be-bun-reentry-4933", "url": null })
            ]
        );
        let products = bash(
            "rtk git switch -c fix/cloud-sync-census-api-execution origin/main",
            json!(
                "Switched to a new branch 'fix/cloud-sync-census-api-execution'\nbranch 'fix/cloud-sync-census-api-execution' set up to track 'origin/main'.\n"
            ),
        );
        assert_eq!(
            products[0].branch.as_deref(),
            Some("fix/cloud-sync-census-api-execution")
        );
        assert!(
            bash(
                "git checkout -b feat/x",
                json!({ "stdout": "", "stderr": "fatal: a branch named 'feat/x' already exists" })
            )
            .is_empty()
        );
    }

    #[test]
    fn merging_links_commits_from_a_later_push_and_their_shared_source() {
        let commits = bash_at(
            "git commit -q -m \"fix(work-view): English asks need an action verb before they become requests\" && git commit -q -m \"feat(work-view): progress readings, measured ETA, goal links and the build story\"; git log --oneline -2",
            json!({ "stdout": "bc8cd096b feat(work-view): progress readings, measured ETA, goal links and the build story\naf4ba749c fix(work-view): English asks need an action verb before they become requests" }),
            10,
            "179579",
        );
        let created = bash_at(
            "git checkout -b feat/my-work-react-viewer",
            json!({ "stdout": "Switched to a new branch 'feat/my-work-react-viewer'" }),
            5,
            "9",
        );
        let pushed = bash_at(
            "rtk git push origin feat/my-work-react-viewer 2>&1 | tail -3",
            json!({ "stdout": " 23 files changed, 1101 insertions(+), 597 deletions(-)\nTo https://github.com/thedotmack/claude-mem.git\n   3990ea7e1..bc8cd096b  feat/my-work-react-viewer -> feat/my-work-react-viewer\nok feat/my-work-react-viewer",
                "gitOperation": { "push": { "branch": "feat/my-work-react-viewer" } } }),
            20,
            "179704",
        );
        let again = bash_at(
            "git commit -m x",
            json!({ "stdout": "[feat/my-work-react-viewer bc8cd096b] feat(work-view): progress readings, measured ETA, goal links and the build story" }),
            8,
            "11",
        );
        let merged = merge(vec![commits, created, pushed.clone(), again], 60);
        assert_eq!(
            merged
                .iter()
                .map(|product| json!([product.kind, product.title, product.url]))
                .collect::<Vec<_>>(),
            vec![
                json!([
                    "branch",
                    "feat/my-work-react-viewer",
                    "https://github.com/thedotmack/claude-mem/tree/feat/my-work-react-viewer"
                ]),
                json!([
                    "commit",
                    "af4ba74 fix(work-view): English asks need an action verb before they become requests",
                    "https://github.com/thedotmack/claude-mem/commit/af4ba749c"
                ]),
                json!([
                    "commit",
                    "bc8cd09 feat(work-view): progress readings, measured ETA, goal links and the build story",
                    "https://github.com/thedotmack/claude-mem/commit/bc8cd096b"
                ])
            ]
        );
        let later = bash_at(
            "git commit -m y",
            json!({ "stdout": "[feat/my-work-react-viewer 20c6a29] fix(agent-runner): wait for each agent CLI instead of timing out" }),
            30,
            "12",
        );
        assert_eq!(
            merge(vec![pushed, later], 60)
                .iter()
                .find(|product| product.kind == "commit")
                .unwrap()
                .url,
            None
        );
    }

    /// #652: a commit takes a push's repository only from a push recorded in its own checkout, by
    /// its branch or by the heads the push names, whatever its branch.
    #[test]
    fn a_commit_takes_a_pushs_repository_only_from_its_own_checkout() {
        let at = |list: Vec<Product>, checkout: &str| -> Vec<Product> {
            (list.into_iter())
                .map(|p| Product {
                    checkout: Some(checkout.to_owned()),
                    ..p
                })
                .collect()
        };
        let commit = at(
            bash_at(
                "git commit -m x",
                json!({"stdout": "[main abc1234] Fix A"}),
                10,
                "1",
            ),
            "a",
        );
        let push = |checkout: &str, command: &str, out: &str| {
            at(
                bash_at(command, json!({ "stdout": out }), 20, "2"),
                checkout,
            )
        };
        let url = |lists: Vec<Vec<Product>>| {
            (merge(lists, 60).into_iter())
                .find(|p| p.kind == "commit")
                .and_then(|p| p.url)
        };
        let main = |repo: &str| {
            format!("To https://github.com/o/{repo}.git\n   0000000..def5678  main -> main")
        };
        let b = push("b", "git push origin main", &main("b"));
        assert_eq!(
            url(vec![commit.clone(), b]),
            None,
            "another checkout's push of main"
        );
        let a = push("a", "git push origin main", &main("a"));
        assert_eq!(
            url(vec![commit.clone(), a]).as_deref(),
            Some("https://github.com/o/a/commit/abc1234")
        );
        let heads = |checkout: &str| {
            let out = "To https://github.com/o/c.git\n   0000000..abc1234  HEAD -> release";
            push(checkout, "git push origin HEAD:release", out)
        };
        assert_eq!(
            url(vec![commit.clone(), heads("b")]),
            None,
            "its sha, pushed from another checkout"
        );
        assert_eq!(
            url(vec![commit, heads("a")]).as_deref(),
            Some("https://github.com/o/c/commit/abc1234")
        );
    }

    #[test]
    fn safe_web_urls_have_a_host_and_no_credentials() {
        for url in [
            "https://example.com",
            " http://example.com/report.pdf ",
            "HTTPS://EXAMPLE.COM:443/a",
            "https://[::1]:8080/a",
        ] {
            assert_eq!(safe_web_url(url).as_deref(), Some(url.trim()));
        }
        for url in [
            "javascript:alert(1)",
            "file:///w/report.pdf",
            "https://",
            "https:///path",
            "https://:80/x",
            "https://u:p@example.com/x",
            "https://@example.com",
            "https://a b/x",
            "https://host:wrong/x",
            "https://host:65536/x",
            "https://[wrong]/",
        ] {
            assert_eq!(safe_web_url(url), None, "{url}");
        }
    }

    #[test]
    fn url_kinds_follow_github_deployments_and_file_extensions() {
        for (url, expected) in [
            ("https://GITHUB.com/o/r/pull/42", "pull-request"),
            ("https://preview.vercel.app/hero.png", "deployment"),
            ("https://preview.netlify.app/", "deployment"),
            ("https://preview.pages.dev/", "deployment"),
            (
                "https://example.com/out/REPORT.PDF?download=1#page",
                "document",
            ),
            ("https://example.com/deck.pptx", "deck"),
            ("https://example.com/report", "link"),
            ("https://vercel.app/", "link"),
        ] {
            assert_eq!(kind_for_url(url, "link"), expected, "{url}");
        }
        for (extension, kind) in FILE_KINDS {
            assert_eq!(file_kind(&format!("/w/report{extension}")), Some(*kind));
        }
        for path in ["/w/app.rs", "/w/data.json", "/w/.png"] {
            assert_eq!(file_kind(path), None);
        }
    }

    #[test]
    fn publishers_take_the_first_public_link_and_apply_the_title_rule() {
        for (tool, kind) in [
            ("mcp__notion-create-pages", "page"),
            ("mcp__Claude_Docs__batch", "document"),
            ("mcp__Claude_Docs__create", "document"),
            ("mcp__chatgpt_space__create_page", "page"),
            ("mcp__sites__save_version_and_deploy_extra", "deployment"),
        ] {
            let products = from_tool_use(
                tool,
                &json!({ "title": " Input   title " }),
                &json!({
                    "apiUrl": "https://example.com/api/page", "nested": [{ "url": "https://example.com/oauth/token" },
                        { "url": "https://example.com/first" }, { "url": "https://example.com/second" }],
                    "title": " Response\n title "
                }),
                None,
                None,
                false,
            );
            assert_eq!(products.len(), 1, "{tool}");
            assert_eq!(products[0].kind, kind, "{tool}");
            assert_eq!(products[0].title, "Response title");
            assert_eq!(
                products[0].url.as_deref(),
                Some("https://example.com/first")
            );
        }
        for (input, response, title) in [
            (
                json!({ "name": "Named" }),
                json!({ "url": "https://example.com/x" }),
                "Named",
            ),
            (
                json!({ "file_path": "/w/deck.html" }),
                json!({ "url": "https://example.com/x" }),
                "deck.html",
            ),
            (
                json!({ "title": "Ignored", "file_path": "/w/deck.html" }),
                json!({ "title": "", "url": "https://example.com/x" }),
                "deck.html",
            ),
            (
                json!({}),
                json!({ "url": "https://EXAMPLE.com:443/x" }),
                "example.com",
            ),
            (
                json!({ "action": null, "asset": false }),
                json!("https://example.com/x"),
                "example.com",
            ),
        ] {
            assert_eq!(
                from_tool_use(
                    "Artifact",
                    &input,
                    &response.to_string().into(),
                    None,
                    None,
                    false
                )[0]
                .title,
                title
            );
        }
        assert!(
            from_tool_use(
                "Artifact",
                &json!({}),
                &json!({ "text": "https://example.com/ignored" }),
                None,
                None,
                false
            )
            .is_empty()
        );
        assert!(
            from_tool_use(
                "Artifact",
                &json!({}),
                &json!("not JSON"),
                None,
                None,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn work_product_fields_keep_overrides_aliases_and_indices_across_fields() {
        let fields = json!({
            "status": "done",
            "deliverable": "{\"link\":\"https://example.com/report\",\"name\":\"A report\",\"kind\":\"custom\"}",
            "deliverables": [{ "path": "/w/result.txt", "kind": "text" }, { "path": "relative.pdf" }],
            "work_products": [{ "url": "/w/result.csv", "title": "Results" }, { "url": "https://example.com/page.html", "path": "/w/other.pdf" }],
            "custom_url": "https://example.com/deck.key", "ignored_url": { "url": "https://example.com/ignored" },
            "Work_Product": "https://example.com/ignored"
        });
        let products = from_fields(
            fields.as_object().unwrap(),
            None,
            Some(source("work-state", "2")),
        );
        assert_eq!(
            products
                .iter()
                .map(|product| (
                    product.kind.as_str(),
                    product.title.as_str(),
                    product.source.as_ref().unwrap().index
                ))
                .collect::<Vec<_>>(),
            vec![
                ("custom", "A report", Some(0)),
                ("text", "result.txt", Some(1)),
                ("spreadsheet", "Results", Some(3)),
                ("page", "example.com", Some(4)),
                ("deck", "example.com", Some(5))
            ]
        );
        for (field, kind) in LEGACY_KINDS {
            let fields = Map::from_iter([(field.to_string(), json!("https://example.com/report"))]);
            assert_eq!(from_fields(&fields, None, None)[0].kind, *kind);
        }
        assert!(is_work_product_field("custom_url"));
        assert!(!is_work_product_field("custom_URL"));
        assert!(
            from_fields(
                json!({ "work_product": "[broken" }).as_object().unwrap(),
                None,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn bash_responses_may_be_objects_json_strings_or_raw_text() {
        for response in [
            json!({ "stdout": "ok abc1234" }),
            json!("ok abc1234"),
            json!(json!({ "stderr": "ok abc1234" }).to_string()),
            json!(json!("ok abc1234").to_string()),
        ] {
            let products = from_tool_use(
                "Bash",
                &json!({ "command": "rtk git commit -m 'subject'" }),
                &response,
                None,
                None,
                false,
            );
            assert_eq!(products[0].title, "abc1234 subject", "{response}");
        }
        assert_eq!(
            output_text(&Value::Null, &json!({ "stdout": "a", "stderr": "b" })),
            "a\nb"
        );
        assert_eq!(
            output_text(&json!("raw"), &json!({ "stdout": 3, "stderr": null })),
            ""
        );
    }

    #[test]
    fn commit_subjects_unescape_inline_messages_and_take_only_the_first_line() {
        assert_eq!(
            commit_subjects(
                "git -c user.name='A' -C \"/w/a b\" commit -am \"first \\\"quoted\\\"\nbody\" && rtk git commit -m 'second\\ subject'"
            ),
            vec![
                Some("first \"quoted\"".to_owned()),
                Some("second subject".to_owned())
            ]
        );
        assert_eq!(
            commit_subjects("git commit -F - <<-\"MSG\"\n subject  line\nbody\nMSG"),
            vec![Some("subject line".to_owned())]
        );
        assert_eq!(commit_subjects("git commit -F file"), vec![None]);
        for separator in ["\n", "\r", "\u{2028}", "\u{2029}"] {
            assert_eq!(
                commit_subjects(&format!("git commit -m \"a\\{separator}b\"")),
                vec![None]
            );
        }
    }

    #[test]
    fn recorded_commits_keep_the_longest_sha_and_the_printed_subject() {
        let response =
            json!({ "gitOperation": { "commit": { "sha": "abc1234567890", "branch": "main" } } });
        let products = from_command(
            "git commit -m 'subject'",
            &response,
            "[main (root-commit) abc1234] subject",
            Some(2),
            None,
            false,
        );
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].sha.as_deref(), Some("abc1234567890"));
        assert_eq!(products[0].branch.as_deref(), Some("main"));
        assert_eq!(products[0].title, "abc1234 subject");
        let products = from_command("git status", &response, "", None, None, false);
        assert_eq!(products[0].title, "abc1234");
        assert!(
            from_command(
                "git status",
                &json!({ "gitOperation": { "commit": { "sha": "bad" } } }),
                "",
                None,
                None,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn pushes_preserve_ref_order_and_match_heads_across_branches() {
        let products = from_command(
            "git commit -m 'subject'; git push https://github.com/o/r.git HEAD:main",
            &json!({}),
            "[other abc1234] subject\nTo https://github.com/o/r.git\n + 1111111...abc123456  HEAD -> refs/heads/main\n * [new branch] feat/x -> feat/x",
            None,
            None,
            false,
        );
        assert_eq!(products.len(), 3);
        assert_eq!(products[0].repo.as_deref(), Some("o/r"));
        assert_eq!(products[0].branch.as_deref(), Some("other"));
        assert_eq!(products[1].branch.as_deref(), Some("main"));
        assert_eq!(products[1].heads, vec!["abc123456"]);
        assert_eq!(products[2].branch.as_deref(), Some("feat/x"));
        let products = from_command(
            "git checkout -b 'feat/x' && git push https://github.com/o/r.git feat/x",
            &json!({}),
            "",
            None,
            None,
            false,
        );
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].repo.as_deref(), Some("o/r"));
    }

    #[test]
    fn merging_is_stable_for_equal_times_and_applies_its_limit_after_deduplication() {
        let lists = vec![vec![
            Product {
                title: "first".to_owned(),
                path: Some("/a.png".to_owned()),
                ..Product::default()
            },
            Product {
                title: "duplicate".to_owned(),
                path: Some("/a.png".to_owned()),
                at: Some(0),
                ..Product::default()
            },
            Product {
                title: "second".to_owned(),
                path: Some("/b.png".to_owned()),
                ..Product::default()
            },
            Product {
                title: "third".to_owned(),
                path: Some("/c.png".to_owned()),
                at: Some(-1),
                ..Product::default()
            },
        ]];
        assert!(merge(lists.clone(), 0).is_empty());
        assert_eq!(
            merge(lists.clone(), 2)
                .iter()
                .map(|product| product.title.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(merge(lists, 60).len(), 3);
    }

    #[test]
    fn titles_collapse_whitespace_and_use_the_javascript_utf16_limit() {
        assert_eq!(short(" \n A\t title \r "), Some("A title".to_owned()));
        assert_eq!(short(" \n\t"), None);
        assert_eq!(short(&"a".repeat(140)), Some("a".repeat(140)));
        assert_eq!(
            short(&"a".repeat(141)),
            Some(format!("{}…", "a".repeat(139)))
        );
        assert_eq!(short(&"😀".repeat(70)), Some("😀".repeat(70)));
        assert_eq!(
            short(&format!("a{}", "😀".repeat(70))),
            Some(format!("a{}…", "😀".repeat(69)))
        );
        assert_eq!(short("\u{feff}title\u{feff}"), Some("title".to_owned()));
        assert_eq!(short("a\u{85}b"), Some("a\u{85}b".to_owned()));
        assert_eq!(
            short(&format!("{}\u{85}bc", "a".repeat(138))),
            Some(format!("{}\u{85}…", "a".repeat(138)))
        );
        assert_eq!(
            short(&"😀".repeat(71)),
            Some(format!("{}\u{fffd}…", "😀".repeat(69)))
        );
    }

    /// Codex on #650: a failed call made only what its output confirms. A branch its command
    /// names is none, and a failed publishing tool's answer links nothing; a commit its output
    /// printed stands although the push after it failed.
    #[test]
    fn a_failed_call_makes_only_what_its_output_confirms() {
        let checkout = |failed| {
            from_command(
                "git checkout -b release",
                &Value::Null,
                "Permission denied",
                None,
                None,
                failed,
            )
        };
        assert_eq!((checkout(false).len(), checkout(true).len()), (1, 0));
        let out = "[main abc1234] Fix the parser\n 1 file changed\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs";
        let products = from_command(
            "git commit -m 'Fix the parser' && git push",
            &Value::Null,
            out,
            None,
            None,
            true,
        );
        let titles: Vec<&str> = products.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["abc1234 Fix the parser"]);
        let push = |out, failed| {
            from_command(
                "git push origin feat/x",
                &Value::Null,
                out,
                None,
                None,
                failed,
            )
        };
        assert_eq!((push("", false).len(), push("", true).len()), (1, 0));
        let named = push(" * [new branch]      feat/x -> feat/x", true);
        assert_eq!(
            named.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            ["feat/x"]
        );
        let answer: Value = json!({"url": "https://example.com/artifacts/abc"})
            .to_string()
            .into();
        let artifact = |failed| from_tool_use("Artifact", &json!({}), &answer, None, None, failed);
        assert_eq!((artifact(false).len(), artifact(true).len()), (1, 0));
    }
