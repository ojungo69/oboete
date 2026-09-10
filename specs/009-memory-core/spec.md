# Feature Specification: Reliable memory and work continuity

**Feature Branch**: `009-memory-core`

**Created**: 2026-09-09

**Status**: Approved product direction; implementation in progress

**Input**: The owner approved a revised Oboete completion target after reviewing the current
implementation, measured failures, and claude-mem/CMEM Pro behavior: automatic work continuity
and useful long-term memory across Claude Code, Codex, Grok Build and Pi, multiple computers,
and Linux/WSL plus macOS in the first completed version. Existing implementation may be replaced
where it conflicts with that outcome. Shared understanding was explicitly confirmed on 2026-09-09.

## Clarifications

### Session 2026-09-09

- Both work continuation and long-term knowledge reuse are required.
- Worktrees separate active work; clear separate purposes within one conversation can form
  separate work items, while investigation and detours serving one purpose stay together.
- Continuation is automatic when unambiguous; otherwise the user makes one brief selection,
  without writing a handoff document.
- Project-specific information stays within its project; explicit personal preferences may be
  shared automatically, while inferred reusable knowledge requires confirmation before sharing
  across projects.
- Paid, free and local AI use is the user's choice; paid use requires explicit selection and a
  configured spending policy, and a free mode never silently switches to paid use.
- Accepted unprocessed evidence remains available for retry; processed full activity has a
  default 30-day retention from successful processing, and important supporting evidence is
  retained with its knowledge.
- Merging a branch may make knowledge reusable; it does not by itself complete the user's work.
- Existing claude-mem/CMEM Pro memories must be migratable with provenance while preserving
  the original store; imported historical progress is not assumed to be current work.
- Initial completion includes Linux, WSL and macOS, all four coding agents, and device sync;
  native Windows and general chat-application capture are outside this first version.
- Technical details were delegated to the implementer; the owner confirmed this shared
  understanding and instructed implementation to begin.

## User Scenarios & Testing

### Owner amendment — 2026-09-10

- Resident background processing is permitted where it improves convenience. Automatic recovery
  should continue after backoff without requiring another coding event; single-owner fencing,
  bounded processing passes, pause/stop, privacy and resource limits remain required.
- On free-tier/API errors or unusable results, try configured alternative models, including another
  provider/API. Every target needs current consent and cost admission. Earlier permission limits
  remain: a free/local selection never silently enables an unselected paid destination.

### User Story 1 - Failed memory work catches up without losing accepted information (Priority: P1)

The developer keeps working when a summarizer is unavailable, rate-limited or incorrect.
Accepted information remains recoverable, and processing catches up when the chosen service
recovers, without charging an unselected service or creating duplicate memories.

**Why this priority**: A memory system cannot rely on a successful model call as the only chance
to retain information that the developer later needs.

**Independent Test**: Capture a declared set of facts and decisions, make the summarizer fail,
advance beyond the ordinary source-retention period, restore it, and process again; every
accepted source remains accounted for and the expected memories can be retrieved.

**Acceptance Scenarios**:

1. **Given** accepted source information and an unavailable model, **When** fallback activity is
   shown, **Then** the information remains eligible for later processing and is not reported as
   successfully converted into useful knowledge.
2. **Given** a batch too large for one model request, **When** processing splits or clips input,
   **Then** source information omitted from that request remains pending rather than completing
   with the rest of the batch.
3. **Given** a crash before or after a successful model response, **When** processing resumes,
   **Then** recovery neither loses accepted sources nor applies the same effect twice.
4. **Given** a full or unwritable store, **When** capture cannot accept another item, **Then**
   coding continues, the recording failure is visible, and existing unprocessed data is preserved.
5. **Given** information accepted more than 30 days ago, **When** generation succeeds for the
   first time, **Then** its ordinary full-activity retention starts at that successful processing
   outcome, with supporting evidence retained independently.
6. **Given** recoverable pending work and enabled background processing, **When** backoff expires
   after the provider recovers, **Then** processing resumes without another hook or manual command.
