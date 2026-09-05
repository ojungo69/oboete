# Feature Specification: Lightweight Automatic Memory Product Reset

**Feature Branch**: `feat/product-reset-alpha`

**Created**: 2026-08-25

**Status**: Accepted

**Input**: User description: "Rebuild free-mem as a reliable, resource-bounded
claude-mem-like product for automatic memory between Claude Code and Codex. Keep the
good automatic experience, simplify configuration, allow independent summary and
embedding model choices, and defer broad continuity and cloud work until the local
product is useful."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Resume Work Across Claude and Codex (Priority: P1)

As a developer who alternates between Claude Code and Codex in one repository, I can
continue a task in either Agent without writing a handoff document or restating the
important decisions, changes, failed attempts, and next actions from the prior session.

**Why this priority**: Automatic cross-Agent memory is the reason the product exists.
Without this complete loop, configuration, search, and diagnostics do not create a
useful product.

**Independent Test**: Complete one fixed task in Claude Code, start Codex in the same
repository, verify that the required facts are supplied automatically, then repeat in
the opposite direction.

**Acceptance Scenarios**:

1. **Given** a configured Claude Code session with a completed decision and an unfinished
   next action, **When** Codex starts in the same repository, **Then** it receives the
   required decision and next action without a manual handoff.
2. **Given** a configured Codex session with a discovered constraint and a failed
   approach, **When** Claude Code starts in the same repository, **Then** it receives
   both facts and does not present the failed approach as a recommended next step.
3. **Given** a task whose relevant facts fit within the configured context budget,
   **When** either Agent requests relevant context, **Then** all required facts and no
   forbidden or unrelated facts appear in the returned context.

---

### User Story 2 - Keep Working During Memory Failures (Priority: P1)

As a developer, I can keep using Claude Code or Codex when the memory runtime, summary
provider, or semantic index is unavailable. Accepted activity is recovered later without
loss or duplicate memories, and degraded retrieval is clearly identified.

**Why this priority**: A background memory tool that blocks the coding Agent or silently
loses work is worse than having no memory tool.

**Independent Test**: Interrupt each memory dependency during capture and retrieval,
continue the coding session, restore the dependency, and verify recovery, deduplication,
fallback results, and visible health state.

**Acceptance Scenarios**:

1. **Given** the memory runtime is unavailable, **When** an Agent emits supported activity,
   **Then** the Agent continues and the activity is durably queued for later processing.
2. **Given** queued activity was delivered more than once during recovery, **When** it is
   processed, **Then** only one durable memory result is created.
3. **Given** semantic retrieval is unavailable, **When** a user asks for relevant context,
   **Then** lexical retrieval remains available and the result states why semantic
   retrieval is degraded.
4. **Given** a provider repeatedly returns invalid or unavailable responses, **When** the
   retry budget is exhausted, **Then** the processing job remains inspectable in
   `retry-exhausted`, timer-only retry stops, and processing resumes only after validated
   configuration activation, a
   daemon-recorded healthy provider transition, or user-confirmed doctor retry.

---

### User Story 3 - Configure Models Without Configuration Sprawl (Priority: P1)

As a user, I can choose a resource profile and independently choose how summaries and
embeddings are produced. Before activation I can see local versus remote execution,
external destination, credential source, and expected cost behavior.

**Why this priority**: The product specifically exists to improve the complex and coupled
model configuration found in existing automatic memory tools.

**Independent Test**: Starting from a clean installation, select each supported profile,
validate its providers, inspect the effective configuration, switch one provider without
changing the other, and confirm that invalid changes leave the prior configuration active.

**Acceptance Scenarios**:

1. **Given** a clean installation, **When** the user selects a resource profile and
   summary and embedding providers, **Then** setup validates the choices and shows the
   effective privacy, endpoint, credential, and cost behavior before activation.
2. **Given** a working summary provider, **When** the user changes only the embedding
   provider, **Then** summary behavior is unchanged and existing lexical retrieval remains
   available while semantic data is prepared.
