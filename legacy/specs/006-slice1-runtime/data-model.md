# Data Model: Slice 1 Automatic Memory Runtime

## Canonical enums

### SensitivityV1

`eligible | local_only | private | secret`

Restriction is monotonic: `secret > private > local_only > eligible`. Missing, malformed,
ambiguous, or legacy-unknown sensitivity is `secret`. A payload/provider claim cannot weaken the
first-class value.

### Provider execution

- `wireProtocol`: `anthropic_messages_v1 | openai_chat_completions_v1`
- `executionLocation`: `local | remote`
- `egressPolicy`: `on_device | explicit_remote`
- `costClass`: `local_zero | external_metered`
- `tlsPolicy`: `system | not_applicable`
- `redirectPolicy`: literal `reject`

There is no unknown active provider state. An unresolved retrieval destination may be `unknown`, but
an unknown ProviderChoice is invalid and cannot activate.

## ProviderProposalV1

The only user/harness input accepted by the Slice 1 compiler. It is closed and contains no derived
policy fields.

| Field | Rule |
|---|---|
| `version` | Literal `1` |
| `role` | Literal `summary` |
| `state` | Literal `enabled` |
| `wireProtocol` | One of the two canonical protocols |
| `modelId` | 1-256 UTF-8 bytes, no ASCII control/NUL |
| `modelRevision` | 1-128 UTF-8 bytes, no ASCII control/NUL |
| `endpointUrl` | 1-2,048 ASCII bytes; complete canonical request URL; no runtime path suffix |
| `credentialRef` | One closed `CredentialRefV1` |

Unknown fields, provider registry names, provider kinds, arbitrary headers, inline credentials,
cookies, filesystem credential paths, self-declared location/policy/cost/TLS/redirect values, and
self-declared fingerprints are rejected.

### CredentialRefV1

```text
{ kind: "none" }
{ kind: "environment", name: <^[A-Za-z_][A-Za-z0-9_]{0,127}$> }
```

Only the named environment variable may be read for request authentication. Its value is never
stored, fingerprinted, logged, displayed, or inherited from an Agent/OpenCode subscription.

### Canonical endpoint rules

- Parse with the platform URL implementation and require the serialized URL to equal the proposal
  after the documented canonicalization pass.
- Require `http:` or `https:`, a non-empty hostname, a non-root API path, and no username, password,
  fragment, or query. The path is the complete protocol endpoint.
- `127.0.0.1` and URL hostname `[::1]` are the only local host literals. `localhost`, localhost
  subdomains, trailing-dot hostnames, other loopback spellings, and wildcard/unspecified addresses
  (including IPv4-mapped unspecified) are rejected.
  Classification never resolves DNS: every other syntactically valid hostname, including the fixed
  runner `summary.stub.invalid`, is remote.
- A local literal may use HTTP (`tlsPolicy=not_applicable`) only with credential `none` and
  eligible-only projection, or HTTPS (`tlsPolicy=system`). Every
  non-loopback host is remote and requires HTTPS with `tlsPolicy=system`.
- For remote or local HTTPS, `NODE_TLS_REJECT_UNAUTHORIZED=0`, an additional CA path/environment
  value, or an equivalent trust override rejects production setup activation and daemon provider
  start. Production uses platform system trust only. The isolated runner provisions its public test
  CA into private system trust before candidate start, outside proposal/manifest/candidate control.
- Setup after confirmation and daemon start each perform a native credential/payload-free TLS
  chain+hostname handshake to the exact host/port/SNI within
  `providerTlsPreflightTimeoutMs=5,000`. Setup failure mutates nothing or restores prior state. A
  daemon-start failure preserves writer/RPC/capture/spool-import/lexical startup and disables only
  provider/AI processing as `provider_unavailable` or `provider_tls_rejected`; local HTTP skips the
  handshake and never receives credential, private, or local-only bytes.
- `redirectPolicy=reject` is compiler-derived and request code uses manual redirect handling; no 3xx
  `Location` is followed or replayed.

## ProviderChoiceV1

The compiler output is the proposal plus only derived fields:

| Field | Rule |
|---|---|
| proposal fields | Preserved exactly after canonical validation |
| `providerFingerprint` | Computed SHA-256 fingerprint below |
| `executionLocation` | Local only for literal `127.0.0.1`/`[::1]`, otherwise remote |
| `egressPolicy` | `on_device` for local, `explicit_remote` for remote |
| `costClass` | `local_zero` for local, `external_metered` for remote |
| `tlsPolicy` | `system` for HTTPS, `not_applicable` only for local HTTP |
| `redirectPolicy` | Literal `reject` |

Fingerprint input is the full ProviderChoice without `providerFingerprint`, encoded as JCS and
prefixed by `free-mem:provider-choice:v1\0`. The stored value is
`sha256:<64 lowercase hexadecimal characters>` and must recompute exactly.

The deterministic test stub is not a ProviderChoice kind. Harness metadata starts the stub and
materializes an ordinary ProviderProposalV1 with a complete endpoint URL and one supported wire
protocol.

### Frozen protocol behavior

`ResourceProfileV1` fingerprints request timeout 60,000 ms, input 12,000 characters, output 4,000
tokens, response 1,048,576 bytes, and temperature 0.2. Both protocols send one system prompt and one
user prompt and reject an oversized response before JSON parse.

- `anthropic_messages_v1`: `content-type: application/json`, fixed
  `anthropic-version: 2023-06-01`, and `x-api-key` only for an environment credential; request
  `{model,max_tokens,temperature,system,messages:[{role:"user",content}]}`; response text is the
  ordered concatenation of `content[]` text blocks.
- `openai_chat_completions_v1`: `content-type: application/json` and `authorization: Bearer` only for
  an environment credential; request
  `{model,max_tokens,temperature,messages:[{role:"system",content},{role:"user",content}]}`;
  response text is `choices[0].message.content`.

`credentialRef.kind=none` emits no authentication header. Redirects, streaming, Responses API,
tier routing, arbitrary headers, tool calls, and provider fallback are outside Slice 1. Every request
uses `AbortSignal.timeout(observerRequestTimeoutMs)`.

Input length is JavaScript UTF-16 code units. Reserve 3,000 of 12,000 units for user content: clip
system to 9,000 and call `toWellFormed()`, then clip user from the start to
`max(3,000, 12,000 - clippedSystem.length)` and call `toWellFormed()`. This allocation is identical
for both protocols.

### Closed Slice 1 fixture choices

- Base remote: `openai_chat_completions_v1`,
  `https://summary.stub.invalid/v1/chat/completions`,
  `{kind:"environment",name:"FREE_MEM_SUMMARY_API_KEY"}`; compiler derives remote,
  `external_metered`, `explicit_remote`, and system TLS.
