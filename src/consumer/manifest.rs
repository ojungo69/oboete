//! Milestone 2 Task 9 (spec 4.9, plan D12): the manifest of each checkout (repo x branch x
//! device), rebuilt from raw when the checkout gets records. Facts point at records by
//! (device, seq); a build reads the few bodies it shows back through `Raw::after`, so what a
//! tombstone hides there (Task 7) is hidden here too. File paths are the one copy (labels).

use crate::manifest::{self, Line, Parts};
use crate::raw::{Event, Item, Raw, SUBAGENT_STOP, Target};
use crate::redact::Mapped;
use crate::worker::Consumer;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Manifest {
    home: PathBuf,
}

impl Manifest {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
        }
    }
}

/// Records per step, as the FTS consumer.
const BATCH: usize = 500;
/// ponytail: one size for every agent until spec 1.5's per-agent injection sizes; under Cursor's
/// 9,500-unit cut with room for the recording-failure line, and near claude-mem's 10,000 for its
/// block alone (docs/cards.md S5).
pub const CAP: usize = 9_000;
/// How much of one prompt, reply, command or output a manifest shows.
const CLIP: usize = 400;
const FILES: usize = 10;
const DIRECTIVES: usize = 10;
/// Delivered claims SessionStart shows with their bodies, and at most this many more one line each
/// (spec 4.4: about 10, then an index).
const BODIES: usize = 10;
/// The index's lines by default (`[inject] session_start_index`, decision 75).
pub const INDEX: usize = 10;
/// Decision 76: a decision or an open item this much older than the read gets no body.
const STALE_MS: i64 = 14 * 24 * 60 * 60 * 1000;
#[cfg(test)]
thread_local! {
    /// Where decision 76 measures a claim's age from in a test whose claims are dated in 1970 and
    /// read at the clock's time (the hooks' tests); none, the read's own time.
    pub(crate) static FOLD_AT: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}
/// The newest delivered claims SessionStart chooses from.
// ponytail: an older claim reaches SessionStart only through search; a larger pool if M22's
// scale check leaves room.
const POOL: usize = 200;
/// How much of a claim an index line shows.
const BRIEF: usize = 80;
/// The index's first line: where the rest is (spec 4.4).
const TOOLS: &str = "`search` finds more of what is remembered here, `get` shows one in full by \
                     its id, and `timeline` lists the earlier sessions.";
/// The stored manifest's own size: the state lines leave the cards their room (docs/cards.md S5).
const MANIFEST: usize = 6_000;
/// The newest cards session start shows, at most: claude-mem's default (docs/cards.md S4).
const CARDS: usize = 50;
/// The newest session summaries session start shows, claude-mem's default (docs/summaries.md S7).
const SUMMARIES: usize = 10;
const TODOS: usize = 20;
/// The checkout's newest prompts the build reads for the owner's last one.
// shortcut: a checkout whose last 200 prompts were all a program's shows no last prompt; read
// further back if that is seen.
const PROMPTS: i64 = 200;
const SESSIONS: usize = 5;
/// ponytail: the owner lines a build reads (a negation older than these no longer matters); a
/// claims table replaces the scan in milestone 3.
const OWNER_LINES: i64 = 500;
/// Spec 4.9, MUST-M8: sessions with a record this close to the time SessionStart reads are active.
pub(crate) const ACTIVE_MS: i64 = 30 * 60 * 1000;
/// MUST-M9: the backlog is shown once its oldest record has waited this long.
const LAG_MS: i64 = 2 * 60 * 1000;
/// The stored ruleset's format: a row built before the read-time lines (Task 8) holds the other
/// sessions and the backlog count in its text, and one before `m5` another program's prompt as
/// the last exchange (docs/session-start.md), so it is built again.
const FORMAT: &str = "m5";
/// D9's `git status` in the worker, given up after this.
const GIT_TIMEOUT: Duration = Duration::from_millis(500);

fn schema(k: &Connection) -> Result<()> {
    k.execute_batch(
        "CREATE TABLE IF NOT EXISTS manifest_facts(
           device TEXT NOT NULL, seq INTEGER NOT NULL, repo TEXT NOT NULL, branch TEXT NOT NULL,
           session TEXT NOT NULL, ts INTEGER NOT NULL, fact TEXT NOT NULL,
           label TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS manifest_facts_seq
           ON manifest_facts(device, repo, branch, fact, seq);
         CREATE INDEX IF NOT EXISTS manifest_facts_ts ON manifest_facts(device, repo, fact, ts);
         -- A turn's reply, whatever the branch (a seek).
         CREATE INDEX IF NOT EXISTS manifest_facts_turn
           ON manifest_facts(device, repo, session, fact, seq);
         -- The session's next prompt, in any repository, that ends a turn (a seek; #856).
         CREATE INDEX IF NOT EXISTS manifest_facts_session
           ON manifest_facts(device, session, fact, seq);
         -- A failure and the success of the same call (a seek, not a scan of later successes).
         CREATE INDEX IF NOT EXISTS manifest_facts_label
           ON manifest_facts(device, repo, branch, fact, label, seq);
         -- A tombstone's record, a seek that covers what its reads take: a large forget's
         -- tombstones each scanned the device's facts (slice 3c, measured).
         CREATE INDEX IF NOT EXISTS manifest_facts_record
           ON manifest_facts(device, seq, repo, branch);
         CREATE TABLE IF NOT EXISTS manifests(
           repo TEXT NOT NULL, branch TEXT NOT NULL, device TEXT NOT NULL,
           built_at INTEGER NOT NULL, text TEXT NOT NULL,
           -- The ruleset version its fields were gated with (Task 3b).
           ruleset TEXT NOT NULL DEFAULT '',
           PRIMARY KEY (repo, branch, device)
         );
         -- Checkouts whose facts changed since their manifest was built.
         CREATE TABLE IF NOT EXISTS manifest_dirty(
           repo TEXT NOT NULL, branch TEXT NOT NULL, device TEXT NOT NULL,
           PRIMARY KEY (repo, branch, device)
         );
         -- Devices whose facts a rewind dropped: the next step reads raw from seq 1 again.
         CREATE TABLE IF NOT EXISTS manifest_rebuild(device TEXT PRIMARY KEY);",
    )?;
    Ok(())
}

/// What SessionStart shows (`text`), and the claims it shows (spec 4.7, 4.8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Start {
    pub text: String,
    pub shown: Vec<Shown>,
    /// How many cards and session summary rows it shows (docs/cards.md S1, docs/summaries.md
    /// S7): the recent work the terminal line names.
    pub cards: usize,
    /// The work state section, which comes before the manifest (docs/work-state.md L7).
    pub work: Option<crate::work_state::Section>,
    /// The cards and session summaries its block shows, by ID and kind (G23): a prompt's block
    /// does not show them again.
    pub docs: Vec<(String, &'static str)>,
    /// The memory tools' use (`mem_search::MEMORY_USE`), after the work state's rule.
    pub memory_use: bool,
}

