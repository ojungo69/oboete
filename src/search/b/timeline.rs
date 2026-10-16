//! docs/tools.md slice 2: the timeline in claude-mem's shape (T1-T5), around an anchor, around a
//! query's first hit, or the newest entries. The viewer keeps [`super::timeline`]'s list.

use super::*;
use crate::cards::Card;
use crate::turns::TurnSummary;

/// The most entries a side of an anchor shows, and the most of the newest (T1).
const MAX_DEPTH: usize = 50;
const MAX_NEWEST: usize = 100;
/// The hits a query's search keeps before its first is the anchor: the search tool's own default,
/// so that a pair search keeps together is not dropped for want of a second place (Greptile on
/// #450).
const QUERY_HITS: usize = 10;

/// What the timeline is around (T1).
pub enum Around {
    /// Any id `get` takes.
    Anchor(String),
    /// A search, as the caller asks it, whose first hit in Q2's order is the anchor.
    Query(Query),
    /// The newest entries, at most this many.
    Newest(usize),
}

/// One entry (T2), placed by its time, its kind and its stored order (T3).
struct Entry {
    ts: i64,
    /// Its stored id in parts, so that card 10 of a window follows card 9.
    order: (String, i64, i64),
    id: String,
    what: What,
}

enum What {
    Card(Box<Card>),
    Summary(Box<TurnSummary>),
    /// A prompt's text from its start, gated, and its repository.
    Prompt(String, Option<String>),
    /// A claim, an imported document or a session start.
    Other(Item),
}

impl What {
    /// Its place among entries of one time (T3): the prompt first, a summary after the cards of
    /// its time as the session start shows them.
    fn rank(&self) -> u8 {
        match self {
            What::Prompt(..) => 0,
            What::Other(_) => 1,
            What::Card(_) => 2,
            What::Summary(_) => 3,
        }
    }

    fn repo(&self) -> Option<&str> {
        match self {
            What::Card(c) => c.repo.as_deref(),
            What::Summary(s) => s.repo.as_deref(),
            What::Prompt(_, repo) => repo.as_deref(),
            What::Other(i) => i.repo.as_deref(),
        }
    }
}

/// The timeline's text (T4): `depths` entries before and after an anchor, or the newest; empty
/// when there is nothing to show without an anchor. An anchor or query of nothing is an error.
pub fn timeline_text<Tz: chrono::TimeZone>(
    home: &Path,
    repo: Option<&str>,
    cwd: &Path,
    around: Around,
    (before, after): (usize, usize),
    tz: &Tz,
) -> Result<String>
where
    Tz::Offset: std::fmt::Display,
{
    let (before, after) = (before.min(MAX_DEPTH), after.min(MAX_DEPTH));
    let Some(w) = window(home, repo, (around, true), (before, after))? else {
        return Ok(String::new());
    };
    if w.shown.is_empty() {
        return Ok(match w.anchor {
            Some(_) => format!(
                "No context found around anchor ({before} records before, {after} records after)"
            ),
            None => String::new(),
        });
    }
    let marked_entry = w.shown.iter().find(|e| Some(&e.id) == w.marked.as_ref());
    let mut out = header(
        w.anchor.as_deref(),
        w.query.as_deref(),
        marked_entry,
        w.shown.len(),
        (before, after),
    );
    render(
        &mut out,
        &w.shown,
        w.marked.as_deref(),
        cwd,
        (&w.local, repo.is_none()),
        tz,
    );
    Ok(out.join("\n"))
}

/// One entry around an anchor as mem_search's context takes it (docs/mem-search.md): what `get`
/// takes, claude-mem's kind (`observation`, `summary` or `prompt`) and type, its repository, time
/// and title, gated.
#[derive(Debug, Clone)]
pub struct Neighbor {
    pub id: String,
    pub kind: &'static str,
    pub kind_of: Option<String>,
    pub repo: Option<String>,
    pub ts: i64,
    pub title: String,
}

/// The entries `before` and `after` `anchor` that a timeline of `repo` shows, in its order; a
/// session start, which claude-mem has no kind for, is left out before the depths are counted
/// (Codex on #527). None around an anchor gone since it was shown (forgotten, or hidden as `get`
/// hides it).
pub fn near(
    home: &Path,
    repo: Option<&str>,
    anchor: &str,
    (before, after): (usize, usize),
) -> Result<Vec<Neighbor>> {
    if super::get(home, anchor)?.is_none() {
        return Ok(Vec::new());
    }
    let around = Around::Anchor(anchor.to_owned());
    let Some(w) = window(
        home,
        repo,
        (around, false),
        (before.min(MAX_DEPTH), after.min(MAX_DEPTH)),
    )?
    else {
        return Ok(Vec::new());
    };
    Ok(w.shown
        .iter()
        .map(|e| {
            let (kind, kind_of) = match &e.what {
                What::Card(c) => ("observation", c.kind.clone()),
                What::Summary(_) => ("summary", None),
                What::Prompt(..) => ("prompt", None),
                What::Other(i) => match (i.class.as_str(), i.kind.as_str()) {
                    ("imported", k @ ("summary" | "prompt")) => {
                        (if k == "summary" { "summary" } else { "prompt" }, None)
                    }
                    (_, k) => ("observation", Some(k.to_owned())),
                },
            };
            Neighbor {
                id: e.id.clone(),
                kind,
                kind_of,
                repo: e.what.repo().map(str::to_owned),
                ts: e.ts,
                title: title(e),
            }
        })
        .collect())
}

