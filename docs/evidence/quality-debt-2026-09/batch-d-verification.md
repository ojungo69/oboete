# Batch D verification — 2026-09-09

Candidate: `b05f73c9d68a3e5ff316a6172d7156a84906c47b`; base: `72bcb23b18feec02858d12c41443be1b45ffaa01`.
T039 remains open. The measurements and dogfood run were executed; the acceptance conditions below are not all met. No service disposition was applied.

## Source and test review

- Both independent review axes found no behavior change in the six extractions: 41 moved function bodies and the moved types/constants are unchanged, every caller imports the new module, and reverse dependencies are type-only.
- The Spec review still reports the FR-016 mismatch: research R7 permits retaining a separable concern when its first extraction would not close the file-length finding. FR-016 has not yet been amended; this decision remains pending.
- Cubic and CodeRabbit CLI each returned zero issues for the candidate. These results do not resolve the Spec finding or replace the runtime gates.
- The additional Grok review is incomplete: its first response was a progress statement, and continuing the same session reached the 10-minute timeout. Neither response is counted as a completed review.
- The missing T037 test moves were completed after that candidate: 11 direct apply tests and 2 direct spool-recovery tests now follow their modules. All 13 bodies and all 42 test names are unchanged. Typecheck, lint, build and the 42 tests in the four affected suites pass. The resulting bundle has the same SHA-256 as the candidate.

## Resource comparison

Each revision was built with `npm ci --no-audit --no-fund` and `npm run build` in its own detached checkout. Both install/build commands exited 0. Measurements ran serially on Node v24.16.0 within 18:35–18:49 UTC; no test suite or candidate dogfood was run concurrently with them.

| Command | Base exit | Candidate exit |
| --- | ---: | ---: |
| `node scripts/measure-cold-start.mjs` | 0 | 0 |
| `node dist/oboete.mjs fixture replay test/fixtures/events-1000.jsonl --json` | 1 | 1 |
| cold-start repeat after the comparison miss | 0 | 0 |

The first cold-start comparison missed the 15% maximum-delta condition only on DB-absent capture: 188.6 → 238.3 ms (+26.35%), while its median changed 179.4 → 187.2 ms (+4.35%). One additional paired run was made because of that failure; its spool maximum was 194.5 → 189.2 ms and every series met the comparison/budget conditions. Both runs are retained below. The repeat does not erase the initial miss.

### Initial base cold-start

- Date: 2026-09-08T18:36:00.385Z
- Node versions: `$HOME/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0)
- Commit: `72bcb23b`
- Bundle: `dist/oboete.mjs` (1598418 bytes)
- Samples: 30 measured runs after 3 warm-up runs per scenario
- Measurement attempts: run 1 load `1.24 1.15 1.16 1/2090 1462824`; run 2 load `1.98 1.32 1.22 3/2111 1466858`; kept run 1 (lower 1-minute load average)
- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`

Load average next to this table (kept run 1, before the measurement set): `1.24 1.15 1.16 1/2090 1462824`

| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |
|---|---|---:|---:|---:|---:|---|---|---:|---|
| v24.16.0 | `--version` | 0 | 58.7 | 62.8 | 63.6 | n/a | n/a | 100 ms | pass |
| v24.16.0 | hook small, DB present | 730 | 182.0 | 205.0 | 213.4 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook clean 200 KB, DB present | 206768 | 189.2 | 198.8 | 205.3 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook secret-dense 200 KB, DB present | 206772 | 197.7 | 211.7 | 218.1 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook small, DB absent (spool) | 742 | 179.4 | 187.1 | 188.6 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |

### Initial candidate cold-start

- Date: 2026-09-08T18:41:10.330Z
- Node versions: `$HOME/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0)
- Commit: `b05f73c9`
- Bundle: `dist/oboete.mjs` (1599105 bytes)
- Samples: 30 measured runs after 3 warm-up runs per scenario
- Measurement attempts: run 1 load `1.26 1.38 1.28 1/2149 1503258`; run 2 load `1.66 1.47 1.31 6/1768 1506560`; kept run 1 (lower 1-minute load average)
- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`

