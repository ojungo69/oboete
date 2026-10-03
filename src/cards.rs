//! Cards (docs/cards.md): what a window of work was, as claude-mem's observation shapes it (a
//! type, a title, a subtitle, a narrative, facts, concepts, files). `consumer::cards` keeps them
//! in knowledge.db from the window ops; every reader takes them through this module (K7).

use crate::raw::{Raw, Removal};
use crate::redact::Rules;
use crate::turns::TurnSummary;
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
    /// Gated as it was written; a summary's card has its first sentence (K1).
    pub title: String,
    /// The title on the one line its row at session start shows it on, gated as it was written
    /// and as that line: a rule may match only the flattened title (Codex on slice 3).
    pub row_title: String,
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

/// A feed page's last kept card: its window time and stored ID, all descending (page.md P3).
pub type Position = (i64, String, i64, i64);

/// claude-mem's icon of each of `TYPES`, in its order.
const ICONS: [&str; 9] = ["●", "◆", "↻", "✓", "○", "⚖", "⚠", "⚷", "⊘"];
/// claude-mem's icon for a type it does not know: here a card without one (K1).
const NO_TYPE: &str = "📝";

impl Card {
    pub fn position(&self) -> Position {
        (self.ts, self.device.clone(), self.op_seq, self.n)
    }

