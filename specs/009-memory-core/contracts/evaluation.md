# US3 T021/T022 evaluation implementation plan
## Boundary and verdict
This plan covers only evaluation correctness required before T023/T024 ranking or semantic work.
It uses the current US1 source receipts, B1 repository-scoped native identity, injection ledger, and
existing isolated-agent answer checker. No retrieval algorithm or dependency changes are needed.
The current harness is not valid real-provider evidence. It can steal a worker lease, silently
continue after fixed waits, resolve sessions without repository identity, classify ready/pending
from its own hold flag, count only current stdout for recall, and report no stage at which a fact
was lost. `why` shows source processing and injection ledgers separately but does not join them.
## Confirmed implementation bugs
| Bug | Current source | Effect |
|---|---|---|
| silent lease wait | `replay.ts:336-351` | timeout and success both return `void`; storage errors are swallowed |
| forced lease ownership | `replay.ts:372-386,803-819,1131-1136` | live owner can be overwritten, then a later owner can be cleared |
| fixed readiness wait | `replay.ts:791-801,1115-1129` | 45/60 s expiry still reaches report; provider envelope can exceed 60 s |
| global/wrong terminal predicate | `replay.ts:389-405` | all repos/sessions are counted and US1 fallback `summary_state=pending` never becomes “ready” |
| inferred timing class | `replay.ts:880-915,1028-1060` | `pendingHold` intent can disagree with the exact persisted injection |
| repository collision | `replay.ts:407-429`; `replay-evaluate.ts:133-167` | another repo with the same agent/native ID makes the current repo unreadable/ambiguous |
| current-pack-only recall | `replay.ts:763-771,939-971` | a valid earlier delivery in the same conversation/epoch is scored missing |
| boolean-only report | `replay-evaluate.ts:251-381`; `replay-report.ts:221-237,393-517` | capture/coverage/application/retention/retrieval/delivery/answer cannot be separated |
| unjoined diagnostics | `why.ts:68-159,292-358` | source outcome cannot be followed to a memory and its later delivery |

`isolated-lifecycle-state.mjs:17-36,87-89` has the analogous repository issue: it filters by agent,
returns stored `native_session_id` instead of `COALESCE(original_native_session_id,native_session_id)`,
and selects the first matching native ID. Its current one-repository-per-home fixtures hide this.
## T021 implementation
### 1. Carry exact repository/session identity
- Resolve `run.repoId` once from the temporary replay repository and add `repoId` to
  `MeasureInput`/test fixtures.
- After each session-start/end/fact/recall hook, resolve the internal session with
  `(repo_id, agent, COALESCE(original_native_session_id,native_session_id))`; zero or multiple rows
  is explicit invalid evidence.
- Scope `startInjectionCount`, `lastClassification`, lifecycle lookup, compaction rows, and barrier
  queries by `repoId`. Remove the cross-repository ambiguity count in `startInjectionCount`.
- In `conversationLookup`, query the replay repo only and key by repo+agent+original native ID.
  Keep internal `session.id` as the foreign-key/delivery locator.
- Update the existing collision test at `replay-evaluate.test.ts:108-122`: repo A must use A's
  lifecycle row and pass even when repo B has the same original native ID; repo B data must not
  change A's result.
If isolated lifecycle inspection is made multi-repository, pass an exact repo ID into
`inspectLifecycle`, scope all four queries, and display native IDs with `COALESCE`. Otherwise state
explicitly that its isolated home is not the same-native cross-repo test owner.
### 2. Replace replay lease mutation with existing fenced helpers
- Replace constant `LEASE_TOKEN`/boolean `leaseHeld` with `leaseToken: string|null`.
- Poll `claimLease(db,{pid:process.pid,now})`; it already uses `BEGIN IMMEDIATE`, rejects a live
  owner, handles SQLite busy, and returns a unique token (`worker/lease.ts:35-52`).
- While held, call `heartbeat(db,token,now)`. A false result is `lease_lost`; do not reclaim.
- Release only through `releaseLease(db,token,()=>true)` and require `released`; `lost` preserves
  the foreign row. Delete all unconditional UPDATE/CLEAR SQL and the final five-second lease steal.
- Acquire replay's hold before its first capture, retain it through each session, and reacquire
  after an explicitly spawned worker has exited. Acquiring for the first time at SessionEnd can
  deadlock behind a legitimately active worker waiting for that very end event. This controlled
  replay measures its own real worker processes; hook-spawned counts may be zero. Automatic native
  spawning remains a separate real-agent gate. Do not weaken the production 20-minute worker rule.
- When fixture sessions overlap, defer draining until active sessions have no unbatched pending
  summarizable rows. Otherwise a worker may legitimately wait for a later fixture end event while
  replay is waiting for that worker. Keep ended target IDs until that drain can run.
  A pending active session outside the fixture invalidates the run with `foreign_active_sources`:
  the production worker drains the whole store, so excluding that row from this check would cause
  a real wait for an end event that this replay cannot supply.
