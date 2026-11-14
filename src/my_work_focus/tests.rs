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

/// F7, claude-mem's `toFocusModel`: a priority lands on its area through a merge, its quotes
/// with the area's own priority's, each once, the newest first, the asked counts summed; an
/// ask under a dismissed area, under none or under Other is Other's; every text through the
/// gate, and a name that reuses a page word says which.
#[test]
fn each_area_carries_the_goals_behind_it() {
    let mut f = Focus::default();
    f.apply("fa_1", area("Viewer", "kept", "owner"));
    let mut merged = area("Old viewer", "kept", "owner");
    f.apply("fa_2", merged.clone());
    merged.insert("merged_into".into(), json!("fa_1"));
    f.apply("fa_2", merged);
    f.apply("fa_3", area("Gone", "dismissed", "owner"));
    f.apply("fa_4", area("The My Work page", "suggested", "system"));
    let q = |ts: i64, text: &str| Quote {
        seq: ts,
        ts,
        text: text.into(),
    };
    let item = |id: &str, kind: &str, area: Option<&str>, asked, quotes| Item {
        id: id.into(),
        kind: kind.into(),
        rank: 1,
        title: format!("Title {id} codename-x"),
        product: String::new(),
        asked,
        quotes,
        area: area.map(str::to_owned),
    };
    let goals = vec![
        item(
            "g1",
            "priority",
            Some("fa_1"),
            2,
            vec![q(30, "show it"), q(10, "a page")],
        ),
        item(
            "g2",
            "priority",
            Some("fa_2"),
            3,
            vec![q(30, "show it"), q(20, "codename-y")],
        ),
        item("g3", "ask", Some("fa_2"), 1, vec![q(25, "fix the chip")]),
        item("g4", "ask", Some("fa_3"), 1, vec![q(5, "gone")]),
        item("g5", "ask", None, 2, vec![q(6, "anything")]),
        item("g6", "ask", Some(OTHER), 1, vec![q(7, "other")]),
    ];
    let toml = "[redaction]\nextra_rules = [{ id = \"codename\", regex = \"codename-[a-z]+\" }]\n";
    let rules =
        crate::redact::Rules::new(&crate::config::parse_capture(Some(toml)).unwrap().redaction)
            .unwrap();
    let shown = areas(&f, &goals, &rules);
    let of = |id: &str| shown.iter().find(|a| a["id"] == id).unwrap().clone();
    let viewer = of("fa_1");
    assert_eq!(
        (&viewer["asked"], &viewer["latest"]),
        (&json!(5), &json!(30))
    );
    let texts: Vec<&str> = (viewer["quotes"].as_array().unwrap().iter())
        .map(|q| q["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert_eq!((texts[0], texts[2]), ("show it", "a page"));
    assert!(!texts[1].contains("codename-y"), "{texts:?}");
    let asks = viewer["asks"].as_array().unwrap();
    assert_eq!(
        (asks.len(), &asks[0]["id"], &asks[0]["latest"]),
        (1, &json!("g3"), &json!(25))
    );
    assert!(!asks[0]["title"].as_str().unwrap().contains("codename-x"));
    assert_eq!(viewer["page_word"], Value::Null);
    let gone = of("fa_3");
    assert_eq!(
        (&gone["asked"], &gone["latest"], &gone["asks"]),
        (&Value::Null, &Value::Null, &json!([]))
    );
    assert_eq!(of("fa_4")["page_word"], "my work");
    let other: Vec<String> = (other_asks(&f, &goals, &rules).iter())
        .map(|a| a["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(other, ["g4", "g5", "g6"]);
    assert!(page_words().contains(&"my work") && page_words().contains(&"その他"));
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
        "マイフォーカス",
        "その他",
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
        let reading = crate::curate::Reading::now(&self.s.raw, crate::curate::Reads::Live).unwrap();
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
    crate::my_work_ops::record(h.s.home.path(), NAME, vec![("fa_o".into(), owned.into())]).unwrap();
    assert_eq!(
        h.act(json!({"action": "restore-suggestion", "area": "fa_o"})),
        Err((409, "focus_restore"))
    );
    let after = h.focus().areas;
    assert_eq!(after["fa_c"], before["fa_c"]);
    assert_eq!(after["fa_a"].merged_into(), Some("fa_b"));
}

/// #746: an owner's action writes its change alone: an area whose model text an exclusion
/// takes (S6) is gone with the owner's decision on it, and no owner op holds the text.
#[test]
fn an_owners_action_keeps_none_of_the_models_text() {
    let mut h = Home::new();
    let repo = "github.com/o/x";
    let seq = h.s.said(
        "s1",
        repo,
        crate::db::now_ms() - 60_000,
        "Fix the save button.",
    );
    h.s.run();
    let op = crate::my_work_ops::MyWorkOp {
        job: NAME.into(),
        about: "fa_a".into(),
        entry: "fake".into(),
        ts: crate::db::now_ms(),
        read: crate::my_work_ops::Read {
            spans: vec![(seq, seq)],
            ops: Vec::new(),
        },
        removed: Vec::new(),
        fields: area("Save button work", "suggested", "system")
            .into_iter()
            .collect(),
        skipped: false,
        forgotten: None,
    };
    let op = serde_json::to_value(op).unwrap();
    h.s.raw
        .append_ops(&[(crate::raw::OpKind::MyWork, op)])
        .unwrap();
    h.s.run();
    h.act(json!({"action": "keep", "area": "fa_a"})).unwrap();
    assert_eq!(h.focus().areas["fa_a"].status(), "kept");
    let owner: i64 = rusqlite::Connection::open(h.s.home.path().join("raw.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM ops WHERE type = 'my_work' AND instr(body, 'Save button') > 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(owner, 1, "the system op alone holds the name");
    (h.s.raw).exclude_capture(("repo", repo), false).unwrap();
    h.s.run();
    assert!(!h.focus().areas.contains_key("fa_a"));
}

/// #746: an area's id, and the area a request sits in, pass the answer's gate as selectors,
/// so a rule that would mask them leaves the actions working.
#[test]
fn focus_ids_pass_the_answer_gate() {
    let mut h = Home::new();
    h.s.said(
        "s1",
        "github.com/o/r",
        crate::db::now_ms() - 60_000,
        "Fix the save button.",
    );
    h.s.run();
    let id = "fa_0123456789ab";
    h.suggested(id, "Viewer");
    h.act(json!({"action": "keep", "area": id})).unwrap();
    std::fs::write(
        h.s.home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"hex\", regex = '[0-9a-f]{12}' }]\n",
    )
    .unwrap();
    let request = h.page().0[0].0.clone();
    h.act(json!({"action": "move-request", "request": request, "area": id}))
        .unwrap();
    let (answer, _) = crate::my_work::reviewing(h.s.home.path(), crate::db::now_ms()).unwrap();
    let answer = crate::view::answered("my-work", answer);
    assert_eq!(
        (&answer["areas"][0]["id"], &answer["requests"][0]["focus"]),
        (&json!(id), &json!(id))
    );
    assert!(is_id(&json!(OTHER)) && !is_id(&json!("fa_0123456789AB")) && !is_id(&json!("fa_1")));
}

/// #747: a rule added later masks an area's name in the stored ops when the rescan runs, while
/// a placement and a merge keep naming their areas.
#[test]
fn the_op_rescan_keeps_my_focus_ids() {
    let mut h = Home::new();
    let now = crate::db::now_ms();
    h.s.said("s1", "github.com/o/r", now - 60_000, "Fix the save button.");
    h.s.run();
    let (a, b) = ("fa_0123456789ab", "fa_ba9876543210");
    h.suggested(a, "Viewer 0123456789ab");
    h.suggested(b, "Search");
    let request = h.page().0[0].0.clone();
    for body in [
        json!({"action": "keep", "area": a}),
        json!({"action": "keep", "area": b}),
        json!({"action": "move-request", "request": request, "area": a}),
        json!({"action": "merge", "area": a, "into": b}),
    ] {
        h.act(body).unwrap();
    }
    std::fs::write(
        h.s.home.path().join("config.toml"),
        "[redaction]\nextra_rules = [{ id = \"hex\", regex = '[0-9a-f]{12}' }]\n",
    )
    .unwrap();
    h.s.run();
    let Store { home, raw } = h.s;
    drop(raw);
    assert!(crate::forget::purge(home.path()).unwrap().is_some());
    let stored = |text: &str| -> i64 {
        let raw = rusqlite::Connection::open(home.path().join("raw.db")).unwrap();
        raw.query_row(
            "SELECT COUNT(*) FROM ops WHERE instr(body, ?1) > 0",
            [text],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        (stored("Viewer 0123456789ab"), stored("Viewer ****")),
        (0, 1)
    );
    let raw = crate::raw::open(home.path()).unwrap();
    let h = Home {
        s: Store { home, raw },
        saving: Mutex::new(()),
    };
    assert_eq!(h.focus().resolve(a), Some(b));
    assert_eq!(h.page().0[0].1, json!(b));
    // Text under those names is masked all the same.
    let rules = crate::redact::Rules::load(h.s.home.path()).unwrap();
    let text = json!({"about": a, "fields": {"area": "Viewer 0123456789ab"}}).to_string();
    let masked = crate::raw::masked_op_text(crate::raw::OpKind::MyWork, &text, &rules);
    assert!(masked.is_some_and(|m| m.contains("Viewer ****") && m.contains(a)));
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
