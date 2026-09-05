# Research: Slice 1 Automatic Memory Runtime

## Source audit summary

The delivery order and contracts below were checked against the current pinned Codemem source:

- `daemon-lifecycle.ts` constructs `new ObserverClient()` and starts RawEventSweeper without a
  manifest; `observer-client.ts` rereads Codemem/OpenCode/Agent env/config, appends provider paths,
  and currently lets `fetch` follow redirects by default.
- `setup.ts` has hardened per-file snapshot/atomic-replace helpers but commits editor lanes
  separately, keeps transaction snapshots only in-process, and does not compile/activate a provider
  manifest. `readDaemonHealth` already provides a source-verified running-daemon preflight.
- RawEventSweeper defaults are periodic 30 s, idle 120 s, source limit 100, stuck recovery 5 min,
  retention 0 unless configured, and mutable env/config readers. Existing debounce defaults to 60 s;
  Slice 1 deliberately freezes it at 1 s. `nudge()` exists but has no production caller.
- `stop()` waits active auto-flush promises, but an in-flight `finally` can reschedule pending work
  after stop because `scheduleAutoFlush()` does not fence shutdown.
- `raw-event-flush.ts` uses mutable retry env, changes exhausted work to `gave_up`, and advances
  `last_flushed_event_seq`; Store job status/claim/frontier mutations are separate calls.
- Current reads span Store search/recent/timeline/explain/get, reference queries, daemon RPC, MCP,
  viewer direct SQL, pack traces, maintenance AI, export/import, vectors, and dedup/supersession.
  Project basename/filter values are used broadly but are not repository authority.
- `export-import.ts` serializes `SELECT *` session shells plus user prompts and legacy session
  summaries for sessions selected through project/scope filters; viewer memory routes also expose
  content-bearing artifacts/session projections. These rows need first-class v21 classification and
  destination projection, not only MemoryItem filtering.
- `maintenance/ai-structured.ts` constructs an independent OpenAI observer and logs content excerpts.
  `extraction-replay.ts` reads raw rows and builds prompts, but its only non-test reachability is the
  public core barrel export; the CLI command is explicitly unavailable.

These observations are why a provider-only patch or raw-flush-only filter is insufficient.

## Decision 1: Correct the accepted fixture contract first

**Decision**: Before runtime code, mechanically update the Product Reset capability-manifest
contract, data model, Slice 1 schema/fixture/semantic validator, and bound result examples to the
closed provider/resource shape. Recompute provider, manifest, fixture, result, and runner-evidence
fingerprints and run the complete existing validator suite.

Both `localDerivationManifest` and output-limit recovery become complete immutable successor
manifests bound to the prior fingerprint; scenario-local partial overlays are rejected.

**Rationale**: The pre-PR 0 fixture had `providerKind`, endpoint scheme/host fragments, a free-form
credential string, self-declared provider policy, and missing scheduler fields. Building runtime
against that baseline would have preserved an unbuildable contract or forced a second incompatible
migration.

**Scope note**: PR 0 co-delivers this 006 hardening and the scoped
`specs/005-product-reset/` mechanical correction; it contains no runtime code.

**Alternatives considered**: Runtime compatibility shims would create two sources of truth. Quietly
changing only the fixture would leave schemas, semantic checks, and bound evidence inconsistent.

## Decision 2: Closed protocol plus complete endpoint, not a provider registry

**Decision**: ProviderProposalV1 accepts only `anthropic_messages_v1` or
`openai_chat_completions_v1`, a complete canonical endpoint URL, model identity, and
CredentialRefV1 (`none` or one environment name). The compiler derives location, egress, cost, TLS,
and redirect policy and computes ProviderChoice fingerprint.

Only literal `127.0.0.1` and `[::1]` are local. `localhost` and its subdomains, trailing-dot names,
wildcard or unspecified addresses, and alternate loopback spellings are rejected; DNS-to-loopback
is never guessed. Every other accepted host is remote and HTTPS-only with system TLS; redirects are
rejected manually. The runtime never appends `/messages`, `/responses`, or `/chat/completions`.

