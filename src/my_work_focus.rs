//! My Focus (docs/my-work-model.md F, decision 61): the focus areas and where each request sits,
//! as claude-mem 15.4 keeps them, from the ops of jobs `focus` and `place` (F8): an area op
//! (subject its id), a placement op (subject the request's anchor conversation) and the Other
//! pass's op, each a patch. The owner's actions (F6) are `focus` ops with `decided_by: owner`; the
//! placement job's (`my_work_place`) are `place` ops.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::my_work_goals::{Item, Quote};
use crate::settings::{Refusal, refused};

/// The job of the focus ops (S1).
pub(crate) const NAME: &str = "focus";
/// The bucket of the requests that fit no area: an id of placements only (F1).
pub(crate) const OTHER: &str = "other";
/// F1's caps, in characters.
pub(crate) const NAME_CHARS: usize = 80;
pub(crate) const GOAL_CHARS: usize = 300;
/// claude-mem's page words (`nav-words.mjs:8`), as whole words of a name.
const PAGE_WORDS: &[&str] = &[
    "my focus",
    "my work",
    "my rules",
    "other",
    "suggested",
    "past focus",
    "monitor",
    "steer",
];
/// The page's Japanese tab and section names, which a name may not be.
const PAGE_NAMES: &[&str] = &["マイワーク", "目標", "規則", "マイフォーカス", "その他"];
const STATUSES: &[&str] = &["suggested", "kept", "dismissed", "done"];
const SOURCES: &[&str] = &["owner-goals", "clustering", "promotion", "owner"];
const DECIDERS: &[&str] = &["owner", "system", "owner-restore"];

/// One focus area (F1), as the fold leaves it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Area {
    pub id: String,
    pub fields: Map<String, Value>,
    /// Whether the owner decided it last: a system row is then skipped (claude-mem's rule).
    owner_decided: bool,
}

impl Area {
    pub(crate) fn text(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }
    pub fn status(&self) -> &str {
        self.text("status").unwrap_or_default()
    }
    pub fn name(&self) -> &str {
        self.text("name").unwrap_or_default()
    }
    fn merged_into(&self) -> Option<&str> {
        self.text("merged_into")
    }
    fn rank(&self, key: &str) -> f64 {
        self.fields
            .get(key)
            .and_then(Value::as_f64)
            .unwrap_or(f64::INFINITY)
    }
    /// Takes requests (F2): kept or suggested, not merged.
    pub(crate) fn placeable(&self) -> bool {
        matches!(self.status(), "kept" | "suggested") && self.merged_into().is_none()
    }
    /// Whole and readable: a row that leaves it otherwise is not applied (claude-mem throws).
    fn valid(&self) -> bool {
        !self.name().is_empty()
            && STATUSES.contains(&self.status())
            && SOURCES.contains(&self.text("source").unwrap_or_default())
    }
}

/// What the focus ops say (F2, F8): the areas by id, and for each anchor the owner's and the
/// router's newest placement.
#[derive(Debug, Default)]
pub(crate) struct Focus {
    pub areas: BTreeMap<String, Area>,
    owner: HashMap<String, String>,
    router: HashMap<String, String>,
    /// Each area's ops, by device and op seq: what a call that shows the area read (S2).
    sources: HashMap<String, Vec<(String, i64)>>,
}

impl Focus {
    /// The focus and place ops in knowledge.db, oldest first (time, device, op seq), passing over
    /// those left out while an exclusion stands (S6) and the forgotten ones (S5).
    pub(crate) fn read(
        raw: &crate::raw::Raw,
        k: &Connection,
        reading: &crate::curate::Reading,
    ) -> Result<Self> {
        let mut out = Self::default();
        if !crate::consumer::manifest::exists(k, "table", "my_work")? {
            return Ok(out);
        }
        let hidden = crate::my_work_ops::hidden(raw, k, reading)?;
        let mut st = k.prepare(
            "SELECT device, op_seq, job, about, fields FROM my_work
             WHERE job IN (?1, ?2, ?3) AND skipped = 0 AND forgotten IS NULL
             ORDER BY ts, device, op_seq",
        )?;
        let jobs = [NAME, crate::my_work_place::NAME, crate::my_work_goals::NAME];
        let rows = st.query_map(jobs, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (device, op_seq, job, about, fields) = row?;
            let op = (device, op_seq);
            if hidden.contains(&op) {
                continue;
            }
            let fields: Map<String, Value> = serde_json::from_str(&fields)?;
            if job == crate::my_work_goals::NAME {
                out.mined(op, &fields);
                continue;
            }
            if fields.get("kind").and_then(Value::as_str) == Some("area") {
                let ops = out.sources.entry(about.clone()).or_default();
                ops.push(op);
            }
            out.apply(&about, fields);
        }
        Ok(out)
    }