- Local derivation: `openai_chat_completions_v1`,
  `https://127.0.0.1:1234/v1/chat/completions`, `{kind:"none"}`; compiler derives local,
  `local_zero`, `on_device`, and system TLS with verified IP peer identity. It is a complete
  successor with `baseConfigurationFingerprint` equal to the active base manifest and otherwise
  independently fingerprinted; it is never a scenario-local overlay.
- Repaired remote: one complete successor manifest using
  `https://summary-repaired.stub.invalid/v1/chat/completions`, the same wire protocol/environment
  credential form, its own computed provider/manifest fingerprints, and the base manifest
  fingerprint as predecessor. Validated-
  configuration, redirect-recovery, and HTTPS-downgrade-recovery signals all target these same
  computed fingerprints; free-form `summary-config-*-v2` labels are removed.

Runner-observed provider cost is 0 because the stub is runner-owned. That observation does not
change the remote ProviderChoice's compiler-derived `external_metered` cost class.

## EffectiveCapabilityManifestV1

Immutable non-secret state compiled and activated only by setup.

| Field | Rule |
|---|---|
| `manifestVersion` | Literal `1` |
| `manifestId` | 1-128 ASCII bytes matching `^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$` |
| `configurationFingerprint` | Computed manifest fingerprint; never accepted as input |
| `baseConfigurationFingerprint` | Prior active fingerprint for a successor; absent only initially |
| `destinationPolicyMap` | Closed Claude Code/Codex local/remote entries |
| `resourceProfile` | Exact `ResourceProfileV1` below |
| `summaryProvider` | One validated ProviderChoiceV1 |
| `embeddingProvider` | `{state:"disabled", reason:"slice1_semantic_not_owned", packDegradationReason:"semantic_disabled"}` |
| `legacyDispositions` | At most 64 unique keys matching `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`, each with `translated`, `ignored`, or `overridden`; no values |

Any detected legacy `conflict` belongs to the rejected compile result, not an active manifest.
Unknown fields are rejected. Secret values, content, prompts, memory text, arbitrary headers, and
absolute project paths are prohibited.

Manifest fingerprint input is the full manifest after the provider fingerprint is populated but
without `configurationFingerprint`, encoded as JCS and prefixed by
`free-mem:effective-capability-manifest:v1\0`. The stored value is
`sha256:<64 lowercase hexadecimal characters>` and must recompute exactly.

### Storage and activation

```text
control/
├── install-manifest.json                       # owner-only editor/install ownership manifest
└── capabilities/
    ├── manifests/<configurationFingerprint>.json  # owner-only immutable generation
    ├── current                                     # atomic fingerprint pointer
    ├── lifecycle.lock                              # shared setup/daemon start exclusion
    ├── activation-receipt.json                     # content-free next-start signal import
    └── setup-transaction.json                      # owner-only activation journal while in progress
```

Setup compiles and displays a proposal before mutation. After confirmation it writes the immutable
generation, activation receipt, current pointer, and all selected Claude/Codex editor files as one
recoverable transaction using the existing snapshot/atomic-replace mechanism. An unreferenced
generation may remain after rollback; it has no authority. The activation journal records the prior
current pointer for rollback; Slice 1 defines no second rollback pointer.

The transaction first acquires the shared lifecycle lock, rechecks daemon writer/socket/health state
while held, then acquires the existing setup/spool owner lock for its full duration. Daemon start
acquires the same lifecycle lock before journal/manifest resolution and writer-lock acquisition and
releases it only after startup state is published. No path acquires the lifecycle lock after a
writer or spool lock. This fixed order removes the preflight-to-activation race.

The transaction uses one owner-only `control/capabilities/setup-transaction.json` journal. It stores
phase, intended fingerprint, every target path/hash including `activation-receipt.json`, and
`control/install-manifest.json`, plus mode-0600 prestate bytes needed by the existing snapshot
restore; those bytes never enter manifest/log/evidence. Order is: journal prepared+fsynced, editor
files and install ownership manifest, immutable generation, mode-0600
activation receipt, `current` pointer last, then journal commit/removal. Same-process failure restores
or removes in reverse only after every target is classified as recorded prestate or journal-owned
poststate; any unknown target leaves all targets unchanged and the journal retained. On next
setup/start, a fully published state is finalized only when
the receipt fingerprint, current generation, and all poststate hashes match. A partial state is
restored only when every target still matches its recorded prestate or journal-owned poststate. If
any target matches neither, recovery changes nothing, retains the journal, reports a bounded
recovery conflict, and blocks provider startup; external edits are never overwritten.

A running daemon blocks the first manual activation path before any mutation. Full coordinated
stop/activate/start or attach behavior is a later increment.

At daemon start:

- no `current` pointer: explicit capture-only restricted mode; no ObserverClient or RawEventSweeper;
- malformed pointer, missing generation, digest mismatch, invalid JSON/shape, or failed validation:
  startup failure before provider construction;
- valid pointer: freeze one in-memory generation and pass the same object/fingerprints to every
  doctor/status projection and later provider, scheduler, maintenance, viewer, job, and destination
  resolver. Until v21 jobs plus the complete DestinationBoundary delivery exist, runtime reports
  `pending_privacy_boundary` and keeps provider calls, AI maintenance, and RawEventSweeper disabled.

No runtime consumer rereads provider/resource env or legacy config.

## ResourceProfileV1

The one profile retains the accepted fixture envelope and closes values already used by production
RawEventSweeper/flush behavior.

| Field | Fixed value |
|---|---:|
| `profileId` | `slice1-short-run` |
| `version` | `1` (base) |
| `captureConcurrencyLimit` | 2 |
| `processingConcurrencyLimit` | 2 |
| `processingQueueCapacity` | 25 |
| `processingRetryLimit` | 3 |
| `maxMemoryItemsPerDerivation` | 16 |
| `maxSourceEventsPerJob` | 100 |
| `observerRequestTimeoutMs` | 60,000 |
| `providerTlsPreflightTimeoutMs` | 5,000 |
| `observerMaxInputChars` | 12,000 |
| `observerMaxOutputTokens` | 4,000 |
| `observerMaxResponseBytes` | 1,048,576 |
| `observerTemperature` | 0.2 |
| `workerWarmLifetimeMs` | 30,000 |
| `periodicSweepIntervalMs` | 30,000 |
| `idleFlushMs` | 120,000 |
| `eventDebounceMs` | 1,000 |
| `stuckClaimTimeoutMs` | 300,000 |
| `rawEventRetentionEnabled` | `false` |
| `rawEventRetentionMs` | 0 |
| `maxSteadyProductProcessCount` | 3 |
| `maxShortRunRssGrowthMiB` | 32 |
| `maxPendingQueueDepth` | 20 |
| `maxStorageGrowthBytes` | 1,048,576 |
| `selectionTimeBudgetMs` | 750 |
| `admittedCandidateLimit` | 32 |
| `maxRenderedBytes` | 16,384 |
| `maxSelectedItems` | 8 |
| `maxInjectedTokens` | 800 |
| `exactSessionLaneMax` | 4 |
| `lexicalLaneMax` | 8 |
| `semanticLaneMax` | 0 |
| `recencyLaneMax` | 2 |

