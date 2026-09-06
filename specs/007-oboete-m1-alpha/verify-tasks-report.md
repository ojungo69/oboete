# Verify-tasks report — Phases 6-10 (T069-T087)

- Date: 2026-09-06
- Scope: `all` (branch 007-oboete-m1-alpha at 584d2387, clean tree); tasks filtered to T069-T087 marked `[X]`; T085, T088, T089 open by design
- Verifiers: five fresh `sonnet` agents (Workflow `wf_a512bc1b-17e`), one group of two to four tasks each; the implementing session only assembled this report
- ⚠️ FRESH SESSION ADVISORY: the verification ran in independent agents, not in the implementing context

## Scorecard

- ✅ VERIFIED: 18
- 🔍 PARTIAL: 0
- ⚠️ WEAK: 0
- ❌ NOT_FOUND: 0
- ⏭️ SKIPPED: 0

## Flagged items

None.

## Verified items

| Task | Verdict | L1 files | L3 symbols | L4 wiring | L5 semantic | Summary |
|---|---|---|---|---|---|---|
| T069 | ✅ VERIFIED | positive | positive | positive | positive | doctor.ts:86-167 wires all listed items, exit 3/1/0 logic at line 163; cli.ts:32 wires `doctor:`. pi_child_hang literal (agents.ts:329); pi_child_failed set in capture.ts:893, read via generic message_code (agents.ts:348-368); literal 'pi_spawn_failed' absent (closest: capture_spawn_failed) — paraphrase, not phantom. 26/26 tests pass in build/test/unit/doctor.test.mjs. |
| T070 | ✅ VERIFIED | positive | positive | positive | positive | pause.ts implements runPause/runResume (marker mode 0o600, pause.ts:53), wired in cli.ts:44-45. isPaused (config.ts:397) checked in capture.ts:874 before db open, per comment 'R12: paused marker read before stdin and before the database'. 5/5 tests pass in build/test/unit/pause.test.mjs. |
| T071 | ✅ VERIFIED | positive | positive | positive | positive | doctor.test.ts has one test per break: hook removed(281), chmod(362), corrupt header(383), stale worker lease(412), unreachable provider(439), exhausted allowance(465), stale Pi .started(492), Pi spawn failure(510), each asserting degraded then recovery turns green. Also --json(564), paused warning exit0(577), config 0o644(586), --no-probe-agents(539), unknown option exit2(709). 26/26 pass via node --test. |
| T072 | ✅ VERIFIED | positive | not_applicable | not_applicable | positive | docs/evidence/m1-dogfood.md:178-212 section 'setup timing and doctor break-one-at-a-time (SC-008)' has measured setup time 11.9s vs 2min bound, baseline doctor run, 8-row break table with reason/recovery/after state matching T071 tests, exit codes 1/3/0 documented, 'FAILS=0, SC-008 pass' plus two real defects found/fixed same day — genuine evidence, not a stub. |
| T073 | ✅ VERIFIED | positive | positive | positive | positive | test/unit/degraded.test.ts (8 tests) covers summary precedence (src/observer/classify.ts sessionSummary), pack `> degraded:` lines (src/injection/pack.ts), usageEstimate exhaustion (src/observer/reservation.ts). `node --test build/test/unit/degraded.test.mjs`: 8/8 pass, 0 fail. |
| T074 | ✅ VERIFIED | positive | positive | positive | positive | src/why.ts:174 runWhy parses <session-id>/--turn/--json, findSession (:62) resolves native ids, calls whyReport, renders ItemReason/DEGRADED_SENTENCES/deferred attempts. Wired src/cli.ts:17,40. `node --test build/test/unit/why.test.mjs`: 7/7 pass. Matches contracts/cli.md:19. |
| T075 | ✅ VERIFIED | positive | not_applicable | not_applicable | positive | docs/evidence/m1-dogfood.md:214-232 has concrete per-pair table (6 pairs, 22885-32990ms, all pass) and confirms degraded_marker:true; exhaustion gap honestly left open pending Grok hold, covered by doctor break 6 + degraded.test.ts. Minor prose slip ('nine' vs six pairs at line 232) doesn't indicate fabrication. |
| T076 | ✅ VERIFIED | positive | positive | positive | positive | src/memories-cli.ts exports runSearch(:202)/runTimeline(:275)/runGet(:308)/runPin(:383)/runUnpin(:390)/runDelete(:397) on db/queries.ts; LEXICAL_NOTE(:24) emitted on empty results. Wired src/cli.ts:37-43. test/unit/cli-memories.test.ts:453 covers A13 tombstone non-recreation directly (:538-540). |
| T077 | ✅ VERIFIED | positive | positive | positive | positive | src/mcp.ts implements legacy-era JSON-RPC: initialize echo (196-203), tools/list w/ inputSchema (29-61,213), tools/call content+structuredContent+isError (135-192), -32601 for server/discover (220), -32602 for repo arg (141-143), 1MiB frame -32600 (67,242-246). Wired src/cli.ts:48. test/unit/mcp.test.ts covers raw frames; build/test/unit/mcp.test.mjs: 10/10 pass. |
| T078 | ✅ VERIFIED | positive | positive | positive | positive | src/viewer/server.ts: Hono on @hono/node-server (120-122), loopback-only bind (105-107), per-launch token via timingSafeEqual on every route (97-102,142-146), Origin check on mutating routes (148-154), SSE PRAGMA data_version poll every 500ms (34,238-259), sessions/turns/memories/review/pin/delete/search routes, --open browser launch. Wired src/cli.ts:49. build/test/unit/viewer-server.test.mjs: 6/6 pass incl. non-loopback refusal, token-less refusal, SC-011 SSE timing. |
| T079 | ✅ VERIFIED | positive | positive | positive | positive | src/viewer/app/main.tsx: sessions/turns list, MemoryCard w/ sensitivity/provenance/review-state/degraded-reason/pin/delete-confirm (46-114), full-sentence copy (SENSITIVITY_LABEL/DEGRADED_LABEL 9-27), search (118-129), why-ledger, SSE live refresh via api.events (175). scripts/build.mjs (esbuild, no Vite per plan.md) compiles to dist/viewer/app.js/app.css, present and freshly built. Served by src/viewer/server.ts. Genuine implementation, no stubs. |
| T080 | ✅ VERIFIED | positive | positive | positive | positive | scripts/e2e/mcp-clients.mjs drives tools/list and tools/call for claude/codex/grok (21-22, registration fns ~490-540) with raw frames via probe-lib/mcp-tee.mjs, plus Pi tool path via extension prompt/assertion (~727-756). scripts/e2e/mcp-clients.test.mjs: 14/14 pass. docs/evidence/m1-dogfood.md:111-124 records isolated-user run 2026-09-06T07-00-54-911Z, 4/4 agents pass with concrete per-agent protocolVersion/tool/frame data. |
| T081 | ✅ VERIFIED | positive | positive | not_applicable | positive | docs/evidence/m1-dogfood.md:111 '## 2026-09-06 MCP clients run' has 4/4 client pass table (claude/codex/grok/pi); :132 '## 2026-09-06 viewer timing (SC-011)' has 5-row timing table, worst-case 6ms vs 2s bound, 500ms SSE poll. Concrete figures matching cli.md/plan requirements. Evidence task, no code to wire. |
| T082 | ✅ VERIFIED | positive | positive | positive | positive | src/transfer.ts: EXPORT_FORMAT='oboete-export/1' (17), MAX_LINE_BYTES=64KB (18), MAX_FILE_BYTES=256MB (19), exportMemories (100), importMemories (243) w/ material_hash check (165-166), content_hash recompute (173), union lookup (176); runExport/runImport (370,398) wired in src/cli.ts:46-47. test/unit/transfer.test.ts covers tombstones, lattice, --map-repo, rejection. Ran build/test/unit/transfer.test.mjs: 7/7 pass. |
| T083 | ✅ VERIFIED | positive | positive | positive | positive | runExport/runImport wired in src/cli.ts commands map (46-47), matching cli.md rows for export/import w/ --dry-run/--map-repo. Exit 2 on parseArgs failure, oversized/unreadable file, bad --map-repo, rejected lines; exit 0 on success. test/unit/transfer.test.ts:325 CLI test asserts these exact codes (344-373). build/test/unit/transfer.test.mjs: 7/7 pass incl. this test. |
| T084 | ✅ VERIFIED | positive | not_applicable | not_applicable | positive | docs/evidence/m1-dogfood.md:149 '## export -> import round trip and fixture replay (SC-003)': round trip (31 exported, 8+2 mapped imported into B, idempotent, quarantine-until-observe, 'SC-003 export/import part pass' at :160) plus fixture replay section w/ RSS/per-1000-events figures vs m1-resource-envelope.md. Concrete measured data present, matches task description. |
| T086 | ✅ VERIFIED | positive | positive | not_applicable | positive | README.md (402 lines) has substantive install/setup/doctor/privacy/support-matrix/degraded-mode sections (README.md:53 support matrix, tables at 132-149, 284-303). docs/agents/{claude,codex,grok,pi}.md all exist (92-124 lines each), cross-linked from README.md:319,326,332,337. Full-sentence prose, no stubs/TODOs found. |
| T087 | ✅ VERIFIED | positive | positive | positive | positive | package.json:25 `pack-check: node scripts/pack-check.mjs`. scripts/pack-check.mjs really builds, npm packs, installs into empty --prefix, sumInstalledBytes() walks tree, exceedsSizeLimit() gates at 30MB (LIMIT_BYTES), exits 1 on breach. ci.yml:88-89 runs `npm run pack-check` in CI. README:65-67,375-381 cite matching measured 20.280 MB pass. |

## Notes

- T069: the verifier found no literal `pi_spawn_failed` in the source (the agent:pi reason is the sentence `Pi could not be started for the probe.`); the task line's name is a paraphrase of that diagnostic, not a missing feature.
- T075: the evidence prose said "nine non-Grok pairs" where six were run; corrected in docs/evidence/m1-dogfood.md after this report.
