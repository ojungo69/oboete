# Phase 3 resume / continuity OSS comparison

Date: 2026-08-16  
Scope: `free-mem` Phase 3 preflight  
Related: #1, #8, #13

## Purpose

This document compares public behavior, documentation, and source from memory and agent-runtime projects that directly inform reliable coding-task resumption. It does not reproduce private prompts, private services, or undocumented implementation details.

The research was triggered by concrete gaps in v6.1:

- prompt-aware and compact injection were assumed but not proven in the repository's exact-version real-CLI matrix;
- canonical work state was untyped and session-scoped;
- in-flight side effects had no safe `unknown result` contract;
- checkpoint delivery, acceptance, and lifecycle were mixed;
- current workspace drift was not reconciled precisely enough;
- resume-mode, capsule, sensitivity, and selection fallback contracts were incomplete;
- memory update/invalidation/source-history behavior lacked conformance fixtures;
- v6.1 and #8 disagreed on Core 1.0 quality authority.

## Source manifest and reproducibility

Retrieval date for live documentation: **2026-08-16 (Asia/Tokyo)**.

| Project | Pinned public source | Exact pages/files inspected | Status for design evidence |
|---|---|---|---|
| claude-mem | [`thedotmack/claude-mem@d768ba364302d12b76e69e4f021f0bb1d2d50ed6`](https://github.com/thedotmack/claude-mem/commit/d768ba364302d12b76e69e4f021f0bb1d2d50ed6) | [`src/cli/handlers/context.ts`](https://github.com/thedotmack/claude-mem/blob/d768ba364302d12b76e69e4f021f0bb1d2d50ed6/src/cli/handlers/context.ts), [`src/services/context/ContextBuilder.ts`](https://github.com/thedotmack/claude-mem/blob/d768ba364302d12b76e69e4f021f0bb1d2d50ed6/src/services/context/ContextBuilder.ts), [`src/servers/mcp-server.ts`](https://github.com/thedotmack/claude-mem/blob/d768ba364302d12b76e69e4f021f0bb1d2d50ed6/src/servers/mcp-server.ts), [`docs/public/architecture/hooks.mdx`](https://github.com/thedotmack/claude-mem/blob/d768ba364302d12b76e69e4f021f0bb1d2d50ed6/docs/public/architecture/hooks.mdx) | **Pinned/reproducible** |
| LangGraph | [`langchain-ai/langgraph@644815f9e5bc52ad8f7a5227a456227e9c3e639b`](https://github.com/langchain-ai/langgraph/commit/644815f9e5bc52ad8f7a5227a456227e9c3e639b) | Official persistence docs: [`docs.langchain.com/oss/python/langgraph/persistence`](https://docs.langchain.com/oss/python/langgraph/persistence) | Source commit pinned; live docs **provisional until B0 capture/hash** |
| Letta | [`letta-ai/letta@56ba9c25552605eec89de8ed3dc6394b625c1993`](https://github.com/letta-ai/letta/commit/56ba9c25552605eec89de8ed3dc6394b625c1993) | [`Memory blocks`](https://docs.letta.com/guides/core-concepts/memory/memory-blocks), [`Context hierarchy`](https://docs.letta.com/guides/core-concepts/memory/context-hierarchy), [`Attaching/detaching blocks`](https://docs.letta.com/tutorials/attaching-detaching-blocks/) | Source commit pinned; live docs **provisional until B0 capture/hash** |
| Graphiti | [`getzep/graphiti@203b792af462be6973373986f08f3a5cdebca430`](https://github.com/getzep/graphiti/commit/203b792af462be6973373986f08f3a5cdebca430) | Pinned [`README.md`](https://github.com/getzep/graphiti/blob/203b792af462be6973373986f08f3a5cdebca430/README.md), [`Episodes`](https://help.getzep.com/episodes), [`Searching the graph`](https://help.getzep.com/searching-the-graph) | README pinned; live product docs **provisional until B0 capture/hash** |
| Mem0 | [`mem0ai/mem0@001c235229be8795e3834520467bd0d661ed8f34`](https://github.com/mem0ai/mem0/commit/001c235229be8795e3834520467bd0d661ed8f34) | [`Memory history`](https://docs.mem0.ai/api-reference/memory/history-memory), [`Update memory`](https://docs.mem0.ai/api-reference/memory/update-memory), [`Delete memory`](https://docs.mem0.ai/core-concepts/memory-operations/delete), [`Entity-scoped memory`](https://docs.mem0.ai/platform/features/entity-scoped-memory) | Source commit pinned; live docs **provisional until B0 capture/hash** |
| Hindsight | [`vectorize-io/hindsight@396f63aafc9b618f04d446e2465cac95aa1cb426`](https://github.com/vectorize-io/hindsight/commit/396f63aafc9b618f04d446e2465cac95aa1cb426) | [`recall.mdx`](https://github.com/vectorize-io/hindsight/blob/396f63aafc9b618f04d446e2465cac95aa1cb426/hindsight-docs/docs/developer/api/recall.mdx), generated client/OpenAPI at the same commit, public tests around observation enrichment | **Pinned/reproducible** |
| Cline | [`cline/cline@8bbdde2a5c1f972864fe1b954f639c21fac61a40`](https://github.com/cline/cline/commit/8bbdde2a5c1f972864fe1b954f639c21fac61a40) | [`Checkpoints`](https://docs.cline.bot/core-workflows/checkpoints), [`IDE restore modes`](https://docs.cline.bot/usage/ide), [`Auto Compact`](https://docs.cline.bot/features/auto-compact) | Source commit pinned; live docs **provisional until B0 capture/hash** |

### Evidence freeze rule

Before any benchmark result or release claim depends on a provisional page, B0 MUST store:

- exact URL;
- retrieval timestamp;
- response/content SHA-256;
- relevant excerpt location or normalized JSON fact record;
- project version/commit when available;
- license/provenance classification.

A later page change does not retroactively change the frozen contract. Re-baselining requires a new evidence manifest and comparison report.

## Comparison matrix

| Concern | claude-mem | LangGraph | Letta | Graphiti | Mem0 | Hindsight | Cline | free-mem decision |
|---|---|---|---|---|---|---|---|---|
| Immediate resume UX | silent SessionStart `additionalContext`, fail-open empty output | resume a thread checkpoint | bounded core blocks are always visible | retrieval-driven context | application-driven search | application-driven recall | reopen persistent task | bounded silent hint; full only after exact capability/relevance/reconciliation |
| Work state vs durable memory | timeline of observations/summaries | checkpointer vs Store | memory blocks vs archival memory | episodes vs derived facts | memory rows/history | facts vs observations | task history vs workspace snapshot | task-scoped state, checkpoint, and DurableMemory remain separate |
| In-flight work | no inspected typed replay contract | task writes/pending writes persist | messages/tool history persist | episode provenance, not execution continuation | async/history operations | async retain/consolidate | task/file checkpoints | typed `PendingOperation` with `never_auto`, `verify_first`, `safe_idempotent` |
| History/fork | session timeline | immutable checkpoint history/fork | persistent mutable blocks | temporal fact history | ADD/UPDATE/DELETE history | observations retain source facts | checkpoint history/restore | immutable state/checkpoint revisions + append-only disposition events |
| Stale facts | recent context; no inspected validity window in resume path | application creates new state | mutable current blocks | valid/invalid/expired temporal windows | explicit update/delete history | source evidence behind observations | compare/restore workspace | temporal history + fail-closed workspace reconciliation |
| Concurrency | local session/worker behavior | checkpoint namespaces/task writes | shared block LWW risk | temporal graph updates | scoped operations/history | bank isolation/evidence links | task-scoped checkpoint manager | initial claim CAS + fenced delivery attempts + no LWW canonical state |
| Workspace state | project/recent context | application-defined | external state can be mirrored | not coding-workspace-specific | metadata filters | tags/scopes | shadow Git, task/files/both restore | deterministic reconciliation now; optional snapshot provider later |
| Injection safety | bounded hook output | application-owned rendering | XML-like block rendering | application-owned | application-owned | application-owned | native task UI | canonical JSON, fixed limits, sensitivity filtering, escaped wrapper, owned-ledger verification |
| Retrieval duplication | summary/timeline may overlap | application-defined | archival hierarchy | graph hybrid retrieval | extraction/search dedupe | consolidated observation preference | not a memory engine | presentation-level dedupe without source deletion |

## Findings and adoption decisions

### 1. claude-mem: adopt smooth delivery, not task ambiguity

Inspected code returns prior context through `hookSpecificOutput.additionalContext`, fails open to empty context, and builds a bounded timeline from observations and summaries.

**Adopt**

- silent pre-model context delivery;
- empty-context fail-open behavior;
- bounded timeline/search/get progressive disclosure;
- platform-specific output limits.

**Reject**

- treating recent project context as proof of task continuation;
- direct reader-side SQLite access;
- injecting a large timeline before evaluating the new prompt.

### 2. LangGraph: separate execution checkpoints from long-term memory

LangGraph separates thread-scoped checkpoint state from a cross-thread Store, persists task-level writes, and creates new checkpoint forks instead of rewriting prior state.

**Adopt**

- immutable checkpoint IDs/history;
- task-scoped namespace;
- explicit pending-operation records;
- replay policy instead of automatic command replay;
- corrections as new revisions/forks.

### 3. Letta: keep the always-visible state small

Letta's memory blocks are bounded, named, purpose-described context, while larger memory is retrieved through archival/external mechanisms.

**Adopt**

- a small typed resume capsule with explicit purpose/limits;
- separation of current task state from archival DurableMemory;
- persistent evidence after context eviction.

**Reject**

- Agent-writable canonical observed state;
- last-write-wins shared canonical state.

### 4. Graphiti: preserve temporal truth and source episodes

Graphiti models validity windows and keeps raw episodes behind derived facts.

**Adopt**

- validity/invalidation/supersession history;
- source-event links for derived notes;
- `exact / stale / requires verification / incompatible` reconciliation instead of binary match;
- point-in-time diagnostics.

### 5. Mem0: make mutation/history explicit

Mem0 exposes change-history entries and explicit update/delete operations with scope filters.

**Adopt**

- append-only revision history;
- explicit supersede/retract operations;
- old/new/source metadata;
- strong scope filters.

**Reject**

- generic model extraction as authority for current repository state;
- vector search as sole truth.

### 6. Hindsight: preserve facts behind observations

Hindsight observations remain linked to source facts and retrieval can prefer consolidated output while keeping evidence stored.

**Adopt**

- source IDs on semantic resume notes and consolidated memory;
- no raw-evidence deletion during refinement;
- output-time `prefer_consolidated` dedupe;
- token-budget-first selection;
- RRF rather than adding incomparable raw scores.

### 7. Cline: task and workspace are different restore domains

Cline keeps task history and workspace checkpoints separate and exposes task-only, workspace-only, or combined restoration.

**Adopt now**

- independent task/workspace reconciliation;
- compare-before-restore and drift warnings;
- future `task / workspace / both` restore interfaces.

**Defer**

- mandatory shadow Git after every tool use. Core 1.0 records HEAD, dirty fingerprint, affected-file evidence, and pending operations. A future `WorkspaceSnapshotProvider` needs separate security/storage/UX proof.

## Resulting free-mem architecture

```text
NormalizedEvent stream
        │
        ├── immutable evidence + event idempotency ledger
        ├── TaskWorkState revision (one current pointer per taskLineageId)
        │       ├── Observed<T> provenance/freshness/sensitivity
        │       ├── PendingOperation[]
        │       └── RepositoryStateSnapshot
        ├── immutable ContinuationCheckpoint revision
        ├── append-only CheckpointDispositionEvent projection
        ├── fenced CheckpointDeliveryAttempt
        └── DurableMemory revisions / derived observations
                ├── source evidence links
                ├── temporal validity / supersession
                └── presentation-level consolidated preference
```

Before automatic full injection:

```text
exact-version capability
  + mode
  + dataset-versioned selection decision
  + workspace reconciliation
  + sensitivity policy
  + atomic claim
  -> typed bounded capsule
  -> owned-ledger verified capture stripping
  -> deterministic engagement
  -> atomic accepted disposition + attempt
```

## Normative choices that supersede v6.1 continuity details

The addendum supersedes v6.1 only for:

- task-lineage-scoped state instead of one state per session;
- typed state instead of `canonicalStateJson: unknown`;
- duplicate logical events as explicit revision no-ops;
- immutable checkpoint content + append-only disposition projection;
- separate initial claim CAS and delivery-attempt lifecycle;
- deterministic engagement evidence before atomic acceptance;
- fail-closed workspace reconciliation;
- exact capability × mode delivery;
- dataset-versioned candidate-selection wire contract;
- bounded sensitivity-aware JSON capsule and capture verification;
- durable-memory revision/temporal/source-history fixtures;
- #8 non-inferiority as Core 1.0 authority.

All Phase 1 safety invariants remain unchanged.

## Licensing and provenance

This document adopts architectural patterns, public behavior, and interface ideas only. It copies no implementation code. Any future code-level port must identify exact source path, commit, license, copied/derived range, and pass #10 before merge.