    /// F5, claude-mem's `ownerGoalsAreaRows`: each priority of a goals op `op` is a system row of the
    /// area its `area` names: a new suggested area (source owner-goals, its title as the name, its
    /// product as the goal, its rank), or, for a suggested area, its rank and an empty goal filled;
    /// nothing else of an area is changed, and `apply` skips the row of an area the owner decided.
    fn mined(&mut self, op: (String, i64), fields: &Map<String, Value>) {
        let items = fields.get("items").and_then(Value::as_array);
        for item in items.into_iter().flatten() {
            let (Some(area), Some("priority")) = (item["area"].as_str(), item["kind"].as_str())
            else {
                continue;
            };
            let (title, product) = (item["title"].as_str(), item["product"].as_str());
            let product = product.filter(|p| !p.trim().is_empty());
            let mut patch = json!({"kind": "area", "priority_rank": item["rank"],
                "decided_by": "system"});
            // `apply` skips a system row of an area the owner decided: it is none of its sources.
            if self.areas.get(area).is_some_and(|a| a.owner_decided) {
                continue;
            }
            match self.areas.get(area) {
                None => {
                    let Some(name) = title.filter(|t| page_word(t).is_none()) else {
                        continue;
                    };
                    patch["name"] = json!(name);
                    patch["goal"] = json!(product);
                    patch["status"] = json!("suggested");
                    patch["source"] = json!("owner-goals");
                }
                Some(a) if a.status() == "suggested" => {
                    if a.text("goal").is_none_or(|g| g.trim().is_empty()) && product.is_some() {
                        patch["goal"] = json!(product);
                    }
                }
                Some(_) => continue,
            }
            self.sources
                .entry(area.to_owned())
                .or_default()
                .push(op.clone());
            let patch = patch.as_object().cloned().unwrap_or_default();
            self.apply(area, patch);
        }
    }

    /// One op's patch: a null field removes it, any other value sets it.
    fn apply(&mut self, about: &str, mut patch: Map<String, Value>) {
        match patch.remove("kind").as_ref().and_then(Value::as_str) {
            Some("area") => {
                let by = patch
                    .get("decided_by")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !DECIDERS.contains(&by) || about == OTHER || about.is_empty() {
                    return;
                }
                let restore = by == "owner-restore";
                let owner = by == "owner";
                let area = self.areas.get(about);
                if by == "system" && area.is_some_and(|a| a.owner_decided) {
                    return;
                }
                let mut next = area.cloned().unwrap_or(Area {
                    id: about.to_owned(),
                    fields: Map::new(),
                    owner_decided: false,
                });
                for (key, value) in patch {
                    if value.is_null() {
                        next.fields.remove(&key);
                    } else {
                        next.fields.insert(key, value);
                    }
                }
                next.owner_decided = owner || (next.owner_decided && !restore);
                if next.valid() {
                    self.areas.insert(about.to_owned(), next);
                }
            }
            Some("placement") => {
                let area = patch.get("area").and_then(Value::as_str);
                let slot = match patch.get("by").and_then(Value::as_str) {
                    Some("owner") => &mut self.owner,
                    Some("router") => &mut self.router,
                    _ => return,
                };
                match area {
                    Some(area) if !area.is_empty() => {
                        slot.insert(about.to_owned(), area.to_owned())
                    }
                    // A cleared placement: not sorted again.
                    _ => slot.remove(about),
                };
            }
            _ => {}
        }
    }

    /// The live area `id` stands for (F2): Other for `other`; a merged area's target, through every
    /// merge; none for an unknown id, a loop, or a dismissed area.
    pub(crate) fn resolve<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        let mut seen = HashSet::new();
        let mut at = id;
        loop {
            if at == OTHER {
                return Some(OTHER);
            }
            if !seen.insert(at) {
                return None;
            }
            let area = self.areas.get(at)?;
            match area.merged_into() {
                Some(next) => at = next,
                None if area.status() == "dismissed" => return None,
                None => return Some(&area.id),
            }
        }
    }

    /// Where the request of anchor `subject` sits: the owner's placement while its area is live,
    /// else the router's; none: not sorted yet.
    pub(crate) fn of(&self, subject: &str) -> Option<&str> {
        let owner = self.owner.get(subject).and_then(|a| self.resolve(a));
        owner.or_else(|| self.router.get(subject).and_then(|a| self.resolve(a)))
    }

    /// F3: whether the owner placed the request of anchor `subject` in an area that is live, which
    /// the router then never places again (claude-mem's `focusDue`).
    pub(crate) fn owner_placed(&self, subject: &str) -> bool {
        (self.owner.get(subject)).is_some_and(|a| self.resolve(a).is_some())
    }

    /// The live area of the router's placement of anchor `subject`.
    pub(crate) fn router_of(&self, subject: &str) -> Option<&str> {
        self.router.get(subject).and_then(|a| self.resolve(a))
    }

    /// The ops of area `id`, by device and op seq.
    pub(crate) fn sources(&self, id: &str) -> &[(String, i64)] {
        self.sources.get(id).map_or(&[], Vec::as_slice)
    }

    /// The areas not merged away, by the owner's rank, then the goals' rank, then the name.
    pub(crate) fn listed(&self) -> Vec<&Area> {
        let mut out: Vec<&Area> = (self.areas.values())
            .filter(|a| a.merged_into().is_none())
            .collect();
        out.sort_by(|a, b| {
            (a.rank("owner_rank").total_cmp(&b.rank("owner_rank")))
                .then(a.rank("priority_rank").total_cmp(&b.rank("priority_rank")))
                .then(a.name().cmp(b.name()))
        });
        out
    }
}

/// Whether `v` is an id of My Focus as code makes one: `other`, or `fa_` and 12 hex digits.
pub(crate) fn is_id(v: &Value) -> bool {
    v.as_str().is_some_and(|id| {
        id == OTHER
            || id.strip_prefix("fa_").is_some_and(|hex| {
                hex.len() == 12
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
    })
}

/// The subject of a request's placement (F2): its anchor conversation, by the request's held id;
/// a group is known by its anchor, so a conversation keeps its placement when grouping moves it out.
pub(crate) fn subject(held: &str) -> String {
    match held.strip_prefix("g:") {
        Some(anchor) => format!("c:{anchor}"),
        None => held.to_owned(),
    }
}

/// F7: the areas as the page reads them, every text through the gate (S6).
pub(crate) fn areas(focus: &Focus, goals: &[Item], rules: &crate::redact::Rules) -> Vec<Value> {
    let gate = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(|t| crate::redact::outbound_with(t, rules))
    };
    (focus.listed().into_iter())
        .map(|a| {
            let of: Vec<&Item> = (goals.iter())
                .filter(|i| under(focus, i) == Some(a.id.as_str()))
                .collect();
            let priorities: Vec<&Item> = of.iter().copied().filter(|i| i.kind == PRIORITY).collect();
            // Each quote once, by its time and words, the newest first (claude-mem's `toFocusModel`).
            let mut quotes: Vec<&Quote> = priorities.iter().flat_map(|i| &i.quotes).collect();
            quotes.sort_by(|x, y| (y.ts, &y.text).cmp(&(x.ts, &x.text)));
            quotes.dedup_by(|x, y| (x.ts, &x.text) == (y.ts, &y.text));
            let asked = (!priorities.is_empty()).then(|| priorities.iter().map(|i| i.asked).sum::<u32>());
            let asks: Vec<Value> = (of.iter())
                .filter(|i| i.kind != PRIORITY)
                .map(|i| ask(i, rules))
                .collect();
            json!({"id": a.id, "name": gate(a.fields.get("name")), "goal": gate(a.fields.get("goal")),
                "about": gate(a.fields.get("about")), "status": a.status(),
                "source": a.fields.get("source"), "evidence": gate(a.fields.get("evidence")),
                "owner_rank": a.fields.get("owner_rank"), "priority_rank": a.fields.get("priority_rank"),
                "decided_by": a.fields.get("decided_by"), "page_word": page_word(a.name()),
                "asked": asked, "latest": quotes.first().map(|q| q.ts),
                "quotes": quoted(quotes.into_iter(), rules), "asks": asks})
        })
        .collect()
}

/// F7: the open asks under no live area, Other's (claude-mem's `otherAsks`).
pub(crate) fn other_asks(
    focus: &Focus,
    goals: &[Item],
    rules: &crate::redact::Rules,
) -> Vec<Value> {
    (goals.iter())
        .filter(|i| i.kind != PRIORITY && under(focus, i).is_none())
        .map(|i| ask(i, rules))
        .collect()
}

/// The words an area's name may not reuse, as `page_word` reads them, for the page to warn as a
/// person types one.
pub(crate) fn page_words() -> Vec<&'static str> {
    PAGE_WORDS.iter().chain(PAGE_NAMES).copied().collect()
}

const PRIORITY: &str = "priority";

/// The live area goal item `i` sits under, through merges; none under Other, or a dismissed or
/// unknown one (claude-mem's `liveArea`).
fn under<'a>(focus: &'a Focus, i: &'a Item) -> Option<&'a str> {
    (i.area.as_deref())
        .and_then(|a| focus.resolve(a))
        .filter(|a| *a != OTHER)
}

fn quoted<'a>(quotes: impl Iterator<Item = &'a Quote>, rules: &crate::redact::Rules) -> Vec<Value> {
    quotes
        .map(|q| json!({"text": crate::redact::outbound_with(&q.text, rules), "ts": q.ts}))
        .collect()
}

/// An open ask as the header lists it: "Not started", with how often and when it was asked last.
fn ask(i: &Item, rules: &crate::redact::Rules) -> Value {
    json!({"id": i.id, "title": crate::redact::outbound_with(&i.title, rules), "asked": i.asked,
        "latest": i.quotes.first().map(|q| q.ts), "quotes": quoted(i.quotes.iter(), rules)})
}

/// claude-mem's `navWordIn`: the page word a name reuses, as whole words of its ASCII letters and
/// digits, or the page's Japanese tab name it is.
pub(crate) fn page_word(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let padded = format!(" {} ", words.join(" "));
    (PAGE_WORDS.iter())
        .find(|w| padded.contains(&format!(" {w} ")))
        .map(|w| (*w).to_owned())
        .or_else(|| (PAGE_NAMES.iter().find(|n| name.trim() == **n)).map(|n| (*n).to_owned()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    action: String,
    area: Option<String>,
    into: Option<String>,
    name: Option<String>,
    goal: Option<String>,
    order: Option<Vec<String>>,
    request: Option<String>,
}

/// F6: the owner's action on My Focus, `POST /api/my-focus`, refused before it writes when it
/// breaks claude-mem's rules; each change an op of job `focus` decided by the owner.
pub(crate) fn act(home: &Path, saving: &Mutex<()>, body: &[u8]) -> Result<Value, Refusal> {
    let posted: Action =
        serde_json::from_slice(body).map_err(|_| refused(400, "bad_request", ""))?;
    let id_ok = |id: &Option<String>| id.as_deref().is_some_and(|i| (1..=80).contains(&i.len()));
    let _held = saving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unavailable = |_| refused(503, "focus_unavailable", "");
    let rules = crate::redact::Rules::load(home).map_err(unavailable)?;
    let Some((raw, k)) = crate::search::b::stores(home).map_err(unavailable)? else {
        return Err(refused(404, "focus_not_found", "area"));
    };
    let reading =
        crate::curate::Reading::now(&raw, crate::curate::Reads::Live).map_err(unavailable)?;
    let focus = Focus::read(&raw, &k, &reading).map_err(unavailable)?;
    drop((raw, k));
    let ops: Vec<(String, Map<String, Value>)> = match posted.action.as_str() {
        "reorder" => {
            let order = posted.order.unwrap_or_default();
            let kept: HashSet<&str> = (focus.listed().into_iter())
                .filter(|a| a.status() == "kept")
                .map(|a| a.id.as_str())
                .collect();
            let given: HashSet<&str> = order.iter().map(String::as_str).collect();
            if order.is_empty() || given.len() != order.len() || given != kept {
                return Err(refused(400, "focus_order", "order"));
            }
            (order.iter().enumerate())
                .map(|(i, id)| (id.clone(), owned(json!({"owner_rank": i + 1}))))
                .collect()
        }
        "move-request" => {
            let Some(request) = posted
                .request
                .filter(|r| (1..=2_000).contains(&r.chars().count()))
            else {
                return Err(refused(400, "bad_request", "request"));
            };
            if !id_ok(&posted.area) {
                return Err(refused(400, "bad_request", "area"));
            }
            let to = posted.area.unwrap_or_default();
            if to != OTHER && !focus.areas.get(&to).is_some_and(Area::placeable) {
                let code = if focus.areas.contains_key(&to) {
                    409
                } else {
                    404
                };
                return Err(refused(code, "focus_move_target", "area"));
            }
            let held = held_of(home, &request).map_err(unavailable)?;
            let Some(held) = held else {
                return Err(refused(404, "focus_not_found", "request"));
            };
            let mut fields = Map::new();
            fields.insert("kind".into(), json!("placement"));
            fields.insert("area".into(), json!(to));
            fields.insert("by".into(), json!("owner"));
            fields.insert("evidence".into(), json!("Moved by you in My Work."));
            vec![(subject(&held), fields)]
        }
        action => {
            if !id_ok(&posted.area) {
                return Err(refused(400, "bad_request", "area"));
            }
            let id = posted.area.unwrap_or_default();
            let Some(area) = focus.areas.get(&id) else {
                return Err(refused(404, "focus_not_found", "area"));
            };
            if area.merged_into().is_some() && action != "restore-suggestion" {
                return Err(refused(409, "focus_merged", "area"));
            }
            let gate = |t: &str| crate::redact::outbound_with(t.trim(), &rules);
            let change = match action {
                "keep" => json!({"status": "kept"}),
                "dismiss" => json!({"status": "dismissed"}),
                "done" => json!({"status": "done"}),
                "rename" => {
                    let name = posted.name.as_deref().map(str::trim).unwrap_or_default();
                    if name.is_empty() || name.chars().count() > NAME_CHARS {
                        return Err(refused(400, "bad_request", "name"));
                    }
                    if page_word(name).is_some() {
                        return Err(refused(422, "focus_page_word", "name"));
                    }
                    json!({"name": gate(name)})
                }
                "edit-goal" => {
                    let goal = posted.goal.as_deref().map(str::trim).unwrap_or_default();
                    if goal.is_empty() || goal.chars().count() > GOAL_CHARS {
                        return Err(refused(400, "bad_request", "goal"));
                    }
                    json!({"goal": gate(goal)})
                }
                "merge" => {
                    let into = posted.into.unwrap_or_default();
                    if into == id {
                        return Err(refused(400, "focus_merge_self", "into"));
                    }
                    let Some(target) = focus.areas.get(&into) else {
                        return Err(refused(404, "focus_not_found", "into"));
                    };
                    if target.status() == "dismissed" || target.merged_into().is_some() {
                        return Err(refused(409, "focus_merge_target", "into"));
                    }
                    json!({"merged_into": into})
                }
                "restore-suggestion" => {
                    if area.text("source") == Some("owner") {
                        return Err(refused(409, "focus_restore", "area"));
                    }
                    let mut fields = owned(json!({"status": "suggested", "merged_into": null}));
                    fields.insert("decided_by".into(), json!("owner-restore"));
                    return write(home, vec![(id, fields)]);
                }
                _ => return Err(refused(400, "bad_request", "action")),
            };
            vec![(id, owned(change))]
        }
    };
    write(home, ops)
}

/// `change` alone, decided by the owner: a patch on the area, where claude-mem's `ownRow` writes
/// the whole area. A copy would keep the model's text of an area op a forget or an exclusion
/// takes (S5, S6), in an op that read nothing (Codex on #746).
fn owned(change: Value) -> Map<String, Value> {
    let mut fields = change.as_object().cloned().unwrap_or_default();
    fields.insert("kind".into(), json!("area"));
    fields.insert("decided_by".into(), json!("owner"));
    fields
}

fn write(home: &Path, ops: Vec<(String, Map<String, Value>)>) -> Result<Value, Refusal> {
    let ops = ops
        .into_iter()
        .map(|(about, fields)| (about, Value::Object(fields)));
    let applied = crate::my_work_ops::record(home, NAME, ops.collect())
        .map_err(|_| refused(503, "focus_unavailable", ""))?;
    Ok(json!({"state": if applied { "applied" } else { "pending" }}))
}

/// The held id of the request My Work shows as `shown`, read as the page reads it.
pub(crate) fn held_of(home: &Path, shown: &str) -> Result<Option<String>> {
    let (answer, held) = crate::my_work::reviewing(home, crate::db::now_ms())?;
    let answer = crate::view::answered("my-work", answer);
    let requests = answer["requests"].as_array().map_or(&[][..], Vec::as_slice);
    Ok((requests.iter().position(|r| r["id"] == shown)).map(|i| held[i].id().to_owned()))
}

#[cfg(test)]
mod tests;
