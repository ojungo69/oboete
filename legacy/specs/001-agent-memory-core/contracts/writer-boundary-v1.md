# Sole-writer boundary contract v1

## Status

- **Contract version:** v1
- **Frozen from:** `vendor/codemem` Phase 1 (TypeScript daemon implementation)
- **Authority:** this document is authoritative for Rust reimplementation parity. Where this document
  and the vendored TypeScript disagree after a future vendor update, this document still governs unless
  it is explicitly re-frozen as a new version.
- **Scope:** the sole-writer invariant — audited SQLite entry points, the inter-process writer lock,
  daemon identity and force-kill, clean shutdown vs. force-kill fallback, maintenance mode, backup
  preconditions, migration ordering, legacy cutover fencing, and the static-scan rules that enforce all
  of the above at build time.
- All evidence paths are relative to `vendor/codemem/` unless given as a full repo-relative path
  (`harness/…`, `evidence/…`).

---

## 1. The invariant

Exactly one process — the daemon — may hold a write-capable SQLite connection to the canonical
database at a time. Every other component (CLI, MCP server, viewer, hooks) reaches the database only
through the daemon's Unix-socket RPC or a durable on-disk spool; none of them open a SQLite handle to
the canonical file. This is enforced by two independent, redundant mechanisms:

1. **Runtime**: an OS-level file lock on `control/lock.db`, held for the daemon's entire lifetime
   (§4).
2. **Build-time**: a TypeScript AST static scan over all production sources that enumerates every
   permitted SQLite opener and fails the build if a new one appears outside the allow-list (§8).

A reimplementation must reproduce both layers: the runtime lock is what makes concurrent daemons
mutually exclusive; the static scan is what keeps the surface from regressing, and its allow-lists are
themselves part of the frozen contract (a Rust rewrite collapses many of these TS-specific openers into
fewer symbols, but the *set of processes/entry points allowed to touch the DB file* must not grow).

---

## 2. Audited SQLite entry points

These are the only five classes of opener the Phase 1 static scan (`harness/phase1-static-scan.ts`)
permits anywhere in `vendor/codemem/packages/**` production source (test files are excluded — see §8).
Each row is one opener *class*; the "files" column is the exact, closed set of production files allowed
to invoke it.

| opener | permitted production files | role |
|---|---|---|
| `connect()` / `connectReadOnly()` (`db.ts:150`, `db.ts:198`) | `daemon-canonical.ts`, `daemon-jobs.ts` | Opens the canonical DB with standard pragmas (writer) or a private snapshot copy read-only for a comparison job (`connectReadOnly` is called in production from `getDaemonMemoryRoleReport`, `daemon-jobs.ts:475`, reached via the live `"report.role-compare"` job kind — registered at `daemon-jobs.ts:77`, arg schema at `daemon-jobs.ts:112`, dispatched at `daemon-jobs.ts:931-943`) |
| `new MemoryStore(...)` (`store.ts:188`) | `daemon-canonical.ts`, `daemon-jobs.ts` | Wraps an *already-open* `WriterActor`; the constructor takes a connection, never a path — it cannot self-open |
| `WriterActor.open` / `ReadOnlyActor.open` (`writer-actor.ts:269`, `writer-actor.ts:288`) | `db.ts`, `legacy-cutover.ts`, `online-backup.ts`, `storage.ts` | Audited-wrapper primitive used by cutover, backup verification, and storage-artifact integrity checks |
| `new BetterSqlite3(...)` (raw driver constructor) | `daemon-lifecycle.ts`, `writer-actor.ts` | `daemon-lifecycle.ts` uses it only for the instance lock (`control/lock.db`, §4); `writer-actor.ts` uses it only inside `WriterActor.open`/`ReadOnlyActor.open` |
| test-only opener (`openTestMemoryStore`) | `test-utils.ts` (test files only) | `test-utils.ts:23-38` calls `connect(dbPath)` — **not** raw `better-sqlite3** — then runs `runDatabaseMigrations` before constructing `MemoryStore`; excluded from the production scan by exact path (§8), not by any code-level distinction |

Evidence for the exact allow-lists: `harness/phase1-static-scan.ts:30-53` (`expectedHits`), cross-checked
against a live re-scan recorded in `evidence/phase1-disposition.md:184-196` (§11 "live tree
再照合") and the closure record at `evidence/phase1-disposition.md:260-270` (§T048).

### 2.1 `AuditedSqliteConnection` / `WriterActor` / `ReadOnlyActor`

`writer-actor.ts:63-186` wraps the raw `better-sqlite3` `Database` object behind an ECMAScript private
field (`#raw`) so callers holding a `WriterActor`/`ReadOnlyActor` cannot reach the underlying handle
through the public surface (`writer-actor.ts:64`). `WriterActor.open(dbPath)` (`writer-actor.ts:269-279`)
and `ReadOnlyActor.open(dbPath, {readonly:true, fileMustExist:true})` (`writer-actor.ts:288-297`) are the
only constructors; both are `private constructor`, so no code outside the class can bypass `.open()`.

Both openers call `recordOpen(dbPath, mode, owner)` (`writer-actor.ts:38-54`), which — only when the
environment variable `CODEMEM_DB_OPEN_TRACE` is set — appends a JSON line (`{version, event:
"sqlite_open", mode, owner, pid, dbPath, openedAt}`) to that trace file, `mode: 0o600`. This is a
test/audit instrument, not a runtime behavior other components depend on (§10 — free to differ).

### 2.2 `store.ts` `MemoryStore` constructor

`store.ts:188` — `constructor(connection: WriterActor, options: {closeConnection?: boolean} = {})`.
`MemoryStore` never opens a path itself; it is always handed an already-open `WriterActor`. This is what
makes `new MemoryStore` count as a distinct, closed opener class in the static scan (`memory_store` rule)
rather than a bypass of the `WriterActor`/`connect()` allow-lists.

---

## 3. Storage layout paths

`storage-layout.ts:55-73` (`resolveStorageLayout(dataDir)`) derives every control-plane path from one
`dataDir` root:

| field | path (relative to `dataDir`) | evidence |
|---|---|---|
| `controlDir` | `control` | `storage-layout.ts:57` |
| `dbDir` | `db` | `storage-layout.ts:58` |
| `versionsDir` | `db/versions` | `storage-layout.ts:63` |
| `currentPointerPath` | `db/current` (symlink) | `storage-layout.ts:64` |
| `journalPath` | `control/restore-journal.json` | `storage-layout.ts:65` |
| `installManifestPath` | `control/install-manifest.json` | `storage-layout.ts:66` |
| `lockPath` | `control/lock.db` | `storage-layout.ts:67` |
| `identityPath` | `control/identity.json` | `storage-layout.ts:68` |
| `socketPath` | `control/daemon.sock` | `storage-layout.ts:69` |
| `spoolDir` | `control/spool` | `storage-layout.ts:70` |
| `backupsDir` | `control/backups` | `storage-layout.ts:71` |

`ensureStorageLayout(layout)` (`storage.ts:41-48`) creates `dataDir`, `controlDir`, `dbDir`,
`versionsDir`, `spoolDir`, `backupsDir` as private directories (`ensurePrivateDirectory`,
`storage-platform.ts:39-61`: mode `0o700`, must not be a symlink, must not sit on a network/FUSE
filesystem — `NETWORK_FS_TYPES`/`FORBIDDEN_MOUNT_FSTYPES`, `storage-platform.ts:132-172`).

`assertSupportedStoragePlatform()` (`storage-platform.ts:23-27`) refuses any `process.platform !==
"linux"` (WSL counts as Linux). `assertDataDirPreflight(dataDir)` (`storage-platform.ts:225-238`)
additionally rejects a `dataDir` under a WSL Windows share (`/mnt/<letter>/...`,
`isWslWindowsSharePath`, `storage-platform.ts:174-177`) and any ancestor mounted on a forbidden
filesystem type, using `/proc/self/mountinfo` (`mountFstypeFor`, `storage-platform.ts:195-223`).

---

## 4. Inter-process writer lock (`control/lock.db`)

`acquireWriterLock(lockPath)` (`daemon-lifecycle.ts:164-181`):

```ts
const lock = new BetterSqlite3(lockPath, { timeout: 0, fileMustExist: false });
chmodSync(lockPath, 0o600);
lock.pragma("journal_mode = DELETE");
lock.pragma("busy_timeout = 0");
lock.exec("BEGIN IMMEDIATE");
```

