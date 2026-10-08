//! Cached provider observations; never binds a key, reserves a call or refreshes a provider.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use super::{CallSummary, CheckState, Count, RecentCalls, call_outcome, error_state, has_columns};
use crate::providers_db as pdb;

#[derive(Serialize)]
struct Amount {
    state: CheckState,
    value: Option<f64>,
}

impl Amount {
    fn unknown(state: CheckState) -> Self {
        Self { state, value: None }
    }
}

fn read<T>(
    conn: &Connection,
    table: &str,
    columns: &[&str],
    query: impl FnOnce() -> Result<T>,
) -> std::result::Result<T, CheckState> {
    if !has_columns(conn, table, columns).map_err(|error| error_state(&error))? {
        return Err(CheckState::SchemaMissing);
    }
    query().map_err(|error| error_state(&error))
}

fn amount(result: std::result::Result<f64, CheckState>) -> Amount {
    match result {
        Ok(value) if value.is_finite() && value >= 0.0 => Amount {
            state: CheckState::Known,
            value: Some(value),
        },
        Ok(_) => Amount::unknown(CheckState::Unavailable),
        Err(state) => Amount::unknown(state),
    }
}

fn measured(result: std::result::Result<i64, CheckState>) -> Count {
    match result {
        Ok(value) if value >= 0 => Count {
            state: CheckState::Known,
            value: Some(value),
        },
        Ok(_) => Count::unknown(CheckState::Unavailable),
        Err(state) => Count::unknown(state),
    }
}

#[derive(Serialize)]
struct Spend {
    curation_usd: Amount,
    embed_query_usd: Amount,
    reserved_usd: Amount,
}

#[derive(Serialize)]
struct Stopped {
    owner: Count,
    resting: Count,
}

#[derive(Serialize)]
struct Isolation {
    historical: bool,
    passed: Count,
    failed: Count,
}

#[derive(Serialize)]
pub(super) struct Usage {
    daily_calls: Count,
    tokens: Count,
    reserved_calls: Count,
    reserved_tokens: Amount,
    key_cache_rows: Count,
    key_binding: &'static str,
}

impl Usage {
    pub(super) fn unknown(state: CheckState) -> Self {
        Self {
            daily_calls: Count::unknown(state),
            tokens: Count::unknown(state),
            reserved_calls: Count::unknown(state),
            reserved_tokens: Amount::unknown(state),
            key_cache_rows: Count::unknown(state),
            key_binding: "unchecked",
        }
    }
}

#[derive(Serialize)]
pub(super) struct LedgerChecks {
    pub(super) integrity: CheckState,
    spend: Spend,
    recent: RecentCalls,
    stopped: Stopped,
    pending: Count,
    isolation: Isolation,
    pub(super) usage: Usage,
    pub(super) embedding_quota: EmbeddingQuota,
}

impl LedgerChecks {
    pub(super) fn complete(&self) -> bool {
        self.embedding_quota.complete()
            && (matches!(self.integrity, CheckState::Absent)
                || [
                    self.integrity,
                    self.spend.curation_usd.state,
                    self.spend.embed_query_usd.state,
                    self.spend.reserved_usd.state,
                    self.stopped.owner.state,
                    self.stopped.resting.state,
                    self.pending.state,
                    self.isolation.passed.state,
                    self.isolation.failed.state,
                    self.usage.daily_calls.state,
                    self.usage.tokens.state,
                    self.usage.reserved_calls.state,
                    self.usage.reserved_tokens.state,
                    self.usage.key_cache_rows.state,
                ]
                .into_iter()
                .all(|state| matches!(state, CheckState::Known))
                    && self.recent.complete())
    }

    pub(super) fn unknown(state: CheckState) -> Self {
        Self {
            integrity: state,
            spend: Spend {
                curation_usd: Amount::unknown(state),
                embed_query_usd: Amount::unknown(state),
                reserved_usd: Amount::unknown(state),
            },
            recent: RecentCalls { state, calls: None },
            stopped: Stopped {
                owner: Count::unknown(state),
                resting: Count::unknown(state),
            },
            pending: Count::unknown(state),
            isolation: Isolation {
                historical: true,
                passed: Count::unknown(state),
                failed: Count::unknown(state),
            },
            usage: Usage::unknown(state),
            embedding_quota: EmbeddingQuota::unknown(state),
        }
    }
}

