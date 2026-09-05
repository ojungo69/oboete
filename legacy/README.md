# legacy/

Read-only evidence from the free-mem era of this repository (before the oboete rewrite decided on
2026-09-02). Nothing here is built, tested, or released. It is kept so that decisions, measurements,
and review history stay reachable from the current tree.

| Path | What it is |
| --- | --- |
| `README-free-mem.md` | The last free-mem README, describing the Product Reset and Slice 1 state. |
| `vendor/codemem/` | The pinned MIT snapshot of kunickiaj/codemem that free-mem built on (daemon, spool, redaction, hooks, viewer). |
| `specs/` | Spec Kit features 001-006, including the Product Reset (005) and Slice 1 runtime (006). |
| `evidence/` | ADRs, research (cmem pro parity, prior art), measurements, and review reports. |
| `harness/` | Capability matrix, continuity and evidence contract tests, mutation gates, notice and license gates. |
| `agent-memory-*.md`, `codex-review-report-2026-08-12.md`, `spec-review-2026-08-12.md` | The v5/v6 continuity specifications and their reviews. |
| `THIRD_PARTY_NOTICES.md` | Notices for the dependencies bundled by `vendor/codemem`. |
| `docs/` | Superpowers plans from the free-mem era. |

Disposition rule (`CONSTITUTION.md`, Product and Technical Constraints): only the destination
boundary SQL, test fixtures, and the mutation gate may be ported into oboete, and each ported piece is
deleted from here once it lands. Everything else stays until the rewrite reaches M3, then this
directory is removed in one pull request.
