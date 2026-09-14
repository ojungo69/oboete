# Resident worker contract

This contract implements T047. It extends [memory-core.md](memory-core.md) — whose 2026-09-10
amendment adds resident waiting for due retries while retaining the one-shot path — and
[work.md](work.md). The owner's 2026-09-11 decision scopes the resident to a coding session: capture
starts it, no init system does, and it never outlives the work that needs it.

## What changes

Today a hook spawns `oboete observe` when the lease looks free, that process runs bounded passes
until its queue predicate is empty, and a retry that becomes due afterwards waits for the next
coding event. The resident keeps one process alive across idle periods so a due retry wakes on time.

`oboete observe` stays one-shot with its current arguments and exit codes; manual processing,
migration safety and reproducible tests keep a run that terminates. `oboete observe --resident` is
the resident, and is what `src/capture-command.ts` spawns while `[worker] resident` is true.

### Epochs, and why the lease token rotates

The current queue predicate excludes a waiting source when any earlier attempt carries **the lease
token now held** (`DUE_SOURCE_SQL` in `src/worker/batches.ts`, whose comment says deferred sources
"wait between bounded worker runs, never in a resident retry loop"). A process that keeps one token
forever therefore never retries its own failures — the single thing T047 exists for. Measured: at one
overdue timestamp the predicate returns 0 eligible rows for the original owner and 1 for any other.

The resident's unit of work is an **active epoch**. An epoch begins only when the idle probe below
finds work, and it begins by rotating the lease to a fresh token inside one `BEGIN IMMEDIATE`
transaction that asserts the outgoing token: `UPDATE worker_lease SET owner_token = <new> WHERE
owner_token = <outgoing>`, keeping `pid` and `started_at`. It is not a release followed by
`claimLease`, so the lease is never free and no second worker is invited in. Rotation happens at the
start of an epoch only, never while idle, and never with an operation in flight — the previous epoch
has ended, which means no batch of it is being reserved, requested or applied.

Each epoch is then exactly today's bounded run: same predicate, same suppression, same fences, same
at-least-once provider attempt with exactly-once applied effects. Nothing about retry semantics is
redefined and no schema changes.

**The idle probe runs with a token that was never issued.** This is the part that is easy to get
wrong twice: asking "is anything due?" with the token the resident currently holds re-applies the
same exclusion one layer up, so the probe would report an empty queue forever and the next epoch
would never start. The probe is a read that passes the empty string as the owner token, which
matches no attempt, so a row whose `retry_after` has passed is visible to it.

The epoch is also the reset point for the three pieces of state a one-shot run allocates once: the
run deadline, the provider-state map and the ancestor cache. Each is established per epoch. No new
cache layer is introduced — these three already exist and only their lifetime changes.

## Ownership

Ownership stays the existing lease: `claimLease` under `BEGIN IMMEDIATE`, `assertLease` in every
fenced transaction, 6,000 ms staleness and 60,000 ms future skew from `lease-clock.ts`. One
heartbeat schedule runs for the whole process — the same two-second cadence the bounded run already
uses — and the heartbeat timestamp is read at the fenced write rather than inherited from a receipt,
so a late apply cannot overwrite a newer heartbeat with an older one.

`isLeaseFree` is a read hint, so two captures can both spawn; `claimLease` decides. The loser keeps
today's behaviour (`another_worker`, exit 0) and is not a lease-loss case. What this contract
promises is one valid owner and fenced effects, not one spawned process. A resident that loses the
lease later stops touching sources, batches and memories; it still writes its own log lines and the
conservative provider-quota accounting the existing code records without a token, and it never
releases or overwrites a successor's lease row.

## Waking

A resident alternates idle waits with active epochs. An idle wait ends at the sooner of a fixed
2,000 ms poll and the earliest `retry_after` among otherwise-due rows, recomputed after each wake.
Capture writes rows and does not signal the resident: one indexed read per poll is cheaper than an
IPC channel, and a missed signal cannot strand work. Adaptive backoff is deliberately absent —
heartbeats and control checks have to run during any backoff anyway, so the extra scheduling state
removes no wakes, and a cap would add that much latency before a stop or an upgrade is noticed.

