//! Milestone 4 Task 9: the cut-over from v1's `oboete.db` to Design B's stores (spec 7.4, D6).

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};

use crate::capture::{self, Captured, Settings};
use crate::raw::{Checkpoint, IMPORT_BATCH, ImportDoc, MAX_BATCH_BYTES, Raw, V1Row};

/// The source of what v1's store holds, as records and documents in Design B (D6).
const SOURCE: &str = "oboete-v1";

/// Refuse a source that a writable raw open would reuse, including a stopped restore's file.
pub fn check_source(home: &Path, from: &Path) -> Result<()> {
    let destination = crate::raw::path(home);
    let source = crate::db::store_file(from);
    anyhow::ensure!(
        source.is_empty() || source != crate::db::store_file(&destination),
        "v1 source aliases destination raw.db: use a separate source and destination"
    );
    Ok(())
}

/// What a pass imported.
#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct Stats {
    /// v1 events read past the checkpoint, and the records they became.
    pub events: u64,
    pub records: u64,
    /// `session_repos` rows recorded as `touch` records.
    pub repos: u64,
    /// Observations, summaries and prompts appended as import ops, and those imported before.
    pub documents: u64,
    pub seen: u64,
    /// Imported v1 sessions whose events oboete.db no longer holds: the old viewer deleted them
    /// after a pass, and the owner may forget them in Design B (spec 7.4, A105).
    pub deleted: Vec<String>,
    /// Stored identifiers whose earlier redaction or clipping prevents a reliable match.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub uncertain: Vec<String>,
}

/// One pass of the v1 import (spec 7.4, D6): the events past the checkpoint as records, in
/// batches each appended with its checkpoint; the deleted sessions; `session_repos` as `touch`
/// records; then the documents as import ops. Everything is read in one read transaction of the
/// store at `from`, which is never written; a rerun imports nothing twice.
pub fn pass(home: &Path, raw: &mut Raw, from: &Path) -> Result<Stats> {
    Ok(read_pass(home, raw, from)?.0)
}

/// v1's highest ids (events, observations, summaries, prompts) and its newest event: what
/// `--finish` checks v1 has not moved since its pass.
#[derive(Debug, PartialEq)]
struct Fingerprint {
    highest: [i64; 4],
    newest: Option<V1Row>,
}

fn fingerprint(v1: &Connection) -> Result<Fingerprint> {
    let mut highest = [0; 4];
    for (n, table) in ["events", "observations", "summaries", "prompts"]
        .into_iter()
        .enumerate()
    {
        highest[n] = v1.query_row(
            &format!("SELECT COALESCE(MAX(id), 0) FROM {table}"),
            [],
            |r| r.get(0),
        )?;
    }
    Ok(Fingerprint {
        highest,
        newest: row_at(v1, highest[0])?,
    })
}

/// v1's event `id`, fingerprinted, if there is one.
fn row_at(v1: &Connection, id: i64) -> Result<Option<V1Row>> {
    let sql = "SELECT id, ts, session_id FROM events WHERE id = ?1";
    Ok(v1.query_row(sql, [id], v1_row).optional()?)
}

/// A v1 event's fingerprint from (id, ts, session_id): the session id by its SHA-256 only. The
/// checkpoint is stored text, and only a record's labels pass the gate.
fn v1_row(r: &rusqlite::Row) -> rusqlite::Result<V1Row> {
    Ok(V1Row {
        id: r.get(0)?,
        ts: r.get(1)?,
        session_id: crate::curate::sha256_hex(&r.get::<_, String>(2)?),
    })
}

/// `pass`, with v1's fingerprint read in the same transaction.
fn read_pass(home: &Path, raw: &mut Raw, from: &Path) -> Result<(Stats, Fingerprint)> {
    let v1 = open_v1(from)?;
    // One read transaction: a consistent snapshot while v1's hooks keep writing.
    v1.execute_batch("BEGIN")?;
    let device = crate::db::device_id(&v1)
        .with_context(|| format!("{} has no device_id: not a v1 store", from.display()))?;
    let settings = Settings {
        source: SOURCE,
        ..Settings::load(home)?
    };
    let mut stats = Stats::default();
    events(&v1, raw, &device, &settings, &mut stats)?;
    deleted(&v1, raw, &settings, &mut stats)?;
    repos(&v1, raw, &settings, &mut stats)?;
    documents(&v1, raw, &device, &settings, &mut stats)?;
    Ok((stats, fingerprint(&v1)?))
}

/// Spec 7.4, A106: v1's `config.toml` (beside its store) into a home that has none, unchanged; a
/// home's own file is never edited. Then what the home's file leaves unanswered, one line each.
pub fn settings(home: &Path, from: &Path) -> Result<Vec<String>> {
    let ours = home.join("config.toml");
    let theirs = from.with_file_name("config.toml");
    if !ours.exists() && theirs.exists() {
        crate::db::private(home, 0o700);
        // Whole or not at all: a copy cut short would read as a file with nothing set, which a
        // rerun keeps. A link, not a rename, so a config.toml written meanwhile is never replaced.
        let part = home.join("config.toml.part");
        match std::fs::remove_file(&part) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut staged = options.open(&part)?;
        std::io::copy(&mut std::fs::File::open(&theirs)?, &mut staged)
            .with_context(|| format!("copy {}", theirs.display()))?;
        staged.sync_all()?;
        drop(staged);
        match std::fs::hard_link(&part, &ours) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => std::fs::remove_file(&part)?,
        }
    }
    let text = match std::fs::read_to_string(&ours) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", ours.display())),
    };
    // Only the line in an error: the text can quote a value `[redaction]` hides.
    let table: toml::Table = text
        .parse()
        .map_err(|e| crate::config::toml_error(&text, &e))
        .with_context(|| format!("parse {}", ours.display()))?;
    let mut lines = Vec::new();
    if table.get("summary").and_then(|s| s.get("curate")).is_none() {
        lines.push(
            "[summary] curate is not set: nothing is curated until it is (spec 7.4)".to_owned(),
        );
    }
    let unset: Vec<&str> = ["inject", "capture", "redaction", "chain"]
        .into_iter()
        .filter(|t| !table.contains_key(*t))
        .collect();
    if !unset.is_empty() {
        lines.push(format!(
            "not set, so their defaults apply: [{}]",
            unset.join("], [")
        ));
    }
    Ok(lines)
}

/// Doctor's lines on the cut-over (spec 7.4): v1's events not migrated yet, v1's old files until
/// `--finish`, and `eval/`, whose evaluation copies forget does not reach.
pub fn doctor(home: &Path) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let store = home.join("oboete.db");
    if store.exists() {
        let v1 = open_v1(&store)?;
        let device: Option<String> = v1
            .query_row("SELECT value FROM meta WHERE key = 'device_id'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(device) = device {
            let key = format!("{SOURCE}:{device}");
            let through = if crate::raw::exists(home) {
                let raw = crate::raw::open(home)?;
                raw.migration_checkpoints(&key)?
                    .remove(&key)
                    .map_or(0, |c| c.through)
            } else {
                0
            };
            let waiting: i64 = v1.query_row(
                "SELECT count(*) FROM events WHERE id > ?1",
                [through],
                |r| r.get(0),
            )?;
            lines.push(format!(
                "v1 events not migrated yet: {waiting} (`oboete migrate` imports them)"
            ));
        }
    }
    let old: Vec<String> = old_files(home)?
        .iter()
        .map(|(path, bytes)| format!("{} ({bytes} bytes)", path.display()))
        .collect();
    if !old.is_empty() {
        lines.push(format!(
            "v1's old files, kept until `oboete migrate --finish`: {}",
            old.join(", ")
        ));
    }
    let eval = home.join("eval");
    if eval.is_dir() {
        lines.push(format!(
            "evaluation copies, which forget does not reach: {} ({} bytes)",
            eval.display(),
            size(&eval)?
        ));
    }
    Ok(lines)
}

