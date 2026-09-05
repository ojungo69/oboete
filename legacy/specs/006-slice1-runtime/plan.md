# Implementation Plan: Slice 1 Automatic Memory Runtime

**Branch**: `spec/slice1-contract-post-142` | **Date**: 2026-08-30 | **Spec**: [spec.md](spec.md)

**Input**: Focused specification under `specs/006-slice1-runtime/`, Product Reset contracts under
`specs/005-product-reset/`, and Slice 1 issues #130/#137.

## Summary

Deliver the first Linux/WSL Claude Code ⇄ Codex automatic-memory path by deepening the retained
Codemem kernel:

1. correct the accepted Slice 1 fixture/schema/validators mechanically so the provider/resource
   manifest is buildable;
2. ship one vertical manifest PR in which setup can compile, disclose, confirm, atomically activate,
   and roll back the same frozen manifest the daemon/Observer/doctor/maintenance/viewer consume;
3. migrate once to schema v21 and land durable processing-job semantics before privacy closure;
4. close #130 only after one DestinationBoundary protects raw provider input, derived memory, every
   retrieval/read/export consumer, dedup/supersession, logs, and traces;
5. wire bounded lifecycle nudges and the fixed bidirectional memory semantics;
6. add managed setup start/attach only after the preceding PRs are independently mergeable;
7. prove the integrated packed hooks, privacy, recovery, and resource profile with one fixed runner.

No daemon manifest consumption lands before setup activation. No later US3 migration is allowed to
complete job durability after #130. No generic provider registry, policy engine, job framework,
second runtime, or harness platform is introduced.

## Technical Context

**Language/Version**: TypeScript 6.0.3 on Node.js 24.16.0

**Primary Dependencies**: Existing pnpm 11.8.0 workspace, Commander, Drizzle ORM,
`better-sqlite3`, Vitest, Hono, and native Node/URL/fetch/crypto/filesystem APIs. No new runtime
dependency.

**Storage**: Existing daemon-owned SQLite database plus owner-only capability generations under the
existing runtime `control/` directory. One v20→v21 migration after verified backup.

**Testing**: Existing Vitest suites, generated-schema parity, packed-artifact smoke,
no-agent-blockage harness, Product Reset validators, and one fixed real-hook runner.

**Target Platform**: Linux and WSL on local Linux filesystems; Claude Code and Codex only.

**Constraints**: One database writer; zero Agent blockage, accepted-event loss, stale-claim commit,
frontier advance on failure, duplicate active memory, restricted disclosure, incompatible-repository
injection, content-bearing diagnostic, or fabricated success. Unknown values fail closed.

**Fixed profile**: Accepted fixture limits plus periodic 30 s, idle 120 s, event debounce 1 s, stuck
claim 5 min, at most 100 source events per newly admitted v21 job, and raw retention disabled/0.
Embedding is disabled;
lexical is the healthy required lane.

## Constitution Check

*GATE: PASS before research and PASS after hardened design.*

Authority: [free-mem Constitution v2.0.0](../../../CONSTITUTION.md) (v2.0.0 is superseded by v3.0.0; the cited text is in the git history of `CONSTITUTION.md`).

| Principle | Status | Plan evidence |
|---|---|---|
| I. Automatic Memory UX First | PASS | Both Agent directions, production nudge, prompt drain, later one-flow setup, and no manual handoff are exit gates. |
| II. Local-First and Explicit Egress | PASS | Closed manifest, compiler-derived egress, exact repository identity, pre-prompt projection, and one all-consumer DestinationBoundary fail closed. |
| III. Bounded and Predictable Resources | PASS | One immutable profile owns process/queue/retry/concurrency/storage/token plus scheduler/source/retention limits; each field has a named enforcement PR. |
| IV. Durable Capture and Honest Degradation | PASS | Existing spool/raw ledger survive; v21 jobs retain sources and atomically commit; retry exhaustion and semantic-disabled lexical fallback are explicit. |
| V. Product Slices Before Speculative Platforms | PASS | Linux/WSL, two Agents, one profile, two wire protocols, lexical-only Slice 1; registry/policy/job/harness platforms remain deferred. |

No constitutional exception is required.

## Delivery order and independent PRs

