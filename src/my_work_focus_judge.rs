//! The focus judge (docs/my-work-model.md J5): before placement (F3), when a kept area exists,
//! Jev picks for each queued request the kept area it is part of, or none, as claude-mem 15.4's
//! `askFocusArea` asks it. The pick is a hint the placement model is shown, and it places nothing:
//! claude-mem's `FOCUS_AUTO_PLACE_THRESHOLD` is null. A request is asked once for its basis and the
//! kept areas as they are: the pick, or the outcome of a call refused or failed, is kept as an op of
//! its own job about the placement's subject, so that a request that moved, or kept areas that
//! changed, are asked again, and nothing else is (docs/my-work-model.md, Slice 11c).

use std::collections::HashSet;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::judge::{Ask, Asked};
use crate::my_work::{AskFrom, ReviewInput, Seen};
use crate::my_work_focus::{Area, Focus, GOAL_CHARS, NAME_CHARS, OTHER};
use crate::my_work_group::Key;
use crate::my_work_judge::{CANDIDATES, Shown, TITLE_CHARS, UNTITLED, WINDOW};
use crate::my_work_ops::{Kept, MyWorkOp};
use crate::my_work_run::{Judging, Stores};

/// The ops' job (S1).
pub(crate) const NAME: &str = "focus-judge";

// claude-mem's focus judge (focus-judge.mjs, 15.4.0, as 15.7.0 asks it), word for word.
const AREA: &str = "`request` is one piece of the owner's work. Which of `areas` is it part of: the \
area the owner could ask for it inside of (read each area's goal and about)? Working in the same \
repo is not enough. Pick `none` if it fits none of these areas.";
const NONE: &str = "None of these focus areas";

/// A request placement has queued (F3), as it reads it.
pub(crate) struct Queued<'a> {
    pub subject: &'a str,
    pub basis: i64,
    pub input: &'a ReviewInput,
}

/// The areas the judge chooses from: those the owner kept (claude-mem's `status === 'kept'`).
pub(crate) fn kept(focus: &Focus) -> Vec<&Area> {
    (focus.listed().into_iter())
        .filter(|a| a.status() == "kept" && a.placeable())
        .collect()
}

/// The kept areas as the question shows them, as a number: their ids, names, goals and what they
/// are about, so that an area renamed or given another goal is asked about again.
pub(crate) fn digest(kept: &[&Area]) -> i64 {
    let mut lines: Vec<String> = (kept.iter())
        .map(|a| {
            let text = |key| a.text(key).unwrap_or_default();
            format!(
                "{}\0{}\0{}\0{}\n",
                a.id,
                a.name(),
                text("goal"),
                text("about")
            )
        })
        .collect();
    lines.sort();
    i64::from_str_radix(&crate::curate::sha256_hex(&lines.concat())[..13], 16).unwrap_or(0)
}

/// The op the judge keeps for `subject` now (Slice 11c's "Current"): this device's newest about it, a pick or an
/// outcome, when it is of `basis` and `areas` and nothing took it; none otherwise.
pub(crate) fn current(
    s: &Stores,
    hidden: &HashSet<(String, i64)>,
    subject: &str,
    (basis, areas): (i64, i64),
) -> Result<Option<Kept>> {
    let device = Some(s.raw.device());
    let last = crate::my_work_ops::newest_shown(s.k, (NAME, subject), device, hidden, false)?;
    Ok(last.filter(|k| {
        let number = |key: &str| k.op.fields.get(key).and_then(Value::as_i64);
        k.op.forgotten.is_none()
            && !k.op.skipped
            && number("basis") == Some(basis)
            && number("areas") == Some(areas)
    }))
}

/// A pick as the placement model is shown it (Slice 11c's "The hint"): the area's id, or `other` when none fits, and
/// its probability; none for an outcome with no pick.
pub(crate) fn hint(kept: &Kept) -> Option<String> {
    let focus = kept.op.fields.get("focus").and_then(Value::as_str)?;
    let p = kept.op.fields.get("p").and_then(Value::as_f64)?;
    Some(format!("{focus} (p {p:.2})"))
}

