# Spool on-disk format contract v1

## Status

- Contract version: **v1**
- Frozen from: `vendor/codemem` Phase 1 TypeScript implementation (package `@codemem/core`, plus its `cli` and `mcp-server` consumers)
- Authority: **this document** is the authority for Rust reimplementation parity of the spool subsystem. Where this document and any other design note disagree, this document wins for the spool on-disk format.
- Scope: the write-ahead spool used to hold `POST /v1/events` and `POST /v1/memories/record` mutations when the daemon RPC is unavailable, plus the daemon-side sweeper that drains it. Paths below are relative to `vendor/codemem/` unless stated otherwise.

All normative statements are describing **current behavior only**. No proposals, no TODOs. Gaps and inconsistencies found in the source are recorded under [Known gaps](#known-gaps-documented-not-fixed), not fixed.

---

## 1. Directory layout and permissions

The spool lives under the daemon's control directory, itself under the resolved data directory:

```
<dataDir>/control/spool/
├── tmp/           # in-flight writes, named "<ready-name>.tmp"
├── ready/         # committed, durable, awaiting import by the daemon
├── quarantine/    # entries the daemon refused to import
├── lock           # spool-lock owner file (JSON, distinct from the daemon writer lock)
└── dropped-counter  # fixed-size 4096-byte drop-count record
```

| Path field | Value | Evidence |
|---|---|---|
| `controlDir` | `<dataDir>/control` | `packages/core/src/storage-layout.ts:57` |
| `spoolDir` (`rootDir`) | `<controlDir>/spool` | `packages/core/src/storage-layout.ts:70`, `packages/core/src/spool.ts:173` |
| `tmpDir` | `<spoolDir>/tmp` | `packages/core/src/spool.ts:176` |
| `readyDir` | `<spoolDir>/ready` | `packages/core/src/spool.ts:177` |
| `quarantineDir` | `<spoolDir>/quarantine` | `packages/core/src/spool.ts:178` |
| `lockPath` | `<spoolDir>/lock` | `packages/core/src/spool.ts:179` |
| `counterPath` | `<spoolDir>/dropped-counter` | `packages/core/src/spool.ts:180` |

The daemon's own writer lock (`<controlDir>/lock.db`, a SQLite `BEGIN IMMEDIATE` file lock) is a **separate** file from the spool lock (`packages/core/src/storage-layout.ts:67` vs. `packages/core/src/spool.ts:179`, `packages/core/src/daemon-lifecycle.ts:164-181`). A reimplementation must not conflate the two.

### 1.1 Directory permission enforcement

`ensureSpoolDirectories()` calls `ensurePrivateDirectory()` on `rootDir`, `tmpDir`, `readyDir`, and `quarantineDir` every time the spool is touched (`packages/core/src/spool.ts:184-189`), and it is invoked on **every** lock acquisition (`acquireSpoolLock` → `ensureSpoolDirectories`, `packages/core/src/spool.ts:303`), i.e. on every `spoolMutation`, every sweep, `readSpoolStatus`, and `quarantineSpoolEntry` call.

`ensurePrivateDirectory(path)` (`packages/core/src/storage-platform.ts:39-61`), on every call:

1. Requires `process.platform === "linux"` (`assertSupportedStoragePlatform`, `storage-platform.ts:23-27`).
2. If the path does not exist: `mkdirSync(path, { recursive: true, mode: 0o700 })` (`storage-platform.ts:50`).
3. Whether newly created or pre-existing, `lstatSync` the path and reject if it is a symbolic link or is not a directory (`assertNotSymlinkDirectory`, `storage-platform.ts:29-37`, called at `storage-platform.ts:48,51`).
4. Unconditionally `chmodSync(path, 0o700)` — this re-asserts the mode on **every** call, not only at creation (`storage-platform.ts:53`).
5. Rejects if `statfsSync(path).type` is a known network filesystem magic number (`isNetworkFilesystemType`, `storage-platform.ts:54-56,132-146,164-167`).
6. Rejects if the mount that contains the path, resolved by parsing `/proc/self/mountinfo`, has a forbidden fstype string (`mountFstypeFor` + `isForbiddenMountFstype`, `storage-platform.ts:57-60,148-172,195-223`).

| Check | Rejects |
|---|---|
| Symlinked directory | Any spool subdirectory that is (or becomes) a symlink |
| `statfsSync` magic number | NFS, SMB/CIFS/SMB2, 9P, FUSE-backed (sshfs/rclone/gvfs/s3fs), virtiofs, Ceph, kAFS, AFS, Lustre, GFS2, OCFS2 (`storage-platform.ts:132-146`) |
| `/proc/self/mountinfo` fstype string | `nfs`, `nfs4`, `cifs`, `smb3`, `smbfs`, `9p`, `drvfs`, `virtiofs`, `ceph`, `afs`, `lustre`, `gfs2`, `ocfs2`, and any fstype starting with `fuse` (`storage-platform.ts:148-172`) |

Individual spool files are created `0o600`: the lock owner file via `openSync(lockPath, "wx", 0o600)` (`spool.ts:315`), the drop counter via `writeFileSync(..., { mode: 0o600 })` (`spool.ts:401-405`), and ready/tmp entry files via `writeFileSync(..., { mode: 0o600 })` (`spool.ts:929-934`).

---

## 2. File naming

Every entry file name is **content-addressed**:

```
<quotaClass>-<sha256hex(idempotencyKey, "utf8")>-<sha256hex(canonicalJson({method, body, redaction}))>.json
```

| Component | Derivation | Evidence |
|---|---|---|
| `quotaClass` | `"normal"` or `"reserved"` (§5) | `spool.ts:792-793` |
| key hash | `sha256(idempotencyKey, "utf8").hex()` | `spool.ts:792,871` |
| `payloadHash` | `sha256(canonicalMutationJson({ method, body, redaction }), "utf8").hex()` | `spool.ts:666,789`; `packages/core/src/mutation-dispatcher.ts:65-67` |

- Ready file: `<stem>.json` in `readyDir`.
- Temp file: `<stem>.json.tmp` in `tmpDir` — i.e. exactly the ready file name with `.tmp` appended (`spool.ts:794,874`). `parseSpoolEntry` enforces this exact relationship when validating a file against its own claimed name (`spool.ts:794-796`).
- Quarantine file: `<reason>-<original-name>` placed flat in `quarantineDir`, where `reason` is `"broken_json"` or `"idempotency_conflict"` and `<original-name>` is the source file's own basename (`spool.ts:961,972`). There are no quarantine subdirectories.
- Fixed names: the lock file is always named `lock`, the drop counter is always named `dropped-counter` (§1).

`canonicalMutationJson` (`mutation-dispatcher.ts:54-63`) is `JSON.stringify` with a replacer that recursively sorts the keys of every plain object (by JS string comparison) while leaving arrays and primitives untouched. This is the canonical encoding used both for the hash and for what is written to disk (§3).

### 2.1 Idempotent-write pre-checks

Before admitting a candidate entry as a new write, `spoolMutation` probes for two pre-existing on-disk states, in this order, before ever reaching quota admission (§5):

1. It checks whether the content-addressed `readyPath` already exists (`spool.ts:879-896`).
2. If it exists and its on-disk content is byte-for-byte identical to the freshly canonicalized entry, `spoolMutation` performs two `fsync`s (`tmpDir` then `readyDir`) and returns `{status: "duplicate", quotaClass, path: readyPath}` **without writing anything** — the existing file is left untouched (`spool.ts:889-892`).
3. If it exists but its content differs from the freshly canonicalized entry (a content-hash collision — normally unreachable given content-addressed naming, but defensively handled), this is treated as an I/O failure via `resultForIoFailure` (`spool.ts:880-887`); see §7.2 for what this does to the drop counter.
4. Only if no matching `readyPath` exists does `spoolMutation` check whether a matching `tmpPath` (the same stem with `.tmp` appended) already exists (`spool.ts:897-915`).
5. If `tmpPath` exists and its content matches, this is an interrupted prior write; `spoolMutation` finishes it — `fsyncPath(tmpPath)`, `renameSync(tmpPath, readyPath)`, `fsyncPath` of both directories — and returns `{status: "queued", quotaClass, path: readyPath}`, **not** `{status: "duplicate", ...}` (`spool.ts:907-912`). This is the "rejected as already-in-progress" case being finished, not rejected.
6. If `tmpPath` exists but its content differs, this is likewise treated as an I/O failure (`spool.ts:898-905`).
7. Only after both checks find no matching file does `spoolMutation` proceed to the quota admission check (§5).

A reimplementation must probe for both pre-existing-file states, in this order, before treating the content-addressed name as free to write.

---

## 3. Record encoding and required fields

### 3.1 On-disk bytes

The file's exact byte content is `canonicalMutationJson(entry) + "\n"` (`spool.ts:798,875`). On read, `parseSpoolEntry` re-serializes the parsed object and requires the stored bytes to equal that canonical form exactly, or the entry is treated as unparseable and quarantined (`spool.ts:798-800`).

### 3.2 Entry envelope

```ts
type SpoolEntry = {
  version: 1;
  method: "POST /v1/events" | "POST /v1/memories/record";
  idempotencyKey: string;
  payloadHash: string;      // sha256 hex, 64 lowercase hex chars
  quotaClass: "normal" | "reserved";
  redaction: SpoolRedactionMetadata;
  body: Record<string, unknown>;
};
```
Evidence: `spool.ts:125-133` (type), `spool.ts:55-63` (allowed field set `SPOOL_ENTRY_FIELDS`), `spool.ts:753-761` (parse-time validation). Any field not in `{version, method, idempotencyKey, payloadHash, quotaClass, redaction, body}` makes the entry invalid (`spool.ts:528-535,752`). `version` must be exactly `1`. `payloadHash` must match `/^[a-f0-9]{64}$/` (`spool.ts:52,757-758`).

### 3.3 Redaction metadata

```ts
type SpoolRedactionMetadata = {
  sensitivity: "normal" | "private" | "secret";
  secret_rules_version: string; // see regex below
  redaction_degraded: boolean;
  private_content_omitted: boolean;
  local_only: boolean;
};
```
Evidence: `spool.ts:135-141` (type), `spool.ts:64-70` (allowed field set `REDACTION_FIELDS`), `spool.ts:727-742` (validator `validateSpoolRedaction`).

- `secret_rules_version` must match `^[a-f0-9]{64}(:degraded)?(\+[a-f0-9]{64}(:degraded)?)*$` (`spool.ts:53`, applied `spool.ts:733`).
- Invariant enforced on every parse: `redaction_degraded === secret_rules_version.includes(":degraded")` (`spool.ts:737`). A mismatch is treated as malformed.

### 3.4 Body fields per method

| `method` | Allowed body fields | Evidence |
|---|---|---|
| `POST /v1/events` | `idempotencyKey`, `event` | `spool.ts:72` |
| `POST /v1/memories/record` | `idempotencyKey`, `kind`, `title`, `body`, `confidence`, `project` | `spool.ts:73` |

Unknown body fields are rejected (`rejectUnknownFields`, `spool.ts:528-535`, invoked at `spool.ts:607,768`).

`body.idempotencyKey` must equal the envelope's `idempotencyKey` (`spool.ts:608-610,769-771`).

For `POST /v1/events`, `body.event` must be a normalized event object (validated by `validateNormalizedEvent`, whose required-field set is `NORMALIZED_EVENT_FIELDS` in `packages/core/src/normalized-event.ts:9-33`); `event.idempotencyKey` must equal the envelope key, and `event.sensitivity` must equal `redaction.sensitivity` (`spool.ts:633-636,773-779`). The event's `quotaClass` consistency is checked as described in §5.

For `POST /v1/memories/record`, the body must satisfy `validatePreparedMemoryBody` (`spool.ts:537-561`): `kind` passes `validateMemoryKind`; `title` and `body` are non-empty strings; `project`, if present, is a string; `confidence`, if present, is a finite number in `[0, 1]`.

When the computed sensitivity is `secret`, or the redaction is `redaction_degraded`, a prepared memory-record body has `title`/`body` overwritten with a placeholder (`"[REDACTED:secret]"` or `"[REDACTED:degraded]"`) and `project` deleted, **before** the body is hashed and written (`spool.ts:655-662`). The event branch is **not** symmetric with this: `payload = {}` via `sealDegradedNormalizedEvent` is applied only when this write's redaction is degraded or a merged-in previous redaction was degraded (`redacted.degraded || previousRedaction?.redaction_degraded`). A non-degraded event whose computed sensitivity is `secret` keeps its redacted payload and only has `sensitivity` forced to `"secret"` — it is **not** sealed (`spool.ts:628-637`; the same sealing happens upstream in the RPC clients before the fallback spool write, `packages/cli/src/commands/hook-rpc-client.ts:264-270`, `packages/mcp-server/src/rpc-client.ts:218-224`).

### 3.5 Size bound

`SPOOL_FILE_MAX_BYTES = 65536` (64 KiB) (`spool.ts:39`). Enforced on write and on read, but the two paths behave differently. On write, the `throw` at `spool.ts:877` never reaches the caller: it is raised inside `spoolMutation`'s main `try`, whose `catch` (`spool.ts:946-947`) routes it to `resultForIoFailure`, which increments the drop counter, warns `"spool write failed; event was dropped."`, and **returns** `{status: "dropped", quotaClass, reason: "io_error"}` (`spool.ts:817-830`). `reason` is `"io_error"` rather than `"disk_full"` because an oversize entry is not a `capacityError`. A reimplementation must preserve this as a returned drop result — propagating an exception to the hook fallback path would break fail-open and skip the counter update. On read — `readSpoolFile` rejects any spool file that is not a regular, non-symlink file of at most 64 KiB (`spool.ts:712-718`), also applied to legacy-spool entries (`spool.ts:1160-1162`).

### 3.6 `idempotencyKey` string constraints

Independent of the entry envelope, the raw `idempotencyKey` string itself must be (`validateIdempotencyKey`, `spool.ts:505-526`):
- a `string`,
- non-empty,
- ≤ 256 UTF-8 bytes (`Buffer.byteLength(value, "utf8") <= 256`),
- free of C0 control characters and DEL (any code point `< 0x20` or `=== 0x7f`),
- unchanged after being passed through the redaction preprocessor with `allowlist: ["idempotencyKey"]` — i.e. it must not itself look like sensitive content, or the mutation is rejected (`spool.ts:520-525`).

---

## 4. Idempotency key derivation and when it is fixed

- **Hook-originated events** (`POST /v1/events` from the Claude Code / Codex hook adapters): `idempotencyKey = eventId`, where `eventId` is computed deterministically by a `stableEventId(...)` call, then copied into the normalized event as both `eventId` and `idempotencyKey` (`packages/core/src/normalized-event.ts:112-115`). The two hook sources use separate, differently-shaped derivations:
  - Claude path: computed inside `mapClaudeHookPayload` (called by `buildRawEventEnvelopeFromHook`) as `stableEventId(sessionId, hookEvent, eventIdTsSeed, toolUseId, payloadHash)` — session id, hook event kind, a timestamp seed, tool-use id, and a hash of the adapter payload (`packages/core/src/claude-hooks.ts:611-627`).
  - Codex path: computed inside `mapCodexHookPayload` (called by `buildRawEventEnvelopeFromCodexHook`), using `codex-hooks.ts`'s own separately-defined `stableEventId` (`packages/core/src/codex-hooks.ts:47`), as `stableEventId(sessionId, hookEvent, eventIdTs, turnId, toolUseId, eventIdNonce, payloadHash)` — the same idea plus a `turnId` and a per-call `eventIdNonce` that the Claude version's call does not take (`packages/core/src/codex-hooks.ts:237-245`).
- **Memory-record calls** (`POST /v1/memories/record`, MCP `remember`): `idempotencyKey` is supplied by the caller in the request body; the spool layer does not derive it (`packages/mcp-server/src/rpc-client.ts:55` field list; no derivation code in `spool.ts` for this method).
- In both cases the key is computed **once**, before the RPC attempt, and is reused unchanged for the spool fallback write if the RPC fails:
  - Hook path: `prepareHookEvent()` builds `PreparedHookEvent.event.idempotencyKey` once (`hook-rpc-client.ts:243-287`); `deliverHookEvent()` tries RPC first with that key, and on failure calls `spoolMutation` with the identical `idempotencyKey` (`hook-rpc-client.ts:324-372`).
  - MCP path: `prepare()` builds the request body (including `idempotencyKey`) once; `requestWithSpool()` tries `send()` first, and only on a *retryable* RPC error does it call `spoolMutation` with the same `prepared.body.idempotencyKey` (`rpc-client.ts:352-384`).
- Because the same key is used for both the RPC attempt and the spool fallback, and the daemon's mutation dispatch is itself idempotent on `(method, idempotencyKey)` (§9), a client that races an RPC timeout against a spool write for the same logical event cannot double-apply it.
- Once a mutation reaches `spoolMutation`, the key is validated to equal `body.idempotencyKey` and, for events, `event.idempotencyKey` (`spool.ts:608-610,633-635`); it is never rewritten by the spool layer itself.
- On a spool-fallback write that follows a failed RPC, the caller may pass `previousRedaction` (the redaction metadata already computed for the RPC attempt). `prepareMutation` then merges it with the freshly recomputed redaction via `mergeSpoolRedaction` (monotonic union: `secret` beats `private` beats `normal`; booleans OR together; `secret_rules_version` sets are unioned and sorted) rather than re-running user redaction rules from scratch, specifically to avoid a second run mutating required IDs on an already-degraded payload (`spool.ts:573-597,615-617`). This does not change the `idempotencyKey`, only the `redaction` field baked into `payloadHash`.

---

## 5. Quota classes and reservation accounting

| Constant | Value | Evidence |
|---|---|---|
| `SPOOL_NORMAL_QUOTA_BYTES` | 128 MiB (134,217,728) | `spool.ts:36` |
| `SPOOL_RESERVED_QUOTA_BYTES` | 16 MiB (16,777,216) | `spool.ts:37` |
| `SPOOL_QUARANTINE_QUOTA_BYTES` | 32 MiB (33,554,432) | `spool.ts:38` |
| `SPOOL_FILE_MAX_BYTES` | 64 KiB (65,536) | `spool.ts:39` |
| `SPOOL_RESERVED_MIN_EVENTS` | 64 | `spool.ts:40` (see note below) |

- `quotaClass` is `"reserved"` **only** for `POST /v1/events` whose normalized `event.kind` is in `RESERVED_EVENT_KINDS = {"pre_compact", "session_ended"}` (`spool.ts:51,645`). Every other event kind, and **every** memory record regardless of kind, is `quotaClass: "normal"` (`spool.ts:614,645`). This is re-validated on parse: for events, `quotaClass !== (RESERVED_EVENT_KINDS.has(kind) ? "reserved" : "normal")` is an error (`spool.ts:776-781`); for memory records, `quotaClass !== "normal"` is an error (`spool.ts:783-786`).
- Usage accounting is **not** a persisted counter. `scanUsage()` walks `tmpDir` and `readyDir` fresh, under the spool lock, on every quota check, classifying each file by whether its name starts with `"reserved-"` (`scanDirectory`, `spool.ts:678-695`, invoked by `scanUsage`, `spool.ts:697-710`, and by `spoolMutation`'s admission check, `spool.ts:918-921`).
- Both **pending** (`tmp/`) and **committed-but-not-yet-imported** (`ready/`) bytes count toward the same quota bucket for a given `quotaClass` — an in-flight write already counts against quota before it is durable.
- Quarantine usage is tracked separately from normal/reserved quota (`scanDirectory` with `area === "quarantine"` only increments `usage.quarantineBytes`/`usage.quarantineFiles`, `spool.ts:685-688`).
- Admission: for a candidate entry of size `bytes`, let `used` be the scanned bytes already in that entry's `quotaClass` bucket and `quota` be that class's byte limit. If `used + bytes > quota`, the write is dropped with `reason: "quota_full"` and the drop counter is incremented (`spool.ts:918-926`).
- `SPOOL_RESERVED_MIN_EVENTS = 64` is an exported constant but is **not enforced by any runtime check**. It is used only by a unit test asserting `SPOOL_RESERVED_QUOTA_BYTES / SPOOL_FILE_MAX_BYTES >= SPOOL_RESERVED_MIN_EVENTS` (`packages/core/src/spool.test.ts:213-215`, i.e. `256 >= 64`), documenting the intended capacity floor for the reserved class rather than being enforced in production code paths.

### 5.1 Directory-scan validity check

1. `scanDirectory` (`spool.ts:678-695`) — the function underlying every quota-usage scan (`scanUsage`, itself invoked by `spoolMutation`'s admission check at `spool.ts:918`, by `moveToQuarantineLocked`'s quarantine-quota check at `spool.ts:967`, and by `readSpoolStatus` at `spool.ts:1103`) — `lstatSync`s **every** entry in whichever directory it scans (`tmp/`, `ready/`, or `quarantine/`) and throws `"Spool directories may contain only regular files."` if any single entry is not a regular, non-symlink file (`spool.ts:681-684`), regardless of that entry's filename or whether it is the file the caller actually cares about.
2. This is distinct from §1.1 (symlink rejection of the three spool subdirectories themselves) and from §3.5 (per-target-file checks applied only to the one file being read/parsed by `readSpoolFile`) — it is a whole-directory-scan check that runs over every file present, on every usage scan, not just the file being written or read.
3. The throw is not caught locally by `scanUsage`. `readSpoolStatus` (`spool.ts:1098-1128`) has no `try/catch` around its `scanUsage` call, only a `finally` releasing the lock, so the throw propagates to `readSpoolStatus`'s own caller.
4. Inside `spoolMutation`, the same throw during admission (`spool.ts:918`) is caught by the function's own outer `try/catch` (`spool.ts:946-947`) and reported via `resultForIoFailure` as `{status: "dropped", reason: "io_error"}`; because this occurs after the spool lock is held, the drop counter **is** incremented for it (§7.2). This recurs for every subsequent write attempt sharing the same `quotaClass` bucket until the offending non-regular entry is removed from the spool directories.
5. Inside `importReadySpoolEntries`, the same throw from a `scanUsage` call nested inside `moveToQuarantineLocked` behaves differently depending on where it originates: if raised during `recoverTmpEntriesLocked` (run once per `importReadySpoolEntries` call, before the per-entry loop), it aborts the entire tmp-recovery pass and is caught by the outer `try/catch` at `spool.ts:1018-1025`, logged as "spool importer recovery failed; entries were retained." — not a per-entry failure. If raised later, from a `moveToQuarantineLocked` call inside the per-ready-entry loop (`spool.ts:1043` or `spool.ts:1061`), it is caught by that entry's own `try/catch` (`spool.ts:1036,1068-1069`) and logged as "spool importer could not process an entry; it was retained.", leaving only that one ready file in place for the next sweep.

A reimplementation must preserve this whole-directory validity check (a single non-regular entry anywhere in `tmp/`, `ready/`, or `quarantine/` fails every usage scan, not just an operation on that specific file) and its differing containment per call site (§5.1.3-5).

---

## 6. Warning threshold

- `WARNING_RATIO = 0.8` (`spool.ts:45`).
- **Write-time**: immediately after a successful admission and write, if `(used + bytes) / quota >= 0.8` for the entry's own `quotaClass`, `spoolMutation` emits a single warning through the `onWarning` callback (default `console.error`) with the message `` `${quotaClass} spool usage reached 80%.` `` (`spool.ts:942-944`, `warn()` helper `spool.ts:468-474`). This is edge-observed per call, not edge-triggered/deduplicated across calls — a caller that keeps writing near the threshold will see the warning repeatedly.
- **Status-time**: `readSpoolStatus()` independently recomputes usage and returns a `warnings: string[]` array containing `"normal spool usage reached 80%"`, `"reserved spool usage reached 80%"`, and/or `"spool quarantine usage reached 80%"` whenever the corresponding ratio is `>= 0.8` at call time (`spool.ts:1098-1128`). There is no persisted "already warned" state; this is a pure recomputation.
- `critical: true` iff any of: `dropped.total > 0`, `dropped.quarantineRejected > 0`, `usage.reservedBytes >= SPOOL_RESERVED_QUOTA_BYTES`, or `usage.quarantineBytes >= SPOOL_QUARANTINE_QUOTA_BYTES` (`spool.ts:1119-1123`). Note that `normalBytes` reaching 100% of quota is **not** itself a `critical` condition — it only becomes visible via `dropped.total` once an actual normal-class write is subsequently refused.

---

## 7. Drop counter

### 7.1 Storage shape

```ts
type DropCounter = {
  version: 1;
  total: number;
  byKind: Record<string, number>;
  firstDroppedAt: string | null; // /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/
  lastDroppedAt: string | null;
  quarantineRejected: number;
};
```
Evidence: `spool.ts:116-123` (type), `spool.ts:44` (timestamp regex `COUNTER_TIMESTAMP`), `spool.ts:409-442` (`readCounter` validation — `byKind` keys must each be `"memory_record"` or a recognized normalized-event kind, `spool.ts:432-440`).

The file is a **fixed-size, space-padded 4096-byte record** (`COUNTER_BYTES = 4096`, `spool.ts:43`): `encodeCounter()` serializes the JSON, throws if it exceeds 4096 bytes, and pads the remainder with ASCII spaces (`0x20`) (`spool.ts:391-397`). It is created lazily with `flag: "wx"` (fails if it already exists) the first time it's needed (`initializeCounter`, `spool.ts:399-407`), and thereafter is rewritten **in place** by opening with `"r+"` and overwriting the full 4096-byte buffer, `fsyncSync`, close — not via a temp-file-and-rename durable replace (`writeCounter`, `spool.ts:444-458`).

### 7.2 What increments it, and what does not

`incrementDrop(layout, kind, onWarning)` (`spool.ts:484-503`) bumps `total`, `byKind[kind]`, sets `firstDroppedAt` if it was `null`, and always updates `lastDroppedAt` to the current ISO timestamp.

| Drop `reason` | Increments drop counter? | Where |
|---|---|---|
| `"quota_full"` | Yes | `spool.ts:923` |
| `"disk_full"` / `"io_error"` from a write/rename/duplicate-mismatch failure **after the spool lock is held** | Yes | `resultForIoFailure` → `incrementDrop`, `spool.ts:817-831`, called from `spool.ts:881-886,893-895,898-903,913-915,938-940,946-947` |
| `"invalid"` (mutation failed `prepareMutation`) | **No** | `spool.ts:833-845` — this branch returns before `layout` is even resolved, so `incrementDrop` is never called |
| `"lock_timeout"` | **No** | `spool.ts:848-855` |
| `"disk_full"` / `"io_error"` encountered while **acquiring** the spool lock (before the lock is held) | **No** | `spool.ts:856-866` |

`byKind`'s key is `"memory_record"` for `POST /v1/memories/record`, or the (best-effort) `String(event.kind)` — falling back to `"unknown"` if it can't be read — for `POST /v1/events` (`dropKind`, `spool.ts:476-482`).

If `incrementDrop` itself throws (e.g. the disk is so full that even the fixed 4096-byte counter rewrite fails), the error is swallowed and only a warning is emitted; the drop is **not** retried or recorded, so `total` under-counts in that corner case (`spool.ts:499-502`, comment: "A completely full disk may prevent even the preallocated counter rewrite.").

### 7.3 Quarantine-rejection counter

`quarantineRejected` is incremented only via `incrementQuarantineRejected()` (`spool.ts:460-466`), called exactly when `moveToQuarantineLocked` finds that moving an entry into quarantine would push `quarantineBytes` over `SPOOL_QUARANTINE_QUOTA_BYTES` (`spool.ts:967-971`). In that case the source entry is **left in place** (not deleted, not moved) and the function returns `{ status: "full" }` instead of moving it.

---

## 8. Quarantine and conflict rules

`moveToQuarantineLocked(layout, area, name, reason)` (`spool.ts:955-978`), called only while the spool lock is held:

1. `basename(name) !== name` is rejected (path traversal guard) (`spool.ts:961`).
2. The source (`tmp/<name>` or `ready/<name>`) must be a regular, non-symlink file (`spool.ts:963-966`).
3. If `quarantineBytes + sourceSize > SPOOL_QUARANTINE_QUOTA_BYTES`: increments `quarantineRejected` and returns `{status: "full"}` **without moving the file** (§7.3).
4. Otherwise, `renameSync` to `quarantine/<reason>-<name>`. If that destination already exists, `moveToQuarantineLocked` **throws** (`spool.ts:973`) — see [Known gaps](#known-gaps-documented-not-fixed).
5. On success, both the source directory and the quarantine directory are `fsync`'d (`spool.ts:974-976`).

`reason` is one of:

| Reason | Meaning | Raised from |
|---|---|---|
| `"broken_json"` | Entry failed to parse/validate as a well-formed `SpoolEntry`, or a recovered `tmp` entry no longer matches its already-promoted `ready` counterpart | `recoverTmpEntriesLocked` (`spool.ts:980-1003`), `importReadySpoolEntries` on a parse failure (`spool.ts:1039-1048`) |
| `"idempotency_conflict"` | The daemon's mutation dispatcher found an existing committed mutation under the same `(method, idempotencyKey)` with a **different** `payloadHash` | `dispatchSpoolMutation` returning `"conflict"` on `MutationConflictError`, or after event reconciliation when the durable receipt hash differs from the incoming event hash (`packages/core/src/daemon-rpc.ts`), handled in `importReadySpoolEntries` (`spool.ts:1060-1065`) |

`quarantineSpoolEntry(dataDir, readyName, reason)` (`spool.ts:1076-1096`) is the externally callable form (e.g. for operator tooling): it requires `readyName` to be a bare basename ending in `.json` and `reason` to be one of the two values above, acquires the spool lock itself, and delegates to `moveToQuarantineLocked` against `readyDir`.

---

## 9. Lock and deadline behavior

### 9.1 Lock file protocol

The spool lock is a plain file at `spool/lock` (not a `flock`/`fcntl`-style advisory lock) containing a JSON owner record, created with `O_CREAT|O_EXCL` semantics (`openSync(path, "wx", 0o600)`, `spool.ts:315`):

```ts
type SpoolLockOwner = {
  version: 1;
  pid: number;
  startTime: string;   // "<boot_id>:<starttime-field-from-/proc/pid/stat>"
  fingerprint: string;  // sha256(exe-realpath + "\0" + cmdline)
  nonce: string;        // randomUUID(), fresh per acquisition attempt
};
```
Evidence: `spool.ts:191-197` (type), `spool.ts:305-311` (construction), `packages/core/src/storage-platform.ts:240-265` (`readProcessIdentity`, producing `startTime`/`fingerprint`).

Acquisition (`acquireSpoolLock`, `spool.ts:297-389`):

1. `ensureSpoolDirectories(layout)` (§1.1).
2. Attempt `openSync(lockPath, "wx", 0o600)`.
3. On success: write the JSON owner record (`+ "\n"`), `fsyncSync` the fd, `lstat` the path back and re-verify device/inode and owner-content identity to detect a replace-during-init race (`spool.ts:318-342`), then `fsyncPath(rootDir)` to durably record directory entry creation. If that `lstat` fails with `ENOENT`, the path is gone because a waiting writer already reclaimed this still-uninitialized lock; that is the same race, so it is raised as `EEXIST` and retried rather than surfacing as an fs error (`spool.ts:326-335`). Any other `lstat` error propagates unchanged.
4. On `EEXIST`: call `removeStaleLock(lockPath)` (below), then, if `expiresAt - now <= 0`, throw `SpoolLockTimeoutError`; otherwise sleep `min(LOCK_WAIT_MS, remaining)` and retry.
5. `LOCK_WAIT_MS = 5` — the poll sleep uses a synchronous `Atomics.wait` on a dedicated `SharedArrayBuffer`-backed `Int32Array` (`spool.ts:48-49,286-288`).

`removeStaleLock(lockPath)` (`spool.ts:242-276`):

- Non-existent lock file → considered removable (returns `true`, nothing to remove).
- Existing lock that is not a regular file, or is a symlink → **throws** (`spool.ts:249-251`), it does not silently steal.
- Reads the owner record. If a well-formed owner is present and `lockOwnerAlive(owner)` is true (process not a zombie/dead **and** `/proc/<pid>` start-time+fingerprint still match the recorded values, `spool.ts:229-240`), the lock is **not** stale — returns `false`.
- If there is no parseable owner record at all, an initialization grace period applies: `Date.now() - info.mtimeMs <= LOCK_INITIALIZATION_GRACE_MS` (25 ms, `spool.ts:47,254`) — a lock file younger than 25 ms with unreadable content is *not yet* considered stale (it may still be mid-write by its creator).
- Before unlinking, re-reads device/inode and owner content and only proceeds if they are unchanged from the first read (`spool.ts:256-267`) — avoids stealing a lock that was replaced between the staleness check and the removal. An `ENOENT` from that re-read means a competing writer already removed the lock, which is the same outcome as the first read's guard: returns `true` (`spool.ts:259-264`). Any other error propagates unchanged.
- Unlinks and `fsyncPath`s the parent directory (`spool.ts:268-275`).

Release (`SpoolLockHandle.close()`, `spool.ts:344-365`): re-verifies device/inode + owner-content match before unlinking (so a lock this handle no longer actually owns — e.g. because it was reaped as stale by someone else — is left alone), unlinks, `fsyncPath`s the parent, closes the fd. A failed release is deliberately swallowed ("recovered after this process exits").

### 9.2 Deadline

| Constant | Value | Evidence |
|---|---|---|
| `SPOOL_LOCK_DEADLINE_MS` (default) | 100 | `spool.ts:41` |
| Caller-supplied bound | must be an integer in `[1, 250]` | `normalizedDeadline`, `spool.ts:809-815` |
| `LOCK_INITIALIZATION_GRACE_MS` | 25 | `spool.ts:47` |
| `LOCK_WAIT_MS` (poll interval) | 5 | `spool.ts:48` |
| `LOCK_OWNER_MAX_BYTES` | 2048 (owner file larger than this, or a symlink, is treated as absent/unowned) | `spool.ts:46,202` |

`acquireSpoolLock` validates its `deadlineMs` argument via `normalizedDeadline` as the very first step, before any I/O (`spool.ts:301`) — a direct caller of `acquireSpoolLock` observes a synchronous throw on an out-of-range value. `spoolMutation`, however, calls `acquireSpoolLock` inside its own `try { … } catch (error) { … }` block (`spool.ts:848-866`), and that catch is a blanket catch: only `SpoolLockTimeoutError` is special-cased (`spool.ts:852-855`). A deadline-validation `Error` is not that type, so it falls through to the generic branch (`spool.ts:856-866`), and `spoolMutation` *returns* `{status: "dropped", quotaClass: entry.quotaClass, reason: "io_error"}` instead of throwing to its own caller. Because this failure occurs while *acquiring* the lock (before the lock is held), the drop counter is **not** incremented for it (§7.2). A caller passing an invalid `lockDeadlineMs` to `spoolMutation` therefore observes an ordinary dropped-write result indistinguishable from a real lock-acquisition I/O failure, not a synchronous exception. On a genuine lock-acquisition timeout (a valid deadline that simply expires), `spoolMutation` returns `{status: "dropped", reason: "lock_timeout"}`, likewise without incrementing the drop counter (§7.2).

---

## 10. Sweeper scheduling and its skip conditions

The daemon's spool sweep is the function `sweepSpool` defined inline in `startDaemon()` (`packages/core/src/daemon-lifecycle.ts:329-336`):

```ts
const sweepSpool = () => {
  if (jobs?.isMaintenanceMode() || rpc.restoreState?.active) return;
  try {
    importReadySpoolEntries(layout.dataDir, (entry) => dispatchSpoolMutation(rpc, entry));
  } catch {
    console.error("[codemem] spool sweep failed; ready entries were retained.");
  }
};
```

- **Skip conditions** (checked at the top of every invocation, before acquiring the spool lock): `jobs.isMaintenanceMode()` is true (`packages/core/src/daemon-jobs.ts:531-533`, flag flipped around maintenance-mode job execution), **or** `rpc.restoreState.active` is true (set for the duration of a backup restore, `packages/core/src/daemon-rpc.ts:513-522`). Either condition causes the **entire sweep to no-op** for that tick — it is not a partial skip of only affected entries.
- **Scheduling**: `sweepSpool()` is called once synchronously at daemon startup, after draining the legacy hook spools (`daemon-lifecycle.ts:358`), and then on a `setInterval(sweepSpool, 1_000)` (1000 ms) that is immediately `.unref()`'d so it cannot keep the Node process alive on its own (`daemon-lifecycle.ts:362-363`). The timer is cleared on daemon shutdown (`releaseResources`, `daemon-lifecycle.ts:204`).
- `importReadySpoolEntries` catches and logs lock-acquisition failures at both acquisition sites (`spool.ts:1012-1017,1029-1035`) and returns, so those failures do not propagate to `sweepSpool`. Any other error that escapes the importer is caught and logged by `sweepSpool`; ready entries are retained for the next tick.

---

## 11. Commit-before-delete ordering

`importReadySpoolEntries(dataDir, handler)` (`spool.ts:1005-1074`):

1. Acquire the spool lock; run `recoverTmpEntriesLocked(layout)` (§11.1); snapshot `readdirSync(readyDir).sort()` into `names`; release the lock. (One lock acquisition for recovery + listing.)
2. For **each** name in `names`, acquire the spool lock fresh (one acquisition per entry, so a long-running sweep does not hold the lock across an unbounded handler call):
   - Re-check `existsSync(path)` (another sweep or process may have already consumed it).
   - Parse the entry (`parseSpoolEntry`); on failure, quarantine it as `"broken_json"` and continue to the next name.
   - Call `handler(entry)` — this is `dispatchSpoolMutation`, which applies the mutation to the canonical store and returns `"committed"` or `"conflict"`, or throws for any other failure. Event reconciliation compares the retained Class A receipt hash with the incoming event hash so a non-success reconciliation cannot be mistaken for a commit (`daemon-rpc.ts`).
   - **If the handler throws** (anything other than a conflict): caught, logged ("spool import failed; ready entry was retained."), and the file is left untouched in `ready/`.
   - **If `"committed"`**: only now is the file removed, via `durableRemoveFile(path)` (`spool.ts:1056-1058`) — `unlinkSync` followed by `fsyncPath` of the parent directory (`storage-platform.ts:95-104`).
   - **If `"conflict"`**: the entry is moved to quarantine with reason `"idempotency_conflict"` (§8); it is not deleted outright.
   - Release the lock for this entry regardless of outcome.

The critical ordering guarantee is: **the mutation is durably applied to the canonical store before its spool file is deleted.** If the daemon crashes between the commit and the `unlink`, the entry is simply reprocessed on the next sweep; because the canonical mutation-dispatch table is itself keyed on `(method, idempotencyKey)` and treats a matching `payloadHash` as an idempotent no-op returning the already-recorded receipt (`packages/core/src/mutation-dispatcher.ts:105-121`), redelivery through `dispatchSpoolMutation` is safe and simply reaches `"committed"` again, after which the file is deleted. A differing replay is durably reconciled/quarantined and the spool file moves to `idempotency_conflict` rather than being deleted. There is no window in which a mutation is applied twice, and no window in which a crash can lose an applied mutation's spool file without the mutation having been durably applied.

### 11.1 Tmp-entry recovery (`recoverTmpEntriesLocked`, `spool.ts:980-1003`)

Runs under the same lock, before the ready-directory listing is taken, once per `importReadySpoolEntries` call:

For each name in `tmpDir` (sorted):
- Read + `parseSpoolEntry` it as a `tmp`-area entry; on failure, quarantine as `"broken_json"` and continue.
- Compute the corresponding ready path (`name` minus the trailing `.tmp`).
- If a ready file with that name already exists:
  - If its content matches the tmp file's content byte-for-byte, the tmp file is a stale duplicate of a write that already completed its rename — `durableRemoveFile` it.
  - Otherwise (content differs — should be unreachable given content-addressed naming, but is defensively handled), quarantine the tmp file as `"broken_json"`.
- Otherwise (no ready file yet — the process crashed between `writeFileSync(tmp)` and `renameSync(tmp → ready)`): `fsyncPath(tmpFile)`, then `renameSync` it into `readyDir`, then `fsyncPath` both directories — i.e. finish the interrupted write.

---

## Known gaps (documented, not fixed)

1. **Drop counter under-counts certain drop reasons.** `incrementDrop` is only called for `reason: "quota_full"` and for `"disk_full"`/`"io_error"` that occur **after** the spool lock has been acquired (§7.2, `spool.ts:833-867` vs. `spool.ts:824,923`). Drops with `reason: "invalid"` (malformed mutation), `reason: "lock_timeout"`, and `reason: "disk_full"`/`"io_error"` encountered while *acquiring* the lock are never reflected in `dropped.total`, `dropped.byKind`, or the `critical` flag derived from them. This is directly observable via `readSpoolStatus()`.
   **A reimplementation must preserve this exact under-counting** — the drop counter's contract is "counts these specific reasons," not "counts every drop," and status output (including `critical`) is derived from it.

2. **The drop counter can itself silently fail to record a drop.** If disk exhaustion prevents even the fixed 4096-byte in-place rewrite of `dropped-counter`, `incrementDrop`'s own write throws, the error is caught, only a warning is emitted, and the drop is not counted (`spool.ts:499-502`). This is an explicit, commented fail-open ("A completely full disk may prevent even the preallocated counter rewrite.") rather than an oversight.
   **Must preserve**: do not make the counter update itself infallible in a way that changes observable `total`/`byKind` under total disk exhaustion.

3. **Quarantine-destination collisions throw and can livelock a sweep.** `moveToQuarantineLocked` throws `Error("Quarantine entry already exists.")` if `quarantine/<reason>-<name>` already exists (`spool.ts:973`) rather than treating it as already-handled. Because quarantine file names are derived from the *same content-addressed* ready-file name, this is reachable in practice: a mutation that is quarantined as `"idempotency_conflict"`, then resubmitted with identical content (byte-identical `idempotencyKey`+`payloadHash` → identical `readyName`) by a client that does not know it was quarantined, produces a new `ready/<name>` file which will conflict again on the next sweep and then throw when the sweep tries to quarantine it a second time under the same destination name. `importReadySpoolEntries`'s per-entry `catch` (`spool.ts:1068-1070`) turns this into "spool import failed; ready entry was retained." and the same failure repeats on every subsequent sweep tick until an operator intervenes (e.g. via `quarantineSpoolEntry` with the original name, which removes the ready file, or by deleting the stale quarantine file).
   **Must preserve** the throw-and-retain behavior (a reimplementation that instead silently overwrites the existing quarantine file would produce different observable retained/quarantined state and could lose the first quarantined entry's forensic content).

4. **A duplicate-content write can be reported as `dropped` while the queued mutation is still present.** This presupposes the successful duplicate-detection path described in §2.1: when `spoolMutation` finds a ready file whose content already matches the candidate (a true duplicate submission), it still performs `fsyncPath(tmpDir)`/`fsyncPath(readyDir)` before returning `status: "duplicate"` (`spool.ts:889-892`). If that fsync itself fails, the failure is routed through `resultForIoFailure` → `incrementDrop`, returning `status: "dropped"` and bumping the drop counter (`spool.ts:893-895`) — even though the pre-existing ready file was never touched, remains on disk unchanged, and will still be picked up and committed by the next sweep. The caller is told the mutation was dropped when it is, in fact, still queued and will still be applied.
   **Must preserve**: a reimplementation must not "correct" this into a `duplicate` result on fsync failure, since downstream callers (and the drop counter's observable count) are calibrated to this behavior.

5. **`SPOOL_RESERVED_MIN_EVENTS` is not runtime-enforced.** It documents an intended floor (`SPOOL_RESERVED_QUOTA_BYTES / SPOOL_FILE_MAX_BYTES >= 64`) but no code path checks it; only a unit test does (`packages/core/src/spool.test.ts:213-215`).
   **Free to differ**: a reimplementation is not required to reproduce this constant or check as a runtime behavior — it constrains only the relationship between the two quota constants, which are themselves normative (§5).

---

## Rust parity requirements

### Must reproduce byte-for-byte or exactly

- Directory layout, names, and permissions: `control/spool/{tmp,ready,quarantine}`, `spool/lock`, `spool/dropped-counter`; `0o700` on every directory on every touch; `0o600` on every file; the full symlink/network-filesystem/forbidden-mount rejection set in §1.1, including the `fuse*`-prefix wildcard.
- Filename scheme: `{quotaClass}-{sha256hex(idempotencyKey)}-{payloadHash}.json`, its `.tmp` sibling, and quarantine's `{reason}-{originalName}`.
- Canonical JSON encoding: recursive key-sort of plain objects (arrays and primitives untouched), trailing `\n`, used both for `payloadHash` and for the literal on-disk bytes — read-time re-serialization must match byte-for-byte or the entry is treated as unparseable.
- The entry envelope's exact field set (§3.2) and the redaction metadata's exact field set and `redaction_degraded ⇔ secret_rules_version.includes(":degraded")` invariant (§3.3).
- Quota numbers: 128 MiB normal / 16 MiB reserved / 32 MiB quarantine / 64 KiB per-file, and `WARNING_RATIO = 0.8`.
- `RESERVED_EVENT_KINDS = {pre_compact, session_ended}` as the only reserved-class events; all memory records are `normal`.
- Quota accounting as a live directory scan (not a persisted counter) over `tmp/`+`ready/` combined per class, classified by filename prefix.
- Drop-counter storage shape (fixed 4096-byte padded record, in-place `r+` overwrite, not durable-replace) and exactly the reason set that increments it vs. does not (§7.2, and Known gaps 1 and 4 — do not "fix" the undercount or the duplicate-fsync-failure over-count/misreport).
- Idempotent-write pre-checks (§2.1): probe for an existing `readyPath` before an existing `tmpPath`, in that order, before quota admission; `duplicate` (matching ready file, no write) vs. `queued` (matching tmp file, finish the interrupted rename) are distinct result shapes, and a content mismatch at either check is an I/O failure, not a rejection.
- The whole-directory-scan validity check on every usage scan (§5.1): a single non-regular file anywhere in `tmp/`, `ready/`, or `quarantine/` fails the entire scan, not just an operation on that file.
- Quarantine admission rule (reject-when-would-exceed-quota, leave source untouched, bump `quarantineRejected`) and the throw-on-existing-destination behavior (Known gap 3 — preserve, do not silently overwrite).
- Lock protocol: JSON owner record shape, `O_CREAT|O_EXCL` creation, fsync-then-verify-identity publish check, staleness detection via `/proc/<pid>/stat` liveness plus start-time+exe+cmdline fingerprint match, the 25 ms init grace window, default 100 ms deadline with caller range `[1, 250]` ms, and dropping with `reason: "lock_timeout"` (uncounted, per Known gap 1) on expiry.
- Commit-before-delete ordering: a ready entry's file is deleted only after the canonical store has durably committed the mutation; idempotent re-dispatch on crash-recovery replay must be safe (relies on the canonical store's own `(method, idempotencyKey)`-keyed idempotent-receipt behavior, which is out of this document's scope but is the property the spool layer's crash-safety depends on).
- Tmp-entry recovery rules on sweep start (§11.1): promote-if-ready-missing, drop-duplicate-if-ready-matches, quarantine-if-ready-mismatches.
- Sweeper skip conditions (maintenance mode OR active restore → entire sweep no-ops) and the whole-sweep failure containment (a thrown error from the import pass is logged and entries are retained, never propagated to crash the sweep loop).
- `idempotencyKey` string constraints (≤256 UTF-8 bytes, non-empty, no C0/DEL control characters, must not itself be flagged as sensitive content) and that it is fixed once (at hook-normalization time for events, at caller-supply time for memory records) and reused unchanged across the RPC attempt and the spool fallback for the same logical request.

### Free to differ (semantics only, not the observable contract above)

- The specific blocking-sleep primitive used while polling for the lock (Node uses `Atomics.wait` on a `SharedArrayBuffer`), as long as the deadline math, `[1,250]` ms bound, and 5 ms poll cadence's *effect* (bounded wait, then `lock_timeout`) hold.
- Internal error types, control-flow structure, and how `onWarning`/log-callback plumbing is implemented, as long as the counted-vs-uncounted drop-reason set (§7.2) and the quarantine/`readSpoolStatus` observable fields are unchanged. The exact English wording of warning log messages is not itself part of this on-disk-format contract.
- Which specific `fsync` calls are issued and in what granularity, as long as the crash-safety invariants they exist to provide are preserved: a `tmp` write is recoverable after a crash, a `ready` file only becomes visible after an atomic rename, and a spool file is removed only after its mutation is durably committed.
- The `SPOOL_RESERVED_MIN_EVENTS` constant itself (Known gap 5) — it is documentation of a relationship between two other, normative, constants, not an independently observable runtime behavior.
