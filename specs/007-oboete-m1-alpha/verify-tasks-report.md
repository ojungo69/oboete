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

---

# verify-tasks report — Phase 5 (User Story 3), T064–T068

- Date: 2026-09-06
- Scope: `all` (origin/main 96885197 → HEAD 6b72308a on 007-oboete-m1-alpha; working tree clean)
- Tasks verified: 5 (filter: T064 T065 T066 T067 T068)
- ⚠️ FRESH SESSION ADVISORY: the cascade was run by five fresh workflow agents (one per task, no implementation context; opus for T065/T068, sonnet for the rest), orchestrated by the session that also drove the implementation (Claude Code wrote T064–T066; Grok Build wrote T067 and the T068 first pass and fix round; Codex wrote the T068 round 2). Re-run in a separate session for a fully independent pass.
- Evidence beyond the layers below: `npm test` on Node 24.16.0 and Node 22.23.1 — 678 + 44 tests, 0 failures (this session, after the T068 round-2 commit, from the lane worktree at the same tree); `oboete fixture replay` recorded run at load 0.38 in docs/evidence/m1-resource-envelope.md (every gate pass except SC-009 without provider credentials). Verifiers did not execute tests or the replay (read-only).

## Scorecard

| Verdict | Count |
|---|---|
| ✅ VERIFIED | 5 |
| 🔍 PARTIAL | 0 |
| ⚠️ WEAK | 0 |
| ❌ NOT_FOUND | 0 |
| ⏭️ SKIPPED | 0 |

## Flagged items

None.

## Verified items

| Task | L1 files | L2 diff | L3 symbols | L4 wired | L5 semantic | Summary |
|---|---|---|---|---|---|---|
| T064 | positive | positive | positive | n/a (tests) | positive | test/unit/privacy.test.ts drives the real hook, detector, worker and pack through test/helpers/observe.ts: mixed-session outbound body (eligible rows travel, secret row, `<private>` span, path-rule row, local-only memory and other repository absent, `repo_ref` opaque), FR-020 cross-repository refusal via injection_items count, pack recognition on the spool path (`recognized_packs` = [packHash]), agent-swap invariance over memories, pack text and decisions. |
| T065 | positive | positive | positive | positive | positive | src/injection/recognize.ts `stripRecognizedPacks` (sha256 of header→footer span against injections.pack_hash, every footer line tried, unissued span stays content) is called inside the capture write transaction (capture.ts:819) and in spool recovery (worker/batches.ts:346); contentHash is rewritten after stripping. test/unit/logs.test.ts is an end-to-end credential scan over rows, spool, log, pack, doctor output and the data directory; `credentialValues` (log.ts) is used by capture, observe and detect. |
| T066 | positive | positive | positive | positive | positive | `reclassifyImportedRow` (privacy/classify.ts) decides from two detector results and the directive check (clean → unreviewed with detector texts; secret or directive → tombstoned `secret`; unfinished detector → retry); observe.ts runs it after classifyPending in keyset pages of 50 and nearbyCandidates excludes imported rows. Decision-table unit test plus fixture-level end-to-end test match the note's seeded ids and assertions. |
| T067 | positive | positive | positive | n/a (fixture) | positive | scripts/fixtures/generate-1000-events.mjs and test/fixtures/events-1000.jsonl: 1,051 lines, 48 sessions, 40 bilingual facts recalled later, 37 corpus secret ids (32 + 5 negatives), 32 directives, 4 size events at the exact byte boundaries, fork/resume/compact/clear per agent — each count reproduced by the verifier by parsing the fixture; regeneration byte-identical. |
| T068 | positive | positive | positive | positive | positive | src/fixture/replay.ts (1,816 lines, loaded lazily by cli.ts `fixture`) spawns the real hook per event and derives every measurement from live processes and SQL; eleven gates (SC-002, injection, session start, SC-003, SC-005, SC-009, SC-010, lifecycle, directives, hooks) all feed the exit code; docs/evidence/m1-resource-envelope.md carries the recorded run. |

## Unassessable items

None.

## Verdict lines

| T064 | ✅ VERIFIED | privacy tests drive the real pipeline; every done-note claim found in the test bodies |
| T065 | ✅ VERIFIED | pack recognition wired into capture and spool recovery; credential scan end to end |
| T066 | ✅ VERIFIED | imported-row reclassification implemented, wired and tested at unit and fixture level |
| T067 | ✅ VERIFIED | fixture counts reproduced independently from the committed file |
| T068 | ✅ VERIFIED | replay implemented, all printed gates reach the exit code, evidence recorded |

## Gaps recorded by the verifiers (not defects)

- Tests and the replay were not executed by the verifiers (read-only pass); execution evidence is the session's `npm test` and the recorded replay run above.
- T065's done note says "batches.ts recoverSpool" without a directory; the code is src/worker/batches.ts. Doctor is still the T069 stub, so the credential scan's doctor leg only asserts the command ran and printed no credential (the note says so).
- T068's evidence Setup names Commit `16303cac` and the lane worktree's bundle path: the evidence is generated from the tree one commit before the commit that records it (NOTES-T068.md states this). SC-009 is a recorded fail (17.5%) without provider credentials, so the command exits 1 until replayed with a provider. The replay is not wired into package.json scripts or CI (a 6-minute, timing-sensitive run); the Phase 5 checkpoint "SC-005 and SC-006 pass in CI" is met by the T064/T065 unit tests that run in `npm test` (mixed-session outbound body: no secret, no local-only or private row; credential scan), and "the replay evidence file exists" by T068. SC-006 is not a replay row; it is covered by those unit tests.
