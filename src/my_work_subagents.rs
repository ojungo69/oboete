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

/// The subagents of `convs` (Claude Code conversations), the `MAX_LISTED` with the newest last steps,
/// each conversation's newest step first. The caller holds the read fence: a forget registers before
/// this read or after it.
pub(crate) fn read(
    raw: &Raw,
    k: &Connection,
    convs: &[Key],
    rules: &Rules,
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
    // A subagent is listed from its first step: a stop or a launch with no step makes none. The
    // newest last steps first, at most `MAX_LISTED`.
    let mut stepped: Vec<((Key, String), Seen, i64)> = (seen.into_iter())
        .filter_map(|(id, s)| {
            s.last_step
                .as_ref()
                .map(|(at, _)| *at)
                .map(|at| (id, s, at))
        })
        .collect();
    stepped.sort_by_key(|(.., at)| std::cmp::Reverse(*at));
    stepped.truncate(MAX_LISTED);
    let imported = imported_stops(raw, &wanted)?;
    let mut out = Vec::new();
    for ((key, agent_id), s, last_step) in stepped {
        let finished = (s.stops.iter())
            .chain(
                imported
                    .get(&(key.clone(), agent_id.clone()))
                    .into_iter()
                    .flatten(),
            )
            .copied()
            .filter(|&at| at >= last_step)
            .max();
        let task = match &s.launch {
            Some((device, seq, _)) => task_of(raw, device, *seq, rules)?,
            None => None,
        };
        let Some((_, step_record)) = s.last_step else {
            continue;
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

/// Mark finished: the owner's stop of `s`, sent through the hook's own path as if Claude Code had
/// sent it from `folder`, the conversation's recorded folder, at `now`, so the checkout's capture
/// setting, the exclusions and the gate read it as they read the hook's. `raw` is the handle the
/// caller read `s` with, opened before its readers' hold. True when it is recorded,
/// or kept to be written back as a hook's stop would be.
pub(crate) fn mark(
    home: &Path,
    raw: &mut Raw,
    s: &Subagent,
    folder: &str,
    now: i64,
) -> Result<bool> {
    let settings = crate::capture::Settings::load(home)?;
    let mut payload = json!({"session_id": s.conversation.1, "cwd": folder,
        "hook_event_name": "SubagentStop", "agent_id": s.agent_id});
    if let Some(kind) = &s.agent_type {
        payload["agent_type"] = json!(kind);
    }
    payload[crate::capture::OWNER_MARK] = json!(true);
    // The stop is the listed conversation's and subagent's, or nothing is recorded: a rule of now
    // that masks the session or the id would make it another's (Codex on slice 2).
    let keeps = |e: &crate::raw::Event| {
        e.kind == SUBAGENT_STOP
            && e.agent == s.conversation.0
            && e.session == s.conversation.1
            && serde_json::from_str::<Value>(&e.body)
                .is_ok_and(|b| b["agent_id"] == s.agent_id.as_str())
    };
    let (mut failed, mut written_back) = (0, 0);
    let recorded = crate::hook::record_if(
        home,
        raw,
        &s.conversation.0,
        "SubagentStop",
        &payload,
        (now, &settings),
        (&mut failed, &mut written_back),
        &keeps,
    )?;
    // A stop kept behind older kept files is recorded when they are written back, as the hook's.
    Ok(recorded.kept || recorded.events.iter().any(|(_, e)| e.kind == SUBAGENT_STOP))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer::subagents::Subagents;
    use crate::raw::{self, Event, OpKind, Target, test_event};
    use crate::worker;

    const MIN: i64 = 60_000;

    fn ev(session: &str, kind: &str, ts: i64, body: Value) -> Event {
        Event {
            session: session.into(),
            kind: kind.into(),
            ts,
            ..test_event(&body.to_string())
        }
    }

    fn step(session: &str, id: &str, ts: i64) -> Event {
        ev(session, "tool", ts, json!({"tool": "Read", "agent_id": id}))
    }

    fn stop(session: &str, id: &str, ts: i64) -> Event {
        ev(session, SUBAGENT_STOP, ts, json!({"agent_id": id}))
    }

    /// A launch record as capture keeps it: what it launched, and its input with the task given
    /// here as `task`.
    fn launch(session: &str, ts: i64, mut launched: Value) -> Event {
        let task = launched.as_object_mut().unwrap().remove("task");
        let input = json!({ "description": task }).to_string();
        ev(
            session,
            "tool",
            ts,
            json!({"tool": "Agent", "input": input, "launched": launched}),
        )
    }

    fn drain(home: &Path, store: &Raw) -> Connection {
        let mut consumers: Vec<Box<dyn worker::Consumer>> = vec![Box::new(Subagents)];
        let mut k = crate::knowledge::open(home).unwrap();
        worker::drain(store, &mut k, &mut consumers).unwrap();
        k
    }

    fn key(session: &str) -> Key {
        ("claude".to_owned(), session.to_owned())
    }

    fn states(found: &[Subagent], now: i64) -> Vec<(&str, &'static str, Option<i64>)> {
        (found.iter())
            .map(|s| (s.agent_id.as_str(), s.state(now), s.finished))
            .collect()
    }

    /// Slice 2 (docs/subagents.md): Finished with the time when a stop came at or after the last
    /// step (in the same millisecond too), a foreground launch that returned among the stops; Working again after a later step,
    /// within 15 minutes of it, then No recent step. A launch or a stop with no step makes no
    /// subagent; another conversation's and a Codex one's are not read. The type is the newest
    /// step's or stop's, none when no record names one; the task is the launch's.
    #[test]
    fn a_subagents_state_follows_its_steps_stops_and_launch() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let typed = ev(
            "s",
            "tool",
            MIN,
            json!({"tool": "Read", "agent_id": "a1", "agent_type": "Explore"}),
        );
        for e in [
            typed,
            stop("s", "a1", 2 * MIN),
            stop("s", "b2", MIN),
            step("s", "b2", 3 * MIN),
            step("s", "c3", 4 * MIN),
            launch(
                "s",
                5 * MIN,
                json!({"agent_id": "c3", "status": "completed", "async": false,
                    "task": "Find the parser"}),
            ),
            step("s", "d4", 4 * MIN),
            launch(
                "s",
                5 * MIN,
                json!({"agent_id": "d4", "status": "async_launched", "async": true}),
            ),
            launch(
                "s",
                5 * MIN,
                json!({"agent_id": "e5", "status": "completed", "async": false}),
            ),
            stop("s", "f6", 5 * MIN),
            step("s", "i9", 6 * MIN),
            stop("s", "i9", 6 * MIN),
            step("t", "g7", 5 * MIN),
            Event {
                agent: "codex".into(),
                ..step("s", "h8", 5 * MIN)
            },
        ] {
            store.append(&e).unwrap();
        }
        let k = drain(home.path(), &store);
        let codex = ("codex".to_owned(), "s".to_owned());
        let found = read(&store, &k, &[key("s"), codex], &Rules::default()).unwrap();
        assert_eq!(
            states(&found, 5 * MIN),
            [
                ("i9", "finished", Some(6 * MIN)),
                ("c3", "finished", Some(5 * MIN)),
                ("d4", "working", None),
                ("b2", "working", None),
                ("a1", "finished", Some(2 * MIN)),
            ]
        );
        let of = |id: &str| found.iter().find(|s| s.agent_id == id).unwrap();
        assert_eq!(of("a1").agent_type.as_deref(), Some("Explore"));
        assert_eq!(of("b2").agent_type, None);
        assert_eq!(of("c3").task.as_deref(), Some("Find the parser"));
        assert_eq!(of("d4").task, None);
        let b2 = of("b2");
        assert_eq!(b2.state(3 * MIN + RECENT_MS), "working");
        assert_eq!(b2.state(3 * MIN + RECENT_MS + 1), "quiet");
        assert!(
            read(&store, &k, &[key("u")], &Rules::default())
                .unwrap()
                .is_empty()
        );
    }

    /// Slice 2: at most `MAX_LISTED` subagents, those with the newest last steps.
    #[test]
    fn the_subagents_with_the_newest_steps_are_listed() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        for n in 0..=MAX_LISTED as i64 {
            let session = if n % 2 == 0 { "s" } else { "t" };
            store
                .append(&step(session, &format!("a{n}"), n * MIN))
                .unwrap();
        }
        let k = drain(home.path(), &store);
        let found = read(&store, &k, &[key("s"), key("t")], &Rules::default()).unwrap();
        assert_eq!(found.len(), MAX_LISTED);
        assert!(found.iter().all(|s| s.agent_id != "a0"), "{found:?}");
        assert_eq!(
            (found[0].conversation.1.as_str(), found[0].agent_id.as_str()),
            ("s", "a300")
        );
    }

    /// Slice 2, with the consumer held behind: a stop appended since its pass counts at once, a
    /// forget or a mask registered since hides what it took, a record expired since (its rebuild
    /// not run) is passed over, and a stop both among the rows and past the checkpoint counts once.
    /// With no checkpoint, there is nothing to show.
    #[test]
    fn the_records_past_the_checkpoint_count_at_once() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let empty = crate::knowledge::open(home.path()).unwrap();
        store.append(&step("s", "a1", MIN)).unwrap();
        assert!(
            read(&store, &empty, &[key("s")], &Rules::default())
                .unwrap()
                .is_empty()
        );
        drop(empty);
        let b = store.append(&step("s", "b2", MIN)).unwrap();
        let c = store.append(&step("s", "c3", MIN)).unwrap();
        let d = store.append(&step("s", "d4", MIN)).unwrap();
        let task = launch(
            "s",
            MIN,
            json!({"agent_id": "a1", "status": "async_launched", "async": true,
                "task": "Read the tests"}),
        );
        let l = store.append(&task).unwrap();
        let k = drain(home.path(), &store);
        let ids = |found: &[Subagent]| -> Vec<String> {
            found.iter().map(|s| s.agent_id.clone()).collect()
        };
        assert_eq!(
            ids(&read(&store, &k, &[key("s")], &Rules::default()).unwrap()),
            ["a1", "b2", "c3", "d4"]
        );
        let device = store.device().to_owned();
        let stopped = store.append(&stop("s", "a1", 2 * MIN)).unwrap();
        (store.append_tombstone(Target::Record {
            device: device.clone(),
            seq: b,
        }))
        .unwrap();
        let Item::Event(e) = store.after(&device, c - 1, 1).unwrap()[0].item.clone() else {
            panic!("an event")
        };
        (store.append_tombstone(Target::Range {
            device: device.clone(),
            seq: c,
            offset: e.body.find("c3").unwrap() as i64,
            length: 2,
        }))
        .unwrap();
        // The launch's task goes with its record.
        (store.append_tombstone(Target::Record {
            device: device.clone(),
            seq: l,
        }))
        .unwrap();
        store.expire_for_test(&[d]);
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        assert_eq!(states(&found, 2 * MIN), [("a1", "finished", Some(2 * MIN))]);
        assert_eq!(found[0].task, None);
        drop(k);
        // The consumer's rows hold the stop now; its checkpoint set back before it, the stop is
        // read from raw too.
        let k = drain(home.path(), &store);
        k.execute(
            "UPDATE checkpoints SET seq = ?1 WHERE consumer = ?2",
            params![stopped - 1, subagents::NAME],
        )
        .unwrap();
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        assert_eq!(states(&found, 2 * MIN), [("a1", "finished", Some(2 * MIN))]);
        k.execute(
            "DELETE FROM checkpoints WHERE consumer = ?1",
            [subagents::NAME],
        )
        .unwrap();
        assert!(
            read(&store, &k, &[key("s")], &Rules::default())
                .unwrap()
                .is_empty()
        );
    }

    /// Slice 2 (Codex on slice 2): a launch's task is read from its record's input as capture kept it
    /// whole (a closed block and a run of base64 taken out there), through the rules of now before
    /// its cut: a value a rule added since masks is masked where the cut splits it.
    #[test]
    fn a_launch_task_is_read_whole_through_the_rules_of_now_before_its_cut() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let settings = crate::capture::Settings::load(home.path()).unwrap();
        let captured = |id: &str, description: String| {
            let payload = json!({"session_id": "s", "tool_name": "Agent",
                "tool_input": {"description": description, "prompt": "p"},
                "tool_response": {"agentId": id, "status": "async_launched"}});
            (crate::capture::events("claude", "PostToolUse", &payload, MIN, &settings))
                .remove(0)
                .event
        };
        let secret = format!("{} teal-{}{}", "d".repeat(150), "1234", "5678");
        let encoded = format!("{} {}", "e".repeat(10), "aA0z".repeat(400));
        let private = format!("{} <private>zqx hidden</private> after", "f".repeat(150));
        for (id, description) in [("a1", secret), ("b2", encoded), ("c3", private)] {
            store.append(&captured(id, description)).unwrap();
            store.append(&step("s", id, 2 * MIN)).unwrap();
        }
        let k = drain(home.path(), &store);
        std::fs::write(
            home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = 'teal', regex = 'teal-[0-9]{8}' }]\n",
        )
        .unwrap();
        let rules = Rules::load(home.path()).unwrap();
        let found = read(&store, &k, &[key("s")], &rules).unwrap();
        let task = |id: &str| -> String {
            let s = found.iter().find(|s| s.agent_id == id).unwrap();
            s.task.clone().unwrap_or_else(|| panic!("no task of {id}"))
        };
        assert!(!task("a1").contains("1234"), "{}", task("a1"));
        assert!(task("a1").starts_with(&"d".repeat(150)), "{}", task("a1"));
        assert!(!task("b2").contains("aA0zaA0z"), "{}", task("b2"));
        assert!(!task("c3").contains("zqx"), "{}", task("c3"));
        assert_eq!(task("c3").chars().count(), 156, "{}", task("c3"));
    }

    /// Slice 2 (Codex on slice 2): a launch's task falls back as claude-mem reads one, to the input's
    /// prompt (cut at 500) and to the response's description when the input does not parse; a task
    /// a build before this one kept in the launch is never read, so with neither text parsing there
    /// is none.
    #[test]
    fn a_launch_task_falls_back_as_claude_mem_reads_one() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        let record = |id: &str, input: String, output: String| {
            ev(
                "s",
                "tool",
                MIN,
                json!({"tool": "Agent", "input": input, "output": output, "launched":
                    {"agent_id": id, "status": "async_launched", "async": true,
                     "task": "Kept task"}}),
            )
        };
        let prompt = json!({ "prompt": "p ".repeat(400) }).to_string();
        let described = json!({"description": "From the response"}).to_string();
        for (id, input, output) in [
            ("a1", prompt, String::new()),
            ("b2", "{cut".to_owned(), described),
            ("c3", "{cut".to_owned(), "{cut".to_owned()),
        ] {
            store.append(&record(id, input, output)).unwrap();
            store.append(&step("s", id, 2 * MIN)).unwrap();
        }
        let k = drain(home.path(), &store);
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        let task = |id: &str| (found.iter().find(|s| s.agent_id == id).unwrap().task).clone();
        assert_eq!(task("a1").map(|t| t.chars().count()), Some(500));
        assert_eq!(task("b2").as_deref(), Some("From the response"));
        assert_eq!(task("c3"), None);
    }

    /// Slice 2 (cubic on #850): Mark finished's folder is the one the subagent's newest step was
    /// recorded in, whichever device recorded it: a copied home's first device's too. One a
    /// tombstone took, or one no directory now, gives none.
    #[test]
    fn the_folder_is_the_newest_steps_of_any_device() {
        let from = tempfile::tempdir().unwrap();
        let place = tempfile::tempdir().unwrap();
        let folder = |name: &str| {
            let p = place.path().join(name);
            std::fs::create_dir_all(&p).unwrap();
            p.to_string_lossy().into_owned()
        };
        let (old, new) = (folder("old"), folder("new"));
        let mut store = raw::open(from.path()).unwrap();
        let at = |cwd: &str, ts: i64| Event {
            cwd: Some(cwd.to_owned()),
            ..step("s", "a1", ts)
        };
        store.append(&at(&old, MIN)).unwrap();
        store.append(&at(&new, 2 * MIN)).unwrap();
        let first = store.device().to_owned();
        drop(drain(from.path(), &store));
        drop(store);
        let to = tempfile::tempdir().unwrap();
        for f in ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"] {
            if from.path().join(f).exists() {
                std::fs::copy(from.path().join(f), to.path().join(f)).unwrap();
            }
        }
        let mut store = raw::open(to.path()).unwrap();
        let k = crate::knowledge::open(to.path()).unwrap();
        let rules = Rules::default();
        let found = read(&store, &k, &[key("s")], &rules).unwrap();
        assert_eq!(
            recorded_folder(&store, &found[0]).unwrap(),
            Some(new.clone())
        );
        std::fs::remove_dir(&new).unwrap();
        assert_eq!(recorded_folder(&store, &found[0]).unwrap(), None);
        std::fs::create_dir(&new).unwrap();
        (store.append_tombstone(Target::Record {
            device: first,
            seq: found[0].step_record.1,
        }))
        .unwrap();
        assert_eq!(recorded_folder(&store, &found[0]).unwrap(), None);
    }

    /// Slice 2 (Codex on slice 2): in a home copied from another device's, whose own device the
    /// consumer has no checkpoint of yet, a forget of the first device's record and a stop recorded
    /// there count at once.
    #[test]
    fn a_copied_homes_own_records_count_before_its_first_checkpoint() {
        let from = tempfile::tempdir().unwrap();
        let mut store = raw::open(from.path()).unwrap();
        let a = store.append(&step("s", "a1", MIN)).unwrap();
        store.append(&step("s", "b2", MIN)).unwrap();
        let first = store.device().to_owned();
        drop(drain(from.path(), &store));
        drop(store);
        let to = tempfile::tempdir().unwrap();
        for f in ["raw.db", "raw.db-wal", "knowledge.db", "knowledge.db-wal"] {
            if from.path().join(f).exists() {
                std::fs::copy(from.path().join(f), to.path().join(f)).unwrap();
            }
        }
        let mut store = raw::open(to.path()).unwrap();
        assert_ne!(store.device(), first);
        let k = crate::knowledge::open(to.path()).unwrap();
        (store.append_tombstone(Target::Record {
            device: first,
            seq: a,
        }))
        .unwrap();
        store.append(&stop("s", "b2", 2 * MIN)).unwrap();
        let found = read(&store, &k, &[key("s")], &Rules::default()).unwrap();
        assert_eq!(states(&found, 2 * MIN), [("b2", "finished", Some(2 * MIN))]);
    }

    /// Slice 2: a claude-mem `agent-lifecycle` row `oboete import claude-mem` brought in is its
    /// subagent's stop at its `stopped_at_epoch` (the platform `claude` when none); one before the
    /// last step does not finish it, and an agent's own write to a list of that name is no stop.
    #[test]
    fn an_imported_agent_lifecycle_row_is_its_subagents_stop() {
        let home = tempfile::tempdir().unwrap();
        let mut store = raw::open(home.path()).unwrap();
        for id in ["a1", "b2", "c3"] {
            store.append(&step("s", id, 2 * MIN)).unwrap();
        }
        let row = |n: &str, fields: Value| {
            json!({"repo": "claude-mem:p", "list": crate::work_state::AGENT_LIFECYCLE,
                "fields": fields, "clock": 1, "at": 1, "source": "claude-mem:p",
                "source_id": n})
        };
        let stopped = |id: &str, at: i64| {
            json!({"task": format!("Agent {id} in claude session s"), "status": "done",
                "content_session_id": "s", "agent_id": id, "stopped_at_epoch": at,
                "stop_source": "subagent-stop-hook"})
        };
        let mut b2 = stopped("b2", MIN);
        b2["platform_source"] = json!("claude");
        // Another conversation's row of an id alike is no stop of this one's.
        let mut other = stopped("c3", 4 * MIN);
        other["content_session_id"] = json!("u");
        store
            .append_ops(&[
                (OpKind::WorkState, row("w1", stopped("a1", 3 * MIN))),
                (OpKind::WorkState, row("w2", b2)),
                (OpKind::WorkState, row("w3", other)),
            ])
            .unwrap();
        let fields = stopped("c3", 3 * MIN);
        store
            .work_state(
                "github.com/o/r",
                crate::work_state::AGENT_LIFECYCLE,
                fields.as_object().unwrap(),
                None,
            )
            .unwrap();
        let k = drain(home.path(), &store);
        assert_eq!(
            states(
                &read(&store, &k, &[key("s")], &Rules::default()).unwrap(),
                3 * MIN
            ),
            [
                ("a1", "finished", Some(3 * MIN)),
                ("b2", "working", None),
                ("c3", "working", None),
            ]
        );
    }
}
