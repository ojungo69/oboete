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
/// `prompts`' query, of the repository `?1` when `named`, else of every one. N5: a repository's by
/// the consumer's index `raw_docs_prompts`, in the side's order: the scan of every record took
/// 0.85 s of a cold lookup's context on this PC's store (2026-10-20).
fn prompts_sql(named: bool, older: bool) -> String {
    let side = if older {
        "ts <= ?2 ORDER BY ts DESC, device DESC, seq DESC"
    } else {
        "ts > ?2 ORDER BY ts, device, seq"
    };
    let of = if named { "repo = ?1" } else { "?1 IS NULL" };
    format!(
        "SELECT device || ':' || seq FROM raw_docs
         WHERE kind = 'prompt' AND {of} AND {side} LIMIT ?3 OFFSET ?4"
    )
}

fn prompts(
    raw: &Raw,
    k: &Connection,
    repo: Option<&str>,
    at: i64,
    older: bool,
    limit: usize,
) -> Result<Vec<Entry>> {
    let mut st = k.prepare(&prompts_sql(repo.is_some(), older))?;
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
mod tests;