ResourceProfileV1 fixes timeout 60,000 ms, input 12,000 characters, output 4,000 tokens, response
1,048,576 bytes, and temperature 0.2. The protocol names fix Anthropic/OpenAI auth headers, request
and response shapes, credential-none behavior, and pre-JSON response bounding; streaming, Responses
API, tier routing, tools, arbitrary headers, and fallback are unsupported.

**Rationale**: Current provider/model discovery, custom header merging, endpoint suffixing, and token
cascades make effective egress impossible to prove. Two explicit wire shapes cover the retained
transport without adding a registry or SDK dependency.

**Alternatives considered**: Built-in providers, OpenAI Responses, arbitrary compatible protocols,
custom headers, Azure-style query endpoints, Agent subscriptions, and automatic credential
discovery are deferred. They require their own closed contracts.

The deterministic stub remains harness metadata and produces a normal proposal. There is no
`fixture` cost class or stub ProviderChoice in production.

The base remote proposal is OpenAI Chat Completions at
`https://summary.stub.invalid/v1/chat/completions` with environment credential
`FREE_MEM_SUMMARY_API_KEY`; remote cost class remains `external_metered`. The local successor uses
`https://127.0.0.1:1234/v1/chat/completions` and credential `none`. One complete repaired-remote
successor at `https://summary-repaired.stub.invalid/v1/chat/completions` replaces the current three
free-form repair labels; configuration, redirect, and downgrade signals bind its computed manifest/
provider fingerprints. Stub cost evidence 0 is runner evidence, not cost class.

The corrected fixture pins complete local and remote stub URLs. Restricted local derivation uses a
fixed literal-loopback HTTPS URL; unauthenticated local HTTP remains credential-none/eligible-only.
Remote uses a fixed HTTPS hostname mapped to the runner-owned loopback service inside its
isolated network namespace. Before candidate start, the runner installs a per-run hostname/IP-
matching public test CA into its private system trust, outside candidate/manifest control.
Production rejects added CA path/environment configuration. The candidate still performs system
chain/hostname verification. The runner records and
binds the generated public CA fingerprint in its evidence; no private key is committed. An
unavailable/mismatched endpoint fails rather than choosing a new URL and changing provider/manifest
fingerprints.

After user confirmation, setup holds the shared lifecycle lock and performs a native 5-second
credential/payload-free TLS chain+hostname handshake before mutation; daemon start repeats it before
provider construction. Setup invalid-chain/hostname cannot activate or send an HTTP request. A
daemon-start outage/certificate failure preserves capture/RPC/spool-import/lexical service, disables
provider/AI processing with a bounded reason, and can recover only through a persisted healthy edge.

## Decision 3: One vertical manifest PR, after the contract checkpoint

**Decision**: The first runtime PR contains the complete usable manifest slice:

- compiler, validation, JCS provider/manifest fingerprints, immutable storage/pointer;
- explicit setup proposal, safe disclosure, confirmation, editor+pointer transaction, rollback, and
  running-daemon refusal;
- one owner-only interruption journal around existing setup snapshots, with `current` published last
  and next-start hash-based finalize/restore that, on any unknown external edit, preserves every
  target unchanged and retains the conflicting journal;
- one shared lifecycle lock and fixed `lifecycle -> setup/spool -> daemon writer` order, with daemon
  state rechecked while held;
- daemon absent/malformed/valid state handling, frozen snapshot, doctor/status projection;
- manifest-only ObserverClient factory/transport without daemon execution yet;
- structured-maintenance and viewer observer/config projections from that snapshot;
- frozen scheduler fields with mutable reads removed.

Daemon consumption cannot merge before setup can create/activate the exact manifest. Full setup
start/attach remains later; the first setup path requires a stopped daemon.

