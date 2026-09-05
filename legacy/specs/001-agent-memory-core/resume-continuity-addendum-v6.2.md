# Resume Continuity Addendum v6.2

Date: 2026-08-16  
Amended: 2026-08-17 (see §0.1)  
Status: **Normative pre-implementation contract**  
Related: #1, #8, #13  
Research basis: [`evidence/phase3-resume-oss-comparison.md`](../../evidence/phase3-resume-oss-comparison.md)

## 0. Authority and scope

This addendum supplements `agent-memory-final-spec-v6.md` v6.1.

When this addendum conflicts with v6.1, it takes precedence only for:

- §7 adapter capability claims used by continuation;
- §10 SessionWorkState / task-state ownership;
- §11 ContinuationCheckpoint, task boundaries, claim, delivery, acceptance, and resume-mode semantics;
- §17 resume hint, selection, full-resume injection, sensitivity, and serialization;
- §22.6 timestamp encoding, for the continuity contracts defined here. §3 narrows both the accepted
  spelling and, for leap seconds, the set of representable instants; that narrowing is authorized
  here and its consequence is stated in §3;
- §27 Phase 3 / Phase 4 / Core 1.0 continuity quality gates.

All Phase 1 sole-writer, fail-open, spool, redaction, peer-auth, backup, and user-authority invariants remain unchanged.

The contract is runtime-language neutral. The TypeScript reference and Rust candidate MUST implement the same schemas, transition semantics, fixtures, hashes, and reports.

Core 1.0 may claim smooth automatic continuation only for an exact Agent/native-CLI/capability-hash tuple that passes §8, the completed preflight in §13, and the release gates in §14.

### 0.1 Revision log

The document name and section numbering are stable; amendments are recorded here rather than by
renaming the file, so existing references stay valid. Every amendment states whether it ratifies
behaviour the reference implementation had already chosen, or introduces a new rule.

| Date | Change | Issue | Kind |
|---|---|---|---|
| 2026-08-17 | §0 authority scope extended to v6.1 §22.6 timestamp encoding, so §3 may narrow it — including the leap-second instants it makes unrepresentable | #33 | New rule |
| 2026-08-17 | §3 canonical timestamp profile: UTC RFC3339 restricted to `Z` offset and seconds `00`-`59`, with the residual fractional-second ambiguity, the `chrono` offset default, and the leap-second loss all stated | #33 | Ratifies existing behaviour |
| 2026-08-17 | §3 `confidence` range applies to `Observed<T>` only; other `confidence` fields stay unbounded | #30 | Ratifies existing behaviour |
| 2026-08-17 | §4.1 `lastIngestSeq` defined as a monotone watermark, explicitly **not** a state ordering key and **not** a claim of contiguous coverage | #38 | New rule (the field previously had no definition) |
| 2026-08-17 | §4.3 operation-class table location, unmapped-kind defaults, and the sensitivity migration condition | #36 | New rule |
| 2026-08-17 | §4.3 retention and eviction policy for `pendingOperations` at the §10 `arrayItems` limit, with array position (not `startedAt`) as the within-group order | #39 | Ratifies existing behaviour |
| 2026-08-17 | §4.3 the material for the three terminal checks lives on `PendingOperation` (`startIngestSeq`, `startTurnIdSource`, `terminalFingerprint`), all optional, with absence meaning "cannot verify" and a redelivered start never overwriting the start fields | #35, #44 | New rule (the checks previously had no state-side material) |
| 2026-08-17 | §4.3 evidence leaving the live set — evicted operations and orphaned terminals — is preserved in `droppedEvidence`, bounded, front-dropping, identification-only, with both append and overflow reported; recording an orphaned terminal does not consume the delivery key and is not repeated on resend | #43, #39 | New rule |
| 2026-08-18 | §4.1 a record-only quarantine produces a revision without applying the event, so it leaves `lastIngestSeq` unchanged while still advancing `updatedAt` | #39 | Clarifies the #38 rule (the watermark's "applied" was untested against this path) |
| 2026-08-18 | §4.3 the fingerprint check is decided over the whole candidate set — a candidate carrying no fingerprint makes the terminal explainable — and a `startIngestSeq` that is not a decimal string counts as absent rather than raising | #44, #35 | Clarifies the #35/#44 rule (both cases were left to the implementation and both were wrong) |
| 2026-08-18 | §4.1 a caller MUST take the returned state whatever the outcome; "quarantined" bounds the delivery key, not the state | #39 | Clarifies the #39 rule (recording made one outcome cover two state behaviours) |
| 2026-08-18 | §4.3 a missing `startTurnIdSource` leaves the operation `unknown` with a turn-unverifiable diagnostic; the rule 2 exemption governs candidate selection, not the verdict | #35 | Clarifies the #35 rule (the exemption was written without saying what the terminal then does, and the implementation closed) |
| 2026-08-18 | §4.3 the eviction policy points at `droppedEvidence` for the evicted entry instead of deferring the placement; only `unknown` expiry stays open | #43, #44 | Corrects stale text (the placement was settled in the same amendment round) |
| 2026-08-18 | §4.3 an orphan record is deduplicated by `canonicalFingerprint`, not by `eventId`, because a redelivery is a fresh envelope | #39 | Corrects the #39 rule (the `eventId` key does not converge, measured) |
| 2026-08-18 | §4.3 a terminal whose matched candidates are all settled and none turn-compatible is recorded too; "matched no candidate at all" was too narrow, and the wider rule is "no operation could retain it" | #43 | Corrects the #43 rule (the narrow reading let a terminal leave the state unrecorded) |
| 2026-08-18 | §4.1 the versioned schema block now carries the three optional `PendingOperation` fields, `DroppedEvidenceEntryV1`, and the optional `droppedEvidence` array, matching the machine-readable schema | #35, #43, #44 | Corrects the block (a port reading only this document could not store the material §4.3 requires) |
| 2026-08-18 | §4.3 an orphan whose delivery id matches a record but whose fingerprint differs is reported as `delivery_conflict` rather than suppressed as a duplicate | #39 | Clarifies the same-day key rule (the delivery-first key would otherwise hide the corruption the ledger path already names) |
| 2026-08-18 | §4.3 an evicted record also carries the evicted operation's start event id, because `operationId` is not unique and id-plus-status cannot say which sibling left the live set | #43 | Corrects the #43 rule (the audit record was ambiguous exactly in the duplicate-id states the reducer supports) |
| 2026-08-18 | §4.3 the orphan record's duplicate key follows §8.2's order — delivery id first, fingerprint as fallback, separate keyspaces — because the fingerprint alone collapsed 300 distinct deliveries into one record and the delivery id alone stops the honest resend loop converging | #39 | Corrects the same-day `canonicalFingerprint` rule (both single-value keys lose one direction, measured) |
| 2026-08-18 | §4.3 the over-limit repair is stated by the path that performs it (every start, every recorded terminal) instead of by "the next event that appends", and the paths that still carry an over-limit array forward are named | #43 | Corrects the #43 wording (the prose contradicted the implementation and FR-017 forbids leaving that) |
| 2026-08-18 | §4.3 the fingerprint check is decided over one element when correlation selects a single open candidate, so a restored open operation's fingerprint is a conflict rather than something the closing path overwrites | #44 | Extends the same-day set-wide rule to the branch it never covered (measured: the recorded fingerprint was replaced with no diagnostic) |

## 1. Separation of concerns

`free-mem` MUST keep these independent:

1. **Task execution state** — current work and unresolved side effects.
2. **Continuation checkpoint** — immutable point-in-time task state.
3. **Durable memory** — long-lived knowledge searched across tasks/sessions.

A DurableMemory or summary MUST NOT substitute for a checkpoint. Generation, embedding, rerank, and sync failures MUST NOT prevent deterministic continuation.

## 2. Task lineage and boundary decisions

### 2.1 Canonical unit

Canonical mutable work state belongs to a `taskLineageId`, not directly to a session. A session may touch multiple lineages, but Core 1.0 permits at most one active primary binding.

```ts
type TaskBindingRole = "primary" | "side" | "subagent";
type BoundaryEvidenceKind =
  | "explicit_user"
  | "native_fork"
  | "accepted_resume"
  | "agent_proposal"
  | "deterministic_shift";

type TaskBoundaryProposalState = "proposed" | "confirmed" | "rejected";

interface BoundaryEvidence {
  kind: BoundaryEvidenceKind;
  sourceEventIds: string[];
  proposedAt: string;
  confidence?: number;
}

interface SessionTaskBinding {
  sessionId: string;
  taskLineageId: string;
  role: TaskBindingRole;
  boundAt: string;
  unboundAt?: string;
  boundaryEvidence: BoundaryEvidence;
  revision: string;
}

interface TaskBoundaryProposalV1 {
  proposalId: string;
  sessionId: string;
  currentTaskLineageId: string;
  proposedTaskLineageId: string;
  proposedRole: TaskBindingRole;
  evidence: BoundaryEvidence;
  state: TaskBoundaryProposalState;
  revision: string;
}

type TaskBoundaryDecisionSource = "user" | "native_runtime";

type TaskBoundaryDecisionV1 =
  | {
      kind: "confirm";
      proposalId: string;
      expectedProposalRevision: string;
      expectedBindingRevision: string;
      source: TaskBoundaryDecisionSource;
      sourceEventIds: string[];
    }
  | {
      kind: "reject";
      proposalId: string;
      expectedProposalRevision: string;
      expectedBindingRevision: string;
      source: TaskBoundaryDecisionSource;
      sourceEventIds: string[];
    };

interface TaskBoundaryAuthorityContextV1 {
  sourceEvents: NormalizedContinuityEvent[];
  agent: string;
  exactAgentVersion: string;
  capabilityHash: string;
  provenScenarioIds: string[];
  userSurfaceAuthority?: {
    surface: "cli" | "viewer" | "mcp_user_authority";
    grantedAt: string;
  };
}
```

