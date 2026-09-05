# Feature Specification: Slice 1 Automatic Memory Runtime

**Feature Branch**: `spec/slice1-contract-post-142`

**Created**: 2026-08-30

**Status**: Ready

**Input**: Product Reset Slice 1 issue #137, including local-only egress blocker #130 and the
Product Reset contracts under `specs/005-product-reset/`.

## Clarifications

### Session 2026-09-01

- Q: How does new PR3 provider XML bind each summary or observation to exact projected sources? → A:
  Each item contains an ordinal-based `citations` child; the active claim maps bounded source
  ordinals to exact raw-event IDs, and optional spans use half-open UTF-8 byte offsets.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Keep Device-Only Memory On Device (Priority: P1)

As a developer, I can mark work as local-only and trust that no remote or wrong-repository consumer
can receive it, including after summaries, durable memories, indexes, exports, or traces are derived
from it.

**Why this priority**: Device-only content reaching a remote summary provider is a current privacy
defect and blocks any user-facing egress guarantee.

**Independent Test**: Capture fixed eligible, local-only, private, secret, degraded, and legacy
sentinels in two repositories; exercise every provider, retrieval, maintenance, MCP, viewer, and
export boundary; prove exact request counts, transmitted/rendered bytes, inherited sensitivity, and
repository eligibility.

**Acceptance Scenarios**:

1. **Given** a remote or unknown summary destination and an all-restricted job, **When** processing
   runs, **Then** no provider request is made, no restricted byte is materialized for the provider,
   and an atomic payload-free privacy skip completes the source range.
2. **Given** a remote summary destination and a mixed job, **When** processing runs, **Then** only
   eligible events are sent in source order and local-only, private, secret, and degraded events
   contribute zero request bytes.
3. **Given** an explicit loopback HTTPS provider with verified peer identity and a source with a
   verified repository identity,
   **When** local-only or private work is processed, **Then** it may produce memory while retaining
   its strongest sensitivity and exact repository identity; secret work remains excluded.
4. **Given** a restricted memory, **When** search, recent, timeline, explain, a reference query,
   daemon get/search/pack, MCP direct/indexed read, viewer read, maintenance, export, or pack trace
   reaches an unknown, remote, or different-repository destination, **Then** the content is omitted
   before materialization and only a closed EgressDiagnosticV1 reason/count remains.
5. **Given** two repositories with the same basename or caller-supplied project label, **When** a
   restricted item is requested, **Then** the label grants no authority and only an exact verified
   repository identity can match.
6. **Given** a legacy item with unknown sensitivity or repository identity, **When** any disclosure
   path evaluates it, **Then** it is treated as secret or otherwise ineligible, never eligible by
   default.
7. **Given** export includes memory, prompt, legacy summary, and session sections, **When** it is
   serialized, **Then** every content-bearing row passes the same destination/repository boundary,
   session shells omit cwd/remote/user/free-form metadata, and legacy v1 imports cannot become
   remotely eligible.
8. **Given** a Claude Code, Codex, or MCP caller claims that its locally running client implies an
   on-device model, **When** injection eligibility is resolved, **Then** the claim is ignored and the
   destination is remote/unknown; Slice 1 production creates no on-device Agent attestation, so only
   runner-owned local-consumer fixtures may select a local destination.

---

### User Story 2 - Recall Work Across Claude Code and Codex Automatically (Priority: P1)

As a Linux or WSL developer, I can use one setup flow, work in Claude Code, switch to Codex, and
receive the relevant summary and durable facts automatically; the same works in reverse without a
handoff file.

**Why this priority**: Bidirectional automatic memory is the first user-visible Product Reset slice.

**Independent Test**: Run the committed bidirectional fixture in isolated user and data directories
for both Agent directions and verify required facts, forbidden facts, provenance, processing
triggers, and injection reasons at the receiving prompt boundary.

**Acceptance Scenarios**:

1. **Given** Claude Code captures a decision and next action, **When** the user starts the matching
   Codex task, **Then** both facts and their source evidence arrive automatically and no unrelated or
   forbidden fact appears.
2. **Given** Codex captures a failed approach and its correction, **When** the user returns to Claude
   Code, **Then** the correction arrives automatically and the failed approach is not recommended.
3. **Given** an identical event or semantic no-op is delivered again, **When** processing converges,
   **Then** no duplicate active summary, memory, semantic entry, or injection item is created and
   deduplication cannot weaken sensitivity or repository identity. A same-payload replay with
   stronger trusted sensitivity atomically strengthens the canonical event and every already-derived
   record; quarantine is absorbing and returns no normal ACK.
4. **Given** newly accepted activity, session end, or pre-compact, **When** the lifecycle signal is
   committed, **Then** the existing sweeper is nudged using the frozen profile and relevant work is
   processed within the hook deadline without depending only on the idle sweep.

