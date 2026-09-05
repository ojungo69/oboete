# Data Model: Lightweight Automatic Memory Product Reset

This is the product-level model shared by the later focused implementation slices. M0 introduces
no persisted runtime schema.

## AgentSession

Represents one observed Claude Code or Codex work period.

- `sessionId`: stable local identity
- `agent`: Claude Code or Codex
- `repositoryScope`: authenticated local repository identity
- `startedAt`, `lastActivityAt`, `endedAt`: lifecycle timestamps
- `captureState`: active, pending, complete, or degraded
- `effectiveManifestId`: configuration used for this session

Relationships: owns captured events; may produce summaries and durable memory items; may consume
multiple injection packs.

## CapturedEvent

An ordered, idempotent record of supported Agent activity.

- `eventId`: stable delivery identity
- `sessionId`: owning session
- `agentSequence`: source order when available
- `kind`: `prompt`, `tool_activity`, `tool_result`, `assistant_message`, or `session_boundary`
- `repositoryScope`: scope resolved at capture
- `sensitivity`: `eligible`, `local_only`, `private`, or `secret`
- `redactedPayload`: canonical content retained for asynchronous processing after reserved, private,
  and secret spans are removed
- `payloadDigest`: `SHA-256(UTF-8("free-mem:event-payload-digest:v1\0") ||
  JCS(redactedPayload))`, without exposing its content
- `payloadDigestVersion`: immutable `event-payload-digest-v1` for the initial Alpha
- `deliveryState`: accepted, spooled, processing, committed, failed, retry-exhausted, or quarantined
- `failureReason`: bounded machine-readable reason when not committed
- `attemptCount`, `retryBudgetRemaining`: persisted retry accounting
- `lastResumeSignal`: `validated_configuration_activation`,
  `recorded_provider_healthy_transition`, or `user_confirmed_doctor_retry`, plus stable signal
  identity, producer receipt identity, exact target job/component, and computed provider/manifest
  fingerprints; absent before the first resume
- `lastConsumedResumeSequence`: monotonic per-component sequence persisted outside bounded history
- `transitionHistory`: bounded ordered entries containing from/to state, timestamp, reason, retry
  budget before/after, and resume signal

Uniqueness: canonical `(repositoryScope, source, streamId, eventId)` is permanently bound to the
first accepted `payloadDigest`. Replay with the same digest is idempotent and produces at most one committed
effect. Before acknowledging a same-digest replay, one Store transaction joins sensitivity using
`secret > private > local_only > eligible`; quarantine is absorbing. It applies the stronger value
to the canonical event and every existing record derived from that event. A quarantine escalation
also makes the canonical source unavailable to every consumer and returns a non-success quarantine
receipt. Downgrade replays are no-ops. Payload bytes and the payload digest remain unchanged.

A different digest for that identity creates or reuses a durable `EventIdentityConflict`,
returns its explicit conflict receipt, and quarantines only the incoming delivery with
`event_identity_payload_conflict`; it is never a normal ACK or silent discard. The canonical event,
payload, and security disposition remain unchanged. Digest comparison occurs after deterministic redaction and
canonical encoding; the conflicting raw payload is never persisted. Retrying the same conflicting
digest returns the same non-success receipt. A different conflicting digest creates a different
receipt. The conflict record and terminal incoming-delivery
quarantine commit atomically before that receipt is returned. A sender converges by replaying the
canonical digest or uses a new event identity for corrected content; neither doctor nor a retry
replaces canonical bytes.

Spool and retry records carry `redactedPayload`, `payloadDigest`, and `payloadDigestVersion` from
first acceptance. They never recompute an old event with the current algorithm. When a same-ID
delivery arrives under a newer algorithm, conflict comparison canonicalizes it with the stored
event version first; a version change alone cannot produce a conflict. A real conflict record stores
the canonical version and both digests computed under that version.

State transitions:

```text
accepted -> processing -> committed
accepted -> spooled -> processing -> committed
processing -> failed -> processing
failed -> retry-exhausted
retry-exhausted -> processing
accepted|spooled|processing -> quarantined
```