3. **Given** a proposed provider configuration is invalid, **When** validation fails,
   **Then** the prior effective configuration remains active and no secret value is shown.
4. **Given** a non-expert user chooses a preset, **When** setup finishes, **Then** no manual
   editing of a configuration file is required.

---

### User Story 4 - Understand and Control Stored Memory (Priority: P2)

As a user, I can inspect what was remembered, why an item was selected for injection,
whether any component is degraded, and delete memories I no longer want retained.

**Why this priority**: Automatic memory must remain understandable and reversible to earn
trust, but the first cross-Agent loop can be demonstrated before a polished inspection UI.

**Independent Test**: Capture a known set of memories, inspect their sources and injection
reasons, delete one item, and verify it no longer appears in search or injection.

**Acceptance Scenarios**:

1. **Given** a generated memory and a later context injection, **When** the user inspects
   them, **Then** the source session, memory state, selection reason, and degradation state
   are visible without exposing secrets.
2. **Given** a user deletes a memory, **When** subsequent search and injection run, **Then**
   the deleted memory is absent from both.
3. **Given** pending or failed memory work exists, **When** the user runs diagnostics,
   **Then** the affected stage, reason, safe next action, and effective profile are shown.

---

### User Story 5 - Install a Bounded Linux/WSL Alpha (Priority: P2)

As an early adopter on Linux or WSL, I can install, configure, verify, update, back up,
restore, and remove the Technical Alpha through one documented product path whose resource
use remains stable during long sessions.

**Why this priority**: External users cannot validate the product until a repeatable artifact
and lifecycle exist; other operating systems are intentionally deferred to avoid blocking
the first useful release.

**Independent Test**: Use a clean supported environment to perform the full lifecycle, run
the fixed bidirectional scenario and resource soak, restore a backup, and remove the product
without leaving active processes or managed configuration behind.

**Acceptance Scenarios**:

1. **Given** a clean supported environment, **When** a user follows the documented setup,
   **Then** both Agents pass diagnostics and the first automatic cross-Agent recall works.
2. **Given** a fixed long-running workload, **When** the resource soak completes, **Then**
   memory and storage use reach the declared profile envelope without continuing unbounded
   growth or deleting durable memories.
3. **Given** a valid backup, **When** the user restores it after local data loss, **Then**
   the expected memories and indexes become usable without duplicate durable items.
4. **Given** an installed Alpha, **When** the user uninstalls it, **Then** managed integration
   entries and active processes are removed while user data is retained or deleted according
   to the explicit choice presented during uninstall.

### Edge Cases

- An Agent switches before the prior session's summary work has completed.
- The same event is retried after an uncertain delivery result.
- The same scoped event identity and payload digest is retried with stronger trusted sensitivity or
  quarantine state. The canonical row and every already-derived record atomically retain the
  strongest sensitivity; quarantine is absorbing and returns a non-success receipt before any
  further selection or normal ACK.
- The same scoped event identity is retried with a different deterministically normalized,
  post-redaction payload digest; a durable payload-free conflict record captures the event identity
  and both digests in the same atomic transaction that makes the incoming delivery terminally
  quarantined. Only after that commit does the caller receive a non-success conflict receipt; a
  normal success ACK or silent discard is forbidden. The first accepted event's payload remains
  immutable and no additional durable result is created. The receipt is unique to the canonical
  identity and ordered digest pair: retrying the same conflict returns it, while a different
  conflicting digest receives a different receipt. Correction requires the canonical digest or a
  new event identity.
- A queued or replayed event crosses a redaction/digest algorithm upgrade. The event retains the
  immutable normalization version recorded at first acceptance, and comparison uses that version;
  the upgrade alone MUST NOT fabricate an identity conflict.
