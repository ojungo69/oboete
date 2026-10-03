//! Cards (docs/cards.md): what a window of work was, as claude-mem's observation shapes it (a
//! type, a title, a subtitle, a narrative, facts, concepts, files). `consumer::cards` keeps them
//! in knowledge.db from the window ops; every reader takes them through this module (K7).

use crate::raw::{Raw, Removal};
use crate::redact::Rules;
use anyhow::Result;
use rusqlite::Connection;

/// One card as a reader gets it: current (K3), over no record removed since (K4), gated (K6).
// Its first reader outside the tests is session start (docs/cards.md, slice 3).
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub device: String,
    pub op_seq: i64,
    pub n: i64,
    /// The time of its window's last record, unix ms.
    pub ts: i64,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub repo: Option<String>,
    /// One of claude-mem's observation types; none for a card made of a window's summary (K1).
    pub kind: Option<String>,
    pub title: String,
    pub subtitle: String,
    pub narrative: String,
    pub facts: Vec<String>,
    pub concepts: Vec<String>,
    pub files_read: Vec<String>,
    pub files_modified: Vec<String>,
}

pub(crate) fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "-- One row per card of a curated window op; the lists are JSON arrays of strings.
         CREATE TABLE IF NOT EXISTS cards(
           device TEXT NOT NULL, op_seq INTEGER NOT NULL, n INTEGER NOT NULL,
           from_seq INTEGER NOT NULL, from_offset INTEGER,
           to_seq INTEGER NOT NULL, to_offset INTEGER,
           -- The records its curator was shown besides the window's own (K4): a JSON array of
           -- seqs. And what was removed from all of them before the window was cut, as its op
           -- lists it: a JSON array of [seq, offset, length].
           goals TEXT NOT NULL DEFAULT '[]',
           removed TEXT NOT NULL DEFAULT '[]',
           ts INTEGER NOT NULL,
           agent TEXT, session TEXT, repo TEXT,
           -- No title: it is the narrative's first sentence, cut when the card is read (K1).
           type TEXT, title TEXT NOT NULL DEFAULT '', subtitle TEXT NOT NULL DEFAULT '',
           narrative TEXT NOT NULL DEFAULT '',
           facts TEXT NOT NULL DEFAULT '[]', concepts TEXT NOT NULL DEFAULT '[]',
           files_read TEXT NOT NULL DEFAULT '[]', files_modified TEXT NOT NULL DEFAULT '[]',
           -- The recuration op that curated its records again (K3).
           replaced_by INTEGER,
           PRIMARY KEY (device, op_seq, n)
         );
         CREATE INDEX IF NOT EXISTS cards_repo ON cards(repo, ts);",
    )?;
    Ok(())
}

/// A title made of a narrative's first sentence, at most (K1).
const TITLE: usize = 120;

/// A text's first sentence, within `TITLE` characters and ending in `…` when it was cut: a line's
/// end, a Japanese full stop, or a `.`, `!` or `?` that ends a word ("v1.2" has none).
pub(crate) fn first_sentence(text: &str) -> String {
    let end = text
        .char_indices()
        .find(|&(i, c)| match c {
            '。' | '！' | '？' | '\n' => true,
            '.' | '!' | '?' => text[i + 1..].chars().next().is_none_or(char::is_whitespace),
            _ => false,
        })
        .map_or(
            text.len(),
            |(i, c)| {
                if c == '\n' { i } else { i + c.len_utf8() }
            },
        );
    let sentence = &text[..end];
    if sentence.chars().count() <= TITLE {
        return sentence.to_owned();
    }
    let cut: String = sentence.chars().take(TITLE - 1).collect();
    format!("{}…", cut.trim_end())
}

/// `repo`'s newest cards, at most `limit`, the newest first.
#[cfg_attr(not(test), allow(dead_code))]
pub fn recent(
    k: &Connection,
    raw: &Raw,
    repo: &str,
    limit: usize,
    rules: &Rules,
) -> Result<Vec<Card>> {
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok(Vec::new());
    }
    let mut st = k.prepare(
        "SELECT device, op_seq, n, ts, agent, session, repo, type, title, subtitle, narrative,
                facts, concepts, files_read, files_modified, from_seq, to_seq, removed, goals
         FROM cards WHERE repo = ?1 AND replaced_by IS NULL
         ORDER BY ts DESC, device DESC, op_seq DESC, n",
    )?;
    // K6: every text as the rules are now, the labels it is shown under too.
    let gate = |s: String| crate::redact::outbound_with(&s, rules);
    let list = |r: &rusqlite::Row, i: usize| -> rusqlite::Result<Vec<String>> {
        let items: Vec<String> = serde_json::from_str(&r.get::<_, String>(i)?).unwrap_or_default();
        Ok(items.into_iter().map(gate).collect())
    };
    let mut rows = st.query([repo])?;
    let mut out = Vec::new();
    while out.len() < limit
        && let Some(r) = rows.next()?
    {
        let device: String = r.get(0)?;
        // K4: its text may say what a removal its op does not list took from a record its
        // curator was shown.
        let listed: Vec<Removal> =
            serde_json::from_str(&r.get::<_, String>(17)?).unwrap_or_default();
        let goals: Vec<i64> = serde_json::from_str(&r.get::<_, String>(18)?).unwrap_or_default();
        let mut removed = raw.removed_in(&device, r.get(15)?, r.get(16)?, None)?;
        for goal in goals {
            removed.extend(raw.removed_in(&device, goal, goal, None)?);
        }
        if removed.iter().any(|x| !listed.contains(x)) {
            continue;
        }
        let narrative = gate(r.get(10)?);
        // Gated before it is cut: a value the cut would split is whole when the rules read it.
        let title = match r.get::<_, String>(8)? {
            t if t.is_empty() => first_sentence(&narrative),
            t => gate(t),
        };
        out.push(Card {
            device,
            op_seq: r.get(1)?,
            n: r.get(2)?,
            ts: r.get(3)?,
            agent: r.get::<_, Option<String>>(4)?.map(gate),
            session: r.get::<_, Option<String>>(5)?.map(gate),
            repo: r.get::<_, Option<String>>(6)?.map(gate),
            kind: r.get(7)?,
            title,
            subtitle: gate(r.get(9)?),
            narrative,
            facts: list(r, 11)?,
            concepts: list(r, 12)?,
            files_read: list(r, 13)?,
            files_modified: list(r, 14)?,
        });
    }
    Ok(out)
}
