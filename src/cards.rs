//! Cards (docs/cards.md): what a window of work was, as claude-mem's observation shapes it (a
//! type, a title, a subtitle, a narrative, facts, concepts, files). `consumer::cards` keeps them
//! in knowledge.db from the window ops; every reader takes them through this module (K7).

use crate::raw::{Raw, Removal};
use crate::redact::Rules;
use crate::turns::TurnSummary;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;

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
    /// The records its window covers on `device`; none for an imported observation.
    pub window: Option<(i64, i64)>,
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

/// The session start's icon of a card's type (S3), `NO_TYPE` for none or one it does not know.
pub(crate) fn icon(kind: Option<&str>) -> &'static str {
    kind.and_then(|k| TYPES.iter().position(|t| *t == k))
        .map_or(NO_TYPE, |i| ICONS[i])
}

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
         CREATE INDEX IF NOT EXISTS cards_repo ON cards(repo, ts);
         -- The order every reader's page is in, and one session's page (`page_sql`,
         -- `around_sql`): W4.
         CREATE INDEX IF NOT EXISTS cards_newest ON cards(ts, device, op_seq, n);
         CREATE INDEX IF NOT EXISTS cards_session ON cards(agent, session, ts);",
    )?;
    // Parity row 3: 1 for a card removed from the page (`hide`), left out by every reader. Before
    // the full-text backfill below, which leaves such a card out.
    crate::claims::migrate(
        k,
        "SELECT count(*) = 0 FROM pragma_table_info('cards') WHERE name = 'hidden'",
        "ALTER TABLE cards ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;",
    )?;
    if !crate::consumer::manifest::exists(k, "table", "cards_fts")? {
        // A store whose cards consumer has already passed its ops needs a backfill. Creating
        // the index and filling it commit together, also when a search opens it first.
        let tx = if k.is_autocommit() {
            Some(k.unchecked_transaction()?)
        } else {
            None
        };
        k.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS cards_fts USING fts5(text, tokenize='trigram');",
        )?;
        crate::consumer::fts::cards(k, None)?;
        if let Some(tx) = tx {
            tx.commit()?;
        }
    }
    if !crate::consumer::manifest::exists(k, "table", "card_files")? {
        // X5 F3: the files each card names, by their last part. Filled from the cards a store
        // already has, in the same transaction as the table, as `cards_fts` is.
        let tx = if k.is_autocommit() {
            Some(k.unchecked_transaction()?)
        } else {
            None
        };
        k.execute_batch(
            "CREATE TABLE IF NOT EXISTS card_files(
               device TEXT NOT NULL, op_seq INTEGER NOT NULL, n INTEGER NOT NULL,
               name TEXT NOT NULL, path TEXT NOT NULL, modified INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS card_files_name ON card_files(name);",
        )?;
        let cards: Vec<(String, i64, i64, String, String)> = k
            .prepare("SELECT device, op_seq, n, files_read, files_modified FROM cards")?
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (device, op_seq, n, read, modified) in cards {
            let list = |s: &str| serde_json::from_str(s).unwrap_or_default();
            index_files(k, &device, op_seq, n, [&list(&read), &list(&modified)])?;
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
    }
    // Parity row X1: 1 for a card of a window that is a subagent's work alone
    // (`Raw::subagent_only`), 0 for another; none for one kept before, until the cards consumer
    // marks it (`consumer::cards::mark`).
    crate::claims::migrate(
        k,
        "SELECT count(*) = 0 FROM pragma_table_info('cards') WHERE name = 'subagent'",
        "ALTER TABLE cards ADD COLUMN subagent INTEGER;
         CREATE INDEX IF NOT EXISTS cards_unmarked ON cards(ts) WHERE subagent IS NULL;",
    )?;
    crate::hide::schema(k)
}

/// A path as `card_files` keeps it and is searched by: `/` for `\\`.
pub(crate) fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