- A summary succeeds while embedding fails, or embedding succeeds after the active model changes.
- An embedding model changes dimension or identity while an older semantic index is active.
- A remote provider redirects, changes capabilities, rate-limits, or returns malformed output.
- A configuration file changes during validation or activation.
- A context request contains only short Japanese text or mixed Japanese and English terms.
- The memory corpus is large enough that only part of it fits within the context budget.
- A captured item contains a secret, private marker, or data scoped to another repository.
- The local data directory is a symbolic link, unsupported mount, or non-local filesystem.
- Multiple Agent processes emit activity near the same time for the same repository.
- The runtime terminates during queue processing, index activation, backup, or restore.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The product MUST support the Technical Alpha on Linux and WSL using a local
  Linux filesystem and MUST report unsupported platforms before modifying user configuration.
- **FR-002**: The product MUST configure both Claude Code and Codex through one setup flow
  without requiring a user-authored handoff file.
- **FR-003**: The product MUST automatically capture supported activity from both Agents and
  preserve the originating Agent, repository, session, ordering, and event identity.
- **FR-004**: Captured activity accepted by the product MUST survive runtime and provider
  interruption and MUST NOT produce duplicate durable memories after retry or recovery. A scoped
  event identity replayed with the same payload digest MUST atomically join trusted sensitivity and
  capture state before acknowledgement: sensitivity can only strengthen, quarantine is absorbing,
  and every existing record derived from that event must be strengthened in the same transaction.
  A quarantine escalation returns a non-success receipt and cannot leave selectable content. A scoped
  event identity with a conflicting post-redaction digest MUST persist a payload-free quarantined
  conflict record atomically before returning an explicit non-success conflict receipt. The
  conflicting delivery is terminally quarantined and MUST NOT receive a normal success ACK, be
  silently discarded, overwrite or duplicate the first accepted result, or retain unprocessed
  secret data. Its receipt MUST be deterministic for the canonical identity and ordered digest pair;
  repeated delivery of that pair reuses one durable receipt; a different conflicting digest receives
  a distinct durable receipt and MUST NOT reuse the existing receipt.
  The accepted event and every spool/retry record MUST persist the digest-normalization version;
  replay uses the original version even after a newer algorithm activates.
- **FR-005**: Capture and memory-processing failures MUST NOT block normal Agent work.
- **FR-006**: The product MUST asynchronously produce concise session summaries and a bounded
  set of durable decisions, discoveries, changes, failed approaches, and next actions from supported
  content rather than assuming transport event kind determines semantic memory kind. One source
  event or aggregate source set MAY yield zero to many distinct MemoryItems. Each output MUST retain
  semantic kind and provenance, receive its own stable logical lineage and deduplication key, and be
  bounded as an output item; retries MUST neither merge sibling facts nor drop one because another
  output shares its source events. Each ResourceProfile MUST publish an exact
  `maxMemoryItemsPerDerivation`. A provider result above that limit enters recoverable
  `retry-exhausted` atomically with `memory_output_limit_exceeded` and zero budget: no partial
  derived batch is committed, all committed source events and previously valid sibling memories
  remain unchanged, and only a changed validated limit/provider may retry the retained work. The
  failure record is payload-free and contains only the error code, job identity, source event IDs,
  observed result count, and active limit; it MUST NOT retain
  raw provider output, copied source text, or any uncommitted derived item.
- **FR-007**: Low-signal activity MUST be excluded without discarding required decisions,
  corrections, failures, or next actions.
- **FR-008**: Users MUST be able to retrieve memory by lexical relevance and, when enabled and
  healthy, semantic relevance, with deterministic deduplication and bounded result size.
- **FR-009**: The product MUST automatically provide a bounded context pack to the receiving
  Agent and MUST record why each included item was selected.
- **FR-010**: Secrets MUST never be persisted, transmitted to a summary or embedding provider,
  rendered off-host, or injected; every remote call MUST remove them or be blocked before sending.
  Private items and local-only items MAY be summarized and injected only by on-device consumers in
  the same repository scope and MUST never reach a remote provider or off-host renderer, even after
  private spans are removed. Deleted and incompatible-scope memories MUST never be selected.
