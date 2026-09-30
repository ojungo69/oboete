# Read-hook timing: today's baseline (milestone 4, Task 0)

`oboete replay <fixture> --home <home> --spawn-sample 0 --read-sample 30` replays the fixture, then times the read path twice:

- **Cold:** before any consumer has run on the replayed events. For a checkout the worker has never built, SessionStart shows nothing (spec 4.2: the hook runs before the worker is up).
- **Warm:** after `worker::drained` has run every consumer.

Each arm spawns 30 `oboete hook claude SessionStart` and 30 `oboete hook claude UserPromptSubmit` processes, as an agent runs them: the write with its fsync, the read of what is injected, and the worker-lock attempt. It also times 30 in-process reads of what SessionStart shows (`hook::inject_text`, `oboete inject`'s text). This process holds the worker lock, so the hooks start no worker. What each arm's hooks record is forgotten after it (tombstones, applied by the next drain), so neither the warm arm nor a later run on the same home sees a start at the time of the run as the checkout's newest event. The runs below were taken before that change: their warm manifest was as of the time of the run, not of the fixture's last event.

These numbers are a baseline and decide nothing. SessionStart shows the manifest and the current decisions, as milestone 3 left it. The read-hook line is set once in Task 12, from the first measurement of the delivered SessionStart's warm path on the slowest machine (spec 8.2, Read hook row).

## Fresh home, the fixture of record only

`events-1000.jsonl` (255 Claude Code events), replayed into an empty home, 3 runs each, a new home for each run. There are no claims, so warm SessionStart shows the manifest only (about 1,160 bytes). Milliseconds, p50 / p95 of the run whose p95 was the worst of the three.

| Machine | SessionStart, cold | SessionStart, warm | Prompt, warm | In-process read, warm |
|---|---|---|---|---|
| WSL (ext4) | 10.8 / 13.4 | 11.8 / 14.8 | 11.7 / 19.8 | 1.0 / 1.2 |
| Windows native, GNU build (NTFS, Defender on) | 20.6 / 23.3 | 27.3 / 32.0 | 21.0 / 23.1 | 8.6 / 9.8 |
| M1 iMac (APFS, fullfsync) | 23.1 / 35.0 | 27.0 / 29.1 | 19.9 / 26.1 | 1.4 / 2.0 |

## A home with claims (WSL)

A copy of milestone 3's dev arm ov-B1 (knowledge.db 955 MB, 23 claims of the repository used), with the fixture replayed into a checkout of that repository (an empty git repository whose `origin` names it). 3 runs on the same home.

| Run | SessionStart, cold | SessionStart, warm | Prompt, warm | In-process read, warm |
|---|---|---|---|---|
| 1 | 13.3 / 16.1 (nothing shown) | 30.7 / 40.1 | 29.1 / 32.8 | 1.4 / 1.6 |
| 2 | 13.5 / 15.0 | 13.0 / 15.6 | 11.3 / 12.7 | 1.4 / 1.5 |
| 3 | 13.3 / 15.2 | 13.1 / 15.2 | 11.1 / 12.4 | 1.4 / 1.5 |

Warm SessionStart showed 2,474 bytes, 1,789 characters of text: the manifest and the repository's current decisions.

## Reading

- **Run 1 of the home with claims** is 25 ms slower warm than runs 2 and 3, and so is its prompt hook, which reads nothing. The first hooks after the drain were slow and the later ones fast: a separate trial of 30 prompt hooks right after one run gave p50 25.1 ms, and the next 30 gave 11.5 ms. Why is not measured. Task 12's measurement discards a warm-up before it times.
- **In runs 2 and 3, "cold" showed the stored manifest.** Run 1's drain had built that checkout's manifest, and a manifest stays shown while the worker is behind. So cold measures "not built yet" only on a checkout's first run, as in the fresh homes.
- **The read is small on WSL (1-1.5 ms) and large on Windows (9-10 ms).** Opening raw.db and knowledge.db and reading the manifest costs Windows about 8 ms, most of the difference between its cold and warm SessionStart. Where it goes is not measured; file opens under Defender are the first suspect.
- **On the iMac, cold's p95 (32-35 ms) is above warm's (27-29 ms)**, while its p50 is below. The cold arm runs first, right after the replay's writes: a warm-up again. The iMac's write floor is its fullfsync (25.2 ms p95 in milestone 2's M14 line), so its read adds little.
- **A checkout with claims but no manifest row shows nothing** (run 1, cold: the repository had 23 claims). Milestone 4's plan changes that (D4).

## Machines

- WSL: the owner's PC, Ubuntu on WSL 2, ext4, release build of 43eefdb.
- Windows native: the same PC, `cargo zigbuild --target x86_64-pc-windows-gnu`, NTFS, Defender on, run from WSL through interop.
- M1 iMac: 8 GiB, macOS 26.6.2, release build of 43eefdb built on it.
