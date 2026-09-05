# Contract: Alpha Foundation Comparison

## Purpose

Provide one small, reproducible contract for comparing a candidate foundation or resource-policy
change before adopting it. This contract is not a general benchmark framework.

## Required scenarios and owner slices

1. **Slice 1**: Claude Code capture followed by Codex lexical retrieval and injection.
2. **Slice 1**: Codex capture followed by Claude Code lexical retrieval and injection.
3. **Slice 1**: Runtime unavailable during capture, followed by duplicate delivery of the same
   stable event identities during spool recovery and a same-ID/different-digest conflict probe.
4. **Slice 1**: Summary provider unavailable or malformed, followed by each authorized resume
   signal and duplicate/out-of-order signal delivery.
5. **Slice 1**: Summary provider returns one item above the active derivation limit; no partial
   output commits, and only a changed larger limit/provider resumes the retained job.
6. **Slice 1**: Summary provider returns either a cross-host HTTPS redirect or a same-host
   HTTPS-to-HTTP downgrade; no request or payload is sent to either redirect location and doctor
   reports the bounded rejection reason.
7. **Slice 1**: Remote HTTP providers with and without configured credentials are each rejected with
   a non-empty redacted payload before activation, credential transmission, request, or payload
   transmission. Verified-HTTPS activation is separately rejected for an invalid certificate chain
   and a hostname mismatch with the same zero-egress evidence.
8. **Slice 1**: Local-only content is considered for a remote summary provider but produces zero
   remote requests or payloads.
9. **Slice 1**: A mixed eligible/local-only/private/secret batch sends one allowed-only projection
   while restricted sentinels contribute zero transmitted bytes.
10. **Slice 1**: Private content is considered for a remote summary provider but produces zero remote
   requests, payloads, or injection.
11. **Slice 1**: Memory derived locally from a local-only source retains `local_only` and is omitted
   from a remote InjectionPack destination.
12. **Slice 1**: A candidate from another repository scope is considered and omitted before
   injection.
13. **Slice 2**: Semantic provider or index unavailable, with lexical fallback and InjectionPack
   reasons.
14. **Slice 3**: Deleting a fact prevents regeneration after profile, model, or semantic-kind
   reclassification while sibling source facts remain available.
15. **Resource samples**:
    - Slice 1: cold and warm short-run samples only;
    - Slice 3: long, burst, packed-artifact, and eight-hour soak samples.

An owner slice is required to implement only its scenarios. Later-slice scenarios are recorded as
`not_run`, never inferred from a disabled capability or used to block an earlier slice.

## Fixture requirements

Each fixture defines:

- pinned Agent and candidate versions
- isolated configuration and data locations plus pinned OS/kernel, CPU, memory, and filesystem
  descriptors for the reference runner
- ordered input events and lifecycle milestones
- ordered expected injected items binding fact, memory kind, source events, lane, and selection
  reason; expected omissions; forbidden facts; and retrieval queries
- expected durable event and MemoryItem counts, including persisted summaries
- declared effective-manifest identity/fingerprint; complete local-derivation, repaired-remote, and
  output-limit successor manifests with their activation boundaries; computed ProviderChoice
  fingerprints; and a versioned destination-policy map resolving every scenario target class
- base remote `openai_chat_completions_v1` at
  `https://summary.stub.invalid/v1/chat/completions` with environment credential
  `FREE_MEM_SUMMARY_API_KEY`; local successor at
  `https://127.0.0.1:1234/v1/chat/completions` with credential `none`; and repaired remote at
  `https://summary-repaired.stub.invalid/v1/chat/completions`
- the fixed complete ResourceProfile, including 100 source events/job, 60,000 ms observer timeout,
  exact JavaScript UTF-16 system/user allocation within 12,000 units, 4,000 output tokens, 1 MiB
  response, temperature 0.2, 5,000 ms provider TLS preflight,
  sweep/idle/debounce/stuck intervals, and disabled zero-duration raw-event retention; only the
  runner-owned output-limit successor uses profile version 2/derivation limit 17
- the complete pinned InjectionPack selection envelope, including time, candidate, byte, item,
  token, and per-lane budgets
- latency, process, memory-growth, queue, storage, and token thresholds, plus fixed repetitions,
  warm-up/reset rules, sample boundaries, and percentile calculation
