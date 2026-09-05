# Feature Specification: oboete M1 Self-Use Alpha

**Feature Branch**: `007-oboete-m1-alpha`

**Created**: 2026-09-02

**Status**: Draft

**Input**: User description: "oboete M1 self-use alpha: automatic shared memory across Claude Code,
Codex, Grok Build, and Pi, for one developer on one machine."

## Clarifications

### Session 2026-09-02

- Q: Grok Build 1.0.17 has no pre-turn injection channel (SessionStart and UserPromptSubmit output
  never reaches the model; PreToolUse output arrives after the first tool call). How does M1 treat
  injection into Grok Build? → A: Deferred injection: deliver the pack right after the first tool
  call of the turn, label it as deferred, and judge the Grok-receiving pairs of SC-001/SC-009 at
  the first tool result.
- Q: The default summarizer costs about 45 neurons per call (reasoning cannot be suppressed), so
  the free allowance funds roughly 220 calls a day; per-turn summarization would exhaust it on busy
  days. What cadence does M1 use? → A: Batch: one provider call at session end and one every 10
  turns during a session, capped at 150 provider calls per day with rule-based fallback beyond
  that; the summary text agents' hooks supply for free (last assistant message, compaction summary)
  is captured as raw events and used as summarizer input. Observation granularity is unchanged.
- Q: Are new memories active immediately, or held until the developer approves them? → A:
  Store-then-review: a memory is eligible for injection the moment it is created; the developer
  corrects mistakes afterwards by deleting or pinning in the viewer or the command line.
- Q: When an agent's own memory feature is active (Codex memories, Claude Code auto-memory, Grok
  Build native memory), what does oboete do? → A: Warn only: setup and doctor report that both
  systems will record memory; oboete never reads those stores and never changes their settings
  (the rule already stated for Grok Build in FR-032, extended to all three).
- Q: May the injection scope be widened beyond the same repository in M1? → A: No. Same
  repository is the only scope in M1; per-repository widening and global memories are deferred
  to a later milestone, and every memory keeps its repository identity so widening stays possible.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Memory follows the developer across agents (Priority: P1)

The developer works on a repository in one coding agent (Claude Code, Codex CLI, Grok Build, or Pi),
then opens the same repository in any other of the four agents. The new session already knows the
relevant decisions, discoveries, failed approaches, changes, and next actions from the earlier
session, without the developer having written a handoff note. Within a session, each prompt the
developer submits is enriched with the memories most relevant to that prompt.

**Why this priority**: This is the product. Everything else exists to make this loop safe, cheap,
and inspectable.

**Independent Test**: Run a scripted session in agent A that establishes three distinct facts about
a synthetic repository, then start a scripted session in agent B on the same repository and ask
about those facts. Passes when agent B's first turn contains all three facts from injected memory,
for every ordered pair of the four agents.

**Acceptance Scenarios**:

1. **Given** a completed session in agent A on repository R that produced memories, **When** the
   developer starts a session in agent B on R, **Then** the session start injects the latest
   session summary of R plus every pinned memory of R, and the injected text is visibly marked as
   coming from oboete. On Grok Build the same pack arrives with the first tool call of the first
   turn and is labelled as deferred (FR-045).
2. **Given** memories exist for R, **When** the developer submits a prompt that relates to some of
   them, **Then** the prompt is enriched with the memories that pass the relevance threshold, up
   to a cap proportional to that agent's documented context limit, and never with a memory that was
   already injected earlier in the same session since the last context compaction.
3. **Given** a session in agent A on R has just ended and its summary is still being produced,
   **When** the developer starts a session in agent B on R within seconds, **Then** session start
   waits at most 1 second for the summary and, if it is still not ready, injects the most recent
   raw activity labelled "summary pending" instead of nothing.
4. **Given** memories exist for repository R, **When** the developer starts a session in a different
   repository S, **Then** no memory of R is injected.
5. **Given** the developer's prompt is written in Japanese and a matching memory was written in
   Japanese, **When** the prompt is submitted, **Then** that memory is found and injected.

---

### User Story 2 - The agent is never blocked or slowed by memory (Priority: P1)

Whatever state oboete is in (database missing, locked, or corrupt; background worker dead;
provider unreachable; disk full), the developer's agent turn proceeds normally and the capture step
finishes inside its time budget.