/// `oboete migrate --finish` (spec 7.4, A58): one more pass; then v1's old files in the home with
/// their sizes, and the imported v1 sessions oboete.db no longer holds; then one line from
/// `answer`. Only on `yes`, and only while v1 has written nothing since the pass, the files are
/// deleted, each failure reported. It reads the home's own store: the files are the home's, so
/// `--from` is refused with it.
pub fn finish(
    home: &Path,
    mut answer: impl std::io::BufRead,
    out: &mut impl std::io::Write,
) -> Result<()> {
    let from = &home.join("oboete.db");
    // Open to the end: its shared lock keeps a restore from swapping raw.db, with the batches the
    // pass just checked, while the answer is read and v1 is deleted.
    let mut raw = crate::raw::open(home)?;
    let (stats, before) = read_pass(home, &mut raw, from)?;
    let mut files = old_files(home)?;
    writeln!(out, "v1's old files in {}:", home.display())?;
    for (path, bytes) in &files {
        writeln!(out, "  {} ({bytes} bytes)", path.display())?;
    }
    if !stats.deleted.is_empty() {
        writeln!(
            out,
            "v1 sessions deleted from oboete.db after they were imported (forget them to remove \
             them here too): {}",
            stats.deleted.join(", ")
        )?;
    }
    if !stats.uncertain.is_empty() {
        writeln!(
            out,
            "v1 session deletion cannot be determined (earlier redaction or clipping): {}",
            stats.uncertain.join(", ")
        )?;
    }
    write!(out, "Delete these files? Type yes to delete them: ")?;
    out.flush()?;
    let mut line = String::new();
    answer.read_line(&mut line)?;
    if line.trim() != "yes" {
        writeln!(out, "Nothing was deleted.")?;
        return Ok(());
    }
    let now = {
        let v1 = open_v1(from)?;
        v1.execute_batch("BEGIN")?;
        fingerprint(&v1)?
    };
    anyhow::ensure!(
        now == before,
        "oboete.db changed after the import pass (an old hook still writes to it): nothing was \
         deleted; run `oboete migrate --finish` again"
    );
    // The store first, right after the recheck, since a v1 write between the two is lost.
    // ponytail: so is one by a v1 process that still holds oboete.db open after it is deleted; spec
    // 7.5 runs `--finish` once the sessions started before the switch have restarted, and the
    // recheck catches most that have not. Excluding v1's writers would need v1's write lock.
    files.sort_by_key(|(path, _)| {
        !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("oboete.db"))
    });
    let mut failed = 0;
    for (path, _) in &files {
        // `remove_dir_all` removes a link, never what it points to.
        let gone = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match gone {
            Ok(()) => writeln!(out, "deleted {}", path.display())?,
            Err(e) => {
                failed += 1;
                writeln!(out, "not deleted {}: {e}", path.display())?;
            }
        }
    }
    anyhow::ensure!(failed == 0, "{failed} file(s) not deleted");
    Ok(())
}

/// v1's runtime files in the home, spec 7.4's list (what ~/.oboete held on WSL on 2026-09-25):
/// oboete.db and memory.db with their -wal and -shm, the pre-*.db snapshots with theirs, and the
/// pre-rollout-*, spool, cache and logs directories; with their sizes, a directory's in all.
/// `eval/` is not among them.
fn old_files(home: &Path) -> Result<Vec<(std::path::PathBuf, u64)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(home)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.file_type()?.is_dir();
        let db = |stem: &str| {
            ["", "-wal", "-shm"]
                .iter()
                .any(|end| name == format!("{stem}{end}"))
        };
        let snapshot = name.starts_with("pre-")
            && [".db", ".db-wal", ".db-shm"]
                .iter()
                .any(|e| name.ends_with(e));
        let old = if dir {
            name.starts_with("pre-rollout-") || ["spool", "cache", "logs"].contains(&name.as_str())
        } else {
            db("oboete.db") || db("memory.db") || snapshot
        };
        if old {
            files.push((entry.path(), size(&entry.path())?));
        }
    }
    files.sort();
    Ok(files)
}

fn size(path: &Path) -> Result<u64> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        total += size(&entry?.path())?;
    }
    Ok(total)
}

/// v1's events past the checkpoint, read in id order and cut into batches by `IMPORT_BATCH`
/// records and `MAX_BATCH_BYTES` of bodies, each appended in (ts, id) order with its checkpoint:
/// its highest id and that event's fingerprint (D6).
fn events(
    v1: &Connection,
    raw: &mut Raw,
    device: &str,
    settings: &Settings,
    stats: &mut Stats,
) -> Result<()> {
    let key = format!("{SOURCE}:{device}");
    let through = match raw.migration_checkpoints(&key)?.remove(&key) {
        None => 0,
        Some(c) => {
            // v1 reuses the ids of its newest events when the old viewer deletes their session.
            let now = row_at(v1, c.through)?;
            anyhow::ensure!(
                now.is_some() && now == c.row,
                "v1's event {} is not the one an earlier pass imported up to: the old viewer \
                 deleted the newest session since, and v1 may give its ids to new events, which \
                 this pass would skip. Nothing was imported",
                c.through
            );
            c.through
        }
    };
    // ponytail: an event whose session row is gone is passed over; v1 deletes a session's events
    // with it (`db::delete_session`), and the owner's store held none on 2026-10-01.
    let mut st = v1.prepare(
        "SELECT e.id, e.ts, e.session_id, e.event, e.payload, s.agent, s.repo, s.cwd
         FROM events e JOIN sessions s ON s.id = e.session_id WHERE e.id > ?1 ORDER BY e.id",
    )?;
    let mut rows = st.query([through])?;
    let ruleset = settings.rules.version();
    let mut batch = Batch::default();
    while let Some(r) = rows.next()? {
        let row = v1_row(r)?;
        let (session, event, stored): (String, String, String) = (r.get(2)?, r.get(3)?, r.get(4)?);
        let (agent, repo, cwd): (String, String, Option<String>) =
            (r.get(5)?, r.get(6)?, r.get(7)?);
        let payload = payload(&session, &stored);
        let captured = capture::imported(
            &agent,
            &event,
            &payload,
            row.ts,
            &repo,
            cwd.as_deref(),
            settings,
        );
        let bytes: usize = captured.iter().map(|c| c.event.body.len()).sum();
        if batch.records.len() + captured.len() > IMPORT_BATCH
            || batch.bytes + bytes > MAX_BATCH_BYTES
        {
            stats.records += batch.append(raw, &key, ruleset)?;
        }
        batch.bytes += bytes;
        batch.records.extend(captured);
        batch.last = Some(row);
        stats.events += 1;
    }
    stats.records += batch.append(raw, &key, ruleset)?;
    Ok(())
}

/// v1 events read since the last append, in v1's id order: their records, and the last one's
/// fingerprint.
#[derive(Default)]
struct Batch {
    records: Vec<Captured>,
    bytes: usize,
    last: Option<V1Row>,
}

impl Batch {
    /// The records in (ts, id) order with the checkpoint, also when none was recorded; nothing
    /// when no event was read. The records appended.
    fn append(&mut self, raw: &mut Raw, key: &str, ruleset: &str) -> Result<u64> {
        let Some(last) = self.last.take() else {
            return Ok(0);
        };
        let mut records = std::mem::take(&mut self.records);
        // Stable, so (ts, id): at most one record per event, read in id order.
        records.sort_by_key(|c| c.event.ts);
        self.bytes = 0;
        let checkpoint = Checkpoint {
            key: key.to_owned(),
            through: last.id,
            row: Some(last),
            prefix: None,
        };
        Ok(raw
            .append_imported(&records, ruleset, Some(&checkpoint))?
            .len() as u64)
    }
}