The vertical manifest delivery compiles the provider transport and removes mutable scheduler reads,
but keeps provider calls, AI maintenance, and RawEventSweeper execution disabled as
`pending_privacy_boundary`. Schema-backed processing concurrency, capacity, retry, derivation, and
atomic completion become enforceable with the following v21 job delivery. The privacy PR then starts
the Sweeper/provider and enforces periodic/idle/debounce/stuck/source-count/retention plus Injection
Pack limits. No earlier PR claims readiness. Mutable legacy values for every field are reported as
ignored/translated/overridden and are never a fallback.

Enforcement ownership is exhaustive:

| Delivery | Fields first truthfully enforced |
|---|---|
| PR 1 | closed shape/freeze only; execution fields report pending |
| PR 2 | capture/processing concurrency, queue capacity, retry limit, max derivation count, max source events, claim timeout/recovery, retention disabled and retained-job purge safety |
| PR 3 | provider warm lifetime, periodic/idle/debounce scheduling, pack selection/candidate/render/item/token/lane limits |
| PR 6 | process count, RSS, pending-depth, and storage-growth warning thresholds as measured runner gates |

`captureConcurrencyLimit=2` is a non-blocking admission limit: at most two direct capture RPCs run
concurrently, and each singular or bounded max-200 batch request owns one request-level slot. Excess
hook clients use the existing bounded atomic spool within their current hard deadline.
`processingConcurrencyLimit=2` is an upper bound on simultaneously claimed summary
jobs; a deployment may run one, never more than two.

### Closed output-limit recovery successor

The accepted fixture already includes one changed configuration for output overflow. Its test-only
fault contract permits exactly one successor shape: `profileId=slice1-short-run`; compared with
version 1, only `version=2` and `maxMemoryItemsPerDerivation=17` differ. Its manifest is
otherwise identical to the base remote destination/provider/embedding/legacy configuration with
`baseConfigurationFingerprint` equal to the active version-1 manifest. Production setup exposes no
resource-profile selector and always compiles version 1/max16. The runner may materialize the full
test successor; no other resource mutation/profile ID validates. It creates new manifest/attempt
fingerprints but never rewrites admission profile, retry limit, source range, or lifetime attempt
count.

## RepositoryIdentityV1

Repository authority is a domain-separated digest, not a display label:

```text
repo-v1:sha256:<64 lowercase hexadecimal characters>
```

Resolution uses the active local filesystem:

1. Run bounded, non-shell Git probes for the supplied cwd: `rev-parse --show-toplevel`,
   `rev-parse --path-format=absolute --git-common-dir`, and `remote get-url origin`. The probed root/
   common dir must exist and realpath successfully; caller-supplied remote text is ignored.
2. If `origin` is HTTPS, reject all credentials and form
   `remote:https:<host[:nondefault-port]>/<path>`. If it is `ssh://` or SCP-like SSH, require a
   username matching `[A-Za-z_][A-Za-z0-9._-]{0,63}` and form
   `remote:ssh:<username>@<host[:nondefault-port]>/<path>`. In both branches, lowercase the DNS host,
   drop only that protocol's default port, normalize repeated/trailing path separators, and remove
   exactly one trailing `.git`. Transport classes and distinct
   SSH usernames never share authority in Slice 1. Invalid, file, empty-path, ambiguous-user, or
   unsupported remote forms do not become authority. Hash
   `free-mem:repository-remote:v1\0<canonical-remote>`.
3. If no supported verified `origin` exists, resolve the real Git common directory (the primary Git
   anchor for linked worktrees) from the successful probe and hash
   `free-mem:repository-anchor:v1\0<realpathed-common-dir>`.
4. If Git probes fail or neither identity source can be verified, repository identity is NULL/unknown.

All probes share one 100 ms wall-clock budget and one 8 KiB stdout/stderr cap and do not use a shell.
The daemon may cache non-authoritative realpath/common-dir resolution, but it MUST re-read and
canonicalize the current verified `origin` before each capture identity and each restricted
DestinationBoundary decision. A cached remote-derived identity is reusable only when that current
canonical remote matches. If `origin` changes from A to B, the boundary identity becomes B and A's
restricted rows no longer match; probe failure falls back to the currently verified realpathed
common-dir anchor when available, otherwise unknown. Persisted event/session identity remains stable
for already admitted jobs. Probes do not log the raw remote/path. A remote may still be unreachable;
"verified" here means read from the current Git repository rather than trusted from an event/request
claim.

`project`, basename, cwd spelling, caller `workspaceKey`, and CLI/MCP project filters remain display
or query metadata. They never authorize private/local-only disclosure. A linked worktree and its
primary checkout share the remote identity, or the realpathed primary-anchor fallback.

## CapturedEvent persistence

`raw_events` gains:

| Field | Rule |
|---|---|
| `sensitivity` | Checked canonical enum; missing/invalid trusted write normalizes to `secret` |
| `repository_identity` | RepositoryIdentityV1 or NULL/unknown |
| `capture_manifest_fingerprint` | Active manifest at acceptance, or NULL in capture-only mode |
| `capture_state` | `accepted` or `quarantined` |
| `safe_error_code` | Bounded content-free code; required for quarantine |
| `payload_digest_version` | Literal `event-payload-digest-v1` |
| `payload_digest` | SHA-256 of domain plus JCS redacted canonical payload |

Existing source, stream, event ID, sequence, timestamps, and redacted payload remain. Payload fields
never override first-class columns.

Migration drops the legacy global `idx_raw_events_source_stream_event_id` and replaces it with one
unique expression index over
`(COALESCE(repository_identity, 'repo-v1:unknown'), source, stream_id, event_id)`. Valid repository
identities can never equal the closed `repo-v1:unknown` sentinel. NULL remains the stored unknown
representation and authorizes no restricted use; the index-only sentinel deliberately puts all
unknown repositories into one fail-closed collision bucket instead of letting SQLite accept multiple
NULL identities. Fresh v21 and migrated DDL must match, and migration verifies the old index is gone
before commit.

