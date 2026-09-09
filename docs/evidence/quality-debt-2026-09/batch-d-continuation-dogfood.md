# Batch D continuation candidate dogfood

Candidate `ebe687dcf99103cf3965649b969bb0f2bb4d5d62` was built from its Git archive with
`npm ci --no-audit --no-fund`, `npm run build` and `npm pack`, then installed in the dedicated
account's candidate prefix. [Structured results](batch-d-continuation-dogfood.json) include
pair verdicts, lifecycle assertions, MCP results, native probe evidence and doctor rows;
model transcripts, event payloads and credentials remain private.

- Tarball SHA-256: `709cd13c2cb51903a492a3aed7856497a4ef126a4b1ce75708f4f15e468fad5e`.
- Packed/installed engine SHA-256: `97bd89dd1771dadc77ca8052491f565c38131c123ef4c28b8ef981185d739761`.
- All 29 bundle references across seven copied agent configuration files point at the candidate.
- The real account HOME was retained; five candidate-home variables and the Pi loader select the copies.
- All eight checked daily configuration files have mtimes before this run; the daily installation
  was not replaced, and private candidate/per-leg credentials are retained under the runbook guard.

| Lane | Result | Run ID / limit |
|---|---|---|
| Setup | Exit 0 | Candidate references verified before running agents |
| Claude/Codex/Pi pairs | 6/6 pass, exit 0 | `2026-09-09T07-09-53-170Z`; partial, twelve are required |
| Claude/Codex resume/compact/fork/clear | 8/8 pass, exit 0 | `2026-09-09T07-14-07-876Z` |
| MCP/native-tool clients | 3/3 pass, exit 0 | `2026-09-09T07-18-21-589Z`; includes stdio rejection/missing-item checks |
| Five affected native probes | 1 pass, 4 fail; exit 1 | `2026-09-09T07-19-07-524Z` |
| Doctor with provider probe | Exit 0; Workers AI answered | Grok's hook row is not proof of a successful model response |

The runner preserves failure across lanes and exits 1. Its completion does not convert a partial
pair run or a failed probe into acceptance. Grok was omitted after its dedicated account returned
HTTP 402 usage-balance exhaustion; no additional purchase or authentication change was made.

The native probes report:

- Pi continues with `DONE` after an extension error but emits no durable error record, matching
  the previously recorded base and candidate failure.
- Claude auto compaction emits PreCompact and PostCompact, with `compact_summary` in PostCompact,
  but SessionStart `source=compact` arrives before PostCompact; the probe's requested ordering
  and distinct identity field are absent, and its manual TUI attempt records no PostCompact.
- Codex session-start probing observes `startup` and `resume`; its automatic-compaction attempt
  records no compaction, and `/new` does not emit the immediate `source=clear` the probe expects.
- Codex manual compaction emits PreCompact/PostCompact, but no summary field or separate compact
  identity; its probe also observes no subsequent `SessionStart source=compact`.
- The legacy Codex MCP client passes, negotiating protocol `2025-06-18`, listing/calling tools
  and preserving the `mcp__oboete_probe__search` hook name.

These native-probe failures are recorded as observations, not attributed to this extraction from
a single run. The production lifecycle assertions above pass. T039 still requires the full
twelve-pair run and the resource/replay acceptance; neither is waived by this evidence.