/// A v1 event's stored body (v1's hook kept these fields of each event) as the hook payload
/// capture reads. It has no `transcript_path`, so capture reads no transcript.
fn payload(session: &str, stored: &str) -> Value {
    let stored: Value = serde_json::from_str(stored).unwrap_or(Value::Null);
    let mut payload = json!({"session_id": session});
    for (v1, hook) in [
        ("prompt", "prompt"),
        ("source", "source"),
        ("tool", "tool_name"),
        ("input", "tool_input"),
        ("output", "tool_response"),
        ("assistant", "last_assistant_message"),
        ("summary", "compact_summary"),
        ("reason", "reason"),
    ] {
        if let Some(v) = stored.get(v1) {
            payload[hook] = v.clone();
        }
    }
    payload
}

/// The imported v1 sessions none of whose events oboete.db still holds.
fn deleted(v1: &Connection, raw: &Raw, settings: &Settings, stats: &mut Stats) -> Result<()> {
    // Design B's labels passed the gate: v1's ids are compared as capture labels them, and each
    // is printed as the rules read now.
    let label = |id: &str| {
        let payload = json!({ "session_id": id });
        capture::imported("claude", "Touch", &payload, 0, "", None, settings)
            .pop()
            .map(|c| c.event.session)
            .unwrap_or_default()
    };
    let mut st = v1.prepare("SELECT DISTINCT session_id FROM events")?;
    let held = st
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|id| id.map(|id| label(&id)))
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    for stored in raw.sessions_of(SOURCE)? {
        let current = label(&stored);
        if held.contains(&current) {
            continue;
        }
        if stored.contains(crate::redact::MASK) || stored.contains("\n…[cut: ") {
            stats.uncertain.push(current);
        } else {
            stats.deleted.push(current);
        }
    }
    Ok(())
}

/// v1's `session_repos` rows as `touch` records at their session's start (A57), so the
/// repositories a v1 session touched stay known; a row whose labels Design B holds is passed over.
fn repos(v1: &Connection, raw: &mut Raw, settings: &Settings, stats: &mut Stats) -> Result<()> {
    let mut known = raw.touches(SOURCE)?;
    let mut st = v1.prepare(
        "SELECT r.session_id, r.repo, s.agent, s.cwd, s.started_at
         FROM session_repos r JOIN sessions s ON s.id = r.session_id
         ORDER BY s.started_at, r.session_id, r.repo",
    )?;
    let mut rows = st.query([])?;
    let ruleset = settings.rules.version();
    let mut batch = Vec::new();
    while let Some(r) = rows.next()? {
        let (session, repo, agent): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let (cwd, started): (Option<String>, i64) = (r.get(3)?, r.get(4)?);
        let payload = json!({"session_id": session});
        for c in capture::imported(
            &agent,
            "Touch",
            &payload,
            started,
            &repo,
            cwd.as_deref(),
            settings,
        ) {
            if known.insert((c.event.session.clone(), c.event.repo.clone())) {
                batch.push(c);
            }
        }
    }
    for chunk in batch.chunks(IMPORT_BATCH) {
        stats.repos += raw.append_imported(chunk, ruleset, None)?.len() as u64;
    }
    Ok(())
}

/// v1's observations, summaries and prompts as import ops (D5), under their uids (`<device_id>:
/// <o|s|p><id>` where v1 left it NULL) and through the gate, each once by its source id.
fn documents(
    v1: &Connection,
    raw: &mut Raw,
    device: &str,
    settings: &Settings,
    stats: &mut Stats,
) -> Result<()> {
    let source = format!("{SOURCE}:{device}");
    let mut known = raw.import_keys(&source)?;
    let gate = |text: String| crate::redact::outbound_with(&text, &settings.rules);
    let mut docs = Vec::new();
    for (letter, sql) in [
        (
            "o",
            "SELECT id, uid, session_id, repo, ts, kind, title, body FROM observations ORDER BY id",
        ),
        (
            "s",
            "SELECT id, uid, session_id, repo, ts, 'summary', '', body FROM summaries ORDER BY id",
        ),
        (
            "p",
            "SELECT id, uid, session_id, repo, ts, 'prompt', '', body FROM prompts ORDER BY id",
        ),
    ] {
        let mut st = v1.prepare(sql)?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let source_id = format!("{letter}{}", r.get::<_, i64>(0)?);
            if !known.insert(source_id.clone()) {
                stats.seen += 1;
                continue;
            }
            let uid: Option<String> = r.get(1)?;
            docs.push(ImportDoc {
                uid: uid.unwrap_or_else(|| format!("{device}:{source_id}")),
                source: source.clone(),
                source_id,
                kind: r.get(5)?,
                // Labels are stored text too (spec 2.2), as capture gates a record's.
                repo: gate(r.get(3)?),
                session: gate(r.get(2)?),
                ts: r.get(4)?,
                title: gate(r.get(6)?),
                body: gate(r.get(7)?),
            });
        }
    }
    stats.documents = raw.append_imports(docs)? as u64;
    Ok(())
}

/// v1's store, read only (spec 7.4: the old store is left byte for byte), with the stores' busy
/// wait. Never `immutable`, which skips the WAL, where the newest events are, and assumes no one
/// writes: v1's hooks may write meanwhile.
pub fn open_v1(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{self, Event, Item, OpKind};
    use rusqlite::params;
    use std::path::PathBuf;

    /// v1's tables as its `db.rs` left them, `uid` columns included, in plain SQL: a fixture no
    /// Design B code writes.
    const V1_SCHEMA: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT NOT NULL, repo TEXT NOT NULL, cwd TEXT,
  started_at INTEGER NOT NULL, ended_at INTEGER, last_event_at INTEGER NOT NULL,
  injected_at INTEGER, last_prompt_step INTEGER, reinject_pending INTEGER NOT NULL DEFAULT 0,
  observed_event_id INTEGER NOT NULL DEFAULT 0);
CREATE TABLE session_repos(session_id TEXT NOT NULL, repo TEXT NOT NULL,
  PRIMARY KEY(session_id, repo)) WITHOUT ROWID;
CREATE TABLE events(id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, event TEXT NOT NULL,
  ts INTEGER NOT NULL, payload TEXT NOT NULL);
CREATE INDEX events_session ON events(session_id, id);
CREATE TABLE observations(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
  repo TEXT NOT NULL, ts INTEGER NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL,
  body TEXT NOT NULL, provider TEXT NOT NULL, uid TEXT);
CREATE TABLE summaries(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
  repo TEXT NOT NULL, ts INTEGER NOT NULL, body TEXT NOT NULL, provider TEXT NOT NULL, uid TEXT);
CREATE TABLE prompts(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
  repo TEXT NOT NULL, ts INTEGER NOT NULL, body TEXT NOT NULL, uid TEXT);