Even with a valid active manifest, PR 1 and PR 2 remain `pending_privacy_boundary`: capture works,
but provider calls, AI maintenance, and RawEventSweeper stay disabled until PR 3 installs every
DestinationBoundary consumer. This prevents an independently mergeable prerequisite from enabling
the existing #130 leak.

**Rationale**: A compiler-only PR is not independently usable; a daemon-only PR cannot start from a
manifest users cannot create. The existing setup snapshot helpers and `readDaemonHealth` allow a
single vertical boundary without a second configuration system.

**Alternatives considered**: Temporarily reading legacy config from daemon/maintenance or creating a
second `config activate` CLI would preserve bypasses. Automatic lifecycle in the same PR would mix a
larger independent concern into the privacy/config root.

No active manifest yields explicit capture-only restricted mode with no provider and no sweeper.
Malformed pointer/reference/fingerprint/shape fails startup; it is not treated like absence.

## Decision 4: Fixed resource fields from current behavior

**Decision**: ResourceProfileV1 includes the accepted fixture envelope plus periodic 30,000 ms, idle
120,000 ms, debounce 1,000 ms, stuck claim 300,000 ms, retention disabled/0, and maximum 100 source
events per newly admitted v21 job.

Preserve the accepted output-limit recovery as a runner-owned test-only fault successor: the same
profile ID, version 2, and every field unchanged except derivation limit 17. Base remote/local/
repaired manifests remain version 1/max16, and production setup exposes no profile selector. This is
not a profile registry or general override surface.

The manifest PR removes mutable provider/scheduler reads and compiles the fields but keeps execution
disabled. v21 processing capacity/retry/derivation becomes enforceable in PR 2; provider/sweeper and
pack limits become enforceable only with the complete privacy delivery in PR 3. Earlier doctor output
reports those states pending.

**Rationale**: Current RawEventSweeper/flush hide these values in env/config or constants. Claiming
one effective manifest while continuing to reread them would be false. A fixed profile needs no
config framework.

**Alternatives considered**: Preserving override env vars creates an undocumented second profile.
Adding a user-configurable scheduler schema is not required for one Alpha profile.

## Decision 5: One schema v21 before privacy activation

**Decision**: Advance v20 to v21 once, after verified backup, and add all Slice 1 event, memory,
repository, lineage, job, claim, admission/attempt provenance, resume, and diagnostic fields. The
schema/job PR lands before complete #130 privacy closure; no later US3 migration is allowed.

**Rationale**: Capture/provider/derived/retrieval enforcement must share first-class values. A
payload-only disposition is caller-controlled, and splitting job state into a later migration would
let #130 close without durable failure semantics.

**Alternatives considered**: Multiple additive migrations, payload parsing, and shadow databases
increase recovery states without benefit.

## Decision 6: Deepen flush batches, with immutable admission and per-attempt provenance

**Decision**: Keep `raw_event_flush_batches` as the sole job. Capacity includes every uncompleted
state, including retry-exhausted. Capture precedes admission; at most 100 source events enter a newly
admitted v21 job, while migration preserves a wider immutable v20 recovery range.

Raw events store the accepted domain-separated payload digest/version. Same identity/same digest is
idempotent only after an atomic strongest-sensitivity/absorbing-quarantine join strengthens the
canonical row and every derived record that cites it. Same identity/different digest atomically
creates or reuses a durable non-success EventIdentityConflict receipt unique to the canonical
identity and ordered digest pair without replacing the canonical row, ACKing, or creating memory.
The v21 migration replaces the legacy source/stream/event unique index with a repository-aware
expression index using `COALESCE(repository_identity,'repo-v1:unknown')`. The sentinel is index-only,
cannot match a valid digest identity, and makes unknown repository collisions fail closed despite
SQLite's normal multiple-NULL uniqueness behavior. A collision is retained outside canonical
`raw_events` as a secret durable quarantine record with redacted payload/digest and non-success
receipt, never silently discarded or normally ACKed.