- runner-owned network trust binding the exact base/local/repaired hostnames, public CA fingerprint,
  setup/start credential/payload-free native TLS preflights, normal chain/hostname validation, and
  absence of a committed private key; exactly six unique raw receipts cover base/local/repaired by
  setup activation/daemon start and bind host/SNI/exact endpoint port/5,000 ms, the per-run CA trust
  anchor, one peer-certificate fingerprint per endpoint that is identical across its two phases and
  distinct from the CA, verified duration, setup completion strictly before daemon-start beginning,
  the current runner invocation repeated by the network object and all six receipts, and zero
  bytes/requests
- one runner-owned 12-window duplicate/no-op plateau: discard 1-2, measure 3-12, evaluate final
  8-12 for constant processes, drained queue, equal item/token counts, RSS span at most 16 MiB,
  storage span at most 65,536 bytes, concurrency at most 2, unique path-free
  drain/checkpoint/workload receipt IDs, strict runner-monotonic workload-start → workload-receipt →
  drain-receipt → checkpoint-receipt → resource-sample order with non-overlapping windows, one
  identical positive duplicate-delivery attempt count across all 12 windows, exact
  `duplicate_noop`, zero durable/job deltas, and zero orphans; measured RSS/storage ceilings are
  maximum increase from window 3, while only the final-five predicates use max-minus-min spans
- an explicit drain condition proving comparable completion across candidates
- a versioned structural fixture schema and mandatory semantic validation path; neither check alone
  establishes fixture conformance
- a domain-separated fingerprint over the entire fixed fixture except the fingerprint field itself,
  and additionally over its structural schema, semantic validator text, and canonical executable
  validator text, plus the result schema/semantic/canonical-validator artifacts; text is normalized
  to LF and the fixture executable's pinned-fingerprint literal is normalized to a placeholder to
  avoid a self-hash cycle. All imported result-validator modules and the shared JCS/schema runtime
  implementations are included too. Changing any included element requires a new pinned
  fingerprint/version review

The committed Slice 1 fixture is
[`../fixtures/slice1-bidirectional-en-v1.json`](../fixtures/slice1-bidirectional-en-v1.json), with
structure fixed by
[`../fixtures/slice1-bidirectional-en-v1.schema.json`](../fixtures/slice1-bidirectional-en-v1.schema.json)
and cross-field invariants enforced by
[`../fixtures/slice1-bidirectional-en-v1.semantic.jq`](../fixtures/slice1-bidirectional-en-v1.semantic.jq).
The canonical executable validation path is
[`../fixtures/validate-slice1-fixture.mjs`](../fixtures/validate-slice1-fixture.mjs); consumers do not
claim conformance by running only one underlying layer.

Real credentials, private transcripts, and local absolute paths are forbidden in committed fixtures.
The `*-local` destination classes are runner-only loopback-consumer evidence. Slice 1 production
resolves Claude Code, Codex, and MCP remote/unknown; caller location or model labels cannot select a
local class.

Both protocols use the same JavaScript UTF-16 allocation: clip system from the start to 9,000 units
and call `toWellFormed()`, then clip user from the start to
`max(3,000, 12,000 - clippedSystem.length)` and call `toWellFormed()`. Anthropic Messages sends JSON
content type, fixed `anthropic-version: 2023-06-01`, optional environment-backed `x-api-key`,
`{model,max_tokens,temperature,system,messages:[{role:"user",content}]}`, and concatenates response
`content[]` text blocks. OpenAI Chat Completions sends JSON content type, optional
environment-backed `authorization: Bearer`,
`{model,max_tokens,temperature,messages:[{role:"system",content},{role:"user",content}]}`, and reads
`choices[0].message.content`. Credential `none` sends no authentication header, and the 1 MiB
response limit is enforced before JSON parsing.

## Result record

Every candidate/scenario comparison emits one machine-readable aggregate record containing:

- fixture, candidate, scenario, and runner-evidence case identity
- the domain-separated fingerprint of one runner-owned evidence bundle for the candidate suite
- separate domain-separated fingerprints of the bundle's network-trust and resource-plateau
  evidence objects
- resolved target destination class and effective-manifest fingerprint
- the fixture-pinned execution environment and candidate artifact metadata, each with a
  domain-separated JCS fingerprint
