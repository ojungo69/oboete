use super::*;
use crate::config::Limits;
use crate::providers_db::{Call, Usage, open, record};

fn admit(
    db: &Connection,
    p: &Provider,
    tokens: f64,
    cap: f64,
    ceilings: &[u32],
) -> Result<Option<Refusal>> {
    admit_with_history(db, (p, p), "curator", tokens, cap, ceilings)
}

fn entry(name: &str, limits: Limits) -> Provider {
    Provider::Openai {
        enabled: true,
        name: name.into(),
        base_url: "http://127.0.0.1:9".into(),
        api: Default::default(),
        key_file: None,
        model: "m".into(),
        daily_budget: Some(10),
        timeout_s: 1,
        retry_429: false,
        extra: Default::default(),
        headers: Default::default(),
        limits,
        subscription: false,
    }
}

fn cost(
    db: &Connection,
    p: &Provider,
    est: u32,
    usage: Usage,
    billed: bool,
) -> Result<Option<f64>> {
    let reservation = reserve(db, p, "curator", "estimate", est, f64::MAX, &[])?
        .map_err(|r| anyhow::anyhow!(r.detail))?;
    let usd = reservation.cost(p, usage, billed);
    reservation.cancel(db)?;
    Ok(usd)
}

/// A call the budget refused: recorded, nothing sent, no usage.
fn refusal(db: &Connection, provider: &str) {
    record(
        db,
        &Call {
            provider,
            role: "curator",
            span: "s",
            outcome: "budget",
            ms: 0,
            detail: Some("budget"),
            bytes_out: 0,
            est_tokens: None,
            usage: Usage::default(),
            usd: None,
        },
    )
    .unwrap();
}

fn call(db: &Connection, provider: &str, est: Option<u32>, prompt: i64, completion: i64) {
    record(
        db,
        &Call {
            provider,
            role: "curator",
            span: "s",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: est,
            usage: Usage {
                prompt: Some(prompt),
                completion: Some(completion),
                ..Default::default()
            },
            usd: None,
        },
    )
    .unwrap();
}

/// `oboete recurate`'s estimate: every window billed by every paid entry, its largest answer
/// each time; none when nothing is paid.
#[test]
fn the_most_a_recuration_costs_is_every_paid_entry_billed() {
    let db = open(tempfile::tempdir().unwrap().path()).unwrap();
    let free = entry("free", Limits::default());
    let priced = |name: &str, usd: f64| {
        let limits = Limits {
            usd_per_mtok_in: usd,
            usd_per_mtok_out: 2.0 * usd,
            max_output_tokens: 1_000,
            ..Default::default()
        };
        entry(name, limits)
    };
    let all = [free.clone(), priced("cheap", 1.0), priced("dear", 3.0)];
    // 10,000 tokens in, two answers of 1,000 at twice the input price, on each paid entry.
    let most = most_usd(Some(&db), &all, 10_000, 2).unwrap().unwrap();
    let each = |usd: f64| (10_000.0 * usd + 2_000.0 * 2.0 * usd) / 1e6;
    assert!((most - (each(1.0) + each(3.0))).abs() < 1e-12);
    assert_eq!(most_usd(Some(&db), &[free], 10_000, 2).unwrap(), None);
}

/// A call to paid entry `p` with its cost stored, as the chain records one.
fn paid_call(db: &Connection, p: &Provider, prompt: i64, completion: i64) {
    let usage = Usage {
        prompt: Some(prompt),
        completion: Some(completion),
        ..Default::default()
    };
    record(
        db,
        &Call {
            provider: p.name(),
            role: "curator",
            span: "s",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: None,
            usage,
            usd: cost(db, p, 0, usage, true).unwrap(),
        },
    )
    .unwrap();
}

#[test]
fn reservation_retry_uses_only_rows_contributing_to_each_accounting_window() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    // A fixed UTC clock at 2026-09-05 makes day/month boundary fixtures deterministic.
    let now = 1_788_566_400_000_i64;
    let add = |name: &str, role: &str, usd: Option<f64>, at: i64| {
        record(
            &db,
            &Call {
                provider: name,
                role,
                span: name,
                outcome: "reserved",
                ms: 0,
                detail: Some("[100,4000]"),
                bytes_out: 0,
                est_tokens: None,
                usage: Usage::default(),
                usd,
            },
        )
        .unwrap();
        db.execute(
            "UPDATE provider_calls SET ts=?2 WHERE id=?1",
            rusqlite::params![db.last_insert_rowid(), at],
        )
        .unwrap();
    };
    add(
        "old-paid",
        "curator",
        Some(2.0),
        now - 2 * providers_db::DAY_MS,
    );
    add(
        "ancient-paid",
        "curator",
        Some(9.0),
        now - 35 * providers_db::DAY_MS,
    );
    add("free", "curator", None, now);
    add("zero", "curator", Some(0.0), now);
    add("query", "query", Some(9.0), now);
    add("embed", "embed", Some(9.0), now);
    assert_eq!(
        reservation_retry_at(&db, Some("old-paid"), now).unwrap(),
        now + 1_000
    );
    assert_eq!(reservation_retry_at(&db, None, now).unwrap(), now + 600_000);
    add("daily", "curator", Some(1.0), now - 3_600_000);
    assert_eq!(
        reservation_retry_at(&db, Some("daily"), now).unwrap(),
        now + 600_000
    );
    // Only another positive paid reservation in this month restarts the short recheck.
    add("fresh-paid", "curator", Some(0.5), now - 500);
    assert_eq!(reservation_retry_at(&db, None, now).unwrap(), now + 1_000);
    assert_eq!(
        reservation_retry_at(&db, Some("daily"), now).unwrap(),
        now + 600_000
    );
}

