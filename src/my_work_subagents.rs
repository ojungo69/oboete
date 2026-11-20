//! Slice 2 of docs/subagents.md: the Claude Code subagents of the conversations My Work lists, with
//! their state (Working, No recent step, Finished) from the facts `consumer::subagents` keeps and the
//! records past its checkpoint, and the owner's Mark finished.

use crate::consumer::subagents::{self, Fact};
use crate::raw::{Item, Raw, SUBAGENT_STOP};
use crate::redact::Rules;
use crate::worker::Consumer as _;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// A conversation, as agent and session.
pub(crate) type Key = (String, String);

/// claude-mem's `recentMs`: a subagent whose last step is this recent is Working.
pub(crate) const RECENT_MS: i64 = 15 * 60 * 1000;

/// The most subagents an answer lists, those with the newest last steps. claude-mem's agent view
/// is made from its newest 300 observations and its recent work state writes, so a subagent with
/// nothing recent leaves it; this PC's store held 1,522 with a step (2026-11-07), which would have
/// more than doubled each minute's read of My Work.
pub(crate) const MAX_LISTED: usize = 300;

/// What a subagent's rows say once read: its last step, its newest stop at or after it, its type,
/// and the launch record it was started by.
#[derive(Debug, Default)]
struct Seen {
    /// Its newest step's time, and that step's record (device, seq).
    last_step: Option<(i64, (String, i64))>,
    stops: Vec<i64>,
    /// Its newest type, with that row's time.
    kind: Option<(i64, String)>,
    /// The newest launch record (device, seq) and its time.
    launch: Option<(String, i64, i64)>,
}

/// One subagent as My Work shows it, before the gate.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Subagent {
    pub conversation: Key,
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub last_step: i64,
    /// Its newest step's record (device, seq), which Mark finished reads the folder of.
    pub step_record: (String, i64),
    /// When it finished, when a stop came at or after its last step.
    pub finished: Option<i64>,
    pub task: Option<String>,
}

impl Subagent {
    /// Working, No recent step or Finished, at `now`.
    pub(crate) fn state(&self, now: i64) -> &'static str {
        match self.finished {
            Some(_) => "finished",
            None if now - self.last_step <= RECENT_MS => "working",
            None => "quiet",
        }
    }
}

/// Which of the subagents `read_picked` keeps.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Pick {
    /// The newest `MAX_LISTED` by last step: oboete's page's.
    Newest,
    /// Every one, with no task read: Mark finished's lookup (W4).
    All,
    /// Decision 118: those not finished, or finished within `RECENT_MS` of the time given, then
    /// the newest `MAX_LISTED` of them: the work-view's.
    Window(i64),
}