| Boundary | Delivery | Independent result |
|---|---|---|
| PR 0 | Planning plus contract/fixture checkpoint | The 006 artifacts and Product Reset fixture, schemas, validators, bound examples, and fingerprints agree on one buildable closed manifest; no runtime change. |
| PR 1 | Vertical effective manifest | Setup activation exists in the same PR as daemon frozen consumption; absent manifest is capture-only, malformed manifest fails startup, and valid manifest remains pending-privacy/no provider/no sweeper while Observer/maintenance/viewer lose legacy bypasses. |
| PR 2 | Schema v21 and durable processing jobs | One migration, capacity/claims/retry/resume/provenance/retention/atomic terminal Store transactions are available before privacy activation. |
| PR 3 | Complete #130 privacy closure | First-class capture, provider projection, derived sensitivity, all read/export consumers, dedup/supersession, content-free diagnostics, generated artifacts, and semantic-disabled retention are enforced; only then close #130. |
| PR 4 | Bidirectional memory and triggered lifecycle | Fixed kinds/provenance, nudge wiring, stop-race fix, PreCompact, request-time drain, and source/packed Claude⇄Codex scenarios work. |
| PR 5 | Managed setup lifecycle and basic doctor | Setup coordinates start/attach/restart safely, installs both Agents, and verifies version/fingerprint/readiness; no duplicate writer. |
| PR 6 | Fixed runner, docs, and CI | Pinned real-hook privacy/failure/resource evidence gates #137. |

Each later branch is based on the preceding merged boundary. A PR may expose a safe pending state but
must not claim enforcement owned by a later PR.

Implementation ancestry is also a hard gate: PR 0 is delivered from a fresh worktree created from
refreshed `origin/main` after merged PR #142. Every runtime branch is based on merged PR 0.

## PR 0 - Contract and fixture correction checkpoint

Co-deliver these planning artifacts and mechanically correct, in the same scoped PR:

- `specs/005-product-reset/contracts/capability-manifest.md`,
  `specs/005-product-reset/contracts/alpha-comparison.md`, and
  `specs/005-product-reset/data-model.md`;
- Slice 1 JSON schema, fixture, semantic validator, JS validator, and validator tests;
- bound alpha success/failure/suite examples and provider/resource result checks;
- closed runner-evidence fields for 12 plateau windows, drain/checkpoint,
  item/token/concurrency, unique duplicate/no-op workload receipts with strict non-overlapping
  workload/drain/checkpoint/sample timestamps and zero memory/job deltas,
  hostname-valid public CA plus six base/local/repaired setup/start TLS receipts binding trust-anchor/
  peer-certificate/zero-byte proof, runner-owned provider-egress timing, pair-bound
  same-ID/different-digest conflict, canonical no-activity null plateau evidence, sensitivity-byte
  totals bounded by observed payload bytes, runner-owned zero restricted/sentinel observations, and
  the exact 16+1 suite;
- all provider/manifest/fixture/result/runner-evidence fingerprints.

Replace provider kind/scheme/host/free-form credential/self-declared policy with closed
ProviderProposalV1/ProviderChoiceV1. Add missing fixed resource fields. Keep summary stub metadata in
the harness; it materializes normal proposals. Preserve the existing output-limit recovery as the
only runner-owned/test-only resource successor, differing from version 1 only in `version=2` and
`maxMemoryItemsPerDerivation=17`; base/local/repaired manifests remain version 1/max16 and production
setup has no profile selector. Add one complete repaired-remote successor and bind configuration/redirect/downgrade
signals to its computed manifest/provider fingerprints. Materialize local-derivation and
output-limit cases as complete successor manifests rather than partial overlays. PR 0 proves static
shape/fingerprints only; stub transport materialization remains PR 6. Run the complete current
validator suite, not only the positive fixture validator.

PR 0 co-delivers this plan and the scoped 005 correction; no runtime source is included.

## PR 1 - Vertical effective manifest

### Compiler and storage

- Add one core `capability-manifest.ts` with closed proposal/choice/manifest types, URL validation,
  compiler-derived policy, native JCS-compatible canonicalization, SHA-256 `providerFingerprint`/manifest
  fingerprints, safe projection, immutable generation read/write, and current-pointer operations.
- Add only the necessary capability paths to the existing storage layout and export only stable
  compiler/read APIs.
- Reject unknown fields, self-declared policy/fingerprints, arbitrary headers, inline credentials,
  `localhost`, non-literal local hosts, remote HTTP, non-system TLS, incomplete/non-canonical URLs,
  and redirects.

