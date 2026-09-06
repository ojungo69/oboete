# Data Model: oboete M1

All tables live in `~/.oboete/memory.db`. Migrations are numbered SQL files
(`src/db/migrations/0001_core.sql`, `0002_memory_search.sql`, `0003_operations.sql`) embedded at
build time and applied forward only, one transaction each, by `setup`, the worker, and user-facing
commands; the hook never migrates and spools when `PRAGMA user_version` is older than the bundle's
latest migration. A smoke test applies every migration on an empty database and on the previous
version's fixture database on Node 22.16 and 24.x. Time columns are Unix milliseconds. Ordinary
tables are `STRICT`; FTS5 virtual tables cannot be. Every write from the worker is fenced by
`worker_lease.owner_token` except the exhaustion signal in `provider_usage` (R6).

## schema_migrations

| column | type | notes |
|---|---|---|
| version | INTEGER PK | matches `PRAGMA user_version` after apply |
| name, sha256 | TEXT | checksum of the SQL text; mismatch aborts |
| applied_at | INTEGER | |

## repos

| column | type | notes |
|---|---|---|
| id | TEXT PK | first 16 hex of sha256 over the normalized identity (FR-004); the only repository value a remote observer ever receives |
| identity_kind | TEXT | `remote` or `common_dir` (machine-local; see import mapping) |
| normalized_identity | TEXT UNIQUE | `host/path` (userinfo, query, and fragment removed before storage) or realpath of the git common dir |
| display_root | TEXT | last seen working tree |
| created_at, last_seen_at | INTEGER | |

## sessions