/// The subagents of `convs` (Claude Code conversations) that `pick` keeps, each conversation's
/// newest step first. The caller holds the read fence: a forget registers before this read or
/// after it.
pub(crate) fn read_picked(
    raw: &Raw,
    k: &Connection,
    convs: &[Key],
    rules: &Rules,
    pick: Pick,
) -> Result<Vec<Subagent>> {
    let convs: Vec<&Key> = convs.iter().filter(|(a, _)| a == "claude").collect();
    if convs.is_empty() || !crate::consumer::manifest::exists(k, "table", "subagent_facts")? {
        return Ok(Vec::new());
    }
    // The checkpoints first, then the rows, then the records past the checkpoints: a row the
    // consumer commits meanwhile is in the rows or past the checkpoint, and counts once by its key.
    let checkpoints: HashMap<String, i64> = k
        .prepare("SELECT device, seq FROM checkpoints WHERE consumer = ?1")?
        .query_map([subagents::NAME], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if checkpoints.is_empty() {
        return Ok(Vec::new());
    }
    type Row = (String, i64, Fact, Key, i64);
    let mut rows: BTreeMap<(String, i64, &'static str, String), Row> = BTreeMap::new();
    let mut st = k.prepare_cached(
        "SELECT device, seq, fact, agent_id, ts, agent_type, returned FROM subagent_facts
         WHERE agent = ?1 AND session = ?2",
    )?;
    for key in &convs {
        let found = st.query_map(params![key.0, key.1], |r| {
            let fact: String = r.get(2)?;
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                fact,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, bool>(6)?,
            ))
        })?;
        for row in found {
            let (device, seq, fact, agent_id, ts, agent_type, returned) = row?;
            let fact = match fact.as_str() {
                "step" => "step",
                "stop" => "stop",
                "launch" => "launch",
                _ => continue,
            };
            let f = Fact {
                fact,
                agent_id: agent_id.clone(),
                agent_type,
                returned,
            };
            rows.insert(
                (device.clone(), seq, fact, agent_id),
                (device, seq, f, (*key).clone(), ts),
            );
        }
    }
    // Past each device's checkpoint: the stops appended since count at once (the owner's Mark
    // finished among them), and a tombstone's target since then is hidden, as the manifest's
    // reader hides it. Steps and launches past it wait for the consumer. Every device it reads,
    // one it has no checkpoint of yet from its first record: a copied home's own, where a forget
    // of another device's record is written (Codex on slice 2).
    let mut hidden: HashSet<(String, i64)> = HashSet::new();
    let wanted: HashSet<&Key> = convs.iter().copied().collect();
    for device in subagents::Subagents.devices(raw)? {
        let at = checkpoints.get(&device).copied().unwrap_or(0);
        hidden.extend(raw.tombstones_after(&device, at)?);
        let seqs = raw.stops_after(&device, at)?;
        for r in raw.at(&device, &seqs)? {
            let Item::Event(e) = r.item else { continue };
            let key = (e.agent.clone(), e.session.clone());
            if e.kind != SUBAGENT_STOP || !wanted.contains(&key) {
                continue;
            }
            for f in subagents::facts(&e) {
                rows.entry((device.clone(), r.seq, f.fact, f.agent_id.clone()))
                    .or_insert((device.clone(), r.seq, f, key.clone(), e.ts));
            }
        }
    }
    let rows: Vec<Row> = (rows.into_values())
        .filter(|(device, seq, ..)| !hidden.contains(&(device.clone(), *seq)))
        .collect();
    // A row whose record expired since (retention stopped before its rebuild) is passed over.
    let mut by_device: HashMap<&str, Vec<i64>> = HashMap::new();
    for (device, seq, ..) in &rows {
        by_device.entry(device).or_default().push(*seq);
    }
    let mut expired: HashSet<(String, i64)> = HashSet::new();
    for (device, seqs) in by_device {
        for seq in raw.expired_among(device, &seqs)? {
            expired.insert((device.to_owned(), seq));
        }
    }
    let mut seen: BTreeMap<(Key, String), Seen> = BTreeMap::new();
    for (device, seq, f, key, ts) in rows {
        if expired.contains(&(device.clone(), seq)) {
            continue;
        }
        let s = seen.entry((key, f.agent_id.clone())).or_default();
        if let Some(kind) = f.agent_type.filter(|_| f.fact != "launch")
            && s.kind.as_ref().is_none_or(|(at, _)| *at <= ts)
        {
            s.kind = Some((ts, kind));
        }
        match f.fact {
            "step" => {
                if s.last_step.as_ref().is_none_or(|(at, _)| *at <= ts) {
                    s.last_step = Some((ts, (device, seq)));
                }
            }
            "stop" => s.stops.push(ts),
            _ => {
                if f.returned {
                    s.stops.push(ts);
                }
                if s.launch.as_ref().is_none_or(|(.., at)| *at <= ts) {
                    s.launch = Some((device, seq, ts));
                }
            }
        }
    }
    // A subagent is listed from its first step: a stop or a launch with no step makes none. Each
    // one's finish first, which `Pick::Window` keeps by; then the newest last steps first, at most
    // `MAX_LISTED` but for `Pick::All`, and, for oboete's page, the task of those kept (the
    // work-view's pages show none: Codex on #1121).
    type Stepped = ((Key, String), Seen, (i64, (String, i64)), Option<i64>);
    let imported = imported_stops(raw, &wanted)?;
    let mut stepped: Vec<Stepped> = (seen.into_iter())
        .filter_map(|(id, mut s)| {
            let last = s.last_step.take()?;
            let finished = (s.stops.iter())
                .chain(imported.get(&id).into_iter().flatten())
                .copied()
                .filter(|&at| at >= last.0)
                .max();
            Some((id, s, last, finished))
        })
        .collect();
    if let Pick::Window(now) = pick {
        // A stop timed after `now` (another clock's) is not within the window (Codex on #1121).
        stepped
            .retain(|(.., finished)| finished.is_none_or(|at| at <= now && now - at <= RECENT_MS));
    }
    stepped.sort_by_key(|(_, _, (at, _), _)| std::cmp::Reverse(*at));
    if !matches!(pick, Pick::All) {
        stepped.truncate(MAX_LISTED);
    }
    let mut out = Vec::new();
    for ((key, agent_id), s, (last_step, step_record), finished) in stepped {
        let task = match (&s.launch, pick) {
            (Some((device, seq, _)), Pick::Newest) => task_of(raw, device, *seq, rules)?,
            _ => None,
        };
        out.push(Subagent {
            conversation: key,
            agent_id,
            agent_type: s.kind.map(|(_, kind)| kind),
            last_step,
            step_record,
            finished,
            task,
        });
    }
    out.sort_by(|a, b| (a.conversation.cmp(&b.conversation)).then(b.last_step.cmp(&a.last_step)));
    Ok(out)
}

