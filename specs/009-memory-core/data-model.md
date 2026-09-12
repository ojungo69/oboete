# Memory and continuity data model

Keep the existing database and migration 1-3 hashes. Add later entities only with their consumers.

## Accepted sources and processing — increment A

`raw_events` remains the acceptance point for sanitized activity. Processing completion is
independent of `classification_state`: successful secret detection does not prove extraction.
`captured_at` stays immutable; full-source expiry starts at successful processing.

Persist source portions considered by an attempt and their result. Identity is the accepted
event plus deterministic portion range/version; whole events use the whole range. Request caps
cannot complete an unsent range. Diagnostics contain IDs, ranges, counts and fixed codes, not
provider bodies or credentials. Empty output does not acknowledge the original batch.

An accepted portion is pending, considered with an explicit outcome, or excluded by a named
privacy/classification rule. Recoverable failure, omission or rejected output stays pending.
Applied/noop/irrelevant/suppressed outcomes are inspectable. A source is processed after all
admissible portions have outcomes. Supporting portions remain with the memory's provenance.

`observation_batches` still groups one session/destination, with bounded retry metadata.
Fallback describes temporary output, not successful source completion. Future retry dates or
missing consent must not keep the worker spinning. Rerouting checks each source under current
rules. Source progress, memory effects, receipts and temporary-guidance retirement commit under
the same lease fence. Tombstones and content hashes remain authoritative. Migration can recover
only legacy raw material still present, not data already purged.

### First implementation checkpoint

Migration 0004 adds `raw_events.processing_state` (`pending`, `waiting`, `processed`,
`excluded`, `legacy_unknown`), `processed_at`, `retry_after` and `processing_attempts`. New inserts default
to pending, so capture/spool records need no version change. Surviving sources belonging to old
terminal batches become legacy_unknown; already queued/unbatched sources continue pending.
Legacy-unknown sources are held for an explicit reprocessing/migration choice, not automatically
sent as a new paid backlog merely because the schema was upgraded.

`observation_batch_sources` stores `(batch_id, raw_event_id)` membership and a fixed-code
outcome/reason/time. Its raw ID is not a foreign key, so the receipt survives raw expiry.
Every batch is attempt history. Settling fallback/uncovered/rejected sources clears their current
batch attachment and puts them in waiting; the next due attempt reroutes each row under current
destination rules. Backoff starts at five minutes, doubles per settled attempt, and caps at
24 hours. A changed model still uses that bounded schedule and the existing consent/cost gate.

A fully represented source is processed only when at least one accepted provider observation
accounts for it and no cited observation was rejected. An explicit noop with a nonempty reason
is a no-memory decision; omitted output is not. Partial/omitted request coverage never completes
a source. This is processing evidence, not a claim that the model retained every useful fact.
Supporting raw referenced by active knowledge is held conservatively until checked evidence
portions are stored independently. No physical purge of that evidence is enabled beforehand.

Native `summary_state` describes completion readiness, not work-item completion. Temporary
activity summaries may exist while source generation stays pending. Queue release considers due
generation and summaries needing an update, so waiting sources do not create a polling loop.

The same unshipped migration adds a versioned `processing_hash` and `processing_offset` cursor,
and range/hash columns on attempt receipts. Existing `memory_sources` stores independent
sanitized evidence with its original raw ID, range/hash, capture time and completion time; no
second evidence hierarchy is needed. `memories.source_captured_at` records the newest contributing
source time, backfilled when surviving raw proves it. Unknown time cannot authorize overwriting
a newer/undated target. The migration refuses a live worker lease and fences an expired owner
inside its existing immediate transaction. The version-3 worker refuses the new schema; its old
hook lacks a schema-ahead guard. Upgrade hook and worker bundles together. The new hook and worker
both refuse later unsupported schemas.

Captured payload metadata retains the resolved root and sanitized paths that the original
detector checked, including file-result paths absent from the event's normalized tool input.
The spool carries that metadata unchanged. Accepted `memory_sources` evidence also retains its
root and paths, so privacy can be rechecked after raw expiry. The current Git identity must still
match the recorded repository; missing/reused origins stay held. Nearby admission checks these
source/citation paths and rereads memory privacy/tombstone state immediately before sending.
For an absent worktree, the reviewed checkpoint contract adds a source-context reference and the
context's bounded repository-path-rule snapshot. An explicit, verified resume context may then
apply both old and current rules without changing the source root. Existing but replaced roots and
unknown old provenance stay withheld; credentials and home policy are always current.

Provider-generated or confirmed ordinary memories and checkpoints retain `provenance_complete`
and a bounded flat union of all actually supplied source contexts. `memory_sources.context_only`
separates direct privacy references (`raw_event_id` or `source_memory_id`) from retained facts,
citations and source completion. Sensitivity and later context additions reach descendants,
including through tombstoned ancestors, in the apply/exclusion transaction. Unknown or oversized
proof remains withheld; see [generation privacy](contracts/generation-privacy.md).

## Work context and work items — increment B

- Context: stable ID, project, native directory-generation key, root, last-known bounded
  repository path rules and last seen. Unknown generation gets only a session-local binding.
  Local paths and branch names are not cross-device security identities.
- Work item: UUID, origin context, purpose, active/dormant/completed state, current checkpoint
  and revision lineage. Merge, session end and inactivity do not prove completion.
- Session binding: existing session plus context/work item. Conversation ID/epoch retain native
  resume and delivery semantics.
- Checkpoint: purpose, constraints, decisions and outstanding actions with sources. New versions
  supersede old ones; unrelated work is never overwritten by selection.

Ambiguous bindings stay unresolved until selected. Imported sessions remain historical by
default; removing a directory does not delete retained work.

## Visibility and sharing — increment C

Visibility links knowledge to work/project/personal scope without changing sensitivity or
consent. Integration adds project visibility to reusable knowledge and preserves origin work.
It does not promote active progress. Personal shared statements require direct unambiguous user
declarations or approved proposals. Proposals contain a sanitized candidate, origin and pending/
approved/rejected state. Imported/quoted/tool text cannot approve itself. Shared projections
omit project payloads, and source inspection remains scoped.

## Transfer and replicas — increment D

Extend transfer with format version, stable origin installation/record IDs, scope, work identity
and provenance. Preserve hashes, tombstones and quarantine; keep the old export reader.
Unknown project mappings remain unresolved. Imports are previewable, atomic and repeatable.
Replica revisions carry parent revision IDs; repeated delivery is a noop, deletion dominates,
and incompatible sibling progress enters `sync_conflicts`. Clocks are descriptive. Encryption
does not replace validation or authorize promotion.