- Acquisition timeout invalidates evidence (exit 1); non-busy/unusable storage stays exit 3.
  Diagnostics contain fixed state/IDs/pid, never prompts, provider bodies, credentials, or secrets.
### 3. Use an exact persisted worker barrier
Track internal IDs of sessions whose `SessionEnd/session_shutdown` was just captured. Mirror the
poll/throw style of `waitForLifecycleState` (`isolated-lifecycle.mjs:35-61`), but keep the helper in
TypeScript and query only those target IDs.

A target is settled for this worker invocation when all are true:

1. session exists in `run.repoId` and is ended;
2. no target batch is `pending` or `running`;
3. no target receipt remains `assigned`;
4. no target summarizable row is unbatched with `processing_state='pending'`;
5. `summary_state='no_content'`, or `summary_updated_at` is non-null and no completed target batch
   is newer than it;
6. the spawned child exited and its pid no longer owns `worker_lease`.

Worker exit 0 or 1 proceeds to these predicates; exit 1 alone is not a successful-generation claim.
Exit 3 invalidates storage evidence; a signal/missing status or any other exit invalidates the run.

Condition 5 intentionally accepts US1's terminal degraded case: a fallback/waiting source may leave
`summary_state='pending'` while a current temporary summary has `summary_updated_at`. `waiting`,
`deferred`, `uncovered`, `rejected`, `excluded`, and `legacy_unknown` remain visible stage outcomes;
they are not silently called successful application. A child that exits with unfinished pending
rows/batches fails the barrier.

Use a named safety deadline covering the current four-request worst case (two 60 s attempts in each
of the initial and language-retry provider calls) plus margin, e.g. five minutes. The deadline is
only failure cutoff; persisted state is success. Return `settled|timeout|storage_error` with target
states. `driveRun` must not call `measureRun` after timeout/storage failure.

### 4. Classify ready/pending from the injection created by that hook