- Opening `better-sqlite3` with `timeout: 0` disables the driver's built-in busy-wait/retry.
- `journal_mode = DELETE` (not WAL) is required: `BEGIN IMMEDIATE` under the rollback-journal mode
  takes SQLite's `RESERVED`→`EXCLUSIVE` file lock path (a real `fcntl`/`flock`-backed OS lock on the
  lock file), which is what makes a second process's `BEGIN IMMEDIATE` fail immediately rather than
  queue.
- `busy_timeout = 0` means SQLite itself never retries either; failure is immediate.
- The transaction opened by `BEGIN IMMEDIATE` is **never committed or rolled back** while the daemon is
  alive — it is held open for the daemon's entire process lifetime. The open `lock` handle (and its
  live, uncommitted `BEGIN IMMEDIATE`) *is* the mutual-exclusion mechanism, not a row or table inside
  the lock database.
- On failure, the lock handle is always closed (`lock.close()`) before the error propagates
  (`daemon-lifecycle.ts:174`). If the driver's error message matches `/busy|locked|SQLITE_BUSY/i`, it is
  translated to `"Daemon already running for this data_dir (writer lock busy)."`
  (`daemon-lifecycle.ts:176-178`); any other error is rethrown unchanged.
- `recordLockOpen(lockPath)` (`daemon-lifecycle.ts:77-93`) writes the same optional
  `CODEMEM_DB_OPEN_TRACE` JSON-line trace as §2.1, tagged `mode: "lock"`, `owner: "daemon_lifecycle"` —
  test/audit instrumentation only.

`acquireWriterLock` is called from three places, and the resulting handle's lifetime differs by caller:

1. **`startDaemon`** — acquires the capability lifecycle lease, then the setup/spool owner lock, then
   this writer lock. All three are held while it recovers a setup journal. The setup/spool lock is then
   released so hook capture is not blocked while the still-held lifecycle and writer locks protect
   manifest resolution, TLS preflight, canonical-writer open, and the remaining startup work. The
   lifecycle lease is released only after socket, identity, and live state are published; the writer
   lock remains held for the daemon lifetime. A competing start that cannot take the writer lock
   closes the two earlier locks without touching restore journals, socket, or identity state.
2. **`cleanupIfStillOwner`** (`daemon-lifecycle.ts:472-488`) — acquired transiently to *prove* no daemon
   currently holds the lock before removing stale control artifacts (socket, identity file); always
   closed in a `finally` (`daemon-lifecycle.ts:486`). If acquisition fails with an "already running"
   message, cleanup is silently skipped (a live daemon still owns the artifacts) — any other acquisition
   error propagates.
3. **Legacy cutover** (`legacy-cutover.ts:307`, `WriterActor.open(input.legacyPath)`) opens a *different*
   file (the legacy DB itself, not `lock.db`) under an EXCLUSIVE SQL transaction for the duration of the
   cutover — see §9. This is a separate mechanism from the instance lock, scoped to one legacy file.

**Failure mode is exactly-once, immediate, no retry.** A second `startDaemon` call for the same
`dataDir` while a daemon is alive throws synchronously; there is no polling/backoff built into
`acquireWriterLock` itself.

---

## 5. Daemon identity and force-kill rules

### 5.1 Identity shape and derivation

```ts
type DaemonIdentity = { version: 1; pid: number; startTime: string; fingerprint: string; nonce: string };
```
(`daemon-lifecycle.ts:38-44`)

- `pid` — `process.pid` at daemon start.
- `startTime` / `fingerprint` come from `readProcessIdentity(pid)` (`storage-platform.ts:240-265`):
  - `startTime = "${bootId}:${starttimeField}"`, where `starttimeField` is field 22 (0-indexed offset
    19 after the closing `)` of the comm field) of `/proc/<pid>/stat`, and `bootId` is
    `/proc/sys/kernel/random/boot_id` trimmed (best-effort — if unreadable, `bootId` is the empty
    string and only `starttimeField` distinguishes PID reuse within one boot).
  - `fingerprint = sha256(exePath + "\0" + cmdlineBytes)` hex, where `exePath` is
    `realpathSync(readlinkSync("/proc/<pid>/exe"))` with a trailing `" (deleted)"` suffix stripped
    first, falling back to the unresolved link target if `realpathSync` throws; `cmdlineBytes` is the
    raw contents of `/proc/<pid>/cmdline`.
- `nonce` — `randomUUID()`, freshly generated every daemon start (`daemon-lifecycle.ts:292`); used both
  as the clean-stop authorization token (§6) and for identity comparisons.
- `identity.json` is written via `durableReplaceFile` (write-temp, `flag:"wx"`, `mode:0o600`, rename,
  fsync parent — `storage-platform.ts:72-93`) only *after* the RPC socket is already bound
  (`daemon-lifecycle.ts:359-360`), so a reader never observes an identity file whose socket isn't live
  yet.

### 5.2 Liveness and identity-match predicates

- `processAlive(pid)` (`daemon-lifecycle.ts:126-135`) reads `/proc/<pid>/stat`, extracts the state
  character, and returns `false` for state `Z` (zombie) or `X` (dead), `false` on any read error
  (ENOENT etc.), `true` otherwise.
- `identitiesMatch(file, live)` (`daemon-lifecycle.ts:110-115`) compares only `startTime` and
  `fingerprint` (not `pid`/`nonce`) between a recorded identity and a freshly re-derived
  `readProcessIdentity` result.
- `sameIdentity(left, right)` (`daemon-lifecycle.ts:117-124`) requires **all four** fields
  (`pid`, `startTime`, `fingerprint`, `nonce`) to match — used to confirm two identity snapshots
  describe the exact same daemon incarnation.
- `isDaemonProcessAlive(layout)` (`daemon-lifecycle.ts:148-162`) = identity file readable AND
  `processAlive(identity.pid)` AND `identitiesMatch(identity, readProcessIdentity(identity.pid))`. Any
  read/parse failure is treated as "not alive" (fail-open toward allowing recovery, not fail-closed
  toward blocking it).

### 5.3 Force-kill (`forceKillDaemon`, `daemon-lifecycle.ts:510-560`)

A strict, double-checked sequence — every step that fails throws `"Force-kill refused: ..."` and
performs no kill:

1. Read `identity.json` ("first" snapshot). Missing file → refuse (no record to act on).
2. If a caller-supplied `expected` identity was given and does not `sameIdentity()`-match "first" →
   refuse.
3. Re-derive live process identity for `first.pid` (`readProcessIdentity`); a read failure (process
   gone) → refuse. If it doesn't `identitiesMatch()` "first" → refuse (PID was reused by an unrelated
   process).
4. Re-read `identity.json` a **second** time ("second" snapshot) and require `sameIdentity(first,
   second)` — guards against the identity file changing between steps 1–3 (the daemon restarted, or a
   competing force-kill already replaced it).
5. Re-derive live process identity for `second.pid` and require `identitiesMatch(second, liveSecond)` —
   same PID-reuse guard as step 3, applied a second time.
6. If `second.pid === process.pid` (the caller *is* the daemon process) → refuse with
   `"...use stopDaemon."` instead of self-signaling.
7. `process.kill(second.pid, "SIGKILL")`.
8. `waitUntil(!processAlive(second.pid), 1000ms)` (20ms poll) — if the process is still alive after 1s
   → throw `"Force-kill did not terminate the identified process."` (no retry of the kill itself).