/// What a timeline shows (T2-T4) before it is rendered: its entries, what they are around, and
/// the anchor's own entry when it has one. `None` when the stores are not made yet and there is no
/// anchor.
struct Window {
    shown: Vec<Entry>,
    anchor: Option<String>,
    query: Option<String>,
    marked: Option<String>,
    local: String,
}

fn window(
    home: &Path,
    repo: Option<&str>,
    (around, starts): (Around, bool),
    (before, after): (usize, usize),
) -> Result<Option<Window>> {
    // A query's anchor is found first, under search's own fence: a reader opens raw.db before it
    // takes the fence (#439).
    let (anchor, query, limit) = match around {
        Around::Query(q) => {
            let q = Query {
                limit: QUERY_HITS,
                ..q
            };
            let Some(hit) = super::query(home, &q)?.hits.into_iter().next() else {
                anyhow::bail!("nothing matches {:?} to anchor on", q.text);
            };
            (Some(hit.key), Some(q.text), 0)
        }
        Around::Anchor(id) => (Some(id), None, 0),
        Around::Newest(limit) => (None, None, limit.min(MAX_NEWEST)),
    };
    let Some((raw, k)) = stores(home)? else {
        if let Some(id) = anchor {
            anyhow::bail!("no document {id} to anchor on");
        }
        return Ok(None);
    };
    // Milestone 5 D5 (#439): no forget commits until the answer is built.
    let _read = crate::dispatch::reading(home, crate::db::OPEN_WRITE_WAIT)?;
    let rules = redact::Rules::load(home)?;
    let local = raw.device().to_owned();
    let (at, marked) = match &anchor {
        Some(id) => anchored(&raw, &k, id, &rules)?,
        None => (i64::MAX, None),
    };
    // Without an anchor, the newest `limit` are the ones before the end of time.
    let depths = match anchor {
        Some(_) => (before, after),
        None => (limit, 0),
    };
    let window = |read: &dyn Fn(bool, usize) -> Result<Vec<Entry>>| gather(read, at, depths);
    let mut entries = window(&|older, n| {
        Ok(others(&raw, &k, repo, (older, starts), at, "", n)?
            .into_iter()
            .map(|i| Entry {
                ts: i.when,
                order: (i.key.clone(), 0, 0),
                id: i.key.clone(),
                what: What::Other(i),
            })
            .collect())
    })?;
    entries.extend(window(&|older, n| {
        Ok(crate::cards::around(&k, &raw, repo, at, older, n, &rules)?
            .into_iter()
            .map(|c| Entry {
                ts: c.ts,
                order: (c.device.clone(), c.op_seq, c.n),
                id: c.id(&local),
                what: What::Card(Box::new(c)),
            })
            .collect())
    })?);
    entries.extend(window(&|older, n| {
        Ok(crate::turns::around(&k, &raw, repo, at, older, n, &rules)?
            .into_iter()
            .map(|s| Entry {
                ts: s.ts,
                order: (s.device.clone(), s.op_seq, 0),
                id: s.id(&local),
                what: What::Summary(Box::new(s)),
            })
            .collect())
    })?);
    entries.extend(window(&|older, n| prompts(&raw, &k, repo, at, older, n))?);
    entries.sort_by(|a, b| (a.ts, a.what.rank(), &a.order).cmp(&(b.ts, b.what.rank(), &b.order)));
    let shown: Vec<Entry> = if anchor.is_some() {
        // An anchor with no entry of its own (a record of another kind) splits at its time.
        let at_index = marked
            .as_ref()
            .and_then(|m| entries.iter().position(|e| &e.id == m));
        let (from, to) = match at_index {
            Some(i) => (i.saturating_sub(before), i + 1 + after),
            None => {
                let split = entries.partition_point(|e| e.ts <= at);
                (split.saturating_sub(before), split + after)
            }
        };
        entries.drain(from..to.min(entries.len())).collect()
    } else {
        let from = entries.len().saturating_sub(limit);
        entries.split_off(from)
    };
    Ok(Some(Window {
        shown,
        anchor,
        query,
        marked,
        local,
    }))
}