### Setup activation before daemon consumption

- Extend the existing setup command, not a second CLI, with explicit wire protocol/model/complete
  endpoint/CredentialRef inputs and optional legacy translation.
- Make Claude Code+Codex the default selected lanes. OpenCode is not a Slice 1 destination or
  provider/credential source; any retained legacy-only flag does not enter the active manifest.
- Add one shared lifecycle lock used by setup and daemon start. Setup acquires it before
  `readDaemonHealth`, rechecks writer/socket/health while held, and keeps fixed order
  `lifecycle -> setup/spool -> daemon writer` through activation.
- Interactive compile/disclosure/confirmation occurs before taking that lock. After confirmation,
  setup acquires it, rechecks daemon and editor/pointer prestates, and performs a native 5 s
  credential/payload-free TLS chain+hostname handshake before any mutation. Daemon start repeats the
  handshake while holding the same lock; failure there starts writer/RPC/capture/spool-import/lexical in a
  provider-only degraded state rather than aborting the daemon.
- Compile and display safe fields and fingerprints before confirmation.
- Generalize the existing setup snapshot tracker from per-lane commit to one transaction covering
  the selected Claude/Codex files, install ownership manifest, and capability current pointer.
- Add one owner-only durable setup journal around those existing snapshots: fsync prepared state,
  publish editor files/generation, publish `current` last, recover/finalize by target hashes on next
  setup/start, restore only when every target matches a recorded pre/post hash, and if any target is
  unknown preserve all targets unchanged with the journal retained and provider startup blocked.
- Active manifest may retain `translated|ignored|overridden`; detected conflict rejects activation.

### Frozen daemon/runtime boundary

- Resolve capability state before Observer/Sweeper construction:
  - absent current: start capture RPC/spool import only; no provider and no sweeper;
  - malformed/missing/mismatched/invalid: fail startup;
  - valid: freeze one object and pass its safe projection to RPC/doctor/status, but report
    `pending_privacy_boundary` and do not construct an executable provider, start Sweeper, or enable
    AI maintenance until PR 3.
- ObserverClient accepts only ProviderChoiceV1 plus frozen 60 s request timeout, 12,000 input
  characters, 4,000 output tokens, 1 MiB response, and temperature 0.2. Fix Anthropic Messages and
  OpenAI Chat Completions auth headers, request/response shapes, and credential-none behavior.
  Input uses UTF-16 code units, a 3,000-unit user floor, system-first 9,000-unit clipping, then user
  clipping from the start with `toWellFormed()` after each slice.
  Remove production provider discovery, endpoint
  suffixing, arbitrary header/custom provider, implicit credential cascades, OpenAI Responses, and
  mutable provider config reads from this boundary. Fetch uses `redirect: manual` and closed protocol
  header/payload/response code.
- Remove `loadObserverConfig` from the public/runtime surface; legacy parsing remains reachable only
  from the explicit setup translator and produces dispositions rather than effective runtime config.
- Adapt internal extraction-replay/distill tests to the same closed provider transport where
  applicable and remove their unused public core barrel exports in this PR; any future
  public/runtime exposure needs DestinationBoundary.
- Structured maintenance must receive the frozen summary choice rather than create GPT/OpenAI config
  from `loadObserverConfig`. Viewer config reports the same safe snapshot.
- Remove mutable periodic/idle/debounce/stuck/source-count/retention reads and compile those frozen
  fields. They become executable with provider/Sweeper only in PR 3. Schema-backed queue/retry/
  derivation and pack fields are explicitly pending until PR 2/3.

Full setup lifecycle start/attach is not part of this PR. A newly activated pointer takes effect
after the user starts the daemon; PR 5 automates that safely.

## PR 2 - Schema v21 and durable processing jobs

### One migration

- Add all first-class event sensitivity/repository/capture-manifest fields; memory/prompt/legacy-summary/
  content-artifact sensitivity/repository and session repository identity; memory lineage/revision/source/manifest/
  provider/attempt fields; versioned event payload digest and durable EventIdentityConflict receipt;
  and job state/claim/admission/attempt/resume/diagnostic fields in one v21 schema.
- Run after verified backup in one DDL/backfill/validation transaction. Generate the final test DDL
  from the package generator and prove fresh/migrated parity and rollback/idempotent reopen.
