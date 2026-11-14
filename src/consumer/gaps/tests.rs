    use super::*;
    use crate::{knowledge, raw, worker};
    use serde_json::json;

    /// A Claude Code transcript: one typed prompt per text, each answered.
    fn transcript(path: &Path, prompts: &[&str]) {
        let mut lines = String::new();
        for (i, p) in prompts.iter().enumerate() {
            let ts = |s: usize| format!("2026-09-01T00:00:{:02}.000Z", 2 * i + s);
            lines.push_str(
                &json!({"type": "user", "sessionId": "s1", "cwd": "/w", "timestamp": ts(0),
                        "message": {"role": "user", "content": p}})
                .to_string(),
            );
            lines.push('\n');
            lines.push_str(
                &json!({"type": "assistant", "sessionId": "s1", "cwd": "/w", "timestamp": ts(1),
                        "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}})
                .to_string(),
            );
            lines.push('\n');
        }
        std::fs::write(path, lines).unwrap();
    }

    fn end(agent: &str, session: &str, transcript: Option<&Path>) -> raw::Event {
        let mut payload = json!({"session_id": session, "reason": "exit"});
        if let Some(t) = transcript {
            payload["transcript_path"] = json!(t);
        }
        let mut c = crate::capture::events(agent, "SessionEnd", &payload, 9, &Settings::default());
        c.remove(0).event
    }

    fn prompt(session: &str, text: &str) -> raw::Event {
        raw::Event {
            session: session.into(),
            body: json!({"prompt": text}).to_string(),
            ..raw::test_event("")
        }
    }

    #[test]
    fn a_lost_end_leaves_the_session_checked_at_its_older_one() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("s1.jsonl");
        transcript(&file, &["one", "two"]);
        let mut store = raw::open(home.path()).unwrap();
        store.append(&prompt("s1", "one")).unwrap();
        store.append(&end("claude", "s1", Some(&file))).unwrap();
        worker::run_once(home.path()).unwrap();
        // Resumed, and ended again; then that end is lost (MUST-M14).
        store.append(&end("claude", "s1", Some(&file))).unwrap();
        worker::run_once(home.path()).unwrap();
        let top = store.max_seq().unwrap();
        rusqlite::Connection::open(home.path().join("raw.db"))
            .unwrap()
            .execute("DELETE FROM records WHERE seq = ?1", [top])
            .unwrap();
        worker::run_once(home.path()).unwrap();
        let k = knowledge::open(home.path()).unwrap();
        assert_eq!(
            doctor(&k),
            vec![
                "claude: 1 of 1 ended session(s) short of their transcript (1 turn(s) not recorded)"
            ]
        );
    }

    /// Milestone 4 D6: an imported `end` record (v1's, a transcript's) parses no transcript and
    /// writes no row: raw holds only what was imported of that session.
    #[test]
    fn an_imported_end_record_parses_no_transcript() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("s1.jsonl");
        transcript(&file, &["one"]);
        let mut store = raw::open(home.path()).unwrap();
        for (session, source) in [("s1", "oboete-v1"), ("s2", "transcript"), ("s3", "hook")] {
            let e = raw::Event {
                source: source.into(),
                ..end("claude", session, Some(&file))
            };
            store.append(&e).unwrap();
        }
        worker::run_once(home.path()).unwrap();
        let k = knowledge::open(home.path()).unwrap();
        let checked: Vec<String> = k
            .prepare("SELECT session FROM gaps")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(checked, ["s3"]);
    }

    #[test]
    fn a_session_short_of_its_transcript_is_reported_for_its_agent() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("s1.jsonl");
        // Five typed prompts, and one that was all `<private>`: no turn on either side.
        transcript(
            &file,
            &[
                "one",
                "two",
                "three",
                "<private>x</private>",
                "four",
                "five",
            ],
        );
        let mut store = raw::open(home.path()).unwrap();
        for text in ["one", "two", "three", "five"] {
            store.append(&prompt("s1", text)).unwrap();
        }
        store.append(&end("claude", "s1", Some(&file))).unwrap();
        // Codex's session sent no transcript path: not checked, never a gap.
        store.append(&end("codex", "c1", None)).unwrap();
        // A transcript with a line that is not JSON: not checked either.
        let torn = home.path().join("s2.jsonl");
        transcript(&torn, &["one"]);
        let mut text = std::fs::read_to_string(&torn).unwrap();
        text.push_str("{\"type\": \"user\", \"message\n");
        std::fs::write(&torn, text).unwrap();
        store.append(&end("claude", "s2", Some(&torn))).unwrap();
        // One with a record type no parser knows (it may hold a prompt): not checked.
        let new = home.path().join("s3.jsonl");
        transcript(&new, &["one"]);
        let mut text = std::fs::read_to_string(&new).unwrap();
        text.push_str("{\"type\": \"typed-request\", \"sessionId\": \"s1\"}\n");
        std::fs::write(&new, text).unwrap();
        store.append(&end("claude", "s3", Some(&new))).unwrap();
        worker::run_once(home.path()).unwrap();
        let k = knowledge::open(home.path()).unwrap();
        let row: (Option<i64>, i64) = k
            .query_row(
                "SELECT transcript_turns, raw_turns FROM gaps WHERE session = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (Some(5), 4));
        assert_eq!(
            doctor(&k),
            vec![
                "claude: 1 of 1 ended session(s) short of their transcript (1 turn(s) not recorded); 2 not checked (no transcript oboete could read)",
                "codex: 1 ended session(s), not checked (no transcript oboete could read)",
            ]
        );
    }
