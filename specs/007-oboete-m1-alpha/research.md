# Phase 0 Research: oboete M1 Self-Use Alpha

**Date**: 2026-09-02 | **Inputs**: `spec.md`, `CONSTITUTION.md` 3.0.0,
`docs/research/oboete-contracts-2026-09-02.md`, `codex-plan.json` (independent second plan)

Method: ten research topics were investigated in parallel with primary sources and local probes on
Node 24.16.0, each attacked by two independent refuters (source fidelity, feasibility), and the
resulting plan went through two rounds of independent Codex plan review (22 and 16 blocking
findings). Every decision records what the reviewers changed. Deviations from the constitution's
or the spec's literal text are listed in `plan.md` Complexity Tracking and need the owner's
approval before implementation starts (task 0).

## R1. SQLite runtime and write serialization

- **Decision**: One `~/.oboete/memory.db` in WAL mode; every `DatabaseSync` connection opens with an
  explicit `timeout` (the default busy timeout is 0). The capture hook performs one autocommit
  `INSERT` into `raw_events` and, on `errcode` 5 (`SQLITE_BUSY`) or 6 (`SQLITE_LOCKED`) or any other
  storage failure, writes the already sanitized event to the spool and exits 0. Every worker
  read-then-write unit is a short `BEGIN IMMEDIATE` transaction; network calls never run inside a
  transaction. The worker owns checkpointing (`PASSIVE` after each batch, `TRUNCATE` before exit);
  hook connections set `PRAGMA wal_autocheckpoint = 0`.
- **Rationale**: FTS5 with trigram is compiled into Node's SQLite in both v22.16.0 and v24.16.0;
  `node:sqlite` needs no flag from v22.13.0; a live probe with one `BEGIN IMMEDIATE` holder and three
  concurrent writers serialized correctly with `timeout: 3000` and failed at 0 ms with the default;
  `loadExtension` exists behind `allowExtension: true` for M2.
- **Reviewer changes**: `BEGIN IMMEDIATE` for read-then-write; checkpoint owner; no reliance on
  commit order.
- **Alternatives**: dedicated writer process (a daemon); application retry loops; rollback journal.
- **Open**: hook busy timeout (start 150 ms) and checkpoint discipline are validated by the fixture
  on Node 22.16 (SQLite 3.49.1) and 24.x (3.53.0).

## R2. Build, bundle, and hook startup

- **Decision**: esbuild (dev) compiles TypeScript to one ESM file `dist/oboete.mjs` that bundles the
  small pure-ESM packages the hook path needs (`zod`, `smol-toml`, `@secretlint/core`,
  `@secretlint/secretlint-rule-preset-recommend`) and leaves `node:*` and the heavy packages (`ai`,
  `@ai-sdk/*`, `workers-ai-provider`, `hono`, `@hono/node-server`, `preact`) external, imported
  lazily by `observe`, `view`, `mcp`, `setup`. Shebang preserved; `tsc --noEmit` and `eslint` are
  gates; migrations are real numbered `.sql` files embedded by esbuild's text loader; tests are
  compiled by the same step to `build/test/*.mjs` (Node 22.16 cannot run `.test.ts`). Setup writes
  hook commands in shell form with absolute paths (no `args` field on Grok Build or Codex). The Pi
  extension file is a three-line `.js` loader with a default-export factory.
- **Rationale**: type stripping is flagged below 22.18; tsup adds 17 dependencies; bundling CJS-heavy
  packages into ESM output throws `Dynamic require`; `@secretlint/node` cannot be bundled.
- **Reviewer changes**: honest list of hook-path packages; Pi artifact shape; compiled tests; lint.
- **Gate (R13)**: the real bundle's cold start is measured on 22.16 and 24.x after the minimal
  skeleton exists; budget 100 ms for process start, bundle load, and detector init.

## R3. Observer client and presets