/// The last part of a slashed path, which `card_files` is searched by.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// X5 F3: card `(device, op_seq, n)`'s files, from its `files_read` and `files_modified` lists,
/// into `card_files`.
pub(crate) fn index_files(
    k: &Connection,
    device: &str,
    op_seq: i64,
    n: i64,
    lists: [&serde_json::Value; 2],
) -> Result<()> {
    let mut st = k.prepare(
        "INSERT INTO card_files(device, op_seq, n, name, path, modified)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (modified, list) in [false, true].into_iter().zip(lists) {
        for path in list.as_array().into_iter().flatten() {
            if let Some(path) = path.as_str().map(slashed) {
                let at = rusqlite::params![device, op_seq, n, file_name(&path), path, modified];
                st.execute(at)?;
            }
        }
    }
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

/// Whether `text` begins with all of `title`, so that a line names it by its text alone: an empty
/// title, a title cut with `…` or `...` from the text's start (a summary's card, a note saved
/// without a title), or one that ends where a word or a sentence ends. A title that only begins
/// a word of the text ("Plan" and "Planet-scale") is kept (Codex on #571).
pub(crate) fn begins_with_title(text: &str, title: &str) -> bool {
    let stem = title.trim_end_matches('…').trim_end_matches("...");
    let cut = stem.len() < title.len();
    text.strip_prefix(stem).is_some_and(|rest| {
        cut || !stem.chars().next_back().is_some_and(char::is_alphanumeric)
            || !rest.chars().next().is_some_and(char::is_alphanumeric)
    })
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

/// `2026-10-03 6:16pm GMT+9`, as claude-mem's headers give the local time.
fn stamp<Tz: chrono::TimeZone>(t: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Offset;
    let zone = gmt(t.offset().fix().local_minus_utc());
    format!("{} {zone}", t.format("%Y-%m-%d %-I:%M%P"))
}

/// `9:05am` as claude-mem's row shows it: `9:05a`.
fn clock<Tz: chrono::TimeZone>(t: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let mut s = t.format("%-I:%M%P").to_string();
    s.pop();
    s
}

/// claude-mem's recent-context block (docs/cards.md S2, S3) for `cards` and the session summaries
/// `sessions` (docs/summaries.md S7), newest first as their readers give them, shown the oldest
/// first by day, at `now` in `tz`, read on device `local`, then `latest`'s fields when it is not
/// older than the newest card (S8). `name` is the repository's.
#[allow(clippy::too_many_arguments)]
pub fn block<Tz: chrono::TimeZone>(
    cards: &[Card],
    sessions: &[TurnSummary],
    latest: Option<&TurnSummary>,
    local: &str,
    name: &str,
    work: &HashMap<i64, i64>,
    now: i64,
    tz: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let in_tz = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(tz));
    let header = in_tz(now).map_or_else(String::new, |t| format!(", {}", stamp(&t)));
    let legend: Vec<String> = std::iter::once("🎯session".to_owned())
        .chain(ICONS.iter().zip(TYPES).map(|(i, t)| format!("{i}{t}")))
        .collect();
    // Parity row 13, claude-mem's economics at its default settings (only the percentage on): what
    // the cards shown cost to read, as its TokenCalculator counts an observation, against what the
    // curator spent on their windows (`work`, by window op), each once however many cards it made.
    let read: usize = cards
        .iter()
        .map(|c| {
            let facts = serde_json::to_string(&c.facts).unwrap_or_default();
            [&c.title, &c.subtitle, &c.narrative, &facts]
                .iter()
                .map(|t| t.encode_utf16().count())
                .sum::<usize>()
                .div_ceil(4)
        })
        .sum();
    let windows: std::collections::HashSet<i64> = cards
        .iter()
        .filter(|c| c.device == local)
        .map(|c| c.op_seq)
        .collect();
    let spent: i64 = windows.iter().filter_map(|w| work.get(w)).sum();
    let saved = spent - i64::try_from(read).unwrap_or(i64::MAX);
    let mut stats = format!(
        "Stats: {} obs ({}t read) | {}t work",
        cards.len(),
        grouped(read as u64),
        grouped(spent.unsigned_abs())
    );
    if spent > 0 {
        // JavaScript's Math.round, which rounds a half up.
        let percent = (saved as f64 / spent as f64 * 100.0 + 0.5).floor() as i64;
        stats.push_str(&format!(" | {percent}% savings"));
    }
    let mut out = format!(
        "# [{name}] recent context{header}\n\nLegend: {}\nFormat: ID TIME TYPE TITLE\n\
         Fetch details: get(ID) | Search: search(query)\n\n{stats}\n\n",
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
        let icon = icon(c.kind.as_deref());
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
            ("notes", "Notes"),
        ] {
            if let Some(text) = s.fields.get(field).filter(|t| !t.is_empty()) {
                out.push_str(&format!("**{label}**: {text}\n\n"));
            }
        }
    }
    if spent > 0 && saved > 0 {
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
        let thousands = (spent as f64 / 1000.0 + 0.5).floor() as i64;
        out.push_str(&format!(
            "Access {thousands}k tokens of past work via get(ID) or search(query).\n"
        ));
    }
    out
}

