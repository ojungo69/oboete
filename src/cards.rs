//! Cards (docs/cards.md): what a window of work was, as claude-mem's observation shapes it (a
//! type, a title, a subtitle, a narrative, facts, concepts, files). `consumer::cards` keeps them
//! in knowledge.db from the window ops; every reader takes them through this module (K7).

use crate::raw::{Raw, Removal};
use crate::redact::Rules;
use anyhow::Result;
use rusqlite::Connection;

/// One card as a reader gets it: current (K3), over no record removed since (K4), gated (K6).
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

/// claude-mem's observation types, in its order (plugin/modes/code.json).
pub const TYPES: &[&str] = &[
    "bugfix",
    "feature",
    "refactor",
    "change",
    "discovery",
    "decision",
    "security_alert",
    "security_note",
    "sensitive",
];

/// claude-mem's icon of each of `TYPES`, in its order.
const ICONS: [&str; 9] = ["●", "◆", "↻", "✓", "○", "⚖", "⚠", "⚷", "⊘"];
/// claude-mem's icon for a type it does not know: here a card without one (K1).
const NO_TYPE: &str = "📝";

impl Card {
    /// Its ID as session start shows it and `get` reads it (docs/cards.md S3).
    pub fn id(&self) -> String {
        format!("{}.{}", self.op_seq, self.n)
    }
}

/// claude-mem's observation concepts.
pub const CONCEPTS: &[&str] = &[
    "how-it-works",
    "why-it-exists",
    "what-changed",
    "problem-solution",
    "gotcha",
    "pattern",
    "trade-off",
];

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

/// The offset as claude-mem's header names a zone it has no name for: `GMT+9`, `GMT-7`,
/// `GMT+5:30`, and `UTC` at none.
fn gmt(seconds: i32) -> String {
    if seconds == 0 {
        return "UTC".into();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    match minutes % 60 {
        0 => format!("GMT{sign}{}", minutes / 60),
        m => format!("GMT{sign}{}:{m:02}", minutes / 60),
    }
}

/// claude-mem's recent-context block (docs/cards.md S2, S3) for `cards`, newest first as `recent`
/// gives them, shown the oldest first by day, at `now` in `tz`. `name` is the repository's.
pub fn block<Tz: chrono::TimeZone>(cards: &[Card], name: &str, now: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Offset;
    let local = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(tz));
    // `9:05am` as claude-mem's row shows it: `9:05a`.
    let clock = |t: &chrono::DateTime<Tz>| {
        let mut s = t.format("%-I:%M%P").to_string();
        s.pop();
        s
    };
    let header = local(now).map_or_else(String::new, |t| {
        let zone = gmt(t.offset().fix().local_minus_utc());
        format!(", {} {zone}", t.format("%Y-%m-%d %-I:%M%P"))
    });
    let legend: Vec<String> = ICONS
        .iter()
        .zip(TYPES)
        .map(|(i, t)| format!("{i}{t}"))
        .collect();
    let mut out = format!(
        "# [{name}] recent context{header}\n\nLegend: {}\nFormat: ID TIME TYPE TITLE\n\
         Fetch details: get(ID) | Search: search(query)\n\n",
        legend.join(" ")
    );
    // In time order; a window's cards in the order its curator wrote them.
    let mut shown: Vec<&Card> = cards.iter().collect();
    shown.sort_by(|a, b| (a.ts, &a.device, a.op_seq, a.n).cmp(&(b.ts, &b.device, b.op_seq, b.n)));
    let (mut day, mut minute) = (String::new(), String::new());
    for c in shown {
        let Some(t) = local(c.ts) else {
            continue;
        };
        let d = t.format("%b %-d, %Y").to_string();
        if d != day {
            out.push_str(&format!("### {d}\n"));
            day = d;
            minute.clear();
        }
        let m = clock(&t);
        let time = if m == minute {
            "\"".to_owned()
        } else {
            m.clone()
        };
        minute = m;
        let icon = c
            .kind
            .as_deref()
            .and_then(|k| TYPES.iter().position(|t| *t == k))
            .map_or(NO_TYPE, |i| ICONS[i]);
        let title = match c.title.trim() {
            "" => "Untitled".to_owned(),
            t => t.replace(['\n', '\r'], " "),
        };
        out.push_str(&format!("{} {time} {icon} {title}\n", c.id()));
    }
    out
}