- **Decision**: `ai` + `workers-ai-provider` (REST mode; peers `ai`, `@ai-sdk/provider`) for the
  default `@cf/zai-org/glm-4.7-flash`; `@ai-sdk/openai-compatible` for `ollama` (required local
  option), `nim`, `openrouter`, `gemini` (the `anthropic` preset was removed by the owner on
  2026-09-04, A19); an optional `agent-cli` preset runs the
  developer's already-authenticated `claude -p` / `codex exec` / `grok -p` as a child process
  (text-JSON, no credentials, consumes that subscription, uncapped by oboete) so that a developer
  with no Cloudflare account still gets model-quality summaries, as claude-mem's host provider
  does. Schema-constrained output where the endpoint
  supports `response_format` (verified 2026-09-04: OpenRouter, Workers AI); text-JSON (prompt for one
  JSON object, parse, validate with zod) for any preset whose R13 probe shows no schema support
  (verified: NIM, Gemini). Every HTTP attempt:
  `maxRetries: 0`, `abortSignal: AbortSignal.timeout(60_000)`, its own reservation (R6), and
  oboete's error classification by status and body code (429/3036 exhausted, 403/5035 paid,
  401 or 403 without 5035 `auth_failed`, 408/3007 and 429/3040 one retry, `length`/null/invalid
  JSON one retry then `unusable_output`, model id mismatch `model_alias`, abort `timeout`,
  network `unreachable`). One observer preset is enabled at a time in M1. Neurons from the `cf-ai-neurons` header
  when exposed, else estimated from tokens (5,500 / 36,400 per million).
- **Reviewer changes**: retries disabled; error table only on the binding path; deadline mandatory;
  Anthropic was kept as text-JSON and added to R13, then removed as a preset by the owner on
  2026-09-04 (A19) before its row was probed.

## R4. Secret detection and redaction (before any write)