/// `n` as JavaScript's `toLocaleString()` writes it in claude-mem's lines: `17,868`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// What the curator spent on the window of each card of `cards` this device curated, by its
/// window op (parity row 13), as claude-mem's `discovery_tokens` are what its observer spent: the
/// prompt and answer tokens of the call that made the op's cards, or the estimate of what was
/// sent when a CLI reports no usage. That call is the last one before the op was written, by the
/// provider the op names, over its window's records, that made cards or whose refused answer the
/// op keeps (`unanchored`): the windows a long record is split into share their records, and a
/// recuration comes after. A window of another device, an imported observation and a ledger that
/// cannot be read have none: the line is then without them, and a hook never fails for it.
pub fn work(home: &std::path::Path, raw: &Raw, cards: &[Card]) -> HashMap<i64, i64> {
    let read = || -> Result<HashMap<i64, i64>> {
        let mut out = HashMap::new();
        let Some(db) = crate::providers_db::read_only(home)? else {
            return Ok(out);
        };
        // A hook does not wait for the ledger: a busy one leaves the line without work.
        db.busy_timeout(std::time::Duration::ZERO)?;
        let mut st = db.prepare(
            "SELECT COALESCE(prompt_tokens, est_tokens, 0) + COALESCE(completion_tokens, 0)
             FROM provider_calls
             WHERE role = 'curator' AND span = ?1 AND ts <= ?2 AND provider = ?3
               AND outcome IN ('ok', 'unanchored')
             ORDER BY ts DESC, id DESC LIMIT 1",
        )?;
        for c in cards.iter().filter(|c| c.device == raw.device()) {
            let Some((from, to)) = c.window else {
                continue;
            };
            if out.contains_key(&c.op_seq) {
                continue;
            }
            let Some((at, Some(provider))) = raw.op_stamp(raw.device(), c.op_seq)? else {
                continue;
            };
            let tokens = st
                .query_row(
                    rusqlite::params![format!("{from}-{to}"), at, provider],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(tokens) = tokens {
                out.insert(c.op_seq, tokens);
            }
        }
        Ok(out)
    };
    read().unwrap_or_default()
}

/// What a reader reads of a card, in `read`'s order.
pub(crate) const COLUMNS: &str = "device, op_seq, n, ts, agent, session, repo, type, title, subtitle, \
                       narrative, facts, concepts, files_read, files_modified, from_seq, to_seq, \
                       removed, goals";

/// K4: whether a removal its op does not list took from a record the curator of a row of
/// `COLUMNS` was shown: its window, the stretches that window read, as its op lists them
/// (decision 107, `Raw::window_read`), and its goals.
pub(crate) fn taken_row(r: &rusqlite::Row, raw: &Raw) -> Result<bool> {
    let device: String = r.get(0)?;
    let listed: Vec<Removal> = serde_json::from_str(&r.get::<_, String>(17)?).unwrap_or_default();
    let goals: Vec<i64> = serde_json::from_str(&r.get::<_, String>(18)?).unwrap_or_default();
    let mut removed = raw.removed_in(&device, r.get(15)?, r.get(16)?, None)?;
    for (from, to) in raw.window_read(&device, r.get(1)?)? {
        removed.extend(raw.removed_in(&device, from, to, None)?);
    }
    for goal in goals {
        removed.extend(raw.removed_in(&device, goal, goal, None)?);
    }
    Ok(removed.iter().any(|x| !listed.contains(x)))
}

/// A row of `COLUMNS` as a reader gets it, or none while a removal its op does not list took
/// from a record its curator was shown (K4).
fn read(r: &rusqlite::Row, raw: &Raw, rules: &Rules) -> Result<Option<Card>> {
    if taken_row(r, raw)? {
        return Ok(None);
    }
    let device: String = r.get(0)?;
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
        repo: r.get::<_, Option<String>>(6)?.map(|s| {
            crate::redact::flattened_with(
                &s,
                rules,
                usize::MAX,
                crate::consumer::manifest::one_line,
            )
            .masked()
        }),
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
        window: Some((r.get(15)?, r.get(16)?)),
    }))
}