/// `block` within `room` characters, fitted as claude-mem fits its own (S5): the newest cards,
/// their number halved until it fits, down to one. None when not even one does, or no card.
pub fn fitted<Tz: chrono::TimeZone>(
    cards: &[Card],
    name: &str,
    now: i64,
    tz: &Tz,
    room: usize,
) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let mut n = cards.len();
    while n > 0 {
        let b = block(&cards[..n], name, now, tz);
        if b.chars().count() <= room {
            return Some(b);
        }
        n /= 2;
    }
    None
}

/// What a reader reads of a card, in `read`'s order.
const COLUMNS: &str = "device, op_seq, n, ts, agent, session, repo, type, title, subtitle, \
                       narrative, facts, concepts, files_read, files_modified, from_seq, to_seq, \
                       removed, goals";

/// A row of `COLUMNS` as a reader gets it, or none while a removal its op does not list took
/// from a record its curator was shown (K4).
fn read(r: &rusqlite::Row, raw: &Raw, rules: &Rules) -> Result<Option<Card>> {
    let device: String = r.get(0)?;
    let listed: Vec<Removal> = serde_json::from_str(&r.get::<_, String>(17)?).unwrap_or_default();
    let goals: Vec<i64> = serde_json::from_str(&r.get::<_, String>(18)?).unwrap_or_default();
    let mut removed = raw.removed_in(&device, r.get(15)?, r.get(16)?, None)?;
    for goal in goals {
        removed.extend(raw.removed_in(&device, goal, goal, None)?);
    }
    if removed.iter().any(|x| !listed.contains(x)) {
        return Ok(None);
    }
    // K6: every text as the rules are now, the labels it is shown under too.
    let gate = |s: String| crate::redact::outbound_with(&s, rules);
    let list = |i: usize| -> rusqlite::Result<Vec<String>> {
        let items: Vec<String> = serde_json::from_str(&r.get::<_, String>(i)?).unwrap_or_default();
        Ok(items.into_iter().map(gate).collect())
    };
    let narrative = gate(r.get(10)?);
    // Gated before it is cut: a value the cut would split is whole when the rules read it.
    let title = match r.get::<_, String>(8)? {
        t if t.is_empty() => first_sentence(&narrative),
        t => gate(t),
    };
    Ok(Some(Card {
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
        facts: list(11)?,
        concepts: list(12)?,
        files_read: list(13)?,
        files_modified: list(14)?,
    }))
}

/// `repo`'s newest cards, at most `limit`, the newest first.
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
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards WHERE repo = ?1 AND replaced_by IS NULL
         ORDER BY ts DESC, device DESC, op_seq DESC, n"
    ))?;
    let mut rows = st.query([repo])?;
    let mut out = Vec::new();
    while out.len() < limit
        && let Some(r) = rows.next()?
    {
        out.extend(read(r, raw, rules)?);
    }
    Ok(out)
}