---

### User Story 3 - Keep Working Through Memory Failures (Priority: P1)

As a developer, I can continue normal Agent work while the memory runtime, summary provider, or
semantic lane is unavailable. Accepted work remains recoverable, retries are bounded, and context
uses the strongest truthful fallback.

**Why this priority**: A background memory tool that blocks the Agent, loses accepted work, or
reports an empty result as healthy is worse than no memory tool.

**Independent Test**: Interrupt capture acknowledgement, claims, provider calls, output commit, and
sweeper shutdown; restore them; verify durable admission, retry exhaustion, one-shot resume,
frontier atomicity, retained sources, deduplication, and lexical fallback.

**Acceptance Scenarios**:

1. **Given** the daemon is unavailable during capture, **When** supported activity occurs, **Then**
   the Agent continues and the activity enters the bounded atomic spool for later delivery.
2. **Given** processing capacity is full, **When** another event is accepted, **Then** no event or job
   is evicted, the event remains visibly not admitted, and retry-exhausted jobs still count against
   capacity.
3. **Given** a worker crashes or loses acknowledgement, **When** another worker recovers the job,
   **Then** claim generation rejects stale completion and one canonical result is committed.
4. **Given** a job exhausts automatic attempts, **When** timer sweeps continue, **Then** it remains
   retry-exhausted with retained sources and receives no attempt until one explicit valid resume
   grant is consumed.
5. **Given** changed validated provider configuration, **When** it grants a resumed attempt, **Then**
   the new attempt has a new manifest/provider-bound fingerprint while immutable admission
   provenance and monotonic lifetime attempt count remain unchanged.
6. **Given** a legacy `gave_up` range whose source rows are incomplete, **When** migration runs,
   **Then** it becomes an honest terminal `legacy_unrecoverable` disposition without success claim,
   frontier rewind, capacity deadlock, or fabricated missing work.
7. **Given** semantic retrieval is disabled or unavailable, **When** context is requested, **Then**
   lexical retrieval remains available, existing vectors remain stored, and the degradation reason
   is truthful.
8. **Given** a nudge arrives during an active flush while shutdown starts, **When** the sweeper stops,
   **Then** it waits for active work and schedules no post-stop timer or flush; a later explicit start
   may resume scheduling.
9. **Given** the same repository/event identity arrives with a different canonical payload digest,
   **When** capture retries, **Then** the canonical event is unchanged, one durable non-success
   identity-conflict receipt is reused for that ordered digest pair, a different conflicting digest
   receives a different receipt, no ACK/memory is fabricated, and Agent work remains open.

---

### User Story 4 - Activate and Diagnose One Explicit Runtime (Priority: P2)

As a developer, I can inspect and activate one minimal runtime profile whose complete provider
endpoint, wire protocol, credential reference, location, cost class, and egress policy are explicit.
Setup, runtime, and diagnostics agree on that profile and do not silently read another provider or
resource configuration path.

**Why this priority**: Privacy and lifecycle guarantees are enforceable only when every product
surface consumes the same effective configuration.

**Independent Test**: Compile and activate the minimal profile on supported and unsupported
environments, exercise absent/malformed/current/rollback states, and compare setup disclosure,
daemon snapshot, doctor, Observer transport, maintenance, viewer, and later managed lifecycle.

**Acceptance Scenarios**:

1. **Given** an explicit provider proposal, **When** setup compiles it, **Then** it accepts only one
   supported wire protocol, a complete canonical endpoint URL, an explicit `none` or environment
   credential reference, and computed non-secret fingerprints; it derives location, egress, TLS,
   redirect, and cost behavior rather than trusting those as inputs.
2. **Given** a remote endpoint, **When** validation runs, **Then** HTTPS and system certificate and
   hostname verification are required; **given** a local endpoint, **Then** only literal `127.0.0.1`
   or URL hostname `[::1]` is local and `localhost` is rejected. An insecure TLS-disable environment
   rejects remote activation/start, and redirects are never followed.
3. **Given** a valid proposal, **When** setup reaches activation, **Then** it displays protocol,
   complete safe endpoint, credential source, location, cost class, egress policy, TLS policy, and
   both fingerprints without secret values and waits for explicit confirmation before any mutation.
4. **Given** a running daemon during the first manifest delivery, **When** manual setup attempts to
   mutate configuration, **Then** setup stops before mutation; coordinated stop/start/attach behavior
   remains part of the later lifecycle increment.
5. **Given** setup failure after confirmation, **When** rollback runs, **Then** the previous manifest
   pointer and every touched Claude/Codex configuration file are restored together; an interrupted
   transaction is recovered or rejected before daemon provider startup, and a target changed outside
   the journal causes all targets to remain unchanged while recovery remains blocked.