7. **Given** resident processing, **When** the owner pauses/stops it or changes consent,
   **Then** new requests cease, accepted work remains retained, and another worker cannot concurrently
   take an active owner's lease merely because the queue is temporarily idle.

### User Story 2 - Continue the intended work across agents and computers (Priority: P1)

The developer resumes work A in another agent or on another computer and receives A's current
purpose, constraints, decisions, open questions and next actions, without confusing it with work B.

**Independent Test**: Interleave two worktrees and two distinct purposes in one worktree, then
resume each through another agent and another replica; verify the selected progress and lineage.

**Acceptance Scenarios**:

1. **Given** one unambiguous continuation, **When** a new session begins, **Then** the relevant
   work is selected automatically and no handoff document is required.
2. **Given** multiple plausible continuations, **When** intent remains unclear, **Then** the user
   sees a short choice and unrelated active progress is withheld until the choice is resolved.
3. **Given** a clear new purpose, **When** the developer changes work in the same conversation,
   **Then** progress is separated without splitting ordinary investigation into new work items.
4. **Given** a branch merge with deployment or another requested step still outstanding,
   **When** knowledge is adopted by the parent project, **Then** outstanding work stays outstanding.
5. **Given** a removed worktree, **When** its history is searched or explicitly resumed,
   **Then** retained history and provenance remain available without recreating the old directory.

### User Story 3 - Recall useful, current knowledge rather than merely recent text (Priority: P1)

Relevant decisions, explanations and discoveries remain available when asked in different words,
including Japanese, while obsolete progress and superseded facts are not presented as current.

**Independent Test**: Ask paraphrased Japanese and English questions against a known corpus with
contradictions, older valid facts and unrelated work; separately score retention, retrieval,
delivery and the receiving agent's answer.

**Acceptance Scenarios**:

1. **Given** a relevant older fact, **When** the developer asks about it, **Then** age alone does
   not make it unavailable and its supporting source can be inspected.
2. **Given** a corrected or superseded fact, **When** current guidance is requested, **Then** the
   current version is preferred and the old version remains identifiable as historical.
3. **Given** a fact already delivered within the current context, **When** it is correctly omitted
   as a duplicate, **Then** evaluation accounts for its existing availability to the agent.
4. **Given** an expected fact that is missing, **When** diagnostics are inspected, **Then** they
   distinguish capture, request coverage, output/application, retrieval and delivery outcomes
   without publishing model response bodies or credentials.

### User Story 4 - Share knowledge at the right scope (Priority: P1)

Active progress belongs to its work item, reusable project knowledge belongs to its project, and
explicit personal preferences can follow the developer across projects.

**Independent Test**: Create work-specific, project-specific and personal information, then
switch work items and projects; inspect both included and excluded information and the reasons.

**Acceptance Scenarios**:

1. **Given** confirmed reusable knowledge from integrated work, **When** another worktree needs
   it, **Then** it is available with provenance while unfinished progress is not adopted as guidance.
2. **Given** an explicit stable personal preference, **When** another project is used, **Then**
   the preference can apply without exposing project-specific source details.
3. **Given** inferred cross-project knowledge, **When** it is proposed for sharing, **Then** it
   stays within the project until the user confirms the shared form.
4. **Given** an imported record or quoted/tool-generated instruction, **When** scope is assigned,
   **Then** it is not mistaken for a direct user declaration of a global preference.

### User Story 5 - Keep existing memories when switching tools (Priority: P1)

The developer migrates supported claude-mem/CMEM Pro exports or local records without altering
the original store, losing provenance, reviving deleted information, or activating obsolete tasks.

**Independent Test**: Import a frozen mixed-version corpus twice, including conflicting project
names, deleted memories and sensitive material, and verify the resulting scope and counts.

**Acceptance Scenarios**:

1. **Given** a supported source, **When** migration is previewed, **Then** counts, mappings,
   exclusions and unresolved identities are shown before the destination is changed.
2. **Given** the same corpus twice, **When** it is imported twice, **Then** the result has no
   duplicate effects and the source store remains byte-for-byte unchanged.
3. **Given** an ambiguous project mapping, **When** automatic matching is unsafe, **Then** the
   user chooses the mapping and historical work stays historical by default.

