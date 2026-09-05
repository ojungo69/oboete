# Implementation Plan: Lightweight Automatic Memory Product Reset

**Branch**: `feat/product-reset-alpha` | **Date**: 2026-08-25 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/005-product-reset/spec.md`

## Summary

Reset free-mem's active product authority from a speculative Verified Continuity Engine to a
lightweight, reliable automatic memory product for Claude Code and Codex. Preserve the existing
Codemem safety kernel, use claude-mem as the UX and characterization-test reference rather than
forking its runtime, and establish three later user-visible implementation slices. This M0 change
updates product entry points, records the foundation decision and contracts, and routes GitHub
work; it intentionally changes no runtime source.

## Technical Context

**Language/Version**: Documentation and GitHub metadata for M0; later runtime slices retain
TypeScript 6, Node.js 24.16.0, and pnpm 11.8.0 unless measured evidence justifies a change.

**Primary Dependencies**: Existing Codemem workspace, Claude Code and Codex hook contracts,
GitHub Issues and pull requests; no new runtime dependency in M0.

**Storage**: No schema change in M0. The later Alpha retains the daemon-owned local memory store,
lexical index, and optional semantic index already present in the Codemem safety kernel.

**Testing**: Markdown and reference checks for M0; existing build, TypeScript, Biome, and Vitest
baseline. Later slices add fixed real-hook E2E, provider-failure, packed-artifact, and resource-soak
gates.

**Target Platform**: Technical Alpha is Linux and WSL on a local Linux filesystem. macOS is the
first post-Alpha platform milestone; native Windows is separate.

**Project Type**: Public monorepo containing a CLI, local daemon, MCP server, hooks, and viewer.
The current slice is an authority and backlog migration, not a product-runtime rewrite.

**Performance Goals**: M0 adds no runtime cost. Later slices inherit SC-003 through SC-005:
capture p95 below 200 ms, warm context p95 below one second, truthful cold fallback within three
seconds, and no post-warm growth above the frozen resource envelope.

**Constraints**: Preserve the Phase 1 sole-writer, spool, redaction, backup, and setup safety
assets. Do not merge the competing continuity-contract PR. Do not physically delete continuity
source or evidence until the replacement vertical slice passes. Keep one active product slice and
at most five active blockers.

**Scale/Scope**: Route 69 currently open issues and two open pull requests into one Product Reset
authority, no more than five active blockers, and three subsequent focused runtime/release specs.

## Constitution Check

*GATE: PASS before Phase 0 and PASS after Phase 1 design.*

| Principle | Result | Plan evidence |
|---|---|---|
| I. Automatic Memory UX First | PASS | The master spec and three later slices are organized around the bidirectional automatic-memory scenario. |
| II. Local-First and Explicit Egress | PASS | Local storage remains mandatory; provider destination, credential source, cost, and egress are explicit contract fields. |
| III. Bounded and Predictable Resources | PASS | M0 freezes a comparison contract and resource envelope before runtime optimization; no sidecar or new dependency is added. |
| IV. Durable Capture and Honest Degradation | PASS | Existing daemon/spool safety is preserved and the later runtime slice must expose lexical fallback and degraded reasons. |
| V. Product Slices Before Speculative Platforms | PASS | M0 changes authority only; three focused follow-ups precede macOS, Rust, Cloud, and additional Agents. |

No approved exception is required.

## Project Structure

### Documentation (this feature)

```text
specs/005-product-reset/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── issue-routing.md        # M0 GitHub audit and mutation evidence
├── m0-pre-mutation-issues.json # exact 69-issue state/label rollback baseline
├── m0-post-mutation-issues.json # exact expected state/labels before rollback
├── rollback.md             # GitHub state restoration if M0 is abandoned
├── fixtures/
│   ├── slice1-bidirectional-en-v1.json
│   ├── slice1-bidirectional-en-v1.schema.json
│   ├── slice1-bidirectional-en-v1.semantic.jq
│   ├── validate-slice1-fixture.mjs
│   ├── alpha-result-v1.example.json
│   ├── alpha-result-v1.failure-example.json
│   ├── alpha-result-v1.suite-regression.json
│   ├── runner-evidence/
│   │   ├── alpha-runner-evidence-v1.example.json
│   │   ├── alpha-runner-evidence-v1.failure-example.json
│   │   └── alpha-runner-evidence-v1.suite-regression.json
│   └── artifacts/
│       ├── candidate-example-v1/candidate.bundle
│       └── candidate-failure-example-v1/candidate.bundle
├── contracts/
│   ├── alpha-comparison.md
│   ├── capability-manifest.md
│   ├── injection-pack.md
│   ├── alpha-result-v1.schema.json
│   ├── alpha-result-v1.semantic.jq
│   ├── alpha-runner-evidence-v1.schema.json
│   ├── alpha-runner-evidence.mjs
│   ├── alpha-result-artifact.mjs
│   ├── alpha-result-atomicity.mjs
│   ├── alpha-result-input.mjs
│   ├── alpha-result-latency.mjs
│   ├── alpha-result-lineage.mjs
│   ├── alpha-result-render.mjs
│   ├── alpha-result-resource.mjs
│   ├── alpha-result-retry.mjs
│   ├── alpha-result-security.mjs
│   ├── alpha-result-selection.mjs
│   ├── validate-alpha-result.mjs
│   ├── validate-alpha-result.test.mjs
│   ├── validate-alpha-result-input.test.mjs
│   └── validate-alpha-result-failure.test.mjs
├── checklists/
│   └── requirements.md
├── tasks.md
└── verify-tasks-report.md
```

### Source Code (repository root)

```text
README.md                         # active public product entry point
.github/workflows/ci.yml          # Product Reset contract regression gate
evidence/
├── README.md                     # active versus historical evidence index
└── adr-006-product-reset.md      # foundation and scope decision
specs/
├── 001-agent-memory-core/        # historical Phase 1 safety source
├── 002-continuity-state-evidence/ # historical continuity evidence
├── 003-evidence-hash-normalization/ # historical capability evidence
├── 004-review-residue/           # historical review evidence
└── 005-product-reset/            # new active product authority
vendor/codemem/                   # unchanged by M0; later runtime implementation base
```

**Structure Decision**: Keep the existing repository and pinned vendor tree. M0 changes only the
root product entry point, evidence authority, Product Reset documents, their CI regression step,
and GitHub work tracking.
Historical files remain in place so links and validation assets do not break before a replacement
runtime slice exists.

## Delivery Decomposition

1. **M0 — Product authority reset (this branch)**: specification, ADR, README/evidence index,
   fixed contracts, and GitHub PR/Issue routing.
2. **Slice 1 — Automatic runtime path**: setup starts and diagnoses the runtime; one permanent
   minimal capability manifest binds the resource profile, explicit summary provider, disabled
   embedding lane, and lexical fallback; capture, flush, summarization, storage, and injection
   complete in both directions between Claude Code and Codex. #126, #129, and #130 are child entry
   criteria and land before or inside the same Slice 1 implementation branch. Setup rejects
   unsupported platforms/mounts
   before mutation, embedding-disabled preserves existing semantic data, and the named Phase 1
   sole-writer/spool/redaction/backup gates remain green.
3. **Slice 2 — Profiles and explainable retrieval**: extend the Slice 1 manifest to multiple
   resource profiles and independent embedding providers, then add semantic lifecycle, bounded
   InjectionPack, and visible fallback reasons.
4. **Slice 3 — Doctor and Technical Alpha release**: full lifecycle, failure matrix, read-only
   inspection/deletion, clean package, backup/restore, and resource soak.

Each later slice receives its own Spec Kit directory, branch, tests, and pull request. External
five-user validation follows Slice 3 and gates Core 1.0 planning.

## M0 GitHub Routing Rules

- Close PR #133 without merge because its horizontal source-aware continuity contract conflicts
  with the new product authority. Preserve the branch and discussion as historical evidence.
- Close #134 and #135 as superseded by the Product Reset decision and link the new authority.
- Close issues whose only outcome is deferred continuity, Rust-first, Cloud, additional-Agent,
  generic MCP, or shared-memory expansion; use a standard disposition comment and preserve links.
- Keep concrete current-runtime correctness, privacy, data-loss, setup, and resource defects open,
  but mark non-blockers deferred. No more than five issues may remain in an active status.
- Create one Product Reset parent issue and three child issues matching the delivery decomposition.
- Do not merge, release, or deploy as part of M0.

## Post-Design Constitution Check

The contracts keep local storage independent of Cloud, separate provider choice from the resource
profile, bound injection and operational states, and avoid introducing a protocol or runtime that
is not required by the first product slice. The design remains compliant with Principles I-V.

## Complexity Tracking

No constitution violation or new architectural layer is introduced in M0.
