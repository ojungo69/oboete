# T068 fixture replay notes

Replay lives in `src/fixture/replay.ts` (`oboete fixture replay`). `scripts/fixtures/replay.mjs` only
forwards to `dist/oboete.mjs`. `fillBytes` is reimplemented in TypeScript with the same alphabet as
the generator (`The quick brown fox jumps over the lazy dog. `); the `.mjs` is not imported from `src/`.

Database is created with `openDatabase({ path, timeoutMs })` once before the first hook. The hook
never migrates. Config file is absent (schema default preset `workers-ai`); child env has no
provider credentials, so observe takes the `no_provider` rule-based path.

Lease hold. Replay writes its own token into `worker_lease` row 1 (`LEASE_TOKEN`, fresh heartbeat)
before every `SessionEnd`/`session_shutdown` hook, so `isLeaseFree` is false and the hook spawns no
detached worker; replay then releases the lease and runs `observe` itself, which is the worker run
whose RSS is measured. For one session start per agent (the last one, with the hold opened at that
agent's last session end preceding it) the lease stays held through the start hook, so the pack must
take the pending path; the hold is what makes that sample deterministic (4/4 carried
`summary_pending` on the recorded run; the fixture's tail is shaped so the latest session state at
each of those starts is a pending one). Workers the hook spawns at the head of a session block while
the lease is free are found through `worker_lease.pid` by a 50 ms poll and their VmHWM counts too.

Every printed pass/fail feeds the exit code: capture (SC-002, pooled per spec), injection (every
group, every sample ≤ 300 ms), session start (ready ≤ 300 ms; pending n > 0, all packs pending,
≤ `INJECTION_DEADLINE_MS`), SC-003, SC-005, SC-009, SC-010, lifecycle, directives, hook exits.

Pi capture prints no pack (`INJECTION_BRANCHES.pi` is empty). Replay also spawns
`inject --agent pi --kind start|prompt` after `session_start`/`input`, matching the extension.

Recorded run (2026-09-06T03:10:26Z, ~4 min, load `0.38 0.42 1.40`): SC-002 pass (p99 181.2 ms,
100% ≤ 300 ms, n=717); injection pass (p99 203.1 ms, worst group codex/UserPromptSubmit 207.1 ms);
session start pass (ready max 198.3 ms; pending max 1185.7 ms, 4/4 `summary_pending`); SC-003 pass
(111.2 MB over 43 replay-spawned + 42 hook-spawned worker runs); SC-005, SC-010, lifecycle
(fork 3, resume 4, compact 4, clear 4), directives (0 after FR-021), hook exits (1143/1143) pass.
SC-009 fail 17.5% (7/40): rule-based summaries keep the first prompt, not later planted facts, so the
exit code is 1 until the fixture is replayed with a provider. An earlier run at load 10.79 pushed
codex UserPromptSubmit p99 to 301.7 ms; run the replay on a quiet machine. The evidence's `Commit:`
line names the tree the bundle was built from, one commit before the one that records it.

Did not touch `src/capture.ts`, privacy, injection, worker, db, tests, or the generator.