- Conservative backfill uses `secret`/unknown unless trusted structural evidence proves more.
- Drop legacy `idx_raw_events_source_stream_event_id` and create the repository-aware unique
  expression index over `COALESCE(repository_identity,'repo-v1:unknown')`, source, stream, and event;
  NULL stays stored/unauthorized while the index sentinel creates one fail-closed unknown bucket.
  Persist any NULL-bucket collision in a separate durable secret `raw_event_quarantine` record with
  redacted payload/digest and non-success receipt; never drop or canonically ACK/admit it.

### Existing flush batch becomes the durable job

- Capacity 25 includes queued, processing, failed, and retry-exhausted. Accepted work beyond capacity
  remains not admitted. One newly admitted v21 job contains at most 100 source events; migration
  preserves a wider immutable v20 recovery range without splitting or truncating it.
- Same repository/source/stream/event ID plus the same domain-separated canonical payload digest is
  idempotent. A different digest atomically creates/reuses one durable non-success conflict receipt,
  preserves the canonical row, returns no normal ACK, and creates no memory.
- Enforce capture concurrency 2, processing concurrency 2, retry 3, derivation 16, stuck-claim 5 min,
  and retained-job purge safety here; excess direct capture falls back to the existing spool within
  hook deadlines, and no more than two jobs may be claimed. PR 1 reported them pending.
- New admission starts with `attempt_count=0`. Claim generation fences stale workers. Successful claim atomically increments lifetime
  `attempt_count`, consumes an optional one-shot grant, records current attempt manifest/provider,
  and computes attempt fingerprint. Admission provenance never changes.
- Automatic attempts use frozen limit 3, permitting claims 1-3 only; a failed attempt 3 becomes
  retry-exhausted and any later claim requires a grant. Retry exhaustion stops timers. Configuration activation
  and daemon-observed provider health receipts fan out in one sole-writer transaction to all
  matching exhausted jobs, necessarily at most 25 under the global capacity; user confirmation
  targets exactly one displayed job. Each per-job signal
  binds the producer receipt and creates at most one grant; duplicate/stale/wrong-job signals are
  no-ops.
- Wire those producers durably: import setup activation receipts on daemon start, persist and detect
  provider unhealthy-to-healthy edges, and expose an explicit user-confirmed doctor retry command.
  Per-job sequence CAS, signal/grant insertion, job/receipt uniqueness, and crash replay are
  idempotent.
- Add Store transactions for atomic privacy skip and atomic memory+reference+dedup/supersession+
  job+frontier completion. PR 2 proves the transaction semantics even before PR 3 calls privacy skip.
- Keep retention disabled/0. Harden future purge to exempt every uncompleted job range.
- Audit legacy `gave_up`: exact complete retained ranges become frontier-already-advanced recovery
  candidates; missing/ambiguous ranges never rewind and become terminal content-free
  `legacy_unrecoverable` dispositions that consume no capacity and never claim successful recovery.

This PR completes the durable part of User Story 3 before #130 can close.
Provider calls, AI maintenance, and RawEventSweeper remain disabled through this PR even after the
v21 migration; PR 3 enables them only after the complete boundary is in place.

## PR 3 - Complete privacy closure and #130

### Capture and canonical repository identity

- Compute sensitivity from trusted redaction and persist it outside payload JSON.
- Compute RepositoryIdentityV1 from a transport-preserving Git remote verified against the current
  repository, retaining the exact bounded SSH username, or a realpathed primary Git anchor; caller
  project/basename/workspace values remain labels only. Revalidate the current canonical remote at
  capture and every restricted boundary so an A→B origin change cannot reuse A's authority.
- Quarantine degraded capture with empty payload and content-free error code.

### One DestinationBoundary seam

Add one narrow eligibility module with a closed boundary, pure decision function, and SQL predicate.
Provider/AI-maintenance boundaries carry compiler/runtime-derived peer trust so the pure function
distinguishes unverified local HTTP from verified local HTTPS without mutable manifest lookup.
Require it at:

- raw provider flush and structured maintenance;
- maintenance memory-role pack/report reads;
- Store/search recent/search/timeline/explain and reference findByFile/findByConcept;
- daemon get/search/pack and MCP direct/index/recent/timeline/explain/pack;
- viewer raw-event/status/usage and memory/observation/summary/prompt/artifact/safe-session reads;
- lexical and rehydrated semantic candidates, pack render/ledger/trace;
- export/import and dedup/supersession.