INSERT INTO meta VALUES('device_id', 'd1e5');
";

    /// A v1 store at `<dir>/oboete.db`, in WAL mode as v1 ran.
    struct V1 {
        path: PathBuf,
        conn: Connection,
    }

    impl V1 {
        fn new(dir: &Path) -> Self {
            let path = dir.join("oboete.db");
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.execute_batch(V1_SCHEMA).unwrap();
            Self { path, conn }
        }

        /// A Claude Code session in `repo`, with the `session_repos` row v1 wrote for it.
        fn session(&self, id: &str, repo: &str, started: i64) {
            self.conn
                .execute(
                    "INSERT INTO sessions(id, agent, repo, cwd, started_at, last_event_at)
                     VALUES(?1, 'claude', ?2, ?3, ?4, ?4)",
                    params![id, repo, format!("/w/{id}"), started],
                )
                .unwrap();
            self.touched(id, repo);
        }

        fn touched(&self, session: &str, repo: &str) {
            self.conn
                .execute(
                    "INSERT INTO session_repos VALUES(?1, ?2)",
                    params![session, repo],
                )
                .unwrap();
        }

        fn event(&self, session: &str, event: &str, ts: i64, stored: Value) -> i64 {
            self.conn
                .execute(
                    "INSERT INTO events(session_id, event, ts, payload) VALUES(?1, ?2, ?3, ?4)",
                    params![session, event, ts, stored.to_string()],
                )
                .unwrap();
            self.conn.last_insert_rowid()
        }

        fn prompt(&self, session: &str, ts: i64, text: &str) -> i64 {
            self.event(session, "UserPromptSubmit", ts, json!({"prompt": text}))
        }

        fn observation(&self, session: &str, ts: i64, title: &str, body: &str) {
            self.conn
                .execute(
                    "INSERT INTO observations(session_id, repo, ts, kind, title, body, provider)
                     VALUES(?1, 'github.com/o/r', ?2, 'decision', ?3, ?4, 'groq')",
                    params![session, ts, title, body],
                )
                .unwrap();
        }

        /// What the old viewer's `db::delete_session` takes.
        fn delete_session(&self, id: &str) {
            for sql in [
                "DELETE FROM events WHERE session_id = ?1",
                "DELETE FROM session_repos WHERE session_id = ?1",
                "DELETE FROM sessions WHERE id = ?1",
            ] {
                self.conn.execute(sql, [id]).unwrap();
            }
        }
    }

    /// Design B's events, in seq order.
    fn records(raw: &Raw) -> Vec<Event> {
        let (mut out, mut after) = (Vec::new(), 0);
        loop {
            let page = raw.after(raw.device(), after, 1_000).unwrap();
            let Some(last) = page.last() else {
                return out;
            };
            after = last.seq;
            out.extend(page.into_iter().filter_map(|r| match r.item {
                Item::Event(e) => Some(*e),
                _ => None,
            }));
        }
    }

    fn body(e: &Event) -> Value {
        serde_json::from_str(&e.body).unwrap()
    }

    fn of_kind(raw: &Raw, kind: &str) -> Vec<Value> {
        records(raw)
            .iter()
            .filter(|e| e.kind == kind)
            .map(body)
            .collect()
    }

    /// D6: v1's events become `oboete-v1` records of B's kinds with v1's session, agent, repo and
    /// cwd, in (ts, id) order within a batch, redacted with the rules as they are now; with prompts
    /// off, a prompt is a turn without its text.
    #[test]
    fn v1_events_become_oboete_v1_records() {
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "github.com/o/r", 100);
        v1.event("s1", "SessionStart", 150, json!({"source": "resume"}));
        v1.prompt("s1", 300, "use tabs");
        let tool = json!({"tool": "Bash", "input": "{\"command\":\"ls\"}", "output": "ok",
            "failed": false});
        v1.event("s1", "PostToolUse", 200, tool);
        let reply = json!({"assistant": format!("done with {token}")});
        v1.event("s1", "Stop", 400, reply);
        let failed = json!({"tool": "Bash", "input": "x", "output": "boom", "failed": true});
        v1.event("s1", "PostToolUseFailure", 500, failed);
        v1.event("s1", "PostCompact", 550, json!({"summary": "compacted"}));
        v1.event("s1", "SessionEnd", 600, json!({"reason": "exit"}));
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((stats.events, stats.records, stats.repos), (7, 7, 1));
        let got = records(&raw);
        let kinds: Vec<(&str, i64)> = got.iter().map(|e| (e.kind.as_str(), e.ts)).collect();
        assert_eq!(
            kinds,
            [
                ("start", 150),
                ("tool", 200),
                ("prompt", 300),
                ("reply", 400),
                ("tool", 500),
                ("compaction", 550),
                ("end", 600),
                ("touch", 100)
            ]
        );
        for e in &got {
            let labels = (
                e.agent.as_str(),
                e.session.as_str(),
                e.repo.as_deref(),
                e.cwd.as_deref(),
                e.source.as_str(),
            );
            let want = (
                "claude",
                "s1",
                Some("github.com/o/r"),
                Some("/w/s1"),
                "oboete-v1",
            );
            assert_eq!(labels, want);
            assert_eq!((&e.branch, &e.head), (&None, &None));
        }
        assert_eq!(body(&got[0]), json!({"source": "resume"}));
        let tool = body(&got[1]);
        let tool = (tool["tool"].as_str(), tool["output"].as_str());
        assert_eq!(tool, (Some("Bash"), Some("ok")));
        assert_eq!(body(&got[2]), json!({"prompt": "use tabs"}));
        assert!(!got[3].body.contains(&token) && got[3].body.contains("[REDACTED]"));
        assert_eq!(body(&got[4])["failed"], true);
        assert_eq!(body(&got[5]), json!({"summary": "compacted"}));
        assert_eq!(body(&got[6]), json!({"reason": "exit"}));
        assert_eq!(got[7].body, "{}");
        // A store without v1's device id is not v1's: refused before anything is read.
        v1.conn.execute("DELETE FROM meta", []).unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut raw = raw::open(other.path()).unwrap();
        let refused = pass(other.path(), &mut raw, &v1.path).unwrap_err();
        assert!(
            format!("{refused:#}").contains("not a v1 store"),
            "{refused:#}"
        );
        assert_eq!(raw.max_seq().unwrap(), 0);
        v1.conn
            .execute("INSERT INTO meta VALUES('device_id', 'd1e5')", [])
            .unwrap();
        // With prompts off, the turn stays and its text does not.
        let off = tempfile::tempdir().unwrap();
        std::fs::write(
            off.path().join("config.toml"),
            "[capture]\nstore_prompts = false\n",
        )
        .unwrap();
        let mut raw = raw::open(off.path()).unwrap();
        pass(off.path(), &mut raw, &v1.path).unwrap();
        assert_eq!(of_kind(&raw, "prompt"), [json!({"omitted": true})]);
    }

    /// Spec 7.4: a rerun imports nothing twice, and then only what v1 wrote since: events,
    /// `session_repos` rows and documents.
    #[test]
    fn a_rerun_appends_only_new_v1_events() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "github.com/o/r", 100);
        v1.prompt("s1", 200, "one");
        v1.observation("s1", 210, "Tabs", "We use tabs.");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let first = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((first.records, first.repos, first.documents), (1, 1, 1));
        let again = pass(home.path(), &mut raw, &v1.path).unwrap();
        let nothing = Stats {
            seen: 1,
            ..Stats::default()
        };
        assert_eq!(again, nothing);
        v1.prompt("s1", 300, "two");
        v1.touched("s1", "github.com/o/lib");
        v1.observation("s1", 310, "Spaces", "Not spaces.");
        let next = pass(home.path(), &mut raw, &v1.path).unwrap();
        let counts = (
            next.events,
            next.records,
            next.repos,
            next.documents,
            next.seen,
        );
        assert_eq!(counts, (1, 1, 1, 1, 1));
        let prompts = of_kind(&raw, "prompt");
        assert_eq!(
            prompts,
            [json!({"prompt": "one"}), json!({"prompt": "two"})]
        );
    }

    /// Each listed id reads as the rules read now: a rule added after the import hides it.
    #[test]
    fn a_deleted_session_is_listed_as_the_rules_read_now() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("acme-123456", "r", 100);
        v1.prompt("acme-123456", 101, "one");
        v1.session("b", "r", 200);
        v1.prompt("b", 201, "two");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        pass(home.path(), &mut raw, &v1.path).unwrap();
        v1.delete_session("acme-123456");
        let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
        std::fs::write(home.path().join("config.toml"), rule).unwrap();
        let listed = pass(home.path(), &mut raw, &v1.path).unwrap().deleted;
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert!(!listed[0].contains("acme-123456"), "{listed:?}");
    }

    #[test]
    fn a_stricter_rule_does_not_list_a_held_v1_session_as_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("acme-123456", "r", 100);
        v1.prompt("acme-123456", 101, "one");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().records, 1);
        let rule = "[redaction]\nextra_rules = [{ id = \"acme\", regex = 'acme-[0-9]{6}' }]\n";
        std::fs::write(home.path().join("config.toml"), rule).unwrap();
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert!(stats.deleted.is_empty(), "{:?}", stats.deleted);
    }

    #[test]
    fn prior_redaction_reports_uncertainty_without_recommending_forget() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("acme-123456", "r", 100);
        v1.prompt("acme-123456", 101, "one");
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        std::fs::write(
            &config,
            "[redaction]\nextra_rules = [{ id = 'part', regex = '[0-9]{6}' }]\n",
        )
        .unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().records, 1);
        std::fs::write(
            &config,
            "[redaction]\nextra_rules = [{ id = 'whole', regex = 'acme-[0-9]{6}' }]\n",
        )
        .unwrap();
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert!(stats.deleted.is_empty(), "{:?}", stats.deleted);
        let report = serde_json::to_value(&stats).unwrap();
        assert_eq!(report["uncertain"], json!(["acme-[REDACTED]"]));
        drop(raw);
        v1.conn
            .execute(
                "VACUUM INTO ?1",
                [home.path().join("oboete.db").to_str().unwrap()],
            )
            .unwrap();
        let mut out = Vec::new();
        finish(home.path(), "no\n".as_bytes(), &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains("cannot be determined") && said.contains("acme-[REDACTED]"),
            "{said}"
        );
        assert!(!said.contains("forget them"), "{said}");
    }

    /// D6: v1 gives the ids of deleted newest events to new ones, so a pass refuses when the
    /// event at its checkpoint is gone or another, before it imports anything.
    #[test]
    fn a_reused_id_past_a_deleted_newest_session_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("a", "r", 100);
        v1.prompt("a", 110, "a one");
        v1.session("b", "r", 200);
        v1.prompt("b", 210, "b one");
        v1.prompt("b", 220, "b two");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().records, 3);
        let top = (raw.max_seq().unwrap(), raw.max_op_seq().unwrap());
        v1.delete_session("b");
        let refused = pass(home.path(), &mut raw, &v1.path).unwrap_err();
        assert!(
            format!("{refused:#}").contains("deleted the newest session"),
            "{refused:#}"
        );
        // Hooks write more events than it held: ids 2 and 3 again, and 4.
        v1.session("c", "r", 300);
        for ts in 310..313 {
            v1.prompt("c", ts, "c");
        }
        assert!(pass(home.path(), &mut raw, &v1.path).is_err());
        assert_eq!((raw.max_seq().unwrap(), raw.max_op_seq().unwrap()), top);
    }

    /// Spec 7.4, A105: a session the old viewer deleted after a pass is listed by the next, so the
    /// owner can forget it in Design B.
    #[test]
    fn deleted_v1_sessions_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        for (i, id) in ["a", "b", "c"].into_iter().enumerate() {
            v1.session(id, "r", 100 * i as i64);
            v1.prompt(id, 100 * i as i64 + 1, id);
        }
        // An id the gate masks: its original identifier cannot be recovered from its label.
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let masked = format!("s-{token}");
        v1.session(&masked, "r", 400);
        v1.prompt(&masked, 401, "four");
        // Not the newest: deleting the newest session is refused (D6).
        v1.session("d", "r", 500);
        v1.prompt("d", 501, "five");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert!(stats.deleted.is_empty());
        assert!(stats.uncertain.is_empty(), "{:?}", stats.uncertain);
        v1.delete_session("a");
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!(stats.deleted, ["a"]);
        assert!(stats.uncertain.is_empty(), "{:?}", stats.uncertain);
        v1.delete_session(&masked);
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!(stats.deleted, ["a"]);
        assert_eq!(stats.uncertain.len(), 1, "{:?}", stats.uncertain);
        assert!(stats.deleted.iter().all(|s| !s.contains(&token)));
        assert!(stats.uncertain.iter().all(|s| !s.contains(&token)));
        // And by `--finish`, from the home's own store.
        drop(raw);
        let h = home.path();
        let own = h.join("oboete.db");
        v1.conn
            .execute("VACUUM INTO ?1", [own.to_str().unwrap()])
            .unwrap();
        let mut out = Vec::new();
        finish(h, "no\n".as_bytes(), &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains("here too): a\n")
                && said.contains("cannot be determined")
                && !said.contains(&token),
            "{said}"
        );
    }

    /// Spec 7.4: each batch commits with its checkpoint, so a pass killed between two resumes after
    /// the first, and every event lands once, in order.
    #[test]
    fn a_killed_migrate_resumes_without_duplicates_or_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "r", 100);
        // 100 tool calls of 60 KB: past `MAX_BATCH_BYTES`, so two appends.
        for i in 0..100 {
            let output = format!("{i:03}{}", "x".repeat(60_000));
            let stored = json!({"tool": "Read", "input": format!("file {i}"), "output": output});
            v1.event("s1", "PostToolUse", 1_000 + i, stored);
        }
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        crate::crash::at(2);
        let killed = pass(home.path(), &mut raw, &v1.path);
        crate::crash::off();
        assert!(killed.is_err());
        let landed = of_kind(&raw, "tool").len();
        assert!(landed > 0 && landed < 100, "{landed}");
        pass(home.path(), &mut raw, &v1.path).unwrap();
        let inputs: Vec<String> = of_kind(&raw, "tool")
            .iter()
            .map(|b| b["input"].as_str().unwrap().to_owned())
            .collect();
        let want: Vec<String> = (0..100).map(|i| format!("file {i}")).collect();
        assert_eq!(inputs, want);
    }

    /// D5: `append_imports` commits `IMPORT_BATCH` documents at a time, so a pass stopped between
    /// two keeps the first and the next adds the rest, once.
    #[test]
    fn a_killed_pass_keeps_the_documents_it_appended() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "r", 100);
        for i in 0..600 {
            v1.observation("s1", 110 + i, "Note", &format!("Note {i}."));
        }
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        // The touch label commits first, then the documents' first batch.
        crate::crash::at(3);
        let killed = pass(home.path(), &mut raw, &v1.path);
        crate::crash::off();
        assert!(killed.is_err());
        assert_eq!(
            raw.import_keys("oboete-v1:d1e5").unwrap().len(),
            IMPORT_BATCH
        );
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((stats.documents, stats.seen), (100, 500));
    }

    /// D6: events that record nothing still move the checkpoint, so the next pass does not read
    /// them again.
    #[test]
    fn a_batch_with_nothing_to_record_moves_the_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "r", 100);
        v1.event("s1", "Stop", 110, json!({"assistant": ""}));
        v1.event("s1", "PreToolUse", 120, json!({}));
        v1.prompt("s1", 130, "<private>not for anyone</private>");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((stats.events, stats.records), (3, 0));
        let checkpoints = raw.migration_checkpoints("oboete-v1:").unwrap();
        assert_eq!(checkpoints["oboete-v1:d1e5"].through, 3);
        assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().events, 0);
    }

    /// A57, D6: v1's `session_repos` rows become `touch` records at their session's start, which a
    /// restore keeps, and a pass after it adds none again.
    #[test]
    fn session_repos_become_label_records_that_survive_a_restore() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "github.com/o/r", 100);
        v1.touched("s1", "github.com/o/lib");
        v1.prompt("s1", 110, "one");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().repos, 2);
        let touches = raw.touches("oboete-v1").unwrap();
        let label = |repo: &str| ("s1".to_owned(), Some(repo.to_owned()));
        let want = [label("github.com/o/lib"), label("github.com/o/r")];
        assert_eq!(touches, want.into_iter().collect());
        assert!(
            records(&raw)
                .iter()
                .all(|e| e.kind != "touch" || e.ts == 100)
        );
        drop(raw);
        crate::backup::export(home.path()).unwrap();
        std::fs::write(home.path().join("raw.db"), b"not a database at all").unwrap();
        for f in ["raw.db-wal", "raw.db-shm"] {
            let _ = std::fs::remove_file(home.path().join(f));
        }
        crate::worker::run_once(home.path()).unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        assert_eq!(raw.touches("oboete-v1").unwrap(), touches);
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((stats.records, stats.repos), (0, 0));
    }

    /// Row 30-1: a v1 session that touched an excluded repository, which only its `session_repos`
    /// row tells, reaches no curator, as a live one does not; before the exclusion it would.
    #[test]
    fn an_excluded_repo_through_session_repos_reaches_no_curator() {
        use crate::curate::{Reading, Reads, Span, WINDOW_TOKENS, span_windows};
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("x", "github.com/o/open", 100);
        v1.touched("x", "github.com/o/secret");
        v1.prompt("x", 110, "Secret plan.");
        v1.session("y", "github.com/o/open", 200);
        v1.prompt("y", 210, "Open work.");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        pass(home.path(), &mut raw, &v1.path).unwrap();
        let rules = crate::redact::Rules::default();
        let sent = |raw: &Raw| {
            let reading = Reading::now(raw, Reads::Source("oboete-v1".into())).unwrap();
            let span = Span::records(1, raw.max_seq().unwrap());
            span_windows(raw, &span, WINDOW_TOKENS, &rules, &reading)
                .unwrap()
                .into_iter()
                .map(|w| w.text)
                .collect::<String>()
        };
        let before = sent(&raw);
        assert!(before.contains("Secret plan.") && before.contains("Open work."));
        raw.exclude("github.com/o/secret", false).unwrap();
        let after = sent(&raw);
        assert!(!after.contains("Secret plan.") && after.contains("Open work."));
    }

    /// D5: v1's observations, summaries and prompts become import ops under their uids (v1's own,
    /// a claude-mem one it imported kept), which the worker resolves, never injected.
    #[test]
    fn v1_documents_become_import_ops_under_their_uids() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "github.com/o/r", 100);
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        v1.observation("s1", 110, "Tabs", &format!("We use tabs. {token}"));
        v1.observation("s1", 120, "Imported", "From claude-mem.");
        v1.conn
            .execute_batch(
                "UPDATE observations SET uid = 'claude-mem:abc:o5' WHERE id = 2;
                 INSERT INTO summaries(session_id, repo, ts, body, provider)
                   VALUES('s1', 'github.com/o/r', 130, 'Request: tabs', 'groq');
                 INSERT INTO prompts(session_id, repo, ts, body)
                   VALUES('s1', 'github.com/o/r', 140, 'use tabs');",
            )
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().documents, 4);
        let device = raw.device().to_owned();
        let docs: Vec<ImportDoc> = raw
            .ops_after(&device, 0, 100)
            .unwrap()
            .into_iter()
            .filter(|o| o.kind == raw::OpKind::Import)
            .map(|o| serde_json::from_value(o.body).unwrap())
            .collect();
        let keys: Vec<(&str, &str, &str, &str)> = docs
            .iter()
            .map(|d| {
                (
                    d.uid.as_str(),
                    d.source.as_str(),
                    d.source_id.as_str(),
                    d.kind.as_str(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("d1e5:o1", "oboete-v1:d1e5", "o1", "decision"),
                ("claude-mem:abc:o5", "oboete-v1:d1e5", "o2", "decision"),
                ("d1e5:s1", "oboete-v1:d1e5", "s1", "summary"),
                ("d1e5:p1", "oboete-v1:d1e5", "p1", "prompt"),
            ]
        );
        assert_eq!(docs[0].body, "We use tabs. [REDACTED]");
        drop(raw);
        crate::worker::run_once(home.path()).unwrap();
        let k = crate::knowledge::open(home.path()).unwrap();
        let resolved: i64 = k
            .query_row("SELECT count(*) FROM imported", [], |r| r.get(0))
            .unwrap();
        assert_eq!(resolved, 4);
        let raw = raw::open(home.path()).unwrap();
        let settings = Settings::default();
        let shown =
            crate::hook::start_text_read(home.path(), &raw, "github.com/o/r", "", "s1", &settings)
                .unwrap()
                .map(|s| s.text)
                .unwrap_or_default();
        assert!(!shown.contains("We use tabs."), "{shown}");
    }

    /// MUST-M9: migrated records move neither the manifest's facts nor its count of records not
    /// yet curated.
    #[test]
    fn migrated_records_never_move_the_manifest_or_backlog() {
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let live = |text: &str, ts: i64| Event {
            session: "L".into(),
            kind: "prompt".into(),
            repo: Some("github.com/o/r".into()),
            ts,
            body: json!({"prompt": text}).to_string(),
            ..raw::test_event("")
        };
        raw.append(&live("Live prompt.", 1_000)).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        let settings = Settings::default();
        // The backlog's age in minutes moves with the clock between the two reads.
        let age = regex::Regex::new(r"/ \d+ min").unwrap();
        let manifest = |raw: &Raw| {
            crate::hook::start_text_read(home.path(), raw, "github.com/o/r", "", "L", &settings)
                .unwrap()
                .map(|m| age.replace_all(&m.text, "/ _ min").into_owned())
        };
        let before = manifest(&raw);
        assert!(
            before
                .as_deref()
                .is_some_and(|m| m.contains("Live prompt.")),
            "{before:?}"
        );
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "github.com/o/r", 2_000);
        v1.prompt("s1", 2_100, "Old prompt.");
        let failed = json!({"tool": "Bash", "input": "make", "output": "boom", "failed": true});
        v1.event("s1", "PostToolUseFailure", 2_200, failed);
        pass(home.path(), &mut raw, &v1.path).unwrap();
        crate::worker::run_once(home.path()).unwrap();
        assert_eq!(manifest(&raw), before);
    }

    /// MUST-M16: hooks keep appending while a pass runs, every append succeeds, and some land
    /// between migrated batches.
    #[test]
    fn migrate_and_a_hook_append_together() {
        migrate_with_hooks(std::time::Duration::from_millis(1));
    }

    #[test]
    fn migrate_progresses_while_hooks_append_without_a_pause() {
        migrate_with_hooks(std::time::Duration::ZERO);
    }

    fn migrate_with_hooks(pause: std::time::Duration) {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "r", 100);
        for i in 0..1_200 {
            v1.prompt("s1", 1_000 + i, &format!("old {i}"));
        }
        v1.observation("s1", 2_200, "Migrated document", "Kept once.");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let done = std::sync::atomic::AtomicBool::new(false);
        let live: Vec<i64> = std::thread::scope(|s| {
            let hook = s.spawn(|| {
                let mut store = raw::open(home.path()).unwrap();
                let mut seqs = Vec::new();
                while !done.load(std::sync::atomic::Ordering::SeqCst) {
                    seqs.push(store.append(&raw::test_event("live")).unwrap());
                    std::thread::sleep(pause);
                }
                seqs
            });
            let passed = pass(home.path(), &mut raw, &v1.path);
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            let live = hook.join().unwrap();
            passed.unwrap();
            live
        });
        let all = records(&raw);
        let migrated: Vec<i64> = raw
            .after(raw.device(), 0, 100_000)
            .unwrap()
            .iter()
            .zip(&all)
            .filter(|(_, e)| e.source == "oboete-v1" && e.kind == "prompt")
            .map(|(r, _)| r.seq)
            .collect();
        assert_eq!(migrated.len(), 1_200);
        let (first, last) = (migrated[0], migrated[1_199]);
        assert!(live.iter().any(|s| (first..last).contains(s)), "{live:?}");
        assert_eq!(
            all.iter().filter(|e| e.source == "hook").count(),
            live.len()
        );
        assert_eq!(
            raw.migration_checkpoints("oboete-v1:").unwrap()["oboete-v1:d1e5"].through,
            1_200
        );
        let rerun = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((rerun.events, rerun.records, rerun.documents), (0, 0, 0));
        assert_eq!(rerun.seen, 1);
    }

    /// R05, A106: v1's config.toml goes unchanged into a home that has none, and loads under
    /// Design B with its providers in order, their models, and the key files by path; a home's
    /// own file is never edited; what the file leaves unanswered is printed.
    #[test]
    fn the_v1_config_loads_under_b() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        let text = r#"gemini = "after-subscriptions"