/// A claim shown to a session: a later injection need not repeat its body, and a correction names
/// it if it changes (Task 8, D9).
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub uid: String,
    /// `fingerprint` of its body when it was shown.
    pub fp: String,
    /// Its body, or only its index line.
    pub body: bool,
    /// The line that shows it: a later cut that drops the line leaves it unshown.
    pub line: String,
    /// This occurrence's byte range in `Start::text`, after its gate and cut.
    pub range: std::ops::Range<usize>,
}

/// A body's fingerprint: whether a claim's body changed since it was shown.
pub fn fingerprint(body: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(body.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What SessionStart shows for this checkout (spec 4.4, D3 and D4 of milestone 4's plan): the
/// manifest its records built, with the delivered claims, the cards and the session summaries
/// read from knowledge.db's indexes, and the checkout's backlog and the repository's other live sessions
/// read at `now` (`live`; MUST-M9, M8). Each field read is gated with `rules` before it is
/// formatted, so a rule anchored to a field still matches it, the whole is gated again and cut to
/// `cap` at a line, with at most `index` claims in its one-line index (decision 75). Read-only: a
/// hook never writes knowledge.db, and none is made when the worker has not run yet. `session` is
/// the reader's, which is no other session.
#[allow(clippy::too_many_arguments)]
pub fn text(
    home: &Path,
    raw: &Raw,
    repo: &str,
    branch: &str,
    session: &str,
    rules: &crate::redact::Rules,
    cap: usize,
    index: usize,
    now: i64,
) -> Result<Option<Start>> {
    let path = home.join("knowledge.db");
    // Nothing from a store a rebuild is filling (#855): `raw` is open, so a set-aside waits.
    if !path.exists() || crate::worker::rebuilding(home) {
        return Ok(None);
    }
    let k = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let manifest = stored(&k, raw, repo, branch, &stamp(rules.version()))?.unwrap_or_default();
    // With `[summary] curate` off no curation waits, as the page's count has it
    // (`view::waiting`). A config.toml that does not load stops curation too, and what it holds
    // back is shown.
    let curating = crate::config::load(home).map_or(true, |c| c.summary.curate);
    let live = live(&k, raw, repo, branch, session, rules, now, curating)?;
    let parts = delivered(&k, raw, repo, &manifest, rules, (index, now))?;
    // The cards after the decisions (docs/cards.md S1): the most, halved each time and then as
    // many back as fit (decision 75), whose packet
    // stays within `cap` as it leaves, gated and escaped inside the fence, in UTF-16 units, the
    // measure Cursor cuts by (S5); the rest is never cut for them. Each count is judged on the
    // gated packet: the gate can free room in the rest, or in a row only as the block composes it,
    // so no count taken before it is a bound (Codex on #370). The fence's own text is outside
    // `cap`, as it always was.
    let cards = crate::cards::recent(&k, raw, repo, CARDS, rules)?;
    let work = crate::cards::work(home, raw, &cards);
    let sessions = crate::turns::recent(&k, raw, repo, SUMMARIES, rules)?;
    let name = repo_name(repo, rules);
    let fence = manifest::fenced("").encode_utf16().count();
    // docs/summaries.md S9, claude-mem's order when the block does not fit: the newest summary's
    // fields go first, then the summary rows halve, then the cards, here down to none.
    // The packet with the newest summary's fields or not, `n_s` summary rows and `n` cards, gated,
    // and whether it fits; none when there is nothing to show.
    let attempt = |fields: bool, n_s: usize, n: usize| {
        let latest = sessions.first().filter(|_| fields);
        let empty = n == 0 && n_s == 0 && latest.is_none();
        let block = if empty {
            String::new()
        } else {
            let (shown, rows) = (&cards[..n], &sessions[..n_s]);
            crate::cards::block(
                shown,
                rows,
                latest,
                raw.device(),
                &name,
                &work,
                now,
                &chrono::Local,
            )
        };
        let (packet, bodies) = parts.packet(&manifest, &live, &block)?;
        let (gated, from) = packet.outbound(rules);
        let fits = empty || manifest::fenced(gated.trim()).encode_utf16().count() - fence <= cap;
        Some((bodies, gated, from, fits))
    };
    let (mut fields, mut n_s, mut n) = (true, sessions.len(), cards.len());
    // The last count of cards found too many, once only the cards are left to halve.
    let mut over = None;
    let (mut bodies, mut gated, mut from) = loop {
        let Some((bodies, gated, from, fits)) = attempt(fields, n_s, n) else {
            return Ok(None);
        };
        if fits {
            break (bodies, gated, from);
        }
        if !fields && n_s == 0 {
            over = Some(n);
        }
        shrink(&mut fields, &mut n_s, &mut n);
    };
    // Decision 75: what the rest leaves goes to the cards, as many back as fit below the count
    // that did not.
    if let Some(over) = over
        && let Some((m, (b, g, f))) = fill_back(n, over, |m| match attempt(fields, n_s, m) {
            Some((b, g, f, true)) => Some((b, g, f)),
            _ => None,
        })
    {
        (n, bodies, gated, from) = (m, b, g, f);
    }
    let text = manifest::cut(&gated, cap);
    // Match the surviving occurrence, not another claim with the same rendered line.
    let shown = bodies
        .into_iter()
        .filter_map(|(range, mut shown)| {
            shown.range = surviving_line(&text, &from, range, &shown.line)?;
            Some(shown)
        })
        .collect();
    let docs = (cards[..n].iter().map(|c| (c.id(raw.device()), "card")))
        .chain(
            sessions[..n_s]
                .iter()
                .map(|s| (s.id(raw.device()), "summary")),
        )
        .collect();
    Ok(Some(Start {
        text,
        shown,
        cards: n + n_s,
        work: None,
        docs,
        memory_use: false,
    }))
}

/// Decision 75: the largest count above `n` and below `over` whose packet `fits` takes, tried
/// from the largest down. A block's size need not grow with its cards: it drops its savings line
/// once the cards shown cost as much to read as the work recorded (Codex's review of #854).
fn fill_back<T>(
    n: usize,
    over: usize,
    mut fits: impl FnMut(usize) -> Option<T>,
) -> Option<(usize, T)> {
    (n + 1..over).rev().find_map(|m| fits(m).map(|t| (m, t)))
}

/// The block's next step down (docs/summaries.md S9): the newest summary's fields, then the
/// summary rows halved, then the cards.
fn shrink(fields: &mut bool, rows: &mut usize, cards: &mut usize) {
    if *fields {
        *fields = false;
    } else if *rows > 0 {
        *rows /= 2;
    } else {
        *cards /= 2;
    }
}

/// The same source occurrence survived the packet's gate and cut as a whole line, not an
/// identical line from another claim. Prompt blocks use the same accounting as SessionStart.
pub(crate) fn surviving_line(
    text: &str,
    from: &[(usize, usize)],
    range: std::ops::Range<usize>,
    line: &str,
) -> Option<std::ops::Range<usize>> {
    let at = from.iter().position(|&(s, _)| s == range.start)?;
    let end = at + line.len();
    (text.get(at..end) == Some(line)
        && (at == 0 || text.as_bytes()[at - 1] == b'\n')
        && text.as_bytes().get(end).is_none_or(|&b| b == b'\n')
        && from.get(end - 1).is_some_and(|&(_, e)| e == range.end)
        && from[at..end]
            .iter()
            .all(|&(s, e)| s >= range.start && e <= range.end))
    .then_some(at..end)
}

/// The stored ruleset of a row built under rules of `version` in this format.
fn stamp(version: &str) -> String {
    format!("{FORMAT}/{version}")
}

/// What `text` reads when it is read, not stored (D3): MUST-M9's backlog, the checkout's records
/// past the device's curation checkpoint once the oldest has waited `LAG_MS`, and MUST-M8's other
/// sessions on the repository (this device's; sync brings the others' in milestone 6): at most
/// `SESSIONS`, each with a record in the `ACTIVE_MS` before `now` and no end, the newest first,
/// with its agent, branch and minutes since. A stored list kept naming a checkout left idle.
/// Nothing while a tombstone the worker has not applied may change the facts (as `stored`). The
/// backlog is a home's that curates (`curating`): with summaries off nothing waits to be curated,
/// and "not yet curated" would say it does.
#[allow(clippy::too_many_arguments)]
fn live(
    k: &Connection,
    raw: &Raw,
    repo: &str,
    branch: &str,
    session: &str,
    rules: &crate::redact::Rules,
    now: i64,
    curating: bool,
) -> Result<String> {
    let device = raw.device();
    if !exists(k, "table", "manifest_facts")?
        || !raw
            .tombstones_after(
                device,
                crate::knowledge::checkpoint::get(k, "manifest", device)?,
            )?
            .is_empty()
    {
        return Ok(String::new());
    }
    let mut out = String::new();
    // A record the checkpoint is inside of is curated only up to its offset.
    let (checkpoint, part) = raw.curation_checkpoint(device)?;
    let curated = if part.is_some() {
        checkpoint - 1
    } else {
        checkpoint
    };
    let (waiting, oldest): (i64, Option<i64>) = k.query_row(
        "SELECT COUNT(*), (SELECT ts FROM manifest_facts
           WHERE device = ?1 AND repo = ?2 AND branch = ?3 AND fact = 'event' AND seq > ?4
           ORDER BY seq LIMIT 1)
         FROM manifest_facts
         WHERE device = ?1 AND repo = ?2 AND branch = ?3 AND fact = 'event' AND seq > ?4",
        params![device, repo, branch, curated],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if let Some(ts) = oldest.filter(|ts| curating && now - ts > LAG_MS) {
        out.push_str(&format!(
            "## Backlog\n{waiting} record(s) / {} min not yet curated\n",
            (now - ts) / 60_000
        ));
    }
    // One aggregate, so SQLite takes the branch and time from each session's last record.
    let sessions: Vec<(String, String, i64, i64)> = k
        .prepare(
            "SELECT session, branch, ts, MAX(seq) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'event' AND ts >= ?3
             GROUP BY session ORDER BY ts DESC, session",
        )?
        .query_map(params![device, repo, now - ACTIVE_MS], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut lines = Vec::new();
    for (other, on, ts, seq) in sessions {
        if other == session {
            continue;
        }
        let ended = k
            .query_row(
                "SELECT 1 FROM manifest_facts
                 WHERE device = ?1 AND repo = ?2 AND fact = 'end' AND seq = ?3",
                params![device, repo, seq],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        // Its last record read back for its agent: none once a tombstone removed it.
        let Some(e) = event(raw, device, seq)?.filter(|_| !ended) else {
            continue;
        };
        let gate = |s: &str| one_line(&crate::redact::outbound_with(s, rules), CLIP);
        let on = if on.is_empty() {
            "no branch".to_owned()
        } else {
            gate(&on)
        };
        lines.push(format!(
            "- {} on {on}, {} min ago\n",
            gate(&e.agent),
            (now - ts).max(0) / 60_000
        ));
        if lines.len() == SESSIONS {
            break;
        }
    }
    if !lines.is_empty() {
        out.push_str(&format!("## Other active sessions\n{}", lines.concat()));
    }
    Ok(out)
}

/// Whether knowledge.db has the table or view `name` (`kind`): absent until the worker has run
/// this version's schema.
pub(crate) fn exists(k: &Connection, kind: &str, name: &str) -> Result<bool> {
    Ok(k.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2",
        [kind, name],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

/// The manifest the worker keeps for the checkout, if its records built one. None while the saved
/// text may show what raw now hides (D8): a tombstone the worker has not applied yet, or a
/// checkout still marked for a rebuild. None too when it was built under other redaction rules
/// or in another format than `ruleset` (`stamp`): its fields were flattened and clipped, so a rule
/// of another shape cannot be applied to it after.
fn stored(
    k: &Connection,
    raw: &Raw,
    repo: &str,
    branch: &str,
    ruleset: &str,
) -> Result<Option<String>> {
    let device = raw.device();
    if !exists(k, "table", "manifests")? {
        return Ok(None);
    }
    let dirty = k
        .query_row(
            "SELECT 1 FROM manifest_dirty WHERE repo = ?1 AND branch = ?2 AND device = ?3",
            params![repo, branch, device],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    let at = crate::knowledge::checkpoint::get(k, "manifest", device)?;
    if dirty || !raw.tombstones_after(device, at)?.is_empty() {
        return Ok(None);
    }
    let text: Option<(String, String)> = k
        .query_row(
            "SELECT text, ruleset FROM manifests WHERE repo = ?1 AND branch = ?2 AND device = ?3",
            params![repo, branch, device],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(text.filter(|(_, built)| built == ruleset).map(|(t, _)| t))
}

/// A line that shows a claim, its body's or its index line: its range in the packet, and the
/// claim as `Shown` names it.
type Body = (std::ops::Range<usize>, Shown);

/// Spec 4.4's SessionStart around the checkout's `manifest`: the global preferences first, then the
/// preferences of `repo` the owner made (`claims::owned`, slice 8b); the
/// manifest, with `repo`'s delivered decisions, preferences, open items and lessons (spec 3.4) in
/// its third place (spec 4.9's order); `live` after
/// the manifest; last, the rest of the delivered claims one line each, after the tools that find
/// more, so that the cut drops them first. About 10 claims have bodies, chosen by the words they
/// share with the manifest's files, last prompt and last failing command, then by recency, each
/// earlier claim with the later claim that ended it (`claims::units`), and listed newest first. A
/// checkout with no manifest to show (a new branch, a dirty manifest, a tombstone not yet applied)
/// still gets them (D4). Each body is gated with `rules` before it is flattened
/// and clipped. The text, with each line that shows a claim, its body's or its index line.
/// Read when the text is (D3): claims come from curation, which runs while the owner is idle and
/// appends no record this consumer steps on, so a section built with the text would miss the last
/// session's decisions; and what the worker has not applied yet (`claims::Pending`) is left out.
/// Unchanged where curation never ran (no claims view). At most `lines` claims are in the index,
/// and a decision or an open item older than 14 days at `now` is one of them, never a body
/// (decisions 75 and 76).
fn delivered(
    k: &Connection,
    raw: &Raw,
    repo: &str,
    manifest: &str,
    rules: &crate::redact::Rules,
    (lines, now): (usize, i64),
) -> Result<Delivered> {
    use crate::claims::{self, Claim};
    let mut d = Delivered::default();
    if exists(k, "view", "active")? {
        let pending = claims::Pending::read(raw, k)?;
        let hidden = |uid: &str| Ok(pending.touches(k, uid)? || claims::muted(k, uid)?);
        let mut prefs = Vec::new();
        for c in claims::global(k)? {
            if !hidden(&c.uid)? {
                prefs.push(c);
            }
        }
        // Slice 8b (R4): the owner's own rules of this repository, every one, and not again
        // among its decisions.
        let owned = claims::owned(k, repo)?;
        let mut ours = Vec::new();
        for c in &owned {
            if !hidden(&c.uid)? {
                ours.push(c.clone());
            }
        }
        let words = terms(manifest);
        // #743: the pool counts no owner rule, which leaves it below.
        let mut ranked = claims::decisions(k, repo, POOL + owned.len())?;
        let owned_uids: std::collections::HashSet<&str> =
            owned.iter().map(|o| o.uid.as_str()).collect();
        ranked.retain(|c| !owned_uids.contains(c.uid.as_str()));
        ranked.truncate(POOL);
        // Stable: among claims that share as many words, the newest first.
        ranked.sort_by_cached_key(|c| std::cmp::Reverse(shared(&c.body, &words)));
        #[cfg(test)]
        let now = FOLD_AT.get().unwrap_or(now);
        // `claims::place` with decision 76: a stale unit keeps its rank among the index's lines.
        let stale = |unit: &[Claim]| {
            unit.first().is_some_and(|c| {
                matches!(c.kind.as_str(), "decision" | "open item") && c.valid_from < now - STALE_MS
            })
        };
        let (mut left, mut units, mut rest) = (BODIES, Vec::new(), Vec::new());
        for unit in claims::units(k, &ranked, hidden)? {
            if !stale(&unit) && unit.len() <= left {
                left -= unit.len();
                units.push(unit);
            } else {
                rest.push(unit);
            }
        }
        let (mut index, _) = claims::place(rest, lines);
        claims::newest_first(&mut units);
        claims::newest_first(&mut index);
        let gate = |s: &str, n: usize| crate::redact::flattened_with(s, rules, n, one_line);
        let date = |c: &Claim| crate::db::utc(c.valid_from)[..10].to_owned();
        // Each line that shows a claim joins `bodies` with its range in `out`.
        let shown = |out: &mut Mapped, bodies: &mut Vec<Body>, c: &Claim, line: Mapped, body| {
            let at = out.text.len();
            let shown = Shown {
                uid: c.uid.clone(),
                fp: fingerprint(&c.body),
                body,
                line: line.masked(),
                range: at..at + line.text.len(),
            };
            bodies.push((at..at + line.text.len(), shown));
            out.append(line);
            out.push_str("\n");
        };
        let full = |out: &mut Mapped, bodies: &mut Vec<Body>, c: &Claim, unit: &[Claim]| {
            shown(out, bodies, c, body_line(c, unit, rules), true);
        };
        let id = |uid: &str| uid.chars().take(12).collect::<String>();
        let brief = |c: &Claim| {
            let ended = c
                .later
                .as_ref()
                .map(|l| format!(", superseded by {}", id(l)))
                .unwrap_or_default();
            let mut line = Mapped::default();
            line.push_str(&format!("- {} {} {}{ended}: ", id(&c.uid), date(c), c.kind));
            line.append(gate(&c.body, BRIEF));
            line
        };
        for (heading, claims) in [
            ("Global preferences", &prefs),
            ("Preferences of this repository", &ours),
        ] {
            if !claims.is_empty() {
                d.first.push_str(&format!("## {heading}\n"));
                for c in claims {
                    full(&mut d.first, &mut d.first_bodies, c, &[]);
                }
            }
        }
        if !units.is_empty() {
            d.middle.push_str("## Decisions and open items\n");
            for unit in &units {
                for c in unit {
                    full(&mut d.middle, &mut d.middle_bodies, c, unit);
                }
            }
        }
        if !(units.is_empty() && index.is_empty()) {
            d.last.push_str(&format!("## More from memory\n{TOOLS}\n"));
            for c in index.iter().flatten() {
                shown(&mut d.last, &mut d.last_bodies, c, brief(c), false);
            }
        }
    }
    Ok(d)
}

/// What `delivered` reads, each part with the lines in it that show a claim.
#[derive(Default)]
struct Delivered {
    first: Mapped,
    first_bodies: Vec<Body>,
    middle: Mapped,
    middle_bodies: Vec<Body>,
    last: Mapped,
    last_bodies: Vec<Body>,
}

impl Delivered {
    /// The packet around the checkout's `manifest`, with `block` (the cards) after the decisions
    /// and `live` after the manifest, and each line in it that shows a claim; none when it is empty.
    fn packet(&self, manifest: &str, live: &str, block: &str) -> Option<(Mapped, Vec<Body>)> {
        let at = AFTER_DECISIONS
            .iter()
            .filter_map(|h| {
                let h = format!("## {h}\n");
                manifest
                    .match_indices(&h)
                    .map(|(i, _)| i)
                    .find(|&i| i == 0 || manifest[..i].ends_with('\n'))
            })
            .min()
            .unwrap_or(manifest.len());
        // Each section's lines move by where the section lands in the packet.
        let at_offset = |bodies: &[Body], offset: usize| {
            bodies
                .iter()
                .map(move |(range, s)| (range.start + offset..range.end + offset, s.clone()))
                .collect::<Vec<_>>()
        };
        let mut bodies = self.first_bodies.clone();
        let mut text = self.first.clone();
        text.push_str(&manifest[..at]);
        bodies.extend(at_offset(&self.middle_bodies, text.text.len()));
        text.append(self.middle.clone());
        text.push_str(block);
        text.push_str(&manifest[at..]);
        text.push_str(live);
        bodies.extend(at_offset(&self.last_bodies, text.text.len()));
        text.append(self.last.clone());
        (!text.text.is_empty()).then_some((text, bodies))
    }
}

/// The repository's name as the cards' header shows it: its last part, gated with the whole and
/// again as shown, on one line.
pub(crate) fn repo_name(repo: &str, rules: &crate::redact::Rules) -> String {
    let gated = crate::redact::outbound_with(repo, rules);
    let last = gated
        .rsplit(['/', '\\'])
        .find(|p| !p.is_empty())
        .unwrap_or(&gated);
    crate::redact::flattened_with(last, rules, usize::MAX, one_line).masked()
}

/// The words a claim may share with the checkout's manifest (D4): the names of the files touched,
/// and the words of four letters or more, lowercased, of the last prompt and of what the last
/// failing command ran (MUST-M21: a lesson about it gets its body), not of its tool or output.
// ponytail: words are ASCII letters, digits and `_`, so a prompt in Japanese gives only the ASCII
// words in it; bigrams for CJK text if SessionStart's choice needs them.
fn terms(manifest: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut section = "";
    let words_of = |p: &str| -> Vec<String> {
        p.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map(str::to_owned)
            .collect()
    };
    for line in manifest.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            section = h;
            continue;
        }
        // `{tool}: {what ran}`, then `  failed with: {output}` (`build`).
        if section == "Last failing command" {
            if let Some((_, ran)) = line.split_once(": ").filter(|_| !line.starts_with(' ')) {
                for w in words_of(ran) {
                    let w = w.to_lowercase();
                    if w.chars().count() >= 4 && !out.contains(&w) {
                        out.push(w);
                    }
                }
            }
            continue;
        }
        let Some(item) = line.strip_prefix("- ") else {
            continue;
        };
        let words: Vec<String> = match section {
            "Files touched" => item
                .rsplit(['/', '\\'])
                .next()
                .map(str::to_owned)
                .into_iter()
                .collect(),
            "Last exchange" => item
                .strip_prefix("prompt: ")
                .map(words_of)
                .unwrap_or_default(),
            _ => continue,
        };
        for w in words {
            let w = w.to_lowercase();
            if w.chars().count() >= 4 && !out.contains(&w) {
                out.push(w);
            }
        }
    }
    out
}

/// How many of `terms` `body` holds.
fn shared(body: &str, terms: &[String]) -> usize {
    let body = body.to_lowercase();
    terms.iter().filter(|t| body.contains(t.as_str())).count()
}

/// Spec 4.9's sections after the current decisions, in the manifest's words.
const AFTER_DECISIONS: [&str; 5] = [
    "Owner's directives",
    "Todo list",
    "Last exchange",
    "Files touched",
    "As of",
];

impl Consumer for Manifest {
    fn name(&self) -> &'static str {
        "manifest"
    }

    fn step(&mut self, raw: &Raw, k: &Connection, _device: &str, after: i64) -> Result<i64> {
        schema(k)?;
        let device = raw.device();
        // After a rewind, from seq 1: its checkpoint moves back, which the worker takes.
        let from = if k.execute("DELETE FROM manifest_rebuild WHERE device = ?1", [device])? > 0 {
            0
        } else {
            after
        };
        let recs = raw.after(device, from, BATCH)?;
        for r in &recs {
            match &r.item {
                Item::Event(e) => facts(k, device, r.seq, e)?,
                Item::Expired(e) => label_facts(k, device, r.seq, e)?,
                Item::Tombstone(
                    Target::Record { device: d, seq } | Target::Range { device: d, seq, .. },
                ) => {
                    // The target's facts again, from what raw returns now (masked, or none
                    // when it is removed), in a build of its checkout.
                    k.execute(
                        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
                         SELECT DISTINCT repo, branch, device FROM manifest_facts
                         WHERE device = ?1 AND seq = ?2",
                        params![d, seq],
                    )?;
                    // Every checkout of its repo, as for a new record: a directive or a session
                    // shows on the repo's other branches too.
                    k.execute(
                        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
                         SELECT repo, branch, device FROM manifests
                         WHERE device = ?1 AND repo IN
                           (SELECT repo FROM manifest_facts WHERE device = ?1 AND seq = ?2)",
                        params![d, seq],
                    )?;
                    k.execute(
                        "DELETE FROM manifest_facts WHERE device = ?1 AND seq = ?2",
                        params![d, seq],
                    )?;
                    match raw
                        .after(d, seq - 1, 1)?
                        .into_iter()
                        .find(|r| r.seq == *seq)
                        .map(|r| r.item)
                    {
                        Some(Item::Event(e)) => facts(k, d, *seq, &e)?,
                        Some(Item::Expired(e)) => label_facts(k, d, *seq, &e)?,
                        _ => {}
                    }
                }
                Item::Removed => {}
            }
        }
        // A backlog is built once, at its end, not once per batch. Each field is gated with the
        // rules as they are now before it is flattened and clipped, and a text built under other
        // rules is built again. Settings that do not load stop capture too (doctor names them):
        // the bundled rules then.
        if recs.len() < BATCH {
            let rules = crate::capture::Settings::load(&self.home)
                .map(|s| s.rules)
                .unwrap_or_default();
            k.execute(
                "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
                 SELECT repo, branch, device FROM manifests WHERE device = ?1 AND ruleset != ?2",
                params![device, stamp(rules.version())],
            )?;
            rebuild(raw, k, device, &rules)?;
        }
        Ok(recs.last().map_or(from, |r| r.seq))
    }

    /// The lost commits may hold tombstones that changed or removed older records' facts, so
    /// the device's facts and texts are dropped and built again from seq 1, as a rebuild from an
    /// empty knowledge.db would.
    fn rewind(&mut self, k: &Connection, device: &str, _to: i64) -> Result<()> {
        schema(k)?;
        for table in ["manifest_facts", "manifests", "manifest_dirty"] {
            k.execute(&format!("DELETE FROM {table} WHERE device = ?1"), [device])?;
        }
        k.execute(
            "INSERT OR IGNORE INTO manifest_rebuild(device) VALUES(?1)",
            [device],
        )?;
        Ok(())
    }
}

/// docs/retention.md: the facts an expired record's labels give (its text gave the others, and it
/// is gone), and its checkout marked for a rebuild, as `facts` marks it.
fn label_facts(k: &Connection, device: &str, seq: i64, e: &Event) -> Result<()> {
    if !crate::raw::is_live(&e.source) || matches!(e.kind.as_str(), "directive" | SUBAGENT_STOP) {
        return Ok(());
    }
    let Some(repo) = e.repo.as_deref() else {
        return Ok(());
    };
    let kinds: &[&str] = match e.kind.as_str() {
        kind @ ("prompt" | "reply" | "end") => &["event", kind],
        _ => &["event"],
    };
    let branch = e.branch.as_deref().unwrap_or("");
    for fact in kinds {
        k.execute(
            "INSERT INTO manifest_facts(device, seq, repo, branch, session, ts, fact, label)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, '')",
            params![device, seq, repo, branch, e.session, e.ts, fact],
        )?;
    }
    dirty(k, device, repo, branch)
}

/// Every checkout of `repo` marked for a rebuild, `branch`'s among them.
fn dirty(k: &Connection, device: &str, repo: &str, branch: &str) -> Result<()> {
    // Owner directives and other sessions are the repo's, not the branch's: every checkout of
    // the repo is built again, so its text depends on the records, not on the order they came.
    k.execute(
        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device) VALUES(?1, ?2, ?3)",
        params![repo, branch, device],
    )?;
    k.execute(
        "INSERT OR IGNORE INTO manifest_dirty(repo, branch, device)
         SELECT repo, branch, device FROM manifests WHERE repo = ?1 AND device = ?2",
        params![repo, device],
    )?;
    Ok(())
}

/// The facts of one event, and its checkout marked for a rebuild. Events without a repo label
/// belong to no checkout.
fn facts(k: &Connection, device: &str, seq: i64, e: &Event) -> Result<()> {
    // Imported records are history, not the checkout's state (milestone 4 D6): its facts and its
    // count of records not yet curated come from live records only. Slice 8b: nor is the owner's
    // directive of a repository (`pref add --repo`) any session's work: its claim is read when the
    // start is (`delivered`). Nor is a subagent's stop (docs/subagents.md): it would make an ended
    // session active again, and count as a record to curate.
    if !crate::raw::is_live(&e.source) || matches!(e.kind.as_str(), "directive" | SUBAGENT_STOP) {
        return Ok(());
    }
    let Some(repo) = e.repo.as_deref() else {
        return Ok(());
    };
    let branch = e.branch.as_deref().unwrap_or("");
    let add = |fact: &str, label: &str| -> Result<()> {
        k.execute(
            "INSERT INTO manifest_facts(device, seq, repo, branch, session, ts, fact, label)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![device, seq, repo, branch, e.session, e.ts, fact, label],
        )?;
        Ok(())
    };
    add("event", "")?;
    let body: Value = serde_json::from_str(&e.body).unwrap_or(Value::Null);
    match e.kind.as_str() {
        "prompt" => {
            add("prompt", "")?;
            // A prompt another agent sent holds no directive of the owner's, whatever it says
            // (#273).
            if body["agent_sent"] != true && manifest::is_owner_line(str_at(&body, "prompt")) {
                add("owner", "")?;
            }
        }
        "reply" => add("reply", "")?,
        "end" => add("end", "")?,
        // An interrupted call returned nothing: neither a failure nor the fix of one. Nor is a
        // read of stored memory, or a note saved to it, any of the work's facts.
        "tool" if body.get("interrupted").and_then(Value::as_bool) == Some(true) => {}
        "tool"
            if crate::curate::memory_read(str_at(&body, "tool"), &body)
                || crate::curate::memory_write(str_at(&body, "tool"), &body) => {}
        "tool" => {
            // Every success, not only one after a failure of its call: a tombstone can change a
            // failure's key later, and the build pairs them then.
            let key = call_key(&body);
            add(if failed(&body) { "fail" } else { "fixed" }, &key)?;
            let input: Value = serde_json::from_str(str_at(&body, "input")).unwrap_or(Value::Null);
            if todos(&input).is_some() {
                add("todo", "")?;
            }
            for path in paths(&input, e.cwd.as_deref()) {
                add("file", &path)?;
            }
        }
        _ => {}
    }
    dirty(k, device, repo, branch)
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// What a tool call ran, for "the same call succeeded later": a command (Codex's `cmd`) without
/// Claude's `description`, else the whole input.
fn call_key(body: &Value) -> String {
    use sha2::{Digest, Sha256};
    let text = format!(
        "{}\n{}",
        str_at(body, "tool"),
        what_ran(str_at(body, "input"))
    );
    Sha256::digest(text.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn what_ran(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(v) => match v.get("command").or_else(|| v.get("cmd")) {
            Some(Value::String(c)) => c.clone(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => input.to_owned(),
        },
        Err(_) => input.to_owned(),
    }
}

/// A failed call: the hook said so (PostToolUseFailure), or its output carries a non-zero exit
/// (Grok's `exit_code`; Codex's header, `Exit code: N` or `Process exited with code N`, in the
/// lines before `Output:`, so a command's own output is never read as its exit).
fn failed(body: &Value) -> bool {
    if body.get("failed").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    let output = str_at(body, "output");
    let code = match serde_json::from_str::<Value>(output) {
        Ok(v) => v.get("exit_code").and_then(Value::as_i64),
        Err(_) => output
            .lines()
            .take_while(|l| l.trim_end() != "Output:")
            .find_map(|l| {
                l.strip_prefix("Exit code: ")
                    .or_else(|| l.strip_prefix("Process exited with code "))
            })
            .and_then(|n| n.trim().parse().ok()),
    };
    code.is_some_and(|c| c != 0)
}

/// A todo list's items as (status, text): Claude Code's TodoWrite (`todos`) or Codex's
/// `update_plan` (`plan`).
fn todos(input: &Value) -> Option<Vec<(String, String)>> {
    let (items, text) = match (input.get("todos"), input.get("plan")) {
        (Some(Value::Array(a)), _) => (a, "content"),
        (_, Some(Value::Array(a))) => (a, "step"),
        _ => return None,
    };
    Some(
        items
            .iter()
            .map(|i| (str_at(i, "status").to_owned(), str_at(i, text).to_owned()))
            .collect(),
    )
}

/// The files a call names: the file path fields agents use (not a bare `path`: Glob, Grep and
/// LS take a directory there), and the file lines of Codex's `apply_patch` (its hook's
/// `command`, its transcript's `input`). A path under the call's cwd is shown relative to it.
pub(crate) fn paths(input: &Value, cwd: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = ["file_path", "notebook_path", "target_file"]
        .iter()
        .filter_map(|k| input.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let patch = format!("{}\n{}", str_at(input, "command"), str_at(input, "input"));
    for line in patch.lines() {
        for tag in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(p) = line.strip_prefix(tag) {
                out.push(p.trim().to_owned());
            }
        }
    }
    // `/` on every platform, so a Windows path under a Windows cwd is relative too.
    let slashes = |s: &str| s.replace('\\', "/");
    let prefix = cwd.map(|c| format!("{}/", slashes(c).trim_end_matches('/')));
    let mut out: Vec<String> = out
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| slashes(&p))
        .map(
            |p| match prefix.as_deref().and_then(|c| p.strip_prefix(c)) {
                Some(rel) => rel.to_owned(),
                None => p,
            },
        )
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A claim's first words, with field findings kept until its formatted line is gated.
pub(crate) fn first_words(body: &str, rules: &crate::redact::Rules) -> Mapped {
    crate::redact::flattened_with(body, rules, BRIEF, one_line)
}

/// A claim's line with its body, at SessionStart and at a prompt: its date and kind, the later
/// claim of `unit` that ended it (shown above it), and its body gated with `rules` before it is
/// flattened and clipped. The original context and field findings survive until the packet's gate.
pub(crate) fn body_line(
    c: &crate::claims::Claim,
    unit: &[crate::claims::Claim],
    rules: &crate::redact::Rules,
) -> Mapped {
    let date = |c: &crate::claims::Claim| crate::db::utc(c.valid_from)[..10].to_owned();
    let ended = c
        .later
        .as_ref()
        .and_then(|l| unit.iter().find(|u| u.uid == *l))
        .map(|l| format!(", superseded by the {} {} above", date(l), l.kind))
        .unwrap_or_default();
    let mut line = Mapped::default();
    line.push_str(&format!("- {} {}{ended}: ", date(c), c.kind));
    line.append(crate::redact::flattened_with(
        &c.body, rules, CLIP, one_line,
    ));
    line
}

/// G23: a card or a session summary as one line of a prompt's block, as `body_line` writes a
/// claim: its date, what it is, its ID as `get` takes it, then its text on one line, cut to
/// `CLIP` and gated.
pub(crate) fn doc_line(
    ts: i64,
    what: &str,
    id: &str,
    text: &str,
    rules: &crate::redact::Rules,
) -> Mapped {
    let mut line = Mapped::default();
    line.push_str(&format!("- {} {what} {id}: ", &crate::db::utc(ts)[..10]));
    line.append(crate::redact::flattened_with(text, rules, CLIP, one_line));
    line
}

/// `s` on one line, cut to `n` characters at a space, or after a Japanese or Chinese clause mark
/// (text in those languages has no spaces): a token is shown whole or not at all, so a rule added
/// after the manifest was built still matches it at SessionStart's gate. A first token or clause
/// longer than `n` is left out, not cut.
pub(crate) fn one_line(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(n) {
        Some((at, c)) => {
            let end = if c == ' ' {
                at
            } else {
                flat[..at]
                    .char_indices()
                    .rev()
                    .find_map(|(i, ch)| match ch {
                        ' ' => Some(i),
                        '、' | '。' | '，' | '！' | '？' => Some(i + ch.len_utf8()),
                        _ => None,
                    })
                    .unwrap_or(0)
            };
            format!("{}…", &flat[..end])
        }
        None => flat,
    }
}

/// Every dirty checkout of this device built again (or its row removed when no record is left).
fn rebuild(raw: &Raw, k: &Connection, device: &str, rules: &crate::redact::Rules) -> Result<()> {
    let dirty: Vec<(String, String)> = k
        .prepare("SELECT repo, branch FROM manifest_dirty WHERE device = ?1 ORDER BY repo, branch")?
        .query_map([device], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (repo, branch) in dirty {
        match build(raw, k, device, &repo, &branch, rules)? {
            Some(text) => k.execute(
                "INSERT INTO manifests(repo, branch, device, built_at, text, ruleset)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(repo, branch, device) DO UPDATE SET built_at = excluded.built_at,
                   text = excluded.text, ruleset = excluded.ruleset",
                params![
                    repo,
                    branch,
                    device,
                    crate::db::now_ms(),
                    text,
                    stamp(rules.version())
                ],
            )?,
            None => k.execute(
                "DELETE FROM manifests WHERE repo = ?1 AND branch = ?2 AND device = ?3",
                params![repo, branch, device],
            )?,
        };
    }
    k.execute("DELETE FROM manifest_dirty WHERE device = ?1", [device])?;
    Ok(())
}

/// One record's event, read back through raw (`None` once a tombstone removed it).
pub(crate) fn event(raw: &Raw, device: &str, seq: i64) -> Result<Option<Event>> {
    Ok(raw
        .after(device, seq - 1, 1)?
        .into_iter()
        .find(|r| r.seq == seq)
        .and_then(|r| match r.item {
            Item::Event(e) => Some(*e),
            _ => None,
        }))
}

fn body(e: &Event) -> Value {
    serde_json::from_str(&e.body).unwrap_or(Value::Null)
}

/// The manifest's text for one checkout from its facts: every part but the git state comes from
/// records, so the same records give the same text (spec 4.9).
fn build(
    raw: &Raw,
    k: &Connection,
    device: &str,
    repo: &str,
    branch: &str,
    rules: &crate::redact::Rules,
) -> Result<Option<String>> {
    // Every stored field it shows is gated before it is flattened or clipped.
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
    let last = |fact: &str| -> Result<Option<i64>> {
        Ok(k.query_row(
            "SELECT MAX(seq) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = ?3 AND branch = ?4",
            params![device, repo, fact, branch],
            |r| r.get(0),
        )?)
    };
    let Some((last_seq, as_of)) = k
        .query_row(
            "SELECT MAX(seq), MAX(ts) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'event' AND branch = ?3",
            params![device, repo, branch],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1).unwrap_or(0))),
        )
        .map(|(s, t)| s.map(|s| (s, t)))?
    else {
        return Ok(None);
    };
    let mut p = Parts {
        as_of: crate::db::utc(as_of),
        ..Parts::default()
    };
    if let Some(e) = event(raw, device, last_seq)?
        && let Some(cwd) = e.cwd.as_deref()
    {
        p.git = risky(Path::new(cwd), branch);
    }
    // The last failure not followed by a success of the same call.
    let failing: Option<i64> = k
        .query_row(
            "SELECT f.seq FROM manifest_facts f
             WHERE f.device = ?1 AND f.repo = ?2 AND f.fact = 'fail' AND f.branch = ?3
               AND NOT EXISTS (SELECT 1 FROM manifest_facts x
                 WHERE x.device = f.device AND x.repo = f.repo AND x.fact = 'fixed'
                   AND x.branch = f.branch AND x.label = f.label AND x.seq > f.seq)
             ORDER BY f.seq DESC LIMIT 1",
            params![device, repo, branch],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(e) = failing
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        let b = body(&e);
        p.failing = Some(format!(
            "{}: {}\n  failed with: {}",
            one_line(&gate(str_at(&b, "tool")), CLIP),
            one_line(&gate(&what_ran(str_at(&b, "input"))), CLIP),
            one_line(&gate(str_at(&b, "output")), CLIP)
        ));
    }
    let mut owner: Vec<(i64, String, i64)> = k
        .prepare(
            "SELECT seq, session, ts FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'owner' ORDER BY seq DESC LIMIT ?3",
        )?
        .query_map(params![device, repo, OWNER_LINES], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    owner.reverse();
    // D13 matches directive lines one by one: taking one back leaves the others of its prompt.
    // ponytail: lines and `。`; English sentences on one line stay one line.
    let mut lines = Vec::new();
    for (seq, session, ts) in owner {
        if let Some(e) = event(raw, device, seq)? {
            // A template recorded before capture marked it holds no directive of the owner's.
            if crate::typed::codex_review_preset(str_at(&body(&e), "prompt")) {
                continue;
            }
            let prompt = gate(str_at(&body(&e), "prompt"));
            for text in prompt.split(['\n', '。']).map(str::trim) {
                if manifest::is_owner_line(text) {
                    lines.push(Line {
                        date: crate::db::utc(ts)[..10].to_owned(),
                        session: session.clone(),
                        text: text.to_owned(),
                    });
                }
            }
        }
    }
    // A line said again is shown once, at its latest date.
    let mut kept: Vec<Line> = Vec::new();
    for l in manifest::directives(&lines) {
        let text = one_line(&l.text, CLIP);
        kept.retain(|k| k.text != text);
        kept.push(Line { text, ..l });
    }
    p.directives = kept.split_off(kept.len().saturating_sub(DIRECTIVES));
    if let Some(e) = last("todo")?
        .map(|s| event(raw, device, s))
        .transpose()?
        .flatten()
    {
        let input: Value = serde_json::from_str(str_at(&body(&e), "input")).unwrap_or(Value::Null);
        p.todo = todos(&input)
            .unwrap_or_default()
            .into_iter()
            .take(TODOS)
            .map(|(status, text)| {
                let (status, text) = (gate(&status), gate(&text));
                format!("[{}] {}", one_line(&status, CLIP), one_line(&text, CLIP))
            })
            .collect();
    }
    // The last prompt the owner typed, and the reply that ended its turn: a prompt another program
    // sent, Codex's review templates among them, is not the owner's (docs/session-start.md).
    let prompts: Vec<(i64, String)> = k
        .prepare(
            "SELECT seq, session FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'prompt' AND branch = ?3
             ORDER BY seq DESC LIMIT ?4",
        )?
        .query_map(params![device, repo, branch, PROMPTS], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (seq, session) in prompts {
        let Some(e) = event(raw, device, seq)? else {
            continue;
        };
        let b = body(&e);
        let text = str_at(&b, "prompt");
        if !crate::typed::typed(&e.kind, &e.agent, &e.source, &b)
            || crate::typed::assigns_a_role(text)
            || crate::typed::codex_review_preset(text)
        {
            continue;
        }
        p.last_prompt = Some(one_line(&gate(text), CLIP));
        // Its turn's reply: the first of its session in the repository after it, before the
        // session's next prompt in any repository (#856), on whichever branch the session had
        // moved to (Codex's review of #854). Not another repository's: a record there rebuilds and
        // a forget there reaches the repository's own checkouts only. An agent's own: a session id
        // is the agent's.
        let reply: Option<i64> = k.query_row(
            "SELECT MIN(seq) FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND session = ?3 AND fact = 'reply' AND seq > ?4
               AND seq < COALESCE((SELECT MIN(seq) FROM manifest_facts
                 WHERE device = ?1 AND session = ?3 AND fact = 'prompt' AND seq > ?4),
                 9223372036854775807)",
            params![device, repo, session, seq],
            |r| r.get(0),
        )?;
        if let Some(r) = reply.map(|s| event(raw, device, s)).transpose()?.flatten()
            && r.agent == e.agent
        {
            p.last_reply = Some(one_line(&gate(str_at(&body(&r), "assistant")), CLIP));
        }
        break;
    }
    p.files = k
        .prepare(
            "SELECT label FROM manifest_facts
             WHERE device = ?1 AND repo = ?2 AND fact = 'file' AND branch = ?3
             GROUP BY label ORDER BY MAX(seq) DESC LIMIT ?4",
        )?
        .query_map(params![device, repo, branch, FILES as i64], |r| r.get(0))?
        .map(|f| f.map(|f: String| gate(&f)))
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(manifest::render(&p, MANIFEST)))
}

/// D9's risky git state of the checkout at `cwd`, while it is still on `branch` (or detached, as
/// during a rebase): nothing when `cwd` is gone, is no repository, or now has another branch.
fn risky(cwd: &Path, branch: &str) -> Vec<String> {
    let g = crate::capture::git(cwd);
    let Some(gitdir) = g.gitdir.as_deref().map(Path::new) else {
        return Vec::new();
    };
    if g.branch.as_deref().is_some_and(|b| b != branch) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (path, what) in [
        ("rebase-merge", "a rebase is in progress"),
        ("rebase-apply", "a rebase is in progress"),
        ("MERGE_HEAD", "a merge is in progress"),
        ("CHERRY_PICK_HEAD", "a cherry-pick is in progress"),
        ("REVERT_HEAD", "a revert is in progress"),
    ] {
        if gitdir.join(path).exists() && !out.iter().any(|o| o == what) {
            out.push(what.to_owned());
        }
    }
    if g.branch.is_none()
        && let Some(head) = g.head
    {
        let at: String = head.chars().take(12).collect();
        out.push(format!("detached HEAD at {at}"));
    }
    if let Some(status) = status(cwd) {
        out.extend(porcelain(&status));
    }
    out
}

/// The counts in `git status --porcelain=v2 --branch` output that a new session should know.
fn porcelain(status: &str) -> Vec<String> {
    let (mut changed, mut conflicts, mut untracked, mut ahead) = (0, 0, 0, 0);
    for line in status.lines() {
        match line.split_once(' ') {
            Some(("1" | "2", _)) => changed += 1,
            Some(("u", _)) => conflicts += 1,
            Some(("?", _)) => untracked += 1,
            Some(("#", rest)) => {
                if let Some(ab) = rest.strip_prefix("branch.ab +") {
                    ahead = ab
                        .split(' ')
                        .next()
                        .and_then(|a| a.parse().ok())
                        .unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    [
        (conflicts, "file(s) with merge conflicts"),
        (changed, "uncommitted change(s)"),
        (untracked, "untracked file(s)"),
        (ahead, "commit(s) not pushed"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect()
}

/// `git status --porcelain=v2 --branch` in `cwd`, given up after `GIT_TIMEOUT`. No optional
/// locks (the index is never written) and no fsmonitor (a repository's config cannot make the
/// worker run a command).
fn status(cwd: &Path) -> Option<String> {
    let mut child = Command::new("git")
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v2",
            "--branch",
        ])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read while git runs: a full pipe would otherwise stop it before it exits.
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        (&mut stdout).take(1 << 20).read_to_string(&mut s).ok();
        std::io::copy(&mut stdout, &mut std::io::sink()).ok();
        s
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                // No blocking wait: a git stuck in the kernel may not exit at once. A thread
                // waits for it, or it sits as a zombie for as long as a resident worker runs.
                child.kill().ok();
                std::thread::spawn(move || child.wait());
                break false;
            }
        }
    };
    if !ok {
        // A killed git's pipe can stay open in what it started: the reader finishes on its own.
        return None;
    }
    reader.join().ok()
}

#[cfg(test)]
mod tests;
