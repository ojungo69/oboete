//! claude-mem 15.19.0's work-view snapshot (decision 114, docs/page.md): the answer of
//! `GET /api/work-view`, which the vendored pages read in claude-mem's legacy mode
//! (`ui/work-view/`). It is made from oboete's My Work and goals answers after the outbound gate
//! (`view::answered`), and reads nothing else: each text in it is one of theirs or a word of this
//! file's. The names follow claude-mem's `live/work-view-api-types.ts`. The contract test runs
//! claude-mem's own mapper, `live/to-work-model.ts`, over the answer of a synthetic home
//! (`ui/work-view/contract/`).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::LazyLock;

use chrono::{Local, Months, NaiveDate, SecondsFormat, TimeZone, Utc};
use regex::Regex;
use serde_json::{Map, Value, json};

/// The snapshot, its goals' times in this machine's zone: the pages show those as written.
pub(crate) fn snapshot(work: &Value, goals: &Value, now: i64) -> Value {
    snapshot_at(work, goals, now, &Local)
}

/// `snapshot`, its goals' times in `tz`.
pub(crate) fn snapshot_at<Tz: TimeZone>(work: &Value, goals: &Value, now: i64, tz: &Tz) -> Value
where
    Tz::Offset: Display,
{
    let mut agents = Vec::new();
    let mut seen = HashMap::new();
    let requests: Vec<Value> = (arr(&work["requests"]).iter())
        .map(|r| request(r, &mut agents, &mut seen, tz))
        .collect();
    let areas: Vec<Value> = arr(&work["areas"]).iter().map(focus_area).collect();
    // The pages count unlinked work and read nothing of it.
    let unlinked = usize::try_from(work["unlinked"].as_u64().unwrap_or(0)).unwrap_or(0);
    json!({
        "version": 1, "generatedAt": iso(now), "revision": now.to_string(),
        "source": {"kind": "oboete", "connected": true, "error": null, "lastCheckedAt": iso(now)},
        "agents": agents, "humanRequests": requests, "unlinkedWork": vec![json!({}); unlinked],
        "focusAreas": areas, "goals": goals_list(work, goals, now, tz),
        "goalHistory": history(goals, now, tz),
        "ruleScopes": [{"id": "everywhere", "tier": "everywhere", "name": "Everywhere"}],
        "rules": [], "story": build_story(&work["story"]), "refresh": null,
        // oboete has no cloud sync: a valid "unconfigured" block (`sync-presentation.ts:23-36`)
        // shows "Cloud sync off" where a missing one shows "Cloud freshness unknown".
        "sync": {"configured": false, "state": "unconfigured", "mode": "disabled",
            "checkedAt": null, "observedAt": now,
            "message": "oboete keeps your work on this computer only.", "download": null,
            "projection": {"remaining": null},
            "completeness": {"pushQuarantine": 0, "pullQuarantine": 0, "excludedNative": null,
                "scope": "unknown"}},
    })
}

/// The Build story: the recap, then each week's chapter under its week. The mapper splits the
/// narrative at blank lines and takes its first block that is not a heading as the recap
/// (`to-work-model.ts:433-442`), so a story of chapters alone opens on its newest chapter, and a
/// recap of headings alone gives their text as its paragraphs, as the pages show a heading's.
fn build_story(story: &Value) -> Value {
    let recap = story["recap"].as_str().map(blocks).unwrap_or_default();
    let mut out: Vec<String> = match recap.iter().any(|b| !b.starts_with('#')) {
        true => recap,
        false => (recap.iter())
            .filter_map(|h| prose(h.trim_start_matches('#')))
            .collect(),
    };
    for c in arr(&story["chapters"]) {
        if let (Some(week), Some(text)) = (c["week"].as_str(), c["text"].as_str()) {
            out.push(format!("## {week}"));
            out.extend(blocks(text));
        }
    }
    match out.is_empty() {
        true => Value::Null,
        false => json!({"story": {"narrative": out.join("\n\n"), "writtenAt": null}}),
    }
}

/// A text's blocks as the pages read a narrative (`to-work-model.ts:433-442`): a Markdown heading
/// line is a block of its own, and the lines between headings and blank lines are a paragraph.
fn blocks(text: &str) -> Vec<String> {
    fn heading(line: &str) -> bool {
        let t = line.trim_start_matches(space);
        let n = t.bytes().take_while(|&b| b == b'#').count();
        (1..=6).contains(&n) && t[n..].chars().next().is_none_or(space)
    }
    let (mut out, mut lines) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let blank = line.chars().all(space);
        if blank || heading(line) {
            out.extend(prose(&lines.join("\n")));
            lines.clear();
        }
        if heading(line) {
            out.push(line.trim_matches(space).to_owned());
        } else if !blank {
            lines.push(line);
        }
    }
    out.extend(prose(&lines.join("\n")));
    out
}

