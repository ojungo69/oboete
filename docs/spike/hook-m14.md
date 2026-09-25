# Spike: hook write cost (M14)

Throwaway (spec 8.3). Plan: docs/milestone-1-plan.md, Spike 1. Run 2026-09-26.

## Question

What does one hook write cost as a spawned process, under `synchronous=FULL` (and `fullfsync` on macOS), with 1, 64 and 256 KB tool outputs, with and without a full redaction scan and per-record zstd, on the three target machines? Spec 8.2 M14's provisional line is 20 ms p95; milestone 2 sets the real line from its own measurement of the real hook.

## Harness

`docs/spike/hook-m14/` (its own Cargo workspace, outside the oboete build). `src/main.rs` includes the shipped `src/redact.rs` through `#[path]`, so the redaction timed is the one that ships. Each timed sample spawns the harness once (`hook-m14 one <db> <bytes> <redact> <zstd>`): process start, open, `PRAGMA journal_mode=WAL; synchronous=FULL` (plus `fullfsync` and `checkpoint_fullfsync` on macOS), full redaction, optional zstd level 3, one INSERT keyed by (device, seq). 300 samples per row after one warm-up. The payload is tool-output-like text (paths, code, Japanese), with every 20th line carrying words that wake redaction rules (api, key, token, password) but no secret.

- WSL: `cargo build --release && ./target/release/hook-m14 run ~/.cache/hook-m14 300` (ext4, like ~/.oboete).
- Windows native: built on WSL with `cargo zigbuild --release --target x86_64-pc-windows-gnu` (zig 0.16 from the `ziglang` package; this PC has no MSVC build tools), run from `C:\Users\jura` (NTFS, Defender scanning included, as a Windows user gets it).
- M1 iMac: built and run over SSH in `~/oboete-probe` (macOS 26.6.2).

## Results

| Machine | Output | Redaction | zstd | p50 ms | p95 ms | p99 ms | max ms |
|---|---|---|---|---|---|---|---|
| WSL (ext4) | 1 KB | off | off | 5.8 | 7.3 | 8.1 | 8.9 |
| WSL (ext4) | 1 KB | full | off | 7.7 | 9.4 | 10.2 | 22.8 |
| WSL (ext4) | 1 KB | full | level 3 | 8.2 | 9.7 | 10.8 | 22.2 |
| WSL (ext4) | 64 KB | off | off | 6.9 | 9.8 | 18.7 | 25.2 |
| WSL (ext4) | 64 KB | full | off | 11.7 | 13.4 | 13.8 | 29.7 |
| WSL (ext4) | 64 KB | full | level 3 | 11.8 | 13.5 | 14.3 | 18.6 |
| WSL (ext4) | 256 KB | off | off | 8.7 | 9.9 | 10.8 | 21.9 |
| WSL (ext4) | 256 KB | full | off | 18.7 | 20.3 | 21.5 | 21.8 |
| WSL (ext4) | 256 KB | full | level 3 | 18.0 | 20.0 | 21.0 | 30.3 |
| Windows native (NTFS) | 1 KB | off | off | 10.1 | 11.5 | 12.1 | 12.4 |
| Windows native (NTFS) | 1 KB | full | off | 12.2 | 13.5 | 14.1 | 67.0 |
| Windows native (NTFS) | 1 KB | full | level 3 | 12.6 | 14.9 | 22.1 | 67.5 |
| Windows native (NTFS) | 64 KB | off | off | 12.2 | 14.0 | 14.9 | 16.5 |
| Windows native (NTFS) | 64 KB | full | off | 18.3 | 20.2 | 21.6 | 150.5 |
| Windows native (NTFS) | 64 KB | full | level 3 | 18.6 | 27.7 | 34.2 | 100.1 |
| Windows native (NTFS) | 256 KB | off | off | 24.8 | 34.1 | 140.9 | 545.0 |
| Windows native (NTFS) | 256 KB | full | off | 34.7 | 41.4 | 93.1 | 142.6 |
| Windows native (NTFS) | 256 KB | full | level 3 | 28.6 | 34.7 | 55.6 | 116.2 |
| M1 iMac (APFS, fullfsync) | 1 KB | off | off | 21.1 | 23.1 | 26.3 | 31.4 |
| M1 iMac (APFS, fullfsync) | 1 KB | full | off | 23.0 | 24.9 | 26.9 | 31.1 |
| M1 iMac (APFS, fullfsync) | 1 KB | full | level 3 | 23.0 | 24.9 | 25.2 | 27.0 |
| M1 iMac (APFS, fullfsync) | 64 KB | off | off | 22.0 | 24.0 | 26.6 | 27.1 |
| M1 iMac (APFS, fullfsync) | 64 KB | full | off | 21.9 | 24.0 | 28.8 | 33.1 |
| M1 iMac (APFS, fullfsync) | 64 KB | full | level 3 | 22.0 | 24.0 | 25.8 | 28.5 |
| M1 iMac (APFS, fullfsync) | 256 KB | off | off | 24.1 | 26.1 | 27.2 | 30.7 |
| M1 iMac (APFS, fullfsync) | 256 KB | full | off | 25.0 | 27.0 | 30.7 | 38.1 |
| M1 iMac (APFS, fullfsync) | 256 KB | full | level 3 | 25.0 | 26.2 | 28.1 | 32.0 |

## What it says for milestone 2

- **The slowest machine depends on the write size.** Up to 64 KB it is the iMac, where fsync alone sets the floor: with `fullfsync`, even a 1 KB write without redaction takes 23 ms p95; redaction and size add only 1-4 ms. So 20 ms p95 cannot hold on macOS while every hook write is an `F_FULLFSYNC`. Milestone 2 has to choose between: keeping `fullfsync` and setting the line from it (about 25-27 ms p95), dropping `fullfsync` to `synchronous=FULL` alone on macOS (plain fsync, weaker on Apple SSDs), or not syncing in the hook at all and letting the worker sync a spool. This is a durability decision, not a tuning one.
- **At 256 KB Windows native is the slowest machine** (41.4 ms p95 with full redaction, against 27.0 ms on the iMac). 64 KB with full redaction: 20.2 ms p95 (27.7 ms with zstd). 256 KB: 34-41 ms p95, with tails past 100 ms (Defender). Without redaction the 256 KB write is still 34 ms p95, so size, not redaction, is Windows' cost.
- **WSL meets 20 ms for everything but the largest case**: 256 KB with full redaction is 20.3 ms p95.
- **Redaction costs about 5 ms per 64 KB on WSL and Windows** (64 KB: 9.8 → 13.4 ms on WSL, 14.0 → 20.2 ms on Windows). On the iMac it is hidden under fsync.
- **zstd at write time does not pay for itself below 256 KB**, and at 256 KB it helps only on Windows (41.4 → 34.7 ms p95), where fewer bytes reach NTFS. Per-record compression can move to the worker.
- **The head-and-tail rule of spec 2.2** (outputs above a measured size keep head and tail) is needed on Windows at 256 KB if the line stays near 20 ms; with the line set from the iMac's fsync floor (about 25-27 ms), Windows 256 KB still misses it.

Direction, not the line: milestone 2 measures the real hook (its own binary, the real schema, the real ledger) on the same three machines and sets M14 from the slowest machine at each size the hook writes, after the head-and-tail rule has capped that size.