Verify the manifest PR already removed public extraction-replay and distill barrel exports; their
internal benchmarks stay test-only. Any future public/runtime re-export must take
DestinationBoundary.

Claude Code, Codex, and MCP always resolve remote/unknown in Slice 1 production. A local client or
caller model label cannot select a local policy. Runner-only local fixtures require a verified
loopback consumer observation bound into accepted result evidence.

### Provider and derived memory

- Stably partition local candidates by exact verified repository identity before projection; reject
  mixed/unknown groups content-free. Then project the exact eligible source set before building any
  context/transcript/prompt/request.
- All-restricted uses the PR 2 atomic privacy-skip transaction and zero calls. Mixed remote sends
  eligible only. Unauthenticated local HTTP is credential-none/eligible-only. Local HTTPS processes
  private/local-only only after verified peer identity and for a known source repository; secret
  never.
- Provider output cites only the projected job set. Each new observation/summary carries one direct
  ordinal-based `citations` child. The Store claim transaction applies the compiler-created
  DestinationBoundary and privately binds `ProjectedSourceSetV1` to that claim; no new durable queue
  or schema column is added. Completion revalidates the boundary and raw rows, maps source ordinals to
  exact IDs, normalizes optional half-open UTF-8 byte spans over canonical `redactedPayload`, and includes
  those spans in lineage/anchor/dedup identity. Reject missing/duplicate/noncanonical/out-of-range
  citations, claim/source drift, mixed repositories, and output above the active attempt manifest's
  derivation limit atomically. Provider-backed no-claim ingest is disabled/fail-closed; historical
  NULL provenance stays secret/unknown. Persist strongest sensitivity, exact repository,
  lineage/revision, and attempt provenance.
- Same-repository dedup/supersession retains strongest sensitivity and never revives tombstones.

### Read/export/log boundaries

- Ineligible rows are excluded before content query/materialization where possible and rechecked
  before rendering/serialization. Unknown legacy sensitivity is secret; unknown repository denies
  restricted content.
- Restricted trace/diagnostic/log/maintenance progress has only bounded codes/counts/fingerprints;
  remove current content-excerpt warnings.
- Export requires a verified local same-repository boundary for restricted rows; unknown/all-project
  exports eligible only. Version export as v2 and gate memory items, user prompts, legacy session
  summaries, and safe session shells; omit cwd/Git remote/branch/user/free-form session metadata.
  Import v2 preserves valid fields; absent/invalid and every legacy-v1 content row become
  secret/unknown regardless of project/remap labels.
- `semantic_disabled` skips candidate use but leaves vector rows unchanged.
- Rebuild both generated hook-runtime files from the CLI build script and prove byte parity and no
  restricted sentinel in artifact/output/logs.
- Only after these paths are installed, enable manifest-derived Observer/AI maintenance and start the
  manifest-derived RawEventSweeper in daemon lifecycle, enforcing 30 s periodic, 120 s idle, 1 s
  debounce, 30 s provider warm lifetime, and every pack envelope/lane field.

Issue #130 closes only after focused/full/packed/no-agent-blockage/privacy matrices, reviews, CI, and
the merged boundary prove all reachable consumers. It does not close after raw flush tests alone.

## PR 4 - Bidirectional memory and triggered lifecycle

- Add fixed `summary`, `failed_approach`, and `next_action` parser/store/MCP/presentation shapes that
  reuse the completed PR3 ordinal-citation, normalized-span, lineage, revision, and no-op seam without
  redefining its wire or authority.
- Complete final lexical InjectionPack reasons, limits, stable ordering, and provenance.
- Inject a nudge callback into daemon RPC and call it only after newly accepted event commit,
  including spool replay; batch calls coalesce through 1 s debounce.
- Fix RawEventSweeper shutdown fencing: nudge/timer/in-flight-finally cannot reschedule after stop;
  stop waits active work; explicit restart re-enables scheduling.
- Add Claude PreCompact normalization/registration and immediate bounded session-end/pre-compact
  processing. Prompt pack drains only relevant accepted source-session work within hard deadline.
- Keep Claude/Codex renderer behavior and generated runtime artifacts equivalent.
- Prove both source and packed directions before the external fixed runner.

## PR 5 - Managed setup lifecycle and doctor

