//! The request judge (docs/my-work-model.md J2, J3): before grouping, each new conversation once,
//! as claude-mem 15.4's `routeUndecided` routes it. A single proven link to one open request joins
//! it with no call (claude-mem's rules judge); with no open request it is a request of its own;
//! else Jev picks among the repository's 19 most recently active open requests or none, and checks
//! the pick: from 0.8 it joins, from 0.5 it is its own request with a suggestion to join, below its
//! own request. A call that fails leaves it to grouping, and it is not asked again (J1).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::judge::{Ask, Asked};
use crate::my_work::Seen;
use crate::my_work_group::{Group, Groups, Key, Proof};
use crate::my_work_ops::{MyWorkOp, Read};
use crate::my_work_run::{Judging, Stores};
use crate::redact::Rules;
use crate::settings::{Refusal, refused};

/// The ops' job (S1).
pub(crate) const NAME: &str = "judge";
/// The entry of a decision made with no call; the judge's is its destination's name in the
/// ledger (`Judging::entry`).
const RULES: &str = "judge:rules";

// claude-mem's request judge (request-judge.mjs, 15.4.0, as 15.7.0 has it), word for word.
pub(crate) const CANDIDATES: usize = 19;
const JOIN: f64 = 0.8;
const SUGGEST: f64 = 0.5;
const ASKS: usize = 3;
const ASK_CHARS: usize = 250;
pub(crate) const TITLE_CHARS: usize = 120;
const PICK: &str = "`conversation` was started after each of `requests`. Which of `requests` does \
it continue: the same overall piece of work (same feature, page, skill, release or investigation), \
or another run of the same recurring routine? A person tracking their requests would want both on \
one card. Pick `none` if it is a separate ask of its own.";
const CHECK: &str = "`conversation` was started after `request`. It continues `request`: the same \
overall piece of work (same feature, page, skill, release or investigation), or another run of the \
same recurring routine. A person tracking their requests would want both on one card.";
const NONE: &str = "None of these: a separate ask of its own";
/// claude-mem's `ownTitle` when a request has no title and its ask makes none.
pub(crate) const UNTITLED: &str = "Untitled request";
/// What is gated of a text before it is cleaned and cut, in characters.
pub(crate) const WINDOW: usize = 2_000;

