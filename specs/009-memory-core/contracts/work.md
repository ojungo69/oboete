# Work continuity contract

The repository identifies shared knowledge. A local worktree identifies where work is being
performed. A work item identifies one purpose within that worktree. Native conversation identity
and compaction epochs continue to identify delivery history; they do not replace work identity.

## Context and binding

Reuse the existing Git identity call to read the absolute Git directory as well as the root and
common directory. The local context key hashes the real administration directory and its native
device/inode/birth identity, or the same tuple for a non-Git root. Main and linked worktrees share the repository identity and have different
context keys. Symlinks share a context. A branch change does not destroy context or work.

Migration 0005 adds `work_contexts`, `work_items` and `work_bindings`, and extends existing rows
with binding, checkpoint identity and generation-outcome fields. Contexts have repository,
local key, root and last-seen metadata. Work items have purpose, originating context, lifecycle
state and the current checkpoint pointer. Bindings connect a session and context to a work item,
or retain an unresolved candidate list. A unique partial index allows one current binding per
session. Earlier bindings stay available after raw expiry.

Capture chooses inside its existing immediate transaction, after duplicate-event detection and
before inserting the source. Raw events and observation batches retain their binding ID. The
batch key includes binding, and a provider attempt never combines different work items. A later
choice can resolve an unresolved binding, but switching resolved work closes the old binding and
creates a new one; already accepted sources cannot move to another purpose.

Selection rules, in order:

1. A direct, complete, detector-clean user prompt beginning with an explicit new-task declaration
   creates a new work item. Supported declarations are `New task:`, `Next task:`, `Separate task:`,
   `新しい作業:`, `新しいタスク:`, `別の作業:`, `別件:` and `次の作業:` (Japanese full-width colon also works).
   Quoted, tool, assistant, RPC/extension and compaction material cannot trigger this rule.
2. An existing native session in the same context keeps its selected binding. Resume and related
   investigation keep the work; compaction only changes the existing delivery epoch.
3. A new native session in a context with exactly one active work item continues it, regardless
   of the producing agent. With none, create a work item and fill its purpose from the first
   admissible direct user prompt. With several, persist a bounded choice list and stay unresolved.
4. A different/new worktree starts from its own candidates. A root path, branch name or newest
   repository-wide session cannot silently select another worktree's active progress.

These declarations are a deliberately narrow automatic split rule. Ordinary follow-up prompts
remain in the selected work; an explicit CLI/MCP choice covers purposes that cannot be inferred
without ambiguity. No semantic-model request is added to the hook path.

Spool metadata retains the derived context key and capture root. Recovery reuses the same binding
logic. A late source uses its unambiguous historical binding; unknown or overlapping historical
membership stays unresolved and cannot replace a current binding. Old schema-4 sources without
context provenance remain legacy history until a safe binding can be established or chosen.

Native IDs are also scoped to repository. Keep the old global SQL uniqueness constraint and
existing references; add `original_native_session_id` only when a second repository collides,
using an internal UUID for that row's storage key. All native lookup/display uses the original
value. New event IDs hash `(repo-v1, repository ID, legacy event ID)`; a retained same-repository
legacy ID is still a duplicate. An old spool ID colliding with another repository is namespaced
and retained, never discarded as that repository's duplicate.

`sessions.last_captured_at` survives raw expiry. A late or indistinguishable same-clock spool
record cannot reopen/end the current session, increment its turn count or close its current
turn. It uses an existing historical turn only when the recorded prompt key proves membership.
An unseen late new-purpose declaration and its following recovered records stay in an unresolved
historical interval until the next known binding boundary. They do not replace the live choice.
Late recovery cannot roll a context root back while retaining a newer last-seen timestamp.

## Checkpoints and reading

Reuse `memories.type = session_summary`, `memory_sources`, `valid_to` and `superseded_by` for
checkpoint versions. Add `work_id` and `checkpoint_parent_id`; include work identity in checkpoint
content hashes so identical text from different work items cannot collapse. Preserve old hashes
and the old export reader. A current checkpoint describes purpose, constraints, decisions and
outstanding steps with source provenance. Temporary generation remains labeled and retryable.

