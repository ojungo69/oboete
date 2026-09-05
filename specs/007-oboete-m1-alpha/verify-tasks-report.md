# verify-tasks report — Phase 4 (User Story 2), T057–T063

- Date: 2026-09-06
- Scope: `branch` (3fa73504 = start of Phase 4 → HEAD 41a8f86b on m1/p4-us2)
- Tasks verified: 7 (filter: T057 T058 T059 T060 T061 T062 T063)
- ⚠️ FRESH SESSION ADVISORY: this pass was run by the session that also drove the implementation (Claude Code as orchestrator; Grok Build wrote T058–T060, Codex wrote T063, Claude Code wrote T057/T061/T062 and the log-content fixes). Re-run in a separate session for an independent pass.
- Evidence beyond the layers below: `npm test` on Node 24.16.0 and Node 22.23.1 — 665 + 44 tests, 0 failures (this session, after the T063 commit); the five fault files alone: 40 scenarios, 40 pass.

## Scorecard

| Verdict | Count |
|---|---|
| ✅ VERIFIED | 6 |
| 🔍 PARTIAL | 1 |
| ⚠️ WEAK | 0 |
| ❌ NOT_FOUND | 0 |
| ⏭️ SKIPPED | 0 |

## Flagged items

### T057 — 🔍 PARTIAL

Task text lists `src/db/open.ts` and `src/worker/lease.ts` among the files to wire the seam into; neither changed on the branch.

| Layer | Result | Detail |
|---|---|---|
| 1 File existence | positive | src/testing/faults.ts, src/privacy/detect.ts, src/observer/llm.ts, src/capture.ts, src/db/open.ts, src/worker/lease.ts, test/unit/faults.test.ts, package.json all present |
| 2 Git diff | negative | open.ts and lease.ts are not in the branch diff (faults.ts A; detect.ts, llm.ts, capture.ts, package.json M) |
| 3 Content | positive | `testFault`, `faultFetch` exported; `testFault(` at detect.ts:460, llm.ts:329/434/470, capture.ts:1274; `faultFetch(` at llm.ts:377; package.json test glob carries `build/test/fault-*.test.mjs`; test/unit/faults.test.ts has the gate cases |
| 4 Dead code | positive | testFault 5 callers, faultFetch 1 caller in src/ |
| 5 Semantic | positive | ⚠️ Interpretive: the task note records the deviation on purpose — open/write failures are staged for real (missing, corrupt, chmod, held BEGIN IMMEDIATE) and land in the same catches, and every lease function takes `now` while `worker_lease` is one row a test can write; fault-storage (db-missing, busy, corrupt, readonly, enospc) and fault-worker (lease-steal, clock-jump, lease-lost-after-3036) exercise those paths without a seam. Not a phantom: the seam exists and is wired where a real fault cannot be staged. |

## Verified items

| Task | Verdict | Summary |
|---|---|---|
| T058 | ✅ VERIFIED | test/fault-storage.test.ts (A) with scenarios db-missing, busy, corrupt, readonly, enospc, oversized-payload, detector-never-returns; test/helpers/fault.ts (A) exports scenario/fixture/spawnEngine/runHook/rows/spoolFiles/claudePayload and is imported by all five fault files; 8/8 pass |
| T059 | ✅ VERIFIED | test/fault-worker.test.ts (A) with worker-kill, worker-kill-after-response, lease-steal, lease-lost-after-3036, clock-jump, resume, fork, clear, compact, pause; 10/10 pass ×3 runs |
| T060 | ✅ VERIFIED | test/fault-provider.test.ts (A) with provider-unreachable, provider-hang, provider-429-3036, provider-403-5035, provider-401, provider-length, provider-malformed (+ schema-invalid sibling), provider-wrong-language, cap-boundary (workers-ai and openrouter, cross-preset sum), consent-changed, remote-no-duplicate; 12/12 pass |
| T061 | ✅ VERIFIED | test/fault-pi.test.ts (A) with pi-throw, pi-child-hang, pi-spawn-failure, prior-failure counters recorded; 4/4 pass |
| T062 | ✅ VERIFIED | test/fault-grok.test.ts (A) with the six Grok cases and the ledger assertion after `purgeExpiredEvents`; 6/6 pass |
| T063 | ✅ VERIFIED | src/cli.ts (M: exit 0 + one hook-log line for hook/capture/inject), src/log.ts (M: `errorCode`, 11 callers in src/), src/worker/observe.ts (M: `SAFE_UNUSABLE_DETAILS` + `loggableDetail`, `detail=` on the batch log line), src/capture.ts and src/injection/inject.ts (M: sibling catches on `errorCode`), test/fault-storage.test.ts (M: SessionEnd-driven recovery assertions), test/unit/cli.test.ts and test/unit/observe.test.ts (M: 4 + 3 new tests). ⚠️ Interpretive: "green on 22.16 and 24.x" was measured on Node 22.23.1 (the installed 22.x) and 24.16.0 |

## Unassessable items

None.

## Verdict lines

| T057 | 🔍 PARTIAL | seam wired into detect/llm/capture; open.ts and lease.ts deliberately left without a seam (documented in the task note) |
| T058 | ✅ VERIFIED | storage matrix + shared harness present, changed, wired, green |
| T059 | ✅ VERIFIED | worker matrix present, changed, green |
| T060 | ✅ VERIFIED | provider matrix present, changed, green |
| T061 | ✅ VERIFIED | Pi matrix present, changed, green |
| T062 | ✅ VERIFIED | Grok matrix present, changed, green |
| T063 | ✅ VERIFIED | engine fixes present, changed, wired, matrix and full suite green on both Node lines |

## Walkthrough Log

- T057 (🔍 PARTIAL): disposition **S — skipped, no fix needed**. Auto-disposition by the orchestrating session (the user was not present for the walkthrough): the Layer 2 negative is the documented design deviation in the task note, not missing work. An independent re-run in a fresh session may re-open it.
