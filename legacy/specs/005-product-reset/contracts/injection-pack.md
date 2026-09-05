# Contract: InjectionPack

## Purpose

Define the stable, explainable context product supplied to Claude Code and Codex.

## Inputs

- target Agent/model destination class, session, and repository scope
- active capability manifest and its destination-policy map
- normalized candidates from `exact_session`, `lexical`, `semantic`, and `recency` lanes
- hard time, admitted-candidate, byte, item, and token budgets
- manifest-defined per-lane minimum and maximum budgets

## Selection behavior

The compiler uses one monotonic hard deadline and checks it at entry, before and after every
candidate operation, before and after stages 2-10, and around each render/tokenization attempt.
Expiration at any point aborts pack delivery and returns no context with an out-of-band deadline
reason; later stages never continue past the deadline.

1. Resolve the concrete destination class against the manifest policy map, then traverse supplied
   candidates in stable input order until the time cutoff. Each reached candidate is classified.
   Candidates outside repository scope, deleted, superseded, secret-bearing, or otherwise
   ineligible are not admitted, but remain in the trace with `omitted_ineligible`. Missing or
   unknown destinations are remote/ineligible for private and local-only data. Either disposition
   requires an explicit matching on-device policy in the same repository scope and is never rendered
   to a remote provider or off-host destination.
2. Normalize lane scores without erasing lane identity.
3. Keep only the active revision of each memory lineage, then deduplicate repeated candidates for
   that revision using stable precedence.
4. Establish one total comparison key used by every later ordering decision: lane precedence
   `exact_session > lexical > semantic > recency`, then normalized score descending, then revision
   order, then stable memory identity.
5. Apply the hard admitted-candidate cap before allocation using that total key. Eligible overflow
   is not admitted, records
   `candidate_limit`, and never enters render-time pruning.
6. Allocate each lane's minimum in precedence order without exceeding any global budget. If the
   sum of minima cannot fit, lower-precedence minima receive no allocation and every affected
   candidate records `lane_minimum_not_funded`.
7. Enforce each lane maximum. Fill remaining global budget using the same total key from step 4.
8. Render with the resolved destination renderer and measure exact UTF-8 bytes and
   destination-token count, including wrappers, provenance, escaping, and degradation metadata.
   If any hard byte, token, or item limit is exceeded, remove items in reverse final selected-item
   order from steps 6-7, record `omitted_budget`, and rerender until every exact limit is met.
9. Finalize `packId`, record, and deliver only after the exact rendered output is within all limits.
   If the zero-item envelope exceeds a limit or exact measurement is unavailable, emit no pack and
   no context; report a bounded out-of-band compilation failure reason.
10. Record exactly one terminal inclusion or omission reason for every traced candidate, including
    ineligible, duplicate, candidate-limit, and render-budget omissions. If the time budget stops
    classification early, the untouched deterministic suffix is never selected and is represented
    by `deadlineUnprocessedCount`; `tracedCandidateCount + deadlineUnprocessedCount` must equal
    `inputCandidateCount`.

## Terminal candidate reasons

This is the complete authoritative enumeration. Each traced candidate records exactly one value:

- included: `exact_session`, `lexical`, `semantic`, or `recency`;
- omitted before allocation: `omitted_ineligible`, `duplicate_revision`, or `candidate_limit`;
- omitted during allocation/rendering: `lane_minimum_not_funded` or `omitted_budget`.

`deadlineUnprocessedCount` is an aggregate for candidates never traced and is not a candidate
reason. Pack-level degradation such as `semantic_disabled` is also not a candidate terminal reason.

## Output requirements

- version and pack identity
- target destination class, resolved destination policy, and manifest identity
- ordered rendered sections and source memories
- exact final-rendered bytes and destination tokens; input, traced, deadline-unprocessed, admitted,
  and selected-item counts; and elapsed selection time
- per-item provenance and `sourceLane`
- exactly one terminal reason from the authoritative enumeration for every traced candidate
- semantic or provider degradation and the fallback used

Claude Code and Codex renderers may differ in syntax but must preserve the same selected facts,
order, provenance, and degradation meaning.

## Failure behavior

- A missing semantic lane uses lexical and recency lanes and marks semantic degradation.
- A time budget expiration returns no pack and no context. Diagnostic state retains terminal reasons
  already recorded plus the aggregate untouched suffix count, without continuing compilation. A
  positive `deadlineUnprocessedCount` appears only in that diagnostic record; every delivered pack
  has a zero count.
- A scope or sensitivity validation failure excludes the candidate and is never overridden by
  relevance score.
- Compilation failure returns no fabricated context and must not block the Agent.