/// A paragraph as the pages read it, or none when they would show it as nothing. The pages take a
/// block for a heading when it opens with `#` once its bold and code marks are off, so a paragraph
/// that would (an issue's number: `#1087`, `**#1087**`) opens with a word joiner, and the pages
/// read it as the prose it is.
fn prose(block: &str) -> Option<String> {
    static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*").unwrap());
    static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
    let block = block.trim_matches(space);
    let shown = BOLD.replace_all(block, "$1");
    let shown = CODE.replace_all(&shown, "$1");
    let first = shown.split(space).find(|w| !w.is_empty())?;
    Some(match first.starts_with('#') {
        true => format!("\u{2060}{block}"),
        false => block.to_owned(),
    })
}

/// JavaScript's `\s`, by which the pages split and trim, holds U+FEFF, which Rust's does not.
fn space(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

/// Unix ms as ISO time with 3 fraction digits, in UTC: the times the mapper parses.
fn iso(ms: i64) -> Value {
    (Utc.timestamp_millis_opt(ms).single()).map_or(Value::Null, |t| {
        json!(t.to_rfc3339_opts(SecondsFormat::Millis, true))
    })
}

/// Unix ms as ISO time in `tz`, with its offset: the goals list's, whose day the pages show as
/// written (`to-work-model.ts:321-325`).
fn zoned<Tz: TimeZone>(ms: i64, tz: &Tz) -> Value
where
    Tz::Offset: Display,
{
    (tz.timestamp_millis_opt(ms).single()).map_or(Value::Null, |t| {
        json!(t.to_rfc3339_opts(SecondsFormat::Millis, false))
    })
}

/// Unix ms as `YYYY-MM-DD HH:MM` in `tz`: how the mapper reads a goal's times
/// (`to-work-model.ts:259`), showing the digits as written.
fn stamp<Tz: TimeZone>(ms: &Value, tz: &Tz) -> Value {
    (ms.as_i64()
        .and_then(|ms| tz.timestamp_millis_opt(ms).single()))
    .map_or(Value::Null, |t| {
        json!(t.naive_local().format("%Y-%m-%d %H:%M").to_string())
    })
}

/// A civil day (`YYYY-MM-DD`) at noon with no zone: the browser reads it in its own zone, so the
/// day shown is the one written, wherever it is shown. None for anything else, a day the gate
/// masked among them: the pages would take it for a date they cannot read.
fn noon(day: &Value) -> Value {
    match day
        .as_str()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    {
        Some(d) => json!(format!("{d}T12:00:00")),
        None => Value::Null,
    }
}

fn s(v: &Value) -> Value {
    match v {
        Value::String(_) => v.clone(),
        _ => Value::Null,
    }
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[][..], Vec::as_slice)
}

/// claude-mem's platform for an oboete agent. The same name goes in the conversation keys, which
/// find their agent by it (`to-work-model.ts:226`); a name claude-mem does not know shows as the
/// tool "other".
fn platform(agent: &str) -> &str {
    match agent {
        "agy" => "gemini",
        a => a,
    }
}

fn label(agent: &str) -> &'static str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "grok" => "Grok",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        "pi" => "Pi",
        "agy" => "Antigravity",
        _ => "Agent",
    }
}

/// oboete's status as claude-mem's explicit lifecycle (`status-normalization.ts:94-107`).
fn lifecycle(status: &str) -> &'static str {
    match status {
        "needs" => "needs-you",
        "ready" => "needs-review",
        "working" => "in-progress",
        "blocked" => "blocked",
        "quiet" => "quiet",
        "todo" => "todo",
        "closed" => "closed",
        "shipped" => "shipped",
        _ => "unknown",
    }
}