9. If an in-process `liveDaemons` entry for this `dataDir` still matches `second`'s identity, remove it
   and close its lock handle if open (covers the case where the daemon being force-killed is *this same
   Node process* acting on its own stale bookkeeping — not the same as step 6, which forbids killing the
   calling process's live PID).
10. `cleanupIfStillOwner(layout, second)` (§4 item 2) — re-acquire the instance lock to prove no daemon
    is running, and only if the current on-disk identity still matches `second` (else assume a different
    daemon has since started and leave its artifacts alone), remove the socket and identity file.

**Known limitation, deliberately left as-is** (`daemon-lifecycle.ts:543-545`, a `ponytail:` comment in
source): `process.kill(pid, "SIGKILL")` still addresses a bare PID number. Between the last identity
re-check (step 5) and the actual `kill(2)` syscall (step 7) there is a race window in which the OS could
have already reused `second.pid` for an unrelated process; Node has no `pidfd_send_signal` binding to
close it. This is called out explicitly in code as a known, accepted gap — see §12.

### 5.4 `readDaemonHealth` (`daemon-lifecycle.ts:490-508`)

Returns `{status: "not_running", dataDir}` if the socket file doesn't exist or
`!isDaemonProcessAlive(layout)`, or if the identity file is missing/unparseable. Otherwise returns
`{status: "ok", pid, socketPath, dataDir}` from the identity file's `pid`. This function does not probe
the socket itself — liveness is inferred entirely from `/proc` + identity-file cross-checks.

---

## 6. Clean shutdown vs. force-kill fallback (`stopDaemon`, `daemon-lifecycle.ts:562-582`)

```
snapshot = readIdentityFile(identityPath)                     // may be null
if this Node process holds the live daemon (liveDaemons has dataDir):
    stopLive(dataDir)  →  { action: "stopped" }                // in-process shutdown, no RPC round-trip
else:
    timeoutMs = options.timeoutMs ?? 2000
    if snapshot && socket exists:
        requestCleanStop(socketPath, snapshot.nonce)            // best-effort, ignores result
    if waitUntil(!isDaemonProcessAlive(layout), timeoutMs):      // poll every min(20ms, remaining)
        cleanupIfStillOwner(layout, snapshot)  →  { action: "stopped" }
    else:
        forceKillDaemon(dataDir, snapshot ?? undefined)  →  { action: "force_killed" }
```

- `requestCleanStop(socketPath, nonce)` (`daemon-lifecycle.ts:242-261`) connects, writes `"STOP
  <nonce>\n"`, sets a 500ms socket timeout, and resolves `true`/`false` on `close`/`error`/`timeout` —
  but **the boolean result is not consulted**; `stopDaemon` always proceeds to the liveness poll
  regardless. The actual authorization check happens server-side.
- Server-side handling (`daemon-rpc.ts:1651-1661`, inside `attachDaemonRpc`): a line starting with
  `"STOP"` extracts the nonce after the 4th character; if it is non-empty and **exactly equals**
  `ctx.identity.nonce` (the live daemon's own current nonce, captured at its own startup — not the
  caller-supplied one, and not compared via `sameIdentity`), the daemon responds `{status:"stopping"}`
  and calls `ctx.onStop()`; otherwise it responds `{status:"mismatch"}` and does **not** stop.
- `ctx.onStop` (`daemon-lifecycle.ts:320-327`) re-checks that the currently-registered `liveDaemons`
  entry for this `dataDir` still `sameIdentity()`-matches the `identity` captured when `startDaemon` set
  up the RPC context, then calls `stopLive(dataDir)`.
- **Net effect**: a `stopDaemon` call whose `snapshot.nonce` is stale (daemon restarted since the caller
  last read `identity.json`) is silently ignored by the running daemon; `stopDaemon` then waits out the
  full `timeoutMs` and falls back to `forceKillDaemon(dataDir, snapshot)` (`daemon-lifecycle.ts:580`),
  passing that same stale snapshot as `expected`. `forceKillDaemon` reads the identity file fresh and
  throws `"Force-kill refused: daemon identity mismatch."` on the very first check
  (`daemon-lifecycle.ts:514-516`), so the call **rejects and kills nothing** — it does not fall through
  to killing the current incarnation. A stale-nonce stop request therefore cannot terminate the wrong
  process at either step: the clean-stop request is ignored and the force-kill is refused. Recovering
  requires re-reading `identity.json` and issuing a fresh `stopDaemon`/`forceKillDaemon`. A
  reimplementation must reproduce the refusal, not "kill whatever is currently there".
- `stopLive(dataDir)` (`daemon-lifecycle.ts:235-240`) removes the `liveDaemons` map entry and calls
  `releaseResources(layout, live)` (`daemon-lifecycle.ts:202-233`), which in order: clears the spool-sweep
  and backup-sweep timers, awaits any in-flight `dailyBackupTask`, closes the RPC server, stops
  `DaemonJobService` and the optional `RawEventSweeper`, closes the `MemoryStore`, closes the `WriterActor` if still
  open, removes control artifacts (socket file, identity file — `removeControlArtifacts`,
  `daemon-lifecycle.ts:461-470`), and only *after* artifacts are gone, closes the writer lock. Each step
  is wrapped so a failure in one does not skip the rest (all catches are silent/best-effort, comments
  note the rationale per step).
- `timeoutMs` defaults to 2000ms and is caller-overridable; there is no configurable retry count for the
  clean-stop attempt itself — it is sent once, then superseded by the poll/force-kill fallback.

---

## 7. Maintenance mode

`DaemonJobService` (`daemon-jobs.ts:512-...`) runs all daemon jobs through a single serialized promise
chain (`this.queue = this.queue.then(() => this.run(jobId))`, `daemon-jobs.ts:649`), so job execution is
never concurrent within one daemon regardless of maintenance status.

- `MAINTENANCE_JOB_KINDS` (`daemon-jobs.ts:128-147`, 18 kinds: `db.vacuum`, `dedup-keys.backfill`,
  `memories.dedup`, `memories.prune`, `narrative.backfill`, `observations.prune`, `projects.normalize`,
  `projects.rename`, `raw-events.prune`, `raw-events.retry`, `refs.backfill`, `scopes.backfill`,
  `secrets.scan`, `session-context.backfill`, `structured.backfill`, `summary-dedup.backfill`,
  `tags.backfill`, `vectors.migrate`) enter maintenance mode when run for real (`row.dry_run === 0`);
  dry runs of the same kinds do **not** (`executeWithMaintenance`, `daemon-jobs.ts:747-752`).
- `BACKUP_REQUIRED_JOB_KINDS` (`daemon-jobs.ts:148-157`, a strict subset of 8: `db.vacuum`,
  `memories.dedup`, `memories.prune`, `observations.prune`, `projects.normalize`, `projects.rename`,
  `raw-events.prune`, `secrets.scan`) additionally require a fresh manual-class online backup
  (`createOnlineBackup` with `retentionClass: "manual"`, `operationId: "maintenance-<jobId>"`,
  `reason: "Before daemon maintenance job <kind>"`) immediately before execution, verified with
  `requireVerifiedBackup` + a `verifyOnlineBackup` re-check against the live DB path
  (`daemon-jobs.ts:756-773`); the resulting `backupId` is merged into the job's stored result. A job
  outside this set but still in `MAINTENANCE_JOB_KINDS` enters maintenance mode without a fresh backup
  gate only when `args.internal === true` (internal backfills triggered at startup); any other real run
  of a `MAINTENANCE_JOB_KINDS` job without `args.internal===true` also requires the backup gate
  (`daemon-jobs.ts:756`: `BACKUP_REQUIRED_JOB_KINDS.has(kind) || args.internal !== true`).
- `runInMaintenance(work)` (`daemon-jobs.ts:782-794`): sets `maintenanceMode = true`, calls
  `options.beforeMaintenance?.()`, runs `work()`, then in a `finally` calls `options.afterMaintenance?.()`
  before clearing `maintenanceMode = false` — `afterMaintenance` runs even if `work()` threw.
- PR1 constructs no executable provider or `RawEventSweeper`; maintenance therefore has no sweeper
  callbacks to stop/restart. The optional sweeper shutdown path remains null-safe for the later
  privacy-owned delivery.
- Both periodic sweeps guard on maintenance mode: `sweepSpool` (`daemon-lifecycle.ts:329-336`) and
  `sweepBackup` (`daemon-lifecycle.ts:378-397`) each no-op if `jobs?.isMaintenanceMode() ||
  rpc.restoreState?.active`.
- Every daemon job has `maxAttempts: 1` (`daemon-jobs.ts:183`, `198`, `501`) — a failed job is never
  automatically retried; `run()` only claims rows `WHERE state='queued' AND attempts < max_attempts`
  (`daemon-jobs.ts:706-712`), so a `failed` job with `attempts=1` can never be re-claimed. A new job must
  be explicitly submitted to retry.
- `DaemonJobService`'s constructor fails-open any jobs left `queued`/`running` from a previous process
  incarnation to `state='failed', error_code='daemon_restarted'` (`daemon-jobs.ts:522-528`) — a crash
  mid-job never leaves a job silently stuck as `running` forever.

---

## 8. Static-scan rules that enforce the boundary

`harness/phase1-static-scan.ts` (repo root, outside `vendor/`) is a hand-written TypeScript-AST scanner
(using the vendored `typescript` compiler package) invoked as a plain Node script; it is not part of
`vendor/codemem`'s own test suite, but is called *from* it (`sole-writer-boundary.test.ts:9-16`, via
`execFileSync(process.execPath, ["--experimental-strip-types",
"harness/phase1-static-scan.ts"])`) as part of the vitest test `P1-T048-01-zero-external-db-handles`.