    /// Its ID as session start shows it and `get` reads it (docs/cards.md S3): `<op seq>.<n>` on
    /// `local`, the device that reads it, and `<device>.<op seq>.<n>` for another device's card
    /// (a copied home keeps the ops of the device it was copied from; sync brings others').
    pub fn id(&self, local: &str) -> String {
        if self.device == local {
            format!("{}.{}", self.op_seq, self.n)
        } else {
            format!("{}.{}.{}", self.device, self.op_seq, self.n)
        }
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

/// claude-mem's recent-context block (docs/cards.md S2, S3) for `cards` and the session summaries
/// `sessions` (docs/summaries.md S7), newest first as their readers give them, shown the oldest
/// first by day, at `now` in `tz`, read on device `local`, then `latest`'s fields when it is not
/// older than the newest card (S8). `name` is the repository's.
pub fn block<Tz: chrono::TimeZone>(
    cards: &[Card],
    sessions: &[TurnSummary],
    latest: Option<&TurnSummary>,
    local: &str,
    name: &str,
    now: i64,
    tz: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Offset;
    let in_tz = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(tz));
    // `9:05am` as claude-mem's row shows it: `9:05a`.
    let clock = |t: &chrono::DateTime<Tz>| {
        let mut s = t.format("%-I:%M%P").to_string();
        s.pop();
        s
    };
    let header = in_tz(now).map_or_else(String::new, |t| {
        let zone = gmt(t.offset().fix().local_minus_utc());
        format!(", {} {zone}", t.format("%Y-%m-%d %-I:%M%P"))
    });
    let legend: Vec<String> = std::iter::once("🎯session".to_owned())
        .chain(ICONS.iter().zip(TYPES).map(|(i, t)| format!("{i}{t}")))
        .collect();
    let mut out = format!(
        "# [{name}] recent context{header}\n\nLegend: {}\nFormat: ID TIME TYPE TITLE\n\
         Fetch details: get(ID) | Search: search(query)\n\n",
        legend.join(" ")
    );
    // In time order, a summary after the cards of its time (claude-mem's); a window's cards in
    // the order its curator wrote them.
    enum Row<'a> {
        Card(&'a Card),
        Session(&'a TurnSummary),
    }
    let mut rows: Vec<Row> = cards
        .iter()
        .map(Row::Card)
        .chain(sessions.iter().map(Row::Session))
        .collect();
    rows.sort_by_key(|r| match r {
        Row::Card(c) => (c.ts, false, c.device.clone(), c.op_seq, c.n),
        Row::Session(s) => (s.ts, true, s.device.clone(), s.op_seq, 0),
    });
    let (mut day, mut minute) = (String::new(), String::new());
    for row in rows {
        let ts = match row {
            Row::Card(c) => c.ts,
            Row::Session(s) => s.ts,
        };
        let Some(t) = in_tz(ts) else {
            continue;
        };
        let d = t.format("%b %-d, %Y").to_string();
        if d != day {
            out.push_str(&format!("### {d}\n"));
            day = d;
            minute.clear();
        }
        let c = match row {
            Row::Card(c) => c,
            // A summary's own date and time, and the minute of the card before it kept.
            Row::Session(s) => {
                let request = match s.row.trim() {
                    "" => "Session started",
                    r => r,
                };
                let at = t.format("%b %-d, %-I:%M %p");
                out.push_str(&format!("{} {request} ({at})\n", s.id(local)));
                continue;
            }
        };
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
        let title = match c.row_title.trim() {
            "" => "Untitled",
            t => t,
        };
        out.push_str(&format!("{} {time} {icon} {title}\n", c.id(local)));
    }
    if let Some(s) = latest.filter(|s| cards.iter().all(|c| c.ts <= s.ts)) {
        for (field, label) in [
            ("investigated", "Investigated"),
            ("learned", "Learned"),
            ("completed", "Completed"),
            ("next_steps", "Next Steps"),
        ] {
            if let Some(text) = s.fields.get(field).filter(|t| !t.is_empty()) {
                out.push_str(&format!("**{label}**: {text}\n\n"));
            }
        }
    }
    out
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
    // A summary's title is cut from the gated narrative: a value the cut would split is whole
    // when the rules read it.
    let (title, row_title) = match r.get::<_, String>(8)? {
        t if t.is_empty() => {
            let t = first_sentence(&narrative);
            (t.clone(), t)
        }
        t => (gate(t.clone()), t),
    };
    let row_title = crate::redact::flattened_with(
        &row_title,
        rules,
        usize::MAX,
        crate::consumer::manifest::one_line,
    )
    .masked();
    Ok(Some(Card {
        device,
        op_seq: r.get(1)?,
        n: r.get(2)?,
        ts: r.get(3)?,
        agent: r.get::<_, Option<String>>(4)?.map(gate),
        session: r.get::<_, Option<String>>(5)?.map(gate),
        repo: r.get::<_, Option<String>>(6)?.map(gate),
        // One of the nine or none, whatever an op from elsewhere says (C3, Codex on #371): a
        // type is shown as it is, never gated.
        kind: r
            .get::<_, Option<String>>(7)?
            .filter(|k| TYPES.contains(&k.as_str())),
        title,
        row_title,
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

/// A repository's (or all repositories') cards after `before`, newest first. The one reader
/// still skips K4-hidden rows without using a slot. The boolean says another visible row
/// exists: at most one extra is inspected for exhaustion, never returned or used as a cursor.
pub fn page(
    k: &Connection,
    raw: &Raw,
    repo: Option<&str>,
    before: Option<&Position>,
    limit: usize,
    rules: &Rules,
) -> Result<(Vec<Card>, bool)> {
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok((Vec::new(), false));
    }
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards WHERE replaced_by IS NULL
           AND (?1 IS NULL OR repo = ?1)
           AND (?2 IS NULL OR (ts, device, op_seq, n) < (?2, ?3, ?4, ?5))
         ORDER BY ts DESC, device DESC, op_seq DESC, n DESC"
    ))?;
    let mut rows = st.query(rusqlite::params![
        repo,
        before.map(|p| p.0),
        before.map(|p| p.1.as_str()),
        before.map(|p| p.2),
        before.map(|p| p.3)
    ])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        if let Some(c) = read(r, raw, rules)? {
            if out.len() == limit {
                return Ok((out, true));
            }
            out.push(c);
        }
    }
    Ok((out, false))
}

/// A card with the span of its window and the goals that window was shown.
pub type TurnCard = (Card, (i64, i64), Vec<i64>);

/// The cards a turn's summary may be shown (docs/summaries.md T2): its session's on `device`, of
/// the windows that hold any of the records `from` to `through`, the newest first, at most
/// `limit`; each with the span of its window and the goals it was shown, which a summary shown
/// the card was built on too (T7).
pub fn of_turn(
    k: &Connection,
    raw: &Raw,
    device: &str,
    (agent, session): (&str, &str),
    (from, through): (i64, i64),
    limit: usize,
    rules: &Rules,
) -> Result<Vec<TurnCard>> {
    let mut out = Vec::new();
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok(out);
    }
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards
         WHERE device = ?1 AND agent = ?2 AND session = ?3 AND from_seq <= ?5 AND to_seq >= ?4
           AND replaced_by IS NULL
         ORDER BY ts DESC, op_seq DESC, n"
    ))?;
    let mut rows = st.query(rusqlite::params![device, agent, session, from, through])?;
    while out.len() < limit
        && let Some(r) = rows.next()?
    {
        let span: (i64, i64) = (r.get(15)?, r.get(16)?);
        if !raw.has_live(device, (agent, session), span.0, span.1)? {
            continue;
        }
        let Some(card) = read(r, raw, rules)? else {
            continue;
        };
        let goals: Vec<i64> = serde_json::from_str(&r.get::<_, String>(18)?).unwrap_or_default();
        out.push((card, span, goals));
    }
    Ok(out)
}