Snapshot injection IDs for the exact internal session before and after an eligible start (after
Pi's explicit inject). Store `injectionId`, state, degraded reason, and elapsed time in the sample.

- pending: exact row has `degraded_reason='summary_pending'`;
- ready: exact row exists without that reason;
- unclassified: zero/multiple new rows; fail timing evidence.

Only native events that should build a start pack enter these two classes. Claude's explicit
`fork` and known resume events do not build a new pack; their wall time stays in the ordinary
300 ms hook samples. Missing records for an eligible start remain a failure.

Keep `pendingHold` only to arrange the fixture. Verify the pending sentence in the correlated pack;
for Grok, correlate the later `PreToolUse` pack to the same pending injection rather than its line
number alone.

### 5. Record recall probes; evaluate after the run

Replace immediate `RecallHit.hit` writes with `RecallProbe` containing fact/query seq, expected text,
repo/internal source and query session IDs, conversation ID, epoch, current injection IDs, the set
of injection IDs already present before the query, and current pack text hit.

For a `tags.fact` line, snapshot scoped raw-event IDs before/after capture. Retain all new IDs and
identify the fact-bearing row among them; this handles Pi lines that normalize to multiple events.
For a recall line, snapshot prior injection IDs before the hook and current IDs after it. Grok keeps
the probe open through its correlated `PreToolUse/PostToolUse` so final delivery state is observed.
Freeze the prior fact-bearing included items and their confirmed delivery before the query, and
require the expected text in the corresponding printed pack. An earlier pending injection that is
only confirmed later is not prior delivery; an included memory whose printed excerpt omitted the
fact is not proof that the fact was available.

A prior delivery counts only when it was present before the query, has the same conversation and
context epoch, its fact-bearing `injection_items` row is `included`, and its injection is emitted.
Normal emitted packs may have `delivery_count=0`; Grok deferred requires `state='emitted'` and
`delivery_count>0`. Different repo/conversation/epoch, planned/omitted items, and attempted Grok
packs never count. Report current delivery and prior delivery separately; recall availability is
their union.

## T022 stage accounting

Create one conservative `FactTrace` per probe. Each stage has `status`, fixed `reason`, bounded IDs/
ranges/counts, and never a provider body.

| Stage | Pass evidence | Failure/pending evidence |
|---|---|---|
| capture | scoped fact source ID was accepted | absent, failed classification, excluded secret |
| request coverage | merged receipt ranges cover the recorded source total | no receipt, assigned, not_sent, partial ranges |
| application | latest receipt explicitly accounts for source | deferred/rejected/uncovered/unaccounted; explicit `no_memory` stays distinct |
| retention | active fact-containing memory links to source; raw or independent evidence state is stated | no linked fact, tombstoned/superseded, only temporary fallback |
| retrieval | current planned/included item or same-context prior included item names fact memory/raw source | no candidate, or stored omission reason such as threshold/budget/MMR/stale/retired/duplicate |
| delivery | fact-bearing item belongs to confirmed current/prior emitted injection | built/pending/attempted/omitted/not_delivered; Grok count zero |
| answer | actual receiving-agent output contains expected fact | always `not_run` for fixture replay |

Full coverage is the non-overlapping union `[0,total)` across attempts. Partial is reported as
partial with exact ranges; do not guess that the expected substring was sent. A fact-bearing
injection item may use either `memory_id` or `raw_event_id` (summary-pending activity).

JSON `recall.probes[]` should include `availability=current_delivery|prior_delivery|missing`, all
seven stages, and `firstFailure`; `stageCounts` aggregates each status. Keep Japanese, English, and
overall availability rates. Markdown adds one stage table and includes first failure/reason in the
miss table. `answer:not_run` is neither pass nor delivery failure.

### `why` trace

Extend each bounded source row with a repository-scoped chain:

```text
observation_batch_sources.raw_event_id
  -> memory_sources.memory_id
  -> injection_items.(decision,reason)
  -> injections.(id,session_id,conversation_id,context_epoch,state,delivery_count)
```

Search deliveries across later sessions of the same repo, not only the source session. Return at
most 100 sources and a small capped list per source, with truncation flags. Human/JSON output uses
IDs and existing allowlisted reason/state codes. Do not emit source evidence/content, memory bodies,
provider output, credentials, paths outside the existing scoped view, or unknown free-form reasons.

## Answer owner and existing E2E seams

- Fixture replay runs hooks/worker only, so it must report answer `not_run`.
- `isolated-lifecycle-state` currently proves ledger inclusion, which is delivery evidence, not an
  agent answer (`isolated-lifecycle-state.mjs:208-234,281-319,379-401`). Do not relabel it as answer.
- Actual answer scoring already exists in `assertAgentOutput` and `evaluatePairRecall`
  (`isolated-agent.mjs:45-64`; `isolated-user.mjs:200-215`). Reuse that result for the answer stage
  in T024 real-agent evidence. If lifecycle results also claim recall, retain the seeded fact set and
  score the final stdout/pane through the same helper as a separate assertion.

## Meaningful RED tests

1. Live foreign lease remains byte-for-byte unchanged after acquisition timeout; releasing a lost
   replay token cannot clear its successor.
2. Fake barrier remains pending past 60 s and settles at 65 s without stealing; timeout prevents
   report rendering. A non-busy DB error returns storage failure.
3. Completed fallback batch + waiting source + updated degraded summary settles; an unbatched
   pending source or stale summary does not.
4. DB row says `summary_pending` while hold=false => pending; ready row while hold=true => ready;
   absent/ambiguous row => unclassified failure.
5. Repos A/B share agent/original native ID. With `MeasureInput.repoId=A`, A's lifecycle/resume/
   injection counts pass and B cannot contribute. Reverse for B. Replace the current ambiguity test.
6. Current pack omits `f-en-06`, but an earlier emitted included memory in the same conversation/
   epoch contains it => prior-delivery pass. Repeat with different epoch/conversation/repo => miss.
7. Grok attempted/count0 => undelivered; emitted/count1 => delivered. Normal emitted/count0 remains
   delivered.
8. Stage table seeds: missing source; assigned/no range; partial/full range union; not_sent;
   unaccounted output; explicit no_memory; applied memory missing expected text; retained fact with
   no injection item; MMR/budget/threshold omission; included but not delivered; current delivery;
   duplicate plus prior delivery. Assert JSON counts and Markdown first reason.
9. `why` joins source->memory->later-session delivery in the same repo, excludes a foreign-repo row,
   caps/truncates results, and maps an unknown reason to `other` without printing its text.
10. Replay answer is `not_run`. A pure isolated-agent result with all facts passes answer while one
    missing fact fails answer without changing retention/retrieval/delivery statuses.

Use migrated temporary SQLite fixtures and injected clock/sleep/process snapshots; no real wait,
provider, model, or native agent is needed for T021/T022 unit tests.

## Minimal file map and completion

- `replay.ts`: repo/session locators, fenced lease, target barrier, persisted timing samples, probes.
- `replay-evaluate.ts`: repo-scoped lookup, prior-delivery and fact-stage computation.
- `replay-report.ts`: stage/current/prior JSON and Markdown.
- `why.ts`: bounded source-to-memory-to-delivery trace.
- `replay-evaluate.test.ts` plus narrow `replay.test.ts`: database accounting and orchestration REDs.
- isolated lifecycle files: only repo scoping and explicit answer assertion if those reports claim it.

T021 is complete when timeout/lease conflict blocks measurement, session lookup is repo-exact,
timing uses the persisted injection, and same-context prior delivery affects availability. T022 is
complete when reports/why separate every stage and answer remains explicitly unrun unless an actual
agent response was checked. Ranking fixes and semantic dependency choice remain T023/T024.