/// One My Work request as claude-mem's human request; each conversation's agent goes in `agents`
/// once, titled by the conversation's own request, else by the first request that names it.
fn request<Tz: TimeZone>(
    r: &Value,
    agents: &mut Vec<Value>,
    seen: &mut HashMap<String, usize>,
    tz: &Tz,
) -> Value {
    let status = r["status"].as_str().unwrap_or("unknown");
    let mut own = r["id"].clone();
    let mut agent_ids = Vec::new();
    let mut keys = Vec::new();
    for c in arr(&r["sessions"]) {
        let (agent, id) = (
            c["agent"].as_str().unwrap_or(""),
            c["id"].as_str().unwrap_or(""),
        );
        let key = format!("{agent}:{id}");
        let at = *seen.entry(key.clone()).or_insert_with(|| {
            agents.push(json!({
                "id": key, "label": label(agent), "platformSource": platform(agent),
                "sessionTitle": s(&r["title"]),
                "project": arr(&r["projects"]).first().cloned().unwrap_or(Value::Null),
                "contentSessionId": id,
            }));
            agents.len() - 1
        });
        let conversation = json!(["conversation", platform(agent), id]).to_string();
        // A conversation's own request is known by the conversation's key, as claude-mem's is:
        // its drawer gives that conversation the ask's time (`to-work-model.ts:228`). A write
        // that names it takes oboete's id back from the key's last part.
        if r["id"] == id {
            own = json!(conversation);
            // Its title and project are the conversation's, whichever request named it first.
            agents[at]["sessionTitle"] = s(&r["title"]);
            agents[at]["project"] = arr(&r["projects"]).first().cloned().unwrap_or(Value::Null);
        }
        keys.push(json!(conversation));
        agent_ids.push(json!(key));
    }
    // A list-level update has no task: the list's name stands for it, as on oboete's own page. A
    // status is read as My Work reads it: the pages take a task as done by its word alone
    // (`to-work-model.ts:216`), so `Done` is `done` here too.
    let items: Vec<Value> = (arr(&r["work"]).iter().enumerate())
        .map(|(i, w)| {
            let title = if w["task"].is_string() {
                &w["task"]
            } else {
                &w["list"]
            };
            json!({"id": format!("w{i}"), "kind": "work-state", "title": s(title),
                "status": w["status"].as_str().map(crate::my_work::normalized).unwrap_or_default(), "nextStep": null,
                "lastUpdateEpoch": w["updated"], "sourceRefs": [], "url": ""})
        })
        .collect();
    // Each work list is a workstream, as claude-mem's drawer shows a request's tasks by stream
    // (`to-work-model.ts:214-217`). A list is one per repository, as oboete keeps it; a masked
    // one is known by its fingerprint, in a namespace of its own.
    let mut streams: Vec<(String, Value, Vec<Value>)> = Vec::new();
    for (w, item) in arr(&r["work"]).iter().zip(&items) {
        let id = match w["stream"].as_str() {
            Some(print) => format!("masked:{print}"),
            None => json!([&w["repo"], &w["list"]]).to_string(),
        };
        match streams.iter_mut().find(|(known, ..)| *known == id) {
            Some((.., of)) => of.push(item.clone()),
            None => streams.push((id, s(&w["list"]), vec![item.clone()])),
        }
    }
    let mut copy = Map::new();
    copy.insert("state".into(), json!("current"));
    for field in ["title", "summary", "next"] {
        if r[field].is_string() {
            copy.insert(field.into(), r[field].clone());
        }
    }
    let titles: Vec<Value> = (streams.iter())
        .map(|(id, title, _)| json!({"id": id, "title": title}))
        .collect();
    copy.insert("workstreams".into(), json!(titles));
    let workstreams: Vec<Value> = (streams.into_iter())
        .map(|(id, title, items)| json!({"id": id, "title": title, "items": items}))
        .collect();
    let activity: Vec<Value> = (arr(&r["updates"]).iter().enumerate())
        .map(|(i, u)| {
            json!({"id": format!("u{i}"), "kind": "work-state", "title": s(&u["text"]),
                "detail": null, "atEpoch": u["ts"], "url": ""})
        })
        .collect();
    let follow: Vec<Value> = (arr(&r["follow_ups"]).iter())
        .map(|f| json!({"text": s(&f["text"]), "atEpoch": f["ts"]}))
        .collect();
    // The review the page shows is its newest reading: its own percent and state go, as
    // claude-mem's review does. A request shipped by its status after a review below 100 is
    // 100% by its status, not by its review (`to-work-model.ts:198-200`), and the pages check
    // that the percent is the newest reading's (`data.ts:282`).
    let progress = &r["progress"];
    let readings: Vec<Value> = (arr(&progress["readings"]).iter())
        .filter_map(|r| {
            Some(json!({"at": iso(r["ts"].as_i64()?), "percent": r["percent"].as_f64()?}))
        })
        .collect();
    let reviewed = (progress["reviewed"].as_i64()).zip(
        readings
            .last()
            .map_or(progress["percent"].as_f64(), |r| r["percent"].as_f64()),
    );
    // A review at 100% of a shipped request goes as a shipped review, with the day or time it
    // shipped: the pages take that day for the request's completion when no status gave one
    // (`to-work-model.ts:148-149`), in the browser's zone, as the day is written with none.
    let shipped_review =
        reviewed.is_some_and(|(_, percent)| status == "shipped" && percent >= 100.0);
    let shipped_at = match progress["completed_at"].as_i64() {
        Some(at) => iso(at),
        None => noon(&progress["completed"]),
    };
    // When the newest status recorded completed the request: a done one's clock stops by it. A
    // request shipped by its review alone has no such time: its shipped review's day completes it.
    // When the review cannot carry that day (it is below 100% or does not go, or its day is not
    // one), the day at noon on this machine does, and the request's last update is the last
    // resort: a shipped request needs a completion (`data.ts:276`).
    let noon_here = (progress["completed"].as_str())
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .and_then(|d| d.and_hms_opt(12, 0, 0))
        .and_then(|t| tz.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp_millis());
    let completed = match (r["completed_at"].as_i64(), status) {
        (Some(at), _) => json!(at),
        (None, "shipped") if shipped_review && !shipped_at.is_null() => Value::Null,
        (None, "shipped") => noon_here.map_or_else(|| r["updated"].clone(), |at| json!(at)),
        _ => Value::Null,
    };
    // A review's progress, when a review was recorded: a request shipped by its status alone is
    // complete by definition (`to-work-model.ts:198-200`), with no reading of its own.
    let story = &r["story"];
    let progress = match reviewed {
        Some((reviewed, percent)) => json!({
            "state": if shipped_review { "shipped" } else { "in-progress" },
            "percent": percent,
            "checkedAt": iso(reviewed),
            "shippedAt": shipped_at,
            "readings": readings,
            "narrative": s(&story["narrative"]),
            "milestones": (arr(&story["milestones"]).iter())
                .map(|m| json!({"at": noon(&m["date"]), "text": s(&m["text"])}))
                .collect::<Vec<_>>(),
        }),
        None => Value::Null,
    };
    let mut out = json!({
        "id": own, "contentFingerprint": r["version"],
        "title": s(&r["title"]),
        "titleSource": if r["title"].is_string() { json!("title-cache") } else { Value::Null },
        "humanCopy": Value::Object(copy),
        "original": {"text": s(&r["ask"]["text"]), "atEpoch": r["ask"]["ts"]},
        "scopeUpdates": follow, "items": items, "workstreams": workstreams, "activity": activity,
        "agentIds": agent_ids, "conversationKeys": keys, "requestedBy": null,
        "startedAtEpoch": r["started"], "updatedAtEpoch": r["updated"],
        "completedAtEpoch": completed,
        "lifecycle": {"status": lifecycle(status), "explicit": true, "complete": null,
            "atEpoch": null},
        "review": {"checked": !r["checked"].is_null(), "checkedAtEpoch": r["checked"],
            "available": status == "ready"},
        "progress": progress, "anchorConversationKey": null,
    });
    if r["focus"].is_string() {
        out["focusAreaId"] = r["focus"].clone();
    }
    out
}