/// J5: the first of `queued` with no current op, asked: the op that keeps its pick, or why there
/// is none; none when each has its op, when no area is kept, or when nothing went (the judge
/// turned off or its key gone on the way, or a repository of the request excluded: the next round
/// reads each again, as J3's `left`).
pub(crate) fn next(
    s: &Stores,
    (focus, hidden): (&Focus, &HashSet<(String, i64)>),
    queued: &[Queued],
    judge: &mut Judging,
) -> Result<Vec<MyWorkOp>> {
    let kept = kept(focus);
    if kept.is_empty() {
        return Ok(Vec::new());
    }
    let areas = digest(&kept);
    let Some(q) = due(s, hidden, queued, areas)? else {
        return Ok(Vec::new());
    };
    let keeping = Keeping {
        q,
        areas,
        entry: judge.entry,
        who: judge.label,
        kept: &kept,
    };
    let (request, repos, mut shown) = asked_about(s, q)?;
    let state = shown_areas(s, focus, &kept, &mut shown);
    // Nothing is paid for that could not be kept: an op as large as a pick, else one that says so,
    // read of nothing, which keeps the request from being asked again for its basis.
    let largest = vec![
        ("focus", json!(OTHER)),
        ("p", json!(1.0)),
        (
            "evidence",
            json!(evidence((keeping.who, None), 1.0, kept.len(), (0, 0))),
        ),
    ];
    if !crate::my_work_judge::fits(&keeping.op(s, &shown, largest)?)? {
        return Ok(vec![keeping.outcome(s, &Shown::default(), "too_big")?]);
    }
    let span = format!("{NAME} {}", q.subject);
    let question = Question {
        span: &span,
        request: &request,
        repos: &repos,
        areas: &state,
    };
    let mut tally = (0, 0);
    let mut ask = |set: &[&Area]| question.ask(s, judge, set);
    let answer = pick_among(&kept, &mut tally, &mut ask)?;
    keeping.decided(s, &shown, answer, tally)
}

/// The ops about one queued request: of its basis and of the kept areas as `digest` numbers them.
struct Keeping<'a> {
    q: &'a Queued<'a>,
    areas: i64,
    /// The judge's name in the ledger and its name in evidence (`Judging`).
    entry: &'static str,
    who: &'static str,
    /// The areas it chose among.
    kept: &'a [&'a Area],
}

impl Keeping<'_> {
    fn op(&self, s: &Stores, shown: &Shown, mut fields: Vec<(&str, Value)>) -> Result<MyWorkOp> {
        fields.extend([("basis", json!(self.q.basis)), ("areas", json!(self.areas))]);
        let about = (NAME, self.q.subject, self.entry);
        crate::my_work_judge::op_of(s, about, shown, fields)
    }

    /// An op with no pick, and why.
    fn outcome(&self, s: &Stores, shown: &Shown, why: &str) -> Result<MyWorkOp> {
        self.op(
            s,
            shown,
            vec![("focus", Value::Null), ("outcome", json!(why))],
        )
    }

    /// The op of what the judge answered: its pick, or why there is none; none when nothing went
    /// (the judge turned off or its key gone on the way, or a repository excluded: J3's `left`).
    fn decided(
        &self,
        s: &Stores,
        shown: &Shown,
        answer: Answer,
        tally: (usize, usize),
    ) -> Result<Vec<MyWorkOp>> {
        let (choice, p) = match answer {
            Answer::Picked(choice, p) => (choice, p),
            Answer::Skipped("off" | "key" | "excluded") => return Ok(Vec::new()),
            Answer::Skipped(why) => return Ok(vec![self.outcome(s, shown, why)?]),
        };
        let picked = self.picked(s, &choice, p, tally);
        Ok(vec![self.op(s, shown, picked)?])
    }

    /// A pick's fields: the area's id, or `other` when none fits, its probability and its evidence.
    fn picked(
        &self,
        s: &Stores,
        choice: &str,
        p: f64,
        tally: (usize, usize),
    ) -> Vec<(&'static str, Value)> {
        let area = self.kept.iter().find(|a| a.id == choice);
        let name = area.map(|a| crate::my_work::shown(a.name(), NAME_CHARS, s.rules));
        let evidence = evidence((self.who, name.as_deref()), p, self.kept.len(), tally);
        vec![
            ("focus", json!(area.map_or(OTHER, |a| a.id.as_str()))),
            ("p", json!(p)),
            ("evidence", json!(evidence)),
        ]
    }
}