**Why this priority**: A memory tool that adds latency or failures to every turn is uninstalled
within a day. Fail-open availability is a constitutional invariant.

**Independent Test**: Run the 1,000-event fixture through the capture path with each failure
injected in turn (database file removed, database locked by another process, worker process
killed, provider endpoint returning errors, read-only home directory). Passes when every agent
turn completes, every capture step exits successfully within 300 ms, and the events that could not
be written immediately are recovered once the failure is removed.

**Acceptance Scenarios**:

1. **Given** the memory database is locked by another writer, **When** a capture event arrives,
   **Then** the capture step appends the event to a local spool, exits successfully within 300 ms,
   and the event is stored the next time the database is writable.
2. **Given** the background worker has crashed mid-summary, **When** the next session ends,
   **Then** a new worker takes over the pending work, no event is lost, and no event's summary is
   applied twice (a provider call may be repeated once after a crash between the response and
   its application).
3. **Given** the summarization provider is unreachable, **When** a session ends, **Then** the
   session is still summarized by the rule-based fallback and the resulting memories are labelled
   as degraded.
4. **Given** the Pi extension's handler throws or its detached capture child hangs, **When** a Pi
   event fires, **Then** Pi continues its turn (handlers never wait on storage or network), the
   error is contained, and the failure is recorded for `doctor`.

---

### User Story 3 - Nothing sensitive leaves the machine (Priority: P1)

The developer types secrets, pastes tokens, and works inside private repositories. None of that
appears in stored memories, in requests to a remote summarization provider, or in injected context
for another repository. What the developer marks private stays private, and the developer can see
what each destination would receive before enabling it.

**Why this priority**: The default observer is a remote free-tier service. Without a fail-closed
classification the product cannot be enabled at all.

**Independent Test**: Seed a synthetic session with a corpus of known secret shapes, `<private>`
spans, and files matched by a repository-level secret path rule. Passes when zero corpus items
appear in the stored memories, in any outbound request body, or in any injection pack, and when a
row can only reach a remote destination after passing classification.

**Acceptance Scenarios**:

1. **Given** a tool output contains an API key, **When** the event is captured, **Then** the stored
   event and every memory derived from it contain a redaction marker instead of the key, and the
   key never appears in a request to a remote provider.
2. **Given** the developer wraps text in `<private>` tags, **When** the event is captured, **Then**
   the wrapped text is not stored at all.
3. **Given** a newly captured event, **When** it has not yet been classified, **Then** it is treated
   as local-only and is not sent to a remote provider; it becomes eligible for remote summarization
   only after classification passes.
4. **Given** a remote provider is configured but not yet enabled, **When** the developer runs
   setup, **Then** setup displays the destination host, the credential source, the cost class, and
   exactly which sensitivity classes would be sent, and asks for confirmation before enabling it.
5. **Given** text was injected into a session by oboete, **When** that session is captured, **Then**
   the injected text is recognized and is not summarized again as new activity.
6. **Given** a memory is eligible for injection, **When** eligibility is evaluated, **Then** the
   decision depends only on sensitivity and repository, never on which agent produced the memory.

---

### User Story 4 - Setup and doctor (Priority: P2)

The developer installs oboete, runs one setup command that detects the installed agents, chooses
which agents to wire, and gets working hooks. Later, one doctor command tells the developer whether
every part of the pipeline is healthy and, if not, exactly what is degraded and why.

**Why this priority**: Without it, dogfooding cannot start and failures stay invisible.

**Independent Test**: On a fresh isolated user account with the four agents installed, run setup
non-interactively selecting all agents, then run doctor. Passes when doctor reports every agent's
hook wiring, storage health, worker health, provider reachability, and estimated daily allowance as
healthy; then break each item in turn and confirm doctor names it.

**Acceptance Scenarios**:

1. **Given** some of the four agents are installed, **When** the developer runs setup, **Then** the
   installed agents are detected, offered for multi-selection, and the selected agents' hook or
   extension configuration is written without disturbing unrelated existing hooks.
2. **Given** an agent's own memory feature is enabled (Codex memories, Claude Code auto-memory, or
   Grok Build native memory), **When** setup runs, **Then** setup warns that both systems will
   record memory and continues without changing that agent's setting.
3. **Given** oboete is installed, **When** the developer runs doctor, **Then** the report lists,
   per agent, whether capture and injection are wired; whether the storage file is healthy and
   full-text search is available; whether a worker is alive or stale; whether the configured
   provider answers; and the estimated daily allowance remaining together with whether the
   provider has reported exhaustion.
