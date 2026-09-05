# Tasks: oboete M1 Self-Use Alpha

**Input**: Design documents from `/specs/007-oboete-m1-alpha/` (plan.md, spec.md, research.md,
data-model.md, contracts/{cli,agents,observer,mcp}.md, quickstart.md)

**Tests**: The specification mandates tests (FR-023 both-direction privacy tests, FR-040 fixture
evidence, R11 failure matrix, R13 probes), so test tasks are included where the spec or plan
requires them. Tests are written before the code they gate (red first).

**Organization**: Phase 1 = amendments and verification gate (task 0 of the plan), Phase 2 =
foundation and the capture/privacy/worker/retrieval kernel every story needs, then one phase per
user story in priority order, then evidence and polish.

**Execution lanes** (plan.md "Structure Decision"): tasks tagged `[CC]` touch security-owned
paths (`src/privacy/*`, `src/capture.ts`, `src/config.ts`, `src/repo-identity.ts`,
`src/setup/managed-block.ts`, `src/setup/consent.ts`, `src/setup/write-*.ts`, `src/db/queries.ts`,
`src/worker/batches.ts`, `src/observer/request.ts`, `src/observer/classify.ts`,
`src/injection/*`, `src/mcp.ts`, `src/transfer.ts`, `src/viewer/server.ts`, `scripts/build.mjs`,
`src/agents/*.ts`) or the viewer frontend and are implemented by Claude Code. Tasks tagged
`[EXT]` are delegated to Grok Build (declared file scope) or Codex per `rules/coding.md`; a
delegated result that touches a security-owned path is returned to Claude Code. Every task's file
list is its scope fence.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: User story label for story phases only (US1..US7)
- File paths are repository-relative

## Path Conventions