/// A focus area as claude-mem lists them. A nameless area sorts by an empty name: the pages
/// compare names whole (`to-focus-model.ts:23`), and a null would stop every screen.
fn focus_area(a: &Value) -> Value {
    json!({
        "focusAreaId": a["id"], "name": a["name"].as_str().unwrap_or(""), "goal": s(&a["goal"]),
        "about": s(&a["about"]), "status": a["status"], "priorityRank": a["priority_rank"],
        "ownerRank": a["owner_rank"], "source": a["source"], "mergedInto": null,
        "evidence": s(&a["evidence"]), "decidedBy": a["decided_by"], "savedAt": null,
    })
}

fn evidence<Tz: TimeZone>(quotes: &Value, tz: &Tz) -> Vec<Value> {
    (arr(quotes).iter())
        .map(|q| json!({"at": stamp(&q["ts"], tz), "quote": s(&q["text"])}))
        .collect()
}

/// A goal item as claude-mem's goals list holds it (`WorkViewGoalItem`), every field of its type.
fn goal_item(item: Value) -> Value {
    let mut out = json!({"task": "", "goal": null, "status": "todo"});
    if let (Value::Object(base), Value::Object(item)) = (&mut out, item) {
        base.extend(item);
    }
    out
}

/// The goals list the focus screens read (`to-work-model.ts:338-357`), from the newest goals the
/// page shows: an item per area that was asked for, and one per open ask, under its area or under
/// none. oboete's `areas[]` already fold its priorities in. None when no goals were written.
fn goals_list<Tz: TimeZone>(work: &Value, page: &Value, now: i64, tz: &Tz) -> Value
where
    Tz::Offset: Display,
{
    let Some(written) = page["current"]["written"].as_i64() else {
        return Value::Null;
    };
    let mut areas = Vec::new();
    let mut asks = Vec::new();
    for a in arr(&work["areas"]) {
        // An area with open asks and no priority under it has no count of its own (`asked` is
        // null): it has no item of its own, and its asks stay under it.
        if let Some(asked) = a["asked"].as_f64() {
            areas.push(goal_item(json!({
                "id": format!("a:{}", a["id"].as_str().unwrap_or("")), "kind": "focus-area",
                "rank": areas.len() + 1, "title": s(&a["name"]), "goal": s(&a["goal"]),
                "focusArea": s(&a["name"]), "focusAreaId": a["id"], "product": null,
                "latestAt": stamp(&a["latest"], tz), "timesAsked": asked,
                "evidence": evidence(&a["quotes"], tz),
            })));
        }
        asks.extend(
            arr(&a["asks"])
                .iter()
                .map(|k| (k, s(&a["name"]), a["id"].clone())),
        );
    }
    asks.extend(
        arr(&work["other_asks"])
            .iter()
            .map(|k| (k, Value::Null, Value::Null)),
    );
    let asks: Vec<Value> = (asks.into_iter().enumerate())
        .map(|(i, (k, area, id))| {
            goal_item(json!({"id": s(&k["id"]), "kind": "ask", "rank": i + 1,
                "title": s(&k["title"]), "focusArea": area, "focusAreaId": id, "product": null,
                "latestAt": stamp(&k["latest"], tz),
                "timesAsked": k["asked"].as_f64().unwrap_or(0.0),
                "evidence": evidence(&k["quotes"], tz)}))
        })
        .collect();
    // The page reads the window that ends now, its quotes and its prompts (`my_work_goals::page`),
    // not the one that ended at the run: its days are the period's, the run's time its own.
    let days = page["window"]["days"].as_i64();
    let day = |ms: i64| tz.timestamp_millis_opt(ms).single().map(|t| t.date_naive());
    let start = days
        .and_then(|d| day(now - d * 86_400_000))
        .map(|d| d.to_string());
    let end = (day(now).and_then(|d| d.succ_opt())).map(|d| d.to_string());
    json!({
        "listId": "goals", "listName": "goals", "history": false, "project": "",
        "updatedAt": iso(now), "periodStart": start, "periodEnd": end, "summary": null,
        "generatedAt": zoned(written, tz),
        "windowDays": page["window"]["days"], "sourcePromptCount": page["window"]["prompts"],
        "mapPath": null, "areas": areas, "asks": asks,
    })
}