4. **Given** any component is degraded, **When** doctor runs, **Then** each degraded item states the
   reason and the user-facing consequence.
5. **Given** the developer runs pause, **When** sessions occur, **Then** nothing is captured or
   injected until resume, and existing memories are untouched.

---

### User Story 5 - Zero-credential operation and honest degradation (Priority: P2)

A developer with no provider credentials, or one whose free daily allowance is exhausted, still gets
capture, lexical search, and injection. Every place where quality is reduced says so.

**Why this priority**: The free tier is the default and it has a hard daily limit; silent quality
loss would be indistinguishable from a bug.

**Independent Test**: Run the fixture with no credentials configured, then with an allowance counter
set to exhausted. Passes when memories are still produced by the fallback summarizer, injection
packs carry a degraded flag with the reason, and `why` explains the reduced quality.

**Acceptance Scenarios**:

1. **Given** no provider credentials are configured, **When** a session ends, **Then** memories are
   produced by the rule-based summarizer and are marked as such.
2. **Given** the provider reports that the daily allowance is exhausted, **When** the worker runs,
   **Then** it stops calling the provider without retrying, switches to the fallback summarizer
   until the allowance resets at UTC midnight, and the switch is visible in doctor and in every
   injection pack produced meanwhile.
3. **Given** a search returned nothing or the index is unavailable, **When** an injection pack is
   built, **Then** the pack states that it is empty or degraded rather than presenting an empty
   result as a healthy one.

---

### User Story 6 - Inspect, search, pin, and delete memories (Priority: P3)

The developer opens a local viewer or uses the command line to see what oboete remembered,
grouped by session and turn, with sensitivity and provenance visible; searches memories; pins the
ones that must always be injected; deletes wrong ones; and asks why a given pack was injected.

**Why this priority**: Trust comes from inspection, and wrong memories must be correctable, but the
loop works without the viewer.

**Independent Test**: Create memories from a scripted session, open the viewer, search for a known
phrase, pin one memory, delete another, then start a new session. Passes when the viewer shows the
new memory within 2 seconds of creation, the pinned memory is injected at the next session start,
and the deleted memory is never injected again.

**Acceptance Scenarios**:

1. **Given** the viewer is open, **When** a new memory is created, **Then** it appears without a
   manual refresh within 2 seconds, in its session and turn group, with its sensitivity class and
   the agent that produced it.
2. **Given** a memory is deleted, **When** the same content is captured again later, **Then** it is
   not resurrected as an active memory.
3. **Given** a pack was injected, **When** the developer runs `why` for that session, **Then** the
   output lists each included memory with its relevance reason, each omitted candidate with its
   reason, and any degraded state.
4. **Given** an agent is running, **When** the developer or the agent searches memory through the
   agent's tool interface, **Then** search, timeline, and get-by-id are available with the same
   repository and sensitivity boundaries as injection.

---

### User Story 7 - Export, import, and evidence (Priority: P3)

The developer exports all memories to a portable file, imports them on another installation, and
can read committed measurements of hook time, worker memory, and storage growth for a fixed
workload.

**Why this priority**: Portability protects the developer's data; the evidence is the constitutional
precondition for calling M1 done and for any later rewrite decision.

**Independent Test**: Export from one isolated user account, import into another, and compare
memory counts and content hashes. Run the 1,000-event fixture and compare the numbers against the
committed evidence.

**Acceptance Scenarios**:

1. **Given** memories exist, **When** the developer exports, **Then** the file contains every
   non-deleted memory with sensitivity, provenance, and repository identity, and secrets remain
   redacted.
2. **Given** an export file, **When** it is imported, **Then** duplicates are merged by content,
   deletions are preserved, and the import never lowers a memory's sensitivity.
3. **Given** the committed 1,000-event fixture, **When** it is replayed, **Then** the measured
   hook time, worker memory, and storage growth are within the envelope recorded in the evidence.

---

### Edge Cases

- A repository without a git remote, or the same repository cloned at two paths: identity must
  stay stable for the same clone and must not silently merge unrelated repositories.
- Two agents active in the same repository at the same time: both sessions are captured, one
  worker summarizes both, and neither session's injection includes the other's unfinished turn.