Single package at the repository root: `src/`, `test/`, `scripts/`, `docs/` (plan.md "Source
Code"). Node >= 22.16; tests compile to `build/test/*.mjs`.

---

## Phase 1: Amendments and verification gate (plan delivery order 0)

**Purpose**: Owner decisions on the constitution and spec amendments, and the third-party probes
that need no oboete code. A failed probe blocks its dependents (research.md R13) instead of
switching to a fallback.

- [X] T001 Record the owner's decision on A1-A7 and A9-A13 (plan.md "Amendments and spec corrections") in docs/research/m1-amendments-2026-09.md and apply approved PATCH/MINOR edits to CONSTITUTION.md, .specify/memory/constitution.md, and specs/007-oboete-m1-alpha/spec.md with a Sync Impact Report (done 2026-09-03: constitution 3.1.0; decisions delegated by the owner under the "better than claude-mem" criterion)
- [X] T002 [P] [EXT] Create the isolated `oboete-dogfood` Linux user with its own HOME and logins for Claude Code, Codex CLI, Grok Build, and Pi, documented in docs/research/isolated-user-setup.md (no oboete installed in the maintainer's environment, FR-041) (done 2026-09-03 by the owner; verified headless: Claude Code 2.1.259, Codex CLI 0.153.0, Grok Build 1.0.17 alpha, Pi 0.84.4, Node 24.20; `~/.oboete-credentials` still empty)
- [X] T003 [P] [EXT] Write the probe harness scripts/e2e/probe-contracts.mjs that runs each R13 row under the isolated user and appends a dated section to docs/research/oboete-contracts-probes.md (done 2026-09-03: runner + scripts/e2e/probe-lib/ + scripts/e2e/probes/ + scripts/e2e/dogfood.sh; Grok Build implementation, reviewed)
- [X] T004 [EXT] Run the payload-shape probes (read/write/edit/bash on all four agents) and commit fixtures to test/contracts/{claude,codex,grok,pi}/*.json (R13 row 1) (done 2026-09-03: 16 fixtures, run 2026-09-03T10-22-04-666Z)
- [X] T005 [P] [EXT] Run the Codex probes: SessionStart source = compact and clear, rollout flush at PostToolUse, TUI trust path, PostCompact payload; record in docs/research/oboete-contracts-probes.md (done 2026-09-03: `codex-session-start-sources` fail on `clear` only, `compact` and `resume` verified, `/new` fires no SessionStart → A18; rollout flush pass; TUI trust path pass with trusted_hash rows; PostCompact has no summary field, identity fail → A16 default)
- [X] T006 [P] [EXT] Run the Grok Build probes: user-scoped MCP registration, PreToolUse context on a failed call, parallel-batch delivery once-per-batch vs once-per-call, PermissionDenied payload, PostCompact payload and Stop lastAssistantMessage, resume SessionStart source and session id continuity; record results (done 2026-09-03: MCP pass; failed call delivers context via PostToolUse `exit_code`; parallel batch delivers once per call → A15 default; PermissionDenied payload captured (permission-rule deny only); PostCompact has no summary field, identity and order pass (`timestamp`); resume `source = load` with id continuity)
- [X] T007 [P] [EXT] Run the Pi probes: compaction event, tool registration surface, resume/fork PI_SESSION_ID continuity, durable error surface for extension throws; record results (done 2026-09-03: `session_compact` with distinct `compactionEntry.id` and correct order; `pi.registerTool` tool called and echoed; resume keeps the id, fork changes it, bash child's `PI_SESSION_ID` equals the extension's, `session_start.reason` always `startup`; extension throw reaches stderr only → A8 applied to FR-007)
- [X] T008 [P] [EXT] Run the provider probes for nim, openrouter, gemini, anthropic: transport, auth header, model id (blocking rows) and response_format support (text-JSON fallback row), plus the `agent-cli` headless JSON probe for `claude -p`, `codex exec`, `grok -p`; record results (done 2026-09-04: `agent-cli-json` passes for all four CLIs; provider rows pass for nim, openrouter, gemini, workers-ai with `OBOETE_<PRESET>_API_KEY` from `~/.oboete-credentials` (run 2026-09-03T16-48-21-133Z); `response_format` honoured by OpenRouter and Workers AI, text-JSON path for NIM and Gemini; NIM probe model moved to `meta/llama-3.2-11b-vision-instruct` because `meta/llama-3.1-8b-instruct` reached end of life on 2026-08-26; anthropic preset removed by the owner on 2026-09-04, A19, so no row remains blocked)
- [X] T009 [P] [EXT] Run the hook-runner probe: each agent's behaviour when the hook exits with unread stdin above 1 MB; record results (done 2026-09-03 for the three hook runners: Claude Code, Codex, Grok Build tolerate an unread hook; runners cap the delivered payload at about 31 KB / 5 KB / 165-190 KB. Pi has no hook process, so this row does not apply to it; the in-process equivalent is oboete's capture child, covered by T051/T061)
- [X] T010 [P] [EXT] Build docs/research/context-windows.md (model id → documented window, source URL) for every model each agent reports in the probes (done 2026-09-03: 11 verified rows, runtime-id → catalog-id rules, adversarially verified)
- [X] T011 Evaluate every R13 row against its pass condition, list blocked lanes, and record conditional decisions A8, A14, A15, A16 with the owner in docs/research/m1-amendments-2026-09.md (done 2026-09-03: evaluation table in docs/research/oboete-contracts-probes.md "R13 evaluation"; A8, A15, A16 triggered with defaults applied, A14 pending the detector, A18 added for Codex `/new`; blocked lanes = provider presets without keys, detector, bundle cold start and size, legacy MCP against Claude Code)

**Checkpoint**: amendments recorded; every R13 row is pass or explicitly blocked.

---

## Phase 2: Foundation and kernel (plan delivery order 1-4)

**Purpose**: package, build, schema, capture path with the privacy boundary, worker, observer,
retrieval and the shared pack builder. Every user story depends on this phase.

### Package and build

- [X] T012 [EXT] Create package.json (name oboete, bin, engines >= 22.16, scripts build/typecheck/lint/test/pack-check), tsconfig.json, eslint.config.js, .npmignore, and the src/ and test/ directory skeleton from plan.md "Source Code" (done 2026-09-04: Grok Build; exact pins, no pack-check script yet (T087); files whitelist instead of .npmignore)
- [X] T013 [CC] Write scripts/build.mjs: esbuild ESM bundle dist/oboete.mjs with hook-path packages bundled (zod, smol-toml, @secretlint/core, preset) and ai, @ai-sdk/*, workers-ai-provider, hono, @hono/node-server, preact external; migrations embedded as text; tests compiled to build/test; viewer assets embedded (done 2026-09-04: esbuild single file, hook-path packages bundled, heavy packages external + dynamic import, tests compiled to build/test; viewer assets step deferred to T079)
- [X] T014 [P] [EXT] Add the CI workflow matrix (Node 22.16 and 24.x) running typecheck, lint, build, test, pack-check in .github/workflows/ci.yml (done 2026-09-04: engine matrix 22.16.0 / 24.x + coverage lcov; sonar.sources still scripts (T088); the pack-check step waits for the T087 script)
- [X] T015 [EXT] Measure real bundle cold start on 22.16 and 24.x and installed size in an empty prefix (R13 rows "cold start" and "installed size") and record in docs/evidence/m1-resource-envelope.md; block if over 100 ms or 30 MB (done 2026-09-04: docs/evidence/m1-resource-envelope.md + scripts/measure-cold-start.mjs; --version max 60 ms, hook max 252 ms, installed size 32.7 MB at a9b51fb then 29.15 MB once the bundled packages became devDependencies (both R13 rows pass))

### Storage

- [X] T016 [EXT] Write migrations src/db/migrations/0001_core.sql (schema_migrations, repos, sessions incl. conversation_id/context_epoch/last_compaction_key/summary_state, turns, raw_events, observation_batches, worker_lease seeded row, runtime_state, diagnostics) per data-model.md (done 2026-09-04: Grok Build)
- [X] T017 [P] [EXT] Write src/db/migrations/0002_memory_search.sql (memories with material_hash/content_hash/review_state, memory_sources, memories_fts trigram, memories_fts_cjk, triggers, destination_rules seeded) per data-model.md (done 2026-09-04: Grok Build; FTS triggers narrowed to UPDATE OF the indexed columns (review))
- [X] T018 [P] [EXT] Write src/db/migrations/0003_operations.sql (injections incl. context_epoch/attempts_json/delivery_count, injection_items with the partial unique index on (conversation_id, context_epoch, memory_id), provider_usage, sync_conflicts) per data-model.md (done 2026-09-04: Grok Build; injection_items.reason gained secret_detected and directive (consolidation))
- [X] T019 [EXT] Implement src/db/open.ts: DatabaseSync with explicit timeout, WAL, wal_autocheckpoint = 0 for hooks, forward-only migration runner with sha256 check, hook spools when user_version is behind (done 2026-09-04: Grok Build; hook role never migrates, sha256 mismatch errors)
- [X] T020 [P] [EXT] Write migration smoke tests test/migrations/apply.test.ts on an empty database and on test/fixtures/previous-version.db for both Node versions (done 2026-09-04: Grok Build; fixture test/fixtures/previous-version.db at version 1)
- [X] T021 [CC] Implement src/db/identity.ts (material_hash, content_hash) and src/db/queries.ts: the shared scope + sensitivity + review_state filter used by injection, CLI, MCP, and viewer, with tests test/unit/queries.test.ts (cross-repository and secret rows excluded, imported rows quarantined) (done 2026-09-04: memoryScope reads destination_rules live; nearbyCandidates added by T038 (bm25 + rrf, no threshold))

### Configuration, identity, paths

- [X] T022 [CC] Implement src/paths.ts and src/config.ts: OBOETE_HOME, config.toml via smol-toml, .oboete.toml path rules, credentials only from OBOETE_* variables, consent record (hash of preset/host/credential source/cost class/egress classes), paused marker check before opening the database; tests test/unit/config.test.ts (done 2026-09-04: PRESET_CATALOG without anthropic (A19); credentials only from OBOETE_* env (A17); repository secret_paths bounded to 64 rules of 256 characters (review finding 14))
- [X] T023 [P] [CC] Implement src/repo-identity.ts: normalized remote (userinfo, query, fragment removed) or realpath of git rev-parse --git-common-dir, sha256 prefix id; tests test/unit/repo-identity.test.ts including a credential-bearing remote URL (done 2026-09-04: at most two git calls with an origin, per-call timeout bounded by the remaining budget, Windows drive remotes are local paths (review))

### Capture path and privacy boundary

- [X] T024 [CC] Write red tests first in test/unit/privacy.test.ts: fail-closed (secret corpus test/corpus/secrets.jsonl redacted before any write, `<private>` stripped incl. unclosed tag, path-rule hits stored as metadata only, detector throw and malformed .oboete.toml → classification_state = failed, stdin above 1 MB → redacted partial row marked truncated, A7) and fail-open (eligible content stored whole) (done 2026-09-04: corpus test/corpus/secrets.jsonl (37 lines); stdin bound clause moved to T027 with the A14 value)
- [X] T025 [CC] Implement src/privacy/detect.ts: private strip, path rules, @secretlint/core lintSource with the statically imported recommend preset, gated entropy, `[REDACTED:<rule>]`, run inside a worker_threads Worker with a hard cutoff (done 2026-09-04: secretlint rules registered individually, gated entropy, worker_threads cutoff; per-field redaction (DetectorInput.fields) for T027; repository glob rules compiled by a linear token sweep (compileGlob) after the review found the RegExp form hangs on three wildcards)
- [X] T026 [CC] Implement src/events.ts: zod discriminated union of normalized events, event id derivation (kind in every key, no delivery counter), conversation id rules (resume keeps the root, fork and Grok new start one) (done 2026-09-04: prompt_id optional on lifecycle events and eventId(event, turnOrdinal) after review; residual limits in the eventIdKey comment and contracts/agents.md)
- [X] T027 [CC] Implement src/capture.ts: absolute deadline with spool reserve, 1 MB stdin bound, detector before the first write, insert into raw_events or spool file (write-then-rename), busy timeout min(150 ms, remaining − reserve), worker spawn when the lease is free; wire src/cli.ts `hook --agent <selector> --event <name>`, `capture --agent pi --invocation <id>` (done 2026-09-04: stdin bound 256 KiB per A14 (not 1 MB; secret-dense detection measured 406-665 ms/MB); spool entry = {repo, session, row} owned by src/spool.ts; injection branches wired by T046; hook exits 0 when the data directory cannot be written, git only gets what is left after the detector's 60 ms slice, either order of Claude Code's two compaction hooks advances the epoch once, a resumed session is reopened (review findings 1, 3, 8, 9, 13))
- [X] T028 [CC] Implement src/agents/claude.ts, codex.ts, grok.ts, pi.ts payload mapping from the T004 fixtures (tool names, content fields, paths) plus the bounded prefix scan for partial rows; an agent/tool without a fixture stores metadata only with reason unmapped_payload; tests test/unit/agents.test.ts plant a path-rule secret inside an unknown payload (done 2026-09-04: adapt() cross-checks the payload hook name; scanPartialPrefix reads escaped Windows paths (review))
- [X] T029 [CC] Implement src/privacy/egress.ts (destination_rules evaluation) and src/privacy/classify.ts (worker promotion local_only → eligible after detector and entropy checks); tests in test/unit/privacy.test.ts assert identical decisions when only the producing agent changes (SC-006) (done 2026-09-04: promoteSensitivity / strictest; egress filter without an agent argument)

### Worker and observer

- [X] T030 [EXT] Implement src/worker/lease.ts: claim/heartbeat/release of the seeded worker_lease row in BEGIN IMMEDIATE transactions fenced by owner_token, staleness (6 s old or 60 s in the future), atomic release with the empty-queue check; tests test/unit/lease.test.ts (lease steal, clock jump) (done 2026-09-04: Grok Build; STALE 6 s / FUTURE 60 s)
- [X] T031 [CC] Implement src/worker/batches.ts: spool recovery, batch creation per (session, through event, destination) after classification, ten-turn and session-end triggers, retention-forced batches, reclaim after 120 s; tests test/unit/batches.test.ts assert a mixed-sensitivity session yields disjoint remote and fallback batches (done 2026-09-04: recovery INSERT OR IGNORE on the hook-computed id, quarantine to paths.spoolFailed, ten_turns = ten distinct turns with unbatched summarizable rows, retention rows forced to the fallback; recovery derives the turn of a spooled row itself and quarantines an entry the database refuses, and a second batching round over a finished range carries its own through key (review findings 2, 7))
- [X] T032 [P] [EXT] Implement src/worker/purge.ts: bounded deletes of expired raw_events in applied/fallback batches, forced fallback for expired pending rows, pi-ack cleanup, PASSIVE/TRUNCATE checkpoints; tests test/unit/purge.test.ts (done 2026-09-04: Grok Build; checkpoint PASSIVE per batch / TRUNCATE on release; expired unbatched rows no summarizer can use are purged too (review finding 5))
- [X] T033 [EXT] Implement src/observer/contract.ts: shared zod schema for observer input and output (observations ≤ 20, source_event_ids subset ≤ 50, citation strings ≤ 512, ≤ 20 paths and 10 commits, titles 120, bodies 2,000) per contracts/observer.md (done 2026-09-04: Grok Build; lenient parse -> trim -> strict validate, excerptInput bounded to 12,000 chars with a last-resort pass (review))
- [X] T034 [CC] Implement src/observer/request.ts: the single outbound request builder that applies destination_rules to events, free_summaries, nearby (eligible only for remote), citations, and repo_ref (opaque id), and re-checks the consent hash before reservation and before send; tests test/unit/request.test.ts assert the actual outbound body of a mixed batch (done 2026-09-04: single builder; events, free summaries, nearby candidates and citations pass one egress check; consent check = config.consentMatches; tool_call rows hand their command or edit text back through toolInputOf (review finding 4))
- [X] T035 [P] [EXT] Implement src/observer/providers.ts and src/observer/llm.ts: presets workers-ai (workers-ai-provider REST, JSON schema), ollama/nim/openrouter/gemini via @ai-sdk/openai-compatible (no anthropic preset, A19) (response_format or text-JSON), optional agent-cli (child process `claude -p` / `codex exec` / `grok -p`, text-JSON, uncapped), maxRetries 0, 60 s abort, 1 MB response cap, status + body-code classification table (3036, 5035, 401/403 auth_failed, 3007/3040 retry once), model id check, neuron estimate, single enabled preset (done 2026-09-04: Codex; workers-ai / nim / openrouter / gemini / ollama / agent-cli, maxRetries 0, 1 MB response cap on the HTTP envelope)
- [X] T036 [EXT] Implement per-attempt reservations and the daily cap in src/observer/llm.ts + src/worker/observe.ts: BEGIN IMMEDIATE check of the 150-per-UTC-day sum across capped presets and exhausted_at, session-end reservation when 10 or fewer calls remain, provider_usage.calls and provider_attempts increments, unfenced monotonic exhausted_at write on 3036, fenced apply in one transaction with state = applied; tests test/unit/callpolicy.test.ts (attempt 150 allowed, 151 refused; lease lost after 3036; worker kill after response → 2 calls, 1 apply) (done 2026-09-04: Codex; lives in src/observer/reservation.ts (not observe.ts): reserveAttempt / recordExhausted / usageEstimate, DAILY_CAP 150, SESSION_END_RESERVE 10)
- [X] T037 [P] [EXT] Implement src/observer/fallback.ts: the deterministic record rules table (change, bugfix, discovery, decision), source_event_ids by rule, output budget and trim order, no agent name anywhere; tests test/unit/fallback.test.ts (long paths, 60 tool calls, language of copied text) (done 2026-09-04: Codex; fallbackObserve with suppressed observations)
- [X] T038 [CC] Implement src/observer/classify.ts: target restricted to supplied nearby ids of the same repository, tombstone-aware suppression by content_hash, sensitivity on add = strictest source + detector and on update = max(target, sources, detector) in the apply transaction, delete only with a reason, directive-corpus rejection (test/corpus/directives.jsonl), language check with one retry then language_mismatch; tests test/unit/classify.test.ts (eligible update never relaxes a local-only target; English output for Japanese input) (done 2026-09-04: applyObservations in one fenced transaction, directive corpus test/corpus/directives.jsonl, deterministic sessionSummary (partial rows give paths only))
- [X] T039 [EXT] Implement src/worker/observe.ts: worker main loop (recover spool, claim lease, purge, batch, classify, summarize, apply, deterministic session summary with reconciliation of summary_state = pending, no_content sessions, degraded precedence, checkpoint, release), exit codes per contracts/cli.md; tests test/unit/observe.test.ts (session_start + fully private prompt + session_end produces no memory) (done 2026-09-04: Codex; runObserve with injectable deps, exit 0 / 1 (fallback used) / 3 (storage); catalog result stored outside the lease fence (follow-up); a constraint violation ends the run as a worker error rather than exit 3, and the queue check is an indexed probe (review findings 2, 5))
- [X] T040 [P] [EXT] Implement src/observer/catalog.ts: Workers AI model search once per run with a 24-hour cache in runtime_state, paid-plan flag for doctor (done 2026-09-04: Codex; 24 h cache keyed to the account id, paidPlan from require_workers_paid)

### Retrieval and injection

- [X] T041 [EXT] Implement src/retrieval/fts.ts and src/retrieval/query.ts: Intl.Segmenter routing, trigram terms ≥ 3 chars, CJK bigrams (ー 々 ・ included), particle drop, LIKE only when no indexed term, BM25 normalization; tests test/unit/retrieval.test.ts with seeded Japanese and English facts (done 2026-09-04: Grok Build, then OR-joined terms, stop words, no single-char CJK term, 128-term cap by length (two review rounds))
- [X] T042 [P] [EXT] Implement src/retrieval/rank.ts: normalized-BM25 threshold (default 0.3), RRF k = 60 across tables (LIKE never votes), MMR lambda 0.5 with character-trigram cosine, character budget cut; tests in test/unit/retrieval.test.ts (done 2026-09-04: Grok Build)
- [X] T043 [CC] Implement src/injection/budget.ts: character budget = min(channel cap, context_fraction × documented window from docs/research/context-windows.md), window_unknown for unknown models, lane blocked for agents without any verified window; tests test/unit/budget.test.ts (done 2026-09-04: alias rules derived from docs/research/context-windows.md table rows)
- [X] T044 [CC] Implement src/injection/staleness.ts (fs.existsSync paths, HEAD-keyed ancestor cache for commits) and src/injection/pack.ts: the common pack builder (labels, every content line `> `-framed and single-line canonicalized, no agent name, plain-language `degraded:` sentences mapped from reason codes, whole-pack validation with secret detector, directive corpus, control characters), ledger rows planned → included on delivery, per-epoch uniqueness, session-start pack once per epoch, summary-pending wait of 8 s then raw activity; tests test/unit/pack.test.ts (malicious title, path, remote URL; `{` guard; stale path and commit) (done 2026-09-04: session-start wait 1 s per A2 (the 8 s in this line is stale), whole-pack detector + directive corpus + control characters, --end-of-options before commit citations; the pack reads the worker's citations_head/citations_ok and asks git for HEAD at most once inside the hook budget (review finding 10))
- [X] T045 [CC] Implement src/injection/deferred.ts: the Grok state machine (pending record merge, attach on every PreToolUse until confirmed, attempts_json execution/delivery updates from PostToolUse, PostToolUseFailure, PermissionDenied, Stop in single-row BEGIN IMMEDIATE transactions, delivery_count per probe result, omitted reasons no_tool_call / not_delivered); tests test/unit/deferred.test.ts counting packs the model received in success, execution-failure, oboete-deny, other-handler-deny, parallel-batch, no-tool cases (done 2026-09-04: pending pack in runtime_state per conversation; merged packs validated as a whole (consolidation); a delivered pack ignores later unattached calls, a deny or Stop after delivery still records its attempt, and the record plus its items are one transaction with delivery marking only rendered items (review findings 6, 11))
- [X] T046 [EXT] Implement `oboete inject --agent pi --kind start|prompt` and the injection branches of `oboete hook` in src/cli.ts using src/injection/*, plain stdout output rules per contracts/agents.md (done 2026-09-04: Codex; Claude plain stdout, Codex hookSpecificOutput.additionalContext (A18 merged pack on a new session id), Grok deferred machine on PreToolUse/PostToolUse/failure/deny/Stop, Pi via runInject; injection deadline 1,300 ms; pack directive gate uses the observer's normalized rejectsDirectives (review finding 12))

**Checkpoint**: unit suites green on 22.16 and 24.x; replay skeleton runs one event end to end.

---

## Phase 3: User Story 1 - Memory follows the developer across agents (Priority: P1) 🎯 MVP

**Goal**: a session in any of the four agents receives the previous session's summary, pinned
memories, and prompt-relevant memories of the same repository, marked as oboete's.

**Independent Test**: scripts/e2e/isolated-user.mjs --pairs all seeds three facts with agent A and
asserts agent B's first turn contains all three for 12 of 12 ordered pairs (Grok by the first tool
result).

- [X] T047 [CC] [US1] Implement src/setup/managed-block.ts: managed blocks for TOML (`# oboete:begin/end`) and JSON (`"oboete": true` handlers), backup preserving mode/owner (0600 for credential-bearing files), temp file → re-parse → rename; tests test/unit/managed-block.test.ts (repeat leaves files byte-identical, remove restores) (done 2026-09-04: applyTomlBlock/applyJsonHandlers/removeJsonHandlers with backup then exclusive temp file then re-parse then rename; 0600 backups for credential-bearing files; symbolic links leaving the directory refused)
- [X] T048 [P] [CC] [US1] Implement src/setup/write-claude.ts: handlers for SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop, PostCompact, SessionEnd with `--agent claude-or-grok --event <name>`, timeouts 12 s / 3 s, `claude mcp add oboete` (done 2026-09-04: writeClaude/removeClaude, eight handlers with the fixed selectors, MCP registration argv returned for the caller to run through the resolved CLI path)
- [X] T049 [P] [CC] [US1] Implement src/setup/write-codex.ts: ~/.codex/hooks.json handlers (SessionStart matcher startup|clear|compact, UserPromptSubmit, PreToolUse, PostToolUse, Stop, PostCompact, SessionEnd), managed block in ~/.codex/config.toml with [hooks.state] trusted_hash (sha256 of canonical handler JSON), additionalContextLimit = 0, [mcp_servers.oboete] (done 2026-09-04: writeCodex/removeCodex plus codex-trust.ts; the trust preimage carries the configured timeout, verified against codex-cli 0.153.2 on 2026-09-04, and the research notes were corrected)
- [X] T050 [P] [CC] [US1] Implement src/setup/write-grok.ts: ~/.grok/hooks/oboete.json with explicit per-hook timeout, handlers deduplicated against the Claude compat layer, PermissionDenied and PostCompact included, MCP registration per the T006 probe (done 2026-09-04: writeGrok/removeGrok, per-hook timeouts, duplicate Claude events reported as a warning, and the handlers rolled back when config.toml cannot be written)
- [X] T051 [P] [CC] [US1] Implement src/setup/write-pi.ts and src/pi-extension.ts: three-line loader ~/.pi/agent/extensions/oboete.js, extension with try/catch only, invocation ids, detached capture child with `--invocation`, bounded inject child (8 s at session start, 300 ms per prompt), in-memory failure counters passed as `--prior-failures`, tools search/timeline/get calling the CLI; tests test/unit/pi-extension.test.ts assert no fs or network call from the extension (done 2026-09-04: writePi/removePi and the second bundle entry dist/pi-extension.mjs; failure codes survive a capture child that never starts)
- [X] T052 [EXT] [US1] Implement src/setup/detect.ts and src/setup/probe.ts: installed-agent detection, parallel headless probes under a 90 s deadline asserting a probe event, trust state per agent, native-memory warnings (Codex memories, Claude Code auto-memory, Grok native memory) reported but never changed (done 2026-09-04: Codex; PATH-or-home detection, canonical Codex trust verification, read-only native-memory warnings, and injected parallel headless probes under one shared deadline)
- [X] T053 [CC] [US1] Implement src/setup/consent.ts and the `oboete setup` command in src/cli.ts: consent display (host, credential source, cost class, egress classes; agent-cli shows the subscription it consumes), --accept-egress, --yes only when the stored hash matches the current tuple, --remove, missing-credential guidance (Cloudflare steps with URLs; offer ollama, agent-cli, or no provider), the `oboete view --open` launch line; tests test/unit/consent.test.ts (--yes refused after host, credential source, or egress class changed) (done 2026-09-04: consent.ts and the setup command; consent refused before any foreign file is touched, the record written on every run past the gate, the agent CLI spawned by its absolute path, credentials stripped from every child)
- [X] T054 [EXT] [US1] Write scripts/e2e/isolated-user.mjs: seeds three facts with agent A (claude -p, codex exec, grok -p, pi -p), starts agent B in the same synthetic repository with a prompt forcing one tool call, asserts all three facts, supports --pairs all, --no-credentials, --daily (done 2026-09-04: scripts/e2e/isolated-user.mjs and its unit tests; the Codex trust rows are retargeted to the copied hooks.json so the run needs no trust bypass, and no agent leg receives oboete credentials)
- [X] T055 [EXT] [US1] Run the Claude Code ↔ Codex vertical slice under the isolated user (resume, compact, fork, clear behaviour; SessionStart source policy) and fix adapter mappings until the four Claude/Codex pairs pass (done 2026-09-05: both pair directions pass, and the lifecycle harness (`isolated-user.mjs --lifecycle --agents claude,codex --daily`) passes 8 of 8 checks in run 2026-09-05T08-01-06-892Z (docs/evidence/m1-dogfood.md). The harness seeds a separate session S1, observes it, then drives the lifecycle on S2 so that re-injection is observable. F1: Codex 0.153.0 fires PostCompact on /compact, then lazily fires SessionStart source=compact at the next turn about 200 ms before UserPromptSubmit, delivering the new-epoch pack through codex:SessionStart (runs 2026-09-05T03-30-44-818Z and 2026-09-05T06-02-58-033Z); A21 stays as the fallback when an epoch has no session-start pack. F2: /new fires no parent SessionEnd and creates the new root lazily at its first turn's SessionStart source=startup about 200 ms before UserPromptSubmit; the parent stays active until /quit, so the child pack carries the previous ended session's summary (run 2026-09-05T07-03-44-495Z))
- [X] T056 [EXT] [US1] Run the remaining pairs (Grok Build and Pi as sender and receiver) under the isolated user, including the Grok deferred delivery and the Pi child processes, until 12 of 12 pass; record in docs/evidence/m1-dogfood.md (done 2026-09-05: 12 of 12 pairs pass in run 2026-09-05T11-10-21-871Z once the Grok Build account had balance again; the 2026-09-04 run's six failures were all HTTP 402 on the Grok Build legs, and nothing in oboete changed for those legs between the two runs)

**Checkpoint**: SC-001 and SC-009 pass on the isolated account.

---

## Phase 4: User Story 2 - The agent is never blocked or slowed by memory (Priority: P1)

**Goal**: every capture step exits 0 inside its deadline under every injected failure; spooled
events are recovered; no batch is applied twice.

**Independent Test**: the failure matrix in quickstart.md ("Failure injection") passes: every hook
in deadline, spool recovery, HTTP and apply counts as specified, doctor names the component.

- [X] T057 [CC] [US2] Implement the test-only fault seam (OBOETE_TEST_FAULT honoured only when NODE_ENV=test) in src/testing/faults.ts and wire it into open.ts, capture.ts, llm.ts, lease.ts, pi child spawn (done 2026-09-05 by Claude Code, as the seam routes credential-bearing provider requests and sits in the detector: `testFault(name)` gates four seams — detect.ts `detector-never-returns`, llm.ts `provider-hang` and `worker-kill-after-response`, capture.ts `pi-throw` at the top of runCapture, which is the Pi child entry — and `faultFetch` redirects provider requests to a loopback-only `OBOETE_TEST_FAULT_URL` so 401/429/403/length/malformed/unreachable come from a real local server. open.ts and lease.ts got no seam: every open/write failure is staged for real (missing, corrupt, chmod, held BEGIN IMMEDIATE; `enospc` is the alias for a read-only database plus an unwritable spool directory, since no unprivileged test can fill a disk) and lands in the same catches, and every lease function takes `now` while `worker_lease` is one row a test can write. Gate test: test/unit/faults.test.ts; `npm test` glob extended with `build/test/fault-*.test.mjs`)
- [X] T058 [EXT] [US2] Write test/fault-storage.test.ts: db-missing, busy, corrupt, readonly, enospc, oversized-payload, detector-never-returns (process wall time per event kind asserted, 1 MB input, slow detector + busy database) (done 2026-09-05 by Grok Build; shared spawn harness in test/helpers/fault.ts — scenario() skips all but `OBOETE_TEST_FAULT` under the quickstart loop, spawnEngine scrubs OBOETE_TEST_FAULT*/GROK_* from the child env; `enospc` is the alias for a read-only database plus an unwritable spool directory; wall-time table covers the six capture-only Claude kinds at the read bound with the detector fault and a held write lock, 271-274 ms each. T063 test correction: db-missing, busy and readonly now drive the recovered session to SessionEnd, release the harness-held lease, and assert a second observe exits naturally with 0 or 1, a session_end batch covering the recovered row, and run end reason=empty (contracts/cli.md:15). The first run retains its 2 s harness bound: recovered mid-session rows remain queued work under FR-009; the designed 20-minute worker bound is unchanged and is not an engine defect)
- [X] T059 [P] [EXT] [US2] Write test/fault-worker.test.ts: worker-kill, worker-kill-after-response (HTTP 2 / apply 1), lease-steal, lease-lost-after-3036, clock-jump, resume/compact/fork/clear epochs, pause (done 2026-09-06 by Grok Build; async observe launcher plus an in-process gated fake provider. The seed verifies its own rows: a hook that misses CAPTURE_DEADLINE_MS stores failed rows and the worker then has nothing to batch — that, not a takeover race, was the rare exit-0-without-batch. worker-kill retries in a fresh fixture when the SIGKILL lands after the release; resume/fork/clear seed a foreign conversation root so conversationPolicy is falsifiable; compact asserts the ledger rows; takeover exits are pinned to 1 per contracts/cli.md)
- [X] T060 [P] [EXT] [US2] Write test/fault-provider.test.ts: provider-unreachable, provider-hang, provider-429-3036, provider-403-5035, provider-401, provider-length, provider-malformed, provider-wrong-language, cap-boundary, consent-changed (done 2026-09-06 by Grok Build; scripted loopback fake provider through the faultFetch seam with dummy credentials, plus remote-no-duplicate. The seeded event is re-driven until capture classifies it; cap-boundary exercises the cross-preset sum; the child env drops NODE_USE_ENV_PROXY/NODE_OPTIONS in test/helpers/fault.ts. RED for T063: provider-malformed expects the batch log line to carry CallOutcome.detail, which nothing reads today)
- [X] T061 [P] [CC] [US2] Write test/fault-pi.test.ts: pi-throw, pi-child-hang (stale .started), pi-spawn-failure (doctor probe), prior-failure counters recorded (done 2026-09-05: pi-child-hang spawns the real child with stdin never closed, kills it after `.started` and drives cleanupPiAck with a shifted `now` (count 1 then 2, details invocations); pi-spawn-failure goes through `oboete setup --agents pi --provider none --accept-egress --json` with an executable `pi` first on PATH whose interpreter does not exist (probe fail inside 10 s; setup's JSON carries the status only, the reason belongs to doctor, T069); prior-failure counters use two distinct turns. RED for T063: pi-throw exits 3 through src/cli.ts's uncaught catch, the contract says 0)
- [X] T062 [CC] [US2] Write test/fault-grok.test.ts: grok-success, grok-exec-failure, grok-oboete-deny, grok-other-handler-deny, grok-parallel-batch, grok-no-tool, asserting packs received and attempts_json outcomes after raw-event purge (done 2026-09-05: the six real hook sequences from test/contracts/grok/*.json through the bundle with GROK_SESSION_ID; asserts state, delivery_count, attempts_json outcome pairs, injection_items decisions, and the ledger after purgeExpiredEvents under a claimed lease. Grok reports no model, so every pack carries window_unknown and an omitted pack keeps it over not_delivered / no_tool_call (FR-028); those reasons show on the items)
- [X] T063 [EXT] [US2] Fix every failure surfaced by T058-T062 in the owning module (returning security-owned fixes to Claude Code) until the matrix is green on 22.16 and 24.x (done 2026-09-06 by Codex with the log-content fixes by Claude Code: src/cli.ts exits 0 for hook/capture/inject on an uncaught throw and writes one hook-log line whose `reason` is the error code or class name, never the message — Node 24 JSON.parse errors quote the input, so the four sibling catches in capture.ts and inject.ts were moved to the same `errorCode` (src/log.ts); src/worker/observe.ts logs `detail=` on a failed batch through the SAFE_UNUSABLE_DETAILS allowlist so validation details never carry provider content; the three storage recovery scenarios drive the session to SessionEnd and assert the natural exit and the session_end batch; the worker-kill "takeover without a batch" was the seed missing CAPTURE_DEADLINE_MS under load, not an engine race. Matrix: 40 fault scenarios green, npm test 665+44 on Node 24.16 and 22.23)

**Checkpoint**: SC-002 "100% of turns complete under every injected failure" holds.

---

## Phase 5: User Story 3 - Nothing sensitive leaves the machine (Priority: P1)

**Goal**: secrets, private spans, path-rule files, local-only rows, and other repositories'
memories never reach a remote request or a foreign pack; consent gates every remote destination.

**Independent Test**: the privacy suite (`npm test -- --test-name-pattern privacy`) and the
replay scan report zero secret corpus items in memories, outbound bodies, and packs, and zero
local-only or private rows in any outbound request.

- [X] T064 [CC] [US3] Extend test/unit/privacy.test.ts with the mixed-sensitivity outbound body assertion (events, nearby, citations, repo_ref), cross-repository injection refusal, pack recognition on re-capture (pack_hash), and the agent-swap invariance over hashes, bodies, and decisions (done 2026-09-06: the new tests drive the real hook, detector, worker and pack through `test/helpers/observe.ts` (the fixture extracted from observe.test.ts): the mixed session asserts the actual Workers AI request body (eligible rows travel; the secret row is dropped whole, the `<private>` span, the path-rule row, the local-only memory and the other repository never appear; `repo_ref` is the opaque id and the normalized identity and cwd are absent; no agent name); the other-repository memory is never an injection candidate (`injection_items` has no row for it) while this repository is delivered; a pack that comes back through Stop is recognized on the direct path and on the spool path; a look-alike pack is ordinary content; the agent swap compares memories (hashes, bodies, sensitivity), the pack text and the injection decisions, not the raw-event hashes, which are equal by construction. The request builder itself is covered by request.test.ts, so those assertions were not repeated)
- [X] T065 [CC] [US3] Implement the pack recognition path in src/capture.ts (injected text matched by pack_hash is stored as recognized, never summarized) and the credential scan in test/unit/logs.test.ts (no credential value in logs, spool, doctor output, packs) (done 2026-09-06: `src/injection/recognize.ts` owns the pack markers and `stripRecognizedPacks`: a span from a header line to a footer line whose sha256 is in `injections.pack_hash` is removed from the content and listed in `payload_json.recognized_packs`; every footer line is tried, so a pack line that quotes the footer still matches; a span the database never issued stays content. capture.ts applies it inside the write transaction, batches.ts recoverSpool applies it when a spooled row arrives (the hook had no database to look the hash up). The credential scan found two real gaps and fixed them: the configured credential values (log.ts `credentialValues`, 8 characters or more) are now redacted by the detector itself before the secretlint pass (`[REDACTED:oboete-credential]`, row secret), with the hook and the worker passing their own environment; and on Node 22.16 the detector Worker and the ESM load of node:sqlite printed an ExperimentalWarning to the hook's stderr, so open.ts loads node:sqlite lazily and the Worker keeps its own streams (R6). Doctor output is scanned too; it is the T069 stub until then)
- [X] T066 [CC] [US3] Implement the imported-row quarantine reclassification in src/worker/observe.ts + src/privacy/classify.ts (detector and directive check move imported → unreviewed or secret) with tests (done 2026-09-06: `reclassifyImportedRow` (privacy/classify.ts) decides from the two detector results and the directive check: clean → `unreviewed` with the detector's texts stored (a `<private>` span in an imported body does not survive release), a secret finding or a directive → tombstoned as `secret` with redacted title, body and cjk_bigrams, a detector that did not finish → `retry` (row stays quarantined). observe.ts runs it after classifyPending in keyset pages of 50 with one fenced write per page and a fresh lease clock; `nearbyCandidates` also excludes quarantined rows so a summarizer never sees one. Tests: the decision table and an end-to-end run where four imported pinned rows reach no pack before the worker and only the clean and the private-stripped ones after. Deviation from the file list: the nearby-candidate scope is in src/db/queries.ts)
- [X] T067 [EXT] [US3] Write scripts/fixtures/generate-1000-events.mjs and test/fixtures/events-1000.jsonl: native hook payloads for all four agents from the T004 fixtures, seeded facts (ja/en), the secret corpus, the directive corpus, payloads at and above 1 MB, resume/compact/fork/clear sequences
  - done 2026-09-06 (Grok Build in worktree m1/p5-t067, merged 03042477): 1051 lines, 48 sessions, 40 facts (ja/en) each recalled later, all 37 corpus secret ids (32 positive + 5 negative) as `__SECRET:<id>__`, 32 directives in prompt and output, 4 size events (1048576 ×2, 1048577, 2097152), resume/compact/fork/clear on all agents; two runs byte-identical; assertCoverage checks payload content (fact expect, tokens in adapter output fields, byte equality, Grok timestamps, Pi sources). Notes: scripts/fixtures/NOTES-T067.md
- [ ] T068 [EXT] [US3] Implement `oboete fixture replay` in src/cli.ts + scripts/fixtures/replay.mjs: spawn the real hook per event, measure p99, session-start wait on ready and pending paths, worker maxRSS, database growth, secret and directive corpus scans, duplicate count per (conversation, epoch), fact recall; write docs/evidence/m1-resource-envelope.md

**Checkpoint**: SC-005 and SC-006 pass in CI; the replay evidence file exists.

---

## Phase 6: User Story 4 - Setup and doctor (Priority: P2)

**Goal**: one setup command wires the selected agents with probes and trust state; doctor names
every degraded item with reason, consequence, and recovery.

**Independent Test**: quickstart.md "Setup and doctor on the isolated account": setup under 2
minutes, doctor healthy, then break one item at a time and confirm doctor names it and its
recovery step turns it green.

- [ ] T069 [EXT] [US4] Implement src/doctor.ts and `oboete doctor [--probe-provider] [--no-probe-agents]`: items with { item, status, reason, consequence, recovery } for wiring probes (default on) and trust state, storage integrity and FTS5, migration level, worker liveness, spool backlog and writability, provider (live only with --probe-provider, else unverified), allowance estimate and exhaustion, catalog paid flag, unrecognized agents, native memory coexistence, config file mode, Pi diagnostics (pi_child_hang, pi_child_failed, pi_spawn_failed); corrupt-storage recovery steps
- [ ] T070 [P] [EXT] [US4] Implement `oboete pause` / `oboete resume` (marker file checked before the database is opened) in src/cli.ts with tests test/unit/pause.test.ts (memories untouched)
- [ ] T071 [EXT] [US4] Write test/unit/doctor.test.ts: break-one-at-a-time (hook entry removed, database chmod, corrupted header, worker killed, unreachable provider, exhausted counter, stale Pi .started, Pi extension unable to spawn) asserting reason, consequence, recovery, and that recovery turns the item green
- [ ] T072 [EXT] [US4] Run setup and doctor on the isolated account (quickstart.md), record timing and the break-one results in docs/evidence/m1-dogfood.md (SC-008)

**Checkpoint**: SC-008 recorded.

---

## Phase 7: User Story 5 - Zero-credential operation and honest degradation (Priority: P2)

**Goal**: without credentials or after exhaustion, memories still come from the fallback and every
pack and doctor output says so.

**Independent Test**: scripts/e2e/isolated-user.mjs --pairs all --no-credentials passes SC-001
with `Degraded:` in every pack; the exhausted-counter fixture switches to the fallback without
retrying and shows the switch in doctor and packs.

- [ ] T073 [EXT] [US5] Verify the degraded labelling end to end: summary degraded_reason precedence, `Degraded:` line in packs (summary_pending, index_unavailable, empty, window_unknown, batch reasons), doctor exhaustion flag; tests test/unit/degraded.test.ts
- [ ] T074 [EXT] [US5] Implement `oboete why <session-id> [--turn N]` in src/cli.ts reading injections, injection_items, attempts_json (deliveries per attempt), trims, staleness, deferred and degraded state; tests test/unit/why.test.ts
- [ ] T075 [EXT] [US5] Run the no-credentials and exhausted-allowance runs on the isolated account and record SC-004 in docs/evidence/m1-dogfood.md

**Checkpoint**: SC-004 recorded.

---

## Phase 8: User Story 6 - Inspect, search, pin, and delete memories (Priority: P3)

**Goal**: viewer and CLI expose sessions, memories with sensitivity and provenance, search, pin,
delete, review state, and `why`; the tool interface exposes search, timeline, get under the same
boundaries.

**Independent Test**: quickstart.md "Viewer and MCP": a new memory appears within 2 s, pin/delete/
search work, non-loopback and token-less access refused, each supported MCP client lists and calls
the three tools, a `repo` argument is rejected.

- [ ] T076 [EXT] [US6] Implement `oboete search` (empty result states that M1 search is lexical), `timeline`, `get`, `pin`, `unpin`, `delete` in src/cli.ts on top of src/db/queries.ts with --json output and exit codes per contracts/cli.md; tests test/unit/cli-memories.test.ts (tombstone never re-created from identical title/body, including under another observation type per A13)
- [ ] T077 [CC] [US6] Implement src/mcp.ts: legacy-era stdio JSON-RPC (initialize echo, notifications/initialized, tools/list with inputSchema, tools/call with content + structuredContent, isError results, -32601 for server/discover, -32602 for a repo argument); tests test/unit/mcp.test.ts with the raw frames from contracts/mcp.md
- [ ] T078 [CC] [US6] Implement src/viewer/server.ts: Hono on @hono/node-server bound to 127.0.0.1, per-launch token, Origin check on mutating routes, SSE via PRAGMA data_version polling every 500 ms, routes for sessions/turns, memories (review, pin, delete), search, `--open` launching the browser; tests test/unit/viewer-server.test.ts (non-loopback bind and token-less request refused)
- [ ] T079 [CC] [US6] Implement the viewer frontend in src/viewer/app/ (Preact + Vite, embedded at build): session/turn list, memory cards with sensitivity, provenance, review state, degraded reason, pin/delete/review actions, search, live updates; polite user-facing copy (no abbreviations)
- [ ] T080 [EXT] [US6] Write scripts/e2e/mcp-clients.mjs running tools/list and tools/call through the Claude Code, Codex, and Grok MCP clients under the isolated user with raw frames recorded, and the Pi tool path through the extension
- [ ] T081 [EXT] [US6] Run the viewer timing (SC-011) and the MCP client runs on the isolated account and record in docs/evidence/m1-dogfood.md

**Checkpoint**: SC-011 recorded; MCP client rows of R13 pass or are blocked.

---

## Phase 9: User Story 7 - Export, import, and evidence (Priority: P3)

**Goal**: portable export with sensitivity, provenance, and repository identity; import that
merges by content, keeps deletions, never lowers sensitivity, and quarantines imported rows.

**Independent Test**: quickstart.md "Export / import": counts match, tombstones preserved, hash
mismatch and oversized lines rejected with exit 2, imported rows absent from search and packs until
classified, tombstone under --map-repo still suppresses.

- [ ] T082 [CC] [US7] Implement src/transfer.ts export (`oboete-export/1` header + lines with material_hash, content_hash, provenance, sources) and import (64 KB per line, 256 MB per file, material_hash verified, content_hash recomputed for the local repo id incl. --map-repo, union on content_hash, sensitivity lattice, tombstones win, active rows land local_only / imported); tests test/unit/transfer.test.ts
- [ ] T083 [EXT] [US7] Wire `oboete export [file|-]` and `oboete import [file|-] [--dry-run] [--map-repo]` in src/cli.ts with the exit codes of contracts/cli.md
- [ ] T084 [EXT] [US7] Run the export → import round trip between two isolated installations and the 1,000-event replay comparison against docs/evidence/m1-resource-envelope.md; record in docs/evidence/m1-dogfood.md (SC-003)

**Checkpoint**: SC-003 recorded.

---

## Phase 10: Evidence, dogfood, and polish

**Purpose**: the 7-day dogfood gate, documentation, and the review gates the workflow requires.

- [ ] T085 [EXT] Run scripts/e2e/isolated-user.mjs --daily on the isolated account for 7 consecutive days, appending doctor output, provider usage, spool backlog, duplicate count, and viewer latency to docs/evidence/m1-dogfood.md (SC-007)
- [ ] T086 [P] [EXT] Write README.md (install, setup, doctor, privacy model, support matrix 22.16 / 24.x, degraded modes) and docs/agents/*.md per-agent notes; user-facing copy in full sentences
- [ ] T087 [P] [EXT] Add `npm run pack-check` (npm pack, install into an empty prefix, unpacked size ≤ 30 MB) to package.json and CI
- [ ] T088 Run the review gates on the implementation branch: `/code-review` then `ponytail-review`, plus `rules/security.md` tooling (semgrep, `/codex-review mode=security`, `/codex:adversarial-review`) for the security-owned modules, and record `~/.claude/review-status.json`
- [ ] T089 Run `speckit-verify-tasks` against this file and resolve every phantom completion before opening the PR from `007-oboete-m1-alpha` to main

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (amendments, probes)**: T001 and T002 first; T003-T010 after T002; T011 last. Blocks every dependent lane listed in research.md R13.
- **Phase 2 (foundation, kernel)**: T012-T013 first; T014-T015 after T013; storage T016-T021 after T012; capture T024-T029 after T021-T023 and T004; worker T030-T040 after T029; retrieval and injection T041-T046 after T021 and T033. Blocks all stories.
- **US1 (Phase 3)**: after Phase 2; T047 before T048-T051; T052-T053 before T054; T055 before T056.
- **US2 (Phase 4)**: after Phase 2; T057 before T058-T062; T063 last.
- **US3 (Phase 5)**: after Phase 2; T067 before T068.
- **US4 (Phase 6)**: after US1 (needs the writers and probes); T069 before T071-T072.
- **US5 (Phase 7)**: after Phase 2 and US1's E2E harness (T054).
- **US6 (Phase 8)**: after Phase 2; T078 before T079; T080 after T077.
- **US7 (Phase 9)**: after Phase 2; T082 before T083-T084.
- **Phase 10**: after every story; T085 needs 7 calendar days.

### Parallel Opportunities

- Phase 1: T003, T005-T010 in parallel once T002 exists.
- Phase 2: T017/T018 with T016; T020 with T019; T023 with T022; T032, T035, T037, T040 with their neighbours; T042 with T041.
- US1: T048-T051 in parallel after T047.
- US2: T059-T061 in parallel after T058's seam.
- Phase 10: T086 and T087 in parallel with T085.

## Parallel Example: Phase 2 storage

```bash
# After T016 lands:
Task: "T017 Write src/db/migrations/0002_memory_search.sql"
Task: "T018 Write src/db/migrations/0003_operations.sql"
# After T019:
Task: "T020 Migration smoke tests test/migrations/apply.test.ts"
Task: "T023 src/repo-identity.ts + tests"
```

## Implementation Strategy

### MVP First (US1 + US2 + US3 are all P1)

1. Phase 1: amendments decided, probes recorded, blocked lanes listed.
2. Phase 2: kernel green on both Node versions with the privacy suite red-then-green.
3. Phase 3 (US1): 12 of 12 pairs on the isolated account.
4. Phases 4 and 5 (US2, US3): failure matrix and privacy evidence.
5. **STOP and VALIDATE**: SC-001, SC-002, SC-005, SC-006, SC-009, SC-010 recorded.

### Incremental Delivery

- Add US4 (setup, doctor) → SC-008; add US5 (degradation) → SC-004; add US6 (viewer, MCP) → SC-011;
  add US7 (export, evidence) → SC-003; then the 7-day dogfood → SC-007.

## Notes

- `[CC]` tasks never leave Claude Code; `[EXT]` tasks are delegated with their file list as the
  scope fence (Grok Build `--allowed-file`, or Codex for exploratory work).
- Every delegated task is followed by `speckit-verify-tasks`, `/code-review`, and
  `ponytail-review` before its checkbox is marked.
- R13-blocked lanes stay unchecked with a `blocked: <row>` note until the owner decides.

### Phase 2 follow-ups (collected from the review rounds, 2026-09-04)

- capture.ts readStdin marks an exactly-256 KiB payload partial (`total >= STDIN_READ_BOUND`)
  without probing for a further byte; fail-closed, decide keep or read one more byte.
- test/e2e-hook.test.ts asserts the happy path and flakes under machine load (hook wall 280 ms,
  detector cutoff); the bundle loads twice (main + detector worker). Candidate for T088/T015: split
  dist into a hook chunk and a worker chunk, or lazy-load the secretlint rules.
- catalog.ts refreshWorkersAiCatalog writes runtime_state outside the lease fence; observe.ts should
  store the returned result inside a fenced transaction.
- deferred.ts control-character branch (omit planned, close with index_unavailable) has no test.
- Injection trusts memories.citations_head/citations_ok, refreshed only for the memories of a batch
  the worker just applied; after HEAD moves, older memories render stale until touched. The worker
  needs a HEAD-change re-check.
- classify.ts sessionSummary (request:/next_steps: verbatim) and latestRawActivity excerpts reach
  the CLI, MCP and viewer without rejectsDirectives; packs are gated at build time.
- detect.ts runs matchSecretPath once per tool-input path and events.ts bounds a path only by
  MAX_TEXT (1 MiB) below MAX_TOOL_INPUT_PATHS 50: with a bound rule list, 50 paths of 100 KiB cost
  about 157 ms (fail-closed on the detector cutoff). A path-length bound in events.ts fixes it.
- Path-rule bound: 64 rules of 256 characters against a slash-free 4 KiB path measure about
  250 ms here; tighten MAX_REPO_SECRET_PATH_LENGTH if that ever matters, not the rule count.
- staleness.test.ts teardown can hit ENOTEMPTY on `.git/ai` when `git` on PATH is the git-ai wrapper
  (dev machines only; CI has no wrapper). The retrying rmSync covers it most of the time.

### Phase 3 follow-ups (collected from the review, security and verification rounds, 2026-09-05)

- write-codex.ts rolls back a failed config.toml append by calling removeJsonHandlers, which deletes
  hooks.json's pre-oboete backup unconditionally; fold into whoever touches that file next.
- probe.ts reports fail/probe_event_missing only after polling the whole 90 s deadline, so setup
  takes about 90 s per agent that is installed but not actually wired. A shorter post-exit grace
  (the child already exited cleanly) would cut that without weakening the check.
- events.ts defines a `probe` event kind and the schema keeps it in the enum, but nothing emits it:
  probeEventStored recognises the marker inside a `prompt` row. Delete the kind or emit it.
- The live consent re-check is stricter than contracts/cli.md: preset, credentials and model are a
  snapshot taken when the run starts, so switching presets mid-run stops the send instead of
  following the newly consented preset.
- No test drives the pre-send checkpoint (llm.ts) with a predicate that answers false; the agent-cli
  branch and the checkpoint share one callable by construction only.
- scripts/e2e/isolated-user.mjs runs the observe leg and the search leg with the same environment,
  credentials included. The search leg is oboete's own CLI, so FR-016 does not cover it, but the two
  legs could carry different environments.
- `oboete setup` registers the Claude, Codex and Grok MCP servers while `oboete mcp` is still the
  T077 stub, so `claude mcp get oboete` reports "Failed to connect" until T077 lands. Decide whether
  the registration waits for T077 or the setup output says so.
- `oboete setup --remove` unregisters the Claude tools in user scope only. An alpha install made
  before the scope fix registered them in the directory setup ran in, and that entry survives the
  removal (found in the isolated account, cleared by hand). Either setup names those directories in
  its output, or the first release note says to run `claude mcp remove oboete --scope local` there.
- The isolated run passes without the Pi tool surface working: the 2026-09-04 run was measured with
  every Pi tool answering "oboete could not run that command", and the Pi pairs still passed,
  because delivery goes through the `before_agent_start` injection. The harness therefore cannot
  detect a broken tool surface -- that defect was found by hand. A pair that asks the receiving
  agent to use a tool would close it.
- The isolated user runs pi-coding-agent 0.84.4 and this machine's developer account 0.85.0; the
  `ToolDefinition.execute(toolCallId, params, signal, onUpdate, ctx)` declaration is identical in
  both, so the T051 signature fix covers the two versions the project has seen.
- Databases created before cefeee86 must be removed (including the isolated dogfood account's `~/.oboete/memory.db`); this is the last in-place migration edit before the first published build.