`retryBudgetRemaining` counts automatic attempts that may still be started and is decremented
atomically before each attempt. `retry-exhausted` never resumes on a timer alone. A valid signal
creates one one-shot grant consumed by one claim; configuration activation does not refill the
automatic limit. Grant creation and `lastConsumedResumeSequence` advance in one compare-and-swap;
a duplicate or out-of-order signal is a no-op even after bounded history rotates. CapturedEvent
accepts only signals targeted to its delivery/storage component; provider-only signals are no-ops.
`attemptCount` remains monotonic. Every transition records the signal identity, target, reason, and
retry budget before and after the transition. Retry fields survive every delivery-state transition
and are visible to doctor.

If redaction cannot produce a safe `redactedPayload`, the event is quarantined without persisting
the raw payload. Local-only content may remain only in the redacted payload and remains governed by
its sensitivity boundary.

CapturedEvent retry state applies only to event delivery/commit. Summary and embedding provider
retries use `MemoryProcessingJob`; a committed event remains committed while a derived job is
retry-exhausted.

Captured event kind is transport provenance and a processing trigger, not a semantic classifier.
Summary processing derives any valid MemoryItem kind from the aggregate redacted content when the
fact is supported by source evidence. A decision, failed approach, or next action is not discarded
merely because it appeared in a different transport kind.

## EventIdentityConflict

A durable, payload-free record proving that one scoped event identity arrived with different
post-redaction content.

- `conflictId`: domain-separated deterministic receipt identity for canonical event identity,
  digest version, canonical digest, and conflicting digest
- `repositoryScope`, `source`, `streamId`, `eventId`, `payloadDigestVersion`, `canonicalPayloadDigest`,
  `conflictingPayloadDigest`
- `state`: quarantined
- `reason`: `event_identity_payload_conflict`
- `firstSeenAt`, `lastSeenAt`, `occurrenceCount`

It stores no raw or redacted payload. The database uniquely indexes the canonical identity plus the
ordered canonical/conflicting digest pair. Repeated delivery of that exact pair reuses one record;
a second conflicting digest creates a distinct receipt. The record remains visible after the
canonical event commits.

## MemoryProcessingJob

Represents asynchronous summary or embedding work over already committed events/memories.

- `jobId`, `role`: summary or embedding
- `sourceEventIds`, `sourceMemoryIds`
- `state`: queued, processing, completed, failed, retry-exhausted, or quarantined
- `attemptCount`, `retryBudgetRemaining`, `lastFailureReason`
- `lastResumeSignal`, `lastConsumedResumeSequence`, and bounded `transitionHistory` using the same
  resume mechanics as CapturedEvent
- admission and current-attempt provider/manifest fingerprints

A provider failure changes the job state, never the committed source event. `retry-exhausted` uses
the same durable one-time resume and grant mechanics as CapturedEvent, but a signal applies only
when its exact target job, role, and computed provider/manifest fingerprints match the failed job.
Provider-health and doctor
signals target the active provider/manifest fingerprints. A `validated_configuration_activation`
requires newly active, validated provider/manifest fingerprints and atomically rebinds only the
current attempt; immutable admission provenance remains. Unrelated activations and health
transitions are no-ops. Timer-only resume is prohibited.

Setup activation and provider-health producer receipts are global events, but the sole writer fans
each out to all matching `retry-exhausted` jobs that exist in that transaction. The global
uncompleted-job capacity is 25, so that complete set is necessarily at most 25 and needs no
out-of-transaction continuation. Each
per-job signal includes `targetJobId` and `producerReceiptId`; `(jobId, producerReceiptId)` and
`(jobId, signalId)` are unique. Doctor confirmation targets exactly the displayed job. State,
role/provider/manifest, and `sequence > lastConsumedResumeSequence` are compared atomically;
accepted signals alone advance the sequence and create one pending grant.

A summary result containing more items than the active ResourceProfile's
`maxMemoryItemsPerDerivation` enters `retry-exhausted` atomically with
`memory_output_limit_exceeded` and zero remaining budget. No partial derived batch is committed;
source events and previously committed sibling lineages remain intact. Only activation of a changed,
validated profile with a larger limit or a repaired provider may rebind and requeue the job; health
and doctor signals under the unchanged limit are no-ops. Its payload-free failure metadata is
limited to error code, `jobId`, source event IDs, observed result count, and configured limit; raw
provider output, copied source content, and uncommitted derived items are forbidden.