### 8.1 What it scans

`productionSources(packageSources)` (`phase1-static-scan.ts:137-146`) walks every file under
`vendor/codemem/packages/**` (excluding `dist/` and `node_modules/`) matching `.js/.jsx/.ts/.tsx/.cjs/
.mjs` etc., except files `isTestOnly()` (`phase1-static-scan.ts:128-135`) excludes by suffix
(`*.test.*`, `*.eval.test.*`) or **exact path**: `packages/core/src/test-utils.ts`,
`packages/core/src/test-schema.generated.ts`, `packages/core/scripts/generate-test-schema.ts`.

### 8.2 Rules and what triggers each

| rule | trigger | evidence |
|---|---|---|
| `connect` | call to an identifier resolved (through import aliasing) to `connect` or `connectReadOnly` | `phase1-static-scan.ts:254` |
| `memory_store` | `new X(...)` where `X` resolves to `MemoryStore` | `phase1-static-scan.ts:270` |
| `actor_open` | `X.open(...)` where `X` resolves to `WriterActor` or `ReadOnlyActor` | `phase1-static-scan.ts:263-266` |
| `raw_database` | `new X(...)` where `X` resolves to `Database`, `BetterSqlite3`, or `default` (the default import of `better-sqlite3`) | `phase1-static-scan.ts:268-274` |
| `raw_import` | any import/dynamic-`import()`/`require()` with specifier `"better-sqlite3"` | `phase1-static-scan.ts:205` |
| `ddl` | any string/template literal matching `/\b(?:CREATE\|ALTER\|DROP)\s+(?:TABLE\|INDEX\|TRIGGER\|VIEW\|VIRTUAL\s+TABLE)\b/i`, or a direct `.exec(...)`/`.prepare(...)` call argument containing `VACUUM`/`REINDEX` | `phase1-static-scan.ts:122`, `298-305` |
| `deep_import` (fails the scan) | a **non-core** file importing (statically or dynamically, or via `require`) a specifier whose final path segment (minus extension) is one of `daemon-canonical, daemon-jobs, daemon-lifecycle, db, store, test-utils, writer-actor`, reached either through a `core/src/` path segment or a `@codemem/core/` package specifier | `phase1-static-scan.ts:112-120`, `204-213` |
| `old_direct_path` (fails) | the identifier `BetterSqliteCoordinatorStore`, `buildLocalPack`, `connectCoordinator`, `directEnqueue`, or `flushBoundaryRawEvents` appearing anywhere; or the literal substring `.codemem.sqlite`/`.opencode-mem.sqlite` appearing in a string/template outside `db.ts` and `legacy-cutover.ts` | `phase1-static-scan.ts:92-98`, `276-278`, `288-296` |
| `sidecar` (fails) | the identifier or string-literal substring `_buildSidecarCommand`, `_invokeSidecar`, `_callSidecar`, `_buildCodexSidecarCommand`, `_invokeCodexSidecar`, `_callCodexSidecar`, `bypassPermissions`, `claude_sidecar`, or `codex_sidecar` appearing anywhere | `phase1-static-scan.ts:100-110`, `279-286` |
| `public_bypass` (fails) | `core/src/index.ts` contains a runtime (non-type-only) named export of one of 25 forbidden names, or a wildcard runtime re-export from a forbidden deep module | `phase1-static-scan.ts:64-90`, `307-320` |

### 8.3 Exact-match enforcement (not just "no violations")

For the five opener rules (`connect`, `memory_store`, `actor_open`, `raw_database`, `raw_import`) and
for `ddl`, the scan does not merely forbid *new* hits outside an allow-list — `compareExact`
(`phase1-static-scan.ts:328-350`) requires the **live hit set to equal the expected set exactly** in both
directions: a file hitting the rule that isn't expected is a violation, *and* an expected file that no
longer hits the rule is also a violation (`rule: "disposition"`, `"<rule> allowlist is stale; expected
live hit <file>"`). This means the allow-lists themselves are load-bearing: removing the last caller of
`connect()` from `daemon-jobs.ts` without updating `expectedHits` would fail the build, not silently
pass.

### 8.4 Cross-checks against `evidence/phase1-disposition.md`

`verifyDispositionTable()` (`phase1-static-scan.ts:352-442`) reads
`evidence/phase1-disposition.md` and asserts, independent of the code scan:

- The `## T048 DB handle closure` section's table (`phase1-disposition.md:260-268`) has exactly 5 rows
  whose first column (after stripping backticks) is exactly `{"connect / connectReadOnly", "new
  MemoryStore", "WriterActor.open / ReadOnlyActor.open", "new BetterSqlite3", "test-only opener"}` — no
  more, no fewer, no renamed rows.
- Every file in the code scan's `expectedHits` allow-lists (by basename) is mentioned somewhere in that
  section's text.
- The literal string `packages/core/src/test-utils.ts` and the phrase `test file` both appear in that
  section (the test-only exception must be documented as an exact path/suffix, not a vague carve-out).
- Every RPC method string in `daemon-rpc-contract.ts`'s `RPC_METHODS` array is quoted (backtick-wrapped)
  somewhere in the disposition doc.
- The obsolete string `/v1/operations/backup/` does **not** appear anywhere in the doc (a stale contract
  reference that was superseded).

`verifyCorePackageExports()` (`phase1-static-scan.ts:444-467`) parses `core/package.json`'s `exports`
map and fails if any key or target references a forbidden deep module — catching a *package-level*
bypass (subpath export) that the AST scan of `index.ts` alone wouldn't see.

### 8.5 Runtime double-check on top of the static one

`sole-writer-boundary.test.ts:17-45` additionally imports the compiled `core` package's public surface
(`import * as core from "./index.js"`) and asserts `Reflect.get(core, name)` is `undefined` for all 25
`forbiddenPublicValues` names — a live-object check that a value could not have been re-exported through
a path the AST scanner missed (e.g. `Object.assign` re-export patterns).

### 8.6 Self-test

`phase1-static-scan.ts:501-541` (`--self-test` flag) runs the scanner against two synthetic fixture
strings and asserts it *does* flag `deep_import` and `sidecar`, and *does* record hits for `connect`,
`ddl`, `memory_store`, `raw_database`, `raw_import` — a test of the scanner itself, run separately from
the normal `runStaticScan()` path (mutually exclusive `if (process.argv.includes("--self-test"))`
branch, `phase1-static-scan.ts:543-556`).

---

## 9. Legacy cutover fencing (`legacy-cutover.ts`)

Legacy cutover moves a pre-daemon, path-addressed SQLite file (`~/.codemem.sqlite`,
`~/.opencode-mem.sqlite`, or a `CODEMEM_DB`-configured path) into the daemon-owned
`db/versions/<id>.sqlite` + `db/current` layout, exactly once per `dataDir`, and only as an automatic
step of `startDaemon` (`daemon-lifecycle.ts:283`, `cutoverLegacyLayoutIfNeeded`).

### 9.1 Candidate discovery (`cutoverLegacyLayoutIfNeeded`, `legacy-cutover.ts:439-475`)

- Runs only if `readCurrentDatabasePointer(layout) === null` (no canonical DB has ever been published
  for this `dataDir` yet) — cutover cannot run a second time once a canonical DB exists.
