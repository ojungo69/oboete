# Current work checkpoints

This is the concrete B2 contract reviewed after the B1 work-binding increment. The checkpoint is
separate from ordinary knowledge accounting. A broad progress summary cannot acknowledge detailed
source material that the provider failed to extract or explicitly dismiss.

## One provider response, two independent decisions

The existing observer request adds `checkpoint_context` with state `none`, `withheld` or `provided`.
`provided` contains the current memory ID and its complete canonical title/body. Keep this existing
stored representation in the input: do not parse display text back into structured fields and do
not add a second JSON copy of the checkpoint to storage.

The response adds a structured choice beside `observations`:

```ts
checkpoint:
  | { decision: 'replace'; purpose: string; constraints: string[]; decisions: string[];
      outstanding: string[]; source_event_ids: string[]; reason: string }
  | { decision: 'unchanged'; source_event_ids: string[]; reason: string }
```

Each choice cites a nonempty subset of the actual request's source IDs and gives a nonempty reason.
Replacements are full snapshots of purpose, constraints, decisions and outstanding steps. Use
bounded strings/arrays and reject a replacement whose canonical title/body exceeds the existing
120/2,000-character memory limits. Never silently truncate a prior checkpoint to make it fit.
Missing or malformed required checkpoint output makes the provider result unusable, preserving
the existing retry opportunity. A withheld parent permits only `unchanged`.

Only ordinary observations and explicit ordinary no-memory decisions settle source portions.
Checkpoint citations are provenance, never input to `outcomeForSource`. Shape validation does not
prove semantic retention; real-provider preservation is a separate T041 gate.

## Storage and publication

Reuse `memories.type = session_summary`, `memory_sources`, validity fields, `sync_conflicts` and the
existing injection ledger. Add to unshipped migration 0005:

- `work_items.current_checkpoint_memory_id`
- `memories.work_id` and `memories.checkpoint_parent_id`
- `memories.provenance_complete` and bounded inherited root/path metadata in existing
  `memory_sources` rows with no raw ID, citation or evidence body. Ordinary generated memories
  share this proof and direct privacy dependencies under [generation privacy](generation-privacy.md).
- Batch checkpoint decision, expected parent, result memory ID, fixed reason and bounded cited
  source-ID JSON. Both replacement and unchanged choices retain their admitted subset independently
  of ordinary source outcomes. Unchanged never adds the new source as parent memory evidence.

Checkpoint revision identity includes repository, work, expected parent and material. Confirming
identical current content reuses the current ID. Returning from state A through B to A creates a
new revision so immutable parent links cannot become cyclic. A tombstone for the same work/material
still suppresses every revision of that content. Ordinary knowledge retains its current identity
domain. Work and checkpoint provenance must enter transfer v2 before shipping;
the v1 reader remains supported.

Before sending, Oboete records the expected parent itself. The model cannot choose that pointer.
Prepare and detect replacement content before the existing lease-fenced apply transaction. Inside
that transaction, re-read the resolved batch/work binding, insert/find the checkpoint, retain new
source evidence and compare-and-set the work pointer against the expected parent. Retire the old
parent only after successful publication. Inherited evidence remains reachable through parent
links; copying every old evidence row into each new checkpoint is unnecessary. Copy only a
canonical union of privacy roots/paths, including inherited file citations, so future admission
does not traverse the whole ancestor chain. At most 50 roots and 2 MiB of canonical metadata fit;
an unknown or larger union is marked incomplete and withheld from providers. Never label a clipped
union complete. Parent links are immutable after work assignment. A stricter parent classification
propagates monotonically to every descendant; secret transitions also clear their retained evidence.

Competing independently based replacements retain both candidate memories and record an open
conflict in `sync_conflicts`; current progress is not chosen by timestamp. Same-content provider
confirmation can upgrade a temporary checkpoint in place only after its cited source groups are
processed. Tombstones remain authoritative. Unchanged decisions retain fixed receipts.

## Capture ordering

CAS prevents concurrent overwrite, but an old backlog must also be prevented from rolling back
current progress. Compare only the newest capture time of newly cited sources against the current
checkpoint's source time. Preserve the existing conservative treatment of unknown source times;
do not mix the parent's time into candidate eligibility.

An old or undated candidate is historical: keep its evidence, point `superseded_by` at current,
set a non-active validity interval and record `capture_time_order`. It cannot move the pointer or
create a competing-progress conflict. Equal source times are permitted, so portions of one source
can advance C0 → C1 → C2. After a CAS miss, apply the ordering guard to the actual current pointer
before deciding historical versus sibling conflict.

After a valid publication, stored source time is the maximum of parent and new source times.
An imported undated checkpoint is inactive until explicit resume. That operation may set its
activation `valid_from` as an ordering floor while retaining unknown source time as provenance.
New sources after activation can advance it; old retained backlog cannot.

## Budget and privacy

Reserve room for the complete allowed parent before paging new source events into the existing
12,000-character request. Nearby previews use what remains after source allocation. A checkpoint
that cannot fit is not clipped; processing remains retryable. Exact source coverage/cursors retain
their US1 semantics independently of checkpoint citations.

Parent admission reuses destination rules, source/path provenance, full-text detection, directive
checks and the final policy fingerprint. Immediately before each send/retry, verify pointer,
content hash/text, sensitivity, validity, deletion and bounded provenance state. A withheld parent sends no state text
and cannot be replaced by that provider. New checkpoint sensitivity is the strictest of parent,
newly cited sources and output detection. Legacy learned-title summaries must also retain the
strictest sensitivity of the contributing memories, closing the earlier baseline concern.