- cold or warm mode
- milestone timestamps and completion state
- `drainConditionId`, `drainStatus`, and `drainTimedOut`
- a boolean before-model injection marker derived from observed injection-acknowledgment and
  target-model-dispatch milestones; otherwise null
- a fixed negative fixture that dispatches the model strictly before injection acknowledgment with
  increasing monotonic times, sets the marker false, and requires a non-eligible
  `scenario_oracle_mismatch`
- runner-bound host-observed Agent/repository/session identity plus three single-field caller-claim
  mismatch decisions; caller claims authorize zero persistence or injection
- captured, committed, duplicate, lost, pending, summary, and durable-memory counts
- observed retry signal delivery, consumed/ignored signal identities, provider-attempt/outcome,
  budget transitions, state, and the exact recovered durable output when applicable
- the payload-free pair-bound identity-conflict receipt, canonical repository/source/stream/event
  identity, repeated-attempt receipt IDs, one durable receipt count, canonical/incoming states,
  reason, preservation flag, and durable-memory delta when applicable
- the closed payload-free failure metadata record when an output-limit rejection applies
- individual zero-tolerance counters for Agent blockage, accepted-event loss, duplicate durable
  memory, secret egress, and incompatible-scope injection
- positive considered-event/candidate/activation denominators plus remote request, payload,
  injection, exact aggregate credential/payload wire bytes, provider cost units, and
  forbidden-sentinel observations for security rejection scenarios
- expected-injection recall, expected-omission match, and forbidden-fact count
- attempted and delivered rendered bytes/tokens, selection elapsed time,
  input/traced/deadline-unprocessed/admitted/selected candidate counts, and per-item source lane and
  selection reason
- exact attempted/final canonical-JCS UTF-8 render payload evidence plus pinned renderer/tokenizer
  identity and ordered token-ID records; aggregate byte/token counts are recomputed from this evidence
- no pack-compilation-failure field in Slice 1; a candidate that cannot emit a within-envelope final
  pack cannot fill that positive suite slot, while Slice 2 owns an explicit zero-delivery refusal
  artifact and lifecycle
- all 22 ordinal latency runs, discarded-run markers, event-ordered capture samples, applicable
  warm/cold injection samples, per-run repository/session namespaces and ordinal-scoped event IDs,
  and recomputed nearest-rank P95 aggregates
- process-tree samples, resource plateau, queue depth, and storage growth
- effective provider cost units when known
- healthy, degraded, failed, unsupported, or not-run disposition with reason
- effective profile and bounded safe recovery action for pending or failed work

Each result file or stdin record is limited to 1 MiB and read incrementally before UTF-8 decoding or
I-JSON parsing. One path-only runner evidence bundle is limited to 1 MiB. Suite mode rejects any
positive/negative path count other than the fixed 16+1 shape before opening candidate result paths.
The bundle is staged in a candidate-inaccessible immutable root disjoint from the artifact root and
binds fixture, candidate, environment, artifact, runner invocation, and scenario identity. The
validator requires a runner-supplied current invocation ID and rejects group/other-writable roots or
files; the reference runner executes the candidate under a distinct sandbox identity without access
to that root.

The authoritative format is
[`alpha-result-v1.schema.json`](alpha-result-v1.schema.json), with cross-field rules in
[`alpha-result-v1.semantic.jq`](alpha-result-v1.semantic.jq) and executable fixture/result checks in
[`validate-alpha-result.mjs`](validate-alpha-result.mjs). Runner-specific records are not comparable
until this canonical validator exits 0.