/// claude-mem's `agent-lifecycle` rows that `oboete import claude-mem` brought in as work state ops
/// of its source: the stops of the conversations `wanted` lists, at their `stopped_at_epoch` (the
/// platform `claude` when none, as claude-mem reads one).
fn imported_stops(raw: &Raw, wanted: &HashSet<&Key>) -> Result<HashMap<(Key, String), Vec<i64>>> {
    let mut out: HashMap<(Key, String), Vec<i64>> = HashMap::new();
    let sessions: Vec<&str> = wanted.iter().map(|(_, session)| session.as_str()).collect();
    for fields in raw.imported_list_fields(crate::work_state::AGENT_LIFECYCLE, &sessions)? {
        let platform = fields["platform_source"].as_str().unwrap_or("claude");
        let (Some(session), Some(agent_id), Some(at)) = (
            fields["content_session_id"].as_str(),
            fields["agent_id"].as_str(),
            fields["stopped_at_epoch"].as_i64(),
        ) else {
            continue;
        };
        let conversation = (platform.to_owned(), session.to_owned());
        if wanted.contains(&conversation) {
            out.entry((conversation, agent_id.to_owned()))
                .or_default()
                .push(at);
        }
    }
    Ok(out)
}

/// The task a launch record gave its subagent, as claude-mem's agent view reads a launch: its input's
/// description, else the response's, else the input's prompt, else the response's, as capture kept
/// them (cleaned whole), through the gate as its rules are now and then folded and cut to 160 and
/// 500 characters, as My Work's other texts are (Codex on slice 2). None when the record is gone,
/// or its input and response no longer parse (a text cut when it was kept).
fn task_of(raw: &Raw, device: &str, seq: i64, rules: &Rules) -> Result<Option<String>> {
    let Some(Item::Event(e)) = (raw.at(device, &[seq])?.into_iter())
        .find(|r| r.seq == seq)
        .map(|r| r.item)
    else {
        return Ok(None);
    };
    let body: Value = serde_json::from_str(&e.body).unwrap_or_default();
    let parsed = |key: &str| -> Value {
        (body[key].as_str())
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default()
    };
    let (input, response) = (parsed("input"), parsed("output"));
    let said = |v: &Value, key: &str, cap: usize| {
        let whole = crate::redact::outbound_with(v[key].as_str()?, rules);
        let folded = whole.split_whitespace().collect::<Vec<_>>().join(" ");
        (!folded.is_empty()).then(|| folded.chars().take(cap).collect::<String>())
    };
    Ok(said(&input, "description", 160)
        .or_else(|| said(&response, "description", 160))
        .or_else(|| said(&input, "prompt", 500))
        .or_else(|| said(&response, "prompt", 500)))
}