- A tool output larger than the summarizer's input limit: the event is stored and the summarizer
  receives an excerpt; a hook payload larger than 1 MB is not read past that bound: the part that
  was read is redacted and stored marked as truncated, its metadata is complete, and it never
  reaches a provider or an injection pack, so the capture budget stays measurable
- A session whose entire content was `<private>`: no memory is produced and nothing is sent
  anywhere.
- The provider returns malformed or non-conforming output: the worker retries once, then falls back
  to the rule-based summarizer for that batch and records the reason.
- The daily allowance runs out in the middle of a batch: the remainder of the batch uses the
  fallback; no event is dropped.
- The machine's clock is wrong or changes: allowance accounting and staleness use the stored
  reset instant and do not double-charge or double-reset.
- The storage file is corrupt: capture spools, doctor reports corruption with recovery steps, and
  no agent turn is blocked.
- The disk is full: capture exits successfully; the loss is counted whenever any writable
  location exists, and when neither the database nor the spool is writable doctor reports the
  unwritable state by probing it.
- The injection limit of the agent is smaller than the built pack: the pack is trimmed by relevance
  order and the trim is recorded in `why`.
- A memory cites a file or commit that no longer exists at HEAD: the memory is injected with a
  staleness note or is skipped, according to the staleness policy.
- The same content is captured twice (re-run of a command, re-delivered hook): one memory results.
- A hook is invoked by an agent that oboete does not recognize: the event is stored with unknown
  provenance and doctor reports it.

## Requirements *(mandatory)*

### Functional Requirements

Capture

- **FR-001**: The system MUST capture, for each of Claude Code, Codex CLI, Grok Build, and Pi,
  the start of a session, each submitted prompt, each tool invocation's input and output, the end of
  each turn, and the end of the session, at the granularity claude-mem uses.
- **FR-002**: Each capture step MUST complete and return control to the agent within 300 ms; when
  it cannot finish its normal work in that time it MUST append the event to a local spool and
  return successfully.
- **FR-003**: Spooled events MUST be recovered into the store before the next summarization pass,
  and recovery MUST be idempotent (no duplicates, no loss).
- **FR-004**: The system MUST derive repository identity itself from the repository's remote or
  location; identity MUST NOT be accepted from the agent or the event payload.
- **FR-005**: Each session MUST record which agent produced it as provenance; provenance MUST NOT
  influence eligibility for summarization or injection.
- **FR-006**: The capture command MUST determine which agent invoked it and MUST NOT assume Claude
  Code when the same configuration is executed by Grok Build.
- **FR-007**: The Pi integration runs inside Pi's process, which has no handler timeout; therefore
  its handlers MUST NOT perform storage or network work in-process. Capture is handed to a detached
  child process, the in-process step is a bounded enqueue with a cooperative deadline, and every
  thrown error is contained so that Pi's turn continues. Pi keeps no durable record of extension
  errors (R13 probe, 2026-09-03: a throw reaches stderr only), so errors are recorded through
  in-memory counters handed to the next child spawn and surfaced by `oboete doctor` (A8).
- **FR-008**: Raw captured events MUST be retained for 7 days and then removed; memories MUST be
  permanent except through explicit deletion (tombstone) or supersession by a newer memory.
- **FR-009**: The system MUST NOT run a resident background service; all background work MUST be
  performed by a detached worker that exits when its queue is empty.

Summarization

- **FR-010**: When a session ends, and during a session after every 10 turns, a single background
  worker per machine MUST summarize the accumulated new events in one provider call per batch into
  memories using claude-mem's observation types, and MUST classify each candidate against nearby
  existing memories as add, update, delete, or no-op. Summary text that an agent's hooks supply
  without a provider call (the last assistant message of a turn, the conversation summary produced
  at compaction) MUST be captured as raw events and used as summarizer input.
- **FR-011**: Exactly one worker MUST be active per machine at a time; a stale or crashed worker's
  work MUST be taken over by the next worker without loss or duplication.