An insert that collides only because both repository identities are NULL never drops input and never
reuses the existing canonical row. In the same sole-writer transaction, it persists a separate
`raw_event_quarantine` record containing a stable quarantine receipt ID, source/stream/event identity,
redacted payload and digest/version, `sensitivity=secret`, `capture_state=quarantined`, and safe code
`repository_identity_unknown_collision`. It returns that non-success receipt with no normal ACK,
canonical admission, job, or memory. Repeated delivery of the same quarantined digest is idempotent;
a different digest gets a distinct receipt. The record is ineligible for every provider/read path,
survives backup/restart, and may converge only by replay with verified repository identity or a new
event identity.

Redaction failure persists a quarantined row with `sensitivity=secret`, empty payload, safe ordering
metadata, and `safe_error_code=redaction_degraded`. It survives restart and is never job/provider/
retrieval input.

Capture conversion is:

```text
redaction degraded or secret -> secret
private                      -> private
local_only                   -> local_only
eligible                     -> eligible
missing/malformed/ambiguous/legacy-unknown/other -> secret
```

The daemon computes repository identity from the actual cwd/Git state. Caller `projectKey` is not
authority.

### EventIdentityConflictV1

The canonical identity is the repository-aware unique-index key plus source/stream/event ID. First acceptance stores
`payload_digest=sha256("free-mem:event-payload-digest:v1\0" || JCS(redacted canonical payload))`.
Same identity and digest is an idempotent duplicate only after one Store transaction joins
sensitivity using `secret > private > local_only > eligible` and makes quarantine absorbing. The
transaction strengthens the canonical row and every already-derived record that cites the event;
quarantine makes them ineligible and returns a non-success quarantine receipt. Downgrade replays do
nothing, and payload/digest bytes never change.

Same identity with a different digest never
overwrites the canonical row or returns a normal ACK; one durable conflict row/receipt records both
digests, reason `event_identity_payload_conflict`, non-success state, canonical-unchanged=true, and
memory delta 0. Its domain-separated ID and unique key include canonical identity, digest version,
canonical digest, and conflicting digest. Replays of that exact pair reuse the receipt; a different
conflicting digest creates another receipt. Conflict records are content-free and inherit
repository identity/manifest provenance.

## RawEventFlushBatch as MemoryProcessingJobV1

The existing `raw_event_flush_batches` row remains the only summary job for one immutable
source/stream/sequence range. No second queue or generic job framework is added.

### Fields

| Field | Purpose |
|---|---|
| source range and extractor fields | Immutable admission source identity |
| `status` | Canonical state below |
| `admission_manifest_fingerprint` | Immutable manifest at admission; required new, NULL only for legacy unknown |
| `admission_provider_fingerprint` | Immutable provider at admission; required new, NULL only for legacy unknown |
| `retry_limit` | Automatic attempt limit frozen at admission (3) |
| `attempt_count` | Starts at 0 for new admission; lifetime successful-claim count, monotonic and never reset |
| `claim_generation` | Monotonic stale-worker fence |
| `attempt_manifest_fingerprint` | Manifest selected for the current claim |
| `attempt_provider_fingerprint` | Provider selected for the current claim |
| `attempt_fingerprint` | Computed identity for the current claim |
| `attempt_max_memory_items` | Store-owned active-manifest output limit for the current claim (16 or bound successor 17) |
| `resume_grant_id`, `resume_grant_reason` | One-shot grant identity/reason |
| `resume_grant_state`, `resume_grant_consumed_at` | `none`, `pending`, or `consumed` plus consumption evidence |
| `last_resume_signal_id`, `last_resume_sequence` | Signal identity and monotonic replay fence |
| `last_resume_signal_disposition` | `accepted`, `duplicate`, `stale`, `grant_pending`, `wrong_job`, `wrong_role`, `wrong_provider`, `unchanged_configuration`, or `unrelated_component` |
| `safe_error_code` | Bounded content-free failure code |
| `egress_diagnostic_json` | Version/action/reason/counts only |
| `output_count`, `observed_output_count` | Atomic derivation-limit evidence |
| `completion_disposition` | `none`, `memory_committed`, `privacy_skip`, or `legacy_unrecoverable` |
| `legacy_recovery_state` | `not_legacy`, `complete_range`, or `missing_or_ambiguous_range` |
| `frontier_already_advanced` | Marks exact legacy recovery that must not lower/advance twice |

### Claim-bound ProjectedSourceSetV1

The Store claim transaction consumes one compiler-created `DestinationBoundaryV1`, loads the exact
immutable job range, applies eligibility, and returns a `ProjectedSourceSetV1` bound to that exact
claim object. Raw caller objects are not accepted as compiled boundaries. The Store keeps the
association privately and in memory for the lifetime of the claim; it is not a second durable queue
or a new schema column. A crash invalidates the association, and normal stale-claim recovery must
issue a new claim and projection before another provider request.

```text
ProjectedSourceSetV1 {
  version: 1
  jobId, claimGeneration, attemptFingerprint
  destinationBoundaryFingerprint
  repositoryIdentity
  sources[]: {
    ordinal, eventId, sensitivity,
    redactedPayload, payloadDigest, payloadDigestVersion
  }
}
```

The Store derives the set and its fingerprint from the active claim, closed destination boundary,
and current raw rows. Provider prompt/context/transcript construction and response citation
resolution use this exact object. Completion re-loads the claimed rows, re-evaluates the boundary,
and compares the ordered source identities/digests before any write. A caller-forged set, changed
sensitivity/quarantine/repository, source drift, stale claim, or boundary mismatch rejects the whole
completion. The persisted `attempt_fingerprint` remains the durable stale-worker fence; the private
claim association prevents a caller from attaching a different projection to that attempt.

### States and capacity

```text
queued -> processing -> completed
                    -> failed -> queued               (automatic budget remains)
                    -> retry_exhausted                 (automatic budget exhausted)
retry_exhausted -> queued                              (one valid grant)
processing -> completed                                (atomic privacy skip)
processing -> failed                                   (stale-claim recovery)
```

SQLite persistence uses `retry_exhausted`; the existing public fixture/result spelling remains
`retry-exhausted` and is a deterministic projection, not a second state.

Capacity 25 counts `queued`, `processing`, `failed`, and `retry_exhausted`. Capture persists before
admission. When full, later accepted source events remain durably not admitted and visible; no job or
event is displaced. Completed rows do not consume admission capacity.

At most 100 contiguous source events enter one newly admitted v21 job. Remaining events stay behind
the same session frontier for later admission. A migrated v20 recovery job preserves its original
immutable range even when the old configurable worker admitted more than 100; migration does not
split or truncate it.

New admission requires the first sequence to equal `frontier + 1` and every selected sequence to be
contiguous. A gap creates no job, does not move the frontier, and reports content-free `source_gap`;
it is not bridged by a later event.