### User Story 6 - Carry memory across devices without mandatory service subscriptions (Priority: P1)

The developer continues work on another computer, including macOS, with the same selected work,
knowledge and privacy choices. Offline work catches up safely when connectivity returns.

**Independent Test**: Run two isolated replicas with offline changes, interrupted transfers and
conflicting updates, then reconnect and verify provenance, deletion and conflict handling.

**Acceptance Scenarios**:

1. **Given** an opted-in sync destination, **When** a second device joins, **Then** supported
   knowledge and continuation state arrive without changing their meaning or source identity.
2. **Given** an unselected destination, **When** ordinary work occurs, **Then** no sync transfer occurs.
3. **Given** an interrupted or repeated transfer, **When** sync resumes, **Then** accepted changes
   are neither lost nor duplicated and deletions are not undone.
4. **Given** incompatible concurrent progress updates, **When** neither can safely supersede the
   other, **Then** the conflict is visible instead of silently choosing by machine clock alone.

### User Story 7 - Choose the model, cost and operating mode (Priority: P2)

The developer chooses a supported local model, free service, paid service or existing agent
subscription and sees whether that configuration is ready for useful memory generation.

**Independent Test**: Select each cost class, exhaust a configured allowance and change a
destination; verify calls, consent and pending work against the selected policy.

**Acceptance Scenarios**:

1. **Given** no selected model, **When** setup completes basic capture, **Then** the system states
   that generation is pending and does not claim ordinary memory quality.
2. **Given** a free or local choice, **When** it fails or exhausts its allowance, **Then** no paid
   destination is selected automatically and accepted information remains available for retry.
3. **Given** a paid choice, **When** an attempt would exceed its configured policy, **Then** it is
   deferred and the limit is explained; provider-side and locally estimated limits are distinguished.
4. **Given** a change of destination or data class, **When** consent no longer matches,
   **Then** existing consent is not silently reused.
5. **Given** a configured free fallback chain, **When** a model/API fails or exhausts its allowance,
   **Then** another eligible configured model/API is tried within the chain's attempt and cost bounds.
6. **Given** no eligible successful target, **When** the chain finishes,
   **Then** the accepted source remains retryable and diagnostics distinguish each target's fixed
   failure reason from successful generation; no unselected paid request is attempted.

### Edge Cases

- Resume, fork, compaction and explicit new work are distinct events; session end is not task completion.
- Worktree names, directory names and branch names may collide or change; they are not security identities.
- A squash merge, unfinished deployment or removed worktree is not proof that every requested step is done.
- A model may return valid syntax with no useful observations, reject only some outputs, or omit a source.
- Fallback records can be useful temporarily without satisfying the normal-quality completion criterion.
- Failed detection, secrets and private exclusions retain the existing fail-closed treatment.
- Provider loss, process death, stale leases, full storage and offline replicas must preserve accepted work.
- Current and historical statements can contradict; updates must preserve provenance and user corrections.
- Imported project names may not identify the same repository on another computer.
- A configured CLI or successful wiring hook does not prove that its model produced a useful response.

## Requirements

### Functional Requirements

- **FR-001**: Preserve accepted sanitized source information until each item has an explicit
  processing outcome; a request-size limit or temporary fallback must not silently consume it.
- **FR-002**: Automatically retry recoverable generation work after recovery, with bounded
  attempts/backoff and idempotent application; already completed effects must not be duplicated.
  Enabled resident processing wakes for due work without waiting for another coding event.
- **FR-003**: Keep important supporting evidence with knowledge; ordinary processed full activity
  defaults to 30 days from successful processing, while unprocessed accepted information is not
  deleted because of age alone.
- **FR-004**: Distinguish work progress, project knowledge and personal shared knowledge from
  sensitivity; broader scope never relaxes privacy or destination consent.
- **FR-005**: Identify worktree context and clear work boundaries, preserve native resume lineage,
  select unambiguous continuation automatically, and request a short selection otherwise.
- **FR-006**: Adopt confirmed knowledge independently from work completion and preserve historical
  provenance after integration, worktree removal, import and device transfer.
