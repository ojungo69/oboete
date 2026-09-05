# ADR-006: Product Reset to Lightweight Automatic Memory

- **Status**: Accepted
- **Date**: 2026-08-25
- **Decision owner**: repository owner
- **Supersedes as active direction**: ADR-005 Rust-first product direction, Issue #135 Verified
  Continuity Engine direction, and the v6 continuity phase order
- **Preserves**: ADR-001 pinned Codemem base and completed Phase 1 safety boundary

## Context

free-mem accumulated a large continuity contract, capability harness, Rust migration plan, and
69 open issues before it had an installable product or supported Agent path. The repository was
making measurable progress on internal correctness while the user-visible reason to install the
product remained unproven.

The owner clarified the intended product:

> A reliable, less resource-hungry, easier-to-configure claude-mem-like automatic memory product
> for developers who alternate between Claude Code and Codex.

The good claude-mem experience to preserve is automatic capture, asynchronous summarization,
relevant retrieval, context injection, and inspectability. The problems to improve are unbounded
resource growth, process and lifecycle fragility, configuration sprawl, coupled summary and
embedding choices, and silent degraded search.

## Decision

### 1. Reset the product boundary

Technical Alpha targets one local Linux/WSL user alternating between Claude Code and Codex. It
must provide automatic bidirectional memory, lexical and optional semantic retrieval, explicit
resource/model profiles, honest degradation, diagnostics, inspection/deletion, backup/restore,
and a clean lifecycle.

Verified Continuity Engine contracts, Rust-first cutover, macOS/native Windows, additional
Agents, broad MCP, shared memory, Cloud, teams, and advanced viewers are deferred. They do not
block Technical Alpha.

### 2. Keep the current Codemem safety kernel

The pinned and modified Codemem vendor remains the runtime foundation. It already contains:

- Claude and Codex capture and injection commands and tests;
- a daemon-owned writer and authenticated local mutation path;
- bounded atomic spool and idempotent recovery;
- redaction and secret handling;
- local durable memory, lexical retrieval, and optional semantic retrieval;
- observer, backup/restore, CLI/MCP, and viewer assets.