Redirect rejection immediately records `provider_redirect_rejected` and leaves the job
`retry-exhausted` without following or retaining authority from the old `Location`. Only activation
of the complete repaired-remote successor's changed, validated provider/manifest fingerprints can
requeue it; a health transition under the unchanged redirecting configuration is a no-op.

## MemoryItem

A durable reusable output derived from one or more captured events.

- `memoryId`: stable local identity of this stored item
- `lineageId`: deterministic identity shared by every revision of one derived fact, computed from
  a versioned domain separator, repository scope, and a canonical model-independent source-fact
  anchor. The anchor contains only the minimal supporting evidence set as sorted unique event IDs
  and exact source byte spans. Fact wording, sibling-set membership, processing-batch membership,
  model output order, profile, model generation, and memory kind are forbidden as lineage inputs
- `revisionId`: stable identity of this content revision
- `revisionOrdinal`: monotonically increasing ordinal within one memory lineage
- `supersedesMemoryId`: prior MemoryItem replaced by this revision, when applicable
- `derivationKey`: deterministic identity derived from `lineageId`, processing profile, and summary
  model generation
- `kind`: `summary`, `decision`, `discovery`, `change`, `failed_approach`, or `next_action`
- `title`, `body`: human-inspectable content
- `sourceSessionIds`, `sourceEventIds`: provenance
- `repositoryScope`: retrieval boundary
- `sensitivity`: egress and injection boundary
- `createdAt`, `supersededAt`, `deletedAt`: lifecycle
- `processingProfile`: effective summary configuration
- `lexicalState`, `semanticState`: ready, pending, degraded, stale, or unavailable
- `semanticGenerationId`: generation that owns the active vector for this item, when present

Derived sensitivity is the most restrictive contributing source disposition in this order:
`secret > private > local_only > eligible`. Secret-bearing output is prohibited after redaction;
`private` and `local_only` never downgrade during summarization, revision, indexing, or retrieval.
Both dispositions are eligible only for same-repository on-device processing and InjectionPack
destinations; neither may reach a remote/off-host provider or renderer, even after private spans are
removed.

Deletion is terminal for retrieval and injection. It records a permanent durable tombstone for the
`lineageId`; reprocessing retained sources under any profile or model generation cannot create an
active revision for that lineage, including when a later model reclassifies the fact's `kind`.
Sibling facts with different source-fact anchors are unaffected. Superseded items remain auditable
but are ineligible for normal selection.

The persisted SourceFactAnchor registry resolves reprocessing before lineage creation. An exact
span match reuses its anchor. For the same source events, a proposed span that overlaps, contains,
or is contained by a deleted anchor is suppressed by that tombstone even when its boundaries or
semantic kind differ. Ambiguous overlap with a non-deleted anchor is quarantined for inspection;
only disjoint minimal spans may establish a new sibling anchor automatically. This conservative
coverage prevents boundary drift from bypassing deletion.

Retrying the same source events under the same processing profile and model generation reuses the
same derivation key and converges on one MemoryItem per lineage. Reprocessing under a different
profile or model-generation key may create a new revision only in the same lineage and by recording
the prior item as superseded; it never silently mutates the old derived content or bypasses a
lineage tombstone.

Each derived fact must cite a distinct minimal source span. Multiple outputs claiming the same
anchor, or an output without a stable span, quarantine the provider result rather than creating an
ordinal-based lineage.
The source-fact anchor and `lineageId` are established before semantic-kind classification.

### Lineage v1 canonicalization

`lineageId` is:

```text
SHA-256(
  UTF-8("free-mem:memory-lineage:v1\0") ||
  JCS({ repositoryScope, sourceSpans })
)
```

- `repositoryScope` is the authenticated canonical repository identity.
- `sourceSpans` is deduplicated and sorted lexicographically by `eventId`, then numerically by
  `startByte`, then `endByte`.
- Span coordinates are half-open `[startByte, endByte)` offsets into the canonical
  `redactedPayload` UTF-8 bytes.
- `startByte` and `endByte` MUST fall on UTF-8 scalar boundaries; a span that starts or ends on a
  continuation byte is invalid and quarantines the provider result.
- Profile, model generation, processing batch, fact wording, and semantic kind are absent from the
  digest input.