#[test]
fn an_aged_active_sender_can_settle_and_immediately_release_a_small_probe() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let normal = entry(
        "p",
        Limits {
            daily_tokens: Some(4_200),
            max_output_tokens: 4_000,
            usd_per_mtok_in: 1.0,
            usd_per_mtok_out: 1.0,
            ..Default::default()
        },
    );
    let held = reserve(&db, &normal, "curator", "held", 100, 5.0, &[])
        .unwrap()
        .unwrap();
    db.execute(
        "UPDATE provider_calls SET ts=?1 WHERE span='held'",
        [crate::db::now_ms() - 3_600_000],
    )
    .unwrap();
    let prepared = crate::provider::probe_provider(&normal).unwrap();
    let est = crate::provider::probe_estimate(&prepared);
    let before = crate::db::now_ms();
    let refusal = reserve_with_history(&db, (&prepared, &normal), "probe", "test", est, 5.0, &[])
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(refusal.skip, Skip::Wait(at) if at>=before+600_000 && at<=crate::db::now_ms()+600_000)
    );
    let usage = Usage {
        prompt: Some(10),
        completion: Some(10),
        ..Default::default()
    };
    let usd = held.cost(&normal, usage, true);
    held.settle(
        &db,
        &Call {
            provider: "p",
            role: "curator",
            span: "held",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 100,
            est_tokens: Some(100),
            usage,
            usd,
        },
        None,
        |state| state,
    )
    .unwrap();
    let allowed = reserve_with_history(&db, (&prepared, &normal), "probe", "test", est, 5.0, &[])
        .unwrap()
        .unwrap();
    assert!((allowed.cost(&prepared, Usage::default(), true).unwrap() - 0.000231).abs() < 1e-12);
    assert_eq!(
        providers_db::state(&db, "p").unwrap(),
        providers_db::State::default()
    );
    assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, 20);
    allowed.cancel(&db).unwrap();
}

#[test]
fn reservation_pressure_backs_off_after_restart_without_refunding_it() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let mut p = entry("p", Limits::default());
    if let Provider::Openai { daily_budget, .. } = &mut p {
        *daily_budget = Some(1);
    }
    let held = reserve(&db, &p, "curator", "held", 100, 5.0, &[])
        .unwrap()
        .unwrap();
    let retry = |db: &Connection| {
        let before = crate::db::now_ms();
        let refusal = admit(db, &p, 1.0, 5.0, &[]).unwrap().unwrap();
        let Skip::Wait(until) = refusal.skip else {
            panic!("temporary pressure became a permanent hold")
        };
        (before, until)
    };
    let (before, fresh) = retry(&db);
    assert!((1_000..2_000).contains(&(fresh - before)));
    db.execute(
        "UPDATE provider_calls SET ts=?1 WHERE span='held'",
        [crate::db::now_ms() - 3_600_000],
    )
    .unwrap();
    drop(db);
    let reopened = open(home.path()).unwrap();
    let (before, older) = retry(&reopened);
    assert!(
        (600_000..601_000).contains(&(older - before)),
        "aged reservation retry delay: {}",
        older - before
    );
    assert_eq!(providers_db::calls_in_a_day(&reopened, "p").unwrap().0, 1);
    assert_eq!(
        providers_db::state(&reopened, "p").unwrap(),
        providers_db::State::default()
    );
    // Explicit proof that a sender never sent may still cancel; age alone did not refund.
    held.cancel(&reopened).unwrap();
    assert!(admit(&reopened, &p, 1.0, 5.0, &[]).unwrap().is_none());
}

#[test]
fn settled_missing_components_survive_calibration_and_same_name_parameter_changes() {
    for (usage, reported, used, usd) in [
        (
            Usage {
                prompt: Some(70),
                ..Default::default()
            },
            4_070,
            8_070,
            0.00407,
        ),
        (
            Usage {
                completion: Some(20),
                ..Default::default()
            },
            4_020,
            4_220,
            0.00022,
        ),
        (
            Usage {
                prompt: Some(70),
                completion: Some(20),
                ..Default::default()
            },
            4_090,
            4_090,
            0.00009,
        ),
        (Usage::default(), 4_000, 8_200, 0.0042),
    ] {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let mut normal = entry(
            "p",
            Limits {
                daily_tokens: Some(20_000),
                max_output_tokens: 4_000,
                usd_per_mtok_in: 1.0,
                usd_per_mtok_out: 1.0,
                ..Default::default()
            },
        );
        if let Provider::Openai { daily_budget, .. } = &mut normal {
            *daily_budget = Some(100);
        }
        // 1,000 actual tokens teach factor 2 before admission: the frozen input is 200.
        for _ in 0..5 {
            call(&db, "p", Some(100), 200, 0);
        }
        assert_eq!(factor(&db, "p").unwrap(), 2.0);
        let reservation = reserve(&db, &normal, "curator", "original", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        let price = reservation.cost(&normal, usage, true);
        reservation
            .settle(
                &db,
                &Call {
                    provider: "p",
                    role: "curator",
                    span: "original",
                    outcome: "ok",
                    ms: 1,
                    detail: Some("vetted completion"),
                    bytes_out: 100,
                    est_tokens: Some(100),
                    usage,
                    usd: price,
                },
                None,
                |_| Default::default(),
            )
            .unwrap();
        // A later 3,000 actual tokens change factor to 3. Changing model/prices/output for
        // the same name cannot re-estimate either missing component of the earlier call.
        for _ in 0..10 {
            call(&db, "p", Some(100), 300, 0);
        }
        assert_eq!(factor(&db, "p").unwrap(), 3.0);
        let mut current = crate::provider::probe_provider(&normal).unwrap();
        if let Provider::Openai { model, limits, .. } = &mut current {
            *model = "another-model".into();
            limits.usd_per_mtok_in = 9.0;
            limits.usd_per_mtok_out = 9.0;
            limits.daily_tokens = Some(used + 127);
        }
        let refusal = admit(&db, &current, 0.0, 5.0, &[])
            .unwrap()
            .expect("128 output tokens do not fit");
        assert_eq!(
            refusal.detail,
            format!("{used}/{} tokens in 24 hours", used + 127)
        );
        assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, reported);
        assert!((providers_db::usd_this_month(&db).unwrap() - usd).abs() < 1e-12);
        assert!(
            providers_db::last_calls(&db, 16)
                .unwrap()
                .iter()
                .any(|line| line == "p curator ok 1ms vetted completion")
        );
        assert!(
            providers_db::last_calls(&db, 16)
                .unwrap()
                .iter()
                .all(|line| !line.contains("oboete-budget-v1"))
        );
    }
}