- **FR-012**: The summarizer MUST use the configured provider preset; the default preset MUST be a
  free-tier remote service. The system MUST count its own usage as a pacing estimate that resets
  at UTC midnight and MUST label it as an estimate (the allowance is account-wide and no interface
  returns the true remainder); the authoritative exhaustion signal is the provider's account-limit
  error, which MUST NOT be retried. Provider calls MUST be capped at 150 per day (derived from the
  measured cost of about 45 neurons per call against the 10,000-neuron daily allowance); beyond
  the cap the rule-based summarizer is used and the switch is labelled; when 10 or fewer calls
  remain, mid-session batches use the rule-based summarizer and the remaining calls are reserved
  for session-end batches. The presets MUST include at least one local-model option and MAY
  include an `agent-cli` option that summarizes through an already-authenticated agent CLI
  (`claude -p`, `codex exec`, `grok -p`); that option consumes the developer's own subscription,
  is shown as such on the consent screen, and is exempt from the 150-call cap because it uses no
  oboete allowance.
- **FR-013**: When no provider is configured, the provider is unreachable, the provider's output is
  unusable, or the allowance is exhausted, the system MUST produce memories with a rule-based
  summarizer in the same shape, and MUST label those memories and the affected injection packs as
  degraded with the reason.
- **FR-014**: Summaries MUST be written in the language of the underlying content.
- **FR-015**: Each summarizer input MUST be bounded to 12,000 characters; longer inputs MUST be
  excerpted and the excerpting recorded.
- **FR-016**: The system MUST never read credentials from other agents' session files or
  subscription stores; credentials MUST come only from oboete's own configuration or an
  explicitly named environment variable.

Privacy and sensitivity

- **FR-017**: Every captured row MUST be classified at capture as local-only by default; a row MAY
  be promoted to eligible only after the background worker's secret detection and entropy checks
  pass; a row MUST be classified secret when it matches a secret rule or a repository-level path
  rule.
- **FR-018**: Secrets MUST be redacted before storage in events, and again in generated memories.
- **FR-019**: Text wrapped in `<private>` tags MUST be removed at capture and MUST NOT be stored.
- **FR-020**: Egress MUST follow a single rule table: remote summarizer receives eligible rows
  only; a local summarizer receives eligible, local-only, and private rows of the same repository;
  injection receives rows of the same repository only; secret rows are never sent to any
  destination.
- **FR-021**: Text injected by the system MUST be marked so that it is recognized on capture and
  not summarized as new activity. The marker and the pack MUST be plain factual text: the payload
  MUST NOT begin with `{` and end with `}` (Claude Code parses such output as JSON and silently
  drops it on failure) and MUST NOT be phrased as instructions to the agent.
- **FR-022**: Before enabling any remote destination, setup MUST display the destination host, the
  credential source, the cost class, and the sensitivity classes that would be sent, and MUST
  require confirmation.
- **FR-023**: The privacy invariants MUST have tests in both directions: a fail-closed test that
  restricted content is blocked, and a fail-open test that eligible content is delivered.

Retrieval and injection

- **FR-024**: At the start of a fresh session, and again after context compaction, the system MUST
  inject the most recent session summary of the same repository plus the pinned memories of that
  repository; on a resumed session it MUST NOT inject again, because the agent replays the earlier
  injection from its transcript. The pack MUST be bounded to the delivering channel's per-value
  ceiling (10,000 characters on the Claude Code and Grok Build hook channel), pinned memories
  trimmed in pin order with the trim recorded per FR-028. When the previous session's summary is
  still pending it MUST wait at most 1 second and then inject the latest raw activity labelled
  "summary pending". A context compaction opens a new context epoch of the conversation.
- **FR-025**: At prompt submit the system MUST retrieve memories of the same repository by lexical
  relevance to the prompt, including Japanese and other CJK text, and inject those above a
  relevance threshold up to a cap proportional to the agent's documented context limit; the
  amount MUST NOT be a fixed token count.
- **FR-026**: The system MUST NOT inject the same memory twice within one context epoch of an
  agent conversation, counting resumed continuations of that conversation as the same
  conversation; a context compaction opens a new epoch, so the re-injection FR-024 requires after
  compaction is not a duplicate.
- **FR-027**: For Codex, injection MUST occur only at session start and prompt submit.
- **FR-045**: For Grok Build, which has no channel that reaches the model before a turn starts,
  the session-start and prompt-submit packs MUST be delivered with the first tool call of the turn
  that actually runs: the pre-tool hook attempts delivery, the post-tool hook confirms it, and a
  pack attempted on a denied call stays pending and is attempted again on the next call; the
  delivery MUST be labelled as deferred in `why` and in doctor; a turn with no tool call, or whose
  calls were all denied, receives nothing and the omission is recorded.
- **FR-028**: Every injection pack MUST record what was included, what was omitted and why, and any
  degraded state; `why` MUST present that record for a session.
