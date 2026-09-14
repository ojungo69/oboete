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
forever therefore never retries its own failures — the single thing T047 exists for. Measured at one
overdue timestamp: the outgoing token sees 0 eligible rows, while the empty string and a fresh token
each see 1.

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

"Exactly today's bounded run" is literal: an epoch **is** the existing pass loop, with one
subtraction and nothing else. The subtraction is the easiest thing to get wrong when lifting that
body: **an epoch never calls `releaseLease`.** Today's loop ends by releasing in
`releaseEmptyPass`; a resident that kept that line would release and re-claim at every epoch, which
is precisely what rotation exists to avoid, and a test of the retry case would still pass because a
re-claim also yields a fresh token. What replaces it keeps the other half of that function — the
200 ms wait between passes while the queue is non-empty but undrainable — because dropping it turns
an undrainable queue into a write-transaction busy-spin for the whole epoch budget. Across an idle
wait the lease row's `owner_token` is non-NULL and `pid` is this process; the token changes only at
the start of an epoch.

Writing a second pass loop beside the first is the failure mode to avoid here: the two drift, and
the copy silently loses whichever guard was living in the original. The same applies to the idle
probe — it is `queueIsEmpty`, not a parallel predicate that answers nearly the same question.

The two `yieldAfterPass` outcomes diverge in a resident, and the divergence is the contract, not an
implementation detail. `max_run` ends the **epoch**: the budget is per-epoch, so the loop continues
and the next epoch starts with a fresh budget. `batch_error` ends the **process**, exit 0, exactly
as a one-shot run ends today — a source or summary that fails every attempt then costs one attempt
per spawn, which is today's cadence and needs no new backoff. Neither reason may be rewritten to
`empty`.

**The idle probe is `queueIsEmpty` called with a token that was never issued.** Two things are
easy to get wrong here, and the existing predicate settles both.

The token first. Asking "is anything due?" with the token the resident currently holds re-applies
the same exclusion one layer up, so the probe would report an empty queue forever and the next epoch
would never start. The probe passes the empty string, which matches no attempt — exactly like the
fresh token the epoch is about to rotate to. That correspondence is the point, not a coincidence:
every clause the probe reads binds the token, and one of them (`pendingSummaries`, whose
`NOT EXISTS` contains `DUE_SOURCE_SQL`) reads it with inverted polarity, so only a probe token that
matches no attempt is guaranteed to see what the next epoch will see.

The breadth second. The dueness query alone is not the queue. `queueIsEmpty` in
`src/worker/observe.ts` already counts the four things that make a queue non-empty — a `pending` or
`running` batch, a batchable source cohort, a spool file, and a session awaiting a summary — and a
resident that polled only `DUE_SOURCE_SQL` would idle through a recovered spool file, an orphaned
batch and a pending summary. The probe is therefore that same function with the never-issued token,
not a new query.

`queueIsEmpty` is not extended, and retention is why the probe alone cannot be the only trigger.
A resident holds the lease across idle periods, so no other worker can run the maintenance pass,
and expired rows include `secret` ones — retaining those because nothing happened to be batchable
is a privacy regression, not a latency one. **An epoch therefore also begins when the maintenance
interval, 60,000 ms since the last epoch ended, has elapsed**, whatever the probe says. The
alternative — teaching the probe to ask whether retention would delete anything — was measured and
rejected: that predicate is a correlated `NOT EXISTS` over every row already past `expires_at`, a
range that never empties because cited rows are retained forever, so it costs about 1.6 µs per
retained row on every poll (27.9 ms at 20,000 rows, and rising). A timer pays nothing per poll and
bounds the delay to one minute against a seven-day TTL. Pi-ack file cleanup needs no trigger of its
own: it rides along with any epoch, its material is temporary, and `idle_exit` hands the directory
to the next one-shot run within the idle window.

An epoch may begin and find nothing it can act on: the probe counts a `running` batch that is not
yet reclaimable (`RECLAIM_AFTER_MS`, 120,000 ms after its claim), and until that deadline no owner
can take it. Inside the epoch that is today's behaviour exactly — the pass loop waits 200 ms between
passes while the queue is non-empty but undrainable — so nothing new is needed there. What keeps it
from becoming a rotate-and-log treadmill is that an epoch which changed no counts writes no epoch
line, and that the idle wait which follows runs to the nearest of the fixed poll, the earliest
`retry_after` and the earliest reclaim deadline.

The epoch is also the reset point for the three pieces of state a one-shot run allocates once: the
run deadline, the provider-state map and the ancestor cache. Each is established per epoch. No new
cache layer is introduced — these three already exist and only their lifetime changes.

## Ownership

Ownership stays the existing lease: `claimLease` under `BEGIN IMMEDIATE`, `assertLease` in every
fenced transaction, 6,000 ms staleness and 60,000 ms future skew from `lease-clock.ts`. One
heartbeat schedule runs for the whole process — the same two-second cadence the bounded run already
uses — and the ownership half of every fence is evaluated at the write itself, so a late apply can
never commit under a successor's token. The timestamp such a write leaves behind is the `now` its
caller captured, which for an apply trails the clock by one detector pass, so lease freshness is
owed to the heartbeat schedule and not to the fences.

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

Idle cost is measured as what it is, and the honest list is longer than one read: per poll, the
queue probe (one clause per kind of queued work, each on an existing index), the control checks
below — one `config.toml` parse and stat, one stat of the engine artifact, two sentinel `existsSync`
calls — the two `MAX()` reads that date the newest capture and the newest completed processing, the
wake-delay reads, a heartbeat write, and whatever the existing empty-pass maintenance writes. The
two `MAX()` reads have no index behind them; both tables are small in practice, so T042 measures
them rather than an index being added on speculation. Once a minute the list also carries one
maintenance epoch — a token rotation and one empty pass — which T042 counts as idle cost rather
than treating it as work. Target: under 0.5% of
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
| `SIGTERM` or `SIGINT` received | `signal` | an operator or a process manager |

Cooperative exit is 0 even when the epoch applied a fallback summary, which the existing exit
calculation would otherwise report as 1. That exemption is per reason, not per mode: the reasons in
the table above are exempt, and `batch_error`, `worker_error` and `storage_error` keep the existing
fallback rule and the existing codes in a resident exactly as in a one-shot run. `max_run` never
reaches a resident's run-end line, because it ends an epoch rather than the process. `SIGTERM` and `SIGINT` end the current wait, run the shutdown sequence below, and exit 0 with the
reason `signal` — deliberately not `stopped`, because the sentinel rule below is keyed on the reason
and a signalled process must not consume a stop request meant for whoever holds the lease next.

Shutdown, in order: stop beginning new batches; finish or explicitly abort the operation in flight;
leave pending and running state and every cursor as it is; clear timers; remove the `worker-stop`
sentinel — but only when `stopped` is this process's own exit reason, and only while it still owns
the lease — before that lease is released, so a capture that spawns the moment the lease frees
starts a resident rather than consuming the sentinel and exiting; release the lease if the row still
carries this token, **whether or not the queue is empty**; close the database.

The two conditions on that removal are not defensive padding. A resident exiting for `idle_exit`,
`upgraded`, `config_changed`, `paused`, `lease_lost` or `signal` did not act on the sentinel, so
deleting it would silently discard a stop the user asked for; and a process that has already lost
the lease would be deleting a sentinel aimed at its successor.

That last clause is the `releaseMaxRun` path, not the `releaseEmptyPass` path, and the distinction
is load-bearing. `releaseLease` returns `kept` when its recheck finds work, and today that answer
means "do not exit, go around the loop again" — which is a sound answer only while a loop still
exists. A shutdown has already stopped beginning batches and cleared timers, so `kept` there would
leave the lease held by a process that will never work again, and capture would decline to spawn
because the lease looked live. Shutdown therefore releases unconditionally, exactly as FR-009
already requires of a bounded worker ("releases even with queued work so the next hook can respawn
it"). Work left behind waits for the next capture, which is both today's behaviour and, for
`paused`, `stopped`, `config_changed` and `upgraded`, the intended one. Nothing is left for the
recheck to guard at the empty-pass boundary, because a resident does not release there: the idle
probe two seconds later, reading with the never-issued token, is that recheck. Release alone never loses an accepted batch — the reservation marks it durably
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

That fence cannot be deadlocked by a long-lived process, and the reason is worth stating because the
opposite is the obvious fear. `fenceOldWorker` in `src/db/open.ts` refuses only while the lease's
**heartbeat is fresh** — not merely while a token is present — and when the heartbeat is stale it
clears the row itself under the migration's write lock. A resident that is genuinely working
therefore blocks a migration for as long as it works, which is the intended answer; one that was
killed blocks it for `STALE_AFTER_MS`, 6,000 ms. No exit of the resident is required for a migration
to proceed, so the durable refusal a resident could otherwise create does not exist.

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
settings. That includes a file already malformed when the resident starts: the control is evaluated
before the run's own `loadConfig`, so the reason is `config_changed` and not the `worker_error` a
thrown parse would otherwise produce. `--resident` on the command line wins over `resident = false`, and a configuration change
between the hook's decision and the worker's claim is resolved by the worker's own read.

Per-send consent and privacy rechecks stay exactly where they are; the fingerprint is a lifetime
control, not a substitute for them. Credentials come from the process environment, so a rotated key
reaches a newly spawned process only — documented, not worked around.

`oboete doctor`'s existing worker item gains the effective `[worker]` settings and the stop state.
It keeps reporting an owner rather than a mode: the lease row has no mode column, and a live lease
plus `resident = true` cannot tell a resident from a manual `observe`.

## Logging

One line per epoch that did work, with the counts the bounded run already reports, and one `run end`
line naming the reason from the table. Idle waits and epochs that find nothing batchable write
nothing, so neither an idle day nor a two-minute reclaim wait grows the log.

## Verification

1. A source that fails and becomes due again is retried **in the same process**, in the next epoch,
   and its effects apply once. This is the test the current owner-token predicate fails. It has two
   halves, and both must be asserted: the idle probe after the failed epoch sees the row once
   `retry_after` has passed, and the epoch that follows actually batches it.
2. The probe wakes an epoch for each of the four kinds of queued work, one case each: a due retry, a
   spool file, an adoptable `pending` batch and a session awaiting a summary. A `running` batch
   inside its reclaim window wakes an epoch that batches nothing, writes no epoch line, and does not
   spin — the process performs a bounded number of epochs, not one per poll, before the batch
   becomes reclaimable.
3. Two captures racing: exactly one process claims the lease, the loser exits 0 as `another_worker`
   with no source, batch or memory write, and the winner's lease row is never overwritten. And
   rotation under write contention: a lease rotation that meets `SQLITE_BUSY` retries like every
   other write and the resident keeps the lease it still owns, rather than reporting `lease_lost`.
4. Each control row exits 0 with its own reason and leaves the queue intact — `paused`,
   `worker-stop`, a rewritten config, an unreadable config, a changed engine artifact, a removed
   engine artifact, idle timeout, lost lease — including one case where the epoch had applied a
   fallback summary, proving the exit is still 0.
5. `worker-stop` is consumed by the exiting resident before it releases the lease, a later capture
   starts a new one, and a capture racing that release starts a resident rather than finding the
   sentinel; `paused` is not consumed and nothing starts until `resume`. A sentinel written while a
   resident is exiting for some other reason survives that exit — asserted for `idle_exit` and for
   a signalled shutdown, each from a log that only this run wrote.
6. An upgrade sequence: old resident running, new bundle installed, resident exits `upgraded`,
   schema-behind capture spools and starts a worker, the migration runs, the spool is recovered. And
   its crash variant: the old resident is `SIGKILL`ed instead of exiting, and the migration proceeds
   once the heartbeat is stale, without any process having released the lease.
7. Shutdown at each boundary — before reservation, during a request, after a response and before
   apply, and concurrently with a capture at release time — never commits an effect twice and never
   leaves work that no later spawn can reach. Asserted for every exit reason, with a non-empty queue
   in at least one of them: the lease row's `owner_token` is NULL once the process is gone, so a
   `kept` answer can never survive a shutdown.
8. Heartbeat under load: a long synchronous maintenance stretch and a delayed apply do not let the
   lease go stale, and the apply cannot write an older heartbeat over a newer one.
9. Crash: `SIGKILL` mid-batch. Takeover is possible more than 6,000 ms after the last heartbeat;
   the running batch is reclaimable by another owner 120,000 ms after its claim. Those two latencies
   are asserted separately, and the reclaimed-batch count is reported separately from the spool
   recovery count.
10. Clock changes: a forward jump, a backward jump and suspend/resume leave epoch budgets and control
   ordering correct.
11. `resident = false` reproduces today's one-shot receipts, including the trigger and budget
    conditions under which capture does not spawn at all, and the existing `observe` suites pass
    unchanged on both supported Node versions.
12. Idle cost and RSS over a long run meet the targets, measured with many distinct sessions,
    repositories and retries rather than an empty process, and with concurrent captures and a
    held reader to show the WAL recycles. The per-poll work is counted, not assumed: one queue
    probe, not two, and no full-table scan.
13. A signal during an epoch and a signal during shutdown: the first ends the wait, releases the
    lease and exits 0 as `signal`; the second does not kill the process before the lease is
    released.
14. Retention is not starved by a held lease: with expired material present and nothing batchable,
    the maintenance interval opens an epoch and the purge runs. Asserted with a `secret` expired
    row, because that is the case where starvation is a privacy regression.
15. A `config.toml` that is already malformed when the resident starts ends the process with
    `config_changed`, not `worker_error`.
16. `batch_error` ends the process and `max_run` ends only the epoch: a source that fails every
    attempt produces one attempt per run rather than a loop, and an epoch that exhausts its budget
    is followed by another epoch with a fresh budget.