/// `repo`'s newest cards for session start, at most `limit`, the newest first: none of a
/// subagent's work alone, as claude-mem's `CONTEXT_MAIN_AGENT_ONLY` keeps its rows out (parity
/// row X1); search and the page still have them.
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
    // A hook can read a store the worker has not given the column yet.
    let marked: bool = k.query_row(
        "SELECT count(*) > 0 FROM pragma_table_info('cards') WHERE name = 'subagent'",
        [],
        |r| r.get(0),
    )?;
    let main = if marked { "AND subagent IS NOT 1" } else { "" };
    let shown = crate::hide::shown(k, "cards", "")?;
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards WHERE repo = ?1 AND replaced_by IS NULL {main} {shown}
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

/// `page`'s rows, the newest first, for what is asked: one repository's (`repo`), one session's
/// (`session`), those after a card (`before`), or any of them together. A statement names only
/// what it filters on, its parameters in that order, so SQLite reads each through the index that
/// is in its order and stops at the page's last row: every repository's through `cards_newest`,
/// one repository's through `cards_repo`, one session's through `cards_session` (the last two
/// sort only the cards of one `ts`). One statement for every shape (`?1 IS NULL OR repo = ?1`)
/// had one plan for all of them; with no index in the order that plan read and sorted every card
/// of the store (W4: 85 ms a page at 43,767 cards).
fn page_sql(shown: &str, repo: bool, session: bool, before: bool) -> String {
    let mut sql = format!("SELECT {COLUMNS} FROM cards WHERE replaced_by IS NULL {shown}");
    for (asked, filter) in [
        (repo, " AND repo = ?"),
        (session, " AND agent = ? AND session = ?"),
        (before, " AND (ts, device, op_seq, n) < (?, ?, ?, ?)"),
    ] {
        if asked {
            sql.push_str(filter);
        }
    }
    sql + " ORDER BY ts DESC, device DESC, op_seq DESC, n DESC"
}

/// `around`'s rows on one side of a time, every repository's or one's, as `page_sql` shapes its
/// own: the repository first when it is asked for, then the time.
fn around_sql(shown: &str, repo: bool, older: bool) -> String {
    let repo = if repo { " AND repo = ?" } else { "" };
    let side = if older {
        "ts <= ? ORDER BY ts DESC, device DESC, op_seq DESC, n DESC"
    } else {
        "ts > ? ORDER BY ts, device, op_seq, n"
    };
    format!("SELECT {COLUMNS} FROM cards WHERE replaced_by IS NULL {shown}{repo} AND {side}")
}