| column | type | notes |
|---|---|---|
| id | TEXT PK | |
| repo_id | TEXT FK | |
| agent | TEXT | `claude`, `codex`, `grok`, `pi`, `unknown`; provenance only |
| native_session_id | TEXT | UNIQUE with agent |
| conversation_id | TEXT | oboete id of the root session: `id` for a fresh session; the root's id when the agent resumes (Claude Code `resume` with the same `session_id`, Codex `resume`, Pi when `PI_SESSION_ID` continues per the R13 probe); a new root on `fork` and Grok `new` |
| model | TEXT | reported model when the agent supplies one (context window lookup, R12) |
| started_at, ended_at | INTEGER | |
| status | TEXT | `active`, `ended` |
| turn_count | INTEGER | |
| latest_summary_memory_id | TEXT | set once by the worker's deterministic session summary, in the same transaction as the summary insert and `summary_state = done`; every worker run reconciles `ended` sessions with `summary_state = pending` whose batches are all terminal (R10) |
| context_epoch | INTEGER | authoritative epoch of the conversation root: 0 at start, +1 per compaction (A12); stored on the root session row |
| last_compaction_key | TEXT | the native per-compaction value of the authoritative compaction event (`PostCompact` on Claude Code, Codex, and Grok; Pi's compaction event) that the R13 probe verified as unique across byte-identical compactions and re-deliveries and as committed before any post-compaction injection hook. For an agent whose probe failed the epoch is **not** advanced and compaction re-injection is blocked; only after the owner approves A16 may that agent use the `PostCompact` event's `raw_events.id` (which collapses byte-identical same-turn compactions) under the documented ordering limit. The companion `SessionStart source=compact` never advances the epoch |
| summary_state | TEXT | `pending` (session ended, summary not yet written), `done` (`latest_summary_memory_id` set), `no_content` (zero summarizable events: no `prompt`, `tool_call`, `tool_result`, `tool_failure`, `last_assistant_message`, or `compaction_summary` row with non-empty content after `<private>` removal that is not `secret` and not `classification_state = failed`; lifecycle rows such as `session_start`, `turn_end`, `session_end`, `probe` never count; no memory row is created and nothing is injected, per the spec edge case); reconciliation targets `pending` only |

## turns

| column | type | notes |
|---|---|---|
| id | TEXT PK | |
| session_id | TEXT FK | |
| ordinal | INTEGER | UNIQUE with session_id |
| started_at, ended_at | INTEGER | `ended_at` NULL = unfinished; never feeds retrieval |

## raw_events (append-only acceptance point)

| column | type | notes |
|---|---|---|
| id | TEXT PK | sha256 over the most specific stable key (R7): (agent, native_session_id, kind, tool_call_id or native event id) → (agent, native_session_id, kind, prompt_id) → (agent, native_session_id, turn ordinal, kind, content_hash); no delivery counter, so re-delivery always collapses; two byte-identical events of the last form inside one turn also collapse (accepted) |
| repo_id, session_id, turn_id | TEXT | |
| agent | TEXT | |
| kind | TEXT | `session_start`, `prompt`, `tool_call`, `tool_result`, `tool_failure`, `turn_end`, `session_end`, `compaction_summary`, `last_assistant_message`, `probe` |
| content | TEXT | stored after `<private>` removal and redaction; NULL for path-rule hits and for `classification_state = failed`; the redacted read part for `partial` rows |
| truncated | INTEGER | 1 when the payload exceeded the 256 KiB read bound (A7, A14) |
| payload_json | TEXT | normalized fields (zod-validated); no raw passthrough |
| content_hash | TEXT | |
| sensitivity | TEXT | `local_only` (default), `eligible`, `secret`, `private` |
| classification_state | TEXT | `pending`, `done`, `partial` (payload above the 256 KiB read bound: redacted read part kept with `truncated = 1`, never promoted, metadata only to the fallback, never to a provider or a pack), `failed` (detector or config failure: metadata only with `payload_json.failure_reason`, never summarized or injected) |
| captured_at, expires_at | INTEGER | `expires_at` = captured_at + 7 days |
| batch_id | TEXT | set when claimed |
| via_spool | INTEGER | |

Indexes: (session_id, captured_at), (expires_at), (batch_id).

## observation_batches

| column | type | notes |
|---|---|---|
| id | TEXT PK | |
| repo_id, session_id | TEXT | |
| through_event_id | TEXT | |
| destination | TEXT | `remote_observer`, `local_observer`, `fallback` (batch split by sensitivity, R10) |
| trigger | TEXT | `ten_turns`, `session_end`, `retention` (forced before purge) |
| state | TEXT | `pending`, `running`, `applied`, `fallback` |
| owner_token | TEXT | worker that claimed it; reclaimable after 120 s when the lease changed hands |
| provider_attempts | INTEGER | incremented per HTTP attempt with its reservation |
| last_reservation_id | TEXT | reservation of the most recent attempt |
| degraded_reason | TEXT | `no_provider`, `unreachable`, `unusable_output`, `language_mismatch`, `daily_cap`, `provider_exhausted`, `provider_paid`, `auth_failed`, `consent_changed`, `model_alias`, `timeout`, `rule_based` |
| excerpted | INTEGER | input excerpted to 12,000 characters (FR-015) |
| claimed_at, completed_at | INTEGER | |

UNIQUE (session_id, through_event_id, destination): a remote batch and a fallback batch over the
same range coexist with disjoint rows, and applying either twice is impossible; memory mutations
and `state = applied` commit in one fenced transaction.

## memories

| column | type | notes |
|---|---|---|
| rid | INTEGER PRIMARY KEY | stable alias of `rowid`, the FTS `content_rowid` |
| id | TEXT NOT NULL UNIQUE | public memory identifier and foreign-key target |
| repo_id | TEXT FK | |
| type | TEXT | `bugfix`, `feature`, `refactor`, `change`, `discovery`, `decision`, `security_alert`, `security_note`, `session_summary` |
| title, body | TEXT | <= 120 / <= 2,000 characters |
| concepts | TEXT | JSON array |
| cjk_bigrams | TEXT | generated shadow column for the CJK FTS table |
| material_hash | TEXT | sha256 over (normalized title, normalized body), observation type excluded (A13); repository-independent, kept on tombstones; computed only by `db/identity.ts` |
| content_hash | TEXT UNIQUE | sha256 over (repo_id, material_hash); same helper on every path (provider, fallback, import, tombstone) |
| sensitivity | TEXT | `add`: strictest source row and detector; `update`: max(target, every source row, detector), fixed in the apply transaction |
| review_state | TEXT | `unreviewed` (default, injectable at once per FR-042), `reviewed`, `imported` (quarantined: excluded by the shared query function from search, injection, MCP, and the viewer's injectable set until the worker's detector and directive check move it to `unreviewed` or `secret`) |
| degraded_reason | TEXT | NULL for provider output; on a session summary the most severe reason among the session's batches by the fixed precedence in `contracts/observer.md` |
| source_session_id, source_batch_id | TEXT | provenance; carried by export |
| valid_from, valid_to | INTEGER | bitemporal validity; `valid_to` set on supersession |
| superseded_by | TEXT | |
| pinned_at, pin_order | INTEGER | |
| last_injected_at | INTEGER | 90-day retirement |
| citations_head | TEXT | repository `HEAD` at the last worker citation check |
| citations_ok | INTEGER | 1 when every cited commit is an ancestor of `citations_head` |
| deleted_at | INTEGER | tombstone; the row stays so the same content never resurrects |
| created_at | INTEGER | |

Indexes: (repo_id, deleted_at, pinned_at), (repo_id, valid_to), (repo_id, review_state).

## memory_sources

| column | type | notes |
|---|---|---|
| id | INTEGER PK | one row per (memory, source event, citation) |
| memory_id, raw_event_id | TEXT | index on memory_id |
| citation_kind | TEXT | `file_read`, `file_modified`, `commit`, NULL |
| citation_value | TEXT | path or commit id |
| source_agent | TEXT | provenance only |

## memories_fts, memories_fts_cjk

External-content FTS5 tables over `memories` (`content = 'memories'`, `content_rowid = 'rowid'`,
the stable `rid INTEGER PRIMARY KEY` alias):
`memories_fts (title, body)` with `tokenize = 'trigram'`; `memories_fts_cjk (cjk_bigrams)` with
`tokenize = 'unicode61'`. Maintained by triggers on insert, update, delete. Queries join back to
`memories` for scope, tombstone, sensitivity, review state, and validity.

## destination_rules (seeded)

| destination | sensitivity | allowed | same_repo_required |
|---|---|---|---|
| remote_observer | eligible | 1 | 0 |
| remote_observer | local_only / private / secret | 0 | - |
| local_observer | eligible / local_only / private | 1 | 1 |
| local_observer | secret | 0 | - |
| injection | eligible / local_only / private | 1 | 1 |
| injection | secret | 0 | - |
| sync (M2) | everything except secret | 1 | 0 |

One function evaluates the table for every egress decision: batch composition, every field of an
outbound observer request (`observer/request.ts`), injection, CLI, MCP, viewer.

## injections

| column | type | notes |
|---|---|---|
| id | TEXT PK | |
| repo_id, session_id, conversation_id, turn_id | TEXT | |
| kind | TEXT | `session_start`, `prompt`, `grok_deferred` |
| channel | TEXT | e.g. `claude:SessionStart`, `codex:UserPromptSubmit`, `grok:PreToolUse`, `grok:PostToolUse`, `pi:before_agent_start` |
| state | TEXT | `built`, `emitted`, `omitted`; Grok only: `pending`, `attempted` |
| context_epoch | INTEGER | copied from the root session's authoritative epoch when the pack is built (A12) |
| attempts_json | TEXT | Grok: JSON array of `{ tool_call_id, execution: pending \| ran \| failed \| denied, delivery: pending \| delivered \| dropped, at }`, one entry per attachment. `PostToolUse` → execution `ran`, delivery `delivered`; `PostToolUseFailure` → execution `failed`, delivery `delivered` or `dropped` per the R13 probe; `PermissionDenied` (when its payload is verified) → execution `denied`, delivery `dropped`; `Stop` → every `pending` becomes `dropped`. Each hook updates the row in one `BEGIN IMMEDIATE` transaction (single-row read-modify-write), so concurrent hooks of a parallel batch serialize. Persists after raw events expire so `why` reproduces a mixed deny/success batch (tested after purge) |
| delivery_count | INTEGER | attempts with delivery `delivered`, counted per call when the R13 probe shows per-call delivery (A15) and capped at 1 per batch when it shows per-batch delivery |
| pack_hash | TEXT | recognized on capture (FR-021) |
| char_budget, chars_used | INTEGER | |
| degraded_reason | TEXT | `summary_pending`, `index_unavailable`, `empty`, `window_unknown`, `no_tool_call`, `not_delivered`, plus batch reasons |
| created_at, attempted_at, emitted_at | INTEGER | |

## injection_items

| column | type | notes |
|---|---|---|
| id | INTEGER PK | |
| injection_id | TEXT FK | |
| conversation_id | TEXT | copied from the injection so the uniqueness index is local to this table |
| context_epoch | INTEGER | copied from the injection |
| source_kind | TEXT | `memory`, `raw_activity` (summary-pending fallback), `session_summary` |
| memory_id | TEXT | NULL for raw activity |
| raw_event_id | TEXT | NULL for memories |
| decision | TEXT | `planned` (built, not yet delivered), `included` (delivered), `omitted` |
| reason | TEXT | `below_threshold`, `budget`, `duplicate_in_conversation`, `stale_path`, `stale_commit`, `retired`, `mmr_redundant`, `pinned`, `summary`, `not_delivered` |
| rank | INTEGER | |
| score_bm25, score_rrf, score_mmr | REAL | |
| stale | INTEGER | |

Partial unique index on (conversation_id, context_epoch, memory_id) where decision = `included`
and memory_id is not NULL (SC-010; compaction opens a new epoch so FR-024's re-injection and
FR-026's no-duplicate rule do not conflict, A12). Items are written as `planned` when a pack is built and become `included` only
when delivery is confirmed (immediately for stdout channels; at `PostToolUse` for Grok), so a Grok
pack that is never delivered does not consume the memory for the conversation. A pending Grok
record that is rebuilt on a later prompt merges: items already `planned` stay, new items are added
under the same budget, and the merged pack gets a new `pack_hash`.

## worker_lease (single row, seeded by 0001)

| column | type | notes |
|---|---|---|
| id | INTEGER PK CHECK (id = 1) | |
| owner_token | TEXT | NULL when released; every worker write is fenced on it |
| pid | INTEGER | informational |
| started_at, heartbeat_at | INTEGER | stale when older than 6 s or more than 60 s in the future |

## provider_usage

| column | type | notes |
|---|---|---|
| utc_day, preset | TEXT | PK pair |
| calls | INTEGER | incremented per HTTP attempt before the call, in its own transaction; the FR-012 cap (150 per UTC day, attempt 151 refused) is checked against the sum over all presets of the day |
| neurons_estimate | REAL | |
| reset_at | INTEGER | stored reset instant; day boundary compares against it |
| exhausted_at | INTEGER | set on 3036 by an unfenced, monotonic write keyed by the reservation |
| exhausted_reservation_id | TEXT | reservation that observed 3036 (idempotency key) |
| resolved_model | TEXT | model id returned by the provider |

## runtime_state

Key/value (`key TEXT PK`, `value_json TEXT`, `updated_at INTEGER`): last purge, last checkpoint,
catalog cache, consent record mirror. The
pause flag is the file `~/.oboete/paused`. The context-window table is the versioned document
`docs/research/context-windows.md` embedded at build time (R12), not a runtime row.

## diagnostics

| column | type | notes |
|---|---|---|
| id | TEXT PK | |
| kind, severity, agent, message_code | TEXT | no secret values |
| details_json | TEXT | |
| count | INTEGER | sightings: one per report or worker sweep; distinct subjects (a hung Pi invocation) are de-duplicated in `details_json` |
| first_seen_at, last_seen_at, cleared_at | INTEGER | |

Pi: the extension performs no file or network write; it generates an `invocation_id` per spawn,
keeps in-memory failure counters (message codes), and passes them to the next child as
`--prior-failures`, which records them here. The capture child writes
`~/.oboete/spool/pi-ack/<invocation_id>.started` before reading stdin and renames it to `.done` on
success; the worker folds and deletes `.done` files, records `pi_child_hang` for `.started` files
older than 30 s and deletes them after 24 h. Doctor reports `pi_child_hang` and `pi_child_failed`
from these rows, `pi_spawn_failed` from its own wiring probe, and, when the R13 probe finds one,
Pi's own durable error log (R12, amendment A8).

## sync_conflicts (M2 reservation)

`id, repo_id, content_hash, local_state_json, remote_state_json, status, created_at`; no M1 code
writes it.

## Spool entry (filesystem)

`~/.oboete/spool/<captured_at>-<event_id>.json`, written to a temporary name and renamed; content is
the normalized, already redacted event (never an event whose detector run failed). Recovered in
name order with the same deterministic id.

## Export line (JSONL, `oboete-export/1`)

Header line `{ "format": "oboete-export/1", "exported_at": ..., "repos": [ { id, identity_kind,
normalized_identity } ] }`, then one line per memory: `{ id, repo_id, type, title, body, concepts,
material_hash, content_hash, sensitivity, review_state, degraded_reason, source_session_id,
source_batch_id, source_agent, valid_from, valid_to, superseded_by, pinned_at, pin_order,
deleted_at, created_at, sources: [ { citation_kind, citation_value, source_agent } ] }`. Tombstones
are exported with `deleted_at` set, an empty `body`, and their `material_hash`. Import validates
each line (64 KB max) and the file (256 MB max), verifies `material_hash` against the body when
one is present (mismatch rejects the line), recomputes `content_hash` from the local repository id
(after `--map-repo`) and `material_hash` for active rows and tombstones alike, recomputes ids and
`cjk_bigrams`, unions on `content_hash`, applies the lattice `secret > private > local_only >
eligible` (the stricter wins), lets tombstones win over active rows, and inserts every active
imported row as `local_only` with `review_state = imported` (quarantined until the worker
classifies it).

## State transitions

- raw_events.sensitivity: `local_only` → `eligible` (worker checks pass) | `secret`; `private` is
  set at capture and never promoted; `classification_state = failed` rows are purged unread.
- observation_batches.state: `pending` → `running` → `applied` | `fallback`.
- memories: active → superseded | deleted; deleted never returns to active;
  review_state `imported` → `unreviewed` (worker checks pass) | tombstoned as `secret`;
  `unreviewed` → `reviewed`.
- worker_lease: released → held (fenced claim) → released (atomic with the empty-queue check) |
  stale (missed heartbeats, clock jump) → reclaimed.
- injections.state: `built` → `emitted` | `omitted`; Grok: `pending` → `attempted` (attached on
  every `PreToolUse` until confirmed) → `emitted` (a `PostToolUse` for an attempted call) |
  `omitted` (turn end); items `planned` → `included` on confirmation.
- sessions.context_epoch: incremented once per distinct verified `last_compaction_key`; unchanged
  (and re-injection blocked) for an agent whose R13 compaction probe failed until A16 is approved.
- injections (Grok): an attempt whose delivery is `dropped` leaves the record non-emitted
  (`attempted`); the next `PreToolUse` attaches the pack again.