Load average next to this table (kept run 1, before the measurement set): `1.26 1.38 1.28 1/2149 1503258`

| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |
|---|---|---:|---:|---:|---:|---|---|---:|---|
| v24.16.0 | `--version` | 0 | 59.0 | 62.4 | 65.3 | n/a | n/a | 100 ms | pass |
| v24.16.0 | hook small, DB present | 730 | 185.0 | 203.4 | 215.3 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook clean 200 KB, DB present | 206768 | 186.1 | 196.1 | 202.9 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook secret-dense 200 KB, DB present | 206772 | 194.4 | 203.3 | 204.4 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook small, DB absent (spool) | 742 | 187.2 | 208.6 | 238.3 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |

### Repeated base cold-start

- Date: 2026-09-08T18:47:36.472Z
- Node versions: `$HOME/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0)
- Commit: `72bcb23b`
- Bundle: `dist/oboete.mjs` (1598418 bytes)
- Samples: 30 measured runs after 3 warm-up runs per scenario
- Measurement attempts: run 1 load `0.71 1.15 1.24 2/1726 1535732`; run 2 load `1.01 1.19 1.25 1/1722 1538530`; kept run 1 (lower 1-minute load average)
- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`

Load average next to this table (kept run 1, before the measurement set): `0.71 1.15 1.24 2/1726 1535732`

| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |
|---|---|---:|---:|---:|---:|---|---|---:|---|
| v24.16.0 | `--version` | 0 | 70.2 | 80.1 | 83.2 | n/a | n/a | 100 ms | pass |
| v24.16.0 | hook small, DB present | 730 | 186.7 | 202.9 | 204.6 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook clean 200 KB, DB present | 206768 | 189.0 | 204.7 | 228.8 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook secret-dense 200 KB, DB present | 206772 | 193.7 | 207.4 | 208.4 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook small, DB absent (spool) | 742 | 176.5 | 184.0 | 194.5 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |

### Repeated candidate cold-start

- Date: 2026-09-08T18:48:31.269Z
- Node versions: `$HOME/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0)
- Commit: `b05f73c9`
- Bundle: `dist/oboete.mjs` (1599105 bytes)
- Samples: 30 measured runs after 3 warm-up runs per scenario
- Measurement attempts: run 1 load `1.06 1.18 1.25 1/1716 1540717`; run 2 load `1.18 1.20 1.25 1/1714 1543172`; kept run 1 (lower 1-minute load average)
- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`

Load average next to this table (kept run 1, before the measurement set): `1.06 1.18 1.25 1/1716 1540717`

| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |
|---|---|---:|---:|---:|---:|---|---|---:|---|
| v24.16.0 | `--version` | 0 | 62.5 | 70.3 | 70.3 | n/a | n/a | 100 ms | pass |
| v24.16.0 | hook small, DB present | 730 | 183.7 | 193.3 | 196.2 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook clean 200 KB, DB present | 206768 | 189.1 | 201.0 | 206.4 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook secret-dense 200 KB, DB present | 206772 | 192.8 | 204.5 | 208.9 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook small, DB absent (spool) | 742 | 174.3 | 187.9 | 189.2 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |

### Fixture replay

Full command output: [base JSON](batch-d-base-replay.json), [candidate JSON](batch-d-candidate-replay.json). Each contains all 418 injection samples and every acceptance row.

| Measurement | Base | Candidate |
| --- | ---: | ---: |
| Capture p99 | 222.7 ms | 207.1 ms |
| Injection median / maximum | 199.2 / 462.1 ms | 196.1 / 242.5 ms |
| Ready session-start maximum | 296.4 ms | 212.1 ms |
| Pending session-start maximum | 1211.7 ms | 1211.9 ms |
| Worker peak RSS | 116380 kB | 117824 kB |
| Successful hooks | 1143 / 1143 | 1143 / 1143 |
| Fact recall | 7 / 40 (17.5%) | 7 / 40 (17.5%) |
| Secret / directive / duplicate leaks | 0 / 0 / 0 | 0 / 0 / 0 |
| Lifecycle sequences | pass | pass |