/// The answer's `subagents`: each with its conversation's id as the request's `sessions` give it,
/// an id of its own keyed as the page's other ids (never its agent id), and its texts through the
/// gate.
pub(crate) fn answer(
    found: &[Subagent],
    conversations: &HashMap<Key, (String, Option<String>)>,
    id_of: &mut dyn FnMut(&Subagent) -> Result<String>,
    rules: &Rules,
    now: i64,
) -> Result<Vec<Value>> {
    let gate = |s: &str| crate::redact::outbound_with(s, rules);
    let mut out = Vec::new();
    for s in found {
        let Some((conversation, fingerprint)) = conversations.get(&s.conversation) else {
            continue;
        };
        let state = s.state(now);
        out.push(json!({
            // As the request's `sessions` give it: `view::answered` takes it out of the gate's
            // walk and puts it back as it does theirs.
            "conversation": {"id": conversation, "fingerprint": fingerprint},
            "id": id_of(s)?,
            "type": s.agent_type.as_deref().map(gate),
            "state": state,
            "finished_at": s.finished,
            "last_step": s.last_step,
            // Through the gate when it was read, before its cut.
            "task": s.task,
            "can_mark": state != "finished",
        }));
    }
    Ok(out)
}

/// The held string a subagent's id is the keyed print of.
pub(crate) fn held(s: &Subagent) -> String {
    format!(
        "subagent\u{0}{}\u{0}{}\u{0}{}",
        s.conversation.0, s.conversation.1, s.agent_id
    )
}

/// The folder the subagent's newest step was recorded in, whichever device recorded it (a copied
/// home's first device too; cubic on #850), when that label is whole and a directory now: the
/// place Mark finished sends its stop from. None when the record is gone, the label missing or
/// masked, or no directory there.
pub(crate) fn recorded_folder(raw: &Raw, s: &Subagent) -> Result<Option<String>> {
    let (device, seq) = &s.step_record;
    let cwd = (raw.at(device, &[*seq])?.into_iter())
        .find(|r| r.seq == *seq)
        .and_then(|r| match r.item {
            Item::Event(e) | Item::Expired(e) => e.cwd,
            _ => None,
        });
    Ok(cwd.filter(|cwd| {
        !cwd.is_empty() && !cwd.contains(crate::redact::MASK) && Path::new(cwd).is_dir()
    }))
}

/// What `mark` did with the owner's stop (W4, #1102 item 1).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Marked {
    /// Appended to raw.db.
    Appended,
    /// Kept, to be written back by the next hook call whose store opens: behind older kept files,
    /// or because its append failed and its keep did not.
    Kept,
    /// Neither: the checkout's capture setting, an exclusion, a forget or a rule of now takes it.
    Refused,
}

/// Mark finished: the owner's stop of `s`, sent through the hook's own path as if Claude Code had
/// sent it from `folder`, the conversation's recorded folder, at `now`, so the checkout's capture
/// setting, the exclusions and the gate read it as they read the hook's. `raw` is the handle the
/// caller read `s` with, opened before its readers' hold. An error only when the stop is neither
/// appended nor kept.
pub(crate) fn mark(
    home: &Path,
    raw: &mut Raw,
    s: &Subagent,
    folder: &str,
    now: i64,
) -> Result<Marked> {
    let settings = crate::capture::Settings::load(home)?;
    let mut payload = json!({"session_id": s.conversation.1, "cwd": folder,
        "hook_event_name": "SubagentStop", "agent_id": s.agent_id});
    if let Some(kind) = &s.agent_type {
        payload["agent_type"] = json!(kind);
    }
    payload[crate::capture::OWNER_MARK] = json!(true);
    // The stop is the listed conversation's and subagent's, or nothing is recorded: a rule of now
    // that masks the session or the id would make it another's (Codex on slice 2).
    let keeps = stop_of(s);
    let (mut failed, mut written_back, mut kept) = (0, 0, false);
    let recorded = match crate::hook::record_if(
        home,
        raw,
        &s.conversation.0,
        "SubagentStop",
        &payload,
        (now, &settings),
        (&mut failed, &mut written_back, &mut kept),
        &keeps,
    ) {
        Ok(recorded) => recorded,
        // The hook's error path kept it: written back by the next hook call, as a hook's stop is.
        Err(e) if kept => {
            eprintln!("oboete: Mark finished's stop kept to be written back: {e:#}");
            return Ok(Marked::Kept);
        }
        Err(e) => return Err(e),
    };
    Ok(if recorded.kept {
        Marked::Kept
    } else if recorded.events.iter().any(|(_, e)| e.kind == SUBAGENT_STOP) {
        Marked::Appended
    } else {
        Marked::Refused
    })
}

/// A stop of `s`: its conversation's and its id's, whatever its time and whoever sent it.
fn stop_of(s: &Subagent) -> impl Fn(&crate::raw::Event) -> bool + '_ {
    move |e| {
        e.kind == SUBAGENT_STOP
            && e.agent == s.conversation.0
            && e.session == s.conversation.1
            && serde_json::from_str::<Value>(&e.body)
                .is_ok_and(|b| b["agent_id"] == s.agent_id.as_str())
    }
}

/// What Mark finished did (W4), which both routes answer from.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Finish {
    /// `finished_lock` or write-back's lock not had in time.
    Busy,
    /// No subagent has the id, or there is no store.
    NotFound,
    /// Two subagents have it (#1102 item 2): nothing is recorded.
    Shared,
    /// It has a stop at or after its last step.
    Finished,
    /// Its newest step's folder is gone or masked, or that checkout records nothing now.
    NotRecorded,
    /// Its last step is timed after this machine's clock: a stop now would finish nothing.
    Ahead,
    /// A stop of it waits in a kept file (#1102 item 1): nothing is recorded again.
    Pending,
    /// Its stop is kept, to be written back by the next hook call.
    Kept,
    /// Its stop is appended.
    Appended,
    /// `authorized` said no once config.lock was had: nothing is read or recorded.
    Unauthorized,
}

/// W4 (docs/subagents.md): Mark finished, for both routes. Under `finished_lock`, then config.lock,
/// which a new token takes, with `authorized` asked as soon as it is had, then raw.db and the
/// readers' hold, then write-back's lock, so that no kept file is written back meanwhile: the only
/// subagent `id` names (`my_work::subagent`, read after the locks, with its steps the consumer has
/// not read yet), refused when it finished; a stop of it that waits in a kept file is `Pending` and
/// none more is recorded (#1102 item 1); its folder is the one its newest step was recorded in; a
/// last step timed after now is `Ahead`. The stop is sent at the time read then. The locks let go,
/// an appended or kept stop starts the worker, as a hook's write does.
pub(crate) fn finish(home: &Path, id: &str, authorized: &dyn Fn() -> bool) -> Result<Finish> {
    let done = (|| -> Result<Finish> {
        let Some(_finishing) = crate::my_work::finished_lock(home, crate::db::OPEN_WRITE_WAIT)?
        else {
            return Ok(Finish::Busy);
        };
        // config.lock, which a new token takes, from before the stores (as a restore takes it) to
        // the record, and the caller's token asked as soon as it is had: no new token is made
        // while it is held, so nothing below is read or written for a token replaced meanwhile
        // (Codex on #1121).
        let wait = crate::db::OPEN_WRITE_WAIT;
        let Some(_config) = crate::settings::config_lock_within(home, wait)? else {
            return Ok(Finish::Busy);
        };
        if !authorized() {
            return Ok(Finish::Unauthorized);
        }
        // raw.db first, then the readers' hold, the order a restore takes them in (Codex on slice
        // 2); the same handle reads and appends.
        let Some((mut raw, k)) = crate::search::b::stores(home)? else {
            return Ok(Finish::NotFound);
        };
        let _read = crate::dispatch::reading(home, crate::db::OPEN_WRITE_WAIT)?;
        let Some(_held) = crate::unwritten::hold(home, crate::db::OPEN_WRITE_WAIT)? else {
            return Ok(Finish::Busy);
        };
        let (s, folder) = match crate::my_work::subagent(home, (&raw, &k), id)? {
            crate::my_work::Lookup::None => return Ok(Finish::NotFound),
            crate::my_work::Lookup::Shared => return Ok(Finish::Shared),
            crate::my_work::Lookup::One(s, folder) => (s, folder),
        };
        #[cfg(test)]
        if let Some(hook) = LOOKED_UP.with(|hook| hook.borrow_mut().take()) {
            hook(&mut raw);
        }
        if s.finished.is_some() {
            return Ok(Finish::Finished);
        }
        if crate::unwritten::holds(home, &raw, &stop_of(&s))? {
            return Ok(Finish::Pending);
        }
        let Some(folder) = folder else {
            return Ok(Finish::NotRecorded);
        };
        // A stop before the subagent's last step finishes nothing: a step timed after this
        // machine's clock (a copied home's, or one before the clock was set back) is waited out,
        // nothing recorded (Codex on #1121).
        let now = crate::db::now_ms();
        if s.last_step > now {
            return Ok(Finish::Ahead);
        }
        #[cfg(test)]
        if let Some(hook) = MARKING.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
        Ok(match mark(home, &mut raw, &s, &folder, now)? {
            Marked::Appended => Finish::Appended,
            Marked::Kept => Finish::Kept,
            Marked::Refused => Finish::NotRecorded,
        })
    })()?;
    if matches!(done, Finish::Appended | Finish::Kept) {
        // As a hook starts one after it writes (Codex and cubic on #850), reaped when it leaves,
        // or it sits as a zombie for as long as the viewer runs (cubic on #850).
        match crate::hook::start_worker(home) {
            Ok(Some(mut child)) => {
                std::thread::spawn(move || child.wait());
            }
            Ok(None) => {}
            Err(e) => eprintln!("oboete: worker not started: {e:#}"),
        }
    }
    Ok(done)
}

/// W4 (Codex on #1121): `s`'s steps recorded past the consumer's checkpoints, which `read_picked`
/// leaves to the consumer, read for Mark finished: its newest step governs where its stop is
/// recorded from and whether a stop now would finish it. A record a tombstone hides since, or whose
/// text expired, is passed over, as `read_picked` passes it over.
pub(crate) fn fresh_steps(raw: &Raw, k: &Connection, s: &mut Subagent) -> Result<()> {
    let checkpoints: HashMap<String, i64> = k
        .prepare("SELECT device, seq FROM checkpoints WHERE consumer = ?1")?
        .query_map([subagents::NAME], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let devices = subagents::Subagents.devices(raw)?;
    // Every device's tombstones since its checkpoint, as `read_picked` hides them: another
    // device's can take this one's record (Codex on #1121).
    let mut hidden: HashSet<(String, i64)> = HashSet::new();
    for device in &devices {
        let at = checkpoints.get(device).copied().unwrap_or(0);
        hidden.extend(raw.tombstones_after(device, at)?);
    }
    let mut returned = Vec::new();
    for device in &devices {
        let at = checkpoints.get(device).copied().unwrap_or(0);
        let seqs = raw.tools_after(device, at, &s.conversation.1)?;
        // In pages, so that a long backlog is not held at once (Codex on #1121).
        for page in seqs.chunks(200) {
            let expired: HashSet<i64> = raw.expired_among(device, page)?.into_iter().collect();
            for r in raw.at(device, page)? {
                let Item::Event(e) = r.item else { continue };
                if e.agent != s.conversation.0
                    || hidden.contains(&(device.clone(), r.seq))
                    || expired.contains(&r.seq)
                {
                    continue;
                }
                for f in subagents::facts(&e)
                    .into_iter()
                    .filter(|f| f.agent_id == s.agent_id)
                {
                    match f.fact {
                        "step" if e.ts >= s.last_step => {
                            s.last_step = e.ts;
                            s.step_record = (device.clone(), r.seq);
                        }
                        // A launch that returned with it finished it, as `read_picked` counts one.
                        "launch" if f.returned => returned.push(e.ts),
                        _ => {}
                    }
                }
            }
        }
    }
    let last = s.last_step;
    s.finished = (s.finished.into_iter().chain(returned))
        .filter(|&at| at >= last)
        .max();
    Ok(())
}

/// W4's tests: run once in `finish` after its lookup, with its raw.db handle.
#[cfg(test)]
type LookedUp = Option<Box<dyn FnOnce(&mut Raw)>>;

#[cfg(test)]
thread_local! {
    /// W4's tests: run once in `finish` just before the stop is sent.
    pub(crate) static MARKING: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    pub(crate) static LOOKED_UP: std::cell::RefCell<LookedUp> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests;
