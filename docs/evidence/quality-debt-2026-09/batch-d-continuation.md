# Batch D concern-splitting continuation

The continuation preserves FR-016 and separates the independent concerns recorded in
[research R7](../../../specs/008-quality-debt-zero/research.md#r7-complexity-and-length-refactors-safety-rails).
The comparison snapshot is `d724d5df65f6b7484f09d75dfa595c48fdc54c99`; the earlier six splits
and their runtime measurements remain in [the initial evidence](batch-d-verification.md).

All eighteen original files now have their identified independent concerns separated.
Thirteen frozen findings are planned `fixed #185`; five have source-specific `AcceptedUse`
reasons for their cohesive residual code. Two new lifecycle source/test modules exceed 500 NLOC
and are explicitly assessed in R7; native IDs from the next analysis must be recorded before
final confirmation. No threshold, assertion, provider behavior or acceptance condition changed.

| Check | Result |
|---|---|
| TypeScript, ESLint, bundle build | Pass |
| Node 22.16.0 full `npm test` | 872 unit/migration/harness + 200 serial E2E/fault/CLI = 1,072 pass |
| Node 24.16.0 full `npm test` | 872 + 200 = 1,072 pass |
| Final shared-helper relocation, both engines | 90/90 harness checks pass on each |
| Existing probe discovery | Same 35 IDs in the same order |
| Fixture generator | Same 1,051 lines / 666,961 bytes |
| Planned ledger validation | 721 IDs; 0 missing, duplicate, open or unknown; 56 unconfirmed |

The generator SHA-256 is `9af34aad5ab41ae69a03a654d7c7c792bf0e2c5cd10320e694f522ef3dcd0f3b`.
Lizard 1.24.0 supplies the NLOC figures in R7. Default CCN warnings move with unchanged bodies;
the file-length work does not claim that every Lizard threshold passes.

AST comparisons preserve 101 injection function bodies/values and 38 test bodies;
217 capture/test bodies and all 214 assertions; 99 worker bodies; 203 non-glue replay bodies;
and all 1,295 existing harness source/test bodies with all 308 existing assertions.
The harness comparison normalizes only three namespace-to-direct-import references and one
source-path comment. Replay's new glue passes completed measurements and bounds between the
driver, evaluator and renderer in the original order. A same-input differential produced
byte-equal Markdown (8,552 bytes) and JSON (2,948 bytes); evaluator tests use a migrated temporary
database and cover all ten verdicts, counts, lifecycle checks and failure aggregation.

The runtime import scan includes literal dynamic imports and excludes erased type imports.
It finds no introduced cycle; seven existing cycles use only files byte-identical to the
comparison snapshot. Shared event-file/evidence helpers and the completion prompt now live in
their existing common modules, removing the MCP probes' dependency on lifecycle descriptors.

Standards and Spec reviews found three actionable issues: a stale source-path comment,
the still-independent compaction state machine and completed-run evaluation. Those were fixed,
and both final reviews report `ok: true`. Ponytail reports no further simplification.
Cubic's initial two boundary proposals were triaged: common probe helpers moved without body
changes; redesigning the unchanged mixed timing aggregation was not adopted. Its final review
reports zero issues. CodeRabbit's full continuation review reports zero issues; the final
incremental request ends with `No files to review`, so it supplies no additional review coverage.
The injection component's Grok review completed, but the full continuation's Grok review did
not: it returned progress only, then HTTP 402 usage-balance exhaustion. That is not a review pass.

The [provider diagnostic](batch-d-provider-diagnostic.md) used the unchanged earlier bundle;
it is not acceptance for this continuation. T032, T039 and T041 remain open.
[Candidate dogfood](batch-d-continuation-dogfood.md) subsequently passed all six available
Claude/Codex/Pi pairs, all eight lifecycle checks and all three MCP clients; four native probes
fail. Grok's usage-balance exhaustion prevents the required twelve-pair run.
[Fresh resource comparisons](batch-d-continuation-resources.md) meet all timing/RSS bounds and
all 32 timing comparisons, while both replays still fail SC-009 at 7/40 recall.
[Analysis follow-up](batch-d-analysis-followup.md) records thirteen additional native service
IDs and their individual dispositions, plus eleven corresponding Sonar push-branch IDs, bringing the inventory to 745; the initial source-check
counts above are retained as historical evidence. No tool setting or daily installation changed.