/// The first of `queued` with no current op for `areas`.
fn due<'a>(
    s: &Stores,
    hidden: &HashSet<(String, i64)>,
    queued: &'a [Queued<'a>],
    areas: i64,
) -> Result<Option<&'a Queued<'a>>> {
    for q in queued {
        if current(s, hidden, q.subject, (q.basis, areas))?.is_none() {
            return Ok(Some(q));
        }
    }
    Ok(None)
}

/// The request `q` as the judge is shown it, the repositories of its conversations, and what that
/// read of this device's records and ops.
fn asked_about(s: &Stores, q: &Queued) -> Result<(Value, Vec<String>, Shown)> {
    let seen = crate::my_work::seen(s.raw, s.k, s.rules)?;
    let of: Vec<&Seen> = (seen.iter())
        .filter(|c| q.input.conversations.contains(&c.key))
        .collect();
    let mut repos: Vec<String> = of.iter().filter_map(|c| c.repo.clone()).collect();
    repos.sort();
    repos.dedup();
    let mut shown = Shown::default();
    let request = request(s, q, &of, &mut shown);
    Ok((request, repos, shown))
}

/// What each kept area shows, gated and cut, and the ops it was read from (S2), as placement's
/// call lists them.
fn shown_areas(s: &Stores, focus: &Focus, kept: &[&Area], shown: &mut Shown) -> Map<String, Value> {
    let device = s.raw.device();
    let mut state = Map::new();
    for a in kept {
        let gated = |key: &str, cap| {
            let text = crate::my_work::shown(a.text(key).unwrap_or_default(), cap, s.rules);
            (!text.is_empty()).then_some(text)
        };
        state.insert(
            a.id.clone(),
            json!({"name": gated("name", NAME_CHARS), "goal": gated("goal", GOAL_CHARS),
                "about": gated("about", GOAL_CHARS)}),
        );
        let own = focus.sources(&a.id).iter().filter(|(d, _)| d == device);
        shown.ops.extend(own.map(|(_, seq)| *seq));
    }
    state
}

/// One request's question to the judge, asked of one set of kept areas at a time.
struct Question<'a> {
    span: &'a str,
    request: &'a Value,
    repos: &'a [String],
    /// Each kept area as `shown_areas` shows it, by its id.
    areas: &'a Map<String, Value>,
}

impl Question<'_> {
    fn ask(&self, s: &Stores, judge: &mut Judging, set: &[&Area]) -> Result<Answer> {
        let mut criteria = Map::new();
        let mut areas = Map::new();
        for a in set {
            let name = crate::my_work::shown(a.name(), NAME_CHARS, s.rules);
            criteria.insert(a.id.clone(), json!(format!("`areas.{}`: {name}", a.id)));
            areas.insert(a.id.clone(), self.areas[&a.id].clone());
        }
        criteria.insert("none".into(), json!(NONE));
        let asked = (judge.ask)(Ask {
            span: self.span,
            state: json!({"request": self.request, "areas": areas}),
            questions: json!({"area": {"type": "choice", "instructions": AREA, "criteria": criteria}}),
            repos: self.repos,
        })?;
        let answers = match asked {
            Asked::Answered(answers) => answers,
            other => return Ok(Answer::Skipped(other.unanswered())),
        };
        let choice = answers["area"]["choice"]
            .as_str()
            .unwrap_or("none")
            .to_owned();
        Ok(
            match answers["area"]["probabilities"][choice.as_str()].as_f64() {
                Some(p) if p.is_finite() => Answer::Picked(choice, p),
                _ => Answer::Skipped("shape"),
            },
        )
    }
}