/// The current card an ID names (S3, S6), as `recent` would read it: `<op seq>.<n>` of this
/// device's, or `<device>.<op seq>.<n>`.
pub fn get(k: &Connection, raw: &Raw, id: &str, rules: &Rules) -> Result<Option<Card>> {
    let parts: Vec<&str> = id.trim().split('.').collect();
    let (device, op_seq, n) = match parts[..] {
        [op_seq, n] => (raw.device(), op_seq, n),
        [device, op_seq, n] => (device, op_seq, n),
        _ => return Ok(None),
    };
    let (Ok(op_seq), Ok(n)) = (op_seq.parse::<i64>(), n.parse::<i64>()) else {
        return Ok(None);
    };
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok(None);
    }
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards
         WHERE device = ?1 AND op_seq = ?2 AND n = ?3 AND replaced_by IS NULL"
    ))?;
    let mut rows = st.query(rusqlite::params![device, op_seq, n])?;
    match rows.next()? {
        Some(r) => read(r, raw, rules),
        None => Ok(None),
    }
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
            row_title: title.into(),
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
            Card {
                device: "e".into(),
                ..card(400, 0, at(2, 9, 5, 0), None, "A summary card")
            },
        ]
    }

    /// docs/cards.md S2, S3: claude-mem's recent context, in local time, the oldest first by day;
    /// another device's card is named with its device.
    #[test]
    fn the_block_is_claude_mems_recent_context_in_local_time() {
        let block = block(&four(), &[], None, "d", "oboete", at(3, 7, 37, 0), &jst());
        assert_eq!(
            block,
            "# [oboete] recent context, 2026-10-03 7:37am GMT+9\n\
             \n\
             Legend: 🎯session ●bugfix ◆feature ↻refactor ✓change ○discovery ⚖decision \
             ⚠security_alert ⚷security_note ⊘sensitive\n\
             Format: ID TIME TYPE TITLE\n\
             Fetch details: get(ID) | Search: search(query)\n\
             \n\
             ### Oct 2, 2026\n\
             e.400.0 9:05a 📝 A summary card\n\
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
        block(&four(), &[], None, "d", "oboete", at(3, 7, 37, 0), tz)
    }

    /// A summary of turn op `op_seq` at `ts` that asked `request`, with `fields`.
    fn summary(op_seq: i64, ts: i64, request: &str, fields: &[(&str, &str)]) -> TurnSummary {
        let mut all: std::collections::BTreeMap<String, String> = fields
            .iter()
            .map(|(f, t)| ((*f).to_owned(), (*t).to_owned()))
            .collect();
        if !request.is_empty() {
            all.insert("request".into(), request.into());
        }
        TurnSummary {
            device: "d".into(),
            op_seq,
            ts,
            agent: "claude".into(),
            session: "s".into(),
            repo: Some("r".into()),
            row: request.into(),
            fields: all,
        }
    }

    /// docs/summaries.md S7, S8: the newest summaries are rows among the cards by their own time,
    /// after a card of the same time, `S<op seq>` (another device's with its device) and
    /// `Session started` without a request; the newest one's four fields follow the timeline when
    /// it is not older than the newest card shown.
    #[test]
    fn the_block_shows_the_session_summaries_among_the_cards() {
        let newest = summary(
            500,
            at(3, 6, 20, 0),
            "Fix the lock race",
            &[
                ("investigated", "How two workers take the lock."),
                ("learned", "The lock was never released."),
                ("completed", "Workers release it on exit."),
                ("next_steps", "Measure the wait."),
                ("notes", "Not shown."),
            ],
        );
        let other = TurnSummary {
            device: "e".into(),
            ..summary(450, at(2, 21, 41, 10), "", &[])
        };
        let sessions = [newest.clone(), other];
        let shown = block(
            &four(),
            &sessions,
            Some(&newest),
            "d",
            "oboete",
            at(3, 7, 37, 0),
            &jst(),
        );
        let timeline = shown.split_once("### ").unwrap().1;
        assert_eq!(
            timeline,
            "Oct 2, 2026\n\
             e.400.0 9:05a 📝 A summary card\n\
             412.0 9:41p ○ The worker leaves a lock it no longer holds\n\
             Se.450 Session started (Oct 2, 9:41 PM)\n\
             413.0 \" ✓ The lock file is removed on exit\n\
             ### Oct 3, 2026\n\
             420.1 6:05a ● Two workers no longer race for one lock\n\
             S500 Fix the lock race (Oct 3, 6:20 AM)\n\
             **Investigated**: How two workers take the lock.\n\
             \n\
             **Learned**: The lock was never released.\n\
             \n\
             **Completed**: Workers release it on exit.\n\
             \n\
             **Next Steps**: Measure the wait.\n\
             \n"
        );
        // Older than the newest card shown: no fields.
        let older = summary(501, at(3, 6, 0, 0), "Earlier", &[("learned", "Something.")]);
        let shown = block(
            &four(),
            std::slice::from_ref(&older),
            Some(&older),
            "d",
            "oboete",
            at(3, 7, 37, 0),
            &jst(),
        );
        assert!(
            shown.contains("S501 Earlier (Oct 3, 6:00 AM)\n") && !shown.contains("**Learned**")
        );
    }
}
