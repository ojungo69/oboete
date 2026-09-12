# Batch D continuation resource comparison

Base `72bcb23b18feec02858d12c41443be1b45ffaa01` and candidate
`ebe687dcf99103cf3965649b969bb0f2bb4d5d62` were freshly installed and built in separate
Git checkouts with `npm ci --no-audit --no-fund` and `npm run build`; both commands passed.
The candidate engine hash matches the dogfood bundle:
`97bd89dd1771dadc77ca8052491f565c38131c123ef4c28b8ef981185d739761`.

The unmodified cold-start and replay commands ran serially on Node 24.16.0 on the same machine
within one hour, after the local tests and native-agent probes had finished. No provider preload,
credential override or numeric-bound change was used. [Run metadata and every timing comparison](batch-d-continuation-resources.json)
record the exact start/end times, source/bundle hashes and exits; full replay output is preserved
for [base](batch-d-continuation-base-replay.json) and [candidate](batch-d-continuation-candidate-replay.json).

| Command | Base exit | Candidate exit |
|---|---:|---:|
| `node scripts/measure-cold-start.mjs` | 0 | 0 |
| `node dist/oboete.mjs fixture replay test/fixtures/events-1000.jsonl --json` | 1 | 1 |

Every cold-start and per-agent/event injection median differs by at most 15% from the base,
and none of their maxima exceeds the base maximum by more than 15% or crosses its budget;
ready/pending session-start maxima also meet those upper bounds.
Both replays pass every reported bound except SC-009 recall: Japanese 4/20, English 3/20,
overall 7/40 (17.5%), against 90%. Therefore T032/T039 remain open. The ordinary replay still
strips provider credentials; [the separate diagnostic](batch-d-provider-diagnostic.md) documents
why this result cannot establish real-provider readiness or retention.

| Replay measurement | Base | Candidate |
|---|---:|---:|
| Capture p99 (ms) | 209.0 | 197.1 |
| Injection median (ms) | 186.3 | 186.9 |
| Injection maximum (ms) | 265.6 | 261.9 |
| Ready session-start maximum (ms) | 223.5 | 206.9 |
| Pending session-start maximum (ms) | 1203.1 | 1204.8 |
| Worker peak RSS (KiB) | 125432.0 | 123808.0 |
| Successful hook exits | 1,143/1,143 | 1,143/1,143 |
| Recall | 7/40 | 7/40 |
| Secret / directive / duplicate leaks | 0 / 0 / 0 | 0 / 0 / 0 |
| Lifecycle | Pass | Pass |

## Base cold-start

- Date: 2026-09-09T07:33:53.031Z
- Node versions: `$HOME/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0)
- Commit: `72bcb23b`
- Bundle: `dist/oboete.mjs` (1598418 bytes)
- Samples: 30 measured runs after 3 warm-up runs per scenario
- Measurement attempts: run 1 load `0.77 0.67 0.82 1/2773 3761671`; run 2 load `3.15 1.25 1.00 1/2767 3763856`; kept run 1 (lower 1-minute load average)
- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`

Load average next to this table (kept run 1, before the measurement set): `0.77 0.67 0.82 1/2773 3761671`

| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |
|---|---|---:|---:|---:|---:|---|---|---:|---|
| v24.16.0 | `--version` | 0 | 53.0 | 57.0 | 58.9 | n/a | n/a | 100 ms | pass |
| v24.16.0 | hook small, DB present | 730 | 169.0 | 177.4 | 178.9 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook clean 200 KB, DB present | 206768 | 176.1 | 206.5 | 243.9 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook secret-dense 200 KB, DB present | 206772 | 183.2 | 195.5 | 203.7 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook small, DB absent (spool) | 742 | 162.1 | 170.5 | 171.0 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |

## Candidate cold-start

- Date: 2026-09-09T07:38:58.742Z
- Node versions: `$HOME/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0)
- Commit: `ebe687dc`
- Bundle: `dist/oboete.mjs` (1600907 bytes)
- Samples: 30 measured runs after 3 warm-up runs per scenario
- Measurement attempts: run 1 load `1.31 1.21 1.04 3/2934 3787898`; run 2 load `1.35 1.22 1.05 1/2930 3790042`; kept run 1 (lower 1-minute load average)
- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`

Load average next to this table (kept run 1, before the measurement set): `1.31 1.21 1.04 3/2934 3787898`

| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |
|---|---|---:|---:|---:|---:|---|---|---:|---|
| v24.16.0 | `--version` | 0 | 56.2 | 61.0 | 63.0 | n/a | n/a | 100 ms | pass |
| v24.16.0 | hook small, DB present | 730 | 174.2 | 183.6 | 186.0 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook clean 200 KB, DB present | 206768 | 179.2 | 189.0 | 190.7 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook secret-dense 200 KB, DB present | 206772 | 182.0 | 190.4 | 203.3 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook small, DB absent (spool) | 742 | 163.3 | 170.9 | 172.8 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |
