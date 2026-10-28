//! My Focus (docs/my-work-model.md F, decision 61): the focus areas and where each request sits,
//! as claude-mem 15.4 keeps them, from the ops of job `focus` (F8): an area op (subject its id), a
//! placement op (subject the request's anchor conversation) and the Other pass's op, each a patch.
//! The owner's actions (F6) are such ops with `decided_by: owner`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::settings::{Refusal, refused};

/// The job of the focus ops (S1).
pub(crate) const NAME: &str = "focus";
/// The bucket of the requests that fit no area: an id of placements only (F1).
pub(crate) const OTHER: &str = "other";
/// F1's caps, in characters.
const NAME_CHARS: usize = 80;
const GOAL_CHARS: usize = 300;
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
/// The page's Japanese tab names, which a name may not be.
const PAGE_NAMES: &[&str] = &["マイワーク", "目標", "規則"];
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
    fn text(&self, key: &str) -> Option<&str> {
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
    fn placeable(&self) -> bool {
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
}

impl Focus {
    /// The focus ops in knowledge.db, oldest first (time, device, op seq), passing over those left
    /// out while an exclusion stands (S6) and the forgotten ones (S5).
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
            "SELECT device, op_seq, about, fields FROM my_work
             WHERE job = ?1 AND skipped = 0 AND forgotten IS NULL ORDER BY ts, device, op_seq",
        )?;
        let rows = st.query_map([NAME], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (device, op_seq, about, fields) = row?;
            if hidden.contains(&(device, op_seq)) {
                continue;
            }
            let fields: Map<String, Value> = serde_json::from_str(&fields)?;
            out.apply(&about, fields);
        }
        Ok(out)
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

/// The subject of a request's placement (F2): its anchor conversation, by the request's held id;
/// a group is known by its anchor, so a conversation keeps its placement when grouping moves it out.
pub(crate) fn subject(held: &str) -> String {
    match held.strip_prefix("g:") {
        Some(anchor) => format!("c:{anchor}"),
        None => held.to_owned(),
    }
}

/// F7: the areas as the page reads them, every text through the gate (S6).
pub(crate) fn areas(focus: &Focus, rules: &crate::redact::Rules) -> Vec<Value> {
    let gate = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(|t| crate::redact::outbound_with(t, rules))
    };
    (focus.listed().into_iter())
        .map(|a| {
            json!({"id": a.id, "name": gate(a.fields.get("name")), "goal": gate(a.fields.get("goal")),
                "about": gate(a.fields.get("about")), "status": a.status(),
                "source": a.fields.get("source"), "evidence": gate(a.fields.get("evidence")),
                "owner_rank": a.fields.get("owner_rank"), "priority_rank": a.fields.get("priority_rank"),
                "decided_by": a.fields.get("decided_by")})
        })
        .collect()
}

/// claude-mem's `navWordIn`: the page word a name reuses, as whole words of its ASCII letters and
/// digits, or the page's Japanese tab name it is.
fn page_word(name: &str) -> Option<String> {
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
                .map(|(i, id)| {
                    (
                        id.clone(),
                        owned(&focus.areas[id.as_str()], json!({"owner_rank": i + 1})),
                    )
                })
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
                    let mut fields =
                        owned(area, json!({"status": "suggested", "merged_into": null}));
                    fields.insert("decided_by".into(), json!("owner-restore"));
                    return write(home, vec![(id, fields)]);
                }
                _ => return Err(refused(400, "bad_request", "action")),
            };
            vec![(id, owned(area, change))]
        }
    };
    write(home, ops)
}

/// The whole area with `change` on it, decided by the owner, as claude-mem's `ownRow` writes it.
fn owned(area: &Area, change: Value) -> Map<String, Value> {
    let mut fields = area.fields.clone();
    for (key, value) in change.as_object().cloned().unwrap_or_default() {
        fields.insert(key, value);
    }
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
fn held_of(home: &Path, shown: &str) -> Result<Option<String>> {
    let (answer, held) = crate::my_work::reviewing(home, crate::db::now_ms())?;
    let answer = crate::view::answered("my-work", answer);
    let requests = answer["requests"].as_array().map_or(&[][..], Vec::as_slice);
    Ok((requests.iter().position(|r| r["id"] == shown)).map(|i| held[i].key().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::b::fixture::Store;

    fn area(name: &str, status: &str, by: &str) -> Map<String, Value> {
        let v = json!({"kind": "area", "name": name, "status": status, "source": "owner-goals",
            "decided_by": by});
        v.as_object().unwrap().clone()
    }

    fn placement(area: Option<&str>, by: &str) -> Map<String, Value> {
        let v = json!({"kind": "placement", "area": area, "by": by});
        v.as_object().unwrap().clone()
    }

    /// F2, F8: claude-mem's two rules. An owner row holds against later system rows until Undo's
    /// owner-restore; a null removes a field; a merge forwards, a loop or a dismissal resolves to
    /// nothing; the owner's placement wins while its area is live, else the router's shows.
    #[test]
    fn the_fold_keeps_claude_mems_rules() {
        let mut f = Focus::default();
        f.apply("fa_1", area("Ship the viewer", "suggested", "system"));
        let mut goal = area("Ship the viewer", "suggested", "system");
        goal.insert("goal".into(), json!("A page that shows it all."));
        f.apply("fa_1", goal);
        f.apply("fa_1", area("Ship the viewer", "kept", "owner"));
        f.apply("fa_1", area("Renamed by a refresh", "suggested", "system"));
        assert_eq!(
            (f.areas["fa_1"].name(), f.areas["fa_1"].status()),
            ("Ship the viewer", "kept")
        );
        let mut cleared = area("Ship the viewer", "kept", "owner");
        cleared.insert("goal".into(), Value::Null);
        f.apply("fa_1", cleared);
        assert!(!f.areas["fa_1"].fields.contains_key("goal"));
        f.apply(
            "fa_1",
            area("Ship the viewer", "suggested", "owner-restore"),
        );
        f.apply("fa_1", area("Viewer", "suggested", "system"));
        assert_eq!(
            f.areas["fa_1"].name(),
            "Viewer",
            "system rows apply again after Undo"
        );
        // An unreadable row is not applied.
        f.apply("fa_x", area("", "kept", "system"));
        f.apply("fa_y", area("Odd", "maybe", "system"));
        f.apply(OTHER, area("Other", "kept", "system"));
        assert_eq!(f.areas.len(), 1);
        // Merges and dismissals.
        f.apply("fa_2", area("Old viewer", "kept", "owner"));
        let mut merged = area("Old viewer", "kept", "owner");
        merged.insert("merged_into".into(), json!("fa_1"));
        f.apply("fa_2", merged);
        f.apply("fa_3", area("Loop a", "kept", "owner"));
        f.apply("fa_4", area("Loop b", "kept", "owner"));
        for (from, to) in [("fa_3", "fa_4"), ("fa_4", "fa_3")] {
            let mut m = area("Loop", "kept", "owner");
            m.insert("merged_into".into(), json!(to));
            f.apply(from, m);
        }
        f.apply("fa_5", area("Dropped", "dismissed", "owner"));
        assert_eq!(
            ["fa_2", "fa_3", "fa_5", "fa_9", OTHER].map(|id| f.resolve(id)),
            [Some("fa_1"), None, None, None, Some(OTHER)]
        );
        // Placements.
        f.apply("c:claude:s1", placement(Some("fa_1"), "router"));
        f.apply("c:claude:s1", placement(Some("fa_2"), "owner"));
        assert_eq!(
            f.of("c:claude:s1"),
            Some("fa_1"),
            "the merge forwards the owner's"
        );
        f.apply("c:claude:s2", placement(Some(OTHER), "router"));
        f.apply("c:claude:s2", placement(Some("fa_5"), "owner"));
        assert_eq!(
            f.of("c:claude:s2"),
            Some(OTHER),
            "a dismissed area binds nothing"
        );
        f.apply("c:claude:s2", placement(None, "router"));
        assert_eq!(
            f.of("c:claude:s2"),
            None,
            "a cleared placement is not sorted"
        );
        assert_eq!(f.of("c:claude:s9"), None);
        // Both live and apart: the owner's wins, whatever came later.
        f.apply("fa_8", area("Search", "kept", "owner"));
        f.apply("c:claude:s3", placement(Some("fa_8"), "owner"));
        f.apply("c:claude:s3", placement(Some("fa_1"), "router"));
        assert_eq!(f.of("c:claude:s3"), Some("fa_8"));
        // Listed: the owner's rank, then the goals' rank, then the name; merged ones left out.
        let mut ranked = area("Zeta", "kept", "owner");
        ranked.insert("owner_rank".into(), json!(1));
        f.apply("fa_6", ranked);
        let mut by_goals = area("Beta", "suggested", "system");
        by_goals.insert("priority_rank".into(), json!(2));
        f.apply("fa_7", by_goals);
        let names: Vec<&str> = f.listed().iter().map(|a| a.name()).collect();
        assert_eq!(names, ["Zeta", "Beta", "Dropped", "Search", "Viewer"]);
    }

    #[test]
    fn a_group_is_placed_by_its_anchor_conversation() {
        assert_eq!(subject("g:claude:s1"), "c:claude:s1");
        assert_eq!(subject("c:codex:s2"), "c:codex:s2");
        assert_eq!(subject("r:42"), "r:42");
    }

    #[test]
    fn a_name_may_not_reuse_a_page_word() {
        for name in [
            "Other things",
            "Ship the My Work viewer",
            "MY RULES",
            "規則",
        ] {
            assert!(page_word(name).is_some(), "{name}");
        }
        for name in [
            "Work viewer",
            "Network",
            "My laptop",
            "規則正しく",
            "画面の修正",
        ] {
            assert_eq!(page_word(name), None, "{name}");
        }
    }

    struct Home {
        s: Store,
        saving: Mutex<()>,
    }

    impl Home {
        fn new() -> Self {
            Self {
                s: Store::new(),
                saving: Mutex::new(()),
            }
        }

        /// A system area op, as the jobs of slice 10b write one.
        fn suggested(&mut self, id: &str, name: &str) {
            let mut fields = area(name, "suggested", "system");
            fields.insert("source".into(), json!("clustering"));
            crate::my_work_ops::record(self.s.home.path(), NAME, vec![(id.into(), fields.into())])
                .unwrap();
        }

        fn act(&self, body: Value) -> std::result::Result<Value, (u16, &'static str)> {
            let body = serde_json::to_vec(&body).unwrap();
            act(self.s.home.path(), &self.saving, &body).map_err(|r| (r.status, r.code))
        }

        fn focus(&self) -> Focus {
            let k = crate::knowledge::open(self.s.home.path()).unwrap();
            let reading =
                crate::curate::Reading::now(&self.s.raw, crate::curate::Reads::Live).unwrap();
            Focus::read(&self.s.raw, &k, &reading).unwrap()
        }

        /// My Work's answer: each request's id and area, and the areas' names.
        fn page(&self) -> (Vec<(String, Value)>, Vec<String>) {
            let (answer, _) =
                crate::my_work::reviewing(self.s.home.path(), crate::db::now_ms()).unwrap();
            let answer = crate::view::answered("my-work", answer);
            let requests = (answer["requests"].as_array().unwrap().iter())
                .map(|r| (r["id"].as_str().unwrap().to_owned(), r["focus"].clone()))
                .collect();
            let areas = (answer["areas"].as_array().unwrap().iter())
                .map(|a| a["name"].as_str().unwrap().to_owned())
                .collect();
            (requests, areas)
        }
    }

    /// F6: every action is an op decided by the owner, read back by the fold and by My Work's
    /// answer; each of claude-mem's refusals writes nothing.
    #[test]
    fn the_owners_actions_are_ops_and_refusals_write_nothing() {
        let mut h = Home::new();
        let now = crate::db::now_ms();
        h.s.said("s1", "github.com/o/r", now - 60_000, "Fix the save button.");
        h.s.run();
        h.suggested("fa_a", "Viewer");
        h.suggested("fa_b", "Search");
        h.suggested("fa_c", "Docs");
        let ok = |r: std::result::Result<Value, (u16, &'static str)>| {
            assert_eq!(r.unwrap()["state"], "applied");
        };
        ok(h.act(json!({"action": "keep", "area": "fa_a"})));
        ok(h.act(json!({"action": "keep", "area": "fa_b"})));
        ok(h.act(json!({"action": "rename", "area": "fa_a", "name": "  The viewer  "})));
        ok(h.act(json!({"action": "edit-goal", "area": "fa_a", "goal": "Show it all."})));
        ok(h.act(json!({"action": "reorder", "order": ["fa_b", "fa_a"]})));
        let f = h.focus();
        let a = &f.areas["fa_a"];
        assert_eq!(
            (a.name(), a.status(), a.text("goal"), a.text("decided_by")),
            ("The viewer", "kept", Some("Show it all."), Some("owner"))
        );
        let names: Vec<&str> = f.listed().iter().map(|a| a.name()).collect();
        assert_eq!(names, ["Search", "The viewer", "Docs"]);
        // Moving the request, and the answer saying where it sits.
        let (requests, areas) = h.page();
        assert_eq!(areas, ["Search", "The viewer", "Docs"]);
        let id = requests[0].0.clone();
        assert_eq!(requests[0].1, Value::Null, "not sorted yet");
        ok(h.act(json!({"action": "move-request", "request": id, "area": "fa_a"})));
        assert_eq!(h.page().0[0].1, json!("fa_a"));
        ok(h.act(json!({"action": "merge", "area": "fa_a", "into": "fa_b"})));
        assert_eq!(h.page().0[0].1, json!("fa_b"), "the merge forwards it");
        ok(h.act(json!({"action": "restore-suggestion", "area": "fa_a"})));
        assert_eq!(h.focus().areas["fa_a"].status(), "suggested");
        assert_eq!(h.page().0[0].1, json!("fa_a"));
        ok(h.act(json!({"action": "move-request", "request": id, "area": OTHER})));
        // A name and a goal go through the gate as they are written.
        std::fs::write(
            h.s.home.path().join("config.toml"),
            "[redaction]\nextra_rules = [{ id = \"t\", regex = 'tok-[0-9]{6}' }]\n",
        )
        .unwrap();
        ok(h.act(json!({"action": "rename", "area": "fa_b", "name": "Search tok-123456"})));
        ok(h.act(json!({"action": "edit-goal", "area": "fa_b", "goal": "Find tok-654321."})));
        let b = &h.focus().areas["fa_b"];
        assert!(
            !format!("{:?}", b.fields).contains("tok-"),
            "{:?}",
            b.fields
        );
        std::fs::remove_file(h.s.home.path().join("config.toml")).unwrap();
        ok(h.act(json!({"action": "rename", "area": "fa_b", "name": "Search"})));
        ok(h.act(json!({"action": "dismiss", "area": "fa_c"})));
        ok(h.act(json!({"action": "done", "area": "fa_b"})));
        // Refusals, none of which writes.
        let before = h.focus().areas;
        for (body, refusal) in [
            (
                json!({"action": "keep", "area": "fa_z"}),
                (404, "focus_not_found"),
            ),
            (
                json!({"action": "rename", "area": "fa_a", "name": "Other work"}),
                (422, "focus_page_word"),
            ),
            (
                json!({"action": "rename", "area": "fa_a", "name": "x".repeat(81)}),
                (400, "bad_request"),
            ),
            (
                json!({"action": "edit-goal", "area": "fa_a", "goal": " "}),
                (400, "bad_request"),
            ),
            (
                json!({"action": "merge", "area": "fa_a", "into": "fa_a"}),
                (400, "focus_merge_self"),
            ),
            (
                json!({"action": "merge", "area": "fa_a", "into": "fa_c"}),
                (409, "focus_merge_target"),
            ),
            (
                json!({"action": "merge", "area": "fa_a", "into": "fa_z"}),
                (404, "focus_not_found"),
            ),
            (
                json!({"action": "reorder", "order": ["fa_a"]}),
                (400, "focus_order"),
            ),
            (
                json!({"action": "move-request", "request": id, "area": "fa_c"}),
                (409, "focus_move_target"),
            ),
            (
                json!({"action": "move-request", "request": id, "area": "fa_z"}),
                (404, "focus_move_target"),
            ),
            (
                json!({"action": "move-request", "request": "c:claude:none", "area": OTHER}),
                (404, "focus_not_found"),
            ),
            (
                json!({"action": "explode", "area": "fa_a"}),
                (400, "bad_request"),
            ),
            (
                json!({"action": "keep", "area": "fa_a", "extra": 1}),
                (400, "bad_request"),
            ),
        ] {
            assert_eq!(h.act(body.clone()), Err(refusal), "{body}");
        }
        ok(h.act(json!({"action": "merge", "area": "fa_a", "into": "fa_b"})));
        assert_eq!(
            h.act(json!({"action": "keep", "area": "fa_a"})),
            Err((409, "focus_merged"))
        );
        let mut owned = area("Mine", "suggested", "owner");
        owned.insert("source".into(), json!("owner"));
        crate::my_work_ops::record(h.s.home.path(), NAME, vec![("fa_o".into(), owned.into())])
            .unwrap();
        assert_eq!(
            h.act(json!({"action": "restore-suggestion", "area": "fa_o"})),
            Err((409, "focus_restore"))
        );
        let after = h.focus().areas;
        assert_eq!(after["fa_c"], before["fa_c"]);
        assert_eq!(after["fa_a"].merged_into(), Some("fa_b"));
    }

    /// F8: the areas and placements are ops, so a rebuild keeps them.
    #[test]
    fn a_rebuild_keeps_my_focus() {
        let mut h = Home::new();
        let now = crate::db::now_ms();
        h.s.said("s1", "github.com/o/r", now - 60_000, "Fix the save button.");
        h.s.run();
        h.suggested("fa_a", "Viewer");
        h.act(json!({"action": "keep", "area": "fa_a"})).unwrap();
        let id = h.page().0[0].0.clone();
        h.act(json!({"action": "move-request", "request": id, "area": "fa_a"}))
            .unwrap();
        let before = h.page();
        let Store { home, raw } = h.s;
        drop(raw);
        crate::worker::rebuild(home.path()).unwrap();
        let raw = crate::raw::open(home.path()).unwrap();
        let h = Home {
            s: Store { home, raw },
            saving: Mutex::new(()),
        };
        assert_eq!(h.page(), before);
        assert_eq!(before.0[0].1, json!("fa_a"));
    }
}