Idle cost is measured as what it is: one indexed read plus a heartbeat write per two seconds, the
control checks below, and whatever the existing empty-pass maintenance writes. Target: under 0.5% of
one core averaged over ten idle minutes, with RSS flat across a long run (T042). No transaction and
no unfinished statement iterator is held across a sleep or a provider wait, so a long-lived resident
cannot pin the WAL; the worker keeps SQLite's default auto-checkpoint and the existing per-batch
PASSIVE checkpoint, and no new checkpoint machinery is added unless measurement demands it.

Active and idle budgets are elapsed-time budgets measured on a monotonic clock, while the lease
staleness rule and persisted retry timestamps stay on the wall clock they are already written on. A
clock jump therefore cannot end an epoch early, prolong a lifetime, or reorder a control.

## Controls and exit

Controls are checked before each epoch, before each batch inside an epoch, and before each provider
request. The first that holds ends the process cooperatively with exit 0:

| Condition | Reason | Who sets it |
| --- | --- | --- |
| `paused` sentinel exists | `paused` | `oboete pause` |
| `worker-stop` sentinel exists | `stopped` | `oboete observe --stop` |
| `config.toml` fingerprint differs from the one read at start, or the file became unreadable | `config_changed` | any config or consent edit |
| the resolved engine artifact's identity changed, vanished or became unreadable | `upgraded` | an install or upgrade |
| no capture activity and no completed processing for `[worker] idle_exit_ms`, with nothing due inside it | `idle_exit` | time |
| the lease is held by another owner | `lease_lost` | takeover |

Cooperative exit is 0 even when the epoch applied a fallback summary, which the existing exit
calculation would otherwise report as 1; storage and log-write failures keep their current
precedence and codes, and the existing `max_run`, `batch_error`, `worker_error` and `storage_error`
reasons are unchanged. `SIGTERM` and `SIGINT` end the current wait, run the shutdown sequence below,
and exit 0.

Shutdown, in order: stop beginning new batches; finish or explicitly abort the operation in flight;
leave pending and running state and every cursor as it is; clear timers; remove the `worker-stop`
sentinel before the lease is released, so a capture that spawns the moment the lease frees starts a
resident rather than consuming the sentinel and exiting; recheck the idle predicate
inside the same transaction that releases the lease, so work captured a moment earlier cannot be
stranded by a hook that saw the lease occupied; release only if the row still carries this token;
close the database. Release alone never loses an accepted batch — the reservation marks it durably
and the fenced apply commits effects, terminal state and settlement together — but releasing during
a request discards that response, which the at-least-once rule already permits.

`paused` is persistent: capture already declines to do anything while it exists, so nothing respawns
until `oboete resume`. `worker-stop` stops the resident that is running: the exiting process removes
the sentinel, and the next capture may start a new one. This contract makes no claim that a stopped
resident stays stopped; that is what `pause` is for.

Both controls are files rather than database rows because `openDatabase` refuses to migrate while a
live lease is held (`MigrationBusyError`). A resident makes that refusal durable: if stopping needed
a database write, a newly installed bundle could neither migrate nor stop the resident blocking the
migration. Reading a sentinel and stating a file's identity need no database at all, and the fixed
poll bounds the wait at two seconds.

An upgrade is observed on the artifact the process actually loaded. Capture respawns
`process.argv[1]`, the launcher, which resolves its real path and imports the sibling `engine.mjs`;
the version constant is embedded at build time, so comparing it with itself proves nothing. The
resident stats the resolved engine artifact — device and inode, size, mtime, and the target of a
symlink — and exits when that identity changes, when the artifact disappears, or when it cannot be
read. During the window where an install has removed the artifact and not yet written the new one,
exit is still the answer: the next capture spawns whatever is installed by then.

Because a schema-behind capture closes its handle and spools without spawning a worker, the exit of
an old resident is not by itself enough to get the new bundle running. Capture therefore attempts a
best-effort worker start after a schema-behind spool as well, when the lease is free or stale. The
migration fence itself is unchanged.

## Idle exit, and what "session-scoped" means

The lease belongs to the data directory, not to one native session, and a session row stays active
until an explicit session-end capture that a crashed agent may never send. Liveness is therefore not
read from session or work status. Idle is measured from observable events: the newest capture
activity (`sessions.last_captured_at`) and the newest completed processing, on a monotonic timer.
`idle_exit_ms` defaults to 900,000 ms, bounds 60,000–86,400,000.

A retry due beyond the idle window is not a reason to stay alive. It is preserved for the next spawn
exactly as it is today — which is the honest reading of a session-scoped resident, and the reason
the one-shot path and the daily cron both remain.

## Configuration and credentials

`[worker]` is new in the typed schema and in the known key paths: `resident` (boolean, default true)
and `idle_exit_ms`. An absent `config.toml` fingerprints as absent rather than as a change; an
unreadable or malformed one ends the process with `config_changed` rather than running on stale
settings. `--resident` on the command line wins over `resident = false`, and a configuration change
between the hook's decision and the worker's claim is resolved by the worker's own read.

Per-send consent and privacy rechecks stay exactly where they are; the fingerprint is a lifetime
control, not a substitute for them. Credentials come from the process environment, so a rotated key
reaches a newly spawned process only — documented, not worked around.

`oboete doctor`'s existing worker item gains the effective `[worker]` settings and the stop state.
It keeps reporting an owner rather than a mode: the lease row has no mode column, and a live lease
plus `resident = true` cannot tell a resident from a manual `observe`.

## Logging

One line per epoch with the counts the bounded run already reports, and one `run end` line naming
the reason from the table. Idle waits write nothing, so an idle day does not grow the log.

## Verification

1. A source that fails and becomes due again is retried **in the same process**, in the next epoch,
   and its effects apply once. This is the test the current owner-token predicate fails. It has two
   halves, and both must be asserted: the idle probe after the failed epoch sees the row once
   `retry_after` has passed, and the epoch that follows actually batches it.
2. Two captures racing: exactly one process claims the lease, the loser exits 0 as `another_worker`
   with no source, batch or memory write, and the winner's lease row is never overwritten.
3. Each control row exits 0 with its own reason and leaves the queue intact — `paused`,
   `worker-stop`, a rewritten config, an unreadable config, a changed engine artifact, a removed
   engine artifact, idle timeout, lost lease — including one case where the epoch had applied a
   fallback summary, proving the exit is still 0.
4. `worker-stop` is consumed by the exiting resident before it releases the lease, a later capture
   starts a new one, and a capture racing that release starts a resident rather than finding the
   sentinel; `paused` is not consumed and nothing starts until `resume`.
5. An upgrade sequence: old resident running, new bundle installed, resident exits `upgraded`,
   schema-behind capture spools and starts a worker, the migration runs, the spool is recovered.
6. Shutdown at each boundary — before reservation, during a request, after a response and before
   apply, and concurrently with a capture at release time — never commits an effect twice and never
   leaves work that no later spawn can reach.
7. Heartbeat under load: a long synchronous maintenance stretch and a delayed apply do not let the
   lease go stale, and the apply cannot write an older heartbeat over a newer one.
8. Crash: `SIGKILL` mid-batch. Takeover is possible more than 6,000 ms after the last heartbeat;
   the running batch is reclaimable by another owner 120,000 ms after its claim. Those two latencies
   are asserted separately, and the reclaimed-batch count is reported separately from the spool
   recovery count.
9. Clock changes: a forward jump, a backward jump and suspend/resume leave epoch budgets and control
   ordering correct.
10. `resident = false` reproduces today's one-shot receipts, including the trigger and budget
    conditions under which capture does not spawn at all, and the existing `observe` suites pass
    unchanged on both supported Node versions.
11. Idle cost and RSS over a long run meet the targets, measured with many distinct sessions,
    repositories and retries rather than an empty process, and with concurrent captures and a
    held reader to show the WAL recycles.
