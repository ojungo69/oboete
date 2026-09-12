# Privacy inherited from generated context

The B4 security review reproduced privacy loss in update, add and checkpoint output. A provider
can use any text it receives while citing only a different current source. Source citations prove
which portions it claims to account for; they cannot authorize a lower privacy classification.

## Actual input determines privacy

For successful provider output, use the engine's actual admitted input:

- Raw rows whose existing coverage has `end > start`.
- The existing `ApplyInput.nearby` subset actually sent in the request.
- An additional provided-checkpoint ID only when its complete text was sent.

All generated or confirmed memories and replacements inherit the strictest current sensitivity of
those inputs and their complete source contexts. Re-read memory sensitivity inside the apply
transaction. Preserve the target/current memory's own prior floor. Deterministic fallback reads
only its declared raw sources and retains its existing policy. An unchanged checkpoint remains
unchanged; it does not acquire new context from that request.

The model's `source_event_ids`, explicit no-memory decisions and existing source coverage keep their
semantic/evidence role. Additional privacy dependencies cannot acknowledge an uncited source or
pretend that a provider retained its facts.

## Bounded metadata, original evidence

Extend the unshipped 0005 migration with `memory_sources.context_only` and nullable
`source_memory_id`. Each output records direct admitted raw IDs and provided memory IDs as privacy
dependencies, without copying ancestor evidence or every ancestor raw ID. Self references are
omitted; repeated dependencies are idempotent. No dependency may cross the batch repository.

Retain a canonical flat union of root/path/context metadata for admission. Reuse the checkpoint
limits of 50 contexts and 2 MiB; unknown or larger unions fail closed. Generalize the unshipped
`checkpoint_provenance_complete` column to `provenance_complete` so ordinary generated memories and
checkpoints use the same rule. Legacy ordinary memories retain their current conservative reader;
new incomplete provenance never falls back to the legacy source-less allowance.

Reads inspect the flat union, never recursively walk history. Extend the existing monotone
checkpoint privacy trigger to ordinary memory dependencies as well. A recursive `UNION` closes
over direct memory dependencies and checkpoint parents even if confirmation creates a cycle.
Changing an ancestor to secret quarantines descendants and clears retained source payloads in the
same transaction. Lesser labels never lower a descendant's classification.

Same-content confirmation may extend an already referenced memory's flat proof. Merge its new
canonical union into existing descendants in that same apply transaction; unknown or overflowing
proof invalidates the whole dependent lineage. Iterate rows without loading the graph into JS.
Source exclusion traverses tombstoned ancestors too, without clearing their tombstones.

Privacy-only rows participate in privacy checks and source reclassification. They are excluded
from citation/evidence readers, empty evidence-slot reuse, source-completion accounting, raw-purge
evidence requirements, session/timeline attribution and transfer citation output. The original
evidence remains on its original memory; privacy references alone cannot keep a full raw event
forever or claim a source as newly retained knowledge.

## Verification

The six public worker/reader regressions cover update/add/checkpoint followed by path restriction
or raw-source exclusion. Add same-content confirmation, mixed-sensitivity/uncited inputs,
provided versus withheld checkpoint, multi-generation propagation/cycles, bounded unions, raw
retention and context-only attribution tests. Preserve existing source coverage, lease rollback,
checkpoint publication and no-copy evidence checks. Validate the shared consumers with typecheck,
focused tests and independent security review before closing the finding.