#[test]
fn only_the_final_owned_bounds_count_and_invalid_stored_bounds_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let p = entry(
        "p",
        Limits {
            daily_tokens: Some(4_100),
            max_output_tokens: 4_000,
            usd_per_mtok_in: 1.0,
            ..Default::default()
        },
    );
    let reservation = reserve(&db, &p, "curator", "original", 100, 5.0, &[])
        .unwrap()
        .unwrap();
    assert_eq!(
        providers_db::last_calls(&db, 1).unwrap(),
        ["p curator reserved 0ms allowance reserved"]
    );
    let reason = "vetted failure\u{1e}oboete-budget-v1:[0,0]";
    reservation
        .settle(
            &db,
            &Call {
                provider: "p",
                role: "curator",
                span: "original",
                outcome: "error",
                ms: 1,
                detail: Some(reason),
                bytes_out: 100,
                est_tokens: Some(100),
                usage: Usage::default(),
                usd: None,
            },
            None,
            |state| state,
        )
        .unwrap();
    let small = crate::provider::probe_provider(&p).unwrap();
    assert_eq!(
        admit(&db, &small, 0.0, 5.0, &[]).unwrap().unwrap().detail,
        "4100/4100 tokens in 24 hours"
    );
    assert_eq!(
        providers_db::last_calls(&db, 1).unwrap(),
        [format!("p curator error 1ms {reason}")]
    );
    // Corrupt fixture metadata cannot silently become zero usage or today's smaller cap.
    for malformed in [
        "[-1,4000]",
        "[100,-1]",
        "[100,null]",
        "[1e999,4000]",
        "[100]",
        "[100,4000,7]",
        "[100,4000] trailing",
        "\"private-canary\"",
    ] {
        let detail =
            format!("\u{1e}oboete-call-v1:vetted failure\u{1e}oboete-budget-v1:{malformed}");
        db.execute(
            "UPDATE provider_calls SET detail=?1 WHERE span='original'",
            [detail],
        )
        .unwrap();
        let error =
            admit(&db, &small, 0.0, 5.0, &[]).expect_err("invalid bounds must refuse admission");
        assert_eq!(error.to_string(), "invalid stored token bounds");
        assert_eq!(
            providers_db::last_calls(&db, 1).unwrap(),
            ["p curator error 1ms vetted failure"]
        );
    }
}

#[test]
fn legacy_unmetered_rows_keep_the_existing_fallback_without_invented_history() {
    for detail in [
        "legacy timeout",
        "[9000,9000]",
        "claude error\u{1e}oboete-budget-v1:[0,0]",
    ] {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        record(
            &db,
            &Call {
                provider: "p",
                role: "curator",
                span: "legacy",
                outcome: "error",
                ms: 1,
                detail: Some(detail),
                bytes_out: 100,
                est_tokens: Some(100),
                usage: Usage::default(),
                usd: None,
            },
        )
        .unwrap();
        let p = entry(
            "p",
            Limits {
                daily_tokens: Some(300),
                max_output_tokens: 128,
                usd_per_mtok_in: 1.0,
                ..Default::default()
            },
        );
        let refusal = admit(&db, &p, 1.0, 5.0, &[]).unwrap().unwrap();
        assert_eq!(refusal.detail, "228/300 tokens in 24 hours");
        assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, 0);
        assert_eq!(
            providers_db::last_calls(&db, 1).unwrap(),
            [format!("p curator error 1ms {detail}")]
        );
    }
}

#[test]
fn a_small_probe_cannot_shrink_a_previous_unmetered_curation_call() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let normal = entry(
        "p",
        Limits {
            daily_tokens: Some(4_100),
            max_output_tokens: 4_000,
            usd_per_mtok_in: 1.0,
            usd_per_mtok_out: 1.0,
            ..Default::default()
        },
    );
    let call = reserve(&db, &normal, "curator", "normal", 100, 5.0, &[])
        .unwrap()
        .unwrap();
    let usd = call.cost(&normal, Usage::default(), true);
    call.settle(
        &db,
        &Call {
            provider: "p",
            role: "curator",
            span: "normal",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 100,
            est_tokens: Some(100),
            usage: Usage::default(),
            usd,
        },
        None,
        |_| Default::default(),
    )
    .unwrap();
    let probe = crate::provider::probe_provider(&normal).unwrap();
    let admission = reserve(
        &db,
        &probe,
        "probe",
        "settings",
        crate::provider::probe_estimate(&probe),
        5.0,
        &[],
    )
    .unwrap();
    let refusal = admission.expect_err("the prior call still takes its full 100 + 4000 allowance");
    assert_eq!(refusal.detail, "4100/4100 tokens in 24 hours");
    assert!(matches!(refusal.skip, Skip::Budget(_)));
    assert_eq!(providers_db::tokens_since(&db, "p", 0).unwrap().0, 0);
    assert!(providers_db::token_ratios(&db, "p", 50).unwrap().is_empty());
    assert_eq!(
        providers_db::last_calls(&db, 1).unwrap(),
        ["p curator ok 1ms "]
    );
}

