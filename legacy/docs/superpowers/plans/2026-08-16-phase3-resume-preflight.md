# Phase 3 Resume Preflight Implementation Plan

> **Execution rule:** use `superpowers:subagent-driven-development` or `superpowers:executing-plans`. Every checkbox is a verification boundary. Do not start Phase 3 product tables or RPC implementation until this plan and #1 Stage 0 are complete.

**Goal:** Prove exact Claude/Codex delivery capability and freeze one runtime-neutral contract for task state, task boundaries, pending side effects, terminal correlation, checkpoints, claim/delivery, workspace reconciliation, resume selection, capsule safety, durable-memory history, derived-artifact invalidation, quality reports, and doctor output.

**Toolchain:** Node.js 24.16.0, pnpm 11.8.0, TypeScript via Node type stripping, `node:test`, JSON Schema, isolated real-CLI rigs.

## Constraints

- Preserve all Phase 1 sole-writer, spool, peer-auth, redaction, backup, and fail-open invariants.
- Use only synthetic repositories/data and isolated HOME/config directories for real-CLI evidence.
- Unknown native behavior remains unknown; source inspection cannot promote it.
- No generation, vector, cloud, Python, Redis, or Postgres dependency is required for preflight.
- TypeScript and Rust must consume identical schemas, fixtures, and report hashes.
- `resume_mode=off` disables every automatic delivery, including compact recovery.
- Automatic strategy = exact capability proof ∩ selected mode ∩ explicit delivery boundary ∩ selection/reconciliation result.

## Planned files

Modify:

- `harness/schema/capability.{ts,schema.json}`
- `harness/assemble.ts`
- `harness/rig/{rig.sh,claude-settings-template.json,codex-config-template.toml}`
- `harness/matrix/{claude,codex}.json` (generated)
- `harness/README.md`
- `specs/001-agent-memory-core/tasks.md`
- `.github/workflows/ci.yml`

Create:

- `harness/schema/continuity.{ts,schema.json}`
- `harness/schema/validate.ts`（Task 1 で先行。continuity schema を実行時に検査する土台）
- `harness/schema/capability-scenarios.v1.json`
- `harness/schema/memory-history.{ts,schema.json}`
- `harness/continuity/{capability-contract,contract,reference-model,memory-history}.test.ts`
- `harness/continuity/{validate,fixture-validation}.test.ts`
- `harness/continuity/{reference-model,run-preflight}.ts`
- `harness/phase3-preflight.mjs`
- `harness/fixtures/continuity/*.json`
- `harness/fixtures/memory-history/*.json`
- exact Claude/Codex prompt-aware and compact fixtures/raw captures
- `benchmarks/behavioral/contract.schema.json`
- `benchmarks/behavioral/fixtures/deterministic/resume-core.json`
- `evidence/phase3-preflight-{capability,contract}.md`

---

## Task 1 — Exact-version capability evidence

- [x] Add `ResumeDeliveryStrategy`:

```ts
type ResumeDeliveryStrategy =
  | "native_prompt_gate"
  | "session_start_full"
  | "next_prompt_synthesized"
  | "manual_only";
```

- [x] Add matrix fields:

```ts
resumeDeliveryStrategy: ResumeDeliveryStrategy;
promptDeliveryBeforeModel: CapabilityEvidence;
compactSingleDelivery: CapabilityEvidence;
capabilityHashInputs: string[];
```

- [x] RED: empty matrix resolves to `manual_only`; evidence cells are `unknown`.
- [x] RED: reject a half-proven synthesized prompt path.
- [x] `next_prompt_synthesized` requires both prompt-delivery cells to be synthesized by the same exact-version real-CLI fixture/evidence hash.
- [x] `session_start_full` requires native or synthesized SessionStart real-CLI proof.
- [x] Mirror closed enums/required fields in JSON Schema（対象は fixture 入力側の
      `capability.schema.json`。生成物 `AdapterCapabilities` 側の schema は Task 11 の
      report schema と一緒に作る。それまでは CI の matrix 再生成 diff で手編集を検出する）
- [x] Regenerate matrices; never manually edit generated JSON.

```bash
node --experimental-strip-types --test harness/continuity/capability-contract.test.ts
node --experimental-strip-types harness/assemble.ts --self-test
node --experimental-strip-types harness/assemble.ts harness/fixtures/claude harness/matrix/claude.json
node --experimental-strip-types harness/assemble.ts harness/fixtures/codex harness/matrix/codex.json
```