### 2.2 Boundary rules and authority

- `explicit_user`, `native_fork`, and `accepted_resume` may establish a new primary binding.
- Heuristics may only create a proposal; they cannot supersede, retract, unbind, or delete the old lineage.
- Short acknowledgements such as `yes`, `continue`, or `ok` do not create a new task.
- A **user** may confirm or reject any visible proposal through a user-authoritative surface.
- `native_runtime` may confirm only a proposal whose evidence kind is `native_fork` or `accepted_resume`, and only when every `sourceEventId` resolves to an exact-version capability-proven native event for that session. It may not confirm `agent_proposal` or `deterministic_shift`.
- Agent/model output, prompt-derived classifications, semantic similarity, or heuristic goal-shift scores never constitute `native_runtime` confirmation.
- Confirm and reject take `TaskBoundaryAuthorityContextV1`. Decision authority is verified from resolved source events, never from the caller-supplied `source` field. For `source="native_runtime"` the daemon MUST verify, for every `sourceEventId`, that the resolved event exists, belongs to the binding session, and satisfies §3.1 native authority (`evidenceKind="native"`, `capabilityHash` equal to the context hash, `sourceAgentVersion` equal to `exactAgentVersion`, `scenarioId` in `provenScenarioIds`). Any unresolved, fabricated, cross-session, synthesized, or capability-unproven source event rejects the decision with no binding change.
- For `source="user"` the daemon MUST verify `userSurfaceAuthority` came from a user-authoritative surface; an agent-callable surface cannot supply it.
- Confirm validates proposal/session/binding revisions, marks the proposal confirmed, and creates the new binding in one daemon transaction. The old primary binding is unbound **only** when `proposedRole = "primary"`. Confirming `side` or `subagent` adds the proposed binding in that role and keeps the existing primary binding intact; it never promotes the proposed lineage to primary.
- Reject validates the same revisions, marks the proposal rejected, and leaves the old binding unchanged.
- Stale, competing, cross-session, unsupported-authority, or invalid confirm/reject commands are rejected with no binding change.

## 3. Shared evidence types

**Canonical timestamp profile.** Every field typed as an ISO timestamp in this addendum is UTC
RFC3339 as required by v6.1 §22.6, narrowed by removing two sources of alternative spellings:

- the offset MUST be the literal `Z`; numeric zero offsets such as `+00:00` are rejected;
- the seconds component MUST be `00`-`59`; the RFC3339 leap-second spelling `:60` is rejected;
- the fractional part is optional and its digit count is not fixed (RFC3339 `time-secfrac`).

The narrowing exists because `updatedAt` and the other timestamp fields are inputs to the canonical
content hash. Two spellings of the same instant produce two hashes, which splits both the hash and
the v6.1 §22.8 dedupe decision while every value still validates.

**The offset narrowing is not free, and the common formatter defaults are on both sides of it.**
`Date.prototype.toISOString()` emits `Z` and needs nothing. Rust `chrono`'s `to_rfc3339()` emits a
**numeric** offset — its own documentation gives `1996-12-19T16:39:57-08:00`, and a `DateTime<Utc>`
accordingly renders `+00:00`, which this profile rejects. A `chrono` adapter MUST therefore use
`to_rfc3339_opts(secform, /* use_z */ true)`, or normalize the offset before the value reaches the
contract. Any other formatter MUST be checked against this profile rather than assumed compatible.

**The leap-second narrowing removes instants, not just spellings.** RFC3339 `:60` is the only way to
write a leap second, so rejecting it means an event captured during one has no representation that
denotes the same instant; the nearest accepted values name the adjacent second. This is authorized
in §0 rather than being an accident of the pattern. The trade is deliberate: the leap-second
spelling would otherwise reach the content hash, where two runtimes that disagree about whether to
fold `:60` into the next second would produce two hashes for what they each believe is one instant.
Adapters that receive a `:60` value MUST reject it at intake and MUST NOT silently rewrite it,
because rewriting relabels the instant without recording that it happened.

**This profile does not make the encoding fully canonical.** The fractional part is deliberately
left variable, because fixing the digit count at three would reject the valid microsecond
timestamps that some runtimes emit. `2026-08-16T00:00:00Z` and `2026-08-16T00:00:00.000Z` therefore
remain two spellings of one instant and still split the hash. Closing that requires either fixing
the precision (rejecting values that are valid today) or normalizing before hashing; both are
deferred and tracked in #54. Until then, a single adapter MUST be self-consistent in whether it
emits a fractional part, and implementations MUST NOT rely on hash equality to decide that two
states describe the same instant.