[[providers]]
kind = "openai"
name = "groq"
base_url = "https://api.groq.com/openai/v1"
key_file = "/k/GROQ_KEY.md"
model = "llama-3.3-70b-versatile"

[[providers]]
kind = "cli"
name = "codex"
cli = "codex"
model = "gpt-5.5"

[embedding]
provider = "workers-ai"
account_id = "acct"
key_file = "/k/CF_WORKERS_AI_KEY.md"
"#;
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        let home = tempfile::tempdir().unwrap();
        let lines = settings(home.path(), &v1.path).unwrap();
        let copied = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert_eq!(copied, text);
        assert!(
            lines[0].starts_with("[summary] curate is not set"),
            "{lines:?}"
        );
        let cfg = crate::config::load(home.path()).unwrap();
        let chain: Vec<(&str, Option<&str>, Option<&Path>)> = cfg
            .providers
            .iter()
            .map(|p| match p {
                crate::config::Provider::Openai {
                    name,
                    model,
                    key_file,
                    ..
                } => (name.as_str(), Some(model.as_str()), key_file.as_deref()),
                crate::config::Provider::Cli { name, model, .. } => {
                    (name.as_str(), model.as_deref(), None)
                }
            })
            .collect();
        assert_eq!(
            chain[..2],
            [
                (
                    "groq",
                    Some("llama-3.3-70b-versatile"),
                    Some(Path::new("/k/GROQ_KEY.md"))
                ),
                ("codex", Some("gpt-5.5"), None),
            ]
        );
        assert_eq!(chain.last().map(|c| c.0), Some("gemini"));
        let key = cfg.embedding.key_file.clone();
        assert_eq!(key, Path::new("/k/CF_WORKERS_AI_KEY.md"));
        assert!(!cfg.summary.curate);
        // A home with its own file keeps it as it is.
        let own = "[summary]\ncurate = true\n";
        std::fs::write(home.path().join("config.toml"), own).unwrap();
        let lines = settings(home.path(), &v1.path).unwrap();
        let kept = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert_eq!(kept, own);
        assert!(lines.iter().all(|l| !l.contains("curate")), "{lines:?}");
        // A copied file that does not parse: its line, never its text, which can hold a value
        // `[redaction]` hides.
        let secret = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let broken = format!("[redaction]\nallowlist = [{secret}]\n");
        std::fs::write(dir.path().join("config.toml"), broken).unwrap();
        let fresh = tempfile::tempdir().unwrap();
        // A copy an earlier run left cut short is not the file: it is copied again, whole.
        std::fs::write(fresh.path().join("config.toml.part"), "[redaction]\n").unwrap();
        let refused = settings(fresh.path(), &v1.path).unwrap_err();
        assert!(!fresh.path().join("config.toml.part").exists());
        let said = format!("{refused:#}");
        assert!(said.contains("line 2") && !said.contains(&secret), "{said}");
    }

    /// A58: `--finish` imports once more, lists v1's old files with their sizes, and deletes them
    /// only on `yes`, and only while v1 has written nothing since its pass; Design B's files and
    /// `eval/` stay; the store is deleted first.
    #[test]
    fn finish_deletes_only_on_yes() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let v1 = V1::new(h);
        v1.session("s1", "r", 100);
        v1.prompt("s1", 110, "one");
        for f in ["pre-1.db", "pre-1.db-wal", "memory.db", "notes.txt"] {
            std::fs::write(h.join(f), "x").unwrap();
        }
        for d in ["spool", "cache", "logs", "pre-rollout-1", "eval"] {
            std::fs::create_dir(h.join(d)).unwrap();
            std::fs::write(h.join(d).join("f"), "xyz").unwrap();
        }
        let listed = |out: &[u8]| String::from_utf8(out.to_vec()).unwrap();
        let mut out = Vec::new();
        finish(h, "no\n".as_bytes(), &mut out).unwrap();
        let said = listed(&out);
        assert!(said.contains("spool (3 bytes)") && said.ends_with("Nothing was deleted.\n"));
        assert!(
            !said.contains("eval") && !said.contains("notes.txt"),
            "{said}"
        );
        assert!(h.join("oboete.db").exists() && h.join("spool").exists());
        let raw = raw::open(h).unwrap();
        assert_eq!(of_kind(&raw, "prompt"), [json!({"prompt": "one"})]);
        drop(raw);
        // While the answer is read, raw.db stays open: a restore cannot swap it under the pass.
        struct Swapping<'a>(&'a Path, Option<bool>);
        impl std::io::Read for Swapping<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.1.is_some() {
                    return Ok(0);
                }
                let lock = std::fs::File::open(self.0.join("raw.lock"))?;
                self.1 = Some(lock.try_lock().is_err());
                buf[..3].copy_from_slice(b"no\n");
                Ok(3)
            }
        }
        let mut answer = std::io::BufReader::new(Swapping(h, None));
        finish(h, &mut answer, &mut Vec::new()).unwrap();
        assert_eq!(
            answer.into_inner().1,
            Some(true),
            "a restore could swap raw.db"
        );
        // An old hook writes while the answer is read: nothing is deleted.
        struct Writing<'a>(&'a V1, bool);
        impl std::io::Read for Writing<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if std::mem::replace(&mut self.1, true) {
                    return Ok(0);
                }
                self.0.prompt("s1", 120, "written meanwhile");
                buf[..4].copy_from_slice(b"yes\n");
                Ok(4)
            }
        }
        let answer = std::io::BufReader::new(Writing(&v1, false));
        let refused = finish(h, answer, &mut Vec::new()).unwrap_err();
        assert!(format!("{refused:#}").contains("changed after the import pass"));
        assert!(h.join("oboete.db").exists() && h.join("spool").exists());
        drop(v1);
        // A link named as an old directory goes, never what it points to.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("kept"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), h.join("memory.db-wal")).unwrap();
        let mut out = Vec::new();
        finish(h, "yes\n".as_bytes(), &mut out).unwrap();
        assert!(outside.path().join("kept").exists());
        // The store is deleted first, right after the recheck.
        // The answer is read on the question's line, which the first deletion's line ends.
        let said = listed(&out);
        let first = said
            .split_once("delete them: ")
            .and_then(|(_, a)| a.lines().next());
        let store = format!("deleted {}", h.join("oboete.db").display());
        assert_eq!(first, Some(store.as_str()));
        for gone in [
            "oboete.db",
            "pre-1.db",
            "pre-1.db-wal",
            "memory.db",
            "spool",
            "cache",
        ] {
            assert!(!h.join(gone).exists(), "{gone}: {}", listed(&out));
        }
        for kept in ["raw.db", "eval", "notes.txt"] {
            assert!(h.join(kept).exists(), "{kept}");
        }
        let raw = raw::open(h).unwrap();
        let prompts = of_kind(&raw, "prompt");
        assert_eq!(prompts.len(), 2, "the write during the answer was imported");
        // The old viewer deletes the newest session while the answer is read, and a hook writes
        // as many events again: the same highest ids, another newest event.
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let v1 = V1::new(h);
        v1.session("s1", "r", 100);
        v1.prompt("s1", 110, "one");
        v1.session("s2", "r", 200);
        v1.prompt("s2", 210, "two");
        struct Reusing<'a>(&'a V1, bool);
        impl std::io::Read for Reusing<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if std::mem::replace(&mut self.1, true) {
                    return Ok(0);
                }
                self.0.delete_session("s2");
                self.0.session("s3", "r", 300);
                self.0.prompt("s3", 310, "three");
                buf[..4].copy_from_slice(b"yes\n");
                Ok(4)
            }
        }
        let answer = std::io::BufReader::new(Reusing(&v1, false));
        let refused = finish(h, answer, &mut Vec::new()).unwrap_err();
        assert!(format!("{refused:#}").contains("changed after the import pass"));
        assert!(h.join("oboete.db").exists());
    }

    /// Spec 7.4: doctor counts v1's events not migrated yet, and lists v1's old files and the
    /// evaluation copies.
    #[test]
    fn doctor_names_what_the_cut_over_leaves() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let v1 = V1::new(h);
        v1.session("s1", "r", 100);
        v1.prompt("s1", 110, "one");
        v1.prompt("s1", 120, "two");
        let lines = doctor(h).unwrap();
        assert!(
            lines[0].starts_with("v1 events not migrated yet: 2 "),
            "{lines:?}"
        );
        let mut raw = raw::open(h).unwrap();
        pass(h, &mut raw, &v1.path).unwrap();
        v1.prompt("s1", 130, "three");
        std::fs::create_dir(h.join("eval")).unwrap();
        std::fs::write(h.join("eval").join("queries.jsonl"), "{}\n").unwrap();
        let lines = doctor(h).unwrap();
        assert!(
            lines[0].starts_with("v1 events not migrated yet: 1 "),
            "{lines:?}"
        );
        assert!(lines[1].contains("oboete.db ("), "{lines:?}");
        assert!(lines[2].ends_with("eval (3 bytes)"), "{lines:?}");
    }

    /// Labels are stored text (spec 2.2): what the gate masks in a v1 session id or a document's
    /// repository is kept nowhere, not in a record's labels, a document's or the checkpoint.
    #[test]
    fn no_label_or_checkpoint_keeps_what_the_gate_masks() {
        let token = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g");
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        let session = format!("s-{token}");
        v1.session(&session, "github.com/o/r", 100);
        v1.prompt(&session, 110, "one");
        v1.conn
            .execute(
                "INSERT INTO observations(session_id, repo, ts, kind, title, body, provider)
                 VALUES(?1, ?2, 120, 'decision', 'Tabs', 'We use tabs.', 'groq')",
                params![session, format!("github.com/o/{token}")],
            )
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((stats.records, stats.documents), (1, 1));
        let ops = raw.ops_after(raw.device(), 0, 100).unwrap();
        let kinds: Vec<OpKind> = ops.iter().map(|o| o.kind).collect();
        assert!(
            kinds.contains(&OpKind::Migration) && kinds.contains(&OpKind::Import),
            "{kinds:?}"
        );
        for op in &ops {
            assert!(!op.body.to_string().contains(&token), "{:?}", op.kind);
        }
        for e in records(&raw) {
            assert!(!format!("{e:?}").contains(&token), "{}", e.kind);
        }
        // The fingerprint still matches: a rerun is not refused.
        assert_eq!(pass(home.path(), &mut raw, &v1.path).unwrap().seen, 1);
    }

    /// D6, A104: a v1 event and a document `denied` refuses are left out; the others land.
    #[test]
    fn migrate_leaves_a_denied_event_and_document_out() {
        let forgotten = format!("x {} y", raw::DENIED_IN_TESTS);
        let dir = tempfile::tempdir().unwrap();
        let v1 = V1::new(dir.path());
        v1.session("s1", "r", 100);
        v1.prompt("s1", 110, &forgotten);
        v1.prompt("s1", 120, "kept");
        v1.observation("s1", 130, "Forgotten", &forgotten);
        v1.observation("s1", 140, "Kept", "Kept.");
        let home = tempfile::tempdir().unwrap();
        let mut raw = raw::open(home.path()).unwrap();
        let stats = pass(home.path(), &mut raw, &v1.path).unwrap();
        assert_eq!((stats.records, stats.documents), (1, 1));
        assert_eq!(of_kind(&raw, "prompt"), [json!({"prompt": "kept"})]);
        let keys = raw.import_keys("oboete-v1:d1e5").unwrap();
        assert_eq!(keys, ["o2".to_owned()].into_iter().collect());
    }

    fn sha(path: &Path) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
    }

    /// A v1 store with its newest event only in the WAL: the hashes of the store and its WAL,
    /// and its device id.
    fn v1_store(home: &Path) -> (String, String, String) {
        let conn = crate::db::open(home).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions(id, agent, repo, started_at, last_event_at)
               VALUES ('s', 'claude', 'r', 1, 1);
             INSERT INTO events(session_id, event, ts, payload)
               VALUES ('s', 'Stop', 1, '{\"assistant\": \"one\"}');",
        )
        .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint = 0")
            .unwrap();
        conn.execute(
            "INSERT INTO events(session_id, event, ts, payload)
             VALUES ('s', 'Stop', 2, '{\"assistant\": \"two\"}')",
            [],
        )
        .unwrap();
        let device = crate::db::device_id(&conn).unwrap();
        // Left open: closing the last connection would move the WAL into the store.
        std::mem::forget(conn);
        let wal = home.join("oboete.db-wal");
        assert!(std::fs::metadata(&wal).unwrap().len() > 0);
        (sha(&home.join("oboete.db")), sha(&wal), device)
    }

    /// Spec 7.4: doctor and a pass read v1's store without a write, the events only in its WAL
    /// imported, also on a copy, where v1's open would have given the store a new device id.
    #[test]
    fn the_old_store_is_left_byte_for_byte() {
        let home = tempfile::tempdir().unwrap();
        let before = v1_store(home.path());
        let copy = tempfile::tempdir().unwrap();
        for f in ["oboete.db", "oboete.db-wal"] {
            std::fs::copy(home.path().join(f), copy.path().join(f)).unwrap();
        }
        for h in [home.path(), copy.path()] {
            crate::setup::doctor(h).ok();
            let mut raw = raw::open(h).unwrap();
            pass(h, &mut raw, &h.join("oboete.db")).unwrap();
            let replies = of_kind(&raw, "reply");
            assert_eq!(
                replies,
                [json!({"assistant": "one"}), json!({"assistant": "two"})]
            );
            let conn = open_v1(&h.join("oboete.db")).unwrap();
            let events: i64 = conn
                .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
                .unwrap();
            assert_eq!(events, 2);
            let device = crate::db::device_id(&conn).unwrap();
            drop(conn);
            let after = (
                sha(&h.join("oboete.db")),
                sha(&h.join("oboete.db-wal")),
                device,
            );
            assert_eq!(after, before, "{}", h.display());
        }
    }
}