Runner-owned latency intervals, cold/warm preparation receipts, full observed lifecycle milestones,
process samples, network trust, and host-derived identity decisions live in the separately validated
[`alpha-runner-evidence-v1.schema.json`](alpha-runner-evidence-v1.schema.json) bundle. The result keeps
separate network-trust/resource-plateau fingerprints and derived aggregates, not raw copies. An
executed result requires the 12 raw plateau windows and a matching plateau fingerprint; a canonical
`unsupported`/`not_run` no-activity bundle carries `resourcePlateauEvidence: null` and a null result
fingerprint instead of claiming that workload ran. The validator recomputes every present
fingerprint from the bundle. Each case also carries a runner-derived,
domain-separated fingerprint of
the complete schema-validated result observation, excluding only the bundle fingerprint that would
create a cycle. This binds egress, render, atomicity, and conflict evidence without duplicating
private payload into the runner bundle. A candidate-authored hash or source label is not evidence.
The network object binds `summary.stub.invalid`, literal `127.0.0.1`,
`summary-repaired.stub.invalid`, a hostname/IP-valid
public CA SHA-256, normal chain/hostname validation, and `privateKeyCommitted=false`; no private key
is committed. Its six raw receipts prove base/local/repaired setup and daemon preflights with exact
hostname, remote SNI or null IP SNI, endpoint port, timeout, per-run CA/peer-certificate fingerprints,
duration, verification, and zero HTTP/auth/payload activity. Each real scenario also carries one
runner-owned provider-egress observation spanning candidate start through process-tree termination;
its network gate opens only after a direct durable-store authorization carrying explicit ordered
committed event IDs, their count, and their set fingerprint, and binds the earliest
request interval, provider/location, request/payload/auth aggregates, and source bytes by sensitivity.
Those sensitivity bytes are measured by the runner-owned stub from the request bytes it receives and
the fixture's fixed synthetic source markers/spans; they are never derived from allow policy or a
candidate result field. Their four-bucket sum cannot exceed the observed provider payload bytes.
The same receipt independently records restricted payload bytes and forbidden-sentinel observations;
both must be zero and match the result aggregates.
The projected late-injection negative references the base observation instead of fabricating a run.
Every fixed retry/redirect recovery subcase owns one additional runner observation, including a full
zero-egress observation for each no-op. The sorted wrappers bind exact case/manifest/provider,
globally unique receipt IDs and fresh process-tree roots, the bundle invocation, committed-event
authorization, request timing, TLS, wire bytes, and sensitivity measurements under the same rules as
the initial attempt. Each nested receipt repeats the owning initial/recovery case ID. Live
invocation/root IDs are runner-generated per execution/process generation, never reused PID labels.
The plateau object binds all 12 ordered, non-overlapping duplicate/no-op workload receipts and their
strict workload/drain/checkpoint/sample timestamps, and separately
fingerprints maximum-increase-from-first and final-five-span predicates before any aggregate can
affect eligibility. Malformed ordering, receipt, no-op, or zero-delta evidence is invalid; a
structurally valid threshold miss remains inspectable and yields `resource_threshold_exceeded`
rather than an invalid record. It also binds candidate, artifact, environment, runner invocation,
and one fresh plateau process-tree root that is unique from every observed provider root.
Cold runs require opaque data-root, reset-receipt, and process-generation identities that occur only
once across the entire bundle, including warm records, plus observed zero process and directory-entry
counts within one pinned process-sample interval before measurement. The first cold observation must
also begin within that interval after the declared run start. Warm runs require one retained
data-root/process generation and a fresh ready-process observation.
The late-injection negative is a runner-generated fixture projection, not an independent benchmark
run. It has its own runner-evidence `caseId`, preparation identities, and full observed lifecycle
milestones, while deliberately reusing the base case's latency/resource observations. The validator
requires that exact projection, and the negative never contributes to candidate performance samples.
The committed regression corpus executes the canonical suite branch with all 16 positive records,
the projected negative, and one 17-case runner bundle.

A single `--result` validates one inspectable scenario record only. Candidate comparison requires
suite mode: pass one `--result PATH` per positive scenario and the required late-injection record as
`--negative-result PATH`. The validator requires the positive scenario-ID multiset to equal the
complete fixed fixture, a common candidate/environment/artifact identity, and every positive record
to be comparison-eligible. The negative record applies `beforeModelNegativeFixture` to its named
base scenario; injection must not precede model dispatch, and the record must match the fixed failed,
non-eligible disposition.

The schema and semantic rules define the Alpha v1 vocabulary for Slice 1, including retry signals
bound to computed provider/manifest fingerprints. Each consumed signal creates one one-shot grant;
configuration activation does not refill the automatic retry limit. The current executable validator is deliberately bound to that
fingerprinted fixture bundle; neither the schema nor validator is a generic fixture-plugin interface.
Slice 2 and Slice 3 add their own fingerprinted fixture validator, reuse compatible core fields, and
version-review any new retry family rather than weakening the Slice 1 evidence shape.
Slice 1 applies the shared safety, latency, resource, and eligibility gates above. It also gates the
fixed fixture's final byte/token ceilings and exact attempted/final render evidence. Slice 2 owns
explicit zero-delivery compilation refusal, admitted-candidate, selected-item, lane-allocation,
multi-profile, and generic compiler boundary cases; the Slice 1 gate does not claim those compiler
capabilities are implemented.
The schema owns structure, the jq layer owns fixture-independent record arithmetic and ordering, and
the executable validator alone derives fixture-oracle matches, thresholds, failure priority, and
comparison eligibility; consumers do not combine partial verdicts from those layers.