6. **Given** no active manifest, **When** the daemon starts, **Then** it enters explicit capture-only
   restricted mode with no provider and no sweeper; **given** a malformed or mismatched pointer,
   **Then** daemon startup fails before provider construction.
7. **Given** an active valid manifest, **When** daemon, doctor, maintenance, and viewer inspect it,
   **Then** they use one frozen snapshot and report one identical safe fingerprint; no runtime
   consumer rereads legacy provider or resource env/config.
8. **Given** a legacy setting conflict, **When** setup compiles a proposal, **Then** activation is
   rejected; an active manifest may contain only `translated`, `ignored`, or `overridden`
   dispositions with no legacy values.
9. **Given** the later managed-lifecycle increment, **When** supported setup completes, **Then**
   Claude and Codex are installed, one runtime starts or attaches, and version/fingerprint/doctor
   readiness is verified without duplicate writers.

### Edge Cases

- A provider URL with credentials, fragment, non-canonical form, runtime-appended API path, unknown
  wire protocol, `localhost`, remote HTTP, insecure TLS override, or unsupported loopback spelling
  is rejected before activation.
- The deterministic provider stub remains harness metadata. The runner materializes an ordinary
  closed provider proposal; production has no stub provider kind or registry entry.
- Local-derivation and output-limit recovery configurations are complete immutable successor
  manifests bound to the prior fingerprint, never scenario-local partial overlays.
- The corrected fixture uses one base remote choice (`openai_chat_completions_v1`, complete remote
  HTTPS endpoint, environment credential `FREE_MEM_SUMMARY_API_KEY`, derived
  `external_metered`), one complete local successor
  (`https://127.0.0.1:1234/v1/chat/completions`, credential `none`), and one complete repaired-remote
  successor. Configuration, redirect, and HTTPS-downgrade recovery signals bind to the computed
  repaired manifest/provider fingerprints rather than free-form labels.
- Before schema v21 jobs and the complete all-consumer privacy boundary are merged, even a valid
  active manifest remains `pending_privacy_boundary`: capture stays available, but provider calls,
  AI maintenance, and the RawEventSweeper remain disabled. This keeps each prerequisite PR safe to
  merge without enabling #130 early.
- A mixed source range keeps stable event order and exact bounded provenance after projection.
- A newly admitted v21 batch has at most 100 source events. More accepted work remains for a later
  job; it is not silently truncated or dropped. Migration preserves a wider immutable v20 recovery
  range created by the old configurable worker rather than truncating or misclassifying it.
- The existing output-limit recovery case may activate only the closed `slice1-short-run` version 2
  successor: compared with version 1, only `version=2` and
  `maxMemoryItemsPerDerivation=17` differ. It is not a
  selectable production profile or arbitrary resource override; it remains a runner-owned test-only
  fault contract.
- Redaction-degraded input is never eligible even when payload data claims otherwise.
- A local-only derived memory remains restricted through retry, restart, dedup, supersession,
  backup/restore, export/import, search, reference lookup, MCP, viewer, trace, and injection.
- Raw-event retention is disabled in Slice 1. A later non-zero policy is invalid unless every
  uncompleted job range, including retry-exhausted work, is exempt from purge.
- Queue pressure, duplicate delivery, lost acknowledgement, and interruption cannot evict accepted
  work or create a fabricated successful run.
- Repository paths containing spaces and linked worktrees resolve to the same verified remote or
  realpathed primary Git anchor identity without using the basename as authority.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The product MUST support this slice on Linux and WSL local Linux filesystems for Claude
  Code and Codex only.
- **FR-002**: The completed slice MUST provide one setup flow for both Agent lanes without a
  user-authored handoff file; lifecycle start/attach automation MAY land after the first manual,
  daemon-stopped manifest activation boundary. Default activation targets Claude Code and Codex;
  OpenCode is not a Slice 1 destination or credential/config source.
- **FR-003**: A summary provider proposal MUST be a closed v1 value containing role, state, model
  identity, exactly one `wireProtocol` of `anthropic_messages_v1` or
  `openai_chat_completions_v1`, a complete canonical `endpointUrl`, and a `CredentialRefV1` of
  `none` or one named environment variable. Production MUST NOT use a generic provider registry,
  arbitrary headers, inline secrets, or runtime-appended endpoint paths. The frozen profile MUST
  also fix request timeout, input/output/response limits, and temperature.