Checkpoint publication compares the expected parent with the work's current pointer in the same
fenced transaction. Competing progress retains both versions and exposes the conflict; timestamps
alone cannot overwrite an independently advanced checkpoint. Merge, session end and inactivity
never set work to completed. Completion is an explicit work operation.

For progress, injection, ordinary search and MCP reads include only the selected work's active
checkpoint. Applicable project and personal knowledge remain available alongside it.
An unresolved work selection supplies short, stable candidate IDs and purposes, plus applicable
knowledge; it supplies no active checkpoint or raw activity from any candidate. Prompt-triggered
selection works even if the native session-start pack was already delivered. Existing conversation
delivery receipts continue to prevent repeat delivery of the same memory.

## Work operations

`oboete work status` reports current bindings and active work in the current context; an explicit
repository-wide listing also makes retained work from removed worktrees discoverable.
`oboete work choose <binding-id> <work-id|new>` resolves or switches that exact binding.
`oboete work complete <work-id>` marks a chosen work item complete. Explicit selection can resume
retained dormant work in another local context of the same repository. No user-supplied repository
ID is accepted as authority.

`oboete work choose-source <source-id> <work-id|new>` explicitly assigns one complete retained
unbound source in the current repository. A new historical work item starts dormant. Existing
pending/running attempts without a resolved work binding are detached and held; ordinary worker
invocation and `observe --reprocess-source` cannot bypass work selection.

MCP `work_status` and `work_choose` expose the same operations, with bounded arguments and clear
read/write annotations. A stale binding/choice is refused without changing accepted source
membership or another session. Binding and work IDs are opaque; titles are sanitized display
data, not commands. A missing or out-of-scope ID has the same response.

## Implementation and verification order

Local context identity includes the real Git administration directory plus its native BigInt
device, inode and birth timestamp, under a versioned hash. A directory project uses that same
identity tuple for its own directory. Recreating a checkout at the same path creates a new context;
reading identity never creates or repairs files in Git metadata. Missing/zero/unsupported identity
metadata is unknown, not a path-only fallback. Capture groups such input only within its internal
native session; automatic cross-session selection, CLI/MCP context inference and removed-root
rebasing stay disabled. Retained work remains discoverable by explicit local status/history.

Ordinary CLI, MCP and viewer outputs apply the current source-context/path policy before formatting
memory bodies or source arrays. MCP history remains an agent egress; explicit local `--history`,
`work status --all` and `why` can explain retained or withheld work while preserving stored
sensitivity/deletion rules. Ordinary `work status` uses the same current policy as MCP.
History cannot rebase another work's removed checkpoint through the currently selected work.
MCP verifies its startup repository/root/generation before each data tool call; a replaced or
unverified context requires a new server session. Work purposes and ambiguous-choice labels pass
current credential/source checks independently of checkpoint bodies; rejected labels become null
while their opaque IDs remain usable.
Missing or expired purpose sources are unverified. Injection reserves space for `Untitled work`,
includes displayed purpose sources in the pack's aggregate privacy guard, and rechecks the
purpose-to-source mapping before immediate or delayed delivery. Ambiguous choices apply both
the current context's and the origin's path rules. The local history exceptions above remain.
Each read view checks a single aggregate policy/source stamp across bodies and labels after all
asynchronous detection. A change withholds all affected content; opaque work choices remain.
MCP read tools also recheck their directory generation immediately before returning.

B1: capture/context/binding migration and public capture tests, then spool/retry isolation.
B2: work-scoped checkpoint production and all reading paths, then CLI/MCP choices and removal.
B3: interleaved purposes/worktrees, native resume/compact/fork, sibling checkpoint races, all
ordered synthetic agent pairs and existing isolated lifecycle harness. Real agents/platforms stay
separate acceptance gates in T040-T042.

Required regressions cover: equal remotes and distinct worktrees; symlink identity; same-worktree
new purpose; related investigation; native resume with several active candidates; ambiguous fresh
session; duplicate delivery; late spool recovery; batching after a work switch; foreign/stale
choices; removed worktrees; checkpoint race; and merge/session-end preserving outstanding work.