/// Past focus (`to-work-model.ts:309-318`): each finished month of goals as a list of its own,
/// from its first day to the next month's first, its priorities as its areas.
fn history<Tz: TimeZone>(page: &Value, now: i64, tz: &Tz) -> Vec<Value> {
    (arr(&page["months"]).iter())
        .filter_map(|m| {
            let month = m["month"].as_str()?;
            let start = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").ok()?;
            let end = start.checked_add_months(Months::new(1))?;
            let areas: Vec<Value> = (arr(&m["priorities"]).iter().enumerate())
                .map(|(i, p)| {
                    let quote = &p["quote"];
                    let evidence: Vec<Value> = (quote.is_object())
                        .then(|| json!({"at": stamp(&quote["ts"], tz), "quote": s(&quote["text"])}))
                        .into_iter()
                        .collect();
                    // A priority's product names it where no area does; an empty one, none.
                    let product = (p["product"].as_str()).filter(|t| !t.is_empty());
                    goal_item(
                        json!({"id": format!("p:{month}:{}", i + 1), "kind": "focus-area",
                        "rank": i + 1, "title": s(&p["title"]), "focusArea": null,
                        "focusAreaId": null, "product": product, "latestAt": null,
                        "timesAsked": p["asked"].as_f64().unwrap_or(0.0), "evidence": evidence}),
                    )
                })
                .collect();
            Some(json!({
                "listId": format!("goals:{month}"), "listName": month, "history": true,
                "project": "", "updatedAt": iso(now), "periodStart": start.to_string(),
                "periodEnd": end.to_string(), "summary": s(&m["summary"]), "generatedAt": null,
                "windowDays": null, "sourcePromptCount": m["prompts"], "mapPath": null,
                "areas": areas, "asks": [],
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests;