- **FR-004**: The compiler MUST compute `providerFingerprint` and derive execution location,
  egress policy, cost class, TLS policy, and redirect rejection. Literal `127.0.0.1` or URL hostname
  `[::1]` is local; `localhost`, localhost subdomains, any trailing-dot hostname, wildcard or
  unspecified addresses (including IPv4-mapped unspecified), and every other loopback spelling
  (including IPv4-mapped loopback) are rejected; every other accepted endpoint is remote and
  HTTPS-only with system
  TLS verification; `NODE_TLS_REJECT_UNAUTHORIZED=0`, production added-CA path/environment input, or
  an equivalent trust bypass rejects activation/start. The runner may install its public test CA
  only into isolated system trust before candidate start; redirects are rejected. Before mutation
  and again at daemon start, remote/local-
  HTTPS endpoints MUST pass a credential-free, payload-free native TLS chain/hostname handshake
  within the frozen 5,000 ms preflight timeout. Setup-time failure aborts activation with zero
  mutation. Daemon-start failure MUST still start writer/RPC/capture/spool-import/lexical services,
  disable only provider/AI processing, report `provider_unavailable` or `provider_tls_rejected`, and
  preserve work for a later validated healthy transition.
- **FR-005**: Setup MUST compute the manifest fingerprint, display all safe effective provider fields
  and fingerprints, and obtain explicit confirmation before any activation or editor mutation.
- **FR-006**: Setup MUST atomically activate an immutable manifest generation with all targeted
  editor mutations and `control/install-manifest.json`, or restore their prior state. A running daemon MUST block manual mutation until
  coordinated lifecycle automation owns stop/activate/start. A narrow owner-only durable setup
  journal MUST include the mode-0600 activation receipt target/hash/prestate and make interruption
  recoverable; capability `current` is published last. Daemon import MUST require receipt/current
  fingerprint agreement. Recovery may finalize or restore only targets matching the recorded
  prestate or journal-owned poststate; if any target has an unknown external hash, recovery MUST
  modify no target, retain the journal, and block provider startup. Daemon
  provider startup fails while an unresolved transaction exists. Setup activation and daemon start
  MUST share one lifecycle lock and fixed lock order, with daemon state rechecked while held, so a
  daemon cannot start between preflight and pointer/editor mutation.
- **FR-007**: An absent active manifest MUST yield capture-only restricted mode with no provider and
  no sweeper. A malformed pointer, missing referenced generation, fingerprint mismatch, unknown
  field, or invalid active manifest MUST fail daemon startup. A valid manifest MUST NOT by itself
  enable provider, AI-maintenance, or sweeper execution before schema v21 jobs and the complete
  DestinationBoundary are active; doctor MUST report `pending_privacy_boundary` meanwhile.
- **FR-008**: An active manifest MAY record at most 64 unique legacy keys matching the v1 key syntax,
  each with `translated`, `ignored`, or `overridden`. A conflict MUST prevent activation, and no
  legacy value or secret may enter the manifest, fingerprint, logs, or diagnostics.
- **FR-009**: The fixed Slice 1 resource profile MUST own the accepted fixture limits plus 30,000 ms
  periodic sweep, 120,000 ms idle flush, 1,000 ms event debounce, 300,000 ms stuck-claim timeout,
  raw retention disabled/0, at most 100 source events per newly admitted v21 job, observer request
  timeout 60,000 ms,
  max input 12,000 characters, max output 4,000 tokens, max response 1,048,576 bytes, and temperature
  0.2, plus provider TLS preflight timeout 5,000 ms. Runtime MUST NOT reread mutable
  provider/resource env or legacy config for behavior claimed by the manifest. The only accepted
  successor is the existing output-limit recovery version 2, identical except for derivation limit
  17 and runner-owned/test-only; production setup MUST NOT expose it as a selectable profile.
- **FR-010**: Every accepted event MUST persist first-class `eligible`, `local_only`, `private`, or
  `secret` sensitivity, repository identity, and capture-time manifest identity independently of
  payload claims. Repository identity MAY be NULL/unknown when Git authority cannot be verified, and
  capture-manifest fingerprint MUST be NULL in absent-manifest capture-only mode; those NULL values
  are preserved and fail closed before identity/manifest-dependent processing, without rejecting
  capture itself. A same-identity/same-payload replay MUST atomically retain the strongest trusted
  sensitivity in the canonical row and every already-derived record before ACK; quarantine MUST be
  absorbing, make those records ineligible, and return a non-success receipt.
- **FR-011**: Repository identity MUST be computed from a verified canonical Git remote or a
  realpathed primary Git anchor. Canonical remote identity MUST retain transport class and the exact
  supported SSH username; HTTPS and SSH or distinct SSH usernames MUST NOT collapse. Basename,
  project label, workspace label, or caller override MUST remain display/filter metadata and MUST
  NOT authorize restricted disclosure. The current canonical remote MUST be revalidated before
  capture and every restricted boundary; changing `origin` MUST invalidate reuse of the prior
  remote-derived identity, and probe failure MUST fail closed to a currently verified anchor or
  unknown.