Admission manifest/provider fingerprints, source range, and retry limit never change. New admission
starts with attempt count 0; successful automatic claims 1-3 each increment lifetime attempt count
and claim generation, and a failed attempt 3 becomes retry-exhausted. A changed configuration
creates new attempt manifest/provider/attempt fingerprints without rewriting admission. Automatic
attempts stop at the frozen limit; a valid resume signal creates one grant consumed by one claim.
The contract correction replaces current configuration-activation budget refills with the same
one-shot 1→0 grant semantics used by every resume reason.

Setup activation receipts are imported once by the v21 daemon; persisted Observer unhealthy-to-
healthy edges and an explicit user-confirmed doctor retry are the other two signal producers.
Global setup/health receipts fan out to the bounded matching retry-exhausted job set; doctor targets
one confirmed job. Per-job signal/grant uniqueness, sequence CAS, insertion, and crash replay are
idempotent.

**Rationale**: The existing table already owns source ranges, attempts, claims, and inspection. The
problem is its `gave_up` cursor advancement and non-atomic completion, not absence of a job framework.

**Alternatives considered**: Resetting attempt count on configuration change obscures history and can
loop. A generic job table duplicates source/frontier state. Timer-only retry remains unbounded.

## Decision 7: Atomic privacy skip and atomic memory completion

**Decision**: One Store transaction validates claim/source/output/provenance, commits either the
content-free privacy skip or every memory/reference/dedup/supersession effect, completes the batch,
and advances the contiguous frontier once. Any failure commits none and retains sources.

**Rationale**: Current code separately calls ingest, batch status update, and frontier update. A crash
between them can duplicate, lose, or falsely complete work.

**Alternatives considered**: Reconciliation after partial commit is larger and weaker than using the
existing SQLite writer transaction.

## Decision 8: Never blindly recover legacy `gave_up`

**Decision**: Migration audits each exact legacy range without lowering the frontier. Complete
retained ranges become explicit retry-exhausted recovery candidates marked frontier-already-
advanced. Missing/ambiguous ranges become terminal content-free `legacy_unrecoverable` dispositions,
consume no capacity, and never claim successful recovery.

**Rationale**: Current `gave_up` has already advanced the session cursor and retention may have
removed sources. Rewinding to a prior sequence could replay later completed ranges or invent gaps.

**Alternatives considered**: Global session rewind and synthetic missing events violate durability
and provenance. Deleting the row destroys inspectability.

## Decision 9: Exact repository identity, never project basename authority

**Decision**: Resolve a domain-separated repository digest from a verified canonical Git remote. If
none exists, resolve and realpath the primary Git anchor through linked-worktree metadata and hash
that. Canonical remotes retain HTTPS/SSH transport class and a bounded exact SSH username, so
different transport authorities or SSH users never collapse. Unknown stays NULL.
Project/basename/workspace values remain display/filter metadata. The current canonical remote is
revalidated at capture and every restricted boundary; an origin A→B change cannot reuse A's cached
authority, and a failed revalidation falls back to a current verified anchor or unknown.

**Rationale**: Current `resolveProject`, normalized events, MCP defaults, and export filters use
basenames or caller strings. Two unrelated repositories can share them; linked worktrees can have
different paths for one repository.

**Alternatives considered**: Raw absolute paths leak local structure and differ through symlinks.
Caller-provided remotes are forgeable. Cross-transport normalization and dropping the SSH username
can merge distinct server authorities. One global local scope does not enforce same repository.

## Decision 10: One DestinationBoundary eligibility seam for every content consumer

**Decision**: Add one small closed DestinationBoundary value, one eligibility function, and the
matching SQL predicate. Carry compiler/runtime-derived `providerPeerTrust` so unverified local HTTP
and verified local HTTPS are distinguishable without mutable fingerprint lookup. Route provider
flush, structured maintenance, search/recent/timeline/explain,
reference queries, daemon get/search/pack, MCP direct/indexed reads, viewer raw-event/status/usage
and content projections, lexical/semantic pack/trace, export/import, and dedup/supersession through
it before content materialization.