`environmentFingerprint` is SHA-256 over the execution-environment object with domain
`free-mem:alpha-execution-environment:v1\0`. `artifactFingerprint` uses the same JCS construction
over artifact metadata with domain `free-mem:alpha-candidate-artifact:v1\0`. The validator requires
the environment pins/descriptors and artifact base commit to match the fixed fixture. The artifact
manifest itself is JCS-hashed with `free-mem:alpha-artifact-content:v1\0`; that digest must equal
`contentSha256` before the metadata fingerprint is accepted. Each manifest digest is also recomputed
from the regular file bytes below the candidate's realpath-contained artifact root; self-declared
file hashes are not accepted as artifact evidence. `entrypoint` names one listed file. The harness
owns one immutable staged artifact snapshot from candidate execution through result validation;
the validator requires runner ownership with no group/other write access, rejects
non-regular/unlisted entries, and hashes each open file descriptor between stable-stat checks. The
fixed artifact boundary is at most 64 files, 128 total filesystem entries,
eight directory levels, 16 MiB per file, and 64 MiB total; traversal and hashing are incremental,
use a fixed 64 KiB content buffer, and reject each boundary before unbounded work.
The runner-evidence root is separately realpath-contained, non-overlapping with that artifact root,
and read through the same nonblocking/no-follow regular-file and stable-stat boundary.
`networkTrustEvidenceFingerprint` uses domain
`free-mem:alpha-network-trust-evidence:v1\0`; `resourcePlateauEvidenceFingerprint` uses
`free-mem:alpha-resource-plateau-evidence:v1\0`. Both hash canonical JCS evidence and must match the
runner bundle before the result can be eligible.

## Comparison rules

- Candidates are compared only after their equivalent drain condition completes or times out.
- `drainTimedOut=true` is always a non-success `failed` or `degraded` disposition. Its record remains
  inspectable but is excluded from successful candidate comparison and cannot pass completion,
  quality, or resource gates. Periodic process observation must reach the pinned timeout boundary;
  its orphan-process field is the last timeout observation rather than teardown proof. Safety
  counters and zero-tolerance security evidence remain independently required to be zero.
- After a completed drain, a positive `deadlineUnprocessed` count is non-eligible with result reason
  `selection_deadline_exceeded`; it is not a quality-threshold failure. `drain_timed_out` remains the
  higher-priority reason when the drain itself times out.
- Selection elapsed time that reaches or exceeds the profile deadline has the same
  `selection_deadline_exceeded` result and delivers zero items, bytes, and tokens. Attempted size may
  exceed the envelope during deterministic pruning only when a valid final pack remains; final
  rendered byte/token values always gate eligibility. Slice 1 has no zero-delivery compilation-failure
  result. Slice 2 must define that refusal's own evidence and lifecycle before adding it.
- Selection elapsed time is recomputed from `target_selection_started` and
  `target_selection_finished` in the fixture-pinned lifecycle; the separate timing record must match
  those observed boundaries exactly. Scenarios without a selection lifecycle record no candidate
  counts or selection timing.
- Run ordinals are exactly 1 through 22, ordinals 1-2 are discarded, and the remaining 20 runs feed
  nearest-rank P95. Capture samples bind the fixed event order and flatten across the 20 measured
  runs within that scenario; warm/cold injection contributes one sample per measured run only for
  scenarios named by the fixture metric. A missing sample, false aggregate, or threshold miss is
  non-eligible with `latency_threshold_exceeded`.
- Actual injected items must exactly match `expectedInjectedItems` in order and in every bound field:
  memory, lineage, revision identity and ordinal; fact; memory kind; source event identities; source
  lane; and selection reason. Actual omissions must likewise match the expected active revision and
  source lane. Explicit pre-pruning render items equal the ordered delivered prefix plus only the
  `omitted_budget` suffix; `same_as_final` is valid only when no budget pruning occurred.