- **FR-011**: Setup MUST offer a small set of resource profiles and independent summary and
  embedding provider choices.
- **FR-012**: Before activation, setup MUST show whether each provider is local or remote,
  the effective endpoint host, credential source, expected cost class, and data-egress policy.
- **FR-013**: Provider configuration MUST be validated before activation; a failed change MUST
  leave the prior effective configuration unchanged.
- **FR-014**: Runtime behavior and diagnostics MUST derive from the same versioned effective
  configuration and MUST identify any ignored, translated, or conflicting legacy setting.
- **FR-015**: When semantic retrieval is unavailable or incomplete, the product MUST continue
  lexical retrieval and MUST expose the degraded reason instead of reporting a healthy empty result.
- **FR-016**: Retry, queue, concurrency, injection, and resource limits MUST be bounded and
  observable for every shipped profile.
- **FR-017**: Users MUST be able to inspect memory content, origin, processing state, selection
  reason, effective profile, and safe recovery action without exposing credential values.
- **FR-018**: Users MUST be able to delete a memory and prevent its later retrieval, injection, or
  resurrection by reprocessing retained sources under a different profile, model generation, or
  semantic kind classification.
- **FR-019**: The product MUST support verified backup and restore of durable local memory.
- **FR-020**: Installation, update, diagnostics, backup, restore, and uninstall MUST form one
  documented lifecycle and MUST not leave orphan managed processes after completion.
- **FR-021**: The active README, specification index, and public work tracking MUST describe
  this Product Reset as the current product authority and mark prior continuity-platform work
  as historical or deferred.
- **FR-022**: The Technical Alpha MUST NOT require cloud sync, a hosted account, an additional
  Agent, team features, or a manual memory viewer action for the primary automatic flow.
- **FR-023**: The Technical Alpha MUST preserve semantic-memory capability when enabled; it
  MUST NOT delete durable memory or silently disable semantic retrieval to satisfy a resource target.

### Key Entities

- **Agent Session**: A Claude Code or Codex work period associated with an Agent, repository,
  session identity, timing, and processing state.
- **Captured Event**: An ordered, idempotent record of relevant Agent activity and its scope,
  sensitivity, delivery state, and source evidence.
- **Memory Item**: A durable summary, decision, discovery, change, failed approach, or next action
  with provenance, lifecycle state, search representation, and deletion state. Its logical lineage
  identity is a versioned, domain-separated collision-resistant digest of repository scope and a
  canonical source-fact anchor. The anchor is only the minimal supporting evidence set of sorted
  unique event identities and exact source byte spans—not fact wording, sibling-set membership,
  processing-batch membership, or model output order. Sibling facts MUST cite distinct minimal
  source spans; a provider result that assigns one anchor to multiple facts or cannot provide a
  distinct anchor is quarantined instead of inventing an ordinal. Thus paraphrases and overlapping
  aggregate batches retain one lineage while separately anchored siblings remain distinct. Profile,
  model generation, and semantic kind are revision attributes, not lineage inputs; deletion
  suppression uses the stable lineage identity.
  The anchor and lineage are computed before semantic-kind classification; later classification or
  reclassification can create a revision but cannot select a different lineage for the same spans.
  Reprocessing resolves proposed spans against the persisted anchor registry before hashing: exact
  spans reuse the anchor, overlap/containment with a deleted anchor remains suppressed, ambiguous
  overlap is quarantined, and only disjoint spans may create a new sibling automatically.
- **Resource Profile**: A user-facing operating envelope that defines limits and default
  behavior without coupling summary and embedding provider choices.
- **Provider Choice**: The independently selected summary or embedding execution method,
  including model identity, location, endpoint, credential source, cost class, and health.
- **Effective Capability Manifest**: The validated, versioned result of the selected profile,
  provider choices, and supported environment that both runtime and diagnostics report.
