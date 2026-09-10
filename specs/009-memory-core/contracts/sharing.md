# Scoped knowledge sharing

US4 / T025-T028. This contract extends [work.md](work.md). Visibility and sensitivity are
independent; no visibility grant overrides detector, destination, tombstone, validity or import
quarantine checks. Source changes are serialized in the existing isolated worktree.

## Audiences and identity

| Audience | Read authority | Stored content |
| --- | --- | --- |
| work | Exact selected work and current repository | Ordinary origin memory and its provenance |
| project | Same repository as the project grant | Adopted/reusable origin memory and its provenance |
| personal | Approved proposal points to the projection | Separate generic statement, without origin payload |

Grants are additive. Missing grants fail closed. Unresolved work selection yields project and
personal knowledge only. `history` widens validity time, never work/repository/approval scope.
Checkpoints stay exclusive to their selected work; ordinary retrieval excludes session summaries
and adds only the selected work's current checkpoint. Checkpoints cannot be shared by adoption.

Add forward migration 0006 with `memory_visibility` and `sharing_proposals`; never change historical
migration checksums. Grants have deterministic IDs and unique work `(memory, repo, work)`, project
`(memory, repo)` and personal `(memory)` keys. SQL CHECK constraints enforce the audience shape.
Existing ordinary non-degraded memories receive their original project grant; bound checkpoints
receive work grants. Fallback/degraded memories receive only their proven batch-origin work grant;
unbound legacy fallback remains stored without visibility. Imported rows retain quarantine, and
unbound legacy summaries remain excluded by ordinary
selection. No default grant or permissive trigger covers new writers.

New deterministic session summaries receive a work grant only when every qualifying source has
the same proven session/repository/work binding and all sources fit the 50-link evidence bound.
Their content identity includes that work, so equal progress from another work cannot share
provenance or validity. Reuse within that work merges current source/context proof and sensitivity.
Mixed, unbound or oversized session summaries remain stored without new visibility. They never
replace the current-work checkpoint or complete work.

A proposal stores its origin memory/repository/work, bounded unique source IDs, exact candidate
title/body/material hash/sensitivity, basis, state and decision receipt. States are pending,
approved or rejected. Pending has no decision/projection; rejected has a decision and no projection;
approved has a decision and one projected memory. Proposal identity hashes origin memory, sorted
source IDs and exact candidate title/body; normalization-equivalent alternatives remain separate.
Automatic approval also checks that the stored exact pair matches the verified declaration.
Decisions use the engine's direct-source proof or a human CLI /
viewer action; model-supplied flags do not authorize sharing.

Personal content hashes the `personal-projection-v1` domain plus the exact candidate title and body;
case, Unicode form and internal whitespace do not collapse distinct approved strings. The ordinary
material hash remains available for diagnostics. Exact title/body/material must match on reuse.
Candidates come from the detector-clean prepared observation, even when repository content identity
reuses an older normalized-equivalent origin. The projection is reviewed,
has one personal grant, and has no memory_sources, citations, source session/batch, work ID or
checkpoint lineage. Internal repository ownership may remain for audit/transfer, but public reader
shapes omit origin repository/work/session/batch, paths and raw-source IDs. The originating
repository alone can inspect the proposal audit. Tombstoning the projection hides it everywhere
without deleting its origin or proposal. Repeating an approval never revives a tombstone.

## Generation and direct declarations

Every ordinary observer observation requires `visibility: work | project | personal_proposal`.
The provider supplies no audience target ID, approval, source-authority or proposal-state field.
Fallback is work-scoped. A resolved accepted batch derives its original work/repository from its
immutable binding, verifies batch/session/repository and cited-row binding identity, and grants that
origin work. An already closed binding remains valid for accepted delayed output and recovery.
Interactive adoption separately requires the currently selected work.

`project` adds a project grant. `personal_proposal` creates an origin-work memory plus a candidate.
Updates inherit the target's work/project grants and add the new origin work/requested project
grant. A personal projection is never an observer-mutable nearby target. Existing-content/noop
confirmation can add grants/proposals only for a current usable memory (not deleted, superseded,
imported or secret); a tombstone cannot. Accepted batch application does not require active work
state or a still-open binding.

Automatic personal approval requires all of the following, determined from retained source data:

- Exactly one fully covered, detector-clean prompt, `input_source=user`, with complete capture and
  no secret sensitivity. Request truncation/fragment coverage is not a full declaration.
- Its entire content is one line beginning `Personal preference:`, `個人設定:` or `個人設定：`,
  followed by a non-empty statement of at most 500 characters.
- The observation cites that source exactly once and its body, after edge whitespace trimming only,
  equals the declaration statement. Case, Unicode and internal whitespace are not normalized.
  The projection uses that exact clean statement and a fixed `Personal preference` / `個人設定` title.

Reject CR/LF and Unicode line/paragraph separators before trimming the source. Full coverage means
one full range from zero to total, a current batch ID and a current processed source receipt.
Quoted/multiline/mixed prompts, tools, assistant messages, compaction, Pi RPC/extension input,
imports, failed/partial/unsent sources and paraphrased candidates cannot self-approve. Sanitized
inferred candidates may remain pending. Detector failure creates no proposal/projection. The
strictest candidate/origin/cited-source sensitivity applies. An external model cannot make a local
or private source eligible by choosing another audience.