/// A repository's (or all repositories') cards after `before`, newest first, or one agent's
/// session's (`session`, `(agent, session)`, docs/page.md S4: a card whose window is that
/// session's alone, K2). The one reader still skips K4-hidden rows without using a slot. The
/// boolean says another visible row exists: at most one extra is inspected for exhaustion, never
/// returned or used as a cursor.
pub fn page(
    k: &Connection,
    raw: &Raw,
    (repo, session): (Option<&str>, Option<(&str, &str)>),
    before: Option<&Position>,
    limit: usize,
    rules: &Rules,
) -> Result<(Vec<Card>, bool)> {
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok((Vec::new(), false));
    }
    let shown = crate::hide::shown(k, "cards", "")?;
    let sql = page_sql(&shown, repo.is_some(), session.is_some(), before.is_some());
    let mut st = k.prepare(&sql)?;
    use rusqlite::types::Value;
    let text = |s: &str| Value::Text(s.to_owned());
    let asked = (repo.map(text).into_iter())
        .chain(
            session
                .into_iter()
                .flat_map(|(agent, session)| [text(agent), text(session)]),
        )
        .chain(before.into_iter().flat_map(|p| {
            [
                Value::Integer(p.0),
                text(&p.1),
                Value::Integer(p.2),
                Value::Integer(p.3),
            ]
        }));
    let mut rows = st.query(rusqlite::params_from_iter(asked))?;
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

/// docs/tools.md T2, T3: a repository's (or all repositories') current cards on one side of `at`,
/// those at or before it the newest first (`older`), else those after it the oldest first, at most
/// `limit`; a row the one reader hides takes no slot.
pub fn around(
    k: &Connection,
    raw: &Raw,
    repo: Option<&str>,
    at: i64,
    older: bool,
    limit: usize,
    rules: &Rules,
) -> Result<Vec<Card>> {
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok(Vec::new());
    }
    let shown = crate::hide::shown(k, "cards", "")?;
    let mut st = k.prepare(&around_sql(&shown, repo.is_some(), older))?;
    use rusqlite::types::Value;
    let asked = (repo.map(|r| Value::Text(r.to_owned())).into_iter()).chain([Value::Integer(at)]);
    let mut rows = st.query(rusqlite::params_from_iter(asked))?;
    let mut out = Vec::new();
    while out.len() < limit
        && let Some(r) = rows.next()?
    {
        out.extend(read(r, raw, rules)?);
    }
    Ok(out)
}

/// A card with the span of its window and the goals that window was shown.
pub type TurnCard = (Card, Vec<(i64, i64)>, Vec<i64>);

/// The cards a turn's summary may be shown (docs/summaries.md T2): its session's on `device`, of
/// the windows that hold any of the records `from` to `through`, the newest first, at most
/// `limit`; each with the span of its window, then the stretches that window read (decision
/// 107), and the goals it was shown, which a summary shown the card was built on too (T7).
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
    let shown = crate::hide::shown(k, "cards", "")?;
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards
         WHERE device = ?1 AND agent = ?2 AND session = ?3 AND from_seq <= ?5 AND to_seq >= ?4
           AND replaced_by IS NULL {shown}
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
        let mut spans = vec![span];
        spans.extend(raw.window_read(device, r.get(1)?)?);
        out.push((card, spans, goals));
    }
    Ok(out)
}

/// claude-mem's icons in its note on a file, not its session start's (docs/file-note.md).
const NOTE_ICONS: [(&str, &str); 6] = [
    ("decision", "⚖️"),
    ("bugfix", "🔴"),
    ("feature", "🟣"),
    ("refactor", "🔄"),
    ("discovery", "🔵"),
    ("change", "✅"),
];

/// A card `by_file` found, or an imported observation `imported_by_file` found as a card's row
/// shows it (docs/file-note.md F14), with what the note ranks it by as stored, before the gate
/// (K6) masks the card's own fields (Codex on #387): its agent and session, whether it modified
/// the file, and how many files it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Filed {
    pub card: Card,
    pub(crate) session: Option<(Option<String>, String)>,
    pub(crate) modified: bool,
    pub(crate) named: usize,
    /// An imported observation's uid, its ID; none for a card, whose ID is `Card::id`'s.
    pub(crate) uid: Option<String>,
}

impl Filed {
    /// Its ID as `get` takes it, read on device `local`.
    pub fn id(&self, local: &str) -> String {
        self.uid.clone().unwrap_or_else(|| self.card.id(local))
    }
}