- Exact render evidence covers the complete canonical destination wrapper and InjectionPack,
  including pack, target-session, repository, destination-policy, manifest, degradation, provenance,
  and revision identity. Byte and token totals are derived from that UTF-8 payload, not an item-only
  projection.
- Provider extraction order is not assumed to equal injection order. Fixture validation compares
  provider outputs to dispositions as a multiset; the whole-fixture fingerprint pins the declared
  `expectedInjectedItems` order, and the runtime result must match that order exactly.
- `summaryCount` is included in `durableMemoryCount`; it is not an additional durable entity count.
- Unsupported, not-run, failed, and degraded are distinct states.
- `unsupported` and `not_run` are canonical no-activity records with an empty milestone list: all operation, evidence, resource,
  item, and token counts are zero; retry/failure/operational evidence is absent; and their reasons are
  `capability_unsupported` and `owner_slice_not_run`, respectively. Their plateau object and result
  plateau fingerprint are null. They cannot wrap a failed or executed run.
- The output-limit scenario binds raw authoritative writer receipts and one durable-observer sample
  at every lifecycle milestone across the pinned processing-through-teardown window. The validator
  derives committed batches, item mutations, maximum visible derived items, and forbidden-sentinel
  observations from those records; a terminal zero count or copied aggregate alone is insufficient.
  A timed-out prefix cannot reach the provider-rejection terminal or teardown window and therefore
  records null atomicity evidence instead of fabricating the completed observation.
- Safety counts for Agent blockage, accepted-event loss, duplicate durable memory, secret egress,
  and incompatible-scope injection must be zero.
- Their fixture-bound operation/event/candidate denominators are an independent zero-tolerance
  boundary and must pass even when a higher-priority timeout, latency, resource, or quality failure
  already makes the result non-eligible.
- Agent-blockage and accepted-loss zeroes are backed by each scenario's non-empty lifecycle and
  committed-event denominator; duplicate-durable-memory zero is backed by the spool scenario's
  positive duplicate replay count. Secret-egress and incompatible-scope zeroes require the matching
  `securityOracle` positive considered-event/candidate denominator and an absent forbidden sentinel
  at the protected destination.
- The two HTTP configuration-rejection scenarios end before capture and therefore have zero
  accepted/committed-event denominators. They do not use `acceptedEventLossCount=0` as safety
  evidence; their positive activation-proposal denominators and zero credential/request/transmitted-
  byte evidence prove pre-send rejection instead.
- For the deterministic redirect stub only, the initial request wire body is exactly the event's
  UTF-8 `redactedPayload`. Its positive configured-endpoint byte count and zero redirect-location
  request/byte/resend counters are independently recorded.
- Credential/payload byte evidence is the fixture-pinned aggregate across the configured-endpoint
  attempt set. Allowed verified-HTTPS attempts must match that exact aggregate: remote attempts may
  record fixture-pinned credential and payload bytes; local providers record zero credential bytes
  but may record fixture-pinned payload bytes; rejected activations record both zero. The remote
  ProviderChoice remains `external_metered`; the fixed
  runner records exactly zero provider cost units because its deterministic stub is runner-owned,
  not because of provider cost class.
- Remote request/payload counts cover the initial drain attempt set only. Every independent recovery
  case records fixture-pinned request, payload, credential-byte, payload-byte, restricted-byte, and
  forbidden-sentinel evidence for that attempt; no-op cases record six zeroes. Redirect recovery
  additionally records exact zero request/payload/resend evidence for the rejected Location. The
  initial count is one unless the fixture explicitly pins an exhausted attempt count, and zero for
  rejected or local-provider routes.
- Resource or quality thresholds are frozen before candidate results are inspected.
- Raw records remain available beside the human summary.

Slice 1 candidate terminal reasons are the permanent minimal subset `exact_session`, `lexical`,
`recency`, `duplicate_revision`, `omitted_budget`, and `omitted_ineligible` from the authoritative
InjectionPack enumeration. The separate pack-level degradation reason is `semantic_disabled`; it is
never recorded as a candidate terminal reason. Slice 1 does not emit the remaining Slice 2-owned
lane-minimum, candidate-cap, semantic-scoring, or full-trace reasons.