### Claim and attempt identity

A claim transaction:

1. requires `queued` and either `attempt_count < retry_limit` or an unconsumed one-shot grant;
2. consumes the grant when present;
3. increments `attempt_count` and `claim_generation` exactly once;
4. copies the active manifest/provider fingerprints into the attempt fields;
   the ordinary closed path derives limit 16, while limit 17 additionally requires the full
   validated version-2 successor, a pending grant, the prior max-16 version-1 attempt as its base
   fingerprint, an unchanged provider, and an accepted activation signal targeting that exact
   manifest/provider;
5. computes `attempt_fingerprint` from domain
   `free-mem:processing-attempt:v1\0` plus job ID, immutable source range, new attempt count, claim
   generation, and attempt manifest/provider fingerprints;
6. validates a compiler-created destination boundary, projects the exact source rows, and binds the
   resulting `ProjectedSourceSetV1` privately to the returned claim;
7. enters `processing` atomically.

Changed valid configuration changes only the attempt fields/fingerprint. Admission fingerprints,
source range, retry limit, and prior attempt count remain unchanged. A resumed attempt that fails
returns directly to `retry_exhausted`; another timer attempt is not inferred from a changed limit.

Migration never backfills a legacy admission fingerprint from the currently active manifest. Legacy
uncompleted rows with complete sources retain NULL admission provenance, preserve attempt count, enter
`retry_exhausted`, and require one valid grant whose current fingerprints populate only attempt
fields. Safe projections render NULL admission as `legacy_unknown`, not a fabricated digest.

### ResumeSignalV1

One per-job signal has unique ID, producer receipt ID, exact target job ID, monotonic per-job
sequence, target role `summary`, target provider fingerprint, target manifest fingerprint, and one
`kind`:

- `validated_configuration_activation` (must change manifest or provider fingerprint);
- `recorded_provider_healthy_transition` (daemon-recorded unhealthy→healthy edge);
- `user_confirmed_doctor_retry`.

A valid signal creates one pending grant only when no grant is already pending. Duplicate/out-of-order, wrong-job, wrong-role, wrong-provider,
unrelated-component, or unchanged invalid configuration signals are durable content-free no-ops.
The grant is consumed by at most one claim. Timer passage never creates a grant.

If `resume_grant_state=pending`, a concurrent otherwise-valid setup, health, or doctor signal returns
content-free disposition `grant_pending` and mutates no signal row, producer-receipt uniqueness,
sequence, grant, or attempt. The producer remains retryable after the pending grant is consumed and
that attempt reaches a terminal state. No signal overwrites, queues behind, or creates a second grant.
The first global producer fanout freezes only retry-exhausted jobs and jobs with a pending resume
grant; ordinary queued, processing, and failed jobs are excluded. Eligible exhausted siblings apply
without waiting for pending targets. The same receipt remains `grant_pending` and may be replayed to
fill only its missing per-job signals after a frozen target becomes retry-exhausted; `fanout_count` is
cumulative and an already complete replay is duplicate.

The only producers are durable and component-specific:

1. Setup writes a content-free activation receipt with monotonic activation sequence under the
   lifecycle lock; the next v21 daemon imports it through the sole writer and records one idempotent
   `validated_configuration_activation` signal per currently matching retry-exhausted job, keyed by
   job, receipt ID, and fingerprints. UUID receipt IDs are canonicalized to lowercase before the
   durable identity comparison. Exact receipt replay is duplicate after its frozen set is complete.
   A partial receipt superseded by a greater imported activation sequence is stale for its remaining
   targets and preserves its existing fanout count; an unseen receipt whose sequence does not exceed
   the greatest imported activation sequence is stale with zero fanout. Both stale cases perform no
   signal or job mutation.
2. Observer health is persisted per manifest/provider; only a committed unhealthy-to-healthy edge
   emits one `recorded_provider_healthy_transition`. Repeated probes are no-ops.
3. An explicit user-confirmed doctor retry RPC/CLI command emits one
   `user_confirmed_doctor_retry` for exactly the displayed job after showing its component and
   fingerprints.

The displayed snapshot separates nullable prior attempt provenance from the frozen active
manifest/provider `retryTarget`. A legacy-unknown confirmation compares the NULL attempt pair plus
attempt count, claim generation, state, and grant state, then records the active target only on the
grant. The claim is the first write of current attempt provenance; admission remains NULL.

Setup/health producer receipts are global events, but one sole-writer transaction freezes all
currently matching `retry_exhausted` jobs and jobs with an already-pending resume grant, then signals
the ready exhausted subset. The global uncompleted-job capacity is 25, so that complete set is
necessarily at most 25. Each signal stores
`targetJobId` and `producerReceiptId`; `(job_id, producer_receipt_id)` and `(job_id, signal_id)` are
unique. Job state, `resume_grant_state != pending`, role/provider/manifest fingerprints, and
`incoming.sequence > preLastConsumedResumeSequence` are one CAS. Only acceptance sets
`postLastConsumedResumeSequence=incoming.sequence` and inserts a pending grant; gaps are allowed,
while equal/stale values are no-ops. Claim consumption and attempt increment are one transaction.
Receipt import and crash replay are idempotent. Setup never opens the canonical database directly.
The first import freezes the complete sorted target ID set in `target_job_ids_json`; receipt replay
may fill only missing signals from that set and never targets a job created after the producer event.

Every accepted signal, including changed configuration, has `grantCount=1`; it never resets or
refills a three-attempt budget. After claim, the grant count is 0. Success completes; failure returns
to retry-exhausted and requires another distinct valid signal.

The fixture's malformed-response configuration activation plus redirect and downgrade recovery all
use the one repaired-remote successor's computed manifest/provider fingerprints. Healthy-transition
and user-confirmed retry target the computed active base fingerprints. Output-limit recovery targets
the computed test-only version-2 manifest fingerprint without changing admission provenance.

Each recovery case has one exact signal kind/producer/job mapping. `observedTransition.
lastConsumedSequence` is the post-transition value. Accepted evidence requires
`preLastConsumedSequence < signal.sequence` and
`observedTransition.lastConsumedSequence=signal.sequence`, automatic `budgetBefore=0`, `budgetAfterGrant=1`,
and `budgetAfterAttempt=0` for the consumed signal. `ignoredSignalCount=0` applies only when no
ignored signals are delivered; aggregate cases may count stale/duplicate signals alongside one
accepted transition. Swapped kinds or stale sequence fields are no-ops.

### Completion transactions