- Reuse background `serve start` and existing daemon PID/lock/socket identity; export only the narrow
  lifecycle functions setup needs.
- Setup preflights platform/storage before mutation, installs Claude+Codex by default, coordinates a
  running daemon through stop/activate/start or attaches only when fingerprint/version match, and
  verifies doctor after activation. It never mutates a live daemon behind its frozen snapshot.
- Expand doctor/status readiness from runtime facts: manifest/provider identity, writer, mutation,
  spool, capacity/jobs, summary, lexical, semantic-disabled, hooks, and pack.
- Verify restart/stop/uninstall ownership, packed setup, and no duplicate writer in isolated HOME.

## PR 6 - Fixed runner, docs, and CI

- Add one runner and self-test bound to the corrected existing Slice 1 fixture and schemas.
- Emit exactly 16 positive results plus the required late-injection negative and prove one
  same-event-ID/different-payload-digest conflict without overwrite.
- Materialize ordinary provider proposals from deterministic stub metadata; production receives no
  stub/provider-registry path.
- Use the same pure manifest compiler through the runner adapter; do not add a harness-only provider
  compiler. Base remote uses `FREE_MEM_SUMMARY_API_KEY`/`external_metered`, local uses
  `https://127.0.0.1:1234/v1/chat/completions`/`none`, and observed stub cost 0 remains runner evidence.
- Use fixture-pinned complete local/remote URLs. Map the pinned base/repaired remote HTTPS hostnames
  only inside the runner network namespace to a loopback stub. Before candidate start, install a
  per-run hostname/IP-valid public test CA into isolated system trust outside candidate/manifest
  control; production rejects added CA path/environment configuration. Evidence binds its public
  fingerprint, no private key is committed, and binding
  or verification mismatch fails rather than selecting an ephemeral endpoint.
- Exercise both Agent directions, capture-only absent manifest, malformed manifest, daemon outage/
  spool replay, provider failure/resume, stale claims, queue capacity, privacy across all consumers,
  duplicate/no-op, path-with-spaces/linked worktree, unsupported environment, packed setup/runtime,
  and semantic-disabled retention.
- Record unconditional zero non-loopback socket attempts and remote restricted bytes, plus exact
  expected authenticated-loopback request/credential/eligible/private/local-only payload bytes,
  attempted/final render, lifecycle,
  manifest/provider/admission/attempt identity, capacity/retry, safety, and resource evidence.
- Bind each positive provider gate to runner-read explicit committed event IDs, count, and fingerprint;
  never infer the authorized set from a count or fixture prefix.
- Measure source bytes by sensitivity at the runner-owned stub from actual received request bytes and
  fixed synthetic markers/spans; do not derive observed bytes from policy or candidate results.
- Use ordinals 1-22, discard 1-2, measure 3-22 separately, nearest-rank p95, threshold equality fail.
- Run 12 identical duplicate/no-op windows with strict non-overlapping workload-start/workload-
  receipt/drain-receipt/checkpoint-receipt/sample chains, discard 1-2, enforce every absolute ceiling
  on 3-12, and require the final five to have constant process count, zero drained
  queue, identical item/token counts, RSS span at most 16 MiB, storage span at most 65,536 bytes,
  concurrency at most 2, and zero post-teardown orphan process. Do not claim the deferred eight-hour soak.
- Populate and validate the closed runner-evidence fields for every plateau window,
  drain/checkpoint/workload receipt, strict action/sample timestamps, no-op/zero-delta and item/token/concurrency value, public CA and
  six raw trust-anchor/peer-certificate TLS receipts and runner-owned provider-egress observations;
  bind them through separate result fingerprints
  and derived aggregates alongside canonical no-activity null plateau evidence, the 16+1 case, and
  identity conflict; reject sensitivity-byte totals above observed payload bytes, nonzero runner-owned
  restricted/sentinel observations, any private-key
  artifact or missing observation.
- Add the runner/validator CI job without weakening existing gates and update user-facing docs.

## Project Structure

### Documentation (this feature)

```text
specs/006-slice1-runtime/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── checklists/requirements.md
├── contracts/
│   ├── capability-manifest-v1.md
│   ├── processing-job-v1.md
│   └── sensitivity-egress-v1.md
└── tasks.md
```

### Source changes (repository root)