- **FR-012**: Redaction failure MUST retain no raw content and MUST persist restart-stable safe
  ordering metadata, `secret` sensitivity, quarantine state, and the closed code
  `redaction_degraded` without free-text error content.
- **FR-013**: Before any provider prompt, context, transcript, request, or diagnostic content is
  constructed, the product MUST project candidates through the frozen destination boundary.
- **FR-014**: A remote all-restricted job MUST make zero provider requests and complete via one
  atomic privacy skip. A mixed remote job MUST preserve eligible order/provenance and transmit zero
  restricted bytes. Unauthenticated loopback HTTP is eligible-only and MUST use no credential.
  A loopback HTTPS provider MAY process private/local-only work only after successful chain,
  hostname/IP, and peer verification and with a known source repository; secret work is never
  processed. Before any local prompt is built, events MUST be partitioned by exact verified
  repository identity, and a mixed or unknown group MUST be rejected content-free.
- **FR-015**: Every derived memory MUST cite only event IDs/spans in the job's exact projected source
  set and inherit the strongest cited sensitivity, exact repository identity,
  manifest/provider/attempt provenance, and deterministic lineage/revision identity. A newly
  admitted v21 job projects at most 100 events; a migrated legacy recovery job may project its wider
  immutable actual range. Each new provider-produced `<observation>` or `<summary>` MUST contain
  exactly one direct non-empty `<citations>` child with one or more self-closing
  `<cite source="N"/>` elements; `N` is the zero-based ordinal in the exact ordered projected set
  shown to the provider. A cite MAY add `start` and `end` together as
  half-open UTF-8 byte offsets into that projected event's canonical `redactedPayload` itself;
  omitting both normalizes to the complete payload span. The Store claim transaction creates and
  privately binds the exact projected source set; provider text never supplies an authoritative raw
  event ID, repository, or projection digest. Completion revalidates claim, boundary, ordered source identity/digest,
  and normalized `{eventId,startByte,endByte}` spans. Missing, duplicate, malformed, out-of-range,
  out-of-set, mixed-repository, or noncanonical citation order, and output above the active attempt
  manifest's `maxMemoryItemsPerDerivation`, MUST reject the whole output atomically. Lineage,
  source-anchor/tombstone coverage, and derived dedup MUST include normalized spans, not event IDs
  alone. Historical stored rows with NULL citation/span provenance remain readable but secret/unknown
  at disclosure boundaries; the citation child is mandatory only for new provider output attached to
  a durable PR3 raw-event claim. The no-claim legacy ingest path MUST NOT issue a provider request or
  create remotely eligible derived provenance.
- **FR-016**: One closed internal `DestinationBoundary` and eligibility function MUST govern every
  reachable content consumer: structured maintenance,
  search/recent/timeline/explain, `findByFile`/`findByConcept`, daemon get/search/pack, MCP direct and
  indexed reads, viewer raw-event/status/usage and content reads, pack traces, export/import, and
  dedup/supersession. It MUST carry compiler/runtime-derived provider peer trust of `verified`,
  `unverified`, or `not_applicable`; local HTTP is unverified and local HTTPS becomes verified only
  after exact peer verification. No user filter or caller-supplied model-location/trust claim may
  bypass it. Claude
  Code, Codex, and MCP are remote/unknown in Slice 1 production; a local CLI process is not an
  on-device-model attestation. The unused public extraction-replay and distill exports MUST be
  removed; any future public/runtime exposure MUST take the same boundary before reading raw or
  memory content or building a prompt.
- **FR-017**: Unknown legacy sensitivity MUST behave as `secret`; unknown or mismatched repository
  identity MUST deny private/local-only disclosure. Ineligible content MUST be rejected before
  rendering, preview, byte/token measurement, provider prompt construction, or export serialization.
  Export MUST gate memory items, user prompts, legacy session summaries, and session shells, omit
  cwd/Git remote/user/free-form session metadata, and version the changed payload; legacy v1 import
  content defaults to secret/unknown.
- **FR-018**: Provider, eligibility, retry, and injection logs/diagnostics MUST use only the closed
  EgressDiagnosticV1 action/reason vocabulary, numeric counts, safe fingerprints, state, and safe
  next-action code; they MUST contain no free-text event/memory content, titles, paths, prompts,
  queries, response excerpts, sentinels, credential values, or restricted previews.
- **FR-019**: Supported capture MUST remain non-blocking and fall back to the existing bounded atomic
  spool when direct daemon acknowledgement is unavailable.