- **FR-007**: Support relevant Japanese/English recall, corrections and provenance, with an
  inspectable reason for omission; chronological age alone is not a prohibition on recall.
- **FR-008**: Share explicit personal preferences automatically only when their origin and scope
  are unambiguous; inferred cross-project knowledge remains a proposal until confirmed.
- **FR-009**: Provide previewable, repeatable migration from supported claude-mem/CMEM Pro data
  and existing Oboete data while preserving source stores, deletions and unresolved mappings.
- **FR-010**: Provide opt-in encrypted device synchronization with resumable delivery, stable
  origin identity, deletion propagation and visible incompatible concurrent updates.
- **FR-011**: Let the user select AI destination and cost policy; never initiate paid fallback
  from a free/local selection without an explicit new choice.
  Configured model/provider fallback is required after eligible free-tier/API failures; validate
  each target's current consent, data eligibility and cost constraints before sending.
- **FR-012**: Distinguish capture/wiring, generation, retrieval and sync health, including pending
  and partial states; neither a green test suite nor a successful hook is sufficient quality proof.
- **FR-013**: Support Claude Code, Codex, Grok Build and Pi on Linux/WSL and macOS for initial
  completion; implement native Windows later without labeling it supported now.
- **FR-014**: Keep primary coding responsive and privacy checks intact; resource evidence covers
  the engine, optional local model and their combined consumption over prolonged use.

### Key Entities

- **Accepted source**: Sanitized activity with stable identity, provenance and processing outcome.
- **Work context**: The project and worktree in which activity occurs, distinct from machine-local paths.
- **Work item**: One user purpose, its current checkpoint, outstanding steps and associated sessions.
- **Knowledge**: A reusable statement with validity, scope, supporting evidence and correction history.
- **Sharing proposal**: An inferred broader-scope statement awaiting confirmation.
- **Replica**: One local copy with stable identity and independently resumable synchronization state.
- **Processing attempt**: A bounded attempt whose input coverage, result and cost-policy outcome can be inspected.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Every accepted source in the failure/recovery corpus remains accounted for across
  clipping, process death, provider recovery and retention; repeated recovery creates zero duplicate effects.
- **SC-002**: All twelve ordered agent pairs resume the intended work without manual handoff,
  with zero unrelated active-work checkpoints in the parallel-work corpus.
- **SC-003**: Both Japanese and English reference-provider evaluations reach at least 90% useful
  fact/decision recall; retention, retrieval, delivery and receiving-agent answers are reported separately.
- **SC-004**: The privacy and scope corpus has zero secret leakage, unauthorized cross-project
  disclosure, unconfirmed inferred personal promotion or unselected paid-provider attempts.
- **SC-005**: Repeated migration and interrupted two-replica sync preserve all accepted records,
  provenance and deletions without duplicate effects or silent conflicting progress overwrites.
- **SC-006**: Initial platform checks run on actual Linux, WSL and macOS environments; unavailable
  credentials or hardware are reported as unverified rather than counted as passes.
- **SC-007**: Capture remains within 300 ms, ready start within 300 ms and pending start within
  1,300 ms on the reference workload; engine peak RSS remains within the existing 150 MiB target,
  with local-model consumption measured separately and together.
- **SC-008**: A seven-day real-use run and increasing 1,000/10,000/100,000-event retained-history
  workloads show no accumulating unprocessed loss or unbounded resident-resource growth;
  storage growth and deferred work remain visible.

## Assumptions

- The initial user is one developer with private stores on multiple computers; team/RBAC,
  native Windows and capturing arbitrary consumer chat applications are outside this version.
- Four agent integrations and migration/sync are required for full completion, but implementation
  is delivered in independently verified increments without calling an increment the finished product.
- A supported reference local model and external model are selected and documented during planning;
  compatibility with arbitrary user-selected models does not imply equal quality.
- Cloud destinations, real provider spending, publication and replacement of the daily installation
  retain their existing explicit activation boundaries; coding/testing does not enable them implicitly.
- Specifications 007/008 and previous evidence remain available as history; their conflicting
  completion/retention/scope assumptions are superseded by this approved feature and its constitution update.