The profile is expressible as a pattern and is enforced by the frozen schema. **Calendar existence
is not** — `2026-02-30T00:00:00Z` and `2027-02-29T00:00:00Z` satisfy the pattern. Implementations
MUST reject non-existent instants at the runtime validation layer, before the value reaches the
reducer (#27); a JavaScript `Date` silently rolls such a value into the following month rather than
raising.

**Numeric ranges.** `Observed<T>.confidence` is the only confidence field with a defined range
(`0..1`). `BoundaryEvidence.confidence` and `SemanticResumeNoteV1.confidence` are deliberately left
unbounded: no rule in this addendum or in v6.1 branches on their magnitude, and inventing a range
here would make the frozen schema stricter than the contract it encodes (#30). Implementations MUST
NOT reject values outside `0..1` for those two fields.

```ts
type JsonPrimitive = string | number | boolean | null;
type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };
type EvidenceKind = "native" | "synthesized" | "derived";
type Freshness = "current" | "stale" | "unknown";
type Sensitivity = "normal" | "private" | "secret";

interface Observed<T extends JsonValue> {
  value: T;
  sourceEventIds: string[];
  ingestSeq: string;
  observedAt: string;
  evidenceKind: EvidenceKind;
  confidence: number; // 0..1
  freshness: Freshness;
  truncated: boolean;
  sensitivity: Sensitivity;
}

type ContinuityCaptureMethod =
  | "native_event"
  | "hook"
  | "plugin"
  | "transcript_scan"
  | "user_surface";

interface ContinuityIngestAttestationV1 {
  ingestReceiptId: string;
  peerIdentityId: string;
  channel: "rpc" | "spool";
  attestedAt: string;
}

interface ContinuityEventProvenanceV1 {
  sourceAgentVersion: string;
  evidenceKind: EvidenceKind;
  captureMethod: ContinuityCaptureMethod;
  capabilityHash?: string;
  scenarioId?: string;
  ingestAttestation?: ContinuityIngestAttestationV1;
}

type TurnIdSource = "native" | "synthesized_monotonic" | "unavailable";

type ContinuityOperationPhase = "start" | "progress" | "terminal";

interface ContinuityOperationRefV1 {
  phase: ContinuityOperationPhase;
  operationMatchKey: string;
  operationKind: string;
  nativeOperationId?: string;
  canonicalInputHash?: string;
}

interface NormalizedContinuityEvent {
  eventId: string;
  adapterDeliveryId?: string;
  canonicalFingerprint: string;
  kind: string;
  ingestSeq: string;
  occurredAt: string;
  sessionId: string;
  taskLineageId?: string;
  turnId?: string;
  turnIdSource: TurnIdSource;
  sourceAgent: string;
  provenance: ContinuityEventProvenanceV1;
  operation?: ContinuityOperationRefV1;
  payload: JsonValue;
  successful?: boolean;
}
```

Evidence certainty never grants instruction authority. Model output is always `derived` and retains provider/model/prompt/schema/source provenance.

### 3.1 Provenance, turn identity, and operation envelope

- Every adapter MUST populate `provenance`. `sourceAgentVersion` is the exact CLI version recorded at capture; a missing or non-exact version makes the event non-authoritative for every rule that requires native authority.
- **`evidenceKind` and `ingestAttestation` are assigned by the daemon intake layer, never trusted from the caller.** Intake overwrites any adapter-supplied value: it derives the kind from the authenticated peer identity (§Phase 1 peer auth), the ingest channel, the declared `captureMethod`, and the capability matrix, then stamps `ingestAttestation` with the receipt of that authenticated ingestion. An adapter can therefore claim a capture method, but it cannot self-declare native authority.
- `provenance.evidenceKind = "native"` additionally requires `ingestAttestation` to be present, `capabilityHash` equal to the active exact-version capability matrix hash, and `scenarioId` naming a scenario whose disposition for that exact version is `proven` **for that `captureMethod`/channel pair**. An event that fails any check is `synthesized` regardless of what the caller sent.
- Required negative fixtures: an event submitted with `evidenceKind="native"`, a copied capability hash, and a proven scenario ID over an ordinary hook/spool path is ingested as `synthesized` and cannot confirm a task boundary.
- Turn identity is canonical, not a payload convention. `turnIdSource="native"` requires a native turn identifier proven for that exact version; `synthesized_monotonic` is an adapter-assigned monotonic turn counter per `(sessionId, user-prompt boundary)`; `unavailable` means the adapter cannot establish turn identity.
- `turnId` MUST be present when `turnIdSource` is `native` or `synthesized_monotonic`, and MUST be absent when it is `unavailable`.
- **Intake also derives the session binding.** In addition to the peer identity, channel, `captureMethod`, and capability matrix, intake resolves the `sessionId` that the authenticated peer is entitled to write and compares it with the caller-supplied `sessionId`. A mismatch is not silently rewritten and not downgraded: the event is refused at intake, because `sessionId` is a **scope selector** rather than an evidence-quality claim — §4.3 narrows terminal candidates and abandonment by it, so a downgraded event carrying someone else's session still selects the wrong operations. This closes the gap that §4.3's correlation scope ("same session/task lineage") assumed but §3.1 did not grant. Note precisely what is missing: canonical work state does carry a session **inside each pending operation's correlation**, but that value is an unverified echo of an earlier event's own claim — whoever forged the start also set the value being compared against. Only intake sees anything outside the event stream, so only intake can turn that echo into a binding. Two conditions govern the refusal, and they are independent. It fires **only when intake holds a non-blank binding** — an ingest path that cannot name the entitled session (spool, or any path not yet wired) leaves the binding blank and every event passes through as before. When the binding *is* present the refusal is **unconditional in the other direction**: it does not additionally require an authenticated attestation, so an unauthenticated path that nonetheless declares an entitled session still refuses mismatches. Unlike the `sourceAgent` binding, a blank `expectedSessionId` leaves evidence quality untouched rather than demoting it to `synthesized`. As with the `sourceAgent` binding, an ingest path that cannot name the entitled session (blank) keeps the previous behaviour, and refusal is safe because intake precedes the delivery ledger — the key is not consumed and a corrected resend still applies.
- **The downgrade authority over `turnIdSource` belongs to intake, not to the delivery layer.** An event that claims `turnIdSource="native"` without a native turn identifier proven for that exact version is ingested as `unavailable`, and therefore MUST NOT carry `turnId` (previous bullet). The caller's claim is not preserved: neither the normalized event nor `CanonicalWorkStateV1` has a field for a rejected turn claim, and adding one would let a downstream rule read the unproven value. The fact of the downgrade is reported as a diagnostic. §6.3's `turnIdentityDisposition` is **complementary, not an alternative**: it gates automatic acceptance on the *destination* Agent/version, while this rule governs what an *ingested* event is allowed to claim. An event can be downgraded here and still meet §6.3, or survive here and fail §6.3; neither check substitutes for the other. The destination half is normative but not yet implemented — `turnIdentityDisposition` exists as a schema field with no consuming logic — so only the intake half of this complementarity is exercisable today.
- **`synthesized_monotonic` carries no authentication requirement, and intake does NOT downgrade it on an unauthenticated path.** It is defined by what the adapter did (a monotonic counter per `(sessionId, user-prompt boundary)`), not by who the adapter is. Intake MUST report an unauthenticated `synthesized_monotonic` claim as a diagnostic and MUST NOT rewrite it to `unavailable`. The residual exposure is stated rather than hidden: an event from an unauthenticated path may satisfy §4.3 rule 2's turn-compatibility condition against an existing `synthesized_monotonic` start in the same session and lineage. Downgrading instead would fail closed on the ordinary path as well: the capability matrix has no cell for turn identity, so no adapter can **declare** `native` turn identity today even where the underlying capability is evidenced (the Codex matrix already records a source-tested `turn_id` on its stop payload). Every adapter is therefore on `synthesized_monotonic`, and the rule would disable rule 2 outright rather than narrow it. Revisit once the matrix gains that cell.
- Every rule that requires turn scoping (engagement destination turn, terminal-correlation turn compatibility) fails closed against `unavailable`: the event cannot satisfy the requirement, the affected automatic path is downgraded, and the reason is reported by doctor.
- `operation` is REQUIRED for events whose kind is an operation `start`, `progress`, or `terminal`, and is the only frozen source for `operationMatchKey`, `nativeOperationId`, and operation kind. Correlation logic MUST NOT read these values from `payload`; an operation event without a valid `operation` envelope is a schema violation.

## 4. Canonical task work state

### 4.1 Versioned schema

`canonicalStateJson: unknown` is not a Core 1.0 contract.

```ts
interface ObservedFile {
  path: string;
  role: "active" | "modified" | "read" | "test" | "config" | "unknown";
  contentHash?: string;
  existsAtObservation: boolean;
  sourceEventIds: string[];
  observedAt: string;
  freshness: Freshness;
  sensitivity: Sensitivity;
}

interface ObservedCommand {
  operationId: string;
  commandDisplay: string;
  cwd?: string;
  exitCode?: number;
  status: "succeeded" | "failed" | "unknown";
  sourceEventIds: string[];
  observedAt: string;
  evidenceKind: EvidenceKind;
  sensitivity: Sensitivity;
}

interface ObservedTest {
  operationId: string;
  commandDisplay?: string;
  target?: string;
  status: "passed" | "failed" | "partial" | "unknown";
  summary?: string;
  sourceEventIds: string[];
  observedAt: string;
  evidenceKind: EvidenceKind;
  sensitivity: Sensitivity;
}

interface OperationCorrelationV1 {
  operationId: string;
  startEventId: string;
  nativeOperationId?: string;
  operationMatchKey: string;
  sessionId: string;
  taskLineageId: string;
  turnId?: string;
  toolName?: string;
  canonicalInputHash?: string;
}

type ReplayPolicy = "never_auto" | "verify_first" | "safe_idempotent";

interface PendingOperation {
  operationId: string;
  correlation: OperationCorrelationV1;
  kind:
    | "command"
    | "file_mutation"
    | "test"
    | "tool"
    | "migration"
    | "external_side_effect"
    | "other";
  description: string;
  status: "started" | "succeeded" | "failed" | "unknown";
  replayPolicy: ReplayPolicy;
  sourceEventIds: string[];
  startedAt: string;
  terminalAt?: string;
  idempotencyKey?: string;
  verificationHint?: string;
  sensitivity: Sensitivity;
  // §4.3 の三検査を状態だけで行うための材料。すべて任意で、不在は「検証不能」であって合格ではない
  startIngestSeq?: string;
  startTurnIdSource?: TurnIdSource;
  terminalFingerprint?: string;
}

interface DroppedEvidenceEntryV1 {
  reason: "evicted" | "orphaned_terminal";
  recordedAt: string;
  sensitivity: Sensitivity;
  // この記録を名指す event。孤児は terminal、退避は operation の start
  eventId?: string;
  operationId?: string;
  status?: "started" | "succeeded" | "failed" | "unknown";
  // 孤児の重複判定の鍵。優先順位は §8.2 と同じ（配送鍵 → 指紋）
  terminalFingerprint?: string;
  adapterDeliveryId?: string;
}

interface RepositoryStateSnapshot {
  repositoryId: string;
  workspaceId: string;
  branchKey?: string;
  worktreeId?: string;
  headSha?: string;
  upstreamSha?: string;
  dirtyTreeFingerprint?: string;
  gitStatusSummary?: string;
  capturedAt: string;
}

interface SemanticResumeNoteV1 {
  schemaVersion: 1;
  goal?: string;
  completed: string[];
  currentState?: string;
  nextActions: string[];
  blockers: string[];
  unresolvedQuestions: string[];
  providerId: string;
  modelId: string;
  promptHash: string;
  generatedFromIngestSeq: string;
  confidence: number;
  sourceEventIds: string[];
  sensitivity: Sensitivity;
}

interface CanonicalWorkStateV1 {
  schemaVersion: 1;
  taskLineageId: string;
  projectId: string;
  workspaceId: string;
  branchKey?: string;
  sourceAgent: string;
  latestSubstantivePrompt?: Observed<string>;
  lastAssistantConclusion?: Observed<string>;
  nativeTodoState?: Observed<JsonValue>;
  activeFiles: ObservedFile[];
  modifiedFiles: ObservedFile[];
  recentCommands: ObservedCommand[];
  recentTests: ObservedTest[];
  pendingOperations: PendingOperation[];
  // live な集合から落ちた証跡。`pendingOperations` と同じ §10 `arrayItems` で有界
  droppedEvidence?: DroppedEvidenceEntryV1[];
  repositoryState: RepositoryStateSnapshot;
  semanticResumeNote?: SemanticResumeNoteV1;
  sensitivity: Sensitivity;
  lastIngestSeq: string;
  stateRevision: string;
  updatedAt: string;
}
```

`lastIngestSeq` is a **monotone watermark**: the highest `ingestSeq` of any event applied to this
task state. It never moves backwards, so applying a late-arriving event whose `ingestSeq` is lower
than the current value leaves the field unchanged while still producing a later revision (§4.2).
`updatedAt` is not monotone — it records the `occurredAt` of the event that produced the revision,
which is adapter-supplied and may move backwards for the same reason.

**"Applied" is the operative word, and a revision is not proof of application.** A quarantined event
that still writes a `droppedEvidence` row (§4.3, orphaned terminal) produces a later revision without
being applied: its delivery key is not consumed, so the same event can still be applied later once
its `start` arrives. Such a path MUST leave `lastIngestSeq` unchanged and MUST still advance
`updatedAt` — the record is what made this revision, but the event is not yet in the state. Moving
the watermark there would have the state claim a maximum applied position it has not reached, and
the subsequent real application would then look like a late arrival and leave it unchanged.

It follows that **quarantining bounds what happened to the delivery, not what happened to the state**,
and a caller MUST take the state a reduction returns regardless of its outcome. Reading "quarantined"
as "nothing changed" and keeping the prior state discards the `droppedEvidence` row — reintroducing
exactly the silent loss the record exists to prevent — and leaves the caller holding a revision the
reducer has already moved past, so its next compare-and-set fails. The outcome answers one question
only: was the delivery key consumed, and may a corrected event still arrive under it.

**`lastIngestSeq` is not a state ordering key, and neither is anything else in this schema.** Three
properties block that reading:

- v6.1 §8.3 assigns `ingest_seq` transactionally **per session**, while a task lineage continues
  across sessions. Values from two sessions are drawn from unrelated sequences, so comparing them
  is meaningless. A resumed session's first event can carry an `ingestSeq` far below the previous
  session's last one — the previous session ended at 100, the resumed one starts at 1 — with no
  relation to which is newer. The monotone rule still holds on the state: that event leaves the
  watermark at 100 and produces a later revision. The consequence is that the watermark of a
  lineage that has spanned sessions is a maximum over values drawn from unrelated sequences, and
  therefore does not identify which session, or which event, it came from.
- Even inside one session, a late event produces a later revision while leaving the watermark
  unchanged (above). Equal watermarks therefore do not imply equal states.
- `stateRevision` is a hash chain over the previous revision, the event, and the content. It
  establishes **ancestry** — a consumer holding both revisions can prove one descends from the
  other by replay — but two revisions are not comparable without that replay.

Consumers MUST NOT decide "which state is newer" from `lastIngestSeq`, and MUST NOT use
`updatedAt` for it either (it is adapter-supplied, non-monotone, and attacker-influenceable through
`occurredAt`). Ordering across sessions of a lineage needs a lineage-global mechanism that this
schema does not yet define (#53); it MUST be settled before any consumer implements
last-writer-wins over task states.

**`lastIngestSeq` also says nothing about contiguous coverage.** It is the maximum applied value,
not a high-water mark of a gap-free prefix. Applying sequence 10 before sequence 9 leaves the
watermark at 10 while 9 is still unseen, and the session's sequence carries events for every
lineage in that session, not only this one. A consumer MUST NOT use it to decide that replay or
reconciliation can be skipped. Tracking which events are still missing requires a separate
mechanism that this schema does not define (#53).

The §6.4 event-store watermark is a different quantity — it covers the whole session and lives
inside the daemon, not in the wire contract — and the two MUST NOT be compared or substituted for
each other.

### 4.2 Immutable revisions and duplicate no-op

- Work-state revisions are immutable; one pointer selects the current revision per lineage.
- Dedupe authority is `adapterDeliveryId` or the v6.1 canonical event fingerprint.
- Dedupe is checked **before** allocating a revision.
- A duplicate logical event is a no-op: return the same state bytes, content hash, revision pointer, and history.
- Late terminal/correction events create later revisions without rewriting source evidence.
- Semantic refinement cannot alter canonical observed fields.

### 4.3 Pending-operation start and terminal correlation

- A stable start event creates `OperationCorrelationV1` and `PendingOperation(status="started")`.
- Both start and terminal events carry their correlation values in the frozen `NormalizedContinuityEvent.operation` envelope (§3.1). `OperationCorrelationV1` is populated from that envelope plus the event's `sessionId`, `taskLineageId`, and `turnId`; no correlation value is ever extracted from `payload`.
- `operationMatchKey` is a schema-versioned SHA-256 over the normalized Agent, native session, task lineage, turn when present, operation/tool kind, stable native operation ID when present, and canonical input hash. Volatile time and delivery-attempt fields are excluded.
- Terminal matching authority is ordered:
  1. exact `nativeOperationId` + same session/task lineage;
  2. otherwise exact `operationMatchKey` + same session/task lineage + compatible turn/kind, with exactly one open candidate;
  3. otherwise no match.
- Turn compatibility in rule 2 requires both events to carry a turn identity of the same `turnIdSource` kind. When either side is `unavailable`, rule 2 does not apply and the operation stays `unknown`; only rule 1 can close it.
- Command text, tool name, timestamp proximity, or cwd alone are never sufficient.
- A terminal event must occur after the start event in authoritative event order, must not already be applied, and must have a non-conflicting payload/source hash.
- **The state carries the material for all three of those checks, so an implementation handed only a canonical work state can perform them.** Each pending operation records the ingest sequence and turn-identity kind of its own start (`startIngestSeq`, `startTurnIdSource`), and each operation closed by a terminal records that terminal's canonical fingerprint (`terminalFingerprint`). The material lives on the element rather than in a side index keyed by `operationId`, because the frozen schema does not require `operationId` to be unique: with a key, a state holding two pending operations of the same id makes the material's owner undecidable, and reading it anyway lets one sibling's start order govern the other's terminal. All three fields are optional. A state written before this amendment, or by an implementation that does not populate them, simply lacks them, and the missing material must be treated as "cannot verify" — never as a passed check. Concretely: a missing `startIngestSeq` leaves the operation `unknown` with an order-unverifiable diagnostic, a missing `startTurnIdSource` exempts the candidate from rule 2's kind comparison rather than excluding it — and, because the comparison is then unperformed rather than satisfied, the operation is left `unknown` with a turn-unverifiable diagnostic instead of being closed. The exemption governs which candidates are considered, not the verdict: excluding the candidate would misattribute the terminal, while closing on it would make absent material a passed check, and only rule 1 — which requires no turn compatibility — closes such an operation. A missing `terminalFingerprint` falls back to the previous already-applied behaviour. A redelivered start never overwrites the two start fields: `ingestSeq` is assigned by the event store at ingest, so a redelivery carries the position of the redelivery, not the authoritative order of the original start.

  Two consequences of the material being optional bind the fingerprint check in particular. **The check is decided over the whole candidate set, not over one element.** Rule 2 collects every sibling sharing the `operationMatchKey`, and when they are all closed there is no way to tell which of them a redelivered terminal belongs to. The terminal is therefore explainable — and MUST NOT be quarantined — if any candidate either carries a matching fingerprint or carries none at all; a candidate carrying no fingerprint is exactly the "material absent" case, and FR-012's rule that the check does not fire without material applies to it whether or not a *different* sibling happens to carry one. Quarantining on a sibling's material would deny the delivery key to a healthy redelivery, and since quarantine does not consume the key and the reducer is pure, the adapter would resend forever. **Where correlation has narrowed to exactly one open candidate, the same check is decided over that element.** The set-wide rule exists because an all-closed candidate set cannot say which sibling a redelivered terminal belongs to; two or more open candidates are already ambiguous and close nothing, so a single selected open operation leaves no doubt about the terminal's owner. A fingerprint that operation already carries and an incoming one that disagrees are therefore a conflict on that operation, quarantined and not recorded, rather than an overwrite. The frozen schema does not stop a `started` or `unknown` element from carrying `terminalFingerprint`, so this arises whenever the state was restored or written by another implementation, and without the check the closing path replaces the recorded value and the earlier terminal's only evidence is lost. The check does not fire when the incoming terminal degrades to `unknown`: that path writes no fingerprint, and the degradation is a property of the event itself, so quarantining it would return the same verdict on every redelivery and the operation would never close. **And the material is only as trustworthy as the state.** `startIngestSeq` is compared rather than tested for equality, so a value that is not a decimal string cannot be compared at all; such a value MUST be treated as absent rather than raising, because the comparison runs across the candidate set and a single malformed element would otherwise take down terminals aimed at its siblings. Beyond that spelling check, the reducer does not validate restored material: an implementation that can write `startIngestSeq` can equally write `status`, so the guarantee that the state is authoritative belongs to whatever produced the checkpoint.
- A terminal event that matches zero or multiple open operations does not close any operation. Preserve it as unmatched evidence, leave candidates `unknown`, and emit a diagnostic. A correlation/hash conflict is quarantined according to the event-conflict contract.
- **Evidence that leaves the live set is preserved in the state itself, bounded.** Two things leave it: operations evicted at the §10 `arrayItems` limit, and terminal events that no operation in the live set could retain. The second covers two spellings — no candidate matched at all, and every candidate that matched was already settled with none turn-compatible — because from the state's point of view they are the same loss, and both are recorded under `orphaned_terminal`. A terminal that leaves at least one *open* candidate is not recorded: that candidate goes to `unknown` and so carries the event's effect, and the terminal has not left the live set. Both are appended to `droppedEvidence` with the reason, an identifying event id — the start's for an evicted operation, the terminal's for an orphan, because `operationId` is not unique in the frozen schema and id-plus-status cannot name which of two siblings left — the time of the event that caused the drop, and a sensitivity — inherited from the evicted operation, or the fail-closed default for an orphaned terminal, which has no operation to inherit from. The record carries identification and classification only: never payload, arguments, output, or description. The array is bounded by the same §10 `arrayItems` limit and drops from the **front** when full, matching the `pendingOperations` policy and for the same reason (`recordedAt` is a copy of an adapter-supplied `occurredAt`, so ordering by it lets the emitting side choose what survives). Both the append and the eviction are reported as diagnostics; a record that vanishes silently would reproduce, one level down, the gap this array exists to close. Recording an orphaned terminal does **not** consume the delivery key — the event is still quarantined, so a start arriving later can still close it — and a terminal already present in the record is not appended twice, since quarantine leaves the key unconsumed and a pure reducer would otherwise grow the array on every resend.

  **A record is deduplicated by the terminal's own identity, not by the envelope's `eventId`.** The delivery key is `adapterDeliveryId` precisely because a redelivery is a fresh envelope — a redelivered terminal may carry a new `eventId`, `ingestSeq`, and `occurredAt`. Keying the duplicate check on `eventId` therefore records the same terminal again on every redelivery, and because the quarantine never consumes the delivery key the resends do not stop: the array saturates at `arrayItems` but the revision and the history keep moving, so every compare-and-set token minted downstream misses (measured: 300 redeliveries of one terminal, identical `adapterDeliveryId` and fingerprint, produced 300 revisions and 300 history rows with an empty ledger). The key MUST follow §8.2's order — `adapterDeliveryId` first, `canonicalFingerprint` only when no delivery id is declared — and MUST keep the two in separate keyspaces, exactly as the ledger key does. Neither half alone is sufficient, and both failures were measured. Keyed on the fingerprint alone, three hundred orphans with distinct delivery ids and distinct `operationMatchKey`s collapsed into one record because they all declared the same fingerprint: two hundred and ninety-nine terminals left the state with nothing recorded and no diagnostic, contradicting §8.2's own rule that a different delivery id means a different logical event. Keyed on the delivery id alone — or on `eventId`, which was the first attempt — the honest resend loop stops converging. The fingerprint is a value the adapter computes and puts on the wire, so it cannot be the sole authority for identity; the delivery id is the authority the rest of the reducer already uses. A record that declares neither is not deduplicated at all, which is unreachable from the event path because the ledger key derivation rejects such an event first. **A matching key with a mismatched fingerprint is not a duplicate.** Two orphans that share a delivery id but declare different fingerprints are the same corruption the ledger path reports as `delivery_conflict` — one delivery carrying two different events — so the orphan path reports it the same way: no record, no revision, no key consumed, a diagnostic only. Suppressing it as a duplicate would leave that corruption indistinguishable from an ordinary resend, which is the visibility the delivery-first key would otherwise cost. As everywhere else in §4.3, the check does not fire when either side declares no fingerprint. The record keeps `eventId` as well, for audit: it names which delivery was the one recorded.

  **Only those two things are recorded.** The other quarantines — an identity or fingerprint conflict on a terminal, a delivery-id collision, a conflicting redelivered start — reject an event that was never in the live set, so nothing left it; recording them would file event-level corruption under a heading that means "this used to be in the state". Those paths remain diagnostics-only. Two boundaries follow from the array being bounded and from the state being authoritative. The duplicate check for an orphaned terminal reads the live array only, so once a record has been pushed out by `arrayItems` later drops, a further resend of that same terminal is recorded again and moves the revision; it converges within each window, and reaching the next one takes a full array's worth of drops on a single lineage. And an array that arrives over the limit — from an implementation that does not enforce it — is repaired, with the overflow reported, on the next event whose revision is built from the recording path. The limit is a property of the state rather than of the append, so the repair runs whenever that path produces a revision — including a start that evicts nothing and a terminal whose record is a duplicate, which is otherwise a no-op. Carrying an over-limit array forward would have the reducer emit, revision after revision, a state its own frozen schema rejects. The repair converges, though not always in one revision: one trim brings the array to the limit, and the next delivery is a true no-op unless the trim was what dropped that terminal's own record — in which case the next delivery records it again and the one after that is the no-op. That is the same bound §4.3 already states for a record pushed out by `arrayItems`, and reaching it takes a full array's worth of drops on a single lineage. The revisions that never reach the recording path at all — a terminal closing an operation, and abandonment — do carry an over-limit array forward; that residue is stated in the limitations rather than closed here, since closing it means moving the repair into the shared revision constructor, which nothing in FR-015 asks for.
- A matched late terminal event references the original `operationId` and `startEventId`, creates a later work-state revision, and changes only that operation to `succeeded` or `failed`.
- Missing or ambiguous terminal evidence establishes `unknown` at abandonment/recovery.
- Unknown renders as `result unknown; verify current state before retry`.
- Shell commands default to `verify_first`; migrations/deploys/publishing/destructive/external/credential operations default to `never_auto`.
- `safe_idempotent` requires an explicit idempotency contract and matching capability evidence.

**Operation-class table (#36).** The defaults above are stated in terms of operation classes
(shell, migration, destructive, external, credential), but the value an event carries is
`ContinuityOperationRefV1.operationKind`, an adapter-specific free string (`Bash`, `Edit`, and
whatever a future adapter emits). The mapping from `operationKind` to operation class is normative
and **belongs in this addendum**, not in the capability matrix: the matrix records behaviour proven
by actually running a CLI, whereas this mapping is a static judgement about what a tool name means,
and no capture can prove it.

The table is not populated yet. Until it is, implementations MUST use the fail-closed defaults for
every `operationKind`:

| Field | Default while unmapped |
|---|---|
| `PendingOperation.kind` | `tool` |
| `PendingOperation.replayPolicy` | `never_auto` |
| `PendingOperation.sensitivity` | `private` |

Populating the table is a prerequisite for §9.2 remote routing and MUST NOT be deferred past it.
It also carries a migration condition: `CanonicalWorkStateV1.sensitivity` is the maximum over its
components and is therefore monotone non-decreasing across revisions, so a lineage that has ever
held a pending operation is pinned at `private` or above. Introducing a real classifier MUST come
with a path that re-derives (or migrates) the aggregate, otherwise every lineage that ever used a
tool stays permanently outside the remote-send gate.

**Retention and eviction (#39).** `pendingOperations` is bounded by the §10 `arrayItems` limit.
When a new start arrives at a full array, implementations MUST evict rather than reject the start,
and MUST evict in this group order:

1. entries whose `correlation.taskLineageId` differs from the state's `taskLineageId`;
2. `succeeded`;
3. `failed`;
4. `unknown`;
5. `started`.

Within a group, the authoritative order is the **position of the entry in `pendingOperations`**:
the earliest surviving array index is evicted first, and the array index is the sole tie-breaker.
Implementations MUST NOT sort by `startedAt`. `startedAt` is copied from the adapter-supplied
`occurredAt`, so it is neither monotone with arrival nor outside an event submitter's influence; a
late event carries an early `startedAt` while occupying a late array slot. Two conforming runtimes
that disagree here evict different entries and therefore produce different states, `stateRevision`
chains, and content hashes from the same event sequence.

Out-of-lineage entries go first because they are outside the §4.3 correlation scope and the
abandonment scope, so nothing can ever reach them again; keeping them while discarding in-lineage
settled evidence lets a caller push out real evidence by injecting foreign entries. Rejecting the
start instead of evicting is **not** permitted: no other path removes an `unknown` entry, so a
lineage whose slots fill with open operations would refuse every subsequent start with no recovery
path, and a quarantined start has no corrected version for the adapter to send. Every eviction MUST
be reported as a diagnostic; silent truncation is not permitted. If no entry can be evicted, the
state is not produced and the event is quarantined.

Where the evicted evidence is persisted is settled above: the evicted entry is appended to
`droppedEvidence` in the state itself, bounded and identification-only (#43). Whether `unknown`
entries expire is still open — nothing in this section removes one, so a lineage can only reclaim
those slots through this eviction path (#44).

This policy decides **what** is evicted, not **who** may cause an eviction. Anyone able to submit
events can fill the array with cheap starts and drive eviction at will (#45). Bounding that is the
intake layer's job — the correlation reducer is a pure function of the state and one event, and has
no notion of who sent it — and it is not closed by this section. In particular, until intake binds
`sessionId` to the authenticated peer (#42), an event may claim a foreign session inside the same
Agent and lineage, which reaches this eviction path.

## 5. Immutable checkpoints and disposition history

```ts
interface ContinuationCheckpointV2 {
  id: string;
  schemaVersion: 2;
  checkpointRevision: string;
  projectId: string;
  workspaceId: string;
  branchKey?: string;
  taskLineageId: string;
  sourceSessionId: string;
  sourceAgent: string;
  kind: "pre_compact" | "session_end" | "idle" | "manual" | "crash_recovery";
  parentCheckpointId?: string;
  workStateRevision: string;
  canonicalState: CanonicalWorkStateV1;
  memoryWatermark: string;
  contentHash: string;
  sensitivity: Sensitivity;
  createdAt: string;
  expiresAt?: string;
}

type CheckpointDispositionKind =
  | "created"
  | "accepted"
  | "superseded"
  | "expired"
  | "reopened"
  | "retracted";

interface CheckpointDispositionEvent {
  eventId: string;
  checkpointId: string;
  kind: CheckpointDispositionKind;
  expectedProjectionRevision: string;
  resultingProjectionRevision: string;
  relatedCheckpointId?: string;
  relatedDeliveryAttemptId?: string;
  reasonCode: string;
  source: "daemon" | "runtime" | "user";
  createdAt: string;
}

interface CheckpointDispositionProjection {
  checkpointId: string;
  state: "open" | "accepted" | "superseded" | "expired" | "retracted";
  projectionRevision: string;
  latestEventId: string;
  activeDeliveryAttemptId?: string;
  activeClaimFence?: string;
  activeLeaseUntil?: string;
}

interface CheckpointMetadataV1 {
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  kind: ContinuationCheckpointV2["kind"];
  sourceSessionId: string;
}

interface DispositionAuthorityContextV1 {
  source: "daemon" | "runtime" | "user";
  userAuthorizedCrossLineage: boolean;
  sourceEventIds: string[];
}
```

Disposition projection/validation MUST receive the event list, a checkpoint metadata lookup, and authority context.

- Accepted/superseded/expired/retracted are excluded from automatic candidates.
- Reopen creates a new open projection revision.
- Daemon/runtime supersede is valid only when source and related checkpoints share `taskLineageId`.
- Cross-lineage supersede requires explicit user-authoritative context.
- Missing related-checkpoint metadata fails closed; it never assumes same lineage.

## 6. Claim, delivery, engagement, and acceptance

### 6.1 Delivery attempt and commands

```ts
type DeliveryAttemptState =
  | "claimed"
  | "delivered"
  | "engaged"
  | "accepted"
  | "dismissed"
  | "abandoned";

type EngagementEvidenceKind =
  | "explicit_accept"
  | "explicit_continue_prompt"
  | "related_file_action"
  | "related_command"
  | "related_test"
  | "related_todo_progress"
  | "manual_resume_tool";

interface CheckpointAnchorV1 {
  anchorId: string;
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  kind: "file" | "symbol" | "command" | "test" | "todo" | "task_lineage";
  valueHash: string;
  sourceEventIds: string[];
}

interface EngagementEvidence {
  kind: EngagementEvidenceKind;
  sourceEventIds: string[];
  score: number;
  checkpointAnchorIds: string[];
  successful: boolean;
  observedAt: string;
}

interface ContradictionEvidenceV1 {
  contradictionId: string;
  kind: "explicit_rejection" | "new_task_confirmed" | "workspace_incompatible" | "runtime_invalidated";
  sourceEventIds: string[];
  observedAt: string;
}

interface ContradictionScanRangeV1 {
  fromIngestSeq: string;
  toIngestSeq: string;
  scannedAt: string;
}

interface EngagementEvaluationContextV1 {
  sourceEvents: NormalizedContinuityEvent[];
  checkpointAnchors: CheckpointAnchorV1[];
  contradictions: ContradictionEvidenceV1[];
  contradictionScan: ContradictionScanRangeV1;
  destinationTurnId: string;
  destinationTurnIdSource: TurnIdSource;
  destinationAgentVersion: string;
  destinationCapabilityHash: string;
  turnIdentityDisposition: CapabilityTestDisposition;
  evaluationStartedAt: string;
  evaluationEndedAt: string;
}

interface CheckpointDeliveryAttempt {
  attemptId: string;
  checkpointId: string;
  checkpointRevision: string;
  destinationSessionId: string;
  destinationAgent: string;
  state: DeliveryAttemptState;
  claimFence: string;
  leaseUntil: string;
  heartbeatUntil: string;
  injectedContentHash?: string;
  injectionId?: string;
  engagementEvidence: EngagementEvidence[];
  createdAt: string;
  deliveredAt?: string;
  engagedAt?: string;
  acceptedAt?: string;
  dismissedAt?: string;
  abandonedAt?: string;
  revision: string;
}

type DeliveryCommandV1 =
  | { kind: "mark_delivered"; attemptId: string; revision: string; fence: string; sessionId: string; contentHash: string }
  | { kind: "record_engagement"; attemptId: string; revision: string; fence: string; sessionId: string; evidence: EngagementEvidence }
  | { kind: "accept"; attemptId: string; revision: string; fence: string; sessionId: string; projectionRevision: string }
  | { kind: "dismiss"; attemptId: string; revision: string; fence: string; sessionId: string }
  | { kind: "abandon"; attemptId: string; revision: string; fence: string; sessionId: string; reason: string }
  | { kind: "renew_lease"; attemptId: string; revision: string; fence: string; sessionId: string; requestedLeaseUntil: string };

interface ResumeSuppressionEntryV1 {
  checkpointId: string;
  sessionId: string;
  reason: "dismissed";
  attemptId: string;
  createdAt: string;
}
```

Every post-claim command validates the caller-supplied `attemptId`, attempt revision, fence, and destination session. A mismatched attempt ID is a typed stale/invalid request and causes no state change.

The attempt-local tuple is not sufficient on its own: after lease expiry the reclaimed checkpoint has a new active attempt while the old attempt's own `attemptId`/revision/fence/session remain internally consistent. Therefore every post-claim command additionally CASes against the active projection **inside the same daemon transaction**:

1. load `CheckpointDispositionProjection` by checkpoint ID;
2. require `projection.state === "open"`;
3. require `projection.activeDeliveryAttemptId === command.attemptId`;
4. require `projection.activeClaimFence === command.fence`;
5. require `projection.activeLeaseUntil` to be unexpired at transaction time;
6. only then apply the attempt-local transition.

Appending a **delivery-invalidating** terminal disposition (`superseded`, `expired`, `retracted`) is one transaction that also abandons the active attempt and clears `activeDeliveryAttemptId`, `activeClaimFence`, and `activeLeaseUntil`. Acceptance is the exception: it clears the same three projection fields but advances its own attempt to `accepted` rather than abandoning it (§6.4), because that attempt is the one being accepted.

Either way, a checkpoint that stops being `open` has no attempt that can still pass the CAS, so a delayed `mark_delivered` for a superseded, expired, retracted, or already-accepted checkpoint is `stale_attempt` instead of a late injection. This ordering is a required fixture: supersede/expire/retract between claim and delivery must prevent the delivery.

`dismiss` and `abandon` terminate the attempt **and** clear `activeDeliveryAttemptId`, `activeClaimFence`, and `activeLeaseUntil` in the same transaction, so a dismissed or abandoned attempt never keeps the checkpoint claimed until its lease would have expired.

The two differ in what else the transaction records. `dismiss` is an explicit rejection, so it also appends a `ResumeSuppressionEntryV1` per `(checkpointId, destinationSessionId)` to the resume ledger in the same transaction — the entry is part of the transition's output, not a side effect outside the frozen contract (v6.1 §11: dismiss returns the checkpoint to `open` **and** suppresses it for that session). A suppressed session is not an eligible destination for that checkpoint again, so clearing the claim cannot cause repeated injection into the session that just rejected it. `abandon` records no suppression and the same session may reclaim.

Both immediate-reclaim paths are required fixtures: after `abandon`, the same session reclaims without waiting for lease expiry; after `dismiss`, a **different** eligible session reclaims immediately while the dismissing session is rejected as ineligible.

Heartbeat renewal is the typed `renew_lease` command, not out-of-band prose: it passes the same attempt/revision/fence/session tuple, runs the same projection CAS, and extends the attempt's `heartbeatUntil`/`leaseUntil` together with the projection's `activeLeaseUntil` in one transaction, so lease state never diverges between attempt and projection. A renewal that arrives after reclaim fails the CAS and cannot extend a stale attempt; the heartbeat-versus-reclaim ordering is a required fixture alongside the delivery race. Lease expiry, reclaim, and replacement are a single transaction that terminates the old attempt (`abandoned`, revision bump) and rotates `activeDeliveryAttemptId`/`activeClaimFence` together. A delayed `mark_delivered`, `record_engagement`, `dismiss`, or `abandon` from a reclaimed attempt therefore fails step 2 or 3, is typed `stale_attempt`, and causes no state change and no delivery. The reclaim-versus-delivery race is a required fixture: a delayed command from the reclaimed attempt must never mark delivery, record engagement, or accept.

### 6.2 Initial claim CAS

The candidate-to-claimed operation is a daemon transaction that:

1. loads checkpoint projection by checkpoint ID + expected projection revision;
2. requires `open` and no unexpired active attempt;
3. validates destination session/task binding, delivery boundary, selected mode, capability, selection decision, and reconciliation;
4. creates attempt ID and fenced claim token;
5. inserts the claimed attempt;
6. sets active attempt/lease on the projection;
7. commits together.

Concurrent claims yield exactly one winner. Expiry/replacement uses CAS on projection revision and active attempt identity.

### 6.3 Deterministic engagement

Contract v1 weights:

| Evidence | Score | Requirement |
|---|---:|---|
| explicit_accept | 1.00 | explicit user or user-authoritative UI/CLI |
| manual_resume_tool | 1.00 | user-authoritative invocation |
| explicit_continue_prompt | 0.35 | prompt positively references task/checkpoint |
| related_file_action | 0.35 | successful event linked to file/symbol anchor |
| related_command | 0.40 | successful event linked to command/task anchor |
| related_test | 0.50 | successful/meaningful test linked to checkpoint work |
| related_todo_progress | 0.40 | deterministic todo transition linked to lineage |

- Duplicate `(kind, sourceEventId)` counts once.
- Failed/unknown/unrelated events score zero.
- Evidence labels are not trusted by themselves. The evaluator MUST verify each source event exists in `EngagementEvaluationContextV1`, has the expected kind/success state, occurs after delivery and before evaluation end, and links to a declared anchor.
- Anchors are immutable properties of the checkpoint, not caller input. They are derived and persisted when the checkpoint is written, each carrying `checkpointId`, `checkpointRevision`, and `taskLineageId`. The evaluator resolves anchors from the stored checkpoint inside the acceptance transaction and ignores any anchor the caller supplies that is not among them, so an unrelated anchor cannot be introduced to satisfy the threshold. Evidence linked to an anchor whose `checkpointId`/`checkpointRevision` does not match the attempt's checkpoint scores zero.
- Turn scoping is verified from canonical turn identity (§3.1), not from time window plus anchor match: a source event counts only when `event.sessionId` equals the destination session and `event.turnId` equals `destinationTurnId`. An event with `turnIdSource="unavailable"`, a missing `turnId`, or a different `turnId` scores zero.
- Destination turn provenance is validated **before** scoring, from the evaluation context: `destinationAgentVersion` and `destinationCapabilityHash` must match the active matrix, and `turnIdentityDisposition` must be `proven`. A caller-selected `destinationTurnId` alone never unlocks the automatic path.
- When the destination Agent/version has no proven turn identity (`turnIdentityDisposition != "proven"` or `destinationTurnIdSource="unavailable"`), automatic acceptance is unavailable for that version even if matching events exist; delivery downgrades to hint/manual and doctor reports the downgrade reason. Explicit user acceptance remains available.
- `engaged`: one valid linked item score `>=0.35`.
- Automatic `accepted`: cumulative score `>=0.80`, at least two evidence kinds, at least one successful runtime kind, no contradiction.
- Explicit user/manual acceptance may atomically perform delivered/engaged/accepted at score `1.00`.
- Window is bounded by active lease and 30 minutes.
- Explicit rejection, confirmed other task, incompatible reconciliation, or invalidating runtime evidence blocks automatic acceptance.
- Agent prose alone never constitutes explicit acceptance.

### 6.4 Atomic acceptance

Acceptance receives attempt, current disposition events/projection, checkpoint metadata, authority context, and verified engagement context. In one transaction it:

1. validates command attempt ID/revision/fence/session;
2. revalidates engagement from normalized source events and anchors;
3. **re-queries contradictions from the daemon's own event store inside this transaction**, with per-kind scope — `explicit_rejection` and `new_task_confirmed` are scanned **session-wide** (a confirmed other task belongs to the new lineage, or to none, so a lineage filter would miss it), `workspace_incompatible` is scanned per destination session and workspace, and only `runtime_invalidated` is filtered to the resumed checkpoint/lineage — from delivery through a cutoff the daemon reads for itself — the authoritative event-store watermark (highest applied `ingestSeq`) observed inside this transaction. The caller's `contradictions` array and `contradictionScan` range are advisory diagnostics only and never bound the scan, so a caller cannot exclude a late rejection by submitting an earlier `evaluationEndedAt`. If the watermark cannot be read, acceptance fails closed. Any contradiction found by the daemon blocks acceptance even when the caller omitted it;
4. verifies open checkpoint projection and active attempt identity;
5. appends accepted disposition linked to the attempt;
6. advances projection to accepted and clears active claim;
7. advances attempt to accepted;
8. commits all or none.

An accepted attempt with an open checkpoint projection is invalid.

## 7. Fail-closed workspace reconciliation

```ts
type ReconciliationStatus =
  | "exact"
  | "fast_forward_compatible"
  | "stale_but_usable"
  | "requires_verification"
  | "incompatible";
```

Required checks: repository/workspace identity, branch/worktree, HEAD ancestry, dirty fingerprint, relevant file existence/hash, test/config drift, and pending operations.

- `exact` requires every applicable check to be positively completed and matched.
- Missing hash, unreadable path, unknown ancestry, unknown branch/worktree relationship, contradiction, or unclassified drift is at least `requires_verification`.
- Repository/workspace mismatch is `incompatible`.
- Fast-forward with unchanged affected files is `fast_forward_compatible`.
- Classified low-risk drift is `stale_but_usable`.
- `requires_verification` permits only a verification capsule; `incompatible` prohibits automatic full resume.
- No unhandled input may fall through to exact.

Core 1.0 does not automatically restore workspace files. A future snapshot provider requires separate ADR/security/storage/UX evidence.

## 8. Exact-version capability strategy

```ts
type ResumeDeliveryStrategy =
  | "native_prompt_gate"
  | "session_start_full"
  | "next_prompt_synthesized"
  | "manual_only";
```

- `native_prompt_gate`: pre-model first-prompt delivery is native and real-CLI proven.
- `next_prompt_synthesized`: both pre-model delivery and prompt-aware injection are synthesized by the same exact-version real-CLI fixture/evidence hash.
- `session_start_full`: SessionStart injection is native/synthesized and proven, but prompt-gated proof is absent.
- `manual_only`: no reliable automatic path.

A half-proven synthesized pair is invalid. Source declarations/README claims are insufficient.

The capability hash is a schema-versioned SHA-256 over the exact Agent version, the capability-scenario manifest hash (§13), and the recorded scenario dispositions/evidence hashes for that version. It is the value events cite as `provenance.capabilityHash` (§3.1) and decisions cite as authority context; a hash that does not match the active matrix makes the event non-native for every authority rule.

Tier A requires exact-version proof of hint delivery, claimed prompt-gate delivery, compact persistence/fallback, exactly-one compact restore, retry dedupe, crash/restart semantics, and size/malformed behavior.

At publication, Claude and Codex SessionStart injection are proven; prompt-aware/compact paths remain unproven and must downgrade.

## 9. Resume modes and delivery boundaries

```ts
type ResumeMode = "smart" | "always" | "hint_only" | "compact_only" | "off";
type ResumeDeliveryBoundary = "session_start" | "first_user_prompt" | "post_compact" | "manual";
```

Mode and exact capability are intersected at a specific boundary:

| Mode | session_start | first_user_prompt | post_compact | manual |
|---|---|---|---|---|
| smart | proven hint only | full only with proven prompt gate + selection/reconciliation | full only with proven compact single-delivery | allowed |
| always | full only with proven SessionStart path + selection/reconciliation | no duplicate automatic full if already delivered; otherwise proven prompt fallback + selection/reconciliation only | full only with proven compact single-delivery + selection/reconciliation | allowed |
| hint_only | proven hint only | no automatic full | no automatic full | allowed |
| compact_only | none | none | full only with proven compact single-delivery + selection/reconciliation | allowed |
| off | none | none | none | allowed |

`+ selection/reconciliation` is shorthand for the §11 full-action predicate: one high-confidence candidate outside the ambiguity margin and a reconciliation status that permits automatic full delivery. Capability proof alone never authorizes a full action in any mode.

Boundary is explicit in selection input/output and fixtures. Checkpoint kind or prompt presence is not used to guess the delivery boundary.

## 10. Bounded, sensitivity-aware capsule lifecycle

Shared limits:

```text
hint tokens 120; full capsule tokens 700; prompt-memory tokens 700;
combined automatic tokens 1500; absolute tokens 1800;
payload bytes 32768; wrapper bytes 36864; JSON depth 12;
string UTF-8 bytes 8192; array items 256; object keys 128; candidates 5.
```

```ts
interface ResumeCapsuleV1 {
  schemaVersion: 1;
  injectionId: string;
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  sourceAgent: string;
  ageSeconds: number;
  reconciliation: ReconciliationStatus;
  workState: CanonicalWorkStateV1;
  selectedMemoryIds: string[];
  warnings: string[];
}
```

Sensitivity is filtered before canonical serialization:

- `normal`: eligible by scope/relevance/budget.
- `private`: excluded unless explicit project opt-in and destination `privateEligible=true`; otherwise metadata-only omission/warning.
- `secret`: never automatic; value removed, non-sensitive omission provenance retained.
- Derived notes inherit maximum source sensitivity.
- Hash/bytes are calculated after selection, omission, and redaction.

Rendering stable-sorts JSON, enforces limits, escapes `<`, `>`, `&`, includes schema/bytes/hash/injection ID, and never concatenates raw historical text outside the encoder.

Capture parses and verifies wrapper boundary, schema, bytes, hash, injection ID, and owned ledger:

- valid owned capsule: strip fully; retain metadata only;
- unknown ID, bad hash/size/schema, malformed/nested wrapper: do not trust or auto-promote; retain as protected evidence and emit diagnostic;
- parser failure never blocks the coding Agent;
- round-trip self-ingestion prevention is blocking.

Invalid/oversized fallback obeys mode and boundary: `off` => empty; `compact_only` post-compact => empty+diagnostic; `hint_only` => valid hint only; `smart/always` => valid proven hint only, otherwise empty. Invalid full capsules are never claimed/delivered.

## 11. Dataset-versioned resume selection

```ts
interface ResumeThresholdProfileV1 {
  profileId: string;
  datasetVersion: string;
  fullResumeMinScore: number;
  hintMinScore: number;
  ambiguityMargin: number;
  maxCandidates: number;
}

type ResumeDecisionAction =
  | "none"
  | "hint"
  | "candidate_list"
  | "verification_capsule"
  | "full_capsule"
  | "manual_only";

interface RankedResumeCandidateV1 {
  rank: number;
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  score: number;
  reconciliationStatus: ReconciliationStatus;
  reasonCodes: string[];
  ageSeconds: number;
}

interface ResumeSelectionDecisionV1 {
  schemaVersion: 1;
  boundary: ResumeDeliveryBoundary;
  datasetVersion: string;
  thresholdProfileId: string;
  capabilityHash: string;
  strategy: ResumeDeliveryStrategy;
  mode: ResumeMode;
  action: ResumeDecisionAction;
  selectedCheckpointId?: string;
  rankedCandidates: RankedResumeCandidateV1[];
  topScore?: number;
  topMargin?: number;
  confidenceBand: "high" | "medium" | "low" | "none";
  decisionReasonCodes: string[];
  fallbackReasonCode?: string;
  reconciliationReportHash?: string;
}
```

Initial preflight profile: full `0.75`, hint `0.35`, ambiguity margin `0.08`, max candidates `5`.

- Top below hint => none.
- Between hint/full => hint or candidate list.
- Top above full but second within margin => candidate list, never guess.
- Low/unknown confidence, contradictory reasons, unproven capability, mode mismatch, or incompatible workspace => no full.
- `requires_verification` => verification capsule at most.
- Full requires exactly one high-confidence candidate, outside ambiguity margin, at an allowed boundary/mode/capability/reconciliation state.
- Decision/reasons are persisted. Threshold changes require dataset bump and reviewed before/after report.

## 12. Durable-memory history, temporal validity, and derived-artifact invalidation

### 12.1 Memory history

- Semantic notes and consolidated memories retain source event/fact IDs.
- Raw evidence is not deleted during derivation.
- Memory changes are append-only ADD/UPDATE/SUPERSEDE/RETRACT events.
- Temporal validity/invalidation is retained when supported by evidence.
- Stale/contradicted facts remain inspectable but are excluded/down-ranked from current retrieval.
- Output may prefer consolidated observations while preserving source records.
- Presentation dedupe never deletes source evidence.

### 12.2 Derived-artifact dependencies

```ts
type DerivedArtifactKind =
  | "summary"
  | "semantic_resume_note"
  | "checkpoint_semantic_note"
  | "consolidated_memory"
  | "embedding_item"
  | "context_pack_cache"
  | "cloud_projection";

type DerivedArtifactStatus = "active" | "stale" | "invalidated" | "rebuilding";

type DerivedArtifactSourceRefV1 =
  | {
      kind: "memory";
      memoryId: string;
      memoryRevision: string;
      contentHash: string;
    }
  | {
      kind: "artifact";
      artifactId: string;
      artifactKind: DerivedArtifactKind;
      artifactRevision: string;
      contentHash: string;
    };

interface DerivedArtifactDependencyV1 {
  artifactId: string;
  artifactKind: DerivedArtifactKind;
  artifactRevision: string;
  sources: DerivedArtifactSourceRefV1[];
  baseMemoryClosure: Array<{ memoryId: string; memoryRevision: string; contentHash: string }>;
  sourceEventIds: string[];
  generationId?: string;
}

interface DerivedArtifactInvalidationEventV1 {
  eventId: string;
  artifactId: string;
  artifactKind: DerivedArtifactKind;
  expectedArtifactRevision: string;
  sourceMemoryId: string;
  invalidatingMemoryRevision: string;
  viaArtifactId?: string;
  hopDepth: number;
  reason:
    | "memory_updated"
    | "memory_superseded"
    | "memory_retracted"
    | "memory_invalidated"
    | "source_artifact_invalidated";
  resultingStatus: "stale" | "invalidated";
  idempotencyKey: string;
  createdAt: string;
}
```

### 12.3 Causal invalidation rules

- Every derived artifact records exact source revisions/content hashes and source evidence IDs. `sources` holds the artifact's direct edges, which may be memories **or** other derived artifacts (for example a context-pack cache derived from a consolidated memory). `baseMemoryClosure` holds the transitive set of base memory revisions reachable through those edges and is recomputed on every artifact revision.
- `baseMemoryClosure` is **materialized at write time**, not walked at read time: it is computed once when an artifact revision is written or rebuilt and stored on the dependency row. Eligibility therefore compares a stored list against current memory projections and performs no graph traversal, so query cost is linear in the artifact's own closure size and independent of graph depth.
- Derived-artifact dependency graphs are acyclic. A dependency write that would create a cycle, or a closure that cannot be resolved because an intermediate dependency is missing, is quarantined and the artifact is excluded rather than treated as current.
- In the same daemon transaction that commits an UPDATE/SUPERSEDE/RETRACT or temporal invalidation, **every** dependent artifact — direct dependents and all transitive descendants reached through artifact-to-artifact edges — is marked `stale` or `invalidated` and deterministic rebuild jobs are enqueued. Descendant invalidation events record `viaArtifactId` and `hopDepth` and use reason `source_artifact_invalidated`. The invalidation idempotency key is derived from `(memoryId, invalidatingRevision, artifactId, reason)`.
- Query, injection, resume rendering, embedding activation, cloud projection, and cache lookup MUST verify that every direct `sources` entry **and** every `baseMemoryClosure` entry still matches current projections. Matching an unchanged intermediate artifact revision is not sufficient, so a stale/invalidation job delay or failure cannot make a descendant artifact eligible.
- A multi-hop fixture (memory → consolidated memory → context-pack cache/embedding) is required: after mutating the base memory, every descendant must be excluded by the eligibility check even before its invalidation job runs.
- Immutable historical checkpoints are not rewritten. At resume time, affected selected-memory content or semantic notes are omitted/marked stale unless a compatible re-derived artifact exists. Canonical observed checkpoint state remains historical evidence.
- Embedding eligibility is keyed by `(memoryId, memoryRevision, inputHash, generationId)`; a prior revision vector cannot satisfy current coverage.
- Summary/consolidated-memory/context-pack artifacts remain excluded until rebuild succeeds. Rebuild creates a new artifact revision and dependency set; it never overwrites historical provenance.
- Crash/replay of memory mutation and invalidation converges exactly once. A rebuild failure leaves canonical continuation and FTS fallback available while the derived artifact stays excluded.
- Sync/cloud projections receive the new revision/tombstone as an independent immutable operation; sync is not used to infer local invalidation success.

## 13. Preflight and automatic-support states

```ts
type CapabilityTestDisposition = "not_run" | "proven" | "unsupported" | "unknown_after_test";
type ContractPreflightState = "incomplete" | "complete";

interface RequiredCapabilityScenarioV1 {
  scenarioId: string;
  title: string;
  appliesToAgents: string[];
  requiredFor: Array<"generic_phase3" | "automatic_strategy" | "tier_a">;
}

interface CapabilityScenarioManifestV1 {
  manifestVersion: string;
  manifestHash: string;
  scenarios: RequiredCapabilityScenarioV1[];
}
```

`harness/schema/capability-scenarios.v1.json` is the versioned, closed manifest of required scenario IDs and is the only authority for completeness. Prose checklists are not inputs.

Contract preflight is complete only when the manifest check passes, evidence artifacts exist, matrices are regenerated honestly, all runtime-neutral contract fixtures pass, and #1 Stage 0 fixes runtime direction.

The manifest check is exact-set equality, evaluated per `(agent, exactVersion)` tuple: the generated report/matrix MUST contain a disposition for **exactly** the manifest's applicable scenario IDs. A missing ID, an unknown extra ID, a duplicate ID, or a `manifestHash` mismatch fails preflight; an omitted scenario can therefore never pass vacuously. Adding or removing a required scenario requires a manifest version bump recorded in the evidence file.

Unsupported/unknown-after-test does not block generic/manual continuity implementation; it forces strategy/Tier downgrade. A particular automatic strategy is enabled only when its required exact-version capability is `proven`. Tier A/Core 1.0 also require release E2E and #8 quality.

## 14. Normative quality and doctor reports

`benchmarks/behavioral/contract.schema.json` is the sole machine-readable authority for `ResumeQualityReportV1`.

Zero-tolerance numeric counters include duplicate injection, wrong scope, incompatible auto-resume, unsafe unknown replay, early acceptance, accepted-attempt/open-checkpoint, stale fence, capsule boundary escape, malformed capsule trusted, source evidence deletion, and stale derived artifact use. All must be zero; deterministic critical scenarios must be 100% pass.

Behavioral metrics include wrong resume, unnecessary hint, candidate accuracy, critical-state recall, fabricated/stale state, re-explanation turns/tokens, first useful action, task completion, hint/full tokens, and claude-mem baseline delta. `unsupported` is allowed only where declared inapplicable; a required metric marked unsupported fails the gate.

`doctor continuity --json` is versioned and reports exact version/capability hash, scenario dispositions, strategy, mode, threshold/dataset, last boundary/selection reasons, reconciliation, active attempt/lease summary, unknown pending count, stale/invalidated derived-artifact counts, preflight/unmet gate IDs, and schema/fixture/report hashes. It never emits raw prompts, commands, private/secret values, or capsule content.

Major public resume scenarios MUST meet #8's frozen claude-mem non-inferiority gate or receive an explicit reviewed exception ADR before Core 1.0.

## 15. Non-goals

- Reproducing private claude-mem/CMEM prompts/services.
- Treating model output as canonical truth.
- Automatically restoring workspace files in Core 1.0.
- Requiring shadow Git.
- Letting heuristic boundaries delete/supersede work.
- Claiming Tier A from source inspection.
- Automatically replaying unknown external side effects.
- Weakening Phase 1 safety/security/backup gates.

## 16. Exit criteria

Implemented when machine-readable schemas/fixtures exist; TS/Rust consume identical fixtures; exact Claude/Codex dispositions are recorded; task state/idempotency, terminal correlation, boundary authority/confirm/reject, pending operations, lineage-aware dispositions, initial claim, source-verified engagement, atomic acceptance, fail-closed reconciliation, explicit delivery boundaries, selection, sensitivity, capsule render/capture, memory history, derived-artifact invalidation, quality report, and doctor all pass; and Phase 3/Core 1.0 gates enforce this addendum.