- **FR-020**: Schema v21 MUST add all Slice 1 sensitivity/repository fields for memory items, user
  prompts, legacy session summaries, content-bearing artifacts, and session identity plus lineage, job, claim,
  admission/attempt provenance, resume, event payload digest/version, durable identity conflict, and
  diagnostic fields in one verified-backup transactional migration before issue #130 closes. It
  MUST drop the legacy source/stream/event unique index and enforce canonical repository/source/
  stream/event uniqueness with NULL repository mapped only inside the index to the closed
  non-authoritative `repo-v1:unknown` sentinel. A NULL-bucket collision MUST durably quarantine the
  incoming redacted payload/digest as secret with a stable non-success receipt, and MUST NOT drop it,
  ACK it normally, or canonically admit/process it; migrated and fresh DDL MUST match.
- **FR-021**: The existing raw-event flush batch MUST be the only summary job with states `queued`,
  `processing`, `failed`, `retry_exhausted`, and `completed`; claim generation MUST fence stale
  workers. Capacity MUST count every uncompleted state including `retry_exhausted`, never evict
  accepted work, and leave excess source events visibly not admitted.
- **FR-022**: Privacy skip and successful memory completion MUST each be a single transaction that
  validates the claim and source set, commits diagnostic or every memory/dedup/supersession effect,
  and completes the job. It advances the contiguous frontier once only when
  `frontier_already_advanced=false`; a recovered legacy range leaves it unchanged. Failure,
  overflow, stale claim, or partial parse MUST commit no memory and MUST NOT advance the frontier.
- **FR-023**: Automatic attempts MUST be bounded. `attempt_count` MUST increase only with a successful
  claim and remain monotonic for the job. New admission starts at 0 and `retry_limit=3` permits only
  automatic claims 1-3; a failed attempt 3 becomes retry-exhausted and any later claim requires one
  validated grant. Retry exhaustion stops timer retries; one validated,
  exact-job/component-targeted resume grant authorizes at most one claim and is consumed atomically.
  Global activation and health receipts MUST fan out in one sole-writer transaction to at most the
  capacity-25 currently matching retry-exhausted jobs; doctor retry MUST target exactly the displayed
  job. Daemon
  activation-receipt import, a persisted provider unhealthy-to-healthy edge, and an explicit
  user-confirmed doctor retry MUST be the only durable producers; each producer is sequenced,
  crash-idempotent, and emits at most one grant. While a grant is pending, another valid signal MUST
  return `grant_pending` without inserting/consuming its signal or producer receipt, advancing
  sequence, overwriting/queuing a grant, or authorizing an attempt; the producer may retry after the
  pending attempt terminates.
- **FR-024**: Admission manifest/provider fingerprints MUST be immutable. A changed validated
  configuration MAY create a new attempt manifest/provider/attempt fingerprint without rewriting
  admission provenance. Every signal MUST bind `targetJobId` and `producerReceiptId`; job/receipt and
  job/signal pairs MUST be unique. State, `resume_grant_state != pending`, job,
  role/provider/manifest, and
  `incoming.sequence > preLastConsumedResumeSequence` MUST pass one CAS before the post value becomes
  the incoming sequence or a grant exists. Gaps are allowed; equal/stale, duplicate, wrong-job,
  wrong-role, wrong-provider, or unchanged-config signals are durable no-ops. Legacy-unknown
  admission remains NULL/`legacy_unknown` and MUST NOT be fabricated from the current manifest.
- **FR-025**: Migration MUST NOT blindly rewind a legacy `gave_up` range. Exact complete retained
  ranges MAY become explicit recovery candidates without lowering the frontier; missing or
  ambiguous ranges MUST become terminal `legacy_unrecoverable` completed dispositions, remain
  inspectable, consume no processing capacity, and never be reported as successful recovery. A
  legacy completed range beginning exactly at `frontier + 1` MUST advance the frontier only after
  its exact retained range is complete and unambiguous; migration MUST continue through a contiguous
  completed chain, leave already-advanced frontiers unchanged, and roll back on a missing frontier
  or an incomplete, overlapping, or gapped stale-completed range.
- **FR-026**: Raw-event purge MUST be disabled with retention 0 for Slice 1. Any later retained policy
  MUST delete only at/below the committed frontier and exempt source ranges referenced by every
  uncompleted job; accepted not-yet-admitted backlog above the frontier is never purgeable.
- **FR-027**: Accepted event receipts MUST nudge the production sweeper. Periodic, idle, debounced,
  immediate, and request-time drains MUST share the same bounded flush path, and stop MUST prevent
  post-stop rescheduling while remaining restartable.
- **FR-028**: When semantic retrieval is disabled or unhealthy, lexical retrieval MUST remain
  available with `semantic_disabled` or another truthful reason, and no path may delete existing
  vector rows merely because semantic use is disabled.