---

## Task 2 — Claude prompt-aware and compact E2E

- [ ] Isolate `HOME`, `CLAUDE_CONFIG_DIR`, repo, hooks, transcript, and data paths.
- [ ] Use distinct SessionStart hint and first-prompt full tokens.
- [ ] Prove full context is model-visible before the first assistant/tool action, not merely printed by a hook.
- [ ] Prove checkpoint evidence precedes compact completion.
- [ ] Prove exactly one post-compact full delivery and duplicate-hook dedupe.
- [ ] Prove capture/daemon failure returns empty context without blocking Claude.
- [ ] Record exact version, command, source events, transcript hash, evidence hash, and disposition: `proven | unsupported | unknown_after_test`.
- [ ] Regenerate `harness/matrix/claude.json`.

---

## Task 3 — Codex prompt-aware, compact, and identity E2E

- [ ] Isolate `HOME`, `CODEX_HOME`, repo, hooks, transcript, and data paths.
- [ ] Prove first-prompt model visibility before first assistant/tool action.
- [ ] Prove compact persistence, exactly-one restore, or retain honest unsupported/unknown disposition.
- [ ] Run multiple turns plus restart to test usable stable native session identity.
- [ ] Record exact Codex event names rather than translating them into Claude names.
- [ ] Regenerate `harness/matrix/codex.json`.

---

## Task 4 — Freeze the closed continuity schema

Create `harness/schema/continuity.ts` and a matching closed JSON Schema defining:

- `JsonValue`, `Observed<T>`, `Sensitivity`, `Freshness`
- `NormalizedContinuityEvent` with `turnId`/`turnIdSource`, `ContinuityEventProvenanceV1` (incl. daemon-assigned `ContinuityIngestAttestationV1`), and the `ContinuityOperationRefV1` envelope
- `CanonicalWorkStateV1`, `OperationCorrelationV1`, `PendingOperation`
- `SessionTaskBinding`, `TaskBoundaryProposalV1`, `TaskBoundaryDecisionV1`, `TaskBoundaryAuthorityContextV1`
- `ContinuationCheckpointV2`, checkpoint metadata/disposition/projection
- `CheckpointAnchorV1`, engagement/contradiction/evaluation context
- `CheckpointDeliveryAttempt`, every `DeliveryCommandV1` variant with `attemptId`, `ResumeSuppressionEntryV1`
- `WorkspaceReconciliationReport`
- `ResumeMode`, `ResumeDeliveryBoundary`, capability disposition/preflight state
- `ResumeThresholdProfileV1`, ranked candidate, `ResumeSelectionDecisionV1`
- `ResumeCapsuleV1`, owned injection ledger, capture strip result
- `ResumeQualityReportV1`, `ContinuityDoctorReportV1`
- `DerivedArtifactSourceRefV1` and the closure-carrying `DerivedArtifactDependencyV1`
- `RequiredCapabilityScenarioV1`, `CapabilityScenarioManifestV1`

Shared limits:

```ts
const CONTINUITY_LIMITS = {
  hintTokens: 120,
  fullCapsuleTokens: 700,
  promptMemoryTokens: 700,
  combinedTokens: 1500,
  absoluteTokens: 1800,
  capsulePayloadBytes: 32768,
  wrapperBytes: 36864,
  jsonDepth: 12,
  stringUtf8Bytes: 8192,
  arrayItems: 256,
  objectKeys: 128,
  rankedCandidates: 5,
} as const;
```

- [ ] Reject unknown keys, non-JSON values, invalid timestamps/decimal strings, invalid scores, excessive structure, and schema-version mismatch.
- [ ] Use identical limits in runtime validators and JSON Schema.
- [x] Emit schema/fixture SHA-256 values for TypeScript/Rust parity. — `harness/contract-hashes.mjs` が `harness/schema/**` と `harness/fixtures/**` の `.json` / `.jsonl` 27 件の生バイト SHA-256 を出し、`harness/contract-hashes.json` に凍結する。CI の "Contract hashes are regenerated" が再生成との差分を見る。

### 正本に型定義が無いもの（この Task では凍結しない）

上のリストのうち次の 5 つは、addendum v6.2 §2-13 の `ts` ブロックに型定義が存在しない。
transcription の対象が無いので **書かない**（推測で型を起こすと、正本に無いものが
「凍結された契約」の顔をして残る）。

