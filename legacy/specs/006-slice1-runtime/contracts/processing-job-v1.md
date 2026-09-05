# Contract: Slice 1 Processing Job v1

## Ownership and admission

The existing `raw_event_flush_batches` row is the only durable summary-processing job for its
immutable source/stream/sequence range. No second queue or generic job framework exists.

Capture persists before admission. One newly admitted v21 job contains at most 100 contiguous source
events. A migrated v20 recovery job preserves its immutable legacy range even when the old
configurable worker admitted more than 100; migration never splits or truncates that range. Capacity
25 counts every uncompleted job (`queued`, `processing`, `failed`, and `retry_exhausted`). At
capacity, later accepted events remain durably and visibly not admitted; no event or existing job is
evicted.

Admission starts exactly at `frontier + 1` and rejects any non-contiguous selected range as
content-free `source_gap` without creating a job or advancing the frontier.
Once admitted, a later gap beyond the frozen job range does not block that exact retained prefix;
a gap or identity break inside the frozen range prevents the job from being claimed.

Admission freezes `admission_manifest_fingerprint`, `admission_provider_fingerprint`, source range,
`retry_limit=3`, and `attempt_count=0`. These fields never change, including after configuration
activation or resume, except that each successful claim increments the lifetime attempt count once.
The admission and attempt manifest fields store the exact frozen manifest
`configurationFingerprint`; neither recomputes a distinct manifest hash.
They are required for new jobs; NULL is allowed only as honest legacy-unknown admission provenance
and is never backfilled from the current manifest.

## State machine

```text
queued -> processing -> completed
                    -> failed -> queued       (automatic budget remains)
                    -> retry_exhausted
retry_exhausted -> queued                     (one validated grant)
processing -> completed                       (atomic privacy skip)
processing -> failed                          (stale-claim recovery)
```

The database spelling is `retry_exhausted`; public fixture/status evidence projects it as the
existing `retry-exhausted` string.

Timer scheduling may retry `failed` only while the automatic budget remains. `retry_exhausted`
never transitions because time passed. The initial claim is attempt 1; automatic claims are allowed
only while the pre-claim count is below 3, so attempts 1, 2, and 3 are the complete automatic budget.
A failed attempt 3 enters `retry_exhausted`; a fourth or later claim requires a one-shot valid grant.

## Claims, attempts, and stale workers

A claim transaction requires `queued` plus either remaining automatic budget or one unconsumed
resume grant. It consumes the grant when present, increments monotonic lifetime `attempt_count` and
`claim_generation`, copies the active attempt manifest/provider fingerprints, computes a new
`attempt_fingerprint`, and enters `processing` atomically.

The fingerprint domain is `free-mem:processing-attempt:v1\0`; inputs are job ID, immutable source
range, new attempt count, claim generation, attempt manifest fingerprint, and attempt provider
fingerprint. A completion with a stale claim generation or fingerprint commits nothing.

A changed configuration changes only the attempt fields/fingerprint. It never resets attempt count,
rewrites admission provenance, widens the source range, or silently adopts a new retry budget. A
resumed attempt that fails returns directly to `retry_exhausted` and needs another explicit grant.
The runner-owned closed output-limit recovery fault successor may change the attempt derivation limit
from 16 to 17; production setup does not expose it and admission profile/limit remains unchanged.
The Store accepts limit 17 only when the full validated version-2 manifest matches the claim's
configuration and provider fingerprints, a one-shot resume grant is pending, the manifest's base
fingerprint names the job's prior max-16 version-1 attempt, and the provider is unchanged from that
attempt. The pending grant's durable accepted signal must target that exact manifest/provider and
must be `validated_configuration_activation`; a doctor or health grant cannot be repurposed.
An unbound limit, an attempt-1 use, or a non-exact successor is invalid before claim mutation.

## ResumeSignalV1

A per-job signal has a unique ID, producer receipt ID, exact target job ID, monotonic per-job
sequence, target role `summary`, target provider fingerprint, target manifest fingerprint, and
exactly one `kind`:

- `validated_configuration_activation` (manifest/provider fingerprint must change);
- `recorded_provider_healthy_transition` (daemon-recorded unhealthy to healthy edge);
- `user_confirmed_doctor_retry`.

A valid signal creates one grant with `state=pending` only when no grant is already pending. `(job_id, producer_receipt_id)` and
`(job_id, signal_id)` are unique. The claim changes the grant to `consumed`, records
consumption time, and increments the attempt in the same transaction. Duplicate/out-of-order,
wrong-job, wrong-role, wrong-provider, unrelated-component, unchanged invalid configuration, and already-
consumed signals update only the closed last-signal disposition as durable content-free no-ops and
consume no attempt.
No producer may create a grant unless the job's immutable source range is still retained exactly.
Setup and provider-health fanout skip an invalid range; an exact doctor action fails as a stale
snapshot. The job remains `retry_exhausted` with no pending grant and is not advertised as a doctor
retry target until the source-range fault is repaired.
Claim resolves the pending grant's accepted target by `(job_id, resume_grant_id=signal.grant_id)`,
not by the latest observed signal ID; a later no-op cannot retarget or strand the grant.