- If `CODEMEM_DB` is set, its resolved path is a candidate only if it exists **and** either its parent
  directory equals `layout.dataDir` or `resolveDatabaseRuntimeDataDir(path) === layout.dataDir` (the
  env var must actually resolve to *this* daemon's data directory).
- Otherwise (or if the env candidate didn't qualify), the default legacy path
  `<dataDir>/mem.sqlite` is checked; and — only when `dataDir` is the process-wide `DEFAULT_DATA_DIR`
  — `~/.codemem.sqlite` and `~/.opencode-mem.sqlite` are also checked.
- **More than one existing candidate is a hard error** (`"Multiple legacy database paths exist; cutover
  requires one unambiguous source."`) — cutover refuses to guess.
- `recoverInterruptedLegacyCutover(layout, legacyPath)` runs before `cutoverLegacyDatabase` on every
  `startDaemon` (§9.4).

### 9.2 The cutover transaction (`cutoverLegacyDatabase`, `legacy-cutover.ts:290-433`)

Ordered steps (all evidence within this function unless noted):

1. Refuse if `readCurrentDatabasePointer(layout) !== null` (defense-in-depth against re-entry).
2. Refuse if the legacy path is a symlink or not a regular file (`:298-301`).
3. `assertInstallManifestCurrent(installManifestPath)` — the installed CLI/hook binaries must match the
   recorded manifest before touching data (`:302`).
4. Compute `fileIdentity` (dev+ino) of the legacy path (`:304`).
5. `requestLegacyOwnersStop(identity, legacyPath)` (`:145-180`): wait up to 250ms for open-handle owners
   to clear on their own; if any remain, for each owner PID that is **not** this process and **is**
   `isTrustedLegacyCodememOwner(pid)` (cmdline matches `(^|[\s/])codemem(\.py)?([\s]|$)` or a
   `packages/cli/dist|src/index.{js,ts}` path, **and** contains one of `serve|daemon|mcp|hook|ingest|
   inject` as a whole word — `:131-143`), re-verify the owner's process identity hasn't changed between
   the scan and the signal, then `SIGTERM` it; an untrusted or unrecognized owner (including any
   non-codemem process, or a codemem process with a non-matching subcommand) is a hard refusal — cutover
   **never** signals a process it cannot positively identify as a legacy codemem runtime. After
   signaling, wait up to 2000ms more; if owners still remain, throw
   `"Legacy database owner did not stop; cutover was not started."`
6. `WriterActor.open(legacyPath)` (`:307`) — opens the legacy file itself as a second writer handle
   (this is *not* the instance lock from §4; it is scoped to the legacy DB file only).
7. `busy_timeout = 0`; switch the legacy DB to WAL (`journal_mode = WAL`) and require it actually took
   effect (`:315-317`).
8. Open a `ReadOnlyActor` on the same legacy path (`:318`) — this becomes the backup source (online
   backup reads through a separate read-only handle while the writer holds the transaction, §10).
9. `lock.exec("BEGIN EXCLUSIVE")` (`:319`) — full exclusive SQLite lock, stronger than `BEGIN IMMEDIATE`.
10. `assertSoleCutoverOwner(identity, legacyPath)` (`:182-191`) — the `/proc`+`lsof` owner scan (§9.3)
    must now report **exactly one** open-file owner and it must be `process.pid`; any other owner set
    aborts.
11. `createRecoveryLink` (`:222-233`) — a hardlink of the legacy file into
    `control/<operationId>.legacy-recovery.sqlite`, `chmod 0600`, fsynced (file + `controlDir`) —
    this is the rollback anchor for step 19/20's crash-recovery path.
12. `createOnlineBackup` of the legacy DB through the `ReadOnlyActor` reader, `reason: "legacy-cutover"`
    (`:325-330`), then `requireVerifiedBackup` + a `verifyOnlineBackup` re-check (`:331-337`) — see §10.
13. Re-confirm `pathHasIdentity(legacyPath, identity)` — the legacy path must still point at the same
    inode it did in step 4; if something swapped it during the backup, abort (`:339-341`).
14. `installTombstone(layout, legacyPath)` (`:200-220`, `:342-343`) — atomically renames a temp symlink
    over the legacy path pointing at `control/legacy-db-tombstone/`, fsyncing the parent directory. From
    this instant, the old path is permanently fenced: opening it (by path) now resolves into an inert
    tombstone directory, not a database.
15. `assertSoleCutoverOwner` again, this time scanned by the **recovery hardlink's** identity
    (`:345-346`) — a final check that nothing opened the file between backup and tombstone via a path
    other than the now-tombstoned original.
16. `assertInstallManifestCurrent` re-checked immediately before publish (`:347`) — the manifest could
    have changed during the (possibly slow) backup step.
17. `runLegacyMigration` (`storage.ts:249-273`) copies the *verified backup artifact* (not the live
    legacy file) into `db/versions/<operationId>.sqlite` and calls `activateDatabaseArtifact` (the
    `prepared→switched→committed` storage journal, §11) to publish it as the new canonical DB.
18. `lock.exec("COMMIT")`, close the reader, close the writer lock, record `"legacy_handles_closed"`
    (`:359-365`).
19. Best-effort cleanup: remove the recovery hardlink and any `-wal`/`-shm` sidecars of the old legacy
    path (`:367-374`) — failure here is logged, not fatal (the cutover itself already committed).

### 9.3 Owner scanning (`listOwnersByIdentity`, `legacy-cutover.ts:80-116`)

Walks every numeric `/proc/<pid>` entry, opens `/proc/<pid>/fd`, and `stat`s each descriptor comparing
`dev`/`ino` against the target file's identity. If a given PID's `fd` directory cannot be read (process
gone, or **permission denied for a process owned by a different UID**) that PID is either skipped
(ENOENT) or — if the failure is a same-UID permission error or an unexplained error and the scanning
process's UID could not rule it out — flagged as `procIncomplete`. When `/proc` scanning is incomplete,
the scan additionally shells out to `lsof -nP -F p -- <path>` (`:64-78`) if `/usr/bin/lsof` or
`/usr/sbin/lsof` exists; **if `/proc` is incomplete and `lsof` is unavailable, the whole owner scan
throws** (`"Legacy database owner scan was incomplete and trusted lsof is unavailable."`,
`legacy-cutover.ts:112`) rather than silently under-reporting owners — a fail-closed design.

### 9.4 Interrupted-cutover recovery (`recoverInterruptedLegacyCutover`, `legacy-cutover.ts:243-267`)

Run unconditionally at the top of every `startDaemon`, before a new cutover attempt:

- If the legacy path is neither tombstoned (symlink to `control/legacy-db-tombstone`) nor a regular
  file, there is nothing to recover — return.
- Find all `control/legacy-<uuid>.legacy-recovery.sqlite` files (regex `LEGACY_RECOVERY_NAME`,
  `:240-241`), sorted.
- If none exist: a tombstoned path with no recovery file is a hard error (`"Interrupted legacy cutover
  recovery file is missing."`) — the daemon refuses to start rather than leave data unrecoverable and
  silently proceed; a non-tombstoned path with no recovery file means no cutover was ever interrupted —
  no-op.
- Otherwise, verify **all** found recovery files share the same file identity (dev+ino) — either that of
  the first recovery file (if tombstoned) or of the still-present legacy file (if not tombstoned); a
  mismatch is `"...recovery files are ambiguous."` (fail-closed).
- If tombstoned: rename the first recovery file back over the legacy path (undoing the tombstone), then
  remove any *other* leftover recovery files. If not tombstoned (crash happened before the tombstone
  step but after the recovery hardlink was created), just remove the leftover recovery file(s) — the
  original legacy file is already back in its normal place.

### 9.5 Rollback on any mid-cutover failure (`legacy-cutover.ts:376-432`)

The `catch` block, in order: if a recovery link still exists and (the tombstone was installed, or the
legacy path's identity has since changed), restore the recovery link back over the legacy path; if a
recovery link still exists and neither of those is true, just remove it (nothing was published, cleanup
only); if a canonical artifact was already published (`activateDatabaseArtifact` succeeded) roll it back
via `rollbackPublishedArtifact` — which itself re-checks the current pointer still equals the
just-published one before removing it (defense against a concurrent activation racing the rollback). If
any of these recovery steps themselves throw, they are collected into an `AggregateError` alongside the
original failure rather than swallowed. The `finally` block additionally rolls back the still-open SQL
transaction and closes the reader/lock handles if the whole operation never reached `completed = true`.

---

## 10. Backup preconditions (`online-backup.ts`)

### 10.1 `requireVerifiedBackup` — the universal gate

```ts
export function requireVerifiedBackup(proof: BackupVerification): void {
  if (!proof.verified || !proof.evidence.trim()) {
    throw new Error("Destructive operation requires a verified backup.");
  }
}
```
(`online-backup.ts:152-156`) — every destructive path (migration upgrade, legacy cutover, maintenance
jobs in `BACKUP_REQUIRED_JOB_KINDS`) calls this before proceeding. A `verified: true` proof with an
empty/whitespace `evidence` string is treated as *not* verified — `evidence` is not decorative, its
non-emptiness is part of the check.

### 10.2 `verifyOnlineBackup` — what "verified" means (`online-backup.ts:519-583`)

An artifact passes only if, in order:

1. It exists.
2. It is a regular file, not a symlink (`lstatSync`).
3. It has no `-wal`/`-shm` sidecars (a backup must be a standalone, checkpointed file).
4. If a `sourcePath` was given, the artifact's `dev`/`ino` must differ from it (a backup can never *be*
   the live database by inode).
5. Its SHA-256 matches `expectedSha256`, if one was supplied.
6. It opens successfully as a `ReadOnlyActor`.
7. `PRAGMA integrity_check` returns exactly one row whose value is `"ok"`.

Any failure returns `{valid: false, ...}` with a diagnostic string rather than throwing; callers decide
whether a failed verification is fatal.

### 10.3 `createOnlineBackup` (`online-backup.ts:601-719`, serialized via `enqueueBackup`)

- All backup creation is funneled through a single module-level promise chain (`enqueueBackup`,
  `:585-599`) — concurrent `createOnlineBackup` calls within one process are serialized, never run in
  parallel, and `pendingBackups` tracks the in-flight count (consulted by restore, §10.5).
- `operationId` must match `/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/`; `reason` must be a non-empty string;
  `payloadHash` (if given by the caller) must equal `sha256(reason)` exactly, or the call is rejected as
  `invalid_request`.
- **Idempotent replay**: if a manifest sidecar already exists for this `operationId` with the *same*
  `payload_hash`, and its artifact re-verifies (`verifyOnlineBackup` against the recorded hash) and its
  manifest hash + content snapshot re-check clean, the existing backup is returned as-is — no new backup
  is taken. If the existing sidecar's `payload_hash` differs, the call is rejected `conflict` (an
  operation ID can never be reused for a different `reason`).
- **Fresh backup**: `input.db.backup(temporaryPath)` (the driver's native online-backup API, streamed
  page-by-page from a live handle) → `finalizeStandaloneBackup` (`:721-736`: reopen the temp copy as a
  `WriterActor`, force `journal_mode = DELETE` + `wal_checkpoint(TRUNCATE)`, then remove any leftover
  `-wal`/`-shm`) → `chmod 0600` + fsync the temp file → atomic rename to the final artifact path →
  `chmod 0600` + fsync the artifact + fsync the destination directory. On any failure the temp file is
  best-effort unlinked and the error rethrown; **the artifact path is never left in a partially-written
  state** because the write always lands at the temp path first.
- After the rename, the artifact is hashed and immediately re-verified with `verifyOnlineBackup`; if that
  fails, the artifact is deleted (`unlinkSync`) and the whole call throws — a backup that cannot be
  proven good is not left on disk to be mistaken for a valid one later.
- The manifest (`BackupManifest`, `online-backup.ts:64-98`) captures `schema_version`,
  `sqlite_source_version`, an FTS schema hash + normalization version, an optional `sqlite_vec`
  version/artifact-hash/platform record, `active_embedding_generation_id`, a `canonical_tables[]`
  row-count+checksum snapshot, and a `created_watermark` (max `raw_events.id`/`created_at`). It is
  written as a hash-authenticated sidecar (`BackupSidecarV2`, `:100-106`: `{version:2, manifest,
  manifest_hash, authenticity:"hash-only", signature:null}`) via the same durable-replace primitive as
  other control files.

### 10.4 Retention (`pruneBackupRetention`, `online-backup.ts:924-970`, and `createDailyBackup`,
`:972-989`)