/// A rest codex's allowance read set without credits gives way to a call that drew on them,
/// timed or held; a call of any other entry of the name, which may have been reserved before
/// the rest was written, keeps it, as it keeps any other rest and an owner hold, but never in
/// place of a credited rest as long that the call brings (Codex on #418).
#[test]
fn an_uncredited_rest_gives_way_only_to_a_call_that_drew_on_credits() {
    use providers_db::{OWNER_HOLD, State};
    let later = crate::db::now_ms() + 60_000;
    let rest = |down_until, uncredited| State {
        down_until,
        uncredited,
        ..Default::default()
    };
    let credited = rest(later + 60_000, false);
    let codex = Provider::Cli {
        credits: true,
        enabled: true,
        name: "p".into(),
        cli: "codex".into(),
        model: None,
        daily_budget: 10,
        timeout_s: 1,
        limits: Limits::default(),
    };
    let other = entry("p", Limits::default());
    // (the entry that settles, the stored rest, what the call leaves, the state after)
    for (p, stored, left, after) in [
        (
            &codex,
            rest(OWNER_HOLD, true),
            State::default(),
            State::default(),
        ),
        (
            &codex,
            rest(later, true),
            State::default(),
            State::default(),
        ),
        (&codex, rest(OWNER_HOLD, true), credited, credited),
        (
            &other,
            rest(OWNER_HOLD, true),
            State::default(),
            rest(OWNER_HOLD, true),
        ),
        (
            &other,
            rest(later, true),
            State::default(),
            rest(later, true),
        ),
        (
            &codex,
            rest(later, false),
            State::default(),
            rest(later, false),
        ),
        (&codex, credited, rest(later, true), credited),
        (
            &codex,
            rest(OWNER_HOLD, false),
            credited,
            rest(OWNER_HOLD, false),
        ),
        // A safety hold this call brings outranks an uncredited hold kept for it, and a
        // credited rest as long as an uncredited one is the one kept, whichever was first.
        (
            &other,
            rest(OWNER_HOLD, true),
            rest(OWNER_HOLD, false),
            rest(OWNER_HOLD, false),
        ),
        (
            &other,
            rest(later, true),
            rest(later, false),
            rest(later, false),
        ),
        (
            &other,
            rest(later, false),
            rest(later, true),
            rest(later, false),
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let reservation = reserve(&db, p, "curator", "s", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        providers_db::set_state(&db, "p", stored).unwrap();
        let call = Call {
            provider: "p",
            role: "curator",
            span: "s",
            outcome: "ok",
            ms: 1,
            detail: None,
            bytes_out: 1,
            est_tokens: Some(100),
            usage: Usage::default(),
            usd: None,
        };
        let state = reservation.settle(&db, &call, None, |_| left).unwrap();
        let case = format!("{} {stored:?}", p.name());
        assert_eq!(state, after, "{case}");
        assert_eq!(providers_db::state(&db, "p").unwrap(), after, "{case}");
    }
}

#[test]
fn settlement_keeps_the_admission_month() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let p = entry(
        "p",
        Limits {
            max_output_tokens: 100,
            usd_per_mtok_in: 4.5,
            usd_per_mtok_out: 4.5,
            ..Default::default()
        },
    );
    let earlier = reserve(&db, &p, "curator", "earlier", 100, 0.001, &[])
        .unwrap()
        .unwrap();
    // Age a fixture's admission across the calendar boundary; settlement must not move
    // spending into a pool where other work has already reserved the remaining allowance.
    db.execute("UPDATE provider_calls SET ts=0 WHERE span='earlier'", [])
        .unwrap();
    let _this_month = reserve(&db, &p, "probe", "current", 100, 0.001, &[])
        .unwrap()
        .unwrap();
    let usd = earlier.cost(&p, Usage::default(), true);
    earlier
        .settle(
            &db,
            &Call {
                provider: "p",
                role: "curator",
                span: "earlier",
                outcome: "ok",
                ms: 1,
                detail: None,
                bytes_out: 1,
                est_tokens: Some(100),
                usage: Usage::default(),
                usd,
            },
            None,
            |_| Default::default(),
        )
        .unwrap();
    assert!((providers_db::usd_this_month(&db).unwrap() - 0.0009).abs() < 1e-12);
}

#[test]
fn a_late_response_cannot_replenish_a_live_rate_window() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let peer = open(home.path()).unwrap();
    let p = entry("p", Limits::default());
    let first = reserve(&db, &p, "curator", "s", 100, 5.0, &[])
        .unwrap()
        .unwrap();
    let second = reserve(&peer, &p, "probe", "s", 100, 5.0, &[])
        .unwrap()
        .unwrap();
    let until = crate::db::now_ms() + 60_000;
    let actual = providers_db::RateLeft {
        requests: Some(0),
        requests_reset_at: Some(until),
        tokens: Some(0),
        tokens_reset_at: Some(until),
    };
    let stale = providers_db::RateLeft {
        requests: Some(5),
        tokens: Some(10_000),
        ..actual
    };
    for (reservation, role, rate) in [(second, "probe", actual), (first, "curator", stale)] {
        reservation
            .settle(
                &db,
                &Call {
                    provider: "p",
                    role,
                    span: "s",
                    outcome: "ok",
                    ms: 1,
                    detail: None,
                    bytes_out: 1,
                    est_tokens: Some(100),
                    usage: Usage::default(),
                    usd: None,
                },
                Some(rate),
                |_| Default::default(),
            )
            .unwrap();
    }
    let refused = reserve(&peer, &p, "probe", "next", 100, 5.0, &[]).unwrap();
    assert!(
        matches!(refused, Err(Refusal { skip: Skip::Wait(t), .. }) if t == until),
        "{refused:?}"
    );
    assert_eq!(providers_db::rate(&peer, "p").unwrap(), actual);
}

#[test]
fn reservations_share_the_reported_rate_allowance() {
    for rate in [
        providers_db::RateLeft {
            requests: Some(1),
            requests_reset_at: Some(crate::db::now_ms() + 60_000),
            ..Default::default()
        },
        providers_db::RateLeft {
            tokens: Some(1_500),
            tokens_reset_at: Some(crate::db::now_ms() + 60_000),
            ..Default::default()
        },
    ] {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let peer = open(home.path()).unwrap();
        let p = entry("p", Limits::default());
        providers_db::set_rate(&db, "p", rate).unwrap();
        let _inflight = reserve(&db, &p, "curator", "first", 100, 5.0, &[])
            .unwrap()
            .unwrap();
        let refusal = reserve(&peer, &p, "probe", "second", 100, 5.0, &[]).unwrap();
        assert!(
            matches!(
                refusal,
                Err(Refusal {
                    skip: Skip::Wait(_),
                    ..
                })
            ),
            "{refusal:?}"
        );
        assert_eq!(providers_db::tokens_since(&peer, "p", 0).unwrap().0, 0);
        assert_eq!(factor(&peer, "p").unwrap(), 1.0);
    }
}

#[test]
fn concurrent_reservations_share_daily_calls_tokens_and_the_curation_month() {
    for limit in ["calls", "tokens", "month"] {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let peer = open(home.path()).unwrap();
        let mut p = entry(
            "p",
            Limits {
                daily_tokens: (limit == "tokens").then_some(350),
                max_output_tokens: 100,
                usd_per_mtok_in: 1.0,
                usd_per_mtok_out: 1.0,
                ..Default::default()
            },
        );
        if limit == "calls"
            && let Provider::Openai { daily_budget, .. } = &mut p
        {
            *daily_budget = Some(1);
        }
        let mut other = p.clone();
        if limit == "month"
            && let Provider::Openai { name, .. } = &mut other
        {
            *name = "other".into();
        }
        let cap = if limit == "month" { 0.0003 } else { 5.0 };
        let barrier = std::sync::Barrier::new(2);
        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                let db = db;
                barrier.wait();
                reserve(&db, &p, "curator", "worker", 100, cap, &[]).unwrap()
            });
            let second = scope.spawn(|| {
                let peer = peer;
                barrier.wait();
                reserve(&peer, &other, "probe", "settings", 100, cap, &[]).unwrap()
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        let results = [first, second];
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{limit}");
        assert!(
            results.iter().any(|r| matches!(
                r,
                Err(Refusal {
                    skip: Skip::Wait(_),
                    ..
                })
            )),
            "{limit}"
        );
        // Let the token go without settling, as when a caller crashes. Reopening does not
        // forgive it, and hypothetical tokens cannot alter calibration or reported usage.
        drop(results);
        let reopened = open(home.path()).unwrap();
        assert!((providers_db::usd_this_month(&reopened).unwrap() - 0.0002).abs() < 1e-12);
        assert_eq!(providers_db::embed_usd_this_month(&reopened).unwrap(), 0.0);
        assert_eq!(providers_db::tokens_since(&reopened, "p", 0).unwrap().0, 0);
        assert!(
            providers_db::token_ratios(&reopened, "p", 50)
                .unwrap()
                .is_empty()
        );
        let refusal = reserve(&reopened, &p, "curator", "after-crash", 100, cap, &[]).unwrap();
        assert!(
            matches!(
                refusal,
                Err(Refusal {
                    skip: Skip::Wait(_),
                    ..
                })
            ),
            "{limit}: {refusal:?}"
        );
    }
}