The base also misses its injection budget; the candidate passes every replay row except SC-009 recall. Recall is below 90% on both revisions. `replayEnv` in `src/fixture/replay.ts` calls `childEnvironment`, which removes the provider credential variables: supplying credentials to this command does not enable provider-backed replay. This existing limitation needs a separately reviewed replay path; T032 remains open and SC-009 is not waived.

## Candidate dogfood

- Tarball SHA-256: `dadc674265d1ddc33a9034e1ceaab15f1a728d977b66d62d691931112871b7d5`.
- Packed and installed bundle SHA-256: `f8ea3316c6cbaf997ffd3f45cfadfa72e0f77a17e55c5b7b38b236601ed9ce4a`.
- The real isolated account HOME was retained. Five copied configuration homes were wired to the candidate, including the Pi loader workaround from issue #174. All 28 bundle references across the seven configured files name the candidate; no daily bundle reference was found.
- Setup exited 1: Claude and Codex probes pass; Grok probe fails.
- Pair run `2026-09-08T18-51-16-798Z`: **2 of 12 pairs pass**, command exit 1. Codex → Pi and Pi → Codex pass. Five failing legs reach Claude's weekly usage limit (`resets 2pm (Asia/Tokyo)`); the other five reach Grok's `Not signed in` error, already recorded under issue #175. Each cause was read from that leg's own result.
- Candidate probe run `2026-09-08T18-56-17-500Z`: Pi error-surface exits the agent successfully and continues with `DONE`, but the probe fails because it finds no durable error record (stderr only). Baseline probe run `2026-09-08T18-59-22-889Z` reproduces the same failure on `72bcb23b`.
- The Claude postcompact probe is `blocked`: no `PostCompact` was captured, and the headless calls returned the weekly-limit error. Doctor exited 1 because the Grok agent is degraded; the Workers AI provider answered successfully. Doctor's Claude hook row is healthy because the hook fired before the CLI returned its quota error; that row is not a successful LLM response.
- The daily installation/configuration and cron were not changed. The previous candidate directories were archived and this run's private homes/per-leg credentials are retained under the isolated account so issue #175 does not discard refreshed credentials.

| Doctor item | Status | Evidence |
| --- | --- | --- |
| config | healthy | candidate config loaded with mode 0o600 |
| paused | healthy | not paused |
| storage | healthy | quick_check ok, 96 memories |
| fts | healthy | lexical search available |
| migration | healthy | schema version 3 |
| worker | healthy | live process with fresh heartbeat |
| spool | healthy | writable and empty |
| provider | healthy | Workers AI answered with `@cf/zai-org/glm-4.7-flash` |
| allowance | healthy | 149 of 150 calls remaining in the copied home |
| catalog | unverified | cached catalog stale |
| agent:claude | healthy | hook event stored; this does not prove a successful model call |
| native-memory:claude | warning | native Claude memory enabled |
| agent:codex | healthy | hook event stored, trusted |
| agent:grok | degraded | CLI exited 1 before probe completion |
| agent:pi | healthy | hook event stored, wired |
| unrecognized-agents | healthy | no unrecognized invocation |
| pi | healthy | no Pi diagnostics |

## Service snapshot

Both services had completed analysis of base `72bcb23b` when queried: Sonar analysis `c12be2c1-385f-4027-9949-3e8e011e9862`, 15 open issues; Codacy analysis ended `2026-09-08T18:12:25.452Z`, 38 current issues. Neither current ID set contains an ID outside the frozen inventory. These are live service counts; the ledger's `open 0` describes proposed dispositions and is not a service zero claim.