- Only backups whose manifest `retention_class === "automatic"` are ever pruned; `"manual"` backups
  (legacy cutover, maintenance-job pre-backups, explicit CLI `backup create`) are retained forever by
  this function.
- Keeps at most the 7 most recent distinct **days** (by `created_at` date, UTC) among automatic backups,
  then — among what's left, and not already kept by day or by having a day already represented — up to 4
  more distinct **ISO weeks** (Monday-start, `utcWeekKey`, `:915-922`), each represented by its
  newest-in-week entry (input is pre-sorted newest-first, `:941`). Everything else automatic is removed
  (`durableRemoveFile` on both the sidecar and the artifact).
- `createDailyBackup` derives a deterministic `operationId = "daily-<YYYY-MM-DD>"` from the injectable
  `now()`, creates the backup with `reason: "daily"`, `retentionClass: "automatic"`, then always runs
  `pruneBackupRetention` afterward — meaning at most one automatic backup per calendar day can ever
  exist (the idempotent-replay path in §10.3 makes a same-day second call a no-op re-verify, not a
  duplicate).
- The daemon's own scheduling (`daemon-lifecycle.ts:378-399`) runs `sweepBackup` every 60s, computing
  `backupId = "daily-<today>"` and skipping if a `dailyBackupTask` is already in flight or
  `lastDailyBackupId` already equals today's ID — i.e., it only *attempts* the backup once per day even
  though it's polled every minute — and skips entirely while `jobs.isMaintenanceMode()` or a restore is
  active (§7).

### 10.5 Restore preconditions (`restoreCanonicalBackup`, `online-backup.ts:1143-1248`)

- Refuses (`conflict`) if a staging artifact for the same `operationToken` already exists under a
  *different* `payloadHash`-derived filename (an operation ID can't be replayed against a different
  backup).
- Refuses (`conflict`) if `pendingBackups > 0` — **a restore may never run concurrently with an in-flight
  backup** (`:1175-1180`, `"A backup is active; retry restore after it completes."`).
- If a completed restore result already exists for this exact `(operationId, payloadHash)` and the
  current DB pointer still equals its target, and the staged artifact has no `-wal`/`-shm` and its hash
  still matches the recorded result, the call is idempotently replayed (no re-copy).
- Otherwise: re-verifies the source backup (`verifyCanonicalBackup`); checks the backup manifest's
  `schema_version` falls within `[MIN_WRITABLE_SCHEMA, SCHEMA_VERSION]` and its FTS
  `normalization_version` equals the current `NORMALIZED_SCHEMA_VERSION`. A manifest below the writable
  floor is rejected with `Backup manifest requires the schema 20 bridge before restore.`; a future
  schema or FTS mismatch uses the generic incompatible-manifest error. Both fail before any file is
  touched. `MIN_COMPATIBLE_SCHEMA` remains the read-only compatibility floor; it does not authorize a
  writable restore. The restore then copies the backup artifact into
  `db/versions/` with
  `COPYFILE_EXCL` (fails if the destination already exists and wasn't already validated as a matching
  stage); rebuilds derived indexes on the staged copy (`rebuildStagedDerivedIndexes`, `:1113-1141`: FTS
  `rebuild` if the tables exist, vector table cleared if the vector extension's on-disk hash matches the
  manifest's recorded hash — otherwise vectors are left degraded rather than risk loading a mismatched
  extension binary — then a fresh `integrity_check`); re-diffs the manifest snapshot against the rebuilt
  staged file and aborts if anything drifted; writes a durable restore-result record; then calls
  `activateDatabaseArtifact` (§11) to publish it. The response always carries `restartRequired: true` —
  restore does not attempt to hot-swap a live daemon's open handle.

---

## 11. Migration ordering (`migration-runner.ts`, `db.ts`, `storage.ts`)

### 11.1 `peekMigrationKind(db)` (`migration-runner.ts:33-44`)

```
version = getSchemaVersion(db)
if canAutoBootstrapSchema(db):                    # version==0 AND isSafeEmptyDatabase(db)
    return "bootstrap"
if version == 0:
    throw "Refusing to migrate a non-empty database without a codemem schema."
if !tableExists(db,"memory_items") || !tableExists(db,"sessions"):
    throw "Refusing to migrate an unrecognized or partial codemem database."
if version > SCHEMA_VERSION:
    return null                                    # no downgrade attempt
if version < MIN_WRITABLE_SCHEMA:
    throw "Direct writable upgrade requires MIN_WRITABLE_SCHEMA."
if version == V21_MIGRATION_SOURCE_SCHEMA:          # 20
    return "upgrade"
if isSchemaCompatibilityCurrent(db): return null
return "upgrade"
```

For schema 21, `isSchemaCompatibilityCurrent` requires both the current marker and the required v21
table/column shape. An early v21 database missing
`processing_resume_producer_receipts.target_job_ids_json` therefore enters the verified-backup
upgrade path; the additive shim restores it as `TEXT NOT NULL DEFAULT '[]'`, so historical receipts
remain fail-closed and cannot target jobs created later.