- **Decision**: The hook runs the complete detector before the first write anywhere: strip
  `<private>...</private>` (unclosed tag removes the rest); apply repository path rules (a hit stores
  metadata only, class `secret`); run `@secretlint/core` `lintSource()` with the statically imported
  recommend preset (about 30 ms cold) plus entropy only on regex-captured candidates (3.0 bits/char
  hex, 4.0 base64 >= 32 chars; never on bare words, SHAs, UUIDs, paths); replace hits with
  `[REDACTED:<rule>]`; store `local_only` or `secret`. The worker runs the same detector on every
  candidate memory before insert and decides promotion. **Payload size**: the hook reads
  at most 1 MB from stdin and stops (Pi: the extension's serialized event, same bound); a payload
  that exceeds the bound keeps what was read: the read part goes through the detector and is
  stored as a `partial` row marked `truncated` (event kind from the handler's fixed `--event`
  argument, session id and path fields from a bounded scan of the read bytes; when no session id
  can be recovered nothing is stored and a `diagnostics` counter is incremented). Partial rows
  stay `local_only`, give only metadata (tool name, paths) to the rule-based summarizer, never
  enter a provider request, and are never injected (A7; claude-mem withdrew a drop-everything
  design for the same case). Nothing is drained, so capture time is bounded
  by 1 MB regardless of payload size (spec edge case amended, A7; the summarizer input bound
  stays 12,000 characters). Whether each agent's runner tolerates a hook that exits with unread
  stdin, and whether the runner caps payloads itself, is an R13 probe; a runner that fails the
  hook in that case blocks A7 for an owner decision (no drain fallback). The detector runs in a `worker_threads` Worker terminated at a hard cutoff so the hook's wall
  time is bounded even when secretlint never returns (a terminated run is a detector failure).
  **Detector or config failure** (secretlint throws, `.oboete.toml` malformed,
  bundle mismatch): fail closed, store metadata only with `classification_state = failed`, write a
  diagnostic, exit 0; the unsanitized text is never written to the spool.
- **Rationale**: FR-018 and Principle III require redaction before storage; `@secretlint/node` costs
  120-145 ms and is bundle-hostile; secretlint has no entropy rule; content-size-dependent time must
  be bounded to keep the 300 ms SLA measurable.
- **Reviewer changes**: two-stage draft violated FR-018; gated entropy; stdin cap and fail-closed
  detector failures.
- **Deviation**: Principle VI names `@secretlint/node` (Complexity Tracking #3); the stdin cap
  amends the spec's "stored whole" edge case (Complexity Tracking #16).

## R5. Lexical retrieval

- **Decision**: External-content FTS5 `memories_fts` (`trigram`, title + body) and
  `memories_fts_cjk` (`unicode61` over generated CJK bigrams; runs include ー 々 ・). Query via
  `Intl.Segmenter`; non-CJK segments >= 3 chars → trigram terms; CJK segments → bigram terms;
  one-character particles dropped; `LIKE` only when no indexed term remains. BM25 per table
  normalized by the best score; threshold (default 0.3) on normalized BM25; RRF (k = 60) orders
  across tables (`LIKE` never votes); MMR (lambda 0.5, character-trigram cosine) removes near
  duplicates; cut at the caller's character budget. FTS5 tables are not `STRICT`.
- **Reviewer changes**: threshold on BM25; particles; `LIKE` non-voting; long-vowel marks; no
  `UNINDEXED` shortcut; no `STRICT` on virtual tables.

## R6. Detached worker, lease, spool, retention, reservations

- **Decision**: Hooks spawn `oboete observe` detached only when `worker_lease` is released or stale.
  The single seeded row is claimed, heartbeaten, and released in `BEGIN IMMEDIATE` transactions
  fenced by `owner_token`; release is atomic with the empty-queue check; a heartbeat older than 6 s
  or more than 60 s in the future is stale. Batches carry the owner token; a stale worker's
  `running` batch is reclaimed only after takeover and only if `claimed_at` is older than 120 s.
  **Reservations are per HTTP attempt**: before every attempt (first call and any retry) a short
  transaction checks the daily cap (FR-012: 150 attempts per UTC day summed over every capped preset, so
  switching presets cannot exceed it; attempt 150 is allowed, attempt 151 refused; with 10 or
  fewer left, ten-turn batches fall back and the rest is reserved for session-end batches;
  `agent-cli` is uncounted because it spends no oboete allowance) and
  `exhausted_at`, increments `provider_usage.calls` and `observation_batches.provider_attempts`,
  and commits; a 429/3036 result is persisted in its own
  transaction keyed by the reservation (`provider_usage.exhausted_at`, monotonic, not fenced by the
  lease) so the signal survives a lost lease; the batch apply is fenced separately. **Provider
  attempts are at-least-once, applied effects exactly-once**: memory mutations and `state =
  applied` commit in one fenced transaction; a worker that dies between the response and that
  commit leaves the batch `running`, and the reclaiming worker makes a new call. No response
  journal (a journal needs its own fencing, sanitizing, and retention rules for a rare crash
  window); the worker-kill test asserts HTTP count 2 and apply count 1 for that window (spec
  wording "summarized twice" is read as applied twice, A11). Spool entries are
  one sanitized file per event (write-then-rename). Retention: rows past `expires_at` in `applied`
  or `fallback` batches are deleted in bounded `DELETE ... WHERE id IN (SELECT ... LIMIT 500)` steps;
  rows past `expires_at` in pending batches are forced into a fallback batch first; `secret`
  metadata rows likewise. When neither the database nor the spool is writable the hook exits 0 and
  reports the count to stderr; doctor probes writability.
- **Reviewer changes**: seed row, `SQLITE_BUSY` on claim, atomic release, owner fencing,
  per-attempt reservations, unfenced exhaustion persistence, retention over every terminal state.

## R7. Hook normalization, agent identity, event identity

- **Decision**: One discriminated union of normalized events with a common envelope; no raw
  passthrough; content stored whole up to the R4 size cap. **Agent identity**: each agent's
  setup-written handler carries a fixed selector that the user cannot change without editing the
  managed block: Codex `hook --agent codex`, Pi `capture --agent pi`; Claude Code and Grok Build
  share `hook --agent claude-or-grok`, resolved from the environment (`GROK_HOOK_EVENT` or
  `GROK_SESSION_ID` → grok, else claude). Payload-shape heuristics are used only to label an
  `unknown` invocation for doctor, never to choose an adapter. **Conversation identity**:
  `sessions.conversation_id` is the oboete id of the root session; on Claude Code `resume` (same
  `session_id`) and Codex `resume` the root is the existing session; `fork` and Grok `new` start a
  new root. Pi supports `resume` and `fork` (`session_shutdown.reason` enum, verified); whether
  `PI_SESSION_ID` and `session_start` continue across a resume is an R13 probe, and until it
  passes the Pi adapter treats a session whose native id already exists as a resume of that root.
  The session-start pack is emitted at most once per conversation except after compaction, which
  is how FR-024's "not again on resume" holds on every agent, including Pi. **Event identity**:
  `raw_events.id` = sha256 over the most specific stable native key available in this order:
  (agent, native session id, kind, tool_call_id or the agent's event id) → (agent, native session
  id, kind, `prompt_id`) → (agent, native session id, turn ordinal, kind, content_hash). No
  per-delivery counter enters the key, so an identical re-delivery always collapses; the accepted
  limit is that two byte-identical events of the last form inside one turn also collapse
  (recorded in `contracts/agents.md`). Per-agent mapping essentials:
  Claude Code injects at `SessionStart` for `startup`, `clear`, `compact` only and at
  `UserPromptSubmit`; Codex matcher `startup|clear|compact`; Grok Build follows the FR-045 state
  machine; Pi captures `session_start`, `input`, `tool_result`, `agent_settled`, `session_shutdown`
  through a detached child and injects from `before_agent_start` via a bounded child;
  `PostToolUseFailure` → `tool_failure`.
- **Reviewer changes**: fixed selectors and `--event` arguments instead of a `model`-field
  heuristic (the evidence verifies `cwd`, `hook_event_name`, `session_id`, `transcript_path` as
  universal, not `model`); one conversation-id definition; kind in every event key and no delivery
  counter; Pi and Grok resume probes.

## R8. Configuration, paths, repository identity, foreign files

- **Decision**: `smol-toml` for `config.toml` and `.oboete.toml`; `~/.oboete/` with `OBOETE_HOME`
  (XDG deferred, Complexity Tracking #4, which also touches spec FR-039 and Assumptions);
  repository identity from the normalized remote or the realpath of `git rev-parse --git-common-dir`
  (`common_dir` identities are machine-local; import maps them with `--map-repo`). Foreign files
  are edited through oboete-managed blocks (`# oboete:begin/end` in TOML; `"oboete": true` handlers
  in JSON) with backup → temporary file → re-parse → rename; backups and rewritten files keep the
  original mode and owner, and backups of files that may contain credentials are created 0600.
  Consent for a remote preset is a record of the hash of the displayed tuple (preset, host,
  credential source, cost class, egress classes); `--yes` is accepted only when the stored hash
  matches the tuple setup would display now, otherwise `--accept-egress` is required. The same
  hash is recomputed from the live configuration before every reservation and again immediately
  before the request is sent; a mismatch makes no network call and degrades the batch with
  `consent_changed`. Remote identities are normalized with userinfo, query, and fragment removed
  before anything is stored, so a credential embedded in a remote URL never reaches the database
  or a pack.
- **Reviewer changes**: `--git-common-dir`; managed blocks; consent bound to the full tuple and
  re-checked at send time; backup mode and owner; userinfo stripped from remotes.

## R9. Viewer and search surface

- **Decision**: Hono on `@hono/node-server`, `127.0.0.1` only, per-launch token, `Origin` check on
  mutating routes, SSE via `hono/streaming`, change detection by polling `PRAGMA data_version`
  every 500 ms; Preact + Vite assets embedded at build time. `oboete mcp` is a legacy-era stdio
  server whose exact wire format (initialize result, `tools/list` with `inputSchema`, `tools/call`
  results with `content` text plus `structuredContent`, error mapping) is in `contracts/mcp.md`;
  `server/discover` returns `-32601`. Repository identity from the server's working directory; a
  client-supplied repository is rejected. Pi's tools call the CLI as child processes. FR-030's
  "tool interface" is satisfied for Claude Code, Codex, and Grok Build by MCP and for Pi by the
  extension's own tool registration that calls the CLI; if the R13 probe shows an agent's MCP
  client cannot use the legacy server, M1 is blocked for that agent's tool surface until the owner
  approves a spec amendment (R13).
- **Reviewer changes**: full wire contract; no silent CLI fallback for FR-030.

## R10. Summarizer contract, batch composition, egress boundary

- **Decision**: One JSON contract (`contracts/observer.md`). A session batch is split by
  destination after classification; batch identity is (session, through event, destination), so a
  remote batch and a fallback batch over the same range coexist with disjoint rows. **One request builder applies
  `destination_rules` to every field of an outbound request**: events, free summaries, nearby
  candidates (remote receives only `eligible` memories), citations, and repository metadata (the
  remote observer receives an opaque repository id, never the normalized remote or path). The
  session summary is never a provider output: the worker derives it deterministically at session
  end from the session's rows and the observations just applied (rules in `contracts/observer.md`),
  so `sessions.latest_summary_memory_id` has one source and session end costs one provider call. Classification candidates are the top
  8 same-repository memories from R5 (including tombstones); a provider decision's `target` must
  be one of the supplied ids or it is treated as `add`; the fallback rule is tombstone hash →
  suppressed, active hash → `noop`, else `add`. Fallback bodies are deterministic records (file
  lists, commit ids, error lines, tool names), never raw paragraphs; verbatim tool output is
  retained for 7 days as raw evidence (`raw_events`) and is never injected or indexed. **Agent
  neutrality**: the producing agent is provenance only; it is absent from provider inputs,
  fallback bodies, classification decisions, and `material_hash`, and a test asserts identical
  hashes and decisions when only the agent changes. **Deleted content** (FR-035 "not re-created from the same content"): the plan reads "same
  content" as the same (type, normalized title, normalized body), which is `material_hash`,
  enforced by the tombstone row
  plus the tombstone-aware classification prompt (top 8 nearby includes tombstones); a paraphrase
  of a deleted memory is a new memory by this reading. A source-attribution guard was designed and
  withdrawn because it suppressed correct sibling memories from the same events; the reading is
  recorded for owner confirmation as A13. **Sensitivity on add/update** = max(target's
  sensitivity, every source row, detector result), fixed in the apply transaction. **Language**
  (FR-014): script mismatch → one retry → fallback with `language_mismatch`. Degraded reasons: `no_provider`, `unreachable`,
  `unusable_output`, `language_mismatch`, `daily_cap`, `provider_exhausted`, `provider_paid`,
  `auth_failed`, `consent_changed`, `model_alias`, `timeout`, `rule_based`, `window_unknown`
  (session summaries aggregate by the fixed precedence in `contracts/observer.md`).
- **Reviewer changes**: nearby and repo metadata leak closed; batch identity by destination only;
  deterministic session summary with durable reconciliation; fallback bodies as records under a
  2,000-character budget; injection of verbatim output removed; single preset.

## R11. Fixture, measurement, tests, isolated E2E

- **Decision**: `node:test` on compiled tests; JSONL fixture of native hook payloads replayed through
  the real bundle from synthetic repositories; seeded facts, Japanese/English pairs, synthetic secret
  corpus, a malicious-directive corpus (`ignore previous instructions` and variants) whose phrases are
  legitimately present in raw events and the spool but must be absent from accepted observer
  output, stored memories, and packs (the secret corpus is asserted absent everywhere: database,
  spool, logs, packs, outbound bodies), large-event cases at the R4 cap, detector-throw and malformed `.oboete.toml` cases;
  p99 of capture hooks; session-start wait measured on ready and pending paths; worker peak RSS via
  `process.resourceUsage().maxRSS`; database growth. Failure matrix: db-missing, busy, corrupt,
  read-only home, ENOSPC, worker kill, provider unreachable/hang/429-3036/403-5035/length/malformed,
  retry at the cap boundary (149 → 150), lease loss after 3036, Pi throw and child hang, clock jump,
  mixed-sensitivity batch (assert the outbound body), tombstone round trip, resume/compact/fork/
  clear, setup repeat/remove, lease steal, pause, Grok success/execution-failure/deny/all-denied/
  no-tool (assert the number of packs the model received), Pi spawn failure. Migration smoke test on empty and
  previous-version databases on 22.16 and 24.x. Installed size by installing the tarball into an
  empty prefix. **Every probe and E2E run, including the R13 contract probes, runs under the
  isolated `oboete-dogfood` user with its own `HOME`** (Grok Build resolves Claude-compat hooks from
  `$HOME`, so a temporary `--home` is not isolation). Pi 0.84.4 requires Node >= 22.19: engine
  floor 22.16 in CI, agents on 24.x.
- **Reviewer changes**: isolation by user, not by temp dir; the additional matrix rows.

## R12. Remaining decisions

- **Injection marker and framing**: packs start with `oboete memory context` and end with
  `end of oboete memory context` (labels); bodies are summarizer output or deterministic records,
  never verbatim tool output; a directive-corpus test runs on both summarizer outputs and on packs.
- **Allowance**: `provider_usage` per (utc_day, preset) with `reset_at`; per-attempt reservations.
- **Free-model catalog**: `GET /accounts/{id}/ai/models/search` once per worker run, 24-hour cache.
- **Citation staleness**: every pack builder checks paths with `fs.existsSync` and commits through
  the worker's `HEAD`-keyed ancestor cache (`git merge-base --is-ancestor`, batched), invalidated
  when `HEAD` changes.
- **Pause**: `~/.oboete/paused` checked before the database is opened.
- **Setup probes**: one headless invocation per selected agent in parallel under a 90 s deadline,
  asserting a probe event; trust state reported per agent.
- **Context window**: a versioned table `docs/research/context-windows.md` (model id → documented
  window, source URL) maintained under R13; the character budget uses the documented window of the
  reported model; when the model is unknown the budget is min(channel cap, the smallest verified
  window in the table for that agent) and the pack is labelled `window_unknown`; an agent with no
  verified entry at all cannot satisfy FR-025, so its prompt-submit lane is blocked by the R13
  gate until a window is verified (no omission fallback, no guessed value).
- **Content identity**: one shared helper (`db/identity.ts`) is the only place that computes
  `material_hash` = sha256(normalized title, normalized body) (type excluded, A13) and `content_hash` =
  sha256(repo_id, material_hash); provider output, fallback output, import, and tombstones all go
  through it, and a contract test asserts equal hashes for the same input on every path.
  Tombstones keep both.
- **Export/import**: `oboete-export/1` JSONL; each line carries `material_hash`, `content_hash`,
  `source_session_id`, `source_batch_id`, provenance, and for tombstones an empty body; import
  validates per line (64 KB) and file (256 MB), verifies `material_hash` against the body when one
  is present (mismatch rejects the line), recomputes `content_hash` from the local (possibly
  `--map-repo`-mapped) repository id and `material_hash` for active rows and tombstones alike,
  recomputes ids and `cjk_bigrams`, unions on `content_hash`, applies the sensitivity lattice
  `secret > private > local_only > eligible` (the stricter wins), lets tombstones win over active
  rows, and inserts every active imported row as `local_only` with `review_state = imported`.
  **Quarantine**: the shared query function excludes `review_state = imported` from search,
  injection, MCP, and the viewer's injectable set; the worker runs the detector and the
  directive check on each imported row and moves it to `unreviewed` (or `secret`), so nothing
  imported reaches a pack before classification.
- **Pi diagnostics**: FR-007 forbids in-process storage work and also requires every thrown
  error to be recorded; without a durable error surface owned by Pi those two clauses cannot both
  hold, so the plan records this as amendment A8 (task 0). Design: the extension does no file or
  network work; it try/catches, generates an `invocation_id` per spawn, spawns bounded children
  with `--invocation <id>`, and keeps in-memory failure counters (message code only, never payload
  or exception text) that it hands to the next child as `--prior-failures`. The capture child
  writes `~/.oboete/spool/pi-ack/<invocation_id>.started` before reading stdin and renames it to
  `.done` on success. Doctor reports `pi_child_hang` from `.started` files older than 30 s,
  `pi_child_failed` and prior-failure counters from diagnostics rows, and `pi_spawn_failed` from
  its own headless wiring probe. The R13 probe checks whether Pi logs extension errors durably:
  if it does, doctor reads that surface and every error is recorded as FR-007 states; if it does
  not, the Pi lane is blocked until the owner approves A8 (FR-007 recording = next-spawn counters
  plus the doctor probe, best effort for a failure that stops every later spawn). The worker
  deletes `.done` files after folding them and `.started` files 24 h after recording the hang.
- **Security-owned modules** (implemented by Claude Code, never delegated): `privacy/*`,
  `capture.ts`, `config.ts`, `repo-identity.ts`, `setup/managed-block.ts`, `setup/consent.ts`,
  `setup/write-*.ts`, `db/queries.ts` (scope and sensitivity filter), `worker/batches.ts`
  (destination split), `observer/request.ts` (outbound request builder), `observer/classify.ts`
  (target restriction, update/delete), `injection/*`, `mcp.ts` (repository boundary),
  `transfer.ts`, `viewer/server.ts` (auth), and `scripts/build.mjs` (bundle composition). `agents/*.ts` is also
  security-owned: adapters extract content and paths from raw payloads and therefore decide what
  the path rules and the detector see. External lanes get UI components, fixture generators,
  retrieval scoring, and test scaffolding only; `tasks.md` lists the owned paths as a fence on
  every delegated task.

## R13. Verification gate and its failure policy

Third-party contracts must be verified and recorded before a plan depends on them. Each item
below is probed under the isolated user before dependent code is written; the result is appended
to `docs/research/`. **A probe that fails does not switch to a non-compliant fallback: the affected
item is blocked and the owner decides on a spec amendment or a scope change.** Probe scaffolding
may be built first; the bundle cold-start and installed-size measurements run after the minimal
skeleton (task 1).

| item | probe | if the probe fails |
|---|---|---|
| Native tool payload shapes for read/write/edit/bash on all four agents | capture fixtures from headless runs | blocked for that agent/tool: until its fixture exists the adapter stores metadata only (`classification_state = failed`, reason `unmapped_payload`), never an unmapped payload (path rules cannot be applied to unknown fields); a regression test plants a path-rule secret inside an unknown payload |
| Codex and Grok `PostCompact` payload (summary text field); Grok `Stop` `lastAssistantMessage` field | forced compaction and a normal turn end per agent | if no text is provided: recorded as such, `compaction_summary` / `last_assistant_message` absent for that agent by contract test; FR-010 input then relies on captured events only |
| Grok parallel batches: whether `additionalContext` attached to several calls of one batch reaches the model once or once per call; `PermissionDenied` payload | parallel batch under the isolated user with a pack attached to two calls | once per batch: pass. Once per call, or a denied call suppressing the other calls' context: **blocked** for the Grok lane until the owner decides A15 (accept counted duplicates or exclude parallel-batch delivery from M1) |
| Compaction identity and order per agent. Pass conditions: (a) the authoritative compaction hook (`PostCompact`; Pi's compaction event) carries a value that distinguishes two distinct compactions, including byte-identical ones in the same turn, from a re-delivery (a native compaction id, or a monotonic counter or timestamp in the payload); (b) that hook completes, and the epoch advance is committed, before the agent runs any injection hook for the post-compaction context (`SessionStart source = compact`, `UserPromptSubmit`) | two byte-identical compactions in one turn, one re-delivered hook, and hook-order capture per agent | blocked for that agent's compaction re-injection (FR-024) until the owner decides A16: accept the `raw_events.id` key (collapses byte-identical same-turn compactions) and/or a documented ordering limit, or exclude compaction re-injection for that agent from M1 |
| Detector: the full detector finishes a 1 MB payload inside the capture hard cutoff on Node 22.16 | replay with 1 MB inputs | blocked: the capture lane stops and the measured bound goes to the owner as A14 (no silent smaller bound) |
| Codex `SessionStart` fires with `source = compact` and `clear` | forced compaction and `/clear` in headless runs | blocked: FR-024 cannot be met on Codex without an owner amendment |
| Codex rollout flush at `PostToolUse`; TUI trust path | grep the just-completed `tool_use_id`; interactive trust run | capture from hook stdin only (rollout is never a required source) |
| Grok Build user-scoped MCP registration | write configuration, call `search` headless | blocked for Grok's tool surface (FR-030) pending owner amendment |
| Pi compaction event; Pi tool registration surface | extension probe on 0.84.4 | compaction re-injection on Pi blocked pending amendment; tools blocked likewise |
| NIM / OpenRouter / Gemini transport, auth header, model id (Anthropic removed 2026-09-04, A19) | one call per preset | blocked: the preset is listed by the constitution, so M1 completion is blocked until the row passes or the owner approves an exception removing that preset |
| NIM / OpenRouter / Gemini structured output (`response_format`) | same call, schema requested | text-JSON path for that preset (already compliant) |
| Grok Build `PreToolUse` context on an executed-but-failed call (`PostToolUseFailure`) | force a failing tool call with an attempted pack; check what the model received | if dropped: the attempt is marked `delivery = dropped`, the record stays non-emitted, and the next `PreToolUse` attaches the pack again; if delivered: `PostToolUseFailure` confirms delivery |
| Pi `resume` / `fork`: `session_start` firing and `PI_SESSION_ID` continuity | resume and fork a session under the isolated user | if the id does not continue: Pi resume detection is blocked and FR-024/FR-026 on Pi resume go to an owner amendment |
| Grok Build resume: `SessionStart` `source` value and session id continuity | resume a headless session | if not continuous: Grok resume detection is blocked and FR-024/FR-026 on Grok resume go to an owner amendment |
| Hook runner behaviour when the hook exits with unread stdin above 1 MB (all four agents) | feed an oversized tool result | blocked: A7 goes to the owner if a runner treats the hook as failed; a runner's own payload cap narrows A7 |
| Pi durable error surface for extension throws | throw inside a probe extension, inspect Pi's logs | blocked for the Pi lane pending amendment A8 |
| Legacy-era MCP server against Claude Code, Codex, Grok clients (raw frames compared) | headless `tools/list` + `tools/call` | blocked for that client pending amendment |
| `agent-cli` preset: headless JSON output of `claude -p`, `codex exec`, `grok -p` for a summarization prompt | one call per CLI under the isolated user | the preset is disabled for that CLI (optional preset, no completion block) |
| Per-model context windows | documented window per model id into `context-windows.md` | an agent with no verified window blocks its prompt-submit lane (FR-025) pending owner decision; a known agent with an unknown model uses the smallest verified window and `window_unknown` |
| Real bundle cold start on 22.16 and 24.x (after task 1) | replay harness | blocked; a split entry point needs a constitution amendment first |
| Installed size with dependencies (after task 1) | tarball into an empty prefix | blocked above 30 MB pending a written reason approved by the owner |