#[test]
fn settlement_releases_unbilled_cost_and_keeps_actual_or_missing_usage_conservative() {
    for (sent, billed, usage, expected_usd, expected_tokens) in [
        (false, false, Usage::default(), 0.0, 0),
        (true, false, Usage::default(), 0.0, 0),
        (true, true, Usage::default(), 0.0002, 0),
        (
            true,
            true,
            Usage {
                prompt: Some(20),
                completion: Some(10),
                ..Default::default()
            },
            0.00003,
            30,
        ),
        // Codex on #409: of the 20 prompt tokens, 10 written to the cache count twice.
        (
            true,
            true,
            Usage {
                prompt: Some(20),
                completion: Some(10),
                written: Some(10),
                ..Default::default()
            },
            0.00004,
            30,
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let peer = open(home.path()).unwrap();
        let p = entry(
            "p",
            Limits {
                max_output_tokens: 100,
                usd_per_mtok_in: 1.0,
                usd_per_mtok_out: 1.0,
                ..Default::default()
            },
        );
        let reservation = reserve(&db, &p, "probe", "s", 100, 0.0003, &[])
            .unwrap()
            .unwrap();
        let usd = reservation.cost(&p, usage, billed);
        let hold = providers_db::State {
            down_until: providers_db::OWNER_HOLD,
            fails: 2,
            backoff: 3,
            ..Default::default()
        };
        providers_db::set_state(&peer, "p", hold).unwrap();
        let state = reservation
            .settle(
                &db,
                &Call {
                    provider: "p",
                    role: "probe",
                    span: "s",
                    outcome: "error",
                    ms: 1,
                    detail: Some(if billed {
                        "http request: timeout: global"
                    } else {
                        "http 400"
                    }),
                    bytes_out: usize::from(sent),
                    est_tokens: Some(100),
                    usage,
                    usd,
                },
                None,
                |_| Default::default(),
            )
            .unwrap();
        assert_eq!(state, hold);
        assert_eq!(providers_db::state(&peer, "p").unwrap(), hold);
        assert_eq!(providers_db::calls_in_a_day(&peer, "p").unwrap().0, 1);
        assert!((providers_db::usd_this_month(&peer).unwrap() - expected_usd).abs() < 1e-12);
        assert_eq!(
            providers_db::tokens_since(&peer, "p", 0).unwrap().0,
            expected_tokens
        );
        assert_eq!(
            providers_db::reserved_since(&peer, "p", 0).unwrap().calls,
            0
        );
        assert!(
            reserve(&peer, &p, "probe", "held", 100, 5.0, &[])
                .unwrap()
                .is_err()
        );
    }
}

/// A Messages entry's call is counted as billed: of its reported prompt, the tokens its cache
/// read at a tenth of the input price and those it wrote at 1.25 times.
#[test]
fn a_messages_calls_cache_is_counted_as_billed() {
    let limits = Limits {
        max_output_tokens: 100,
        usd_per_mtok_in: 1.0,
        usd_per_mtok_out: 1.0,
        ..Default::default()
    };
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let mut p = entry("p", limits);
    if let Provider::Openai { api, .. } = &mut p {
        *api = crate::config::Api::Anthropic;
    }
    let reservation = reserve(&db, &p, "curator", "s", 10_000, 5.0, &[])
        .unwrap()
        .unwrap();
    let usage = Usage {
        prompt: Some(10_000),
        completion: Some(100),
        cached: Some(4_000),
        written: Some(1_000),
        ..Default::default()
    };
    // 5,000 fresh, 1,000 written at 1.25, 4,000 read at 0.1, and 100 out: USD 0.00675.
    let usd = reservation.cost(&p, usage, true).unwrap();
    assert!((usd - 0.00675).abs() < 1e-12, "{usd}");
}

/// Decision 80: an entry of OpenAI's own API, its host in any case and its port written or not,
/// is counted as billed: of its reported prompt, the tokens written to its cache at 1.25 times
/// the input price and those read at a tenth; wherever its usage is not known (admission, the
/// reservation, a call whose usage never comes back, recuration's estimate) its input at 1.25
/// times, so a request at the edge of the month's cap is refused rather than overspent.
#[test]
fn an_openai_calls_cache_writes_are_counted_as_billed() {
    for url in [
        "https://api.openai.com/v1",
        "https://API.OpenAI.com:443/v1/",
        " https://api.openai.com/v1",
    ] {
        assert!(crate::config::openai_api(url), "{url}");
    }
    for url in [
        "https://api.openai.com.example.net/v1",
        "https://openrouter.ai/api/v1",
        "not a url",
    ] {
        assert!(!crate::config::openai_api(url), "{url}");
    }
    let limits = Limits {
        max_output_tokens: 100,
        usd_per_mtok_in: 1.0,
        usd_per_mtok_out: 1.0,
        ..Default::default()
    };
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let mut p = entry("p", limits);
    if let Provider::Openai {
        base_url, model, ..
    } = &mut p
    {
        *base_url = "https://api.openai.com:443/v1".into();
        *model = "gpt-6-luna".into();
    }
    assert_eq!(p.input_weight("curator"), 1.25);
    // 60,000 tokens in and 100 out at USD 1 a million each: USD 0.0601 at the input price,
    // 0.0751 at 1.25 times, so a cap of USD 0.07 refuses it.
    assert!(
        admit_with_history(&db, (&p, &p), "curator", 60_000.0, 0.07, &[])
            .unwrap()
            .is_some()
    );
    let reservation = reserve(&db, &p, "curator", "s", 60_000, 5.0, &[])
        .unwrap()
        .unwrap();
    let priced = [
        providers_db::reserved_usd_this_month(&db).unwrap(),
        reservation.cost(&p, Usage::default(), true).unwrap(),
        most_usd(None, std::slice::from_ref(&p), 60_000, 1)
            .unwrap()
            .unwrap(),
    ];
    assert!(
        priced.iter().all(|u| (u - 0.0751).abs() < 1e-12),
        "{priced:?}"
    );
    let usage = Usage {
        prompt: Some(10_000),
        completion: Some(100),
        cached: Some(2_000),
        written: Some(3_000),
        ..Default::default()
    };
    // 5,000 fresh, 3,000 written at 1.25, 2,000 read at 0.1, and 100 out: USD 0.00905.
    let usd = reservation.cost(&p, usage, true).unwrap();
    assert!((usd - 0.00905).abs() < 1e-12, "{usd}");
    // A read as its model's (#861, #863): a twentieth on GPT-6.1 Sol, a Sol snapshot and a
    // GPT-6.1 Terra snapshot, the full input price on an older model, as on a name only like
    // GPT-6's.
    for (name, want) in [
        ("gpt-6.1-sol", 0.00895),
        ("gpt-6.1-sol-2026-10-01", 0.00895),
        ("gpt-6.1-terra-2026-10-01", 0.00895),
        ("gpt-4.1", 0.01085),
        ("gpt-60", 0.01085),
    ] {
        if let Provider::Openai { model, .. } = &mut p {
            *model = name.into();
        }
        let usd = reservation.cost(&p, usage, true).unwrap();
        assert!((usd - want).abs() < 1e-12, "{name}: {usd}");
    }
}

/// Codex on #409: a Messages entry prices each input token at what a cache write may bill,
/// 1.25 times its input price for a fenced prompt's instructions and twice with a cache of its
/// own (`cache_control`) or for a summary's or My Work's call (decision 86), wherever its usage
/// is not known: admission, the reservation, a call whose usage never comes back, recuration's
/// estimate; and a write its usage reports at the same rate. Its tokens count once: a prompt
/// that fits its ceiling is not refused for its price.
#[test]
fn a_cache_writing_entry_prices_its_input_twice_and_counts_its_tokens_once() {
    let limits = Limits {
        max_output_tokens: 100,
        max_request_tokens: Some(100_000),
        usd_per_mtok_in: 1.0,
        usd_per_mtok_out: 1.0,
        ..Default::default()
    };
    // A Messages entry's input at 1.25 times its price, the write of a fenced prompt's
    // instructions to its cache; twice with a cache of its own, and for a summary's or My
    // Work's call, whose marker keeps its entry for an hour (decision 86). Recuration's
    // estimate is the curator's.
    let roles = [
        (false, "curator", 0.0751),
        (true, "curator", 0.1201),
        (false, "summary", 0.1201),
        (false, crate::my_work::ROLE, 0.1201),
    ];
    for (cache, role, usd) in roles {
        let home = tempfile::tempdir().unwrap();
        let db = open(home.path()).unwrap();
        let mut p = entry("p", limits.clone());
        if let Provider::Openai { api, extra, .. } = &mut p {
            *api = crate::config::Api::Anthropic;
            if cache {
                extra.insert(
                    "cache_control".into(),
                    serde_json::json!({"type": "ephemeral"}),
                );
            }
        }
        // 60,000 input tokens and 100 output tokens at USD 1 per million each: within USD 0.10
        // a month at 1.25 times, not twice.
        let refused = admit_with_history(&db, (&p, &p), role, 60_000.0, 0.1, &[]).unwrap();
        assert_eq!(refused.is_some(), usd > 0.1, "{role} cache {cache}");
        // The reservation is admitted at its role's price too.
        let capped = reserve(&db, &p, role, "s", 60_000, 0.1, &[]).unwrap();
        assert_eq!(capped.is_err(), usd > 0.1, "{role} cache {cache}");
        let reservation = match capped {
            Ok(reservation) => reservation,
            Err(_) => reserve(&db, &p, role, "s", 60_000, 5.0, &[])
                .unwrap()
                .unwrap(),
        };
        let pending = providers_db::reserved_since(&db, "p", 0).unwrap();
        assert_eq!(pending.tokens, 60_100.0, "{role} cache {cache}");
        let priced = [
            providers_db::reserved_usd_this_month(&db).unwrap(),
            reservation.cost(&p, Usage::default(), true).unwrap(),
        ];
        assert!(
            priced.iter().all(|u| (u - usd).abs() < 1e-12),
            "{role} cache {cache}: {priced:?}"
        );
        let most = most_usd(None, std::slice::from_ref(&p), 60_000, 1)
            .unwrap()
            .unwrap();
        let curator = if cache { 0.1201 } else { 0.0751 };
        assert!(
            (most - curator).abs() < 1e-12,
            "{role} cache {cache}: {most}"
        );
        // A write the usage reports is priced as the role's marker writes it.
        let reported = Usage {
            prompt: Some(10_000),
            completion: Some(0),
            written: Some(4_000),
            ..Default::default()
        };
        let write = if usd > 0.1 { 2.0 } else { 1.25 };
        let billed = reservation.cost(&p, reported, true).unwrap();
        assert!(
            (billed - (6_000.0 + 4_000.0 * write) / 1e6).abs() < 1e-12,
            "{role} cache {cache}: {billed}"
        );
    }
}

#[test]
fn the_estimate_counts_cjk_and_other_characters_apart() {
    assert_eq!(estimate(""), 0);
    assert_eq!(estimate("日本語"), 3); // 2.4, rounded up
    assert_eq!(estimate("abcd"), 2); // 1.12
    // 12,000 Japanese-heavy characters come out near what Groq counted for them (6,500).
    let ja = "日付の列が dd.mm.yyyy 形式の行が落ちている。".repeat(400);
    assert!((6_000..8_000).contains(&estimate(&ja)), "{}", estimate(&ja));
}

#[test]
fn the_factor_follows_recorded_usage() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    for _ in 0..4 {
        call(&db, "p", Some(100), 150, 1);
    }
    assert_eq!(factor(&db, "p").unwrap(), 1.0); // fewer than 5
    call(&db, "p", Some(100), 120, 1);
    call(&db, "p", None, 999, 1); // no estimate: not a sample
    assert_eq!(factor(&db, "p").unwrap(), 1.5);
}