fn usage(
    conn: &Connection,
    config: std::result::Result<&crate::config::Config, CheckState>,
) -> Usage {
    let config = match config {
        Ok(config) => config,
        Err(state) => return Usage::unknown(state),
    };
    // Names share native accounting pools. Repeated configured entries must count each pool once.
    let names: std::collections::BTreeSet<_> = config.providers.iter().map(|p| p.name()).collect();
    let daily_calls = measured(read(
        conn,
        "provider_calls",
        &["provider", "ts", "outcome"],
        || {
            names.iter().try_fold(0i64, |sum, name| {
                sum.checked_add(i64::from(pdb::calls_in_a_day(conn, name)?.0))
                    .context("call count overflow")
            })
        },
    ));
    let since = crate::db::now_ms() - pdb::DAY_MS;
    let tokens = measured(read(
        conn,
        "provider_calls",
        &[
            "provider",
            "ts",
            "prompt_tokens",
            "completion_tokens",
            "bytes_out",
            "outcome",
            "detail",
        ],
        || {
            names.iter().try_fold(0i64, |sum, name| {
                sum.checked_add(pdb::tokens_since(conn, name, since)?.0)
                    .context("token count overflow")
            })
        },
    ));
    let reservations = read(
        conn,
        "provider_calls",
        &["provider", "ts", "role", "outcome", "detail"],
        || {
            names.iter().try_fold((0i64, 0.0), |(calls, tokens), name| {
                let reserved = pdb::reserved_since(conn, name, since)?;
                Ok((
                    calls
                        .checked_add(i64::from(reserved.calls))
                        .context("reservation count overflow")?,
                    tokens + reserved.tokens,
                ))
            })
        },
    );
    let (reserved_calls, reserved_tokens) = match reservations {
        Ok((calls, tokens)) => (measured(Ok(calls)), amount(Ok(tokens))),
        Err(state) => (Count::unknown(state), Amount::unknown(state)),
    };
    let key_cache_rows = measured(read(
        conn,
        "key_limits",
        &["provider", "free_daily", "read_at", "key_sha"],
        || {
            names.iter().try_fold(0i64, |sum, name| {
                Ok(sum + i64::from(pdb::key_limit(conn, name)?.is_some()))
            })
        },
    ));
    Usage {
        daily_calls,
        tokens,
        reserved_calls,
        reserved_tokens,
        key_cache_rows,
        key_binding: "unchecked",
    }
}

pub(super) fn queries(
    conn: &Connection,
    config: std::result::Result<&crate::config::Config, CheckState>,
) -> LedgerChecks {
    let integrity = match crate::db::quick_check(conn, "providers.db") {
        Ok(()) => CheckState::Known,
        Err(error) if error.downcast_ref::<rusqlite::Error>().is_none() => CheckState::Damaged,
        Err(error) => error_state(&error),
    };
    if !matches!(integrity, CheckState::Known) {
        return LedgerChecks::unknown(integrity);
    }
    let spend = Spend {
        curation_usd: amount(read(conn, "provider_calls", &["ts", "role", "usd"], || {
            pdb::usd_this_month(conn)
        })),
        embed_query_usd: amount(read(conn, "provider_calls", &["ts", "role", "usd"], || {
            pdb::embed_usd_this_month(conn)
        })),
        reserved_usd: amount(read(
            conn,
            "provider_calls",
            &["ts", "role", "outcome", "usd"],
            || pdb::reserved_usd_this_month(conn),
        )),
    };
    let recent = match read(
        conn,
        "provider_calls",
        &["id", "provider", "role", "outcome", "ms", "detail"],
        || pdb::last_call_rows(conn, 5),
    ) {
        Ok(rows) => RecentCalls {
            state: CheckState::Known,
            calls: Some(
                rows.into_iter()
                    .map(|row| CallSummary {
                        outcome: call_outcome(&row.outcome),
                        ms: (row.ms >= 0).then_some(row.ms),
                    })
                    .collect(),
            ),
        },
        Err(state) => RecentCalls { state, calls: None },
    };
    let stopped = match read(conn, "provider_state", &["provider", "down_until"], || {
        pdb::stopped(conn)
    }) {
        Ok(rows) => {
            let owner = rows
                .iter()
                .filter(|(_, until)| *until == pdb::OWNER_HOLD)
                .count() as i64;
            Stopped {
                owner: measured(Ok(owner)),
                resting: measured(Ok(rows.len() as i64 - owner)),
            }
        }
        Err(state) => Stopped {
            owner: Count::unknown(state),
            resting: Count::unknown(state),
        },
    };
    let pending = measured(read(
        conn,
        "pending",
        &[
            "device",
            "from_seq",
            "from_offset",
            "to_seq",
            "to_offset",
            "reason",
            "hold",
            "attempts",
            "next_attempt_at",
            "since",
            "prompt",
        ],
        || i64::try_from(pdb::pending(conn)?.len()).context("pending count overflow"),
    ));
    let isolation = match read(
        conn,
        "isolation",
        &["cli", "version", "passed", "detail", "ts"],
        || crate::isolation::doctor_rows(conn),
    ) {
        Ok(rows) => {
            let passed = rows.iter().filter(|row| row.passed).count() as i64;
            Isolation {
                historical: true,
                passed: measured(Ok(passed)),
                failed: measured(Ok(rows.len() as i64 - passed)),
            }
        }
        Err(state) => Isolation {
            historical: true,
            passed: Count::unknown(state),
            failed: Count::unknown(state),
        },
    };
    LedgerChecks {
        integrity,
        spend,
        recent,
        stopped,
        pending,
        isolation,
        usage: usage(conn, config),
        embedding_quota: embedding_quota(conn, config),
    }
}