/// X5 F3, F13, F14: what the note on a file considers, the newest `limit` of `repo`'s cards and
/// imported observations that name it by one of `paths`, newest first; `names` are the checkout's
/// claude-mem names (decision 52).
pub fn on_file(
    k: &Connection,
    raw: &Raw,
    (repo, names): (&str, &[String]),
    paths: &[&str],
    limit: usize,
    rules: &Rules,
) -> Result<Vec<Filed>> {
    let mut rows = by_file(k, raw, repo, paths, limit, rules)?;
    rows.extend(imported_by_file(
        k,
        raw,
        (repo, names),
        paths,
        limit,
        rules,
    )?);
    // Stable: a card before an observation of the same time, each in its reader's order.
    rows.sort_by_key(|f| std::cmp::Reverse(f.card.ts));
    rows.truncate(limit);
    Ok(rows)
}

/// docs/file-note.md F13: the imported observations that name a file by one of `paths`, newest
/// first, at most `limit`: by their newest files op (F12; only observations have one), their
/// newest row mapping to `repo` by its session as for the per-prompt block (#506,
/// `consumer::imported::maps`) or filed under one of the checkout's claude-mem `names` (decision
/// 52), not forgotten. None until the consumers have made the tables it reads.
pub fn imported_by_file(
    k: &Connection,
    raw: &Raw,
    (repo, names): (&str, &[String]),
    paths: &[&str],
    limit: usize,
    rules: &Rules,
) -> Result<Vec<Filed>> {
    let paths: Vec<String> = paths.iter().map(|p| slashed(p)).collect();
    let Some(name) = paths.first().map(|p| file_name(p).to_owned()) else {
        return Ok(Vec::new());
    };
    for table in ["imported_files", "imported", "session_start"] {
        if !crate::consumer::manifest::exists(k, "table", table)? {
            return Ok(Vec::new());
        }
    }
    let marks = (3..3 + paths.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut st = k.prepare(&format!(
        "SELECT i.uid, i.session, i.ts, i.kind, i.title, i.op_device, i.op_seq,
                MAX(f.modified), MAX(f.named)
         FROM imported_files f JOIN imported i ON i.uid = f.uid
         WHERE f.name = ?2 AND f.path IN ({marks})
           AND (f.op_device, f.op_seq) = (SELECT g.op_device, g.op_seq FROM imported_files g
             WHERE g.uid = f.uid ORDER BY g.rowid DESC LIMIT 1)
           AND i.rowid = (SELECT MAX(j.rowid) FROM imported j WHERE j.uid = f.uid)
           AND (EXISTS (SELECT 1 FROM session_start s WHERE s.session = i.session AND s.repo = ?1)
             OR i.repo COLLATE NOCASE IN (SELECT value FROM json_each(?{labels})))
         GROUP BY i.uid
         ORDER BY i.ts DESC, i.op_device DESC, i.op_seq DESC",
        labels = 3 + paths.len()
    ))?;
    let values = [repo.to_owned(), name]
        .into_iter()
        .chain(paths)
        .chain([crate::import::name_labels(names)]);
    let mut rows = st.query(rusqlite::params_from_iter(values))?;
    let mut out = Vec::new();
    while out.len() < limit
        && let Some(r) = rows.next()?
    {
        let uid: String = r.get(0)?;
        if raw.forgotten(&uid)? {
            continue;
        }
        let (session, title): (String, String) = (r.get(1)?, r.get(4)?);
        let row_title = crate::redact::flattened_with(
            &title,
            rules,
            usize::MAX,
            crate::consumer::manifest::one_line,
        )
        .masked();
        let card = Card {
            device: r.get(5)?,
            op_seq: r.get(6)?,
            n: 0,
            ts: r.get(2)?,
            agent: None,
            session: Some(crate::redact::outbound_with(&session, rules)),
            repo: None,
            // As a card's, one of claude-mem's types or none (C3).
            kind: Some(r.get::<_, String>(3)?).filter(|k| TYPES.contains(&k.as_str())),
            title: crate::redact::outbound_with(&title, rules),
            row_title,
            subtitle: String::new(),
            narrative: String::new(),
            facts: Vec::new(),
            concepts: Vec::new(),
            files_read: Vec::new(),
            files_modified: Vec::new(),
            window: None,
        };
        out.push(Filed {
            card,
            session: Some((None, session)),
            modified: r.get::<_, i64>(7)? != 0,
            named: usize::try_from(r.get::<_, i64>(8)?).unwrap_or(usize::MAX),
            uid: Some(uid),
        });
    }
    Ok(out)
}

/// X5 F3: `repo`'s current cards that name a file by one of `paths` (its absolute path, and its
/// paths relative to the working directory and to the checkout's top level), the newest first, at
/// most `limit`. The index is the worker's: a store it has not reached yet has none.
pub fn by_file(
    k: &Connection,
    raw: &Raw,
    repo: &str,
    paths: &[&str],
    limit: usize,
    rules: &Rules,
) -> Result<Vec<Filed>> {
    let paths: Vec<String> = paths.iter().map(|p| slashed(p)).collect();
    let Some(name) = paths.first().map(|p| file_name(p).to_owned()) else {
        return Ok(Vec::new());
    };
    if !crate::consumer::manifest::exists(k, "table", "card_files")? {
        return Ok(Vec::new());
    }
    let marks = (3..3 + paths.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let shown = crate::hide::shown(k, "cards", "")?;
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS}, (SELECT MAX(f.modified) FROM card_files f
             WHERE f.device = cards.device AND f.op_seq = cards.op_seq AND f.n = cards.n
               AND f.name = ?2 AND f.path IN ({marks}))
         FROM cards WHERE repo = ?1 AND replaced_by IS NULL {shown}
           AND (device, op_seq, n) IN (SELECT device, op_seq, n FROM card_files
             WHERE name = ?2 AND path IN ({marks}))
         ORDER BY ts DESC, device DESC, op_seq DESC, n"
    ))?;
    let values = [repo.to_owned(), name].into_iter().chain(paths);
    let mut rows = st.query(rusqlite::params_from_iter(values))?;
    let mut out = Vec::new();
    while out.len() < limit
        && let Some(r) = rows.next()?
    {
        let (agent, session) = (r.get(4)?, r.get::<_, Option<String>>(5)?);
        let modified = r.get::<_, Option<bool>>(19)?.unwrap_or(false);
        out.extend(read(r, raw, rules)?.map(|card| Filed {
            named: card.files_read.len() + card.files_modified.len(),
            card,
            session: session.map(|s| (agent, s)),
            modified,
            uid: None,
        }));
    }
    Ok(out)
}

