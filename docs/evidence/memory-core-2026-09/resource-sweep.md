# Retained-history resource sweep (T042, SC-008)

What a worker costs while a reader holds the database open and capture keeps arriving. The harness is
`scripts/measure-resources.mjs`; it runs the product's own binaries against a temporary home and reads
only what the product writes (`logs/observe.log`, `worker_lease.pid`, `/proc/<pid>/status`, the
database and its WAL).

Run on 2026-09-17 against `main` `6b683213`, on both supported Node versions. Nothing else was
running on the machine except the load already noted per run.

## What it measures

- **Phase A** replays `test/fixtures/events-1000.jsonl` (1,051 lines) through the real hooks and the
  resident worker, with `[observer] preset = "none"`, and reads the replay's own bounds.
- **Phase B** holds a read-only connection open for 20 seconds while 20 sessions of 9 prompts each
  keep capturing, sampling the database size, the WAL size, the spool and every worker pid's
  `VmRSS`/`VmHWM` about every 200 ms, then drains, stops and samples once more.

`preset = "none"` means no summarizer runs, so every source ends `waiting` as a deferred
`no_provider`. That is the point of the sweep: it measures the cost of holding retained history, not
the cost of generating from it. SC-009 recall is 0/40 for the same reason and is reported, not gated.

## Results

| | Node 24.16.0 | Node 22.23.1 | Bound |
|---|---|---|---|
| worker peak `VmHWM` (phase A) | 110,800 KiB (108.20 MiB) | 103,308 KiB (100.89 MiB) | < 150 MiB |
| worker peak `VmHWM` (phase B hold) | 109,692 KiB | (same run shape) | reported |
| growth per 1,000 events | 4,828,780 bytes | 4,836,575 bytes | recorded, not gated |
| capture hooks (phase A) | p99 207.8 ms, 100.0% ≤ 300 ms (n=717) | — | p99 ≤ 300 ms |
| phase B hooks | n=240, p50 251.0 ms, max 366 ms, 0 non-zero | n=240, p50 235.0 ms, max 318 ms, 0 non-zero | every hook exits 0 |
| WAL peak during the hold | 29,478,632 bytes, back to 0 after the stop | 28,786,472 bytes, back to 0 | grows under a held reader, recycles after |
| spool files at any sample | 0 | 0 | 0 |
| load average at start | 0.49 0.65 1.15 | 2.44 1.38 1.32 | — |

Gated checks, both runs **pass**:

- `retained` — no missing, duplicate or failed-classification source; no failed session; spool empty.
- `not-stuck` — `pending=0`, `liveBatches=0`, `endReason=stopped`, `workerErrors=0`, no bad end
  reason. 1,118 (24.x) / 1,119 (22.x) sources waiting and 4 parked, which is what `preset = "none"`
  produces.
- `wal-recycled` — the WAL grows under the held reader (peak ≈ 28–29 MB, a quarter of the samples
  above the start) and returns to 0 after the product's own stop path runs
  `wal_checkpoint(TRUNCATE)`. 8–9 batches were held open at the peak.
- `rss-bound` — peak `VmHWM` under the 150 MiB bound on both versions.

Reported, not gated (all three are consequences of `preset = "none"` or are tracked elsewhere):

- injection p99 317.5 ms with 96.8% ≤ 300 ms (n=378); the worst group is grok/`UserPromptSubmit` at
  p99 340.3 ms.
- session start: ready max 235.7 ms (n=1); pending max 255.9 ms (n=48), with 46 of 48 packs carrying
  `summary_pending`.
- SC-009 recall 0/40.

## What this run cannot say

- **Long-run growth.** A 20-second hold cannot show it. The seven-day run is #268.
- **Scale.** 1,051 events is the fixture, not the 10,000- and 100,000-event runs of #267.
- **A real summarizer.** With `preset = "none"` nothing is generated, so neither the provider's cost
  nor recall is exercised here.

## Receipts

`/var/tmp/oboete-t042/v6-24.16.0.{md,json,observe.log}` and `v6-22.23.1.{md,json,observe.log}`. The
Markdown is the harness's own report; the JSON is the same data unrounded; `observe.log` is the
worker's log for the run, which is where `endReason`, `workerErrors` and the bad-end set are read
from.