- **FR-029**: Setup lifecycle automation MUST eventually start or attach one managed local runtime,
  verify ownership/version/fingerprint/doctor, and avoid duplicate writers across restart, stop, and
  uninstall; this automation MUST not precede the independently mergeable manifest activation.
- **FR-030**: With external egress disabled, the fixed runner MUST prove zero non-loopback socket
  attempts, zero pre-authorization attempts, and zero prohibited remote or unauthenticated-HTTP
  restricted bytes unconditionally. Base/repaired remote HTTPS MAY match expected request,
  environment-credential, and eligible-payload bytes. Verified local HTTPS is credential-none and
  MAY match only expected request plus eligible/private/local-only payload bytes; its credential
  bytes MUST be zero.
- **FR-031**: All emitted evidence MUST distinguish attempted processing from final delivery and MUST
  never report an interrupted, unpinned, inaccessible, or incomplete run as successful. The closed
  runner-evidence schema MUST represent the 12 raw resource windows, drain/checkpoint receipts,
  selected-item/token/concurrency samples, hostname/IP-valid CA fingerprint with no private-key
  artifact, raw setup/daemon-start TLS preflight receipts for base/local/repaired hosts with exact
  remote SNI or null IP SNI,
  timeout, timing, verified result, per-receipt trust-anchor and per-endpoint phase-stable
  peer-certificate fingerprints, and zero
  request/credential/payload bytes, with setup completion strictly before daemon-start beginning,
  while the network object and all six receipts bind the bundle invocation. The plateau object MUST
  bind candidate/artifact/environment/invocation and one fresh process-tree root unique from every
  observed provider root. Runner-owned provider-egress observations span
  candidate start through process-tree termination and opening only after direct durable-event-set
  authorization that records explicit canonical-order committed event IDs, count, and fingerprint
  without inferring a prefix. Source bytes by sensitivity MUST be runner-stub measurements of actual
  received request bytes against fixed synthetic markers/spans, never policy-derived or candidate-
  reported, and their sum MUST NOT exceed the observed payload bytes. Each receipt MUST also own
  restricted-payload byte and forbidden-sentinel counts, both zero and result-bound. Every fixed retry/redirect
  recovery subcase MUST own one sorted case/manifest-bound full
  observation under the same rules, including zero-egress observations for no-op cases, with receipt
  IDs and observed process-tree roots unique across the bundle. Every initial/recovery receipt MUST
  bind the bundle invocation and its owning process-tree root; the live runner MUST generate both
  identities freshly per execution/process generation and MUST NOT use a reusable PID alone. Each
  nested receipt's `observationCaseId` MUST equal its owning initial/recovery record case. Each plateau
  window MUST carry a unique workload receipt, strict runner-monotonic
  workload-start/workload-receipt/drain-receipt/checkpoint-receipt/sample timestamps, and no overlap
  with its neighboring windows. All 12 windows MUST carry the same positive duplicate-attempt count,
  no-op outcome, and zero memory/job deltas. The result schema carries
  separate trust/plateau fingerprints and derived aggregates, not raw copies. Canonical
  `unsupported`/`not_run` no-activity evidence MUST instead carry a null plateau object and null
  result plateau fingerprint; an executed result MUST carry the complete plateau. The runner-evidence
  suite MUST carry pair-bound repeated same-event-ID/different-digest conflict evidence and exactly
  16 positive observations plus one late-injection-negative observation.

### Key Entities

- **Provider Proposal / Provider Choice**: The closed explicit summary transport input and its
  compiler-derived, computed-fingerprint effective form.
- **Effective Capability Manifest**: One immutable non-secret generation containing the fixed
  resource profile, one summary choice, disabled embedding lane, destination map, legacy
  dispositions, and computed manifest identity.
- **Repository Identity**: A domain-separated canonical digest derived from a verified Git remote or
  realpathed primary Git anchor; labels are not authority.
- **Captured Event**: Accepted Agent activity with ordering, sensitivity, repository identity,
  manifest admission identity, redacted content or content-free quarantine, and delivery lifecycle.
- **Memory Processing Job**: The existing raw-event flush batch deepened with immutable admission
  provenance, current attempt provenance, bounded retry/resume state, claim generation, and atomic
  commit frontier.
- **Memory Item**: A durable fact with semantic kind, source evidence, lineage/revision,
  manifest/provider/attempt provenance, repository identity, and inherited sensitivity.
- **Destination Boundary**: The closed trusted destination and repository context required before
  any content consumer may select or materialize a record.
- **Injection Pack**: A bounded set of eligible memory items with provenance, selection/omission
  reasons, degradation state, and final delivery evidence.