## Temporary guidance and readers

With no current checkpoint, fallback may create a labeled temporary checkpoint from the selected
work purpose and binding-scoped activity. With a parent, fallback keeps it intact and offers only
that work's bounded recent activity under the existing degraded label. Provider recovery can
replace or confirm temporary guidance atomically. Session end, inactivity and Git integration
never mark a work item complete.

Session-start and prompt injection use the selected binding and current work pointer. An unresolved
selection contains stable choices and applicable knowledge, with no candidate checkpoint or raw
activity. Exclude session-summary rows from ordinary project ranking; explicitly add only the
selected current checkpoint. Non-current/conflicting checkpoints are available through deliberate
history/status inspection. Preserve conversation/epoch memory-ID delivery deduplication.

CLI/MCP/why report current checkpoint, parent, temporary/unchanged/historical/conflict outcome and
explicit completion. Conflict selection is an explicit operation. It never silently abandons the
other progress or revives a deleted checkpoint.

## Verification

Reader implementation: the common memory scope excludes session summaries by default. An exact
selected work permits only its current pointer; explicit `history` permits retained non-current
rows while keeping deletion, repository, review and sensitivity filters. CLI/MCP accept an optional
current-context binding token. Without a token, only a unique active work in that context is
selected; native-session recency is never a substitute. Work status returns the current checkpoint
and fixed batch outcomes with the same privacy scope. A checkpoint remains historical after deletion.

Injection resolves the calling native session's current binding and checks its repository/context
root. Pending activity is limited to six allowed captured prompts/tool calls of that selected work.
The parent remains available while new sources wait. Ambiguous choices use engine-authored framing,
opaque binding/work IDs and allowed purpose text, consume the ordinary pack budget and undergo the
same final detection even in choice-only packs. No new memory/source ledger kind is needed. Before
recording a built pack, re-read selection and current checkpoint in the existing write transaction;
the privacy guard revalidates the exact included source bodies. Appended activity alone does not
invalidate a pending pack: the new tool call may be Grok's delivery carrier. Pi binds its session before injection
without inferring a new purpose from an unclassified prompt.

Pi forwards its extension-generated current prompt ID even on the first start injection. Before
including progress, wait within the hook budget for that exact source to be classified and attached
to the session's current binding. A missing, failed, secret or stale source yields knowledge only;
never infer purpose from the inject command's plaintext. Recheck the source binding at publication.

### Removed worktrees and current path policy

Keep the source context and the current resume context separate. `work_contexts` retains a bounded
snapshot of the last successfully parsed repository-local `secret_paths`; `memory_sources` retains
a nullable source-context reference with exact evidence and inherited privacy metadata. Captured
spool metadata carries the same repository-rule snapshot. Credentials, home policy and consent are
never snapshotted; they remain live inputs. Unknown, malformed or oversized metadata stays withheld.

The shared provenance-policy check first verifies the current binding's repository and local context
key. A source root that still exists must match its recorded repository and context key. A replaced
or inaccessible root is not a removed root and remains withheld. Only an absent root may use a
verified current context of the same repository, and only when the selected work originated there
or was explicitly bound there. Keep original provenance unchanged. Detect full content using the
union of the old context's saved repository rules, the current context's repository rules and live
home policy. Check original paths and, when lexically inside the old root, their relative/rebased
forms as well. A row without a source-context reference may use its existing verified live-root
path, but never the removed-root exception.

An absent former root of a moved worktree may follow its recorded context to a live root only
when that root has the identical administration-directory generation. This preserves worktree
move continuity without rewriting the source's original path or accepting a replaced root.

Retain the binding's context ID separately from its mutable current root when processing an older
accepted source. Apply absolute rebased paths under every verified current/source policy root.
A recorded purpose whose source has expired is withheld, rather than treated as source-less.
Reader checks include the emitted unescaped string fields and recheck the inferred active-work
selection after asynchronous detection; a new competing purpose invalidates the prepared view.

Observer parent/nearby admission and injection checkpoint/activity admission use this same check.
The build/send guard includes source provenance, context identity and current detector policy;
provider retries and Grok's actual deferred emission recheck it. Policy changes cancel old text
without weakening detection or relocating evidence. Delivery timestamps and citation cache updates
alone do not invalidate a still-current checkpoint.

Large text or costly rule/path combinations use the existing detector worker and its remaining
hook deadline. A timeout withholds injection; it does not erase accepted source/provenance or mark
it processed. Small checks remain in process to avoid a worker startup for each ordinary pack item.

Once a deferred pack has been printed for an unresolved tool attempt, freeze its text and ledger
membership until that attempt is delivered or dropped. A changed selection prevents any new print;
it cannot erase the earlier attempt's receipt. Incoming packs remain undelivered instead of merging
unprinted items into the carrier. Checkpoint privacy metadata survives ordinary evidence retention.

Exercise checkpoint-only output versus ordinary source completion; whole-parent budget and
privacy withholding; C0 → C1 → C2 over partial pages; same-parent concurrent responses; lease
rollback; old/unknown capture order; strict sensitivity; temporary recovery and same-content
upgrade; selected-work-only injection; and merge/end preserving outstanding work. Run real-model
semantic retention separately; deterministic tests prove invariants, not useful understanding.