- **FR-029**: Memories MUST carry citations (file paths, commits) when the source provides them, and
  citations MUST be checked against the repository's current state before injection.
- **FR-030**: Search, timeline, and get-by-id MUST be available to all four agents through a tool
  interface and through the command line, under the same repository and sensitivity boundaries as
  injection.

Setup, doctor, and lifecycle

- **FR-031**: Setup MUST detect installed agents, let the developer select which to wire, and write
  the three hook installations (Claude Code and Grok Build shared, Codex, Pi) without disturbing
  unrelated configuration; setup MUST be repeatable and MUST offer removal. Each installation MUST
  be complete enough to fire: user-global locations that need no per-project trust (Pi's global
  extension directory), the trust entry Codex requires before it runs a hook (a hash of the
  canonical handler definition, written next to the hook), an explicit per-hook timeout no smaller
  than the work the hook serves (the 1-second session-start wait plus margin; Grok Build's default
  is 5 seconds), and per-hook output limits set so that the pack oboete computes is what reaches
  the model (Codex's default 2,500-token spill MUST be disabled). Setup MUST verify by a probe that
  each hook actually fires and MUST report each agent's trust state before reporting success.
- **FR-032**: Setup and doctor MUST warn when an agent's own memory feature is enabled (Codex
  memories, Claude Code auto-memory, Grok Build native memory) and MUST NOT change that setting.
- **FR-033**: Doctor MUST report, with reasons, the health of: per-agent capture and injection
  wiring (verified by a probe that the hook actually fires, including each agent's trust state,
  not by the presence of a configuration file), storage integrity, full-text search availability,
  worker liveness, provider reachability, the estimated daily allowance remaining (labelled as an
  estimate) and whether the provider has reported exhaustion, spool backlog, and any unrecognized
  agent.
- **FR-034**: The developer MUST be able to pause and resume capture and injection without losing
  or altering existing memories.
- **FR-035**: The developer MUST be able to pin, unpin, and delete memories; deleted memories MUST
  NOT be re-created from the same content, where "the same content" means the same normalized
  title and body regardless of observation type.
- **FR-036**: The developer MUST be able to export all non-deleted memories with sensitivity,
  provenance, and repository identity, and import such a file with merge by content, preservation
  of deletions, and no lowering of sensitivity.

Viewer

- **FR-037**: A local viewer MUST show sessions grouped by turn, memories with sensitivity and
  provenance, and live updates within 2 seconds, and MUST support search, pin, and delete.
- **FR-038**: The viewer MUST be reachable only from the local machine.

Platform and evidence

- **FR-039**: The system MUST NOT depend on Linux-only facilities; configuration and data paths
  MUST live under one data directory (`~/.oboete/`) relocatable through `OBOETE_HOME`; a
  per-platform XDG/AppData split is deferred to a later milestone.
- **FR-040**: A committed fixture of 1,000 events MUST exist, and the measured capture time,
  worker memory, and storage growth for it MUST be recorded before M1 is declared done.
- **FR-041**: Automated end-to-end validation MUST run under an isolated user account with real
  agent hooks for all four agents; oboete MUST NOT be installed into the maintainer's own agent
  environment during M1.

Owner decisions (resolved in Clarifications)

- **FR-042**: New memories MUST become eligible for injection the moment they are stored; there is
  no approval queue. Correction happens after the fact through deletion and pinning (FR-035).
- **FR-043**: The system MUST NOT read another agent's memory store as a memory source and MUST
  NOT advise or perform disabling it; coexistence is reported per FR-032 and otherwise ignored.
- **FR-044**: Injection scope MUST be the same repository only; no configuration in M1 widens it
  to other repositories or to repository-independent memories, and every memory MUST keep its
  repository identity so that a later milestone can widen the scope without migration.

### Key Entities

- **Repository**: A code repository identified by a derived stable identity; the primary boundary
  for injection.
- **Session**: One run of one agent in one repository; carries agent provenance, start and end,
  and its latest summary.
- **Raw Event**: One captured occurrence (prompt, tool input/output, turn end, session end) with a
  sensitivity class; the acceptance point; retained 7 days.
- **Memory**: A summarized, typed observation derived from events; permanent; has content
  identity, sensitivity, citations, validity period, supersession link, pin state, and tombstone.