/// The current card an ID names, `<op seq>.<n>` (S6), as `recent` would read it.
// ponytail: the first device's card of that op seq; a device in the ID with sync (S3).
pub fn get(k: &Connection, raw: &Raw, id: &str, rules: &Rules) -> Result<Option<Card>> {
    let Some((Ok(op_seq), Ok(n))) = id
        .trim()
        .split_once('.')
        .map(|(o, n)| (o.parse::<i64>(), n.parse::<i64>()))
    else {
        return Ok(None);
    };
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok(None);
    }
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards WHERE op_seq = ?1 AND n = ?2 AND replaced_by IS NULL
         ORDER BY device"
    ))?;
    let mut rows = st.query([op_seq, n])?;
    while let Some(r) = rows.next()? {
        if let Some(c) = read(r, raw, rules)? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    /// Unix ms of a time in Japan.
    fn at(d: u32, h: u32, m: u32, s: u32) -> i64 {
        jst()
            .with_ymd_and_hms(2026, 10, d, h, m, s)
            .unwrap()
            .timestamp_millis()
    }

    fn card(op_seq: i64, n: i64, ts: i64, kind: Option<&str>, title: &str) -> Card {
        Card {
            device: "d".into(),
            op_seq,
            n,
            ts,
            agent: None,
            session: None,
            repo: Some("r".into()),
            kind: kind.map(str::to_owned),
            title: title.into(),
            subtitle: String::new(),
            narrative: String::new(),
            facts: Vec::new(),
            concepts: Vec::new(),
            files_read: Vec::new(),
            files_modified: Vec::new(),
        }
    }

    /// Newest first, as `recent` gives them.
    fn four() -> Vec<Card> {
        vec![
            card(
                420,
                1,
                at(3, 6, 5, 30),
                Some("bugfix"),
                "Two workers no longer race for one lock",
            ),
            card(
                413,
                0,
                at(2, 21, 41, 50),
                Some("change"),
                "The lock file is removed on exit",
            ),
            card(
                412,
                0,
                at(2, 21, 41, 10),
                Some("discovery"),
                "The worker leaves a lock it no longer holds",
            ),
            card(400, 0, at(2, 9, 5, 0), None, "A summary card"),
        ]
    }

    /// docs/cards.md S2, S3: claude-mem's recent context, in local time, the oldest first by day.
    #[test]
    fn the_block_is_claude_mems_recent_context_in_local_time() {
        let block = block(&four(), "oboete", at(3, 7, 37, 0), &jst());
        assert_eq!(
            block,
            "# [oboete] recent context, 2026-10-03 7:37am GMT+9\n\
             \n\
             Legend: ●bugfix ◆feature ↻refactor ✓change ○discovery ⚖decision ⚠security_alert \
             ⚷security_note ⊘sensitive\n\
             Format: ID TIME TYPE TITLE\n\
             Fetch details: get(ID) | Search: search(query)\n\
             \n\
             ### Oct 2, 2026\n\
             400.0 9:05a 📝 A summary card\n\
             412.0 9:41p ○ The worker leaves a lock it no longer holds\n\
             413.0 \" ✓ The lock file is removed on exit\n\
             ### Oct 3, 2026\n\
             420.1 6:05a ● Two workers no longer race for one lock\n"
        );
        let utc = FixedOffset::east_opt(0).unwrap();
        assert!(block_of(&utc).starts_with("# [oboete] recent context, 2026-10-02 10:37pm UTC\n"));
        let india = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        assert!(block_of(&india).contains(" GMT+5:30\n"));
        let west = FixedOffset::west_opt(7 * 3600).unwrap();
        assert!(block_of(&west).contains(" GMT-7\n"));
    }

    fn block_of(tz: &FixedOffset) -> String {
        block(&four(), "oboete", at(3, 7, 37, 0), tz)
    }

    /// S5: claude-mem's fit, the newest cards kept: their number halves until the block fits.
    #[test]
    fn a_block_halves_its_cards_until_it_fits_its_room() {
        let now = at(3, 7, 37, 0);
        let whole = block(&four(), "oboete", now, &jst());
        let n = whole.chars().count();
        assert_eq!(fitted(&four(), "oboete", now, &jst(), n), Some(whole));
        let two = fitted(&four(), "oboete", now, &jst(), n - 1).unwrap();
        assert!(two.contains("420.1 ") && two.contains("413.0 "), "{two}");
        assert!(!two.contains("412.0 ") && !two.contains("400.0 "), "{two}");
        assert_eq!(two, block(&four()[..2], "oboete", now, &jst()));
        let one = block(&four()[..1], "oboete", now, &jst());
        let room = one.chars().count();
        assert_eq!(fitted(&four(), "oboete", now, &jst(), room), Some(one));
        assert_eq!(fitted(&four(), "oboete", now, &jst(), room - 1), None);
        assert_eq!(fitted(&[], "oboete", now, &jst(), usize::MAX), None);
    }
}
