# Resident worker and an always-reachable viewer

The design of this unit, written before its code (2026-10-03) and kept up to date as the slices
land. Rules are numbered R1 to R13; the code cites them.

Owner decision 36 (2026-10-03): 「常駐を標準にしたい」 and, for the switch, 「画面もいつでも開ける
(Recommended)」: the worker stays up, and the browser page opens from a bookmark. The bot adapter and
the local HTTP API for other clients stay after the switch (decision 33). Spec text: 1.8, 6.6, A111,
A112 (PR #342). Owner wish 7: 「軽さ: 上限数値は不要。メモリ食い潰しバグが無いこと。」

Scope of this unit: Linux and WSL (the first switch is WSL only). Windows native and macOS keep
today's behaviour until their own unit.

The design was reviewed before any code was written (four lenses: security, lifecycle, the
owner's use, and what could be left out); what the review changed is listed at the end.

## Built so far

- Slice 1 (R2, R3, R10, R12): the worker stays, steps aside and exits on its three conditions.
  `[worker] resident` is read, but nothing writes it yet (`oboete setup` does in slice 5), so a
  home is resident only where its owner wrote the key by hand.

## Before this unit (main 7f8de51)

- A hook starts `oboete worker` when no process holds `state/worker.lock` (`start_worker`,
  src/hook.rs:1813). The worker drains, runs its phases, and exits after `idle_ms` (60 s) with no
  new record, backing up first (src/worker.rs `serve`). While it waits it wakes every 200 ms.
- `oboete view` is a foreground command: it binds 127.0.0.1 on a free port, makes a token for this
  run, prints `http://127.0.0.1:<port>/#t=<token>`, and serves until Ctrl-C (src/view.rs `run`).
  Its default scope is the checkout it was started in (`Viewer.cwd`).
- The worker's stdio is closed (`spawn_detached`); its outcome goes to `state/worker-outcome`, and
  a run that took the lock and never recorded an outcome is what doctor reports as killed.
- `oboete rebuild`, `oboete restore` and `oboete recurate --yes` need the worker lock and say "a
  worker is running; try again when it has exited" when it is held.

## Decisions

R1. **Two resident processes, each as small as it is today.** The worker stays as it is and opens
no network port (spec 1.8, 6.6, A112). The viewer becomes a second resident process: the same
`view.rs` server, detached, one per home through its own lock (`state/view.lock`). A worker that
stops on an error leaves the page up. Every detached start sets the child's working directory to
the home, so no process keeps a checkout alive or dies with one. The environment is inherited as
today.

R2. **Resident is a setting that setup turns on.** `[worker] resident = true` in config.toml.
`oboete setup` and the settings page's first-run wizard write it as true: that is "resident by
default" for a user, and the settings page shows the switch (A111). A home whose config does not
say so (an evaluation, replay or test home) keeps today's worker that exits when idle and today's
foreground viewer, so no harness leaves a process behind.
`--idle-ms` becomes optional. Absent: the worker follows `[worker] resident`, with a 60 s idle
time. Present: it exits when idle and starts no viewer, whatever the config says. Only
`worker::run` decides this and passes it down. `run_once`, `rebuild`, restore, recurate, correct and
pref never read the setting: they exit at idle as today.

R3. **Idle in resident mode.** When the idle time passes, the worker does what it does today before
an exit (backup, prune), but only when a round ran since the last idle step, and then keeps the lock
and waits again: an idle time starts no round. No hook starts a resident worker again, so it is up
for every wait of a phase, not only D10's short ones, and runs the phase again at the time the
phase named; the idle step is taken within such a wait too. At every idle time it re-reads
config.toml and checks its home:
- only a config.toml that loads and does not say `resident = true` ends it, and not while an
  embedding call is out: the call's answer is written first (R10). A file that does not load (one
  the owner is editing) leaves the mode as it was. A worker that starts while the file does not
  load is not resident, as today, and doctor says so;
- a config.toml that changed since the worker last looked (its time or its size) starts a round,
  so a setting the owner changed is followed without a new record;
- "the home is gone" means the path `state/worker.lock` is missing or no longer names the file this
  run locked first (its device and inode, kept from that first lock, against the path: so the
  answer holds after the lock is released too). The worker checks it before every write it makes
  by path (the idle backup, the 30-minute backup, prune, the outcome it records after the release,
  the lock it takes again), at the start of each round, between two batches of a round (a consumer
  opens the home's files by path) and before it opens the stores again, and stops with an error
  without them. A home deleted and made again never gets the old home's backup. Its stores' `-wal` and
  `-shm` files are safe too: SQLite removes them by name when a store's last connection closes,
  but not once the store's file is no longer at its path. The commands that borrow the worker's
  loop (`rebuild`, `restore`, `recurate`) fail with the same error.

R4. **Who starts the viewer.** The resident worker, at its start and then at most once a minute on
a deadline of its own (checked where the backup deadline is checked: between batches and on wait
ticks, not only at idle), starts the viewer when `state/view.lock` is free. It makes the filesystem
check of R6 itself before it starts anything, keeps the child it started and reaps it before it
starts another. After an outcome of "port in use" it tries again only every 10 minutes.
`oboete view` does the same and always tries at once. Hooks do not: their cost stays as it is.
The viewer has a once-a-minute tick of its own. On it, it leaves when config.toml loads and does
not say `resident = true` (and no request arrived since the last tick), when its home is gone
(`state/view.lock` by identity, as R3), when `[view] port` differs from the port it bound, or for a
new binary (R9). It leaves only while no connection is live and no save is running.

R5. **Fixed address.** `[view] port` (default 17373, to be checked against what listens on the
owner's WSL and Windows), 127.0.0.1 only. No scan of other ports: a bookmark needs one address.
The viewer takes `state/view.lock` first (one that does not get it exits and writes nothing), then
replaces `state/view-outcome` with `starting`, binds, and replaces it with `listening <port>`; a
failed start writes its reason there instead ("port in use" among them). A second home on the same
machine sets its own port: the switch runbook writes `[view] port = 17374` into the dogfood home
before `resident = true` is written there. Setup and doctor print the address to bookmark; for a
"port in use" outcome they say that another program or another oboete home holds port N.

R6. **A token that outlives a restart.** `state/view-token`: 32 lowercase hex characters from the
OS generator, mode 0600. One function reads it, and returns it only when the mode check passes (not
readable by group or others) and the content is exactly 32 lowercase hex characters; anything else
refuses every request. `token_ok` is false for an empty or absent header. The viewer reads the file
for each request (no cache to go stale). It writes the file at its start, under `state/view.lock`,
when it is missing or not of that shape, staged, synced and renamed.
It is kept only where the filesystem enforces a Unix mode (the check `keyfile` uses); elsewhere the
resident viewer does not start and `oboete view` serves in the foreground as today.
`oboete view --new-token` replaces the file the same way and, in a resident home, also writes the
next free port into `[view] port`, so the viewer comes back on a new address; it prints that address
and one line telling the owner to delete the old bookmark. The new address is what ends a page a
squatter left in the browser under the old one. The token is never replaced automatically.
What the token may do is an owner decision (below). The local HTTP API and the bot adapter get
their own credential and never reuse `state/view-token`.

R7. **`oboete view` in a resident home.** It makes sure the viewer runs (R4), and starts the worker
too when `state/worker.lock` is free, as a hook does, so one command brings up both. It sends
nothing to the port: ready means `state/view.lock` is held and `state/view-outcome` says
`listening` on the configured port, waited for up to 3 s. Then it prints the address (`--open` opens
it through the owner-only opener page, as today) and returns. Otherwise it says why and serves in
the foreground on a free port with a token for this run, as today, never opening the fixed address.
`--port N` asks for the foreground viewer. The resident viewer removes `view-open-<its port>.html`
by name when a request with the token arrives, so spec 6.6's "the first request with the token
removes the page" stays true.

R8. **No checkout of its own.** The resident viewer holds no checkout: its search has no caller
checkout, its timeline covers every repository, `/api/repos` returns an empty `current` and
`branch`, and `/api/context` with no repository chosen returns empty text with a "choose a
repository" marker. The foreground viewer keeps the checkout it was started in.

R9. **A new binary replaces both, in place.** Each process notes its executable's path once at its
start and uses that path for every detached start. "Replaced" means the path's device and inode
differ from those of `/proc/self/exe`. The process then replaces itself with `exec` of the noted
path with the same arguments, so a start that fails (file half written, not executable, missing)
leaves the old process serving with its lock and port. The worker looks at each round and each wait
tick, but not while an embedding call is out. The viewer looks only on its once-a-minute tick (R4's
conditions). A path that is gone ends a process only when it is still gone at the next minute
check. Hooks already run the new binary; this keeps the time in which an old process and new hooks
share the stores short.

R10. **A kill while it waits is not an alarm.** A resident worker is killed at every shutdown of
the PC or of WSL (this PC: about every two to five days, by the journal's boot list). The worker
records its outcome (empty: all well) each time it finishes a round and starts waiting, and puts
the "stopped before it finished" note back when it starts a round. An embedding call that is out is
work in flight: the outcome stays "stopped before it finished" until its answer is written. So
doctor reports a kill during work, as today, and says nothing about a kill while waiting.

R11. **What is measured before the PR is ready** (spec 1.8), on WSL with the owner's hardware.
- The same resident worker after each of 5 cycles of 1,000 new records and one idle time, and the
  same viewer after 100 and after 1,000 loads of a named page set (index, search, timeline, stats, a
  settings read and one save): `VmRSS`, `VmHWM`, `Threads`, the count of `/proc/<pid>/fd`, and the
  sizes of the three `-wal` files. It passes when threads and open files in the last cycle equal
  cycle 2 and `VmRSS` is within 10% or 2 MB of cycle 2. All readings go in the PR note and to the
  owner.
- CPU time used in 10 idle minutes (the 200 ms wake-up, which stays unless this number says
  otherwise), and `oboete correct` latency against a resident worker.
- With the owner's hand, at the rehearsal: whether both processes survive closing the last WSL
  terminal for a minute, and whether a `wsl.exe -- oboete view --open` shortcut that has returned
  leaves them running. The result decides only the runbook's shortcut (below), not the code.
- The 24-hour run on a copy of the owner's real home is a step of the cut-over runbook (no migrated
  home exists yet, and a copied config has live providers).

R12. **Stepping aside for a command.** `oboete restore`, `oboete rebuild` and
`oboete recurate --yes` that find the worker lock held write `state/worker-yield` and wait up to
30 s for the lock, removing the file when they hold it or give up (the message then says the worker
is busy, not "try again when it has exited"). Two commands may wait at once: one that still waits
writes the file again when the other took it away. The worker looks for the file at its start,
between rounds and on the 200 ms tick beside the restore request: it backs up (a restore reads the
backups), closes its stores, releases the lock, records a clean outcome and exits. It does not
step aside while an embedding call is out: the call is paid for and counted, so its answer is
written first, and a command whose 30 s pass meanwhile says the worker is busy. Once a command
has asked, the worker sends no other batch or query, so a backlog cannot keep the command waiting
for longer than the call that was out. The lock goes
only after the stores are closed: the command that takes it may swap them at once. Only `oboete
worker` steps aside, resident or not; a command that borrows the worker's loop runs to its end.
The worker ignores a file older than a minute, and one dated after now (a clock that went back):
a worker that works on is the smaller harm than one that never works. The next hook or `oboete
view` starts it again.

R13. **What doctor and the page say.** For a resident home doctor prints, from the config and the
two locks: "worker: resident, running" or "not running now (the next hook starts it)", and "page:
http://127.0.0.1:<port> is up" or "not running: <view-outcome>; run `oboete view`". Neither "not
running" line is a failure. `oboete rebuild` holds a lock of its own (`state/rebuild.lock`) while it
runs and the stats route reports `rebuilding` from that lock, so a resident worker cannot make the
page say "rebuilding" for good.

## Owner decision: what the bookmark's token may do

Asked on 2026-10-03 and answered the same day: the bookmark does everything the page does, saves
of settings and keys included (spec 6.6, spec-webui.md). In a resident home the token is permanent,
so whoever learns it can also change settings and replace API keys from this machine until
`oboete view --new-token` is run. On the owner's PC a program able to read the bookmark can already
read the files themselves (Limits, below). A second value made at each start for saves of
credentials, endpoints, agent setup and updates, handed over only by `oboete view --open`, is
left as a setting to add before a public release.

## Not in this unit

- Starting at boot before any hook: no OS service is installed (spec 1.8). On WSL the plain answer
  is a Windows shortcut; the runbook names it. If the rehearsal shows that WSL stops the
  distribution once the shortcut's command has returned, the shortcut holds its session open in a
  minimised window (`wsl.exe -- sh -c 'oboete view --open; exec sleep infinity'`): no new flag.
- The local HTTP API for other clients and the bot adapter (after the switch, decision 33).
- A local model kept in the worker: when the local embedder is built it is loaded on demand and
  dropped after an idle time, so idle memory stays the worker's own.
- Windows native and macOS.
- A stop command, a PID file, a signal handler, restart logic in a yielding command (R12 and R3
  cover stopping: turn `resident` off, or run the command that needs the lock).
- Later, small: the page stops its 3 s polling after the first failed or refused poll and sends the
  token again only on the owner's click; every store opened with `PRAGMA journal_size_limit` (about
  8 MB) if R11 shows a `-wal` file that stays large.

## Limits to state in the docs (the owner's words are in the review result, `verdict.limits`)

- WSL does not separate Linux users: a program under the dogfood user can reach the owner's files
  through Windows, token or not. Untrusted programs belong in another distribution, not another
  user.
- The bookmark and the browser history hold the token; a browser that syncs bookmarks stores it at
  the sync service too.
- While the viewer is down (after a boot, a crash, the moment of an update), another program
  listening on the port receives the click of the bookmark or the poll of an open tab, and its page
  can read the token from the address. `oboete view` itself sends the token nowhere.
- A suspected squat is ended by `oboete view --new-token`, which changes the token and the address;
  the old bookmark is deleted by hand.
- Under choice (a), whoever knows the token can change settings and replace keys from this machine
  until `--new-token`; what they changed is not put back by it.
- The page opens only while WSL runs; right after a boot it opens once the first hook, `oboete view`
  or the shortcut has started it.
- A config.toml broken by hand at a restart: not resident, and the bookmark does not open, until it
  is fixed; doctor says why.
- Two homes use two ports; the runbook sets the dogfood one.
- Resident processes keep the environment of their first start (a proxy setting among it): turn
  `resident` off and on, or restart WSL, after changing it.
- An update at the moment an embedding call is out sends that one call again. The page follows an
  update within a minute.
- "The home is gone" (R3) is a check before each write by path, not a lock on the directory. A
  home removed and made again in the moment between a check and its write can get that one write:
  a backup segment, an outcome note, or the ops of a redaction rescan, which opens raw.db by its
  path. Closing the gap means writing through a handle on the directory the worker started in
  (start in the home, relative paths: Linux refuses a new file in a removed directory); it is a
  candidate for the slice that starts the processes in the home (R8), with a test that replaces
  the home inside the rescan.

## Tests (through `oboete worker`, `oboete view`, and HTTP requests to the listener)

1. With `resident = true` the worker is still running after its idle time, holds the lock, and has
   backed up once (not again at the next idle time with no new record). With the key turned off it
   exits at the next idle time. A phase that waits is run again at its time, not at each idle time. With a config.toml that does not load, both processes still run
   after the idle time and the viewer still answers. With the home removed and made again within
   the idle time, the old worker exits and leaves no `backups` and no outcome in the new home.
2. A resident worker starts the viewer; a second start finds the lock held and starts none.
   `oboete view` in a resident home with nothing running leaves both locks held.
3. The viewer answers on the configured port with the token of `state/view-token`; after a restart
   the same token works; `--new-token` makes the old one fail and the new one work, on the next
   free port, and config.toml names that port. A request with the token removes the opener page.
4. The token file is 0600 when made. A file readable by group or others, a missing file, an empty
   file and a 31-character file each refuse every request, with and without a token header.
5. A port in use: the viewer exits, `state/view-outcome` says the port is in use, the worker keeps
   draining records and starts no second viewer within the next minute and leaves no zombie, and
   `oboete view` serves in the foreground and prints why. A foreign listener on the port receives no
   connection and no `X-Oboete-Token` from `oboete view`.
6. A home on a filesystem without modes (the `FS` test seam of keyfile): no resident viewer.
7. Host check with the fixed port: `127.0.0.1:<port>` and `localhost:<port>` pass, anything else 403.
8. The resident viewer's default scope is every repository; `/api/context` with no repository
   answers the marker; a worker started from a directory that is then removed still starts a viewer
   that answers.
9. A replaced binary (by rename): a worker and a viewer started from a copy of the test binary are
   each replaced by a process that holds the lock. An unexecutable or briefly removed copy ends
   neither. A request in flight is answered.
10. A resident worker killed while it waits leaves doctor without the "stopped" line; one killed
    during a round leaves it.
11. A home without the key behaves as today (the existing worker and viewer tests unchanged). With
    `resident = true` and no worker running, `oboete correct` and `oboete rebuild` return, and an
    explicit `--idle-ms 60000` exits when idle.
12. `oboete rebuild` and `oboete restore` succeed against a resident worker (R12), and a
    `worker-yield` file older than a minute is ignored.
13. A viewer killed while records keep arriving is back within the minute.
14. `[view] port` changed while the viewer runs: within its tick it leaves, and the next start
    listens on the new port.

## Build order (test first, one slice at a time; all of it by Claude: security scope)

1. R2 + R3 + R10 + R12: the worker stays, yields and exits on the three conditions (tests 1 part,
   10, 11, 12). No viewer yet.
2. R5 + R6 + R8: the detached viewer process with its lock, outcome, token file and no checkout
   (tests 3, 4, 5 part, 6, 7, 8).
3. R4 + R7: who starts it, `oboete view` in a resident home, `--new-token` (tests 2, 5, 13, 14).
4. R9: the binary change (test 9).
5. R13, setup writes the key, the settings switch (A111), then R11's measurements and the PR note.

## What the review changed

- Readiness is read from the home (lock and outcome), not asked of the port: no token goes to a
  listener that may not be ours (R5, R7).
- The token file's shape is checked on every read, and the viewer makes it under its lock (R6).
- `--new-token` also moves the port (R6).
- A command that needs the worker lock asks the resident worker to step aside (R12, new).
- `--idle-ms` is optional and only `worker::run` reads the resident setting (R2).
- "The home is gone" is the lock file's identity, checked before every write by path (R3, R4).
- A config.toml that does not load no longer ends a resident process (R3, R4).
- The worker starts the viewer on a one-minute deadline, reaps its child, and backs off to ten
  minutes on "port in use" (R4); the viewer leaves on a changed port (R4, R5).
- The binary change is an `exec` in place, judged by inode against `/proc/self/exe` (R9).
- Detached processes start in the home; the resident viewer holds no checkout (R1, R8).
- The memory check runs in cycles and can fail (R11); the WSL survival check moved to the
  rehearsal with the owner, since it decides only the shortcut.
- Doctor and stats lines (R13). The bookmark's rights were put to the owner.

## Found while building

- config.toml's `[worker]` table has to be named for the capture settings' parser
  (`config::CaptureConfig`), which refuses a table it does not know: without that, a home with
  `resident = true` recorded nothing. The end-to-end test (`tests/resident.rs`) found it; `[view]`
  needs the same in slice 2.
- A command that takes the lock from a worker stepping aside found raw.db still open: the lock is
  now released after the loop's stores are closed (R12).

## What the review of slice 1 changed

Codex's adversarial review of the first build (PR #359), each with a test that failed first:

- The home was checked only where a round starts, and a long drain opens the home's files by path
  at every batch: it is checked between batches too (R3).
- The check compared the held lock's file with the path, so it said nothing once the lock was
  released, and the exit wrote its outcome and took the lock again in a home made meanwhile: the
  identity is kept from the run's first lock and asked after the release too (R3).
- A worker stepped aside, and called itself waiting, with an embedding call out: neither happens
  until the answer is written (R10, R12).
- A command that gave up removed the request of another command still waiting: the one that waits
  writes it again (R12).

CodeRabbit on the same PR: a resident worker whose config.toml stopped saying `resident = true`
left at its next idle time with an embedding call out; it now leaves once the call is settled (R3).

Codex's second round, on the fixes:

- A worker that waited for a call to settle sent the next batch in the same step, and with a
  backlog of batches and of prompt queries some call was always out: a command could wait for as
  long as the backlog lasted. Once asked, the worker now sends nothing new (R12).
- The check before a write by path leaves the moment between the two. Not closed in this slice: it
  is written under Limits with the way to close it.