/// X5 F4, F6, F7: claude-mem's note on a file from `cards`, the newest first as `by_file` gives
/// them, at `now` in `tz`, read on device `local`. None without a card, or when the file was
/// modified (`mtime`, unix ms) at or after the newest: they may tell of an older file.
pub fn file_note<Tz: chrono::TimeZone>(
    cards: &[Filed],
    mtime: i64,
    local: &str,
    now: i64,
    tz: &Tz,
) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let newest = cards.iter().map(|f| f.card.ts).max()?;
    if mtime >= newest {
        return None;
    }
    // One card a session, the newest; a card of no session stands alone. Then claude-mem's
    // rank, stable, so the newer first among equals: +2 for the file modified, +2 for at most
    // 3 files named, +1 for at most 8.
    let mut seen = std::collections::HashSet::new();
    let mut ranked: Vec<(u8, &Filed)> = cards
        .iter()
        .filter(|f| f.session.as_ref().is_none_or(|s| seen.insert(s.clone())))
        .map(|f| {
            let score = 2 * u8::from(f.modified)
                + match f.named {
                    0..=3 => 2,
                    4..=8 => 1,
                    _ => 0,
                };
            (score, f)
        })
        .collect();
    ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut shown: Vec<&Filed> = ranked.into_iter().take(15).map(|(_, f)| f).collect();
    shown.sort_by_key(|f| (f.card.ts, f.card.device.clone(), f.card.op_seq, f.card.n));
    let in_tz = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(tz));
    let mut out = vec![
        format!(
            "Current: {}",
            in_tz(now).map_or_else(String::new, |t| stamp(&t))
        ),
        "This file has prior observations — supplementary context follows. The Read result \
         below is the full requested section."
            .to_owned(),
        "- **Need details on a past observation?** get(ID) — ~300 tokens each.".to_owned(),
    ];
    let mut day = String::new();
    for f in shown {
        let c = &f.card;
        let Some(t) = in_tz(c.ts) else {
            continue;
        };
        let d = t.format("%b %-d, %Y").to_string();
        if d != day {
            out.push(format!("### {d}"));
            day = d;
        }
        let icon = c
            .kind
            .as_deref()
            .and_then(|k| NOTE_ICONS.iter().find(|(t, _)| *t == k))
            .map_or("❓", |(_, i)| *i);
        let title: String = c
            .row_title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect();
        let title = if title.is_empty() { "Untitled" } else { &title };
        out.push(format!("{} {} {icon} {title}", f.id(local), clock(&t)));
    }
    Some(out.join("\n"))
}