/// What one question answered: a kept area's id or `none` with its probability, or why there is
/// no answer (`judge::Asked::Skipped`).
enum Answer {
    Picked(String, f64),
    Skipped(&'static str),
}

/// claude-mem's `pickAmong`: one question over up to 19 areas; more are asked in sets, then a
/// question over the sets' winners, in sets again should they not fit one; with no winner, none
/// at the lowest probability a set gave it. `tally` counts the sets and the final questions.
fn pick_among(
    areas: &[&Area],
    tally: &mut (usize, usize),
    ask: &mut impl FnMut(&[&Area]) -> Result<Answer>,
) -> Result<Answer> {
    if areas.len() <= CANDIDATES {
        return ask(areas);
    }
    let mut picks = Vec::new();
    for set in areas.chunks(CANDIDATES) {
        tally.0 += 1;
        match ask(set)? {
            Answer::Picked(choice, p) => picks.push((choice, p)),
            skipped => return Ok(skipped),
        }
    }
    let winners: Vec<&Area> = (picks.iter())
        .filter_map(|(choice, _)| areas.iter().find(|a| a.id == *choice).copied())
        .collect();
    match winners[..] {
        [] => {
            let p = picks.iter().map(|(_, p)| *p).fold(f64::INFINITY, f64::min);
            Ok(Answer::Picked("none".into(), p))
        }
        [one] => Ok(picks
            .into_iter()
            .find(|(c, _)| *c == one.id)
            .map_or(Answer::Skipped("shape"), |(choice, p)| {
                Answer::Picked(choice, p)
            })),
        _ => {
            tally.1 += 1;
            pick_among(&winners, tally, ask)
        }
    }
}

/// claude-mem's evidence of a pick: the area's name, or none fits, over how many kept areas and how
/// they were asked, with the probability.
fn evidence(
    (who, name): (&str, Option<&str>),
    p: f64,
    kept: usize,
    (sets, finals): (usize, usize),
) -> String {
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let mut asked = plural(kept, "kept focus area");
    if sets > 0 {
        asked.push_str(&format!(
            ", asked in {} of up to {CANDIDATES}",
            plural(sets, "set")
        ));
        if finals > 0 {
            asked.push_str(&format!(
                " and {} over the set winners",
                plural(finals, "final question")
            ));
        }
    }
    match name {
        Some(name) => format!("{who}: \"{name}\" over {asked} (p {p:.2})."),
        None => format!("{who}: none of {asked} fits (p {p:.2})."),
    }
}

/// The request as Jev is shown it (claude-mem's `requestContexts`): its title, then its anchor
/// conversation's context (J2), the conversation its placement is known by, or for a request its
/// agent named, the one whose opening ask came first.
fn request(s: &Stores, q: &Queued, of: &[&Seen], shown: &mut Shown) -> Value {
    let input = q.input;
    let anchor: Option<&Seen> = match q.subject.strip_prefix("c:").and_then(|k| k.split_once(':')) {
        Some((agent, session)) => {
            let key: Key = (agent.to_owned(), session.to_owned());
            of.iter().find(|c| c.key == key).copied()
        }
        None => (of.iter())
            .filter(|c| c.ask.is_some())
            .min_by_key(|c| c.ask.as_ref().map(|a| (a.2, a.1)))
            .copied(),
    };
    let title = match &input.title {
        Some((title, ops)) => {
            shown.ops.extend(ops.iter().copied());
            Some(title.clone())
        }
        None => input.original.as_ref().map(|(from, text)| {
            match from {
                AskFrom::Prompt(seq) => {
                    shown.spans.insert(*seq);
                }
                AskFrom::Summary(op) => {
                    shown.ops.insert(*op);
                }
                AskFrom::Work => {}
            }
            crate::my_work::title(&crate::my_work::shown(text, WINDOW, s.rules))
        }),
    };
    let title = (title.filter(|t| !t.trim().is_empty())).map_or_else(
        || UNTITLED.to_owned(),
        |t| crate::my_work::shown(&t, TITLE_CHARS, s.rules),
    );
    let context = match anchor {
        Some(c) => crate::my_work_judge::context(s, c, shown),
        None => json!({"worktree": input.repos.first(), "started": null, "asks": [],
            "agent_recorded": []}),
    };
    let mut request = json!({"title": title});
    if let (Some(r), Some(c)) = (request.as_object_mut(), context.as_object()) {
        r.extend(c.clone());
    }
    request
}