An exact `(job, producer receipt)` replay validates the stored per-job signal and global producer
receipt identities, then returns the existing signal as `duplicate` before mutable
attempt/grant snapshot checks. A different otherwise-valid signal received while a grant is pending returns `grant_pending` without
inserting/consuming a signal or producer receipt and without changing sequence, grant, or attempt.
The durable producer may retry after the existing grant is consumed and its attempt terminates; no
second slot, overwrite, or hidden queue exists.
The first global setup/health fanout targets only retry-exhausted jobs and jobs whose resume grant is
already pending; ordinary queued, processing, or failed jobs are not frozen. Eligible exhausted jobs
apply immediately without waiting for pending siblings. Exact receipt replay re-evaluates only jobs
in the receipt's frozen, sorted target-ID set that lack a signal from that receipt; a frozen job that
later becomes queued, processing, or failed remains blocked until retry-exhausted. The producer
returns `grant_pending` while any target remains blocked, increments cumulative `fanout_count` as
those targets apply, and then replays as `duplicate`. Jobs created later are never eligible.

Only three durable producers exist:

- the v21 daemon imports one setup activation receipt written under the lifecycle lock and emits one
  idempotent `validated_configuration_activation` signal per matching retry-exhausted job;
- a persisted provider state edge from unhealthy to healthy emits one
  `recorded_provider_healthy_transition`; repeated probes do nothing;
- an explicit user-confirmed doctor retry RPC/CLI action emits one `user_confirmed_doctor_retry`
  for exactly the displayed job after displaying its component and fingerprints.

Activation producer sequences form one global monotonic series. An exact receipt replay returns
`duplicate` after its frozen set is complete. If a greater activation sequence is imported while an
older receipt remains partial, the older receipt's remaining fanout returns `stale`, preserves its
prior fanout count, and performs no signal or job mutation. An unseen receipt whose sequence is not
greater than the greatest imported activation sequence returns `stale` with zero fanout and performs
no receipt, signal, or job mutation. Provider health sequences remain scoped to their
manifest/provider state edge and do not use this global fence.

The exact-job Doctor projection keeps the nullable prior attempt fingerprints separate from a
`retryTarget` derived from the daemon's frozen active manifest. Legacy NULL values display as
`legacy_unknown`; confirmation compares that exact nullable attempt snapshot, count, claim
generation, state, and grant state before targeting the active fingerprints. With no active target,
the action is `activate_valid_manifest`, not `confirm_retry`. The grant leaves legacy admission and
attempt provenance NULL; the next successful claim writes attempt provenance atomically.

Operational status exposes the ascending IDs of at most 25 `retry_exhausted` jobs. Human status
prints the same IDs, allowing the user to select one before the exact-job snapshot and confirmation.

Setup/health receipts are global producer events; one sole-writer transaction fans each out to all
currently matching jobs, necessarily at most 25 under the global uncompleted-job capacity. Job
state, `resume_grant_state != pending`, target role/provider/manifest, and
`incoming.sequence > preLastConsumedResumeSequence` are one CAS. Accepted signals alone set
`postLastConsumedResumeSequence=incoming.sequence` and create a grant; sequence gaps are allowed,
while equal/stale values are no-ops. Receipt IDs and unique job bindings fence crash replay. Setup never
opens the canonical database.

Changed configuration also grants exactly one claim; it does not refill the automatic retry limit.
Any fixture/result field representing budget-after-grant is therefore 1 and becomes 0 after claim.

Configuration activation, redirect recovery, and HTTPS-downgrade recovery use the one complete
repaired-remote successor's computed manifest/provider fingerprints. Healthy transition and user
retry use the computed active-base fingerprints; output-limit recovery uses the computed test-only
v2 manifest/provider binding. Free-form configuration labels are not signal authority.

Fixture and runtime evidence maps each case to its exact producer kind and target job; the kinds are not an
unordered interchangeable set. `observedTransition.lastConsumedSequence` is the post-transition
value. Every accepted transition requires `preLastConsumedSequence < signal.sequence`,
`observedTransition.lastConsumedSequence=signal.sequence`, automatic `budgetBefore=0`, `budgetAfterGrant=1`,
and `budgetAfterAttempt=0` for the consumed signal. `ignoredSignalCount=0` is required only when the
case delivers no ignored signals; an aggregate case may count stale/duplicate signals alongside one
accepted transition. Any consumed-signal mismatch is a durable no-op, not a grant.

## Atomic terminal transitions

A content-free `eligible_only` completion may bypass the provider only when every projected event is
either a trivial prompt or a session start/idle/end lifecycle event; lifecycle-only ranges are
allowed regardless of length. Adapter errors, unknown events, and substantive prompts are never
classified by this shortcut; they remain retryable until a later boundary handles or explicitly
rejects them.