**Memory completion** validates job status, claim generation/fingerprint, exact projected source
set/citations, output limit, lineage/revision/dedup/supersession invariants, and sensitivity. One
transaction commits every memory and reference and marks the job completed. It advances the
contiguous `last_flushed_event_seq` once only when `frontier_already_advanced=false`; a fully
recovered legacy `gave_up` job leaves the frontier unchanged. Any failure commits none of these.
For a durable flush claim, session-end metadata is committed only inside this transaction; failed
attempts do not run the legacy cleanup outside it.

**Privacy skip** validates the same active claim and exact all-ineligible source set. One transaction
stores a content-free diagnostic and marks completed. It advances the frontier once only when
`frontier_already_advanced=false`, with zero provider request and zero memory output.

A completed privacy skip is terminal for that admitted source range. Later configuration activation
does not silently reopen it; a separately specified user-authorized replay contract would be needed
and is outside Slice 1.

`last_flushed_event_seq` is never attempt state. Failed/retry-exhausted work retains sources and does
not move it.

### Legacy completed crash window

The v20 success path could commit `status=completed` before advancing the session frontier. During
migration, a retained completed range starting exactly at `frontier + 1` advances the frontier only
after every exact sequence exists and no other legacy batch overlaps it. Migration repeats this for
a contiguous completed chain and leaves already-advanced frontiers unchanged. An incomplete,
overlapping, gapped, or sessionless stale-completed range aborts the transaction; migration never
replays or deletes the completed row, rewinds the frontier, or discards its committed memory.

### Legacy `gave_up`

Migration never lowers a session frontier. For each legacy `gave_up` row:

- if every exact source sequence still exists and the range is unambiguous, migrate to
  `retry_exhausted`, `legacy_recovery_state=complete_range`, and
  `frontier_already_advanced=true`; an explicit grant may recover memory without moving the frontier;
- otherwise mark `status=completed`, `completion_disposition=legacy_unrecoverable`, retain a
  content-free `missing_or_ambiguous_range` diagnostic, set
  `legacy_recovery_state=missing_or_ambiguous_range`, and create no grant. It consumes no capacity
  and is never projected as successful recovery.

No synthetic source, blind cursor rewind, or range spanning missing events is allowed.

If otherwise-recoverable legacy rows would exceed capacity 25, the v20-to-v21 transaction aborts and
leaves schema 20 unchanged. Overflow rows are not truncated, terminalized, or grandfathered outside
capacity; legacy uncompleted work must be reduced with the prior runtime before migration is retried.

## Raw-event retention

Slice 1 fixes `rawEventRetentionEnabled=false` and `rawEventRetentionMs=0`, so automatic purge does
nothing. The storage method must nevertheless encode the future safety precondition: a non-zero
policy deletes only sequences at/below the committed session frontier and excludes every sequence
referenced by an uncompleted job (`queued`, `processing`, `failed`, or `retry_exhausted`). Accepted
not-yet-admitted rows above the frontier are never purgeable. A profile cannot enable retention until
that test passes.

## EgressDiagnosticV1

Payload-free fixed shape stored on the processing job.

| Field | Values |
|---|---|
| `version` | `1` |
| `action` | `sent`, `projected`, `skipped`, `failed`, or `exhausted` |
| `reason` | Closed bounded reason code |
| `destination` | `local`, `remote`, or `unknown` |
| counts | considered/transmitted plus count per sensitivity; no IDs |
| `configurationFingerprint` | Safe manifest identity |
| `providerFingerprint` | Safe provider identity |
| `attemptFingerprint` | Safe attempt identity when claimed |
| `nextAction` | Required closed safe action code below; never free text |

It contains no event/memory ID, source text, title, path, prompt, query, request/response excerpt,
sentinel, credential value, or restricted preview. Actual request/byte evidence comes from the
runner-owned stub/network boundary.

The closed `reason` values are:

```text
eligible_only | restricted_projected | all_restricted | destination_unknown |
repository_unknown | repository_mismatch | redaction_degraded |
provider_unavailable | provider_redirect_rejected | provider_tls_rejected |
provider_auth_failed | output_invalid | output_limit_exceeded |
retry_exhausted | stale_claim | missing_or_ambiguous_range | source_gap |
omitted_ineligible
```

The pre-provider `eligible_only` terminal shortcut applies only when every projected event is either
a trivial prompt or a session start/idle/end lifecycle event; lifecycle-only ranges have no count
threshold. Adapter errors, unknown events, and substantive prompts remain retryable and are never
silently completed by that shortcut.

`omitted_ineligible` is required for aggregate restricted omissions in pack/trace/read diagnostics;
it carries counts only and never an item ID or preview.

The closed safe next-action values are:

```text
none | activate_valid_manifest | configure_credential | wait_for_capacity |
confirm_retry | restart_daemon | upgrade_runtime
```

## MemoryItem Slice 1 fields

`memory_items` gains:

| Field | Rule |
|---|---|
| `sensitivity` | Strongest contributing source; monotonic |
| `repository_identity` | Exact shared identity of cited sources; unknown/mixed is ineligible |
| `lineage_id` | Deterministic logical-fact identity including repository identity |
| `revision_id` | Deterministic content/profile/model revision identity |
| `revision_ordinal` | Monotonic within lineage |
| `supersedes_memory_id` | Prior same-repository revision when present |
| `derivation_key` | Deterministic retry/dedup identity |
| `source_event_ids_json` | Bounded ordered cited IDs from the projected set |
| `source_spans_json` | Bounded source anchors from the projected set |
| `manifest_fingerprint` | Attempt manifest that produced the revision |
| `provider_fingerprint` | Attempt provider that produced the revision |
| `attempt_fingerprint` | Processing attempt that committed the revision |

### Provider citation wire v1

Every new PR3 provider-produced `<observation>` and `<summary>` contains exactly one direct
`<citations>` child with one or more self-closing cites:

```xml
<citations>
  <cite source="0"/>
  <cite source="3" start="12" end="31"/>
</citations>
```

`source` is the canonical zero-based decimal ordinal in the exact ordered projected source set shown
to the provider. The active processing claim maps that ordinal to the exact raw-event ID and trusted
repository identity; provider output never supplies either authority value. Each source ordinal may
appear at most once per item. `start` and `end` are optional as a pair, use canonical non-negative
decimal integers with `start < end`, and form a half-open UTF-8 byte range within the canonical
`redactedPayload` itself as defined by the Product Reset source model. Omitting both normalizes to the
complete `[0, utf8ByteLength(redactedPayload))` span. Offsets must fall on UTF-8 scalar boundaries. Cites
are strictly ordered by source ordinal, so the count is bounded by the projected source count.

