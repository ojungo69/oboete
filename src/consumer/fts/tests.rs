    use super::*;
    use crate::knowledge::checkpoint;
    use crate::search::b::fixture::Store;
    use serde_json::json;

    const R: &str = "github.com/o/r";

    /// The seqs of the records whose `raw_fts` row `word` finds, in order.
    fn finds(k: &Connection, word: &str) -> Vec<i64> {
        k.prepare(
            "SELECT d.seq FROM raw_fts f JOIN raw_docs d ON d.rowid = f.rowid
             WHERE raw_fts MATCH ?1 ORDER BY d.seq",
        )
        .unwrap()
        .query_map([word], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    /// Record `seq`'s `raw_fts` row made of `text`.
    fn indexed_as(k: &Connection, device: &str, seq: i64, text: &str) {
        let rowid: i64 = k
            .query_row(
                "SELECT rowid FROM raw_docs WHERE device = ?1 AND seq = ?2",
                params![device, seq],
                |r| r.get(0),
            )
            .unwrap();
        k.execute("DELETE FROM raw_fts WHERE rowid = ?1", [rowid])
            .unwrap();
        k.execute(
            "INSERT INTO raw_fts(rowid, text) VALUES(?1, ?2)",
            params![rowid, text],
        )
        .unwrap();
    }

    /// #843: the rows an older oboete indexed with a prompt's entry point as a line of its text
    /// are indexed again once, at most `AGAIN` prompts and envelopes a step (tool output between
    /// them read by none), each step a knowledge.db transaction of its own; then the consumer's
    /// checkpoint follows raw's top, as a caught-up consumer's does.
    #[test]
    fn rows_indexed_with_an_entry_point_are_indexed_again_once_in_batches() {
        let mut s = Store::new();
        let device = s.raw.device().to_owned();
        let typed = |words: &str| json!({"prompt": words, "entrypoint": "cli"});
        let mut rows = Vec::new();
        for i in 0..AGAIN {
            let words = format!("Fix parser number {i}.");
            let ts = 1_000 + i as i64;
            rows.push((
                s.event("prompt", "s", (R, "main"), ts, typed(&words)),
                words,
            ));
            s.event("tool", "s", (R, "main"), ts, json!({"tool_name": "Bash"}));
        }
        let words = "<task-notification>Build passed.</task-notification>".to_owned();
        rows.push((
            s.event("envelope", "s", (R, "main"), 3_000, typed(&words)),
            words,
        ));
        let says = s.event(
            "prompt",
            "s",
            (R, "main"),
            4_000,
            typed("Run the cli tests."),
        );
        s.run();
        let mut k = crate::knowledge::open(s.home.path()).unwrap();
        assert_eq!(finds(&k, "cli"), [says]);
        // The index as an older oboete left it: the entry point a line of each prompt's and
        // envelope's text, and no catch-up begun.
        for (seq, words) in &rows {
            indexed_as(&k, &device, *seq, &format!("{words}\ncli"));
        }
        checkpoint::set_in(&k, checkpoint::SEQS, "fts-text", &device, 0).unwrap();
        assert_eq!(finds(&k, "cli").len(), AGAIN + 2);
        // Each row indexed again is found by its words (Sourcery on #846: not only gone).
        let prompts: Vec<i64> = rows[..AGAIN].iter().map(|(seq, _)| *seq).collect();
        let mut again = crate::worker::consumers(s.home.path())
            .into_iter()
            .find(|c| c.name() == "fts-text")
            .expect("the consumer that indexes the rows again");
        let tx = k
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let first = again.step(&s.raw, &tx, &device, 0).unwrap();
        tx.commit().unwrap();
        assert_eq!(first, rows[AGAIN - 1].0);
        assert_eq!(finds(&k, "cli"), [rows[AGAIN].0, says]);
        assert_eq!(finds(&k, "parser"), prompts);
        // A drain goes on from there and finishes it: the checkpoint is raw's top.
        checkpoint::set_in(&k, checkpoint::SEQS, "fts-text", &device, first).unwrap();
        s.run();
        assert_eq!(finds(&k, "cli"), [says]);
        assert_eq!(finds(&k, "parser"), prompts);
        assert_eq!(finds(&k, "passed"), [rows[AGAIN].0]);
        let top = s.raw.max_seq_of(&device).unwrap();
        assert_eq!(checkpoint::get(&k, "fts-text", &device).unwrap(), top);
        // Once: a row made as before after the catch-up stays as it is through the next drain.
        indexed_as(&k, &device, rows[0].0, &format!("{}\ncli", rows[0].1));
        s.run();
        assert_eq!(finds(&k, "cli"), [rows[0].0, says]);
    }

    /// X17 (docs/subagents.md): a subagent's stop is a mark with no words: no `raw_docs` row, so
    /// no search hit, vector or snippet, as the owner's directive has none.
    #[test]
    fn a_subagent_stop_is_no_row_of_the_index() {
        let mut s = Store::new();
        let body = json!({"agent_id": "a1", "agent_type": "Explore"});
        let seq = s.event("subagent_stop", "s", (R, "main"), 1_000, body);
        s.event(
            "prompt",
            "s",
            (R, "main"),
            2_000,
            json!({"prompt": "Explore the parser."}),
        );
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        let rows: i64 = k
            .query_row("SELECT count(*) FROM raw_docs WHERE seq = ?1", [seq], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0);
        assert_eq!(finds(&k, "Explore"), [seq + 1]);
    }

    /// Sourcery on #846: an older oboete run after the catch-up indexes new prompts with their entry
    /// point and leaves this consumer's checkpoint where it was, so this one indexes them again
    /// when it runs again.
    #[test]
    fn rows_an_older_oboete_indexes_after_the_catch_up_are_indexed_again() {
        let mut s = Store::new();
        let device = s.raw.device().to_owned();
        let typed = |words: &str| json!({"prompt": words, "entrypoint": "cli"});
        s.event("prompt", "s", (R, "main"), 1_000, typed("Fix the parser."));
        s.run();
        let later = s.event("prompt", "s", (R, "main"), 2_000, typed("Ship the lexer."));
        // The older oboete: its consumers without this one, its text with the entry point.
        let mut k = crate::knowledge::open(s.home.path()).unwrap();
        crate::worker::drain(&s.raw, &mut k, &mut [Box::new(Fts) as Box<dyn Consumer>]).unwrap();
        indexed_as(&k, &device, later, "Ship the lexer.\ncli");
        assert_eq!(finds(&k, "cli"), [later]);
        s.run();
        assert_eq!(finds(&k, "cli"), Vec::<i64>::new());
        assert_eq!(finds(&k, "lexer"), [later]);
    }

    /// Sourcery on #846: a copied home's records left under its old id (`Raw::devices`) were
    /// indexed before the copy as this device's were, and are indexed again too.
    #[test]
    fn a_copied_homes_rows_under_its_old_id_are_indexed_again() {
        let mut s = Store::new();
        let old = s.raw.device().to_owned();
        let body = json!({"prompt": "Fix the parser.", "entrypoint": "cli"});
        let seq = s.event("prompt", "s", (R, "main"), 1_000, body);
        s.run();
        let k = crate::knowledge::open(s.home.path()).unwrap();
        indexed_as(&k, &old, seq, "Fix the parser.\ncli");
        checkpoint::set_in(&k, checkpoint::SEQS, "fts-text", &old, 0).unwrap();
        // The copy's raw.db under a new id, as a copied file gets one (`db::ensure_device`).
        rusqlite::Connection::open(s.home.path().join("raw.db"))
            .unwrap()
            .execute("UPDATE meta SET value = 'copy' WHERE key = 'device_id'", [])
            .unwrap();
        s.run();
        assert_eq!(finds(&k, "cli"), Vec::<i64>::new());
        assert_eq!(finds(&k, "parser"), [seq]);
    }

    /// #843: where a Claude Code prompt was typed or sent from (`entrypoint`, #659) is a mark on the
    /// record, as `curate::long_text` reads it (#841), not a word of its text: neither indexed nor
    /// embedded. A field the renderer does not know is a word as before, and so is an `entrypoint`
    /// inside a value.
    #[test]
    fn a_prompts_entry_point_is_no_word_of_its_text() {
        for entry in ["cli", "claude-vscode", "sdk-cli", "remote_trigger"] {
            let of = |body: serde_json::Value| text(&body.to_string());
            assert_eq!(
                of(json!({"prompt": "Fix the parser.", "entrypoint": entry})),
                "Fix the parser."
            );
            assert_eq!(
                of(json!({"prompt": "Ship it.", "agent_sent": true, "entrypoint": entry})),
                "Ship it."
            );
            assert_eq!(of(json!({"omitted": true, "entrypoint": entry})), "");
        }
        assert_eq!(
            text(
                &json!({"prompt": "Fix the parser.", "entrypoint": "cli", "note": "kept"})
                    .to_string()
            ),
            "Fix the parser.\nkept"
        );
        assert_eq!(
            text(&json!({"tool_input": {"entrypoint": "main.rs"}}).to_string()),
            "main.rs"
        );
    }
