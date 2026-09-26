//! Session-start context: the latest summaries plus the newest observations for this repository.

use anyhow::Result;
use rusqlite::{Connection, params};

const MAX_SUMMARIES: usize = 3;
const MAX_OBSERVATIONS: usize = 12;
const MAX_CHARS: usize = 4_000;

pub fn context(conn: &Connection, repo: &str) -> Result<String> {
    let mut out = String::new();
    // One row per session: its newest summary, which covers its earlier parts (observe).
    let mut stmt = conn.prepare(
        "SELECT body FROM summaries m WHERE repo=?1 AND id = (SELECT id FROM summaries
           WHERE session_id = m.session_id ORDER BY ts DESC, id DESC LIMIT 1)
         ORDER BY ts DESC LIMIT ?2",
    )?;
    let summaries: Vec<String> = stmt
        .query_map(params![repo, MAX_SUMMARIES as i64], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut stmt = conn.prepare(
        "SELECT kind, title, body FROM observations WHERE repo=?1 ORDER BY ts DESC LIMIT ?2",
    )?;
    let observations: Vec<(String, String, String)> = stmt
        .query_map(params![repo, MAX_OBSERVATIONS as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    if summaries.is_empty() && observations.is_empty() {
        return Ok(out);
    }
    out.push_str("# oboete: what happened before in this repository\n");
    if !summaries.is_empty() {
        out.push_str("\n## Recent sessions (newest first)\n");
        for s in &summaries {
            out.push_str("- ");
            out.push_str(&truncate(&gate(s), 600));
            out.push('\n');
        }
    }
    if !observations.is_empty() {
        out.push_str("\n## Recent observations\n");
        for (kind, title, body) in &observations {
            out.push_str(&format!(
                "- [{kind}] {}: {}\n",
                gate(title),
                truncate(&gate(body), 240)
            ));
        }
    }
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    Ok(out)
}

/// Stored text passes the egress gate with today's rules: a rule added after capture still keeps
/// its matches out of the agent's context (spec 6.4).
fn gate(s: &str) -> String {
    crate::redact::outbound(s)
}

/// One line, at most `max` characters.
fn truncate(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn a_session_in_many_parts_takes_one_line() {
        let dir = std::env::temp_dir().join(format!("oboete-inject-parts-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = db::open(&dir).unwrap();
        let batch = |conn: &mut Connection, id: &str, ts: i64, body: &str| {
            let s = db::PendingSession {
                id: id.into(),
                agent: "claude".into(),
                repo: "/r".into(),
                last_event_at: ts,
            };
            assert!(db::apply_batch(conn, &s, "p", body, &[], 0).unwrap());
        };
        for id in ["old1", "old2", "long"] {
            db::upsert_session(&conn, id, "claude", "/r", "/r", 1).unwrap();
        }
        batch(&mut conn, "old1", 10, "OLD-ONE");
        batch(&mut conn, "old2", 20, "OLD-TWO");
        for (i, body) in ["PART-1", "PART-2", "PART-3", "WHOLE-SESSION"]
            .iter()
            .enumerate()
        {
            batch(&mut conn, "long", 30 + i as i64, body);
        }
        let text = context(&conn, "/r").unwrap();
        for kept in ["WHOLE-SESSION", "OLD-TWO", "OLD-ONE"] {
            assert!(text.contains(kept), "{kept}: {text}");
        }
        assert!(!text.contains("PART-"), "{text}");
        // Stored before a rule matched it, masked on the way out.
        let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // split: scanners
        batch(&mut conn, "old1", 40, &format!("token {key} here"));
        assert!(!context(&conn, "/r").unwrap().contains(&key));
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }
}