The current architecture source is recorded in
[`vendor/codemem/docs/architecture.md`](../vendor/codemem/docs/architecture.md). The public source
at the decision commit is [accaa29](https://github.com/ojungo69/free-mem/blob/accaa29f5627c20c7e4c106a81211067fcf2bc42/vendor/codemem/docs/architecture.md).

The base is conditional rather than presumed complete. Source inspection found four immediate
product gaps:

1. setup configures Agents but does not start the memory daemon;
2. the observer requires explicit supported provider configuration;
3. default sweep and idle timing can delay memory availability across a quick Agent switch;
4. local embeddings can initialize implicitly and lack the approved user-facing profile boundary.

These gaps define the first two implementation slices; they do not justify discarding the safety
kernel.

### 3. Reject a claude-mem runtime fork; use it as a UX and test donor

claude-mem v13.15.3 is the behavioral reference, not the code foundation. The audit pinned
[v13.15.3](https://github.com/thedotmack/claude-mem/tree/v13.15.3) and followed the
hook-to-worker-to-provider-to-store-to-search path.

Relevant evidence:

- the current settings manager has 84 distinct `CLAUDE_MEM_*` keys and writes the full default
  set for a new install
  ([source](https://github.com/thedotmack/claude-mem/blob/v13.15.3/src/shared/SettingsDefaultsManager.ts));
- Chroma coupling spans 39 production files and 737 references, plus 36 test files and 473
  references;
- the worker coordinates SQLite, Chroma, and Cloud initialization in one lifecycle
  ([DatabaseManager](https://github.com/thedotmack/claude-mem/blob/v13.15.3/src/services/worker/DatabaseManager.ts));
- the session message buffer is in-memory and has no item cap
  ([SessionMessageBuffer](https://github.com/thedotmack/claude-mem/blob/v13.15.3/src/services/worker/SessionMessageBuffer.ts));
- an open v13.15.3 report measured Chroma RSS growing from 1.18 GB to 1.79 GB in roughly two hours
  ([issue #3684](https://github.com/thedotmack/claude-mem/issues/3684));
- replacing Chroma with a local semantic index would still require generation, per-item tracking,
  catch-up, activation, rebuild, fallback, and diagnostics work.

A fork would therefore recreate durable capture, bounded spool, single-writer storage, semantic
index lifecycle, configuration boundaries, and diagnostics already present or closer to complete
in free-mem. The fast-moving upstream would also require permanent selective backport work after
removing core upstream subsystems.

The following claude-mem behaviors remain valuable selective donors, with Apache-2.0 attribution:

- context rendering and prompt-aware injection;
- observation and summary prompt/parser characterization;
- visible observer-health communication;
- adapter and search behavior tests;
- a future import-only migration format.

### 4. Keep TypeScript until measurements justify Rust

Rust is not an Alpha objective. The first comparison contract measures cold and warm latency,
process tree, resident-memory slope, queue depth, storage growth, capture loss/duplication, and
injection usefulness with identical lifecycle milestones.

Rust becomes a candidate only if the TypeScript runtime still misses the frozen envelope after
removing sidecars, bounding workers, and compiling one effective capability manifest. Any Rust
prototype is limited to the measured bottleneck; it does not trigger a repository-wide rewrite.

### 5. Deliver three focused runtime slices after this authority reset

1. **Automatic runtime path** — setup/start/basic doctor plus the permanent minimal capability
   manifest: one resource profile, one explicit summary provider, embedding disabled as a
   first-class state, lexical fallback, Claude-to-Codex and Codex-to-Claude capture, flush,
   summary, store, inject, and durable fail-open recovery.
   Current bugs #126, #129, and #130 are Slice 1 child entry criteria, not parallel product
   tracks. Unsupported platforms or mounts must fail before configuration writes; disabled
   embeddings must not delete existing semantic data; Phase 1 sole-writer, spool, redaction, and
   backup gates remain green.
2. **Profiles and explainable retrieval** — extend that same manifest/compiler to multiple
   profiles and independent embedding providers, then add semantic lifecycle and the bounded
   InjectionPack compiler.
3. **Doctor and Technical Alpha release** — inspection/deletion, full lifecycle, package,
   backup/restore, resource soak, and external user validation.

Only Slice 1 is implementation-ready after this reset. Each slice receives a separate focused
Spec Kit plan and pull request.

## Alternatives Rejected

### Continue the Verified Continuity Engine plan

Rejected because it optimizes correctness of an unvalidated product boundary. Checkpoints,
leases, fences, workspace reconciliation, and source-aware shared memory may be reconsidered only
after automatic memory usage proves a concrete need.

### Fork claude-mem and remove its liabilities

Rejected because the desired removals cross storage, search, settings, lifecycle, providers,
tests, and distribution. The fork does not buy a small stable core; it buys a proven UX attached
to the exact architecture being replaced.

### Build a new runtime from scratch

Rejected because it discards the current safety and test assets before delivering an Alpha.

### Track current Codemem upstream

Rejected because free-mem intentionally removed or isolated upstream sharing, sync, and unsafe
write/auth surfaces. Security fixes may be audited and selected, but normal upstream rebasing is
not the maintenance model.

### Ship macOS in the initial Alpha

Rejected for the first three slices because the current storage boundary explicitly supports only
Linux/WSL and includes Linux-specific filesystem durability checks. macOS is the first independent
post-Alpha platform work package.

## Consequences

### Positive

- Product work begins from a green, safety-hardened base rather than a rewrite.
- The main value proposition is testable in one bidirectional scenario.
- Chroma/Python lifecycle risk is absent from the chosen runtime.
- Non-expert configuration can be small without removing advanced model choice.
- Future Cloud can reuse local memory contracts without becoming a local dependency.

### Negative

- The current Codemem tree remains large and needs deliberate product carve-out.
- Claude/Codex automatic behavior exists in source but is not yet a supported packaged path.
- Linux/WSL-only Alpha reduces the initial user pool.
- Selective UX/test adaptation requires license and provenance discipline.

## Revisit Triggers

Re-open the foundation decision only if one of these is demonstrated with the frozen comparison
contract:

1. the current base cannot complete the bidirectional automatic-memory scenario without a broad
   rewrite;
2. TypeScript lifecycle fixes cannot stop unbounded resource growth;
3. a maintained external base provides the required durable capture, independent provider
   contracts, explainable retrieval, and migration path with a smaller verified delta;
4. external Alpha users reject the product despite meeting reliability and setup gates.

Popularity, implementation language, single-binary packaging, or feature count alone are not
revisit triggers.

## Validation at Acceptance

From the Product Reset worktree based on `accaa29`:

- dependency installation succeeded with the frozen lockfile;
- full workspace build exited 0;
- TypeScript and Biome checks exited 0;
- Vitest passed 124 test files and 1,895 tests, with three existing todo tests;
- no runtime source was modified while making this decision.