| 項目 | 状況 |
|---|---|
| `WorkspaceReconciliationReport` | 本計画 §Task 6 の関数シグネチャに名前だけ現れる。addendum に型定義なし |
| owned injection ledger | 正確な型名すら定まっていない |
| `CaptureStripResult` | addendum に記述なし |
| `ResumeQualityReportV1` | §14 が `benchmarks/behavioral/contract.schema.json` を **sole machine-readable authority** と定めている。continuity.schema.json 側に重複定義しない |
| `ContinuityDoctorReportV1` | §14 に散文の要件のみ。`doctor continuity --json` を実装する Task で型を決める |

- `MEMO`: `DecimalString` も凍結していない。本計画は score を decimal string と書いているが、
  addendum の該当フィールドはいずれも `number` である（§2.1 / §3 / §4）。正本に合わせた。

---

## Task 5 — Task state, duplicate no-op, operation correlation, and boundary authority

Reference interfaces:

```ts
reduceTaskWorkState(previous, event, idempotencyLedger): TaskStateReductionResult
finalizeAbandonedState(state, event): CanonicalWorkStateV1
correlateTerminalEvent(state, terminalEvent): TerminalCorrelationResult
proposeTaskBoundary(state, event): TaskBoundaryProposalV1 | null
confirmTaskBoundaryAtomically(binding, proposal, decision, authority): BoundaryConfirmResult
rejectTaskBoundary(binding, proposal, decision, authority): BoundaryRejectResult
```

`authority` is `TaskBoundaryAuthorityContextV1`: resolved source events plus agent, exact version, capability hash, proven scenario IDs, and optional user-surface authority.

### Event idempotency

- [ ] Reapply the same `adapterDeliveryId` or canonical fingerprint x10.
- [ ] After the first application, state bytes, content hash, revision pointer, history length, and idempotency ledger remain unchanged.
- [ ] Dedupe occurs before revision allocation.

### Pending-operation terminal correlation

- [ ] Correlation values come only from the typed `NormalizedContinuityEvent.operation` envelope; add negative schema fixtures for an operation event missing the envelope, carrying the values only in `payload`, or declaring a mismatched phase.
- [ ] Start events create `OperationCorrelationV1` with `operationId`, `startEventId`, optional native ID, schema-versioned match key, session, lineage, optional turn/tool/input hash.
- [ ] Exact native operation ID + same session/lineage is first authority.
- [ ] Fallback requires exact operation match key + same session/lineage + compatible turn/kind + exactly one open candidate; `turnIdSource="unavailable"` on either side disables the fallback and leaves the operation `unknown`.
- [ ] Command text, tool name, cwd, or timestamp proximity alone never closes an operation.
- [ ] Terminal event must occur after the start in authoritative event order, be unapplied, and have no payload/source-hash conflict.
- [ ] Zero/multiple matches leave all candidates unknown, preserve unmatched evidence, and emit a diagnostic.
- [ ] Correlation/hash conflict is quarantined.
- [ ] A valid late terminal references original `operationId` and `startEventId`, creates a new work-state revision, and changes only that operation.
- [ ] Crash with no trusted terminal => `unknown / verify_first`; dangerous side effects => `never_auto`.

### Task boundary decisions

- [ ] Heuristics create only a proposal; no binding change.
- [ ] User may confirm/reject any proposal through a user-authoritative surface.
- [ ] `native_runtime` may confirm only `native_fork` or `accepted_resume` proposals whose source events are exact-version capability-proven native events.
- [ ] Authority is verified from resolved source events (`evidenceKind`, `ingestAttestation`, `capabilityHash`, `sourceAgentVersion`, proven `scenarioId`, session), never from the caller-supplied `source` field. Fixtures cover fabricated, unresolved, cross-session, synthesized, and capability-unproven source events.
- [ ] `evidenceKind`/`ingestAttestation` are stamped at daemon intake from the authenticated peer and channel; RED: an event submitted over an ordinary hook/spool path with `evidenceKind="native"`, a copied capability hash, and a proven scenario ID is stored as `synthesized` and cannot confirm a boundary.
- [ ] Agent/model text, similarity scores, `agent_proposal`, and `deterministic_shift` cannot be runtime-confirmed.
- [ ] Confirm atomically validates revisions, marks proposal confirmed, and creates the new binding; it unbinds the old primary only for `proposedRole="primary"`. Confirmation fixtures exercise `primary`, `side`, and `subagent` and assert the primary binding survives the latter two.
- [ ] Reject marks proposal rejected and keeps old binding.
- [ ] Stale/competing/cross-session/unauthorized commands cause no binding change.