#[derive(Serialize)]
struct CachedRest {
    state: CheckState,
    kind: Option<&'static str>,
    until_ms: Option<i64>,
}

#[derive(Serialize)]
struct LastError {
    state: CheckState,
    at_ms: Option<i64>,
    role: Option<&'static str>,
}

#[derive(Serialize)]
pub(super) struct EmbeddingQuota {
    state: CheckState,
    cached: bool,
    document_daily_requests: Option<u32>,
    query_daily_requests: Option<u32>,
    monthly_usd_cap: Option<f64>,
    calls_last_day: Count,
    rest: CachedRest,
    last_error: LastError,
}

impl EmbeddingQuota {
    fn complete(&self) -> bool {
        matches!(self.state, CheckState::Off | CheckState::Absent)
            || (matches!(self.state, CheckState::Known)
                && [
                    self.calls_last_day.state,
                    self.rest.state,
                    self.last_error.state,
                ]
                .into_iter()
                .all(CheckState::established))
    }

    pub(super) fn unknown(state: CheckState) -> Self {
        Self {
            state,
            cached: true,
            document_daily_requests: None,
            query_daily_requests: None,
            monthly_usd_cap: None,
            calls_last_day: Count::unknown(state),
            rest: CachedRest {
                state,
                kind: None,
                until_ms: None,
            },
            last_error: LastError {
                state,
                at_ms: None,
                role: None,
            },
        }
    }

    pub(super) fn configure(
        &mut self,
        config: std::result::Result<&crate::config::Config, CheckState>,
    ) {
        let config = match config {
            Ok(config) => &config.embedding,
            Err(state) => {
                *self = Self::unknown(state);
                return;
            }
        };
        if config.provider != "workers-ai" {
            *self = Self::unknown(CheckState::Off);
            return;
        }
        self.state = CheckState::Known;
        self.document_daily_requests = Some(
            config
                .daily_requests
                .saturating_sub(crate::embed_phase::KEPT_FOR_QUERIES),
        );
        self.query_daily_requests = Some(config.daily_requests);
        self.monthly_usd_cap = Some(config.monthly_usd);
    }
}

fn embedding_quota(
    conn: &Connection,
    config: std::result::Result<&crate::config::Config, CheckState>,
) -> EmbeddingQuota {
    let mut quota = EmbeddingQuota::unknown(CheckState::Unavailable);
    quota.configure(config);
    if !matches!(quota.state, CheckState::Known) {
        return quota;
    }
    quota.calls_last_day = measured(read(
        conn,
        "provider_calls",
        &["provider", "ts", "outcome"],
        || Ok(i64::from(pdb::calls_in_a_day(conn, crate::embed::CALLS)?.0)),
    ));
    quota.rest = match read(
        conn,
        "provider_state",
        &["provider", "down_until", "fails", "backoff"],
        || pdb::state(conn, crate::embed::CALLS),
    ) {
        Ok(state) => {
            let until = state.down_until;
            let kind = if until == pdb::OWNER_HOLD {
                "owner_hold"
            } else if until > crate::db::now_ms() {
                "resting"
            } else {
                "ready"
            };
            CachedRest {
                state: CheckState::Known,
                kind: Some(kind),
                until_ms: (kind == "resting").then_some(until),
            }
        }
        Err(state) => CachedRest {
            state,
            kind: None,
            until_ms: None,
        },
    };
    quota.last_error = match read(
        conn,
        "provider_calls",
        &["id", "provider", "outcome", "ts", "role", "detail"],
        || crate::embed_phase::last_embedding_error_in(conn),
    ) {
        Ok(None) => LastError {
            state: CheckState::Known,
            at_ms: None,
            role: None,
        },
        Ok(Some(error)) if error.ts >= 0 => LastError {
            state: CheckState::Known,
            at_ms: Some(error.ts),
            role: Some(match error.role.as_str() {
                "embed" => "embed",
                "query" => "query",
                _ => "other",
            }),
        },
        Ok(Some(_)) => LastError {
            state: CheckState::Unavailable,
            at_ms: None,
            role: None,
        },
        Err(state) => LastError {
            state,
            at_ms: None,
            role: None,
        },
    };
    quota
}