#[cfg(test)]
thread_local! {
    /// A smaller op cap a test sets, in `raw::MAX_OP_BYTES`'s place.
    pub(crate) static CAP: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// What the judge was shown of this device's records and ops (S2): the prompt records, and the
/// card and grouping ops whose text it read.
#[derive(Default, Clone)]
pub(crate) struct Shown {
    pub spans: BTreeSet<i64>,
    pub ops: BTreeSet<i64>,
}

/// The earliest candidate (C1) no judge op names yet, judged (J3): the op that says what was
/// decided, or that the conversation was left to grouping; none when no conversation is left to
/// judge, or when the judge did not ask (its repository excluded on the way, or its key gone: the
/// next round reads it again).
pub(crate) fn request(
    s: &Stores,
    seen: &[Seen],
    groups: &Groups,
    judge: &mut Judging,
    now: i64,
) -> Result<Vec<MyWorkOp>> {
    let (entry, who) = (judge.entry, judge.label);
    // Decision 81: its evidence in oboete's own words' language.
    let ja = crate::config::notes_japanese(&s.summary.language);
    let device = s.raw.device();
    let candidates = crate::my_work_group::candidates(seen, groups, device, now);
    let Some(c) = (candidates.values().flatten())
        .filter(|c| !groups.asked.contains(&c.key))
        .min_by_key(|c| c.ask.as_ref().map(|a| (a.2, a.1)))
        .copied()
    else {
        return Ok(Vec::new());
    };
    let (Some(repo), Some((_, opening, ..))) = (&c.repo, &c.ask) else {
        return Ok(Vec::new());
    };
    let by_key: HashMap<&Key, &Seen> = seen.iter().map(|c| (&c.key, c)).collect();
    let open: Vec<(&Key, &Group)> = (groups.groups.iter())
        .filter(|(_, g)| g.repo == *repo)
        .collect();
    let mut shown = Shown::default();
    shown.spans.insert(*opening);
    let decision = |shown: Shown, entry: &str, mut fields: Vec<(&str, Value)>| {
        fields.push(("conversation", key(&c.key)));
        op_of(s, (NAME, repo, entry), &shown, fields)
    };
    // Too big for an op's cap, a decision would be kept as a skip that names no conversation, and
    // the judge would ask for it again each round: it is left to grouping instead.
    let too_big = || {
        let shown = Shown {
            spans: BTreeSet::from([*opening]),
            ops: BTreeSet::new(),
        };
        decision(shown, entry, vec![("outcome", json!("too_big"))])
    };
    let decided =
        |shown: Shown, entry: &str, fields: Vec<(&str, Value)>| -> Result<Vec<MyWorkOp>> {
            let op = decision(shown, entry, fields)?;
            Ok(vec![if fits(&op)? { op } else { too_big()? }])
        };
    let own = || json!([{"conversations": [key(&c.key)], "title": null, "links": []}]);
    let joined = |anchor: &Key, kind: &str, evidence: String| {
        let ends = json!([key(anchor), key(&c.key)]);
        json!([{"conversations": ends, "title": null,
            "links": [{"conversations": ends, "kind": kind, "evidence": evidence}]}])
    };
    let group_ops = |anchor: &Key, shown: &mut Shown| {
        if let Some((_, g)) = open.iter().find(|(a, _)| *a == anchor) {
            let mine = g.ops.iter().filter(|(d, _)| d == device);
            shown.ops.extend(mine.map(|(_, seq)| *seq));
        }
    };

    // claude-mem's rules judge: a single open request a proven link names (C3) joins, no call.
    let (_, senders) = crate::my_work_group::agent_sent(s.raw, &[c])?;
    let (links, linked) = crate::my_work_group::proven(repo, &[c], &open, &by_key, &senders);
    if let [anchor] = linked.iter().collect::<Vec<_>>()[..] {
        let proof = (links.iter()).find_map(|(proof, ends)| ends.contains(anchor).then_some(proof));
        let sent = matches!(proof, Some(Proof::Sent(_)));
        if let Some(Proof::Sent(record)) = proof {
            shown.spans.insert(*record);
        }
        group_ops(anchor, &mut shown);
        let evidence = rules_evidence(ja, sent);
        return decided(
            shown,
            RULES,
            vec![
                ("outcome", json!("rules")),
                ("groups", joined(anchor, "rules", evidence)),
            ],
        );
    }
    // claude-mem's Jev judge with no open request to continue: a request of its own, no call.
    if open.is_empty() {
        return decided(
            shown,
            entry,
            vec![("outcome", json!("alone")), ("groups", own())],
        );
    }

    // The pick, over the 19 most recently active open requests (J3).
    let newest = |anchor: &Key, g: &Group| {
        (g.members.iter().chain([anchor]))
            .filter_map(|m| by_key.get(m).map(|c| c.last))
            .max()
            .unwrap_or(0)
    };
    let mut ranked: Vec<&(&Key, &Group)> = open.iter().collect();
    ranked.sort_by_key(|(a, g)| std::cmp::Reverse(newest(a, g)));
    ranked.truncate(CANDIDATES);
    let considered = considered(ja, ranked.len(), open.len());
    let conversation = context(s, c, &mut shown);
    let mut requests = serde_json::Map::new();
    let mut criteria = serde_json::Map::new();
    let mut labels: BTreeMap<String, (&Key, Value)> = BTreeMap::new();
    for (i, (anchor, g)) in ranked.iter().enumerate() {
        let label = format!("r{}", i + 1);
        let anchor_seen = by_key.get(*anchor).copied();
        let context = match anchor_seen {
            Some(a) => context(s, a, &mut shown),
            None => json!({"worktree": repo, "started": null, "asks": [], "agent_recorded": []}),
        };
        let asked = anchor_seen.and_then(|a| a.ask.as_ref());
        let title = (g.title.clone())
            .or_else(|| {
                asked.map(|a| crate::my_work::title(&crate::my_work::shown(&a.3, WINDOW, s.rules)))
            })
            .filter(|t| !t.trim().is_empty());
        if g.title.is_none()
            && let Some((d, seq, ..)) = asked
            && d == device
        {
            shown.spans.insert(*seq);
        }
        let title = title.map_or_else(
            || UNTITLED.to_owned(),
            |t| crate::my_work::shown(&t, TITLE_CHARS, s.rules),
        );
        group_ops(anchor, &mut shown);
        // claude-mem's request: its title, then its anchor's context.
        let mut request = json!({"title": title});
        if let (Some(r), Some(c)) = (request.as_object_mut(), context.as_object()) {
            r.extend(c.clone());
        }
        criteria.insert(label.clone(), json!(format!("`requests.{label}`: {title}")));
        requests.insert(label.clone(), request.clone());
        labels.insert(label, (*anchor, request));
    }
    criteria.insert("none".into(), json!(NONE));
    let evidence = |p: f64, score: f64| picked_evidence(ja, who, &considered, p, score);
    // Nothing is paid for that could not be kept: the largest decision this read can make.
    let largest = vec![
        ("outcome", json!("suggested")),
        ("groups", joined(ranked[0].0, "judge", evidence(1.0, 1.0))),
        ("suggestion", json!({"request": key(&c.key), "score": 1.0})),
    ];
    if !fits(&decision(shown.clone(), entry, largest)?)? {
        return Ok(vec![too_big()?]);
    }
    let span = format!("{NAME} {repo}");
    let repos = [repo.clone()];
    let picked = (judge.ask)(Ask {
        span: &span,
        state: json!({"conversation": conversation, "requests": requests}),
        questions: json!({"pick": {"type": "choice", "instructions": PICK, "criteria": criteria}}),
        repos: &repos,
    })?;
    let pick = match picked {
        Asked::Answered(answers) => answers["pick"].clone(),
        other => return left(other.unanswered(), (shown, entry), decided),
    };
    let choice = pick["choice"].as_str().unwrap_or("none");
    let p = pick["probabilities"][choice].as_f64().unwrap_or(0.0);
    let Some((anchor, request)) = labels.get(choice) else {
        return decided(
            shown,
            entry,
            vec![("outcome", json!("none")), ("groups", own())],
        );
    };

    // The check of the pick: its score is the band (claude-mem's `bandFor`).
    let checked = (judge.ask)(Ask {
        span: &span,
        state: json!({"request": request, "conversation": conversation}),
        questions: json!({"same": {"type": "noul", "instructions": CHECK,
            "criteria": {"true": "Same work", "false": "Different work"}}}),
        repos: &repos,
    })?;
    let score = match checked {
        Asked::Answered(answers) => answers["same"]["noul"].as_f64().unwrap_or(0.0),
        other => return left(other.unanswered(), (shown, entry), decided),
    };
    let fields = if score >= JOIN {
        vec![
            ("outcome", json!("joined")),
            ("groups", joined(anchor, "judge", evidence(p, score))),
        ]
    } else if score >= SUGGEST {
        vec![
            ("outcome", json!("suggested")),
            ("groups", own()),
            (
                "suggestion",
                json!({"request": key(anchor), "score": score}),
            ),
        ]
    } else {
        vec![("outcome", json!("own")), ("groups", own())]
    };
    decided(shown, entry, fields)
}

/// What a call the judge did not answer leaves (J1): a refused or failed one an op naming the
/// conversation and why, with no decision, so grouping decides it and the judge does not ask
/// again; none when nothing went because the judge was off, its key gone or the repository
/// excluded on the way, as the next round reads each.
fn left(
    why: &'static str,
    (shown, entry): (Shown, &str),
    decided: impl FnOnce(Shown, &str, Vec<(&str, Value)>) -> Result<Vec<MyWorkOp>>,
) -> Result<Vec<MyWorkOp>> {
    if matches!(why, "off" | "key" | "excluded") {
        return Ok(Vec::new());
    }
    decided(shown, entry, vec![("outcome", json!(why))])
}

/// The rules judge's evidence (decision 81: Japanese when oboete's own words are, else English):
/// the one proven link, a sent opening ask or a work list both wrote.
fn rules_evidence(ja: bool, sent: bool) -> String {
    match (ja, sent) {
        (true, true) => "呼び出しなしでまとめました: この依頼だけにつながる確かな手がかりが 1 つあります (この依頼の会話が、この会話の最初の依頼をツール呼び出しで送った)。".into(),
        (true, false) => "呼び出しなしでまとめました: この依頼だけにつながる確かな手がかりが 1 つあります (両方が同じ作業リストに書いた)。".into(),
        (false, true) => "Joined with no call: one proven link, to this request alone: a conversation of it sent this conversation's opening ask in a tool call.".into(),
        (false, false) => "Joined with no call: one proven link, to this request alone: a work list both wrote.".into(),
    }
}

/// The open requests the judge was shown, of all those open.
fn considered(ja: bool, shown: usize, open: usize) -> String {
    match (ja, shown == open) {
        (true, true) => format!("開いている依頼 {open} 件"),
        (true, false) => format!("開いている依頼 {open} 件のうち最近動いた {shown} 件"),
        (false, true) => format!("{open} open request{}", if open == 1 { "" } else { "s" }),
        (false, false) => format!("the {shown} most recently active of {open} open requests"),
    }
}

/// The judge's pick, with its probability and the check's score.
fn picked_evidence(ja: bool, who: &str, considered: &str, p: f64, score: f64) -> String {
    if ja {
        format!(
            "{who}: {considered}の中から選びました (確率 {p:.2})。「その依頼の続き」の確認は {score:.2} でした。"
        )
    } else {
        format!(
            "{who}: picked over {considered} (p {p:.2}); the check \"continues that request\" \
             scored {score:.2}."
        )
    }
}

/// A conversation as claude-mem's `contextReader` gives it (J2): its repository, the minute it
/// started, its first asks and its first cards' titles, each cut at the attached files, its white
/// space collapsed, gated and cut; what it reads of this device's records and ops in `shown`.
pub(crate) fn context(s: &Stores, c: &Seen, shown: &mut Shown) -> Value {
    let device = s.raw.device();
    let mut asks = Vec::new();
    for (d, seq, _, text) in c.asks.iter().take(ASKS) {
        let text = said(text, ASK_CHARS, s.rules);
        if text.is_empty() {
            continue;
        }
        if d == device {
            shown.spans.insert(*seq);
        }
        asks.push(text);
    }
    let mut recorded = Vec::new();
    for (d, op, title) in &c.cards {
        if d == device {
            shown.ops.insert(*op);
        }
        recorded.push(said(title, TITLE_CHARS, s.rules));
    }
    let started = (c.ask.as_ref()).map(|a| crate::db::utc(a.2)[..16].replacen(' ', "T", 1));
    json!({"worktree": c.repo, "started": started, "asks": asks, "agent_recorded": recorded})
}

/// A judge's op (S1, S2) of `job` about `about`, decided by `entry`: its fields, the last of a
/// name winning, what it was shown of this device's records and ops, and the records removed since
/// in the spans it read. Both judges build theirs with it (#780).
pub(crate) fn op_of(
    s: &Stores,
    (job, about, entry): (&str, &str, &str),
    shown: &Shown,
    fields: Vec<(&str, Value)>,
) -> Result<MyWorkOp> {
    let device = s.raw.device();
    let top = s.raw.max_seq_of(device)?;
    let mut removed = Vec::new();
    for &seq in &shown.spans {
        removed.extend(s.raw.removed_in(device, seq, seq, Some(top))?);
    }
    removed.sort();
    removed.dedup();
    Ok(MyWorkOp {
        job: job.into(),
        about: about.to_owned(),
        entry: entry.into(),
        ts: crate::db::now_ms(),
        read: Read {
            spans: shown.spans.iter().map(|&seq| (seq, seq)).collect(),
            ops: shown.ops.iter().copied().collect(),
        },
        removed,
        fields: (fields.into_iter())
            .map(|(name, v)| (name.to_owned(), v))
            .collect(),
        skipped: false,
        forgotten: None,
    })
}

/// Whether `op` stays whole within an op's cap, its removals aside (`raw::fitted` drops those
/// first), with room for a decision's few words more than the one measured.
pub(crate) fn fits(op: &MyWorkOp) -> Result<bool> {
    #[cfg(test)]
    let cap = CAP.get().unwrap_or(crate::raw::MAX_OP_BYTES);
    #[cfg(not(test))]
    let cap = crate::raw::MAX_OP_BYTES;
    let bare = MyWorkOp {
        removed: Vec::new(),
        ..op.clone()
    };
    Ok(serde_json::to_string(&bare)?.len() + 512 <= cap)
}

/// A text as Jev is shown it (J2): gated first, as it was recorded (A5), so that a rule over
/// several lines still matches; then claude-mem's `clean`, and cut.
pub(crate) fn said(text: &str, max: usize, rules: &Rules) -> String {
    let gated = crate::my_work::shown(text, WINDOW, rules);
    crate::my_work::shown(&clean(&gated), max, rules)
}

/// claude-mem's `clean`: the text before `# Attached files`, its white space collapsed.
fn clean(text: &str) -> String {
    let before = text.split("# Attached files").next().unwrap_or_default();
    before.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn key((agent, session): &Key) -> Value {
    json!([agent, session])
}

/// Whether the my_work consumer has applied every op this device recorded, within
/// `claims::applied`'s wait: `false` only when the wait ran out; a worker or a store that failed is
/// an error (#777).
fn caught_up(home: &Path) -> Result<bool> {
    let Some((raw, k)) = crate::search::b::stores(home)? else {
        return Ok(true);
    };
    let device = raw.device().to_owned();
    let top = raw.max_op_seq_of(&device)?;
    drop(raw);
    match crate::claims::applied(home, &k, &device, &[top], (NAME, "my_work")) {
        Ok(()) => Ok(true),
        Err(e) if e.is::<crate::claims::NotYet>() => Ok(false),
        Err(e) => Err(e),
    }
}

/// J4's answer as the page posts it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    /// The request My Work shows the suggestion on, by its shown id.
    request: String,
    action: String,
}

/// J4: the owner's Join or Not this on the request of a conversation the judge suggested, `POST
/// /api/my-work/suggestion`, refused before it writes; an op of the job that reads nothing, as an
/// owner's move in My Focus is. Join moves the conversation, and every conversation of its own
/// request, into the request its pick is in now; Not this keeps them its own request, so a forget
/// that takes the judge's op later leaves it so. Either closes the suggestion.
pub(crate) fn act(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Answer =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let join = match posted.action.as_str() {
        "join" => true,
        "not-this" => false,
        _ => return Err(refused(400, "bad_request", "action")),
    };
    if !(1..=80).contains(&posted.request.len()) {
        return Err(refused(400, "bad_request", "request"));
    }
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unavailable = |_| refused(503, "my_work_unavailable", "");
    // #775: what this device recorded before, an earlier answer among it, applied first, so that
    // this answer is resolved against it; still pending after the wait, it is refused.
    if !caught_up(home).map_err(unavailable)? {
        return Err(refused(409, "my_work_pending", ""));
    }
    let not_found = || refused(404, "suggestion_not_found", "request");
    let held = crate::my_work_focus::held_of(home, &posted.request)
        .map_err(unavailable)?
        .ok_or_else(not_found)?;
    let Some((raw, k)) = crate::search::b::stores(home).map_err(unavailable)? else {
        return Err(not_found());
    };
    let reading =
        crate::curate::Reading::now(&raw, crate::curate::Reads::Live).map_err(unavailable)?;
    let groups = crate::my_work_group::groups(&raw, &k, &reading).map_err(unavailable)?;
    drop((raw, k));
    let Some((c, picked)) =
        (groups.suggested.iter()).find(|(c, _)| crate::my_work::anchored(c) == held)
    else {
        return Err(not_found());
    };
    let Some(group) = groups.groups.get(c) else {
        return Err(not_found());
    };
    // Its own request's other conversations come with it (claude-mem's `brought`).
    let others = group.members.iter().filter(|m| *m != c).map(key);
    let decision = if join {
        let target = (groups.of.get(picked))
            .filter(|target| *target != c)
            .ok_or_else(|| refused(409, "suggestion_target_gone", "request"))?;
        let ends = json!([key(target), key(c)]);
        let members: Vec<Value> = [key(target), key(c)].into_iter().chain(others).collect();
        let ja = crate::config::load(home).map_or_else(
            |_| crate::config::notes_japanese(&crate::config::os_language()),
            |c| crate::config::notes_japanese(&c.summary.language),
        );
        let evidence = if ja {
            "判定役の提案を受けて、あなたがまとめました。"
        } else {
            "Joined by the owner on the judge's suggestion."
        };
        json!([{"conversations": members, "title": null, "links": [{"conversations": ends,
            "kind": "judge", "evidence": evidence}]}])
    } else {
        let members: Vec<Value> = std::iter::once(key(c)).chain(others).collect();
        json!([{"conversations": members, "title": null, "links": []}])
    };
    let fields = json!({"conversation": key(c), "action": posted.action, "groups": decision});
    let applied = crate::my_work_ops::record(home, NAME, vec![(group.repo.clone(), fields)])
        .map_err(unavailable)?;
    Ok(json!({"state": if applied { "applied" } else { "pending" }}))
}

#[cfg(test)]
mod tests;