- **Injection Pack**: A bounded ordered set of memory items plus selection, omission,
  provenance, and degradation metadata supplied to an Agent.
- **Runner Evidence Bundle**: A candidate-inaccessible immutable comparison artifact that binds the
  complete result observation plus raw latency, resource, host-identity, and fresh cold/warm
  preparation observations to fixture/candidate/environment/invocation identity.
- **Operational Status**: The health and pending-work state for capture, summary, lexical
  retrieval, semantic retrieval, injection, backup, and configured providers.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The fixed Claude-to-Codex and Codex-to-Claude scenarios each complete with all
  required facts, zero forbidden facts, no failed approach rendered as a recommended next action,
  and zero manual handoff steps.
- **SC-002**: Across the required failure matrix, Agent blockage count, accepted-event loss,
  and duplicate durable-memory count are all zero.
- **SC-003**: Capture adds less than 200 ms at the 95th percentile under the reference workload.
- **SC-004**: Warm context retrieval and injection complete in under one second at the 95th
  percentile; a cold local semantic request completes in under three seconds or reports a
  truthful degraded fallback within that time.
- **SC-005**: After warm-up, the balanced-profile reference workload grows resident memory by
  no more than 32 MB over an eight-hour soak and leaves no orphan product process.
- **SC-006**: Reprocessing an unchanged event set under the same `profileId`/profile version and
  `modelId`/model revision leaves active durable-memory and active semantic-entry counts unchanged.
  Historical revision rows are excluded from those active counts and may increase after an allowed
  profile/model change. When semantic indexing is enabled and healthy, validation requires exactly
  one ready semantic entry in the active compatible generation per active durable-memory item. If it
  is enabled but semantic retrieval is incomplete, degraded, stale, or unavailable, validation
  instead requires lexical fallback and the matching degradation reason. When semantic indexing is
  disabled, validation requires no active semantic entry and an unchanged semantic-entry count.
- **SC-007**: Every injected item in the fixed scenarios has a visible source and selection
  reason, and every induced degraded state has the expected reason and safe next action.
- **SC-008**: All secret and cross-scope fixtures produce zero external disclosures and zero
  incompatible-scope injection results.
- **SC-009**: Five external Alpha users complete clean installation, profile selection, first
  bidirectional recall, diagnostics, and uninstall without manually editing configuration files.
- **SC-010**: Clean setup requires no more than three non-credential choices before the
  effective privacy, cost, and resource behavior can be validated and activated.
- **SC-011**: Backup and restore recover all expected durable memories with zero duplicate items
  in the required recovery scenario.
- **SC-012**: The Technical Alpha release exposes no active Cloud, team, additional-Agent,
  continuity-checkpoint, or broad platform promise in its primary product path.

## Assumptions

- Technical Alpha users run Linux or WSL with data stored on a local Linux filesystem.
- One local user may alternate between Claude Code and Codex; team and multi-user access are
  outside the Alpha scope.
- Users explicitly select local or remote providers and supply supported credentials; the
  product does not reuse unrelated Agent subscription credentials automatically.
- Local storage and automatic memory remain fully usable without Cloud or a hosted account.
- Semantic retrieval is an Alpha capability but broad superiority claims require later,
  separately frozen comparison evidence.
- macOS is the first post-Alpha platform milestone; native Windows follows separately.
- Rust, additional Agents, encrypted Cloud sync, hosted viewer, and team features remain
  deferred until external Alpha evidence or a measured blocker justifies them.
- Existing safe capture, single-writer, redaction, backup, and retrieval behavior is preserved
  unless this specification explicitly changes its user-visible contract.
- This specification is the umbrella Technical Alpha contract. The current Product Reset change
  establishes product authority, repository entry points, issue routing, and the fixed comparison
  contract; each runtime slice is implemented through a separate focused specification and pull
  request rather than one repository-wide rewrite.