/// One kind's entries (T2) for the window, read by `read(older, n)` (at most `n` on that side of
/// `at`, the nearest first): all of the anchor's time, which may sit on either side of it in the
/// order (T3), and `before` older ones, then `after` newer ones. The oldest time read may hold
/// more entries than were read, and in another order than the timeline's (claims of one time come
/// by key, the older side too), so the older ones count only from the times before it, reading on
/// until they are enough or the kind has no more (Greptile on #450).
fn gather(
    read: &dyn Fn(bool, usize) -> Result<Vec<Entry>>,
    at: i64,
    (before, after): (usize, usize),
) -> Result<Vec<Entry>> {
    if at == i64::MAX && before == 0 {
        return Ok(Vec::new());
    }
    let mut n = before + after + 1;
    let mut out = loop {
        let got = read(true, n)?;
        let oldest = got.iter().map(|e| e.ts).min();
        let whole = got.iter().filter(|e| e.ts < at && Some(e.ts) != oldest);
        // One older at least, so that the anchor's time is read whole.
        if got.len() < n || whole.count() >= before.max(1) {
            break got;
        }
        n = n.saturating_mul(2);
    };
    if after > 0 {
        out.extend(read(false, after)?);
    }
    Ok(out)
}

/// The answer's first lines (T4): what it is around, the query's anchor (its entry, when it has
/// one), and the window; then a blank line.
fn header(
    anchor: Option<&str>,
    query: Option<&str>,
    marked: Option<&Entry>,
    items: usize,
    (before, after): (usize, usize),
) -> Vec<String> {
    let mut out = Vec::new();
    match (anchor, query) {
        (Some(id), Some(q)) => {
            out.push(format!("# Timeline for query: \"{}\"", redact::outbound(q)));
            let (id, name) = match marked {
                Some(e) => (shown_id(e), title(e)),
                None => (redact::outbound(id), "Unknown".to_owned()),
            };
            out.push(format!("**Anchor:** {id} - {name}"));
        }
        (Some(id), None) => out.push(format!(
            "# Timeline around anchor: {}",
            redact::outbound(id)
        )),
        (None, _) => out.push(format!("# Timeline: the newest {items}")),
    }
    if anchor.is_some() {
        out.push(format!(
            "**Window:** {before} records before -> {after} records after | **Items:** {items}"
        ));
    }
    out.push(String::new());
    out
}

/// The anchor's time and, when it is an entry (T2), that entry's id as the timeline gives it; a
/// record of another kind than a prompt or a session start has none (T3).
fn anchored(
    raw: &Raw,
    k: &Connection,
    id: &str,
    rules: &redact::Rules,
) -> Result<(i64, Option<String>)> {
    let local = raw.device();
    if let Some(c) = crate::cards::get(k, raw, id, rules)? {
        return Ok((c.ts, Some(c.id(local))));
    }
    if let Some(s) = crate::turns::get(k, raw, id, rules)? {
        return Ok((s.ts, Some(s.id(local))));
    }
    Ok(match named(raw, k, id)? {
        None => anyhow::bail!("no document {id} to anchor on"),
        Some(Named::Claims(uids)) => {
            anyhow::bail!("{} claims start with {id}: give more of the id", uids.len())
        }
        Some(Named::Claim(uid)) => (
            k.query_row(
                "SELECT valid_from FROM active WHERE uid = ?1",
                [&uid],
                |r| r.get(0),
            )?,
            Some(uid),
        ),
        Some(Named::Imported(uid)) => (
            k.query_row(
                "SELECT ts FROM imported WHERE uid = ?1 ORDER BY rowid DESC LIMIT 1",
                [&uid],
                |r| r.get(0),
            )?,
            Some(uid),
        ),
        Some(Named::Record(key, e)) => (
            e.ts,
            matches!(e.kind.as_str(), "prompt" | "start").then_some(key),
        ),
        // docs/retention.md: its time anchors the timeline; it has no entry of its own.
        Some(Named::Expired(_, e)) => (e.ts, None),
    })
}

/// Prompt records on one side of `at`, as [`others`] reads its entries, through the record leg's
/// reader: raw.db's text, gated, a record a tombstone hides taking no place (T2).
fn prompts(
    raw: &Raw,
    k: &Connection,
    repo: Option<&str>,
    at: i64,
    older: bool,
    limit: usize,
) -> Result<Vec<Entry>> {
    let side = if older {
        "ts <= ?2 ORDER BY ts DESC, device DESC, seq DESC"
    } else {
        "ts > ?2 ORDER BY ts, device, seq"
    };
    let mut st = k.prepare(&format!(
        "SELECT device || ':' || seq FROM raw_docs
         WHERE kind = 'prompt' AND (?1 IS NULL OR repo = ?1) AND {side} LIMIT ?3 OFFSET ?4"
    ))?;
    let mut out = Vec::new();
    for page in 0.. {
        let keys: Vec<String> = st
            .query_map(
                params![
                    repo,
                    at,
                    super::super::sql_limit(limit),
                    super::super::sql_limit(limit.saturating_mul(page))
                ],
                |r| r.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?;
        for h in super::super::raw_rows(Some(raw), k, &keys, "")? {
            out.push(Entry {
                ts: h.ts,
                id: format!("{}:{}", h.device, h.seq),
                order: (h.device, h.seq, 0),
                what: What::Prompt(h.snippet, h.repo),
            });
        }
        if keys.len() < limit || out.len() >= limit {
            break;
        }
    }
    out.truncate(limit);
    Ok(out)
}

/// An entry's id as the timeline's lines show it: a claim's first 12 characters, as search's.
fn shown_id(e: &Entry) -> String {
    match &e.what {
        What::Other(i) if i.class == "claim" => super::id(&i.key),
        _ => redact::outbound(&e.id),
    }
}

/// An entry's title, for a query's anchor line.
fn title(e: &Entry) -> String {
    match &e.what {
        What::Card(c) => untitled(&c.row_title).to_owned(),
        What::Summary(s) => request(s).to_owned(),
        What::Prompt(text, _) => first(text),
        What::Other(i) => i.text.clone(),
    }
}

fn untitled(title: &str) -> &str {
    match title.trim() {
        "" => "Untitled",
        t => t,
    }
}

fn request(s: &TurnSummary) -> &str {
    match s.row.trim() {
        "" => "Session summary",
        r => r,
    }
}

/// A prompt's first 100 characters, `...` after a longer one (claude-mem's).
fn first(text: &str) -> String {
    if text.chars().count() > 100 {
        format!("{}...", text.chars().take(100).collect::<String>())
    } else {
        text.to_owned()
    }
}

/// A card's first modified file, else its first read file, else `General`, from `cwd` where it is
/// under it (claude-mem's `extractFirstFile`).
fn first_file(c: &Card, cwd: &Path) -> String {
    let Some(file) = c.files_modified.first().or(c.files_read.first()) else {
        return "General".into();
    };
    match Path::new(file).strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => file.clone(),
    }
}