Claude Code, Codex, and MCP are remote/unknown production destinations. Their local client process
or caller model label is not on-device-model authority. Local destination classes remain
runner-owned fixtures backed by a verified loopback consumer observation.

Unknown sensitivity is secret; unknown repository denies restricted disclosure. Viewer is
eligible-only unless daemon supplies verified repository context. Semantic candidates are rehydrated
from first-class DB fields before the same check.

**Rationale**: Filtering only raw flush fixes one symptom but leaves derived and direct-read paths.
Post-render filtering leaks through previews, traces, token counts, logs, export JSON, or model
prompts.

**Alternatives considered**: Per-surface privacy helpers drift. A generic policy engine is excessive;
the decision table has four sensitivities and three destination/scope states.

## Decision 11: Remove unused public extraction-replay and distill exports

**Decision**: Remove extraction-replay and distill exports from the core barrel in the vertical
manifest PR while adapting internal benchmark/tests to the closed ProviderChoice transport where
applicable. Keep those sources test-only. Any later production/public exposure must accept
DestinationBoundary before raw/memory read or prompt construction.

**Rationale**: Source search found no production caller and the CLI command reports unavailable. A
boundary retrofit solely for an unavailable public API is more code than deleting the reachability.

**Alternatives considered**: Leaving an unrestricted public export makes the all-consumer claim
false. Deleting the internal benchmark would discard useful tests unnecessarily.

## Decision 12: Projection and exact source-bound derivation

**Decision**: Stable-filter events before building session context/transcript/prompt/request. Provider
output gives every item one direct ordinal-based citation child. The Store claim transaction creates
and privately binds the exact `ProjectedSourceSetV1`; completion revalidates the boundary/raw rows,
maps ordinals to exact IDs, and normalizes optional UTF-8 byte spans over canonical `redactedPayload` into
Product Reset `{eventId,startByte,endByte}` anchors. A newly admitted v21 job projects at most 100
events; a migrated legacy recovery job may project its wider immutable actual range. Each output
inherits the strongest sensitivity and exact repository identity; missing/forged/drifted/mixed
citations reject all output. Provider-backed no-claim ingest is fail-closed, while historical NULL
provenance remains readable only as secret/unknown.

Normalized spans establish source anchors and participate in lineage/dedup before semantic-kind
classification. PR4 memory kinds and presentation consume this PR3 seam without redefining it.
Ineligible pack traces are aggregate and content-free; eligible injected items keep source/reason
evidence.

**Rationale**: Filtering at `fetch` is too late, and provider-authored sensitivity is a downgrade
path. Batch-wide fabricated provenance cannot satisfy the fixed runner.

**Alternatives considered**: Treating every batch event as every item's source fabricates evidence.
Letting providers choose disposition breaks the trusted boundary.

## Decision 13: Existing sweeper, with production nudge and stop fence

**Decision**: Inject a nudge callback into daemon RPC and call it only after a new accepted event
commits, including spool replay. Use fixed 1 s debounce; session-end/pre-compact request immediate
bounded work; prompt pack may drain only relevant accepted work within the hard deadline.

Add a stopped/stopping fence so nudge, timer callbacks, and an in-flight flush `finally` cannot
schedule work after stop. Stop still waits active work and a later explicit start clears the fence.

**Rationale**: The current sweeper already has shared flush coordination but is unwired. Its pending
reschedule after stop is a real lifecycle race. A new scheduler duplicates state.

**Alternatives considered**: Periodic 30 s plus 120 s idle misses normal Agent switches. Blocking
capture violates no-agent-blockage.

## Decision 14: Full setup lifecycle follows manifest and privacy foundations