#[test]
fn a_request_over_the_ceiling_or_after_a_peer_refused_it_is_not_admitted() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let groq = entry(
        "groq",
        Limits {
            max_request_tokens: Some(8000),
            ..Default::default()
        },
    );
    assert!(admit(&db, &groq, 6700.0, 5.0, &[]).unwrap().is_none());
    // With no output declared, its largest answer is reserved: 7,000 and 1,250 are over 8,000.
    let r = admit(&db, &groq, 7000.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.outcome, "too_big");
    let r = admit(&db, &groq, 7700.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.outcome, "too_big"); // over 95% of the ceiling
    let r = admit(&db, &groq, 100.0, 5.0, &[4000, 8000])
        .unwrap()
        .unwrap();
    assert_eq!(r.outcome, "too_big");
    assert!(admit(&db, &groq, 100.0, 5.0, &[4000]).unwrap().is_none());
    // A declared output is reserved too: 7,000 in and 4,000 out do not fit in 8,000.
    let mut declared = groq.clone();
    if let Provider::Openai { extra, .. } = &mut declared {
        extra.insert("max_completion_tokens".into(), 4000.into());
    }
    let r = admit(&db, &declared, 7000.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.outcome, "too_big");
}

#[test]
fn daily_tokens_are_a_budget() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let p = entry(
        "p",
        Limits {
            daily_tokens: Some(10_000),
            ..Default::default()
        },
    );
    call(&db, "p", None, 7000, 1000);
    // With no output declared, 1,250 are reserved: 700 in fits in the 2,000 left, 800 does not.
    assert!(admit(&db, &p, 700.0, 5.0, &[]).unwrap().is_none());
    let r = admit(&db, &p, 800.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(
        (r.outcome, r.detail.as_str()),
        ("budget", "8000/10000 tokens in 24 hours")
    );
    // The declared output is reserved: 1,000 in and up to 4,000 out do not fit in 2,000.
    let mut declared = p.clone();
    if let Provider::Openai { extra, .. } = &mut declared {
        extra.insert("max_tokens".into(), 4000.into());
    }
    let r = admit(&db, &declared, 1000.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.outcome, "budget");
    // A sent call with no usage back counts at its estimate and its largest output: 1,000
    // in and the entry's 4,000 out take the day to 13,000.
    record(
        &db,
        &Call {
            provider: "p",
            role: "curator",
            span: "s",
            outcome: "error",
            ms: 1,
            detail: Some("http request: timeout: global"),
            bytes_out: 1,
            est_tokens: Some(1_000),
            usage: Usage::default(),
            usd: None,
        },
    )
    .unwrap();
    let r = admit(&db, &p, 1.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.detail, "13000/10000 tokens in 24 hours");
}