Fixed vectors:

| Scope and normalized spans | Expected SHA-256 |
|---|---|
| `repo-primary`; `event-a:[0,10)` | `ca6ec88cc0156199c5e08e50393dcf4ef473f62c1e0e5372e99d9d791499779f` |
| same anchor with only profile/model/kind changed | `ca6ec88cc0156199c5e08e50393dcf4ef473f62c1e0e5372e99d9d791499779f` |
| `repo-primary`; `event-a:[0,11)` | `ad0ffa99555520b5a1524908a4d08661b3054b0f0f8a1a66e08a8f2a98904378` |
| `repo-primary`; unsorted input normalized to `event-a:[0,10)`, `event-b:[4,9)` | `23274e7fbe3af129f9942e312eec51dfdc6b6825e3e38583d068254e96e2d447` |

An algorithm change creates a new versioned lineage namespace. Migration preserves all old
`(version, lineageId)` tombstones and may add an alias only after verifying the same canonical source
spans; it never silently recomputes or drops a deletion tombstone.

## ResourceProfile

Slice 1 has one closed production operating envelope independent of provider choice:

- `profileId=slice1-short-run`, version 1
- capture/processing concurrency 2, queue capacity 25, retry limit 3
- `maxMemoryItemsPerDerivation=16`, `maxSourceEventsPerJob=100`
- observer request timeout 60,000 ms, maximum input 12,000 JavaScript UTF-16 units, output 4,000 tokens, response
  1,048,576 bytes, temperature 0.2, and provider TLS preflight timeout 5,000 ms
- worker warm lifetime and periodic sweep 30,000 ms, idle flush 120,000 ms, event debounce 1,000 ms,
  and stuck claim timeout 300,000 ms
- raw-event retention disabled with 0 ms
- the fixed InjectionPack envelope and resource warning thresholds

The only successor is the complete runner-owned output-limit fault manifest: version 2 and limit 17,
with every other profile field unchanged. It is not selectable by production setup.

## ProviderChoice

`ProviderProposalV1` is a closed summary input containing only version/role/enabled state, model
identity, one `wireProtocol` (`anthropic_messages_v1 | openai_chat_completions_v1`), one complete
canonical `endpointUrl`, and `credentialRef: {kind:none} | {kind:environment,name}`. Provider
names/kinds, split URLs, appended paths, inline/free-form credentials, arbitrary headers, and
self-declared policy/fingerprints are invalid.

The compiler returns `ProviderChoiceV1` by adding only:

- computed `providerFingerprint = SHA-256("free-mem:provider-choice:v1\0" || JCS(choice without
  providerFingerprint))`
- derived `executionLocation: local | remote`
- derived `egressPolicy: on_device | explicit_remote`
- derived `costClass: local_zero | external_metered`
- derived `tlsPolicy: not_applicable | system`
- literal `redirectPolicy: reject`

Only literal `127.0.0.1` and `[::1]` are local; `localhost`, localhost subdomains, trailing-dot
hostnames, wildcard/unspecified addresses (including IPv4-mapped unspecified), and alternate
loopback spellings (including IPv4-mapped loopback) are rejected. Local HTTP is credential-none and
eligible-only with TLS not applicable. Local HTTPS uses verified system chain/hostname identity and
may receive private/local-only content only for the exact repository. Every other host is remote
HTTPS with system chain and hostname validation. URL
userinfo, query, fragment, noncanonical spelling, and a missing/root-only request path are rejected.
Secret values are never stored or fingerprinted; only the named environment variable reference is
retained.

Anthropic Messages uses JSON content type, fixed `anthropic-version: 2023-06-01`, optional
environment-backed `x-api-key`, request
`{model,max_tokens,temperature,system,messages:[{role:"user",content}]}`, and concatenated text
blocks from `content[]`. OpenAI Chat Completions uses JSON content type, optional environment-backed
`authorization: Bearer`, request
`{model,max_tokens,temperature,messages:[{role:"system",content},{role:"user",content}]}`, and
`choices[0].message.content`. Credential `none` sends no authentication header. Responses are
bounded before JSON parsing; streaming, Responses API, tools, custom headers, tier routing, and
fallback are unsupported.

