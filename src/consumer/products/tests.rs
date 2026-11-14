    use super::*;
    use serde_json::json;

    fn tool(raw: &mut Raw, ts: i64, name: &str, input: serde_json::Value, source: &str) -> i64 {
        let body = json!({"tool": name, "input": input.to_string(), "output": "", "failed": false});
        let e = crate::raw::Event {
            kind: "tool".into(),
            ts,
            source: source.into(),
            ..crate::raw::test_event(&body.to_string())
        };
        raw.append(&e).unwrap()
    }

    /// Slice 4: the consumer keeps where a live call that may have made something is, as
    /// claude-mem picks the calls it reads, and nothing else; a rewind takes back what it kept past
    /// its point.
    #[test]
    fn product_calls_are_the_live_calls_that_may_make_something() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = crate::raw::open(home.path()).unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let commit = tool(
            &mut raw,
            1,
            "Bash",
            json!({"command": "git commit -m 'Add'"}),
            "hook",
        );
        tool(&mut raw, 2, "Bash", json!({"command": "ls -la"}), "hook");
        tool(
            &mut raw,
            3,
            "Read",
            json!({"file_path": "/tmp/git push"}),
            "hook",
        );
        tool(
            &mut raw,
            4,
            "Bash",
            json!({"command": "git push"}),
            "transcript",
        );
        let pr = tool(
            &mut raw,
            5,
            "Bash",
            json!({"command": "gh pr create --fill"}),
            "hook",
        );
        let artifact = tool(
            &mut raw,
            6,
            "Artifact",
            json!({"file_path": "/tmp/a.html"}),
            "hook",
        );
        let device = raw.device().to_owned();
        let mut c = Products;
        assert_eq!(c.step(&raw, &k, &device, 0).unwrap(), artifact);
        let kept = || -> Vec<i64> {
            (k.prepare("SELECT seq FROM product_calls ORDER BY seq")
                .unwrap())
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
        };
        assert_eq!(kept(), [commit, pr, artifact]);
        c.rewind(&k, &device, commit).unwrap();
        assert_eq!(kept(), [commit]);
    }