- **Injection**: A record of what was injected into which session, with the reasons for inclusion
  and omission and the degraded state.
- **Provider Preset**: A named summarizer configuration with destination host, credential source,
  cost class, and allowance accounting.
- **Worker Lease**: The record that makes one background worker the active one per machine.
- **Spool Entry**: An event that could not be stored within the capture budget, awaiting recovery.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For every ordered pair of the four agents, a session in the second agent receives all
  three seeded facts from a preceding session in the first agent on the same repository, on the
  first turn (12 of 12 pairs pass in the isolated end-to-end run); for the three pairs where Grok
  Build receives, the facts MUST be present by the time the first tool call of the first turn
  completes.
- **SC-002**: Across the 1,000-event fixture, the capture step returns within 300 ms for at least
  99% of events, and 100% of agent turns complete under every injected failure.
- **SC-003**: The background worker stays under 150 MB of resident memory for the fixture, and
  storage growth per 1,000 events is recorded in the evidence file.
- **SC-004**: With no provider credentials, the end-to-end scenario still passes SC-001 using
  fallback summaries, and every injection pack carries a visible degraded marker.
- **SC-005**: Zero items from the seeded secret corpus appear in stored memories, outbound
  requests, or injection packs.
- **SC-006**: Zero local-only or private rows appear in any outbound request; zero eligibility
  decisions differ when only the producing agent is changed.
- **SC-007**: Isolated dogfood across all four agents stays green for at least 7 consecutive days
  before M1 is declared done.
- **SC-008**: Setup for all four agents completes in under 2 minutes, and doctor names every
  deliberately broken component in the break-one-at-a-time test.
- **SC-009**: For seeded Japanese and English facts, the correct memory is among the injected
  memories for at least 90% of matching prompts (on Grok Build, among the memories delivered with
  the first tool call of that turn).
- **SC-010**: Zero duplicate injections of the same memory within a session's context epoch across
  the fixture (a compaction opens a new epoch).
- **SC-011**: A newly created memory appears in the viewer within 2 seconds.

## Assumptions

- The maintainer is the only user of M1; multi-user, team, and hosted scenarios are out of scope.
- Injection scope is the same repository only; repository groups and global memories are deferred.
- Milestones M2-M5 (vector search, encrypted cloud sync, package publication, macOS, Windows) are
  out of scope, and M1 decisions must not preclude them.
- Storage lives in one file under the user's oboete directory, following the platform's user
  directory conventions; no server process is ever left running.
- The default summarizer is a remote free-tier service; its daily allowance is account-wide,
  oboete's own count is a pacing estimate, and the reset is assumed at UTC midnight. Summarization
  is batched (session end and every 10 turns), never per turn; the batch size and the 150-call
  daily cap are tunable in configuration.
- Staleness policy default: a memory whose citations no longer resolve at the repository's current
  state is injected with a staleness note rather than dropped; a memory not injected for 90 days is
  retired from automatic injection but remains searchable, and any injection resets the counter.
- Installed size target: the installed package including dependencies stays under 30 MB unpacked;
  a larger footprint requires a written reason in the plan.
- The viewer in M1 is read, search, pin, and delete only; editing memory text and promoting a
  memory into an agent instruction file are deferred.
- The set of observation types and the capture granularity are taken from claude-mem as the
  reference; adaptations are documented, not silently changed.
- The 300 ms capture budget, 150 MB worker limit, 12,000-character summarizer input, and 7-day
  raw event retention are the constitutional values and are not tuned in M1 without an
  evidence-backed amendment. The 1-second summary wait applies to session-start injection only
  (constitution 3.1.0 distinguishes the capture and injection budgets). Search in M1 is lexical
  only and says so in empty results and in `doctor`; semantic search is M2.
- Legacy free-mem assets (destination boundary rules, fixtures, mutation gate) may be ported when
  a task needs them; nothing else from `legacy/` is reused.
- Third-party contracts (each agent's hook payloads and injection limits, the provider's structured
  output behavior) are verified and recorded under `docs/research/` before the plan depends on
  them; where a contract turns out narrower than assumed, the affected requirement is adjusted in
  the plan with a note here.
- Adjusted from the verified contracts in `docs/research/oboete-contracts-2026-09-02.md`
  (2026-09-02): FR-007, FR-012, FR-021, FR-024, FR-026, FR-031, FR-033, and the related
  acceptance scenarios in User Stories 2, 4, and 5.