```text
vendor/codemem/
├── packages/core/src/
│   ├── capability-manifest.ts                 # new narrow compiler/storage seam
│   ├── destination-boundary.ts                # new narrow privacy seam
│   ├── storage-layout.ts, storage.ts, index.ts
│   ├── observer-client.ts, ingest-*.ts
│   ├── schema.ts, db.ts, store.ts, types.ts
│   ├── raw-event-flush.ts, raw-event-sweeper.ts
│   ├── project.ts, filters.ts, search.ts, ref-queries.ts, vectors.ts, pack.ts
│   ├── daemon-lifecycle.ts, daemon-rpc.ts, daemon-rpc-contract.ts
│   ├── daemon-operations.ts, export-import.ts, online-backup.ts
│   ├── maintenance/ai-structured.ts
│   └── viewer-routes/config.ts, viewer-routes/observer-status.ts, viewer-routes/memory.ts
├── packages/cli/src/commands/
│   ├── setup.ts, setup-config.ts
│   ├── serve.ts, serve-invocation.ts
│   ├── status.ts, db.ts, hook-rpc-client.ts
│   └── claude-hook-inject.ts, codex-hook-inject.ts
├── packages/mcp-server/src/
│   ├── rpc-client.ts, project-scope.ts
│   └── tools/items.ts, tools/search.ts, tools/timeline.ts
├── packages/cli/scripts/sync-hook-runtime.mjs
├── plugins/claude/scripts/hook-runtime.mjs
└── plugins/codex/scripts/hook-runtime.mjs

harness/slice1-runtime.mjs
harness/slice1-runtime.test.mjs
.github/workflows/ci.yml
```

**Structure Decision**: Reuse existing setup atomic files, storage layout, daemon sole writer, spool,
raw-event ledger/batch, Store, search/pack, serve lifecycle, MCP RPC, viewer, build generator, and
validators. Add exactly two shared modules because they eliminate repeated configuration and privacy
logic across many callers.

## Verification strategy

- **Contract checkpoint**: corrected 005 schema/semantic/JS validators, success/failure/suite examples,
  and fingerprint mutation cases all pass before runtime.
- **Manifest**: closed-shape/property rejection, canonical URL, literal loopback, remote HTTPS,
  CredentialRef, both fingerprints, redirect manual rejection, safe disclosure ordering, conflict,
  running-daemon refusal, editor+pointer rollback/interruption-journal recovery with all-target zero
  mutation when any external edit is unknown,
  absent/malformed/frozen daemon states, legacy env mutation invariance.
- **Schema/jobs**: fresh v21, verified v20 migration/backfill/rollback/idempotent reopen/generated DDL,
  complete/missing `gave_up`, capacity including exhausted, 100-event split, stale claims, monotonic
  attempts, one-shot grants, admission/attempt fingerprints, atomic skip/completion/frontier, purge
  exemptions.
- **Privacy**: every sensitivity × destination × repository state across provider, maintenance,
  search/reference, daemon, MCP, viewer, pack/trace, export/import, dedup/supersession; basename
  collision, HTTPS/SSH transport separation, SSH-user non-collision, linked worktree, and origin
  A→B revalidation;
  restricted sentinel absent from wire/render/log/diagnostic/artifact.
- **Lifecycle**: production nudge after commit only, debounce/immediate/request drain, stop race,
  restart, PreCompact, daemon outage/spool replay, no Agent blockage.
- **Semantic**: lexical ready with `semantic_disabled`; vector row count/content unchanged.
- **Release**: build, tsc, lint, focused tests, full serial coverage, no-agent-blockage, generated
  parity, packed artifact, fixed runner validators, correctness/security review, code review, and
  over-engineering review.

## Rollback

- Code rollback is a normal commit revert; never rewrite main.
- Before v21, reverting PR 1 restores legacy runtime code but remote provider execution must remain
  disabled if no validated manifest path exists.
- Manifest rollback atomically restores the prior pointer only while daemon lifecycle is coordinated;
  daemon restart and doctor must return that fingerprint. Referenced generations remain.
- v21 is additive and backup-verified. Any older rollback runtime that lacks DestinationBoundary
  must keep remote provider and restricted disclosure disabled.
- Failed/retry-exhausted jobs and raw sources remain retained. No rollback command rewinds or
  advances the frontier. Legacy missing ranges remain diagnosed, not reconstructed.
- Raw retention stays disabled/0 throughout Slice 1 rollback.