/// `shown`, by day in `tz` (claude-mem's `renderTimeline`): cards in tables by file, a new table
/// when the file changes and after any other entry; the anchor's line marked. With every
/// repository (`all`), each line and table names its repository, as the list did (Greptile on
/// #450).
fn render<Tz: chrono::TimeZone>(
    out: &mut Vec<String>,
    shown: &[Entry],
    marked: Option<&str>,
    cwd: &Path,
    (local, all): (&str, bool),
    tz: &Tz,
) where
    Tz::Offset: std::fmt::Display,
{
    let place = |e: &Entry| match (all, e.what.repo()) {
        (true, Some(repo)) => format!(" [{}]", redact::outbound(repo)),
        _ => String::new(),
    };
    let mark = |e: &Entry| {
        if Some(e.id.as_str()) == marked {
            " <- **ANCHOR**"
        } else {
            ""
        }
    };
    let (mut day, mut last) = (String::new(), String::new());
    let mut table: Option<String> = None;
    for e in shown {
        let Some(t) = chrono::DateTime::from_timestamp_millis(e.ts).map(|t| t.with_timezone(tz))
        else {
            continue;
        };
        let d = t.format("%b %-d, %Y").to_string();
        if d != day {
            if table.take().is_some() {
                out.push(String::new());
            }
            out.push(format!("### {d}"));
            out.push(String::new());
            day = d;
        }
        let when = t.format("%b %-d, %-I:%M %p");
        if let What::Card(c) = &e.what {
            let file = format!("**{}**{}", redact::outbound(&first_file(c, cwd)), place(e));
            if table.as_ref() != Some(&file) {
                if table.is_some() {
                    out.push(String::new());
                }
                out.push(file.clone());
                out.push("| ID | Time | T | Title | Tokens |".into());
                out.push("|----|------|---|-------|--------|".into());
                table = Some(file);
                last.clear();
            }
            let time = t.format("%-I:%M %p").to_string();
            let shown_time = if time == last {
                "\"".into()
            } else {
                time.clone()
            };
            last = time;
            out.push(format!(
                "| {} | {shown_time} | {} | {}{} | ~{} |",
                c.id(local),
                crate::cards::icon(c.kind.as_deref()),
                // A `|` in a title would end its cell (Codex and Greptile on #450).
                untitled(&c.row_title).replace('|', "\\|"),
                mark(e),
                card_text(c, local).chars().count().div_ceil(4)
            ));
            continue;
        }
        if table.take().is_some() {
            out.push(String::new());
        }
        match &e.what {
            What::Summary(s) => out.push(format!(
                "**🎯 {}** {} ({when}){}{}",
                s.id(local),
                request(s),
                place(e),
                mark(e)
            )),
            What::Prompt(text, _) => {
                out.push(format!(
                    "**💬 Prompt {}** ({when}){}{}",
                    e.id,
                    place(e),
                    mark(e)
                ));
                out.push(format!("> {}", first(text)));
            }
            What::Other(i) => out.push(format!(
                "**{}** {} {} ({when}){}{}",
                shown_id(e),
                i.kind,
                i.text,
                place(e),
                mark(e)
            )),
            What::Card(_) => {}
        }
        out.push(String::new());
    }
    if table.is_some() {
        out.push(String::new());
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::Store;
    use super::*;
    use serde_json::json;

    const R: &str = "github.com/o/r";
    /// Nov 14, 2023, 10:13 PM UTC.
    const DAY: i64 = 1_700_000_000_000;

    /// A home of two days: a decision the day before; a prompt, the three cards and the summary
    /// of its window; two later decisions, each said in a prompt.
    struct Days {
        s: Store,
        older: (String, String),
        prompt: String,
        cards: Vec<String>,
        summary: String,
        later: Vec<(String, String)>,
    }

    /// A decision the user said in a prompt at `ts`: the prompt's key and the claim's uid.
    fn decided(s: &mut Store, ts: i64, text: &str) -> (String, String) {
        let seq = s.said("s", R, ts, text);
        let uid = s.claim(seq, text, ("decision", "decided", "user"), &[]);
        (s.key(seq), uid)
    }

    fn days() -> Days {
        let mut s = Store::new();
        let older = decided(&mut s, DAY - 86_400_000, "Use the trigram tokenizer.");
        let seq = s.said("work", R, DAY, "Fix the Quartz parser in the reader.");
        let cards = s.cards(
            seq,
            seq,
            json!([
                {"type": "bugfix", "title": "Quartz parser", "files_modified": ["/w/src/reader.rs"]},
                {"type": "discovery", "title": "Reader buffers", "files_read": ["/w/src/reader.rs"]},
                {"type": "change", "title": "Docs note"},
            ]),
            false,
        );
        let summary = s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY + 3_000, "from": seq, "through": seq, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Quartz progress"}, "skipped": false}));
        let later = vec![
            decided(&mut s, DAY + 60_000, "Keep the reader streaming."),
            decided(&mut s, DAY + 120_000, "Ship on Friday."),
        ];
        s.run();
        let prompt = s.key(seq);
        Days {
            s,
            older,
            prompt,
            cards,
            summary,
            later,
        }
    }

    fn text(s: &Store, around: Around, depths: (usize, usize)) -> Result<String> {
        timeline_text(
            s.home.path(),
            Some(R),
            Path::new("/w"),
            around,
            depths,
            &chrono::Utc,
        )
    }

    /// The text with each card row's read cost as `~N` (Q7's count, which slice 1 tests).
    fn costless(text: &str) -> String {
        text.lines()
            .map(|l| match l.rfind("| ~") {
                Some(i) if l.starts_with("| ") && l.ends_with(" |") => format!("{}| ~N |", &l[..i]),
                _ => l.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// docs/tools.md slice 2, test 1 (T2-T4): around a card, the cards, the summary and the prompt
    /// of its window and the claim of the day before, by day and by file, the anchor marked; a side
    /// with fewer entries than its depth has fewer.
    #[test]
    fn a_timeline_around_a_card_shows_its_window_by_day_and_file() {
        let d = days();
        let got = text(&d.s, Around::Anchor(d.cards[0].clone()), (2, 3)).unwrap();
        let expected = format!(
            "# Timeline around anchor: {card0}
**Window:** 2 records before -> 3 records after | **Items:** 6

### Nov 13, 2023

**{older}** decision decided Use the trigram tokenizer. (Nov 13, 10:13 PM)

### Nov 14, 2023

**💬 Prompt {prompt}** (Nov 14, 10:13 PM)
> Fix the Quartz parser in the reader.

**src/reader.rs**
| ID | Time | T | Title | Tokens |
|----|------|---|-------|--------|
| {card0} | 10:13 PM | ● | Quartz parser <- **ANCHOR** | ~N |
| {card1} | \" | ○ | Reader buffers | ~N |

**General**
| ID | Time | T | Title | Tokens |
|----|------|---|-------|--------|
| {card2} | 10:13 PM | ✓ | Docs note | ~N |

**🎯 {summary}** Quartz progress (Nov 14, 10:13 PM)",
            card0 = d.cards[0],
            card1 = d.cards[1],
            card2 = d.cards[2],
            older = &d.older.1[..12],
            prompt = d.prompt,
            summary = d.summary,
        );
        assert_eq!(costless(&got), expected, "{got}");
        // Each row's cost is its card's.
        assert!(
            got.lines()
                .filter(|l| l.contains(" | ~"))
                .all(|l| !l.contains("~0 "))
        );
        // The newest claim: one entry before it (its prompt), none after.
        let (prompt, uid) = &d.later[1];
        let got = text(&d.s, Around::Anchor(uid[..12].into()), (1, 3)).unwrap();
        assert_eq!(
            got,
            format!(
                "# Timeline around anchor: {short}
**Window:** 1 records before -> 3 records after | **Items:** 2

### Nov 14, 2023

**💬 Prompt {prompt}** (Nov 14, 10:15 PM)
> Ship on Friday.

**{short}** decision decided Ship on Friday. (Nov 14, 10:15 PM) <- **ANCHOR**
",
                short = &uid[..12]
            )
        );
    }

    /// T3: an anchor among more cards of its own time than its depths keeps its place, the cards of
    /// one window (which share their time) keep their order past ten, and a summary of their time
    /// follows them; T4: a prompt shows its first 100 characters.
    #[test]
    fn an_anchor_among_many_cards_of_its_time_keeps_its_place() {
        let mut s = Store::new();
        let long = format!("{}end", "Sweep the parser. ".repeat(8));
        let seq = s.said("work", R, DAY, &long);
        let steps = (0..12)
            .map(|n| json!({"type": "change", "title": format!("Step {n}")}))
            .collect();
        let cards = s.cards(seq, seq, serde_json::Value::Array(steps), false);
        let summary = s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY, "from": seq, "through": seq, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Sweep done"}, "skipped": false}));
        s.run();
        let rows = |anchor: usize, depths| -> Vec<String> {
            let got = text(&s, Around::Anchor(cards[anchor].clone()), depths).unwrap();
            got.lines()
                .filter(|l| {
                    ["**💬", "**🎯", "> "].iter().any(|p| l.starts_with(p)) || l.contains(" | ✓ | ")
                })
                .map(|l| l.split(" | ~").next().unwrap().to_owned())
                .collect()
        };
        let card = |n: usize, time: &str, anchor: &str| {
            format!("| {} | {time} | ✓ | Step {n}{anchor}", cards[n])
        };
        let mark = " <- **ANCHOR**";
        assert_eq!(
            rows(0, (1, 2)),
            [
                format!("**💬 Prompt {}** (Nov 14, 10:13 PM)", s.key(seq)),
                format!("> {}...", &long[..100]),
                card(0, "10:13 PM", mark),
                card(1, "\"", ""),
                card(2, "\"", ""),
            ]
        );
        assert_eq!(
            rows(0, (0, 2)),
            [
                card(0, "10:13 PM", mark),
                card(1, "\"", ""),
                card(2, "\"", "")
            ]
        );
        assert_eq!(
            rows(10, (2, 1)),
            [
                card(8, "10:13 PM", ""),
                card(9, "\"", ""),
                card(10, "\"", mark),
                card(11, "\"", ""),
            ]
        );
        assert_eq!(
            rows(11, (1, 1)),
            [
                card(10, "10:13 PM", ""),
                card(11, "\"", mark),
                format!("**🎯 {summary}** Sweep done (Nov 14, 10:13 PM)"),
            ]
        );
    }

    /// T1: depths past 50 are cut to it, however large (Codex and Greptile on #450).
    #[test]
    fn depths_past_their_cap_are_cut_to_it() {
        let d = days();
        let got = text(
            &d.s,
            Around::Anchor(d.cards[0].clone()),
            (1_000, usize::MAX),
        )
        .unwrap();
        assert_eq!(
            got.lines().nth(1),
            Some("**Window:** 50 records before -> 50 records after | **Items:** 11")
        );
    }

    /// T3 (Greptile on #450): of more entries of one time than are asked for, the timeline keeps
    /// the nearest: claims of one time come from their reader by key, the other way round.
    #[test]
    fn of_one_times_entries_the_nearest_are_kept() {
        let mut s = Store::new();
        let mut uids: Vec<String> = ["Alpha rule.", "Bravo rule.", "Charlie rule."]
            .iter()
            .map(|text| decided(&mut s, DAY, text).1)
            .collect();
        let seq = s.said("work", R, DAY + 1_000, "Later prompt.");
        s.run();
        uids.sort();
        let last = &uids[2][..12];
        let newest = text(&s, Around::Newest(2), (3, 3)).unwrap();
        assert!(
            newest.contains(last) && !newest.contains(&uids[0][..12]),
            "{newest}"
        );
        let got = text(&s, Around::Anchor(s.key(seq)), (1, 0)).unwrap();
        assert!(got.contains(last) && !got.contains(&uids[1][..12]), "{got}");
    }

    /// T1 (Greptile on #450): a query's anchor is search's first hit when that hit is one of a
    /// pair search keeps together (D2), which one place would drop.
    #[test]
    fn a_query_anchors_on_the_first_of_a_pair() {
        let mut s = Store::new();
        let quote = "Indent widgets with tabs.";
        let source = s.said("work", R, DAY, quote);
        let earlier = s.claim(source, quote, ("preference", "decided", "user"), &[]);
        let later = s.decided(
            R,
            DAY + 1_000,
            "Indent widgets with spaces, never tabs.",
            &[&earlier],
        );
        s.run();
        let q = Query {
            text: "indent widgets".into(),
            caller: Some(R.into()),
            raw: RawArm::Off,
            ..Default::default()
        };
        let got = text(&s, Around::Query(q), (1, 1)).unwrap();
        assert!(
            got.contains(&format!("**Anchor:** {} - ", &later[..12])),
            "{got}"
        );
    }

    /// T4 (Codex and Greptile on #450): a `|` in a card's title stays in its cell.
    #[test]
    fn a_pipe_in_a_title_stays_in_its_cell() {
        let mut s = Store::new();
        let seq = s.said("work", R, DAY, "Pipes.");
        let card = s.cards(
            seq,
            seq,
            json!([{"type": "change", "title": "Support A | B"}]),
            false,
        )[0]
        .clone();
        s.run();
        let got = text(&s, Around::Anchor(card.clone()), (0, 0)).unwrap();
        assert!(
            got.contains(&format!(
                "| {card} | 10:13 PM | ✓ | Support A \\| B <- **ANCHOR** | ~"
            )),
            "{got}"
        );
    }

    /// T4 (Greptile on #450): with every repository, each line and table names its repository.
    #[test]
    fn every_repository_names_each_entrys_repository() {
        let d = days();
        let got = timeline_text(
            d.s.home.path(),
            None,
            Path::new("/w"),
            Around::Anchor(d.cards[0].clone()),
            (2, 3),
            &chrono::Utc,
        )
        .unwrap();
        let named = format!(" [{R}]");
        for line in [
            format!(
                "**{}** decision decided Use the trigram tokenizer. (Nov 13, 10:13 PM){named}",
                &d.older.1[..12]
            ),
            format!("**💬 Prompt {}** (Nov 14, 10:13 PM){named}", d.prompt),
            format!("**src/reader.rs**{named}"),
            format!(
                "**🎯 {}** Quartz progress (Nov 14, 10:13 PM){named}",
                d.summary
            ),
        ] {
            assert!(got.lines().any(|l| l == line), "{line}: {got}");
        }
        let one = text(&d.s, Around::Anchor(d.cards[0].clone()), (2, 3)).unwrap();
        assert!(!one.contains(&named), "{one}");
    }

    /// Test 2 (T1): a query's anchor is search's first hit, named in the header.
    #[test]
    fn a_query_anchors_on_searchs_first_hit() {
        let d = days();
        let q = Query {
            text: "buffers".into(),
            caller: Some(R.into()),
            raw: RawArm::Below,
            limit: 1,
            ..Default::default()
        };
        let got = text(&d.s, Around::Query(q.clone()), (1, 1)).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines[0], "# Timeline for query: \"buffers\"");
        assert_eq!(
            lines[1],
            format!("**Anchor:** {} - Reader buffers", d.cards[1])
        );
        assert_eq!(
            lines[2],
            "**Window:** 1 records before -> 1 records after | **Items:** 3"
        );
        assert!(
            got.contains(&format!(
                "| {} | \" | ○ | Reader buffers <- **ANCHOR** |",
                d.cards[1]
            )),
            "{got}"
        );
        let none = Query {
            text: "zeppelin".into(),
            ..q
        };
        let err = text(&d.s, Around::Query(none), (1, 1)).unwrap_err();
        assert_eq!(err.to_string(), "nothing matches \"zeppelin\" to anchor on");
    }

    /// Test 3 (T2, Q3): a hidden card (K4), a skipped summary and a forgotten claim are no entries
    /// and take no place; an anchor on the hidden card is an error, as `get` reports it.
    #[test]
    fn hidden_items_take_no_place() {
        let mut d = days();
        let gone = d.s.said("gone", R, DAY + 1_000, "Old notes.");
        let hidden = d.s.cards(
            gone,
            gone,
            json!([{"type": "bugfix", "title": "Hidden card"}]),
            false,
        )[0]
        .clone();
        d.s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY + 2_000, "from": gone, "through": gone, "read": [], "goals": [],
            "removed": [], "fields": {"request": "Skipped turn"}, "skipped": true}));
        d.s.run();
        let device = d.s.raw.device().to_owned();
        d.s.raw
            .append_tombstone(crate::raw::Target::Record { device, seq: gone })
            .unwrap();
        let home = d.s.home.path();
        let target = crate::forget::Target::parse_uid(&d.later[0].1).unwrap();
        crate::forget::start(home, &crate::forget::preview(home, target).unwrap()).unwrap();
        d.s.run();
        // Before the summary, the card before the hidden ones; after it, the prompt that said the
        // forgotten decision (the record stays, D5) and the next prompt, not the decision.
        let got = text(&d.s, Around::Anchor(d.summary.clone()), (1, 2)).unwrap();
        for gone in [
            "Hidden card",
            "Skipped turn",
            "Old notes",
            &d.later[0].1[..12],
        ] {
            assert!(!got.contains(gone), "{gone}: {got}");
        }
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(
            lines[1],
            "**Window:** 1 records before -> 2 records after | **Items:** 4"
        );
        let shown = [
            format!("| {} |", d.cards[2]),
            format!("**💬 Prompt {}**", d.later[0].0),
            format!("**💬 Prompt {}**", d.later[1].0),
        ];
        for line in shown {
            assert!(got.contains(&line), "{line}: {got}");
        }
        for anchor in [hidden, d.later[0].1.clone()] {
            let err = text(&d.s, Around::Anchor(anchor.clone()), (1, 1)).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("no document {anchor} to anchor on")
            );
        }
    }

    /// Test 3 (spec 6.4): a rule added after the entries were written masks their lines, the
    /// card's file among them. In a child process: egress reads the rules of the home
    /// `redact::set_home` names, the process's.
    #[test]
    fn a_rule_added_after_masks_every_line() {
        const HOME: &str = "OBOETE_TEST_TIMELINE_RULE_HOME";
        const SECRET: &str = "QUILL-7731";
        if let Ok(home) = std::env::var(HOME) {
            let home = std::path::PathBuf::from(home);
            crate::redact::set_home(&home).unwrap();
            let got = timeline_text(
                &home,
                Some(R),
                Path::new("/w"),
                Around::Newest(20),
                (3, 3),
                &chrono::Utc,
            )
            .unwrap();
            assert!(!got.contains(SECRET), "{got}");
            // The two prompts, the claim, the card's file and row, the summary.
            assert_eq!(got.matches("[REDACTED]").count(), 6, "{got}");
            return;
        }
        let mut s = Store::new();
        decided(&mut s, DAY, &format!("Use {SECRET} for the parser."));
        let seq = s.said("work", R, DAY + 1_000, &format!("Fix {SECRET} now."));
        s.cards(
            seq,
            seq,
            json!([{"type": "bugfix", "title": format!("Card {SECRET}"),
                "files_modified": [format!("/w/src/{SECRET}.rs")]}]),
            false,
        );
        s.turn(json!({"agent": "claude", "session": "work", "repo": R,
            "ts": DAY + 3_000, "from": seq, "through": seq, "read": [], "goals": [],
            "removed": [], "fields": {"request": format!("Turn {SECRET}")}, "skipped": false}));
        s.run();
        std::fs::write(
            s.home.path().join("config.toml"),
            format!("[redaction]\nextra_rules = [{{ id = \"quill\", regex = '{SECRET}' }}]\n"),
        )
        .unwrap();
        let name = "search::b::timeline::tests::a_rule_added_after_masks_every_line";
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name])
            .env(HOME, s.home.path())
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{said}");
        assert!(said.contains("1 passed"), "{said}");
    }

    /// Test 4 (T1, T4): without an anchor, the newest entries, no window line; the viewer's list
    /// keeps today's entries.
    #[test]
    fn without_an_anchor_the_newest_entries() {
        let d = days();
        let got = text(&d.s, Around::Newest(3), (3, 3)).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines[0], "# Timeline: the newest 3");
        assert_eq!(lines[1], "");
        assert!(got.contains(&d.later[0].1[..12]) && got.contains(&d.later[1].0));
        assert!(!got.contains("Window") && !got.contains("Quartz"), "{got}");
        assert_eq!(text(&Store::new(), Around::Newest(3), (3, 3)).unwrap(), "");
        let listed = super::super::timeline(d.s.home.path(), Some(R), None, None, 20).unwrap();
        let classes: Vec<&str> = listed.iter().map(|i| i.class.as_str()).collect();
        assert_eq!(classes, ["claim"; 3]);
    }

    /// Test 5 (T3): a record anchor of another kind than a prompt gives the entries around its
    /// time, none marked, the entries of its own time before it; a query whose first hit it is
    /// names it; so does a card of another repository than the one asked; an anchor of nothing is
    /// an error.
    #[test]
    fn a_record_anchor_that_is_no_prompt_splits_at_its_time() {
        let mut d = days();
        let tool = d.s.event(
            "assistant",
            "work",
            (R, "main"),
            DAY,
            json!({"message": "Parser verified."}),
        );
        d.s.run();
        let key = d.s.key(tool);
        let got = text(&d.s, Around::Anchor(key.clone()), (1, 1)).unwrap();
        let q = Query {
            text: "verified".into(),
            caller: Some(R.into()),
            raw: RawArm::Below,
            limit: 1,
            ..Default::default()
        };
        let found = text(&d.s, Around::Query(q), (1, 1)).unwrap();
        assert!(
            found.starts_with(&format!(
                "# Timeline for query: \"verified\"\n**Anchor:** {key} - Unknown\n"
            )),
            "{found}"
        );
        // The anchor's window, under the query's two header lines.
        let window = |t: &str, header: usize| {
            costless(t)
                .lines()
                .skip(header)
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(window(&found, 2), window(&got, 1));
        assert!(!got.contains("ANCHOR"), "{got}");
        assert!(
            got.starts_with(&format!("# Timeline around anchor: {key}\n")),
            "{got}"
        );
        assert!(got.contains("**Items:** 2"), "{got}");
        assert!(got.contains(&format!("| {} |", d.cards[2])), "{got}");
        assert!(
            got.contains(&format!("**🎯 {}** Quartz progress", d.summary)),
            "{got}"
        );
        let elsewhere = d.s.said("other", "github.com/o/other", DAY, "Elsewhere.");
        let card = d.s.cards(
            elsewhere,
            elsewhere,
            json!([{"type": "change", "title": "Other repository"}]),
            false,
        )[0]
        .clone();
        d.s.run();
        let other = text(&d.s, Around::Anchor(card.clone()), (1, 1)).unwrap();
        assert!(
            !other.contains("Other repository") && !other.contains("ANCHOR"),
            "{other}"
        );
        assert_eq!(
            window(&other, 1),
            window(&got, 1),
            "the same entries around the same time"
        );
        let err = text(&d.s, Around::Anchor("o9".into()), (1, 1)).unwrap_err();
        assert_eq!(err.to_string(), "no document o9 to anchor on");
    }
}