---

## Task 6 — Fail-closed workspace reconciliation

```ts
reconcileWorkspace(checkpoint, currentEvidence): WorkspaceReconciliationReport
```

Fixtures cover `exact`, `fast_forward_compatible`, `stale_but_usable`, `requires_verification`, `incompatible`, and unknown/incomplete evidence.

```text
repo/workspace mismatch -> incompatible
unreadable evidence or unknown ancestry -> requires_verification
unproven branch/worktree relation -> requires_verification
diverged HEAD + affected file drift -> requires_verification
pending migration/external side effect -> requires_verification
checkpoint HEAD ancestor + affected files unchanged -> fast_forward_compatible
classified low-risk drift only -> stale_but_usable
all applicable checks positively match -> exact
otherwise -> requires_verification
```

- [ ] `exact` is never a default/fallthrough.
- [ ] Stale/verification capsules turn imperative actions into verification suggestions.

---

## Task 7 — Lineage-aware disposition, initial claim, source-verified engagement, atomic acceptance

Reference interfaces:

```ts
projectCheckpointDisposition(events, checkpointLookup, authority): CheckpointDispositionProjection
claimCheckpointAtomically(input): { attempt; projection }
transitionDeliveryAttempt(attempt, projection, suppressionLedger, commandWithoutAccept): { attempt; projection; suppression?: ResumeSuppressionEntryV1 }
acceptDeliveryAttemptAtomically(input): { attempt; projection; appendedEvent }
```

### Disposition

- [ ] Daemon/runtime supersede requires metadata proving source and related checkpoints share `taskLineageId`.
- [ ] Cross-lineage supersede requires explicit user-authoritative context.
- [ ] Missing related-checkpoint metadata fails closed.

### Initial claim

- [ ] One transaction validates open projection, no active unexpired attempt, destination session/binding, explicit delivery boundary, mode, capability, selection decision, and reconciliation; then creates attempt/fence and updates projection.
- [ ] 100-way race yields exactly one winner.

### Command CAS

- [ ] Every post-claim command includes and validates `attemptId`, attempt revision, fence, and destination session.
- [ ] The transition takes the current projection and returns its updated value, so the CAS and any lease change are part of the frozen contract rather than hidden state: the same transaction checks `projection.state === "open"`, `activeDeliveryAttemptId`, `activeClaimFence`, and an unexpired lease, and `renew_lease` returns the projection with `activeLeaseUntil` advanced.
- [ ] Reclaim terminates the old attempt and rotates the projection's active attempt and fence together. Appending a delivery-invalidating disposition (`superseded`, `expired`, `retracted`) abandons the active attempt and clears the active attempt/fence/lease; acceptance clears the same fields but advances its own attempt to `accepted` (§6.4) instead of abandoning it.
- [ ] Supersede/expire/retract-between-claim-and-delivery fixture: the delayed `mark_delivered` is `stale_attempt` and no injection happens.
- [ ] Reclaim-versus-delivery race fixture: a delayed `mark_delivered`/`record_engagement` from the reclaimed attempt is typed `stale_attempt` and changes nothing.
- [ ] `dismiss` and `abandon` clear the projection's active attempt/fence/lease in the same transaction; `dismiss` additionally returns a `ResumeSuppressionEntryV1` for `(checkpointId, destinationSessionId)` that the same transaction appends to the resume ledger, so the rejecting session stops being an eligible destination and the write is part of the frozen contract rather than a side effect.
- [ ] Immediate-reclaim fixtures: after `abandon` the same session reclaims without waiting for lease expiry; after `dismiss` a different eligible session reclaims immediately while the dismissing session is rejected as ineligible.
- [ ] `renew_lease` is a typed post-claim command under the same CAS; heartbeat-versus-reclaim fixture proves a renewal arriving after reclaim cannot extend the stale attempt's lease.
- [ ] Mismatched attempt ID/revision/fence/session is typed stale/invalid and causes no state change.

### Engagement and acceptance

Use fixed weights from the addendum. Tests must revalidate submitted evidence from actual `NormalizedContinuityEvent` records and checkpoint anchors:

- [ ] destination turn provenance is validated before scoring: `destinationAgentVersion`/`destinationCapabilityHash` match the active matrix and `turnIdentityDisposition` is `proven`; otherwise the automatic path is unreachable regardless of matching events;
- [ ] source event exists, belongs to the destination session, and carries `turnId === destinationTurnId` with `turnIdSource != "unavailable"`;
- [ ] kind and success state match the claimed evidence kind;
- [ ] event is after delivery and within lease/30-minute window;
- [ ] anchors are resolved from the stored checkpoint inside the transaction (each carries `checkpointId`/`checkpointRevision`/`taskLineageId`); a caller-supplied anchor that is not one of them is ignored, and evidence linked to a foreign anchor scores zero;
- [ ] duplicate evidence counts once;
- [ ] failed/unknown/unrelated/out-of-window/fabricated labels score zero;
- [ ] explicit rejection, confirmed other task, incompatible workspace, or runtime invalidation blocks acceptance — the daemon re-queries contradictions from its own event store inside the acceptance transaction, from delivery through a watermark it reads itself (caller ranges are advisory only), scanning `explicit_rejection` and `new_task_confirmed` session-wide and filtering only `runtime_invalidated` to the resumed lineage; RED: a context that omits an existing rejection still fails to accept, and a rejection stored after the submitted `evaluationEndedAt` but before acceptance still blocks it;
- [ ] Agent prose alone cannot accept.

Automatic acceptance requires cumulative score ≥0.80, at least two evidence kinds, at least one successful runtime kind, and no contradiction. Atomic acceptance appends accepted disposition and advances projection+attempt together. Accepted attempt/open projection is impossible.

---

## Task 8 — Explicit-boundary selection and complete capsule lifecycle

```ts
selectResumeAction(input): ResumeSelectionDecisionV1
renderResumeCapsule(capsule, sensitivityPolicy): RenderedCapsule
parseAndStripOwnedResumeCapsules(text, ownedLedger): CaptureStripResult
```

Initial threshold profile:

```text
profileId=resume-v1-preflight
fullResumeMinScore=0.75
hintMinScore=0.35
ambiguityMargin=0.08
maxCandidates=5
```

- [ ] Decision includes explicit boundary, dataset/profile, mode, strategy, capability hash, ranked candidates, scores/margin, confidence, reason codes, selected checkpoint, and fallback.
- [ ] Cross-product fixtures cover every mode × capability × delivery boundary.
- [ ] Same `compact_only` state => none at SessionStart, possible full only at proven post-compact boundary.
- [ ] Low score => none; medium => hint/list; close candidates within margin => candidate list; low/unknown confidence => no full; incompatible/manual-only => no full; verification-required => verification capsule at most.
- [ ] Unknown/unsupported capability never produces automatic full; `always` and `compact_only` remain capability-gated.
- [ ] Normal/private/secret selection follows addendum policy before hash/render. Secret never appears; private needs explicit project opt-in + destination eligibility.
- [ ] Enforce shared structural limits, stable key order, escaping, and mode-aware invalid/oversized fallback.
- [ ] Capture verifies owned ID/hash/bytes/schema and rejects unknown/malformed/nested/mismatched/unsupported capsules without blocking the Agent.
- [ ] Render→capture round-trip cannot create a DurableMemory candidate.

---

## Task 9 — Durable-memory history and causal derived-artifact invalidation

Define and schema-test:

- append-only ADD/UPDATE/SUPERSEDE/RETRACT memory revision events;
- temporal validity/invalidation;
- source evidence links;
- presentation-level `preferConsolidated` behavior;
- `DerivedArtifactDependencyV1` and `DerivedArtifactInvalidationEventV1`.

Derived artifact kinds include summaries, semantic resume notes, checkpoint semantic notes, consolidated memories, embeddings, context-pack caches, and cloud projections.

Tests:

- [ ] Stale expected memory revision is rejected; prior events remain immutable.
- [ ] Invalidated fact remains historical but is excluded/down-ranked from current retrieval.
- [ ] Derived observation retains every source memory/event ID; deleting source evidence violates the invariant.
- [ ] Output dedupe never deletes source records.
- [ ] UPDATE/SUPERSEDE/RETRACT/temporal invalidation atomically marks every known dependent artifact — direct and transitive through artifact-to-artifact edges — stale/invalidated and enqueues deterministic rebuild jobs.
- [ ] Query/injection/resume/embedding/cache/cloud eligibility verifies both direct `sources` and `baseMemoryClosure` revisions **and content hashes** — closure entries carry `contentHash` so a descendant can detect a same-revision content mismatch — so delayed/failed invalidation jobs cannot expose stale artifacts.
- [ ] Multi-hop fixture (memory → consolidated memory → context-pack cache and embedding) proves every descendant is excluded before its invalidation job runs; a dependency cycle or unresolvable closure is quarantined and excluded.
- [ ] Immutable checkpoint canonical state remains historical; stale selected-memory or semantic-note content is omitted/marked stale until re-derived.
- [ ] Embedding coverage is keyed by memory ID + revision + input hash + generation ID.
- [ ] Crash/replay of memory mutation + invalidation converges exactly once.
- [ ] Rebuild creates a new artifact revision/dependency set; failure leaves the artifact excluded while canonical continuation and FTS fallback remain available.