Origin memory, sources, grants, proposal/projection, batch result and source outcomes are written
in the existing lease-fenced transaction. Lost lease or stale preparation rolls all effects back.

## Common selection and privacy guards

Extend `memoryScope` with one visibility predicate, used by retrieval, packs, CLI, MCP, viewer and
all scoped mutations. Existing lifecycle/destination filters follow it. Nearby observer candidates
use the batch work/project audience, retain tombstone handling, and exclude personal projections.
Raw activity remains exact-work scoped. Scope parameters always come from current cwd/binding or
accepted batch metadata, never arbitrary input repository/work targets.

Every ordinary aggregate reader and injection guard snapshots the matching visibility grant and approved
proposal/projection link as well as existing body/source/policy state. After asynchronous detection,
fresh selection and the complete stamp must still match. Grant removal, changed approval or changed
projection link withholds prepared output. Session/prompt combined responses and Grok delivery retain
the existing final fresh guard.

The explicit local diagnostic exception in [work.md](work.md#implementation-and-verification-order)
remains: CLI `--history`, `work status --all` and `why` may inspect retained state under stored
sensitivity/deletion and visibility rules. It does not apply to MCP history. Personal projection
fields stay source-free even in local history; the exception cannot approve a pending proposal or
expose origin metadata through a personal row.

Work/project memories retain their full source checks. A verified project grant permits reading
adopted knowledge from an absent origin worktree in another verified context of the same repository:
retain the origin's recorded rules and union them with current rules. This replaces only the
selected-origin-work requirement. Existing/reused/unverified roots still fail closed, and source
provenance is never rewritten. The project grant is freshly verified at the final guard.

Personal projections use current local policy, current credentials/detection, their personal grant
and approved proposal link. They carry no origin source fields into cross-project output and do not
require the old worktree to exist. A secret/deleted/quarantined/invalid projection remains hidden.
No general privacy-bypass flag is part of public API or persisted output.

## Human controls

Use shared plain operations behind:

```text
oboete share status [--json]
oboete share approve <proposal-id> [--json]
oboete share reject <proposal-id> [--json]
oboete share adopt <memory-id> [--binding <binding-id>] [--json]
```

Status is origin-repository scoped and returns at most 50 proposals, prioritizing pending decisions
before newest-first history. It reports omitted rows so terminal history cannot bury pending work.
Wrong-repository or out-of-scope IDs look missing.
CLI derives repository from cwd. Viewer reuses its per-launch token, same-origin mutation guard and
direct Approve/Reject/Make project knowledge buttons; marking reviewed is not approval. MCP exposes
read-only status because its current host provides no independently authenticated human mutation
receipt. Do not add an agent-generated `confirmed: true` substitute.

Approval prepares the exact candidate plus origin/source/current-policy snapshot, detects it
outside a write transaction, then uses BEGIN IMMEDIATE to compare fresh state/stamp and pending
proposal state before inserting/reusing the projection/grant. A policy/candidate/origin/context
change returns stale and creates nothing. Approve/reject concurrency has one winner; a repeat of
the same decision is idempotent. Deleted, superseded, quarantined or newly secret origins cannot
be approved. Origin privacy cannot be bypassed just because approval is a human action.

Adoption requires an active ordinary memory visible to the exact selected work in the current
repository, rechecks privacy, and inserts one project grant. It preserves all source rows and
memory content. Neither adoption, proposal decisions, session end nor integration text changes
work purpose, binding, checkpoint pointer, state, completed_at or outstanding checkpoint steps.
Only `completeWork` completes work. No verified Git integration receipt exists yet, so branch names,
commit-like text and tool reports cannot automate adoption.

## Implementation and checks

Viewer layout follows the existing system-ui type and ink/paper/line/blue-accent tokens. A compact
sharing section precedes the memory list, shows the exact candidate and expandable origin IDs,
and keeps its decision result visible in place. Native fieldsets/buttons disable concurrent actions;
status/errors are announced near the action, focus remains visible, and narrow layouts wrap controls.
Personal cards describe their approved audience without asking for unavailable source metadata.
No dependency, theme, animation or new navigation system is added.

1. T025: reproduce the missing-grant leak in `test/unit/memory-scope.test.ts`, then test the work /
   project / approved personal / pending / secret / imported / history matrix using the same scope.
2. T026: register migration 0006, add observer visibility and lease-fenced grants/proposals; prove
   upgrade preservation, forged fields, full direct-source proof, delayed closed-binding apply,
   update inheritance, tombstones, lease loss and deterministic repetition.
3. T027: thread common selection and source-free projection through readers/packs; cover final
   visibility/approval races, removed/reused worktrees, exact IDs and absence of origin payload.
4. T028: add scoped CLI/viewer human controls and MCP status; test stale/racing decisions, wrong
   repository, repeat operations, privacy drift and byte-identical work state before/after sharing.
5. Run relevant Node 22/24 tests, typecheck/lint/build, frontend browser checks for viewer edits,
   correctness/security review, Standards/Spec review and Ponytail review. Transfer/sync format
   preservation remains US5/US6; real agents/providers require their separate authorized runs.

Plan review incorporated three current-source findings before implementation: accepted closed
bindings remain valid; visibility/proposal state joins every final guard; approval detection has
an initial/final privacy snapshot. These refine the approved product contract without a new owner
decision. Existing shared helpers and SQL transactions are reused; no new dependency is required.