After claim resolution, `source_event_ids_json` preserves strictly increasing cited source-ordinal
order and deduplicates by first cited ordinal. `source_spans_json` stores the Product Reset canonical
shape `[{eventId,startByte,endByte}]`, independently deduplicated and sorted by event ID then numeric
offsets for anchor/lineage identity. Lineage, source-fact anchor matching, tombstone coverage, and
derived dedup identity include the normalized spans, not event IDs alone. Duplicate anchors within
one provider response reject that response atomically. On a later retry, an exact active-anchor match
deduplicates to the existing anchor; ambiguous overlap with an active anchor is quarantined, overlap
with a tombstoned anchor is suppressed, and only disjoint spans may become sibling facts.

Missing, empty, multiple, or nested citation blocks; non-self-closing cites; forbidden provider
event-ID/repository/digest authority; duplicate or noncanonical ordinals; one-sided, malformed, or
out-of-bounds spans; offsets that split a UTF-8 code point; claim/source drift; and any out-of-set or
mixed-repository resolution reject the whole output atomically. Parse/repair preserves the citation
child unchanged. Historical stored records remain readable without this provider wire child; no
legacy record is upgraded by inference.

The citation child is mandatory only for a provider response attached to a durable PR3 raw-event
claim. The legacy no-claim `ingest` parser remains readable for compatibility, but PR3 MUST NOT issue
a provider request or create remotely eligible derived provenance through that path. Existing rows
with NULL citation/span provenance remain secret/unknown at read/export boundaries and are never
upgraded by inferred batch-wide citations.

The fixed fixture adds `summary`, `failed_approach`, and `next_action` while retaining existing
compatible kinds.

Derivation rules:

1. Provider input contains only destination-eligible source events.
2. Provider output cites only the job's exact projected event IDs/spans. New v21 jobs project at most
   100; a migrated legacy recovery job may project its wider immutable actual range.
3. Each item inherits the strongest cited sensitivity and one exact repository identity.
4. Unknown/out-of-set citations, mixed repository identities, partial parse, or output count above
   the active attempt manifest's `maxMemoryItemsPerDerivation` reject the whole result.
5. Derived-memory dedup requires the same repository plus the exact normalized source spans, uses the
   stronger sensitivity, and never reactivates a tombstone. Different span provenance produces a
   separate memory because one memory stores only one attempt provenance.
   Derived matching never falls back to legacy title-only or NULL dedup keys. Unknown identity cannot
   merge into a known repository item.

## DestinationBoundaryV1

One closed trusted value is required before any content-bearing read or provider input:

| Field | Rule |
|---|---|
| `version` | Literal `1` |
| `consumer` | `summary_provider`, `hook_pack`, `daemon_get`, `daemon_search`, `daemon_pack`, `mcp_direct`, `mcp_index`, `viewer`, `maintenance`, `export`, `import`, or `dedup` |
| `targetAgent` | `claude-code`, `codex`, or `none` |
| `targetModel` | 1-256 UTF-8 byte model ID or NULL when not applicable |
| `executionLocation` | `local`, `remote`, or `unknown` from frozen manifest/request context |
| `repositoryIdentity` | Verified RepositoryIdentityV1 or NULL |
| `configurationFingerprint` | Frozen daemon manifest fingerprint |
| `providerFingerprint` | Required for provider/maintenance; otherwise NULL |
| `providerPeerTrust` | `verified` or `unverified` for provider/AI-maintenance transport, otherwise `not_applicable`; compiler/runtime-derived, never caller supplied |

The boundary is internal and cannot be created from a user project/basename filter. One pure
eligibility function and its SQL predicate apply the decision table before any row content is
materialized. This is a narrow privacy seam, not a generic policy engine.

Claude Code, Codex, and MCP always resolve to remote/unknown in Slice 1 production. Their local
process, Agent/model label, project, or RPC payload is not authority over model egress, and setup
creates no on-device Agent attestation. The `*-local` destination classes remain runner-owned
fixtures and can be selected only after the candidate-inaccessible runner verifies its loopback
consumer and binds that observation to the result; otherwise local selection is impossible.

For provider/AI-maintenance boundaries, local HTTP compiles to `providerPeerTrust=unverified` and
local/remote HTTPS becomes `verified` only after the exact peer passes chain+hostname/IP verification
at daemon start; every request still performs normal TLS verification. Other consumers use
`not_applicable`. The pure eligibility function receives this field directly and never resolves a
fingerprint back through mutable configuration.

| Destination | eligible | local_only | private | secret |
|---|---:|---:|---:|---:|
| remote or unknown; or local provider with unverified peer | allow | deny | deny | deny |
| verified local provider or runner-attested local consumer + exact known repository | allow | allow | allow | deny |
| trusted local + cross/unknown repository | allow | deny | deny | deny |

For local summary/maintenance, the boundary repository is the known source repository for the
current projected group. For hook/MCP/export/dedup, it is the verified destination/current
repository. Viewer has no repository authority in Slice 1 unless the daemon supplies a verified
identity, so its unknown boundary returns eligible content only.

Before local provider prompt construction, candidate events are stably partitioned by exact verified
repository identity. A mixed or unknown group is rejected content-free before projection; post-output
citation checks are defense in depth, not the repository-isolation boundary.

The seam covers:

- provider flush and `maintenance/ai-structured` before prompt construction, plus maintenance
  memory-role pack/report reads;
- search, recent, timeline, explain, `findByFile`, and `findByConcept` SQL candidates;
- daemon get/search/pack and MCP full-body/index/recent/timeline/explain/pack reads;
- viewer raw-event/status/usage and memory/observation/summary/prompt/artifact/safe-session
  projections;
- lexical/semantic candidates, final pack rendering, and traces;
- export serialization and import normalization;
- dedup/supersession identity matching.

The currently public extraction-replay and distill barrel exports have no production caller and are
removed. Their internal benchmark code remains test-only; any future public/runtime exposure must
accept the same DestinationBoundary before reading raw/memory rows or constructing an Observer
prompt.

## InjectionPack Slice 1 projection

The existing pack adds manifest/destination identity, semantic-disabled degradation, exact final
bytes/tokens/items, provenance, source lane, and terminal reasons.

Eligibility is resolved before title/body/preview formatting, token/byte measurement, ledger
exposure, or trace construction. A restricted omission contributes only aggregate reason/lane/
sensitivity counts and `omitted_ineligible`; it emits no ID, title, body, preview, query, path, or
source citation. Eligible injected items keep visible source and selection reasons.

Semantic candidates are rehydrated from first-class database columns and pass the same eligibility
function. `semantic_disabled` prevents use, not storage deletion.

## Export, import, backup, and diagnostics