Input length is JavaScript UTF-16 code units. The user has a 3,000-unit floor: slice system from the
start to 9,000 units and call `toWellFormed()`, then slice user from the start to
`max(3,000, 12,000 - clippedSystem.length)` units and call `toWellFormed()`. Both protocols use this
allocation without a tail merge or token-based alternative. Setup after confirmation performs a
native credential/payload-free TLS chain+hostname handshake to the exact host/port/SNI within
5,000 ms; failure mutates nothing or restores prior state. Daemon start performs the same preflight;
failure preserves writer/RPC/capture/spool-import/lexical services and disables only provider/AI
processing as `provider_unavailable` or `provider_tls_rejected`. Production rejects
added CA path/environment input and equivalent trust overrides and uses only platform system trust;
the isolated runner may install its public test CA into private system trust before candidate start.
Local HTTP skips the handshake and remains credential-none/eligible-only.

## SemanticIndexGeneration

The authoritative provenance and lifecycle for one compatible vector space.

- `generationId`
- embedding provider, model identity, model revision, dimensions, and preprocessing profile
- immutable build-set boundary and catch-up watermark
- per-item pending, ready, failed, and deleted ledger state
- generation state: building, validating, active, stale, failed, or retired

A MemoryItem vector is ready only when its `semanticGenerationId` equals the active compatible
generation. Provider, model, revision, dimension, or preprocessing changes create a new generation;
vectors from an older generation never become ready in the new vector space by implication.

## EffectiveCapabilityManifest

The single validated result consumed by setup, runtime, and doctor.

- `manifestVersion`, `manifestId`, optional predecessor `baseConfigurationFingerprint`
- exact `ResourceProfileV1`, compiled summary `ProviderChoiceV1`, and explicit disabled embedding
  lane
- the closed destination-policy map
- up to 64 sorted unique legacy `{key, disposition}` records; dispositions are only `translated`,
  `ignored`, or `overridden`, and contain no values
- computed non-secret `configurationFingerprint`

The configuration fingerprint is SHA-256 over domain
`free-mem:effective-capability-manifest:v1\0` plus JCS of the complete manifest after provider
fingerprinting and excluding only `configurationFingerprint`. A legacy conflict rejects compilation
and cannot appear in an active manifest.

The fixed fixture has one base remote manifest, one complete local successor, one complete repaired
remote successor, and the single complete test-only output-limit successor. Every successor binds
the base fingerprint. Recovery signals carry computed provider/manifest fingerprints, not
free-form summary configuration labels.

The local destination-map entries exist only for runner-owned loopback-consumer evidence. Claude
Code, Codex, and MCP resolve remote/unknown in Slice 1 production; local process location or caller
claims cannot select a local entry.

Activation is atomic: proposed -> validated -> active. Invalid proposals never replace the active
manifest.

## RetrievalCandidate

A normalized candidate offered to the context compiler.

- `memoryId`, `lineageId`, `revisionId`, `revisionOrdinal`
- `sourceLane`: `exact_session`, `lexical`, `semantic`, or `recency`
- normalized relevance score and stable tie-break fields
- provisional estimated bytes and tokens used only for candidate selection
- scope, sensitivity, and lifecycle eligibility
- semantic-index state when applicable

`RetrievalCandidate` revision fields copy the authoritative MemoryItem revision fields. Pack
selection keeps only the active revision of each lineage, deduplicates repeated candidates for that
revision, and uses `revisionOrdinal` only for stable ordering. `derivationKey` controls idempotent
creation and is never substituted for revision ordering.

## InjectionPack

The bounded, versioned product output rendered for Claude Code or Codex.

- `packVersion`, `packId`
- target Agent/model destination class, resolved destination policy, session, repository scope, and
  manifest identity
- ordered selected memories and rendered sections, each bound to its MemoryItem `revisionId`
- exact final-rendered bytes, destination-token count, input-candidate count, traced-candidate count,
  deadline-unprocessed count, admitted-candidate count, selected-item count, and elapsed selection
  time
- selection trace binding each candidate to its provenance, destination-policy decision,
  `sourceLane`, and exactly one terminal reason from the InjectionPack contract enumeration
- degraded capabilities and fallback reasons

The same normalized pack must render equivalent facts for both Agents even when their hook output
formats differ.

