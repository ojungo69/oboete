//! Session-start context: the latest summaries plus the newest observations for this repository.

use anyhow::Result;
use rusqlite::{Connection, params};

const MAX_SUMMARIES: usize = 3;
const MAX_OBSERVATIONS: usize = 12;
const MAX_CHARS: usize = 4_000;

pub fn context(conn: &Connection, repo: &str) -> Result<String> {
    let mut out = String::new();
    let mut stmt =
        conn.prepare("SELECT body FROM summaries WHERE repo=?1 ORDER BY ts DESC LIMIT ?2")?;
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
            out.push_str(&truncate(s, 600));
            out.push('\n');
        }
    }
    if !observations.is_empty() {
        out.push_str("\n## Recent observations\n");
        for (kind, title, body) in &observations {
            out.push_str(&format!("- [{kind}] {title}: {}\n", truncate(body, 240)));
        }
    }
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    Ok(out)
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
