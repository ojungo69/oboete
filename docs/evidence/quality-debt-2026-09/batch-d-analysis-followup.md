# Batch D analysis follow-up

PR #185 analysis of `ebe687dcf99103cf3965649b969bb0f2bb4d5d62` reported eleven Sonar issues
on moved code, two Codacy file-length issues and two CodeQL filesystem-race alerts. Each was
compared with its source and its pre-extraction body at `d724d5df`; new IDs are not assumed to
mean new behavior. The Sonar/Codacy inventory gains thirteen native IDs without dropping any,
for 734 total (321 Sonar, 413 Codacy). These are PR observations, not another main baseline.

| Finding group | Source-backed disposition |
|---|---|
| Sonar S8707, `mcp-report.mjs:99,100` | The operator explicitly chooses `--out`; the local harness writes fixed `report.json`/`report.md` names there. Agent output does not choose the path. Same writes as old `mcp-clients.mjs:845,846`; no less-trusted controller crosses a privilege boundary. |
| Sonar S4036, `process.mjs:59,75`, `replay-report.ts:67` | Fixed `git`/`timeout` commands resolve through the operator's PATH with argv arrays. Fixture/agent output supplies neither PATH nor those executable names. Same calls as old `agents.mjs:189,209` and `replay.ts:520`; an actor able to replace PATH executables already has that user's code-execution authority. |
| Sonar S3516, `childEnv` and `waitUntil` | Returning the same variable name does not mean the same value: credentials are selectively removed, and polling returns either the first truthy value or the final falsy value. Existing disposition reasons remain valid. |
| Sonar S3516, capture and Pi injection CLI | Every successful and failed agent-facing path deliberately returns numeric zero; changing it would violate the CLI's quiet/fail-open contract. |
| Sonar S7784, fixture coverage | The discarded JSON round trip checks serialization for the actual JSONL writer; `structuredClone` accepts values such as BigInt that JSON rejects. Existing disposition remains valid. |
| Sonar S6551, Grok native session ID | String coercion preserves the existing numeric-key behavior; a string-only guard changes identity and resume lookup. Existing disposition remains valid. |
| Codacy lifecycle source/test file length | 724/638 NLOC; the independently reviewed residual state machine and its execution tests are cohesive after preparation, evaluation and reporting moved out. Native IDs: `f24df26860d01b16c092756092dba6ce` / `48b2a00cb7eb3b5e6344415870d01661`. R7 records `AcceptedUse` reasons. |

[CodeQL #158](https://github.com/ojungo69/oboete/security/code-scanning/158) points to
`codex-mcp.mjs:47`, the unchanged check/write from old `probes/codex.mjs:412,413`.
[CodeQL #159](https://github.com/ojungo69/oboete/security/code-scanning/159) points to
`process.mjs:55`, the unchanged fresh-repository setup from old `agents.mjs:184,185`.
Exploiting either race requires write access to the same operator-owned scratch home/repository;
that access already permits arbitrary changes there. Neither is a product entrypoint or a
cross-user operation. The review recommends `used in tests` for these two individual alerts.

The old record classifier recognized only Sonar S8786 as security-related. Consequently the
new S4036/S8707 rows would stay unallocated and could be recorded without their required verdict.
The existing classifier now recognizes those two observed rules, and allocation reuses it.
Three focused checks failed before the fix: two missing-verdict checks and allocation. After the
fix all 158 record CLI tests pass on both Node 22.16.0 and Node 24.16.0; unknown rules remain
unallocated and PL/SQL exclusion priority remains intact. ESLint passes. No new flag was added.

The measured engine is unchanged: SHA-256
`97bd89dd1771dadc77ca8052491f565c38131c123ef4c28b8ef981185d739761`.
The record CLI changes are outside the packed engine. The thirteen current-PR rows were then
resolved individually: all 22 Sonar transition/comment calls returned HTTP 200 and the two
Codacy ignores returned HTTP 204 after account verification returned 200. Sonar's current PR
issue search now returns zero and its Quality Gate is OK; Codacy's cached PR summary awaits
the next analysis. CodeQL #158/#159 are individually dismissed as `used in tests` with their
source-specific reasons. [HTTP receipts](batch-d-pr-alert-responses.json) record these actions.
Original Batch F bulk rows were not applied: main still reports Sonar 15 / Codacy 38, and the
734-row planned ledger has 56 unconfirmed rows. Actual runtime acceptance remains outstanding.