/// Parity row 3: the current cards removed from the page, the newest first after `before`, at most
/// `limit`, as Settings lists them to bring back (#661: page by page); read as every reader reads a
/// card. The boolean says another one exists, as `page` does.
pub(crate) fn removed(
    k: &Connection,
    raw: &Raw,
    before: Option<&Position>,
    limit: usize,
    rules: &Rules,
) -> Result<(Vec<Card>, bool)> {
    let sql = format!(
        "SELECT {COLUMNS} FROM cards WHERE replaced_by IS NULL AND hidden = 1
           AND (?1 IS NULL OR (ts, device, op_seq, n) < (?1, ?2, ?3, ?4))
         ORDER BY ts DESC, device DESC, op_seq DESC, n DESC"
    );
    let (ts, device, op_seq, n) = before.map_or((None, None, None, None), |(ts, d, s, n)| {
        (Some(*ts), Some(d.as_str()), Some(*s), Some(*n))
    });
    crate::hide::removed_page(
        k,
        "cards",
        &sql,
        rusqlite::params![ts, device, op_seq, n],
        limit,
        |r| read(r, raw, rules),
    )
}

/// A card's ID as `get` takes it, `<device>.<op seq>.<n>` or `<op seq>.<n>` of `local`, in parts:
/// the device, the op seq and the place.
pub(crate) fn id_parts<'a>(id: &'a str, local: &'a str) -> Option<(&'a str, i64, i64)> {
    let parts: Vec<&str> = id.trim().split('.').collect();
    let (device, op_seq, n) = match parts[..] {
        [op_seq, n] => (local, op_seq, n),
        [device, op_seq, n] => (device, op_seq, n),
        _ => return None,
    };
    Some((device, op_seq.parse().ok()?, n.parse().ok()?))
}

/// The current card an ID names (S3, S6), as `recent` would read it: `<op seq>.<n>` of this
/// device's, or `<device>.<op seq>.<n>`.
pub fn get(k: &Connection, raw: &Raw, id: &str, rules: &Rules) -> Result<Option<Card>> {
    let Some((device, op_seq, n)) = id_parts(id, raw.device()) else {
        return Ok(None);
    };
    if !crate::consumer::manifest::exists(k, "table", "cards")? {
        return Ok(None);
    }
    let shown = crate::hide::shown(k, "cards", "")?;
    let mut st = k.prepare(&format!(
        "SELECT {COLUMNS} FROM cards
         WHERE device = ?1 AND op_seq = ?2 AND n = ?3 AND replaced_by IS NULL {shown}"
    ))?;
    let mut rows = st.query(rusqlite::params![device, op_seq, n])?;
    match rows.next()? {
        Some(r) => read(r, raw, rules),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests;