**Memory completion** validates the live claim and its Store-private `ProjectedSourceSetV1`
association, host-resolved ordinal citations, normalized Product Reset
`{eventId,startByte,endByte}` anchors into canonical `redactedPayload`, output count, repository identity,
sensitivity, lineage, dedup/supersession, and attempt provenance. Provider text never supplies an
authoritative raw-event ID or repository. Completion re-loads the raw rows and re-evaluates the
compiled destination boundary before one database transaction commits every memory/reference/index
source record and completes the job. It advances
the contiguous event frontier exactly once only when `frontier_already_advanced=false`; a recovered
legacy `gave_up` range leaves it unchanged. Crash or validation failure commits none.
For durable flush claims, session-end metadata is part of this same transaction; a failed attempt
never runs the legacy transaction-external session cleanup.
Derived dedup is valid only for an exact repository/normalized-span identity returned by the
Store-owned derivation context; a caller-supplied projection or unbound dedup completion is rejected
atomically. A crashed process loses the private projection association, so its provider response
cannot commit and the recovered job must obtain a new claim.

**Privacy skip** validates the live claim and exact all-ineligible projection. One database
transaction stores a content-free diagnostic and completes the job. It advances the frontier exactly
once only when `frontier_already_advanced=false`, with zero provider request and zero memory output.

The completed privacy skip is terminal for that source range. Configuration change does not reopen
it; a later explicit user-authorized replay contract is outside Slice 1.

Failure, retry exhaustion, output overflow, partial parse, out-of-set citation, and stale claim retain
source events and do not advance the frontier.

## Legacy completed crash window

The v20 success path could commit a completed job before its separate frontier update. Migration
advances through such completed rows only when the next row starts exactly at `frontier + 1`, every
source sequence in its immutable range is retained, and no legacy batch overlaps it. The transaction
continues through a contiguous completed chain and leaves an already-advanced frontier unchanged.
An incomplete, overlapping, gapped, or sessionless candidate aborts migration; no completed row or
committed memory is replayed, deleted, or reclassified, and the frontier is never rewound.
An already-advanced v20 completed range may lack source rows after the legacy prune command; it is
preserved as completed without replay and does not require source reconstruction.

## Source retention

Slice 1 fixes raw-event retention disabled/0. A future non-zero policy is invalid unless purge
deletes only at/below the committed frontier, excludes every source sequence referenced by any
uncompleted job including retry-exhausted work, and never deletes accepted not-yet-admitted backlog.
Backup/restore and export/import preserve job and source provenance as defined by their boundaries.

## Legacy `gave_up`

Migration never rewinds `last_flushed_event_seq`.

- Exact complete retained range: migrate to `retry_exhausted`, mark
  `legacy_recovery_state=complete_range` and `frontier_already_advanced=true`; explicit recovery may
  commit memory but never advances the frontier again.
- Missing, non-contiguous, overlapping, or ambiguous range: mark completed with
  `completion_disposition=legacy_unrecoverable`, retain a content-free
  `missing_or_ambiguous_range` diagnostic, and create no grant. It consumes no capacity and is never
  reported as successful recovery.

No synthetic event, blind session rewind, or replay across missing ranges is permitted.

Other legacy uncompleted rows with complete sources preserve attempt count, use NULL
`legacy_unknown` admission fingerprints, migrate to retry-exhausted, and require one valid grant. The
grant populates attempt provenance only.

If all recoverable legacy rows would exceed the global capacity of 25, migration aborts atomically
and leaves schema 20 unchanged. It never truncates, silently completes, or exempts overflow rows from
capacity; the operator must reduce legacy uncompleted work with the prior runtime before retrying.

## Diagnostics

Job/status/doctor output contains only bounded state, reason code, counts, safe fingerprints, attempt
count, claim generation, grant state, capacity, and one closed next action (`none`,
`activate_valid_manifest`, `configure_credential`, `wait_for_capacity`, `confirm_retry`,
`restart_daemon`, or `upgrade_runtime`). It contains no event/memory content,
title, path, prompt, query, source excerpt, provider response, sentinel, or credential value.
Operational `raw_events.source_gaps` inspects at most 25 pending streams, ordered by session update
time and stable source/stream keys, and reports only the resulting count from 0 through 25. A
positive bounded-scan count sets `next_action=upgrade_runtime`; no stream or event identity is
projected.
For each selected stream, the diagnostic continuity scan covers its complete pending sequence range
and continues past repository and repeated-event-ID admission boundaries, so a later missing sequence
remains visible without blocking an earlier exact prefix job.
If the pending aggregate or bounded gap scan cannot run, `raw_events.available=false` and
`next_action=upgrade_runtime`; any pending count already collected remains visible without content.
This `nextAction` field is separate from fixture-only `expectedOperationalStatus.safeAction`; the
fixture field is an operational oracle with its own value set, not this seven-value enum.