- Schema v21 gives `user_prompts`, legacy `session_summaries`, and content-bearing `artifacts`
  first-class `sensitivity` and `repository_identity`, and gives `sessions` a canonical
  `repository_identity`. Unknown legacy values backfill to secret/unknown. Current observer summaries
  stored as memory items keep normal MemoryItem fields.
- Export requires a DestinationBoundary and emits payload version 2.0. It applies eligibility to
  every `memory_items`, `user_prompts`, and legacy `session_summaries` row before serialization.
  Restricted rows require a verified same-repository local boundary; unknown/all-project export
  includes eligible rows only.
- Exported `sessions` are referential shells for already-eligible child rows. They may contain stable
  import key, timestamps, safe display project, and repository digest, but omit `cwd`, `git_remote`,
  `git_branch`, `user`, and free-form `metadata_json`. A session with no eligible child is absent.
- Import v2 preserves and validates first-class sensitivity/repository/provenance without downgrade.
  Missing/malformed fields become `secret`/NULL. Legacy v1 content rows always import as
  `secret`/NULL regardless of project/remap labels; caller `--remap-project` remains display metadata
  and cannot authorize disclosure.
- Backup/restore preserves all first-class fields and job/source ranges because it is local durable
  state, not a disclosure surface.
- Logs, status, doctor, job records, and maintenance messages use codes/counts only. Existing
  content-excerpt warnings are removed.

## Fixed short-run plateau

The runner executes 12 identical duplicate/no-op workload windows with a complete drain and SQLite
checkpoint after each window. Windows 1-2 are warm-up; windows 3-12 must stay within every absolute
ResourceProfile ceiling. Across the final five windows (8-12), product process count is constant,
pending queue depth is zero after every drain, selected item/token counts are identical, RSS span is
at most 16 MiB, and storage span is at most 65,536 bytes. Processing concurrency never exceeds 2 and
post-teardown orphan process count is zero. Any missing sample or equality above a ceiling fails.
All 12 windows carry the same positive `duplicateDeliveryAttemptCount`.

## Closed runner evidence additions

For every executed result, the accepted runner-evidence schema requires all 12 raw window records. Each carries
ordinal, process count, RSS MiB, drained queue depth, storage bytes, selected-item count,
injected-token count, max processing concurrency, unique workload/drain/checkpoint receipts, and
runner-monotonic workload start, workload receipt, drain receipt, checkpoint receipt, and sample
times. Each chain is strictly increasing and the prior sample is strictly before the next workload.
The plateau object binds the bundle candidate, artifact, environment, runner invocation, and one
fresh workload process-tree root unique from all observed initial/recovery provider roots.
Provider cases also carry the base/local/repaired hostnames, hostname/IP-valid public CA SHA-256
fingerprint, normal chain/hostname-validation booleans, and `privateKeyCommitted=false`. The network
object includes exactly six runner-owned raw TLS preflight receipts: base, local, and repaired host,
each at `setup_activation` and `daemon_start`. Every receipt has a unique opaque ID, exact hostname,
remote SNI or null IP SNI, endpoint port, timeout 5,000 ms, monotonic start/end within timeout,
verified result, normal chain/
hostname booleans, `trustAnchorSha256` equal to the bundle public CA, one peer-certificate SHA-256
that is identical across the endpoint's setup/start receipts, and zero HTTP requests, credential
bytes, and payload bytes. Each setup receipt finishes strictly before its daemon-start receipt begins.
The network object and all six receipts repeat the current bundle runner invocation.

Each real scenario has one runner-owned provider-egress observation. Its monitor starts before the
candidate and ends after process-tree termination. A runner-only network gate opens strictly after
the runner directly reads the durable store and records explicit canonical-order committed event
IDs, their count, and domain-separated event-set fingerprint; non-prefix sets remain representable.
The receipt binds active provider/location, first/last request time, exact request/payload/auth and
redirect aggregates, zero pre-authorization/non-loopback attempts, and source bytes by sensitivity.
The runner-owned stub measures those sensitivity bytes from the request bytes it actually receives
against fixed synthetic source markers/spans; neither policy expectation nor candidate output
supplies the observed values, and the four-bucket sum cannot exceed `payloadBytesSent`. The receipt
also owns `restrictedPayloadBytesSent=0` and `forbiddenSentinelObservationCount=0`, each matched to
the result instead of trusting candidate counters.
The late-injection negative projects its base receipt instead of claiming another run.
Every fixed retry/redirect recovery subcase has one sorted, case/manifest-bound full observation
under the same authorization/timing/TLS/wire/sensitivity rules. No-op subcases carry full zero-egress
observations rather than relying on an absent receipt. Every observation binds the runner invocation
and an owning initial/recovery process-tree root; observed roots and receipt IDs are unique across the
bundle, and the nested receipt's `observationCaseId` equals its owning record/wrapper case. The live
runner generates invocation/root identities freshly per execution/process
generation and never derives them from a reusable PID alone.

Every plateau window also carries a unique opaque `workloadReceiptId`, one duplicate-delivery
attempt count of at least one, `noOpOutcome=duplicate_noop`, `durableMemoryDelta=0`, and
`processingJobDelta=0`. The result schema carries only separate network/plateau fingerprints and
derived resource aggregates; the validator recomputes those fingerprints from the runner bundle.
Canonical `unsupported`/`not_run` no-activity evidence carries `resourcePlateauEvidence: null` and
a null result plateau fingerprint; any executed result must carry the complete plateau.

The suite contains one same-event-ID/different-payload-digest probe whose repeated attempts reuse one
pair-bound durable receipt while a different digest cannot, plus exactly 16 positive scenario observations and one
late-injection-after-model-dispatch negative. Result-observation and runner-bundle fingerprints bind
all these fields.

## Migration v20 -> v21

1. Verify a backup through the existing migration gate.
2. Begin one database transaction.
3. Set the transactional compatibility floor and reconcile exact contiguous completed ranges that
   still need frontier advancement after the v20 crash window, without lowering any frontier.
   Preserve already-advanced completed rows even when legacy pruning removed their source.
4. Add every Slice 1 column/check/index for events, memory items, user prompts, legacy session
   summaries, content-bearing artifacts, session repository identity, jobs, provenance, and diagnostics.
5. Backfill current content-bearing records conservatively: only trusted structural evidence may retain a known
   sensitivity/repository; otherwise sensitivity is `secret` and repository identity NULL. Translate legacy
   job states, including exact `gave_up` range audit.
6. Validate closed enums, fingerprints, source-range completeness, provenance, and references.
7. Update schema compatibility marker and `user_version` to 21.
8. Commit; on any error roll back all changes and start no provider or sweeper.

Fresh databases receive final v21 DDL directly. The generated test schema is regenerated from the
same source. Export/import and backup/restore tests prove field preservation and restrictive legacy
defaults.