/// Groq's day is a rolling window: a call 23 hours ago still counts, one 25 hours ago does
/// not, and a refused call waits until the oldest counted one leaves the 24 hours.
#[test]
fn daily_tokens_are_counted_over_the_last_24_hours() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let p = entry(
        "p",
        Limits {
            daily_tokens: Some(10_000),
            ..Default::default()
        },
    );
    let (now, hour) = (crate::db::now_ms(), 3_600_000);
    let aged = |ms: i64| {
        db.execute(
            "UPDATE provider_calls SET ts = ?1 WHERE id = (SELECT MAX(id) FROM provider_calls)",
            [now - ms],
        )
        .unwrap();
    };
    for ago in [25, 23] {
        call(&db, "p", None, 5000, 1000);
        aged(ago * hour);
    }
    // A refusal and a 429 older than the counted call used no token: their age frees none.
    refusal(&db, "p");
    aged(23 * hour + hour / 2);
    record(
        &db,
        &Call {
            provider: "p",
            role: "curator",
            span: "s",
            outcome: "wait",
            ms: 1,
            detail: Some("http 429: slow down"),
            bytes_out: 1,
            est_tokens: Some(900),
            usage: Usage::default(),
            usd: None,
        },
    )
    .unwrap();
    aged(23 * hour + hour / 4);
    // 6,000 in the 24 hours: 2,700 in and 1,250 reserved fit in the 4,000 left, 2,800 do not.
    assert!(admit(&db, &p, 2700.0, 5.0, &[]).unwrap().is_none());
    let r = admit(&db, &p, 2800.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.detail, "6000/10000 tokens in 24 hours");
    let Skip::Budget(until) = r.skip else {
        panic!("{:?}", r.skip)
    };
    assert_eq!(until, now - 23 * hour + providers_db::DAY_MS + 1);
}

/// The call budget counts a rolling day too, and waits until its oldest call leaves it.
/// #238: an OpenRouter free entry with no budget of the owner's takes a fifth of the limit its
/// key's last read gave, and 10 until a read gives one; admit counts calls against that, and
/// doctor says which it is. The owner's own budget holds whatever the key says, and a read of
/// a key the entry no longer holds is none.
#[test]
fn a_key_budget_is_a_fifth_of_what_its_last_read_gave() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let key_file = home.path().join("KEY.md");
    std::fs::write(&key_file, "# a test key\nkey-a\n").unwrap();
    let free = format!(
        "kind = \"openai\"\nbase_url = \"https://openrouter.ai/api/v1\"\nmodel = \"m:free\"\n\
             key_file = {key_file:?}\n"
    );
    let p: Provider = toml::from_str(&format!("name = \"o\"\n{free}")).unwrap();
    let k = crate::provider::key_of(&p);
    let k = k.as_str();
    let said = |db: Option<&Connection>| key_budget(db, &p).unwrap();
    assert_eq!(daily(&db, &p).unwrap(), 10);
    assert_eq!(
        said(None),
        "10 calls a day until its key's own limit is read"
    );
    assert_eq!(said(Some(&db)), said(None));
    let read_at = crate::db::now_ms();
    providers_db::set_key_limit(&db, "o", Some(1000), read_at, k).unwrap();
    assert_eq!(daily(&db, &p).unwrap(), 200);
    assert_eq!(
        said(Some(&db)),
        format!(
            "200 calls a day, a fifth of the 1000 free-model requests a day its key has (read {})",
            crate::db::utc(read_at)
        )
    );
    // A read past due: doctor runs no refresh, and says the next run takes it again.
    providers_db::set_key_limit(&db, "o", Some(1000), 1, k).unwrap();
    assert_eq!(
        said(Some(&db)),
        format!(
            "200 calls a day, a fifth of the 1000 free-model requests a day its key has (read {}); \
                 the next curation run reads it again",
            crate::db::utc(1)
        )
    );
    // doctor reads without the chain's refresh: a read of the key the file held before is
    // not this key's.
    std::fs::write(&key_file, "# a test key\nkey-b\n").unwrap();
    assert_eq!(daily(&db, &p).unwrap(), 10);
    assert_eq!(
        said(Some(&db)),
        "10 calls a day until its key's own limit is read"
    );
    std::fs::write(&key_file, "# a test key\nkey-a\n").unwrap();
    assert_eq!(daily(&db, &p).unwrap(), 200);
    let own: Provider =
        toml::from_str(&format!("name = \"o\"\n{free}daily_budget = 30\n")).unwrap();
    assert_eq!(daily(&db, &own).unwrap(), 30);
    providers_db::set_key_limit(&db, "o", None, read_at, k).unwrap();
    assert_eq!(daily(&db, &p).unwrap(), 10);
    assert_eq!(
        said(Some(&db)),
        format!(
            "10 calls a day: the read of its key's own limit at {} gave none",
            crate::db::utc(read_at)
        )
    );
    providers_db::set_key_limit(&db, "o", Some(0), 3, k).unwrap();
    assert_eq!(daily(&db, &p).unwrap(), 0);
    assert!(admit(&db, &p, 10.0, 5.0, &[]).unwrap().is_some());
    // A refusal lasts until the next read of the key's limit is due, if that comes before the
    // oldest call leaves the day: the read may raise the budget (Codex on 53b7277). A read
    // that gave a limit is due after a day, one that gave none after an hour.
    let now = crate::db::now_ms();
    let hour = 3_600_000;
    call(&db, "o", None, 10, 10);
    let refused = |read_at: i64, limit: Option<u32>| {
        providers_db::set_key_limit(&db, "o", limit, read_at, k).unwrap();
        let r = admit(&db, &p, 10.0, 5.0, &[]).unwrap().unwrap();
        let Skip::Budget(until) = r.skip else {
            panic!("{:?}", r.skip)
        };
        (r.detail, until)
    };
    let (detail, day) = refused(now, Some(5));
    assert_eq!(detail, "1/1 calls in 24 hours");
    assert!(day > now + 23 * hour, "{day}");
    let at = now - 20 * hour;
    assert_eq!(refused(at, Some(5)).1, at + KEY_READ_HOLDS_MS);
    (0..9).for_each(|_| call(&db, "o", None, 10, 10));
    let (detail, until) = refused(now, None);
    assert_eq!(detail, "10/10 calls in 24 hours");
    assert_eq!(until, now + KEY_READ_RETRY_MS);
    assert!(until < day);
}