- **Egress Diagnostic**: A bounded content-free action/reason/count record; wire bytes are measured by
  the runner, not self-asserted by the diagnostic.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The fixed Claude-to-Codex and Codex-to-Claude scenarios each deliver all required facts,
  zero forbidden facts, no failed approach as a recommendation, and zero manual handoff steps.
- **SC-002**: Across the failure matrix, Agent blockage, accepted-event loss, duplicate active
  memory, stale-claim commit, frontier advance on failure, fabricated success, identity-conflict
  overwrite/ACK, and orphan process counts are all zero; the induced same-ID/different-digest case
  creates exactly one reusable durable conflict receipt for the repeated pair, while another
  conflicting digest cannot reuse it.
- **SC-003**: Capture adds less than 200 ms at p95 under the fixed workload, measured over ordinals
  3-22 after discarding 1-2 and using nearest-rank p95.
- **SC-004**: Warm retrieval/injection is below one second at p95 and cold semantic-disabled fallback
  completes or reports lexical fallback below three seconds, each over its own ordinals 3-22;
  equality with a threshold fails.
- **SC-005**: Every all-restricted remote case across provider, maintenance, retrieval, MCP, viewer,
  trace, and export surfaces produces zero restricted content bytes; provider all-restricted cases
  also produce exactly zero requests. Mixed provider cases transmit only the expected eligible bytes.
- **SC-006**: Verified loopback-HTTPS processing retains sensitivity and repository identity in every
  derived item; remote, unknown, basename-collision, cross-repository, and unknown-repository
  disclosure delivers zero restricted items and bytes.
- **SC-007**: Replaying unchanged accepted events leaves active memory, semantic entry, lineage, and
  final pack counts unchanged; a stronger same-payload replay atomically strengthens canonical and
  derived sensitivity, quarantine is absorbing, and dedup/supersession never lowers sensitivity or
  crosses repository identity.
- **SC-008**: Setup disclosure, stored generation, daemon snapshot, Observer, maintenance, viewer,
  status, and doctor report one identical manifest/provider identity; provider/resource legacy env
  mutation after daemon start changes none of their effective behavior.
- **SC-009**: The fixed resource profile reports periodic 30 s, idle 120 s, debounce 1 s, stuck claim
  5 min, new-admission source job limit 100, retention disabled/0, queue 25, retry limit 3, and all
  accepted fixture envelope values exactly. The runner executes 12 identical duplicate/no-op windows,
  discards 1-2,
  requires strict non-overlapping workload-start/workload-receipt/drain-receipt/checkpoint-receipt/
  sample order, and requires windows 3-12 to stay inside all absolute ceilings. In windows 8-12, process count is
  constant, drained queue depth is zero, selected item/token counts are identical, RSS span is at
  most 16 MiB, storage span is at most 65,536 bytes, processing concurrency is at most 2, and the
  post-teardown orphan count is zero.
- **SC-010**: Retry-exhausted jobs count against capacity, one valid grant creates exactly one new
  attempt fingerprint, each signal/grant is bound to one job+producer receipt, and invalid or
  cross-job signals create zero claims; admission provenance never changes.
- **SC-011**: Every injected item has visible eligible source and selection reason, while every
  restricted omission, retry diagnostic, log, and failure record contains the expected bounded code
  and no restricted sentinel or content-derived excerpt.

## Assumptions

- The Product Reset purpose and fixed Slice 1 scenarios remain authoritative. The pre-PR 0
  `specs/005-product-reset/` manifest fixture, schema, semantic validators, and bound examples did
  not encode the buildable closed provider/resource shape above; they required the contract-first,
  mechanical correction and fingerprint recomputation co-delivered with these planning artifacts.
- Users explicitly choose the summary wire protocol, model, complete endpoint URL, and credential
  reference. Subscriptions, arbitrary provider registries, custom headers, and unrelated Agent or
  OpenCode credentials are not discovered automatically.
- The deterministic summary stub remains runner-owned harness metadata and materializes a normal
  ProviderProposalV1; it is not a production ProviderChoice kind.
- The profile is `slice1-short-run` presented as `minimal`; its only closed successor is the existing
  test-only output-limit recovery version 2 described above. Base remote, local derivation, and
  repaired-remote manifests retain version 1/max16. Embedding is explicitly disabled and lexical
  retrieval is the healthy required lane.
- Merged #126 and #129 remain prerequisite regression cases.
- PR 0 is delivered from a fresh worktree created from refreshed `origin/main` after merged PR #142;
  later runtime branches start only from the merged PR 0 commit.
- Past remote disclosure cannot be undone. Migration may conservatively quarantine or diagnose old
  rows without copying their content into logs or evidence.
- Remote MCP memory-body delivery, Cloud sync, additional profiles/protocols, semantic quality
  redesign, macOS, native Windows, additional Agents, Rust, and release publication remain outside
  this slice.