`canAutoBootstrapSchema` (`schema-bootstrap.ts:814-816`) requires **both** `version === 0` and
`isSafeEmptyDatabase(db)` — a `version === 0` file that already contains unrelated (non-codemem) tables
is refused outright rather than bootstrapped over. `SCHEMA_VERSION = 21`,
`MIN_COMPATIBLE_SCHEMA = 6`, and `MIN_WRITABLE_SCHEMA = 20` (`db.ts`). Schemas 6–19 retain the
read-only compatibility floor but require a schema-20 bridge before this runtime may migrate or
restore them.

**A schema version newer than this build's `SCHEMA_VERSION` is treated as "no migration needed", not as
an error or a downgrade attempt** (`version > SCHEMA_VERSION` short-circuits to `null` alongside the
already-current case, `migration-runner.ts:42`) — see §12 for the parity implication.

### 11.2 `runDatabaseMigrations(db, {dbPath, backupAndVerify})` (`migration-runner.ts:47-67`)

```
kind = peekMigrationKind(db)
if kind == null: return                             # no-op
verification = backupAndVerify({db, dbPath, schemaVersion, kind})
if !verification.verified || !verification.evidence.trim():
    throw "Database migration requires a verified backup before schema changes begin."
if kind == "bootstrap":
    bootstrapSchema(db)
    ensureAdditiveSchemaCompatibility(db)
else if getSchemaVersion(db) == V21_MIGRATION_SOURCE_SCHEMA:
    migrateV20ToV21(db)                            # includes strict PR2-critical additive DDL
else if getSchemaVersion(db) == SCHEMA_VERSION:
    ensureAdditiveSchemaCompatibility(db)
else:
    throw "Unsupported database migration path."
assertSchemaReady(db)
```

`migrateV20ToV21` does not call the broad fail-open startup compatibility shim. Inside the same
immediate transaction, before creating v21 tables, it reconciles the v20 completed-job crash window:
the frontier advances only through an exact contiguous chain beginning at `frontier + 1` whose source
rows are all retained and whose ranges do not overlap another legacy batch. An already-advanced
frontier is unchanged; its completed source rows may already have been removed by the legacy prune
command and are preserved without replay or retained-range validation. Any incomplete, overlapping,
or gapped completed candidate that still needs frontier advancement, and every sessionless completed
row, aborts the migration and rolls every frontier update back to v20. The migration then invokes the canonical
creators for the PR2-critical retrieval ledger, mutation receipts/quarantine, and daemon-job schema,
and validates those surfaces and the required memory identity columns without catches; any missing
required surface likewise aborts and rolls the whole database back to v20.

**The backup-and-verify step always completes and is checked before any DDL statement runs.** This
ordering is unconditional — `bootstrapSchema`/`migrateV20ToV21`/`ensureAdditiveSchemaCompatibility` are never reached if
`verification.verified` is falsy or `evidence` is blank, regardless of `kind`.

### 11.3 The two `backupAndVerify` strategies actually used

- **Fresh/empty DB** (`verifyFreshDatabase`, `migration-runner.ts:70-73`): `{verified:
  canAutoBootstrapSchema(db), evidence: verified ? "fresh-empty-database" : ""}` — an empty database
  needs no real backup file; the "evidence" is just a fixed sentinel string. Used by
  `runGatedMigration` when `peekMigrationKind === "bootstrap"` (`online-backup.ts:750-752`) and as the
  default in `test-utils.ts:31` (`openTestMemoryStore`).
- **Non-empty DB / upgrade** (`runGatedMigration`, `online-backup.ts:738-774`): creates a real
  `createOnlineBackup` (reason `"migration"`), `requireVerifiedBackup`s it, re-verifies with
  `verifyOnlineBackup`, then passes a `backupAndVerify` closure into `runDatabaseMigrations` that simply
  re-wraps the already-proven `{verified:true, evidence: proof.evidence}` — the real backup work happens
  *before* `runDatabaseMigrations` is even called; `runDatabaseMigrations` itself never has to know how
  the verification was produced, only that it was.

### 11.4 Where migration runs relative to daemon startup

`openCanonicalWriter(layout)` (`daemon-canonical.ts:21-49`) — the only place `runGatedMigration` is
invoked in production — runs `connect(dbPath)` on either the existing canonical pointer or a fresh
`init-<uuid>.sqlite` path, then `runGatedMigration` **before** constructing the `MemoryStore` and before
the RPC socket is bound (`daemon-lifecycle.ts:285`, called before `bindPrivateSocket` at `:359`) — no
external caller can reach the daemon through the RPC surface until migration (if any) has already
completed and been verified. `connect()` enables persistent WAL only for a safe empty bootstrap or a
schema at least `MIN_WRITABLE_SCHEMA`; opening a schema 6–19 database therefore leaves its journal mode
unchanged before `peekMigrationKind()` rejects the writable upgrade. For a brand-new `dataDir`
(`!existing`), after migration it additionally
runs `wal_checkpoint(TRUNCATE)` and calls `activateDatabaseArtifact` to publish the freshly-bootstrapped
file as the canonical pointer for the first time (`daemon-canonical.ts:35-42`).

---

## 12. Known gaps (documented, not fixed)

| # | gap | evidence | must a reimplementation preserve this, or is it free to differ? |
|---|---|---|---|
| G1 | Force-kill's PID-reuse race: between the final live-identity re-check and `process.kill(pid, "SIGKILL")`, the OS can reuse the PID for an unrelated process (Node has no `pidfd_send_signal`). Explicitly called out in source as an accepted limitation. | `daemon-lifecycle.ts:543-549` (`ponytail:` comment + `process.kill(second.pid, "SIGKILL")` at `:546`) | **Free to differ, and should be improved if the target platform allows it** (e.g. a Rust implementation on Linux can use `pidfd_open`/`pidfd_send_signal` to close this window entirely). This is not an observable-behavior requirement — it's an accepted implementation weakness, not a documented protocol contract. A reimplementation that removes the race is *strictly better*, not a parity break. |
| G2 | A schema version newer than the running build's `SCHEMA_VERSION` is silently treated identically to "already current" (no migration, no error, no warning at the migration-runner level — `db.ts` does emit a `console.warn` for this case in `assertSchemaReadyReadOnly`/`assertSchemaReady`, but `peekMigrationKind` itself is silent). | `migration-runner.ts:42` (`if (version > SCHEMA_VERSION \|\| isSchemaCompatibilityCurrent(db)) return null`) | **Must preserve the observable outcome** (no migration attempt, no destructive action taken against a newer-schema DB) since other code paths depend on `peekMigrationKind` returning `null` here to avoid running old migrations against new data. Whether a Rust build additionally logs a warning here is free to differ — that's not part of this migration-ordering contract. |
| G3 | `requestCleanStop`'s boolean result (from the socket `close`/`error`/`timeout` race) is computed but never consulted by `stopDaemon` — the clean-stop attempt is effectively fire-and-forget, and success/failure is inferred later purely from the liveness poll. | `daemon-lifecycle.ts:242-261` return value vs. `:573-575` call site (result not assigned to any variable that gates subsequent logic) | **Must preserve the observable timing/fallback behavior** (stop always falls through to a liveness poll and force-kill fallback regardless of the clean-stop socket outcome) — this is what makes `stopDaemon` robust to a hung/mismatched clean-stop. A reimplementation may use the boolean internally (e.g. to skip straight to force-kill sooner) only if it does not change the observable `{action}` outcome or shrink the guaranteed `timeoutMs` wait before a force-kill is attempted. |
| G4 | `/proc`-based owner scanning during legacy cutover (`listOwnersByIdentity`) can only positively identify same-UID processes when `/proc/<pid>/fd` is unreadable for a cross-UID process; it falls back to `lsof`, and if `lsof` is absent the whole scan throws rather than proceeding with a possibly-incomplete owner list. | `legacy-cutover.ts:80-116`, esp. `:110-113` | **Must preserve the fail-closed behavior**: an incomplete owner scan with no trusted fallback must abort cutover, never proceed with a partial owner list. The specific choice of `lsof` as the fallback tool is an implementation detail free to differ (a Rust build could use a different mechanism to enumerate open-file owners), as long as the fail-closed guarantee holds. |
| G5 | Test-only opener (`openTestMemoryStore` in `test-utils.ts`) is excluded from the static scan by **exact file path**, not by any code-level marker — a new test helper file that also calls `connect()` directly would need its path added to `isTestOnly()`/the exact-path allow-list to avoid breaking the scan, and nothing forces that addition automatically. | `phase1-static-scan.ts:128-135` | **Free to differ** — this is a build-tooling convenience for the TS test suite, not part of the runtime contract a Rust daemon must reproduce. A Rust reimplementation's own test harness can structure its DB-access allow-listing however is idiomatic for Rust; only the *runtime* invariant (production code has exactly one writer) is in scope. |
| G6 | `CODEMEM_DB_OPEN_TRACE` open-tracing (`recordOpen`/`recordLockOpen`) is dev/test instrumentation gated entirely on an environment variable, writes best-effort with no locking of the trace file itself, and is not part of any documented RPC or CLI surface. | `writer-actor.ts:38-54`, `daemon-lifecycle.ts:77-93` | **Free to differ** — it exists to make Phase 1's own test suite assert "no unexpected DB opens happened"; a Rust reimplementation is not required to reproduce this exact trace format, though it may want an equivalent test hook. |
| G7 | A stale-snapshot `stopDaemon` has no way to finish: the clean-stop request is ignored (nonce mismatch), and the force-kill fallback is then handed that same stale snapshot as `expected`, so it throws `"Force-kill refused: daemon identity mismatch."` and `stopDaemon` rejects after paying the full `timeoutMs`. Neither step re-reads `identity.json` to retry against the current incarnation; the caller must do that itself. | `daemon-lifecycle.ts:567` (snapshot read once), `:580` (stale snapshot passed as `expected`), `:514-516` (first identity check throws) | **Must preserve the refusal.** Killing "whatever is currently there" after a stale-snapshot stop would defeat the identity guard that exists to stop the wrong process being killed. Whether a reimplementation additionally re-reads the identity file and retries the *clean stop* (not the kill) with the fresh nonce is free to differ, as long as no kill is issued against an identity the caller never observed. |