#[test]
fn daily_calls_are_counted_over_the_last_24_hours() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let mut p = entry("p", Limits::default());
    if let Provider::Openai { daily_budget, .. } = &mut p {
        *daily_budget = Some(2);
    }
    let (now, hour) = (crate::db::now_ms(), 3_600_000);
    for ago in [25, 23] {
        call(&db, "p", None, 10, 10);
        db.execute(
            "UPDATE provider_calls SET ts = ?1 WHERE id = (SELECT MAX(id) FROM provider_calls)",
            [now - ago * hour],
        )
        .unwrap();
    }
    assert!(admit(&db, &p, 10.0, 5.0, &[]).unwrap().is_none());
    call(&db, "p", None, 10, 10);
    let r = admit(&db, &p, 10.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.detail, "2/2 calls in 24 hours");
    let Skip::Budget(until) = r.skip else {
        panic!("{:?}", r.skip)
    };
    assert_eq!(until, now - 23 * hour + providers_db::DAY_MS + 1);
}

#[test]
fn a_call_whose_largest_output_would_cross_the_cap_is_not_admitted() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let paid = |name: &str| {
        entry(
            name,
            Limits {
                usd_per_mtok_in: 1.0,
                usd_per_mtok_out: 10.0,
                max_output_tokens: 4000,
                ..Default::default()
            },
        )
    };
    let chain = [paid("a"), paid("b"), entry("free", Limits::default())];
    // USD 4.63 spent across both paid entries; the free one does not count.
    paid_call(&db, &chain[0], 1_000_000, 300_000);
    paid_call(&db, &chain[1], 630_000, 0);
    paid_call(&db, &chain[2], 50_000_000, 5_000_000);
    let spent = providers_db::usd_this_month(&db).unwrap();
    assert!((spent - 4.63).abs() < 1e-9, "{spent}");
    // 10,000 in (0.01) and up to 4,000 out (0.04): 4.68, inside 5.
    assert!(admit(&db, &chain[0], 10_000.0, 5.0, &[]).unwrap().is_none());
    // Just below the cap, the same call could cross it by its answer alone.
    paid_call(&db, &chain[1], 0, 33_000);
    let r = admit(&db, &chain[1], 10_000.0, 5.0, &[]).unwrap().unwrap();
    assert_eq!(r.outcome, "budget");
    assert!(admit(&db, &chain[2], 10_000.0, 5.0, &[]).unwrap().is_none());
    // An entry that asks for 100 output tokens is priced at 100 (0.011 in all): it fits.
    let mut small = chain[1].clone();
    if let Provider::Openai { extra, .. } = &mut small {
        extra.insert("max_completion_tokens".into(), 100.into());
    }
    assert!(admit(&db, &small, 10_000.0, 5.0, &[]).unwrap().is_none());
}

#[test]
fn a_sent_call_that_reported_no_usage_is_charged_its_largest_cost_and_keeps_it() {
    let home = tempfile::tempdir().unwrap();
    let db = open(home.path()).unwrap();
    let paid = entry(
        "a",
        Limits {
            usd_per_mtok_in: 1.0,
            usd_per_mtok_out: 10.0,
            max_output_tokens: 4000,
            ..Default::default()
        },
    );
    let none = Usage::default();
    let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-9);
    // A timeout after sending: 1,000 in (0.001) and up to 4,000 out (0.04).
    let timeout = cost(&db, &paid, 1_000, none, true).unwrap();
    assert!(close(timeout, 0.041), "{timeout:?}");
    // An answer with an HTTP error status was not billed.
    assert_eq!(cost(&db, &paid, 1_000, none, false).unwrap(), None);
    // A prompt count only: 500 in (0.0005) and the most out (0.04).
    let partial = Usage {
        prompt: Some(500),
        ..Default::default()
    };
    assert!(close(
        cost(&db, &paid, 1_000, partial, true).unwrap(),
        0.0405
    ));
    // A free entry has no cost.
    let free = entry("f", Limits::default());
    assert_eq!(cost(&db, &free, 1_000, none, true).unwrap(), None);
    // The timeout's cost is stored with it and stays when the entry is removed, repriced or
    // calibrated later: five samples at twice the estimate raise a new call's input to 2,000,
    // not the stored one's.
    record(
        &db,
        &Call {
            provider: "a",
            role: "curator",
            span: "s",
            outcome: "error",
            ms: 1,
            detail: Some("http request: timeout: global"),
            bytes_out: 1,
            est_tokens: Some(1_000),
            usage: none,
            usd: timeout,
        },
    )
    .unwrap();
    for _ in 0..5 {
        call(&db, "a", Some(100), 200, 0);
    }
    assert!(close(cost(&db, &paid, 1_000, none, true).unwrap(), 0.042));
    assert!(close(
        Some(providers_db::usd_this_month(&db).unwrap()),
        0.041
    ));
}