## RunnerEvidenceBundle

A bounded comparison artifact written by the reference runner in an immutable root that the
candidate cannot access. One bundle binds fixture, candidate, environment, artifact, and invocation
identity to exactly 16 positive scenario observations plus the required late-injection negative case
in suite mode.

- runner-owned latency interval endpoints and full observed lifecycle milestones
- runner-owned process, RSS, queue, and storage samples
- one closed network-trust record binding base/local/repaired host identity, public CA SHA-256,
  enabled chain/hostname validation, `privateKeyCommitted=false`, and exactly six unique raw TLS
  preflight receipts for base/local/repaired by setup activation/daemon start; every receipt binds
  host, remote SNI or null IP SNI, exact endpoint port, 5,000 ms, the per-run public CA trust anchor,
  and one peer-certificate SHA-256 per endpoint that is identical across its setup/start receipts and
  distinct from the CA,
  verified monotonic duration, setup completion strictly before daemon-start beginning, and zero
  credential/payload/request activity; the network object and all six receipts repeat the bundle
  runner invocation
- one runner-owned provider-egress observation per real scenario, armed before candidate start and
  retained through process-tree termination, with zero pre-authorization/non-loopback attempts,
  direct durable-store authorization containing explicit canonical-order committed event IDs, their
  count and set fingerprint, earliest request interval, provider/location identity,
  exact wire aggregates, and runner-stub-measured source-content bytes split by sensitivity using
  fixed synthetic markers/spans, never policy-derived or candidate-reported; the four sensitivity
  buckets sum to no more than the observed payload bytes; runner-owned restricted-payload bytes and
  forbidden-sentinel count are both zero and result-bound; the negative case only projects an
  observed base receipt
- one additional full runner-owned provider-egress observation for every fixed retry/redirect
  recovery subcase, including zero-egress no-op cases, sorted and bound to exact case/manifest/
  provider plus bundle invocation and a fresh subcase process-tree root, with receipt and observed
  process-tree IDs unique across the whole bundle; the nested receipt repeats its owning case ID
- for every executed result, one closed short-run plateau record with 12 ordered duplicate/no-op windows, discarded 1-2,
  measured 3-12, final 8-12, one identical positive duplicate-delivery count, fixed item/token counts, bounded
  process/RSS/queue/storage/concurrency, measured RSS/storage maximum increase from the first measured
  window, final-five RSS/storage spans, unique path-free drain/checkpoint/workload receipt IDs, strict
  monotonic workload-start/workload-receipt/drain/checkpoint/sample order and window non-overlap,
  exact `duplicate_noop`, zero durable/job deltas, and zero orphan processes; it binds
  candidate/artifact/environment/invocation plus one fresh plateau
  process-tree root unique from every observed provider root; canonical `unsupported`/`not_run`
  no-activity evidence carries a null plateau object and null result plateau fingerprint instead
- runner-owned effective Agent/repository/session identity and caller-claim authorization decisions
- a runner-derived fingerprint over the complete result observation, binding egress, render,
  atomicity, and conflict evidence without copying private payload into the bundle
- bundle-global unique per-run preparation receipts with path-free ASCII opaque data-directory and
  process-generation identities that cold runs never share with warm runs, observed after the prior
  run and within one pinned process-sample interval before the current run
- cold-reset observations proving zero prior product processes and an empty data directory
- first cold-run measurement observed within one pinned process-sample interval after run start
- warm observations proving one retained ready data directory and process generation

The result record carries the bundle fingerprint plus a domain-separated network-trust fingerprint
and, for executed work, a resource-plateau fingerprint alongside its existing derived resource
aggregates. The raw trust and any plateau object live only in the runner bundle; the validator
recomputes every present fingerprint and matches it to the result before it can affect eligibility.
Canonical no-activity evidence uses null plateau object/fingerprint. The bundle contains
no absolute path, private payload, certificate private key, or secret value.

## OperationalStatus

A secret-free snapshot used by doctor and inspection surfaces.

- component: capture, spool, summary, lexical, semantic, injection, backup, or provider
- state: healthy, degraded, failed, disabled, or pending
- reason, since, pending count, last success
- safe user action
- effective manifest identity

Healthy status requires a relevant end-to-end probe; process existence alone is insufficient.
