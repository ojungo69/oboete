# Evidence Index

## Active Product Authority

The active product direction is the lightweight automatic-memory Product Reset:

| Artifact | Purpose |
|---|---|
| [`../specs/005-product-reset/spec.md`](../specs/005-product-reset/spec.md) | User outcomes, Alpha boundary, requirements, and success criteria |
| [`../specs/005-product-reset/plan.md`](../specs/005-product-reset/plan.md) | M0 authority reset and three-slice delivery order |
| [`adr-006-product-reset.md`](adr-006-product-reset.md) | Codemem-base decision, claude-mem fork rejection, and revisit triggers |
| [`../specs/005-product-reset/issue-routing.md`](../specs/005-product-reset/issue-routing.md) | Complete GitHub work disposition and mutation evidence |

## Reused Safety and Foundation Evidence

These artifacts remain relevant because the Product Reset preserves the existing Codemem safety
kernel:

| Artifact | Reused evidence |
|---|---|
| `adr-001-base.md` | Why the pinned Codemem snapshot was selected over a new runtime |
| `delta-comparison.md` | Reusable source, test, and write-boundary comparison |
| `unsafe-path-action-plan.md` | Historical unsafe write/auth inventory and removal plan |
| `codemem/write-handle-inventory.md` | Original write-capable surface inventory |
| `codemem/write-handle-classification.md` | Fatal versus non-fatal boundary classification |
| `phase1-t043-viewer-security-validation.md` | Viewer authentication and read-only boundary evidence |
| `phase1-t044-cli-rpc-validation.md` | CLI-to-daemon mutation cutover evidence |
| `phase1-t045-t046-daemon-jobs-validation.md` | Daemon job and maintenance-mode evidence |
| `phase1-t047-operations-validation.md` | Export/import operation evidence |
| `phase1-t048-zero-external-db-handles-validation.md` | Daemon-only write-handle evidence |
| `phase1-t051-legacy-cutover-validation.md` | Existing local migration safety evidence |
| `phase1-t052-backup-restore-validation.md` | Backup/restore baseline |
| `phase1-t053-static-scan-validation.md` | Static safety exit gate |
| `phase1-t054-runtime-db-open-validation.md` | Runtime database ownership evidence |
| `phase1-t055-fault-injection-validation.md` | Existing fault-injection evidence |
| `phase1-t056-no-agent-blockage-validation.md` | Existing fail-open Agent evidence |
| `phase1-t057-backup-restore-smoke-validation.md` | Backup/restore smoke evidence |
| `phase1-t058-final-validation.md` | Phase 1 candidate baseline |
| `adr-004-licensing.md` | Repository and third-party material license decision |

The source snapshot remains pinned at Codemem
`26438e75ce1d0fec6be34981f15045a15c89658b`. `vendor/codemem/VENDOR.md` records provenance.

## Historical Evidence

The following artifacts remain available for audit and selective reuse, but they are not active
Product Alpha authority:

- `agent-memory-final-spec-v5.md`, `agent-memory-final-spec-v6.md`, and earlier implementation or
  preimplementation reviews at the repository root
- `specs/001-agent-memory-core/` beyond the completed Phase 1 safety evidence
- `specs/002-continuity-state-evidence/`
- `specs/003-evidence-hash-normalization/`
- `specs/004-review-residue/`
- `adr-003-rust-local-core.md` and `adr-005-rust-core-product-direction.md`
- `phase3-capability-scenario-manifest.md`, `phase3-reference-model.md`, and
  `phase3-resume-oss-comparison.md`
- `direct-competitor-positioning-2026-08-18.md` as a discovery snapshot rather than current
  benchmark proof
- `harness/continuity/`, continuity fixtures, and broad capability-rig artifacts

Historical status means:

- no old phase number, P0 label, or unfinished task blocks the Product Alpha by itself;
- old contracts are not silently copied into a new slice;
- a later slice may reuse a bounded invariant only when its current user-visible failure and test
  are restated in that slice;
- physical deletion waits until no replacement test, reference, or migration needs the artifact.

## Product Reset Acceptance Baseline

At commit `accaa29f5627c20c7e4c106a81211067fcf2bc42`, the isolated Product Reset worktree produced:

- frozen dependency installation: pass
- workspace build: pass
- TypeScript check: pass
- Biome check: pass
- Vitest: 124 files passed, 1,895 tests passed, three todo

This is a code-health baseline only. It is not evidence that the automatic-memory Technical Alpha
is implemented or supported.