---

## Task 10 — One normative quality-report contract

`benchmarks/behavioral/contract.schema.json` is the machine authority for `ResumeQualityReportV1`.

- [ ] Numeric zero-tolerance counters include duplicate injection, wrong scope, incompatible auto-resume, unsafe unknown replay, early acceptance, accepted-attempt/open-checkpoint, stale fence, capsule escape, malformed capsule trusted, source evidence deletion, and stale derived artifact use.
- [ ] Behavioral metrics include wrong resume, unnecessary hint, candidate accuracy, critical-state recall, fabricated/stale state, re-explanation turns/tokens, first useful action, task completion, hint/full tokens, and claude-mem deltas.
- [ ] `unsupported` is permitted only for declared inapplicable behavioral metrics. A required Phase/Release metric marked unsupported fails the gate.
- [ ] Reference, TypeScript, Rust, and public claude-mem adapters consume the same fixture format.
- [ ] Sorted fixture execution produces byte-identical reports.

```bash
node --experimental-strip-types harness/continuity/run-preflight.ts --out /tmp/a.json
node --experimental-strip-types harness/continuity/run-preflight.ts --out /tmp/b.json
cmp /tmp/a.json /tmp/b.json
```

---

## Task 11 — Preflight states, doctor, evidence, tasks, and CI

- [ ] Freeze `harness/schema/capability-scenarios.v1.json`: a versioned closed manifest of required scenario IDs with `appliesToAgents` and `requiredFor`, plus its `manifestHash`.
- [ ] Record each capability scenario as `not_run | proven | unsupported | unknown_after_test`.
- [ ] Contract preflight is complete only when the report/matrix contains exactly the manifest's applicable scenario IDs per `(agent, exactVersion)` — missing, extra, duplicate, or hash-mismatched sets fail — no required scenario is `not_run`, artifacts exist, and every runtime-neutral fixture passes.
- [ ] RED: dropping one scenario from a generated artifact fails preflight instead of passing vacuously.
- [ ] Unsupported/unknown permits generic/manual implementation but forces automatic-strategy/Tier downgrade.
- [ ] Add P3P barriers to `tasks.md`:

```text
#1 Stage 0 + ContractPreflightState = complete (addendum §13 predicate:
manifest exact-set + manifest hash + artifacts + regenerated matrices +
runtime-neutral fixtures green)
  -> generic Phase 3 implementation may start

exact strategy capability proven
  -> that Agent/version may enable automatic strategy

release E2E + #8 non-inferiority
  -> Tier A / Core 1.0 claim
```

- [ ] `doctor continuity --json` reports exact version/hash, dispositions, strategy, mode, threshold/dataset, last delivery boundary/selection reasons, reconciliation, active attempt/lease, unknown pending count, stale/invalidated derived-artifact counts, unmet gates, and schema/fixture/report hashes.
- [ ] Doctor never emits raw prompts, commands, private/secret values, or capsule content.
- [ ] Evidence files record exact commands, versions, commits, hashes, dispositions, failures, and synthetic fixture links.
- [ ] After ten deterministic runs, CI validates every fixture, report completeness, zero counters, doctor schema, and byte reproducibility.

## Final verification

```bash
cd vendor/codemem
corepack pnpm install --frozen-lockfile
corepack pnpm run build
CI=true corepack pnpm run check
cd ../..
node --experimental-strip-types --test harness/continuity/*.test.ts
node --experimental-strip-types harness/continuity/run-preflight.ts --out /tmp/phase3-preflight.json
git diff --check
```

Expected:

- Existing Phase 1 suite remains green.
- Every deterministic fixture passes.
- Every zero-tolerance counter is zero.
- Every required metric is numeric and present.
- Unknown capability remains visible and downgraded.
- Reports are byte-reproducible.
- No production continuity implementation exists before #1 chooses the runtime.