**Decision**: After manifest, v21 jobs, and privacy boundaries merge, reuse background `serve start`
for one managed runtime. Setup may then coordinate stop/activate/start or attach, install Claude and
Codex by default, and accept success only after version/fingerprint/doctor checks.

**Rationale**: This makes the manifest PR independently mergeable and avoids daemon consumption
before activation, while still completing the Product Reset one-flow UX.

**Alternatives considered**: A second supervisor duplicates PID/ownership state. Treating manual
`serve start` as final setup violates the eventual automatic UX.

## Decision 15: Content-free operational surfaces and semantic-disabled retention

**Decision**: Remove content excerpts from logs/maintenance progress and ensure diagnostics contain
only codes/counts/fingerprints/state/next action. `semantic_disabled` prevents vector use but never
deletes stored vectors.

**Rationale**: Current maintenance warnings can echo narrative text; diagnostics and traces are
reachable disclosure paths. The constitution forbids deleting durable semantic state merely for a
resource target.

## Decision 16: Fixed runner, not a harness platform

**Decision**: Add one candidate-inaccessible runner bound to the committed English fixture, exact
host/runtime pins, normal provider proposals created from stub metadata, and existing validators.
Missing pins or incomplete runs emit no success.

The runner denies non-loopback networking for external-egress-disabled cases, requires restricted
bytes sent to prohibited remote or unauthenticated-HTTP destinations to remain zero, records
expected authenticated loopback request/credential/eligible/private/local-only payload bytes, and
uses ordinals 1-22 with 1-2 discarded and nearest-rank p95 per metric. Its resource gate runs 12
duplicate/no-op windows with strict non-overlapping workload/drain/checkpoint/sample timestamps;
windows 8-12 must have constant process count,
zero drained queue, identical item/token counts, RSS span at most 16 MiB, storage span at most 65,536
bytes, concurrency at most 2, and zero post-teardown orphan process.

The corrected runner-evidence schema binds every raw plateau window, drain/checkpoint receipt,
item/token/concurrency sample, hostname/IP-valid public CA fingerprint, six raw base/local/repaired
setup/start TLS receipts with exact remote-SNI/null-IP-SNI/timing/result/trust-anchor/
phase-stable-peer-cert/
zero-byte evidence with setup completion strictly before daemon-start beginning and invocation-bound
network/receipt evidence. The plateau binds candidate/artifact/environment/invocation and a fresh
bundle-unique process root. One runner-owned gated provider-egress observation per real scenario
carries authorization with explicit canonical-order committed event IDs/count/fingerprint. Each
plateau window has a unique workload receipt, strict runner-monotonic action/sample order,
one identical positive duplicate-attempt count, no-op outcome, and
zero memory/job deltas. Results bind the bundle objects through separate fingerprints and derived
aggregates; source-byte sensitivity buckets never exceed the observed payload bytes, and runner-owned
restricted-payload bytes/forbidden-sentinel observations remain zero. Canonical
unsupported/not-run no-activity output uses a null plateau object/fingerprint rather than reusing an
executed workload. The runner also emits repeated same-event-ID/different-digest attempts that reuse one
pair-bound receipt, rejects cross-pair reuse, and emits exactly 16 positives plus the required
late-injection negative; result and runner-bundle fingerprints cover all fields.
Each fixed retry/redirect recovery subcase adds a sorted case/manifest-bound full observation under
the same rules, including full zero-egress observations for no-op subcases and globally unique
initial/recovery receipt and process-tree IDs. Each receipt binds the bundle invocation and its owning
case ID plus fresh per-execution/process-generation runner root; a reusable PID is not identity
evidence.

For remote-provider cases, the deny boundary permits only the fixture-pinned remote hostname mapped
to the runner loopback stub and validates the evidence-bound CA/hostname. This remains a normal remote HTTPS
proposal to the candidate; it is not a production stub registry or insecure TLS bypass.

**Alternatives considered**: A generic fixture plugin, provider registry, or eight-hour soak belongs
later. Source-only synthetic tests do not prove packed real hooks.
