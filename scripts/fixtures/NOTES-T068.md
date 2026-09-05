# T068 fixture replay notes

Replay lives in `src/fixture/replay.ts` (`oboete fixture replay`). `scripts/fixtures/replay.mjs` only
forwards to `dist/oboete.mjs`. `fillBytes` is reimplemented in TypeScript with the same alphabet as
the generator (`The quick brown fox jumps over the lazy dog. `); the `.mjs` is not imported from `src/`.

Database is created with `openDatabase({ path, timeoutMs })` once before the first hook. The hook
never migrates. Config file is absent (schema default preset `workers-ai`); child env has no
provider credentials, so observe takes the `no_provider` rule-based path.

Worker RSS is Linux-only: `/proc/<pid>/status` `VmHWM`, 50 ms poll, on the `observe` processes
replay itself spawns. After SessionEnd the hook may already have spawned a detached worker; replay
waits for the lease then runs `observe` (often an empty pass). Peak RSS is therefore of those
spawned runs, not necessarily the hook-spawned worker.

Pending session-start: after the penultimate SessionEnd of each agent, replay skips its own
`observe` until that agent's last SessionStart. The hook-spawned worker is not killed, so a
non-adjacent last session can still land on the ready path. Recorded, not forced.

Pi capture prints no pack (`INJECTION_BRANCHES.pi` is empty). Replay also spawns
`inject --agent pi --kind start|prompt` after `session_start`/`input`, matching the extension.

`scripts/fixtures/smoke-first-sessions.mjs` is deleted; its placeholder expansion and hook command
are in `replay.ts`. `NOTES-T067.md` still names the smoke script; that file is outside this task.

This run (2026-09-05T19:32:55Z, ~6 min, load `2.62 7.76 10.70`): SC-002 pass (p99 290.8 ms, 99.5% ≤ 300 ms; Grok PostToolUse max 1187 ms and Stop p99 318 ms still inside the 99% cut). SC-003 pass (69.5 MB). SC-005 pass. SC-010 pass. SC-009 fail 17.5% (7/40): rule-based summaries keep the first prompt, not later planted facts. Four directive phrases reached memories/packs via those summaries (allowed in raw_events: 64 rows). Pending session-start n=4, none carried `summary_pending` (hook-spawned worker won). Ready max 582.8 ms. Grok compact left no `last_compaction_key` row.

Did not touch `src/capture.ts`, privacy, injection, worker, db, tests, or the generator.