---

## 13. Rust parity requirements

### 13.1 Must reproduce semantically (behavior-identical, implementation-detail-free)

- **Exactly one process may hold a write-capable connection to the canonical DB at a time**, enforced by
  an OS-level exclusive lock held for the full daemon lifetime, acquired with zero retry/backoff
  (immediate success-or-fail) — §4. The specific mechanism (SQLite `BEGIN IMMEDIATE` under rollback-
  journal mode on a dedicated `lock.db`) is one valid implementation; any mechanism that gives the same
  observable guarantees (immediate mutual exclusion per `dataDir`, held for the daemon's lifetime,
  released deterministically on clean shutdown and on force-kill) satisfies parity **only if it
  contends with the TypeScript daemon's lock**. A plain `flock()` on `lock.db` does **not** qualify:
  on Linux `flock()` and the POSIX byte-range locks SQLite uses are independent lock spaces, so a
  TS daemon and a Rust daemon could each believe it is the sole writer and both write the canonical
  DB. Since cutover and rollback both imply a window where either binary may be started against the
  same `dataDir`, a reimplementation must either speak the same SQLite locking protocol on `lock.db`
  or use a mechanism demonstrated by test to block while the TS daemon holds its lock (and vice
  versa). That cross-runtime lock-race test is a Stage 1 exit condition, not an optional check.
- **Daemon identity must be re-derivable and comparable the same way**: `pid` + a boot-scoped process
  start-time + a content fingerprint (executable path + full argv) + a per-incarnation random nonce,
  compared with the same two granularities used in TS — a "same incarnation" check (all four fields) and
  a "same OS process, possibly re-identified after a restart" check (start-time + fingerprint only) —
  §5.1–5.2. The exact fingerprint algorithm (sha256 of exe-path + NUL + cmdline bytes) is not required to
  match byte-for-byte, but must have the same false-positive characteristics (near-zero chance of two
  distinct daemon incarnations, including across a PID-reuse-after-reboot, colliding).
- **Force-kill's full guard sequence must be preserved**: refuse without an identity record; refuse on
  any identity mismatch at any of the (at least two) re-check points; refuse to kill the calling
  process's own PID; only send the kill signal after every check passes; wait a bounded time for the
  process to actually die and report failure (not silently succeed) if it doesn't; only clean up control
  artifacts afterward, and only if the identity on disk still matches what was killed — §5.3.
- **Clean-shutdown-then-force-kill-fallback must be the only shutdown path visible to callers**: a stop
  request always attempts a graceful stop first, tolerates that attempt being ignored (stale nonce,
  daemon already gone, socket refused), always waits out a bounded timeout, and always falls back to the
  full force-kill guard sequence (never a "softer" kill) if the process is still alive after that
  timeout — §6. "Falls back to the guard sequence" includes the sequence **refusing**: when the caller's
  snapshot is stale, the force-kill throws on the identity check and the stop call fails rather than
  killing the live daemon (§6, G7).
- **Maintenance-mode gating**: the same job-kind partition (which kinds enter maintenance mode; which
  additionally require a fresh backup first; which are exempt only when internally triggered), the same
  "no job is auto-retried, a new job must be explicitly submitted" rule, and the same "a crash mid-job
  marks it failed on next startup rather than leaving it stuck `running`" recovery — §7. Background
  sweeps (spool import, daily backup) must the same way suspend while maintenance mode or a restore is
  active.
- **Backup-before-destructive-action must be unconditional and checked, not just attempted**: every
  destructive path (schema upgrade, legacy cutover, maintenance jobs requiring it) must create-or-reuse
  a backup, verify it against the same criteria (standalone file, no WAL sidecars, not the live DB's
  inode, correct hash, opens read-only, passes `integrity_check`), and refuse to proceed if verification
  fails or is empty — §10.1–10.3. A restore must never be allowed to run concurrently with an in-flight
  backup — §10.5.
- **Migration must never run before its backup is verified**, for both the "fresh/empty, sentinel
  evidence" and "non-empty, real backup" cases, and a schema version newer than the running build's
  known version must be treated as no-migration-needed (not an error, not a downgrade attempt) — §11.
  Migration must complete (or no-op) before the daemon's external RPC/socket surface becomes reachable —
  §11.4.
- **Legacy cutover must run at most once per `dataDir`** (refuse if a canonical pointer already exists),
  must resolve ambiguous multiple candidates as a hard error rather than guessing, must only signal
  processes it can positively, narrowly identify as legacy codemem runtimes (never an unrecognized
  process), must fail closed if it cannot obtain a complete picture of who has the file open, must
  install an irreversible fence (tombstone) only after a verified backup exists, must keep a
  crash-recoverable rollback anchor until the operation fully commits, and on restart must either
  complete or fully roll back any operation interrupted mid-flight — never leave the legacy path
  ambiguous between "still the source of truth" and "already migrated" — §9.
- **The static-scan boundary's *policy*, not its TypeScript-AST mechanism, must be preserved for the
  Rust workspace**: the set of modules/crates permitted to hold a write-capable DB connection must be a
  small, explicit, closed allow-list checked automatically at build/CI time (not just by convention),
  and that check must fail if either a new unlisted caller appears *or* a listed caller disappears
  (catching both directions of drift, per §8.3) — §8.

### 13.2 Free to differ (implementation detail, not observable contract)

- The specific TypeScript-AST scanning mechanism, its rule names, and its exact-path test-file
  exclusions (§8, §12 G5) — a Rust equivalent (e.g. a `cargo` lint, a custom build script walking the
  crate graph, or a `grep`-based CI check) is fine as long as it enforces the same closed-allow-list
  policy in both directions.
- The PID-reuse race window in force-kill (G1) — a Rust implementation on Linux should close it via
  `pidfd_open`/`pidfd_send_signal` rather than reproduce the race.
- The `CODEMEM_DB_OPEN_TRACE` / `recordOpen` / `recordLockOpen` debug-tracing format (G6) — any
  equivalent test-observability hook is acceptable, or none at all.
- Whether a "newer schema version than known" condition (G2) additionally logs/warns — only the
  no-migration-attempted outcome is contractual.
- The exact retention math's tie-breaking details beyond the stated day/week counts (§10.4) are
  contractual (7 daily + 4 weekly, automatic-class only, manual never pruned); the internal iteration
  order of the pruning loop is not, as long as the resulting kept/removed sets match for the same input.
- `better-sqlite3`-specific pragma choices that exist purely for the Node driver's own behavior (e.g.
  `busy_timeout = 5000` on normal `connect()` reads, `cache_size`/`mmap_size`/`temp_store` read-tuning
  pragmas in `db.ts:150-183`) are performance tuning, not part of the sole-writer boundary contract
  itself, and are out of scope for this document.
