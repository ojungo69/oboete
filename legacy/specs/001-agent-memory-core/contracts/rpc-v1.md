# Local RPC wire contract v1

## Status

- Contract version: **v1**
- Frozen from: `vendor/codemem` Phase 1 TypeScript implementation (package `@codemem/core`, plus its `mcp-server` and `cli` consumers)
- Authority: **this document** is the authority for Rust reimplementation parity of the daemon's local RPC surface. Where this document and the TypeScript source disagree, re-derive from source; where this document and any other design note disagree, this document wins for the RPC wire contract.
- Scope: the Unix-domain-socket JSON-line RPC protocol between the daemon (`packages/core/src/daemon-rpc.ts`, `daemon-lifecycle.ts`) and its clients (`packages/mcp-server/src/rpc-client.ts`, `packages/cli/src/commands/hook-rpc-client.ts`). Paths below are relative to `vendor/codemem/` unless stated otherwise.

All normative statements describe **current behavior only**. No proposals, no TODOs. Gaps and inconsistencies found in the source are recorded under [Known gaps](#10-known-gaps-documented-not-fixed), not fixed. Normative rules are numbered `R1…R58` inline for traceability (`R52`–`R58` were added after the initial freeze and appear in their topically-correct section rather than at the end).

---

## 1. Transport

### 1.1 Data directory and socket path

The daemon resolves a `StorageLayout` from a data directory root (`packages/core/src/storage-layout.ts:55-73`):

| Field | Path | Evidence |
|---|---|---|
| `dataDir` | `resolve(dataDir)` | `storage-layout.ts:56,60` |
| `controlDir` | `<dataDir>/control` | `storage-layout.ts:57,61` |
| `dbDir` | `<dataDir>/db` | `storage-layout.ts:58,62` |
| `versionsDir` | `<dbDir>/versions` | `storage-layout.ts:63` |
| `lockPath` | `<controlDir>/lock.db` | `storage-layout.ts:67` |
| `identityPath` | `<controlDir>/identity.json` | `storage-layout.ts:68` |
| `socketPath` | `<controlDir>/daemon.sock` | `storage-layout.ts:69` |
| `spoolDir` | `<controlDir>/spool` | `storage-layout.ts:70` |
| `backupsDir` | `<controlDir>/backups` | `storage-layout.ts:71` |

**R1**: The default data directory, when neither `dataDir` nor `dbPath`/`CODEMEM_DATA_DIR`/`CODEMEM_DB` is supplied, is `~/.codemem` (`storage-layout.ts:6-7,47-53`). A custom `dbPath` maps deterministically to `~/.codemem/runtimes/<sha256(resolve(dbPath))[:32]>`, except for one legacy tombstone-symlink special case that maps back to the legacy directory itself (`storage-layout.ts:24-45`).

**R2**: The RPC socket is always `<dataDir>/control/daemon.sock` for the resolved data directory — clients never learn the socket path from a handshake response; they compute it themselves via `resolveStorageLayout(...).socketPath` (`rpc-client.ts:275`, `hook-rpc-client.ts:302`).

### 1.2 Directory and file permissions

**R3**: `dataDir`, `controlDir`, `dbDir`, `versionsDir`, `spoolDir`, and `backupsDir` are each created (if missing) with mode `0o700` and re-`chmod`ed to `0o700` on every call, via `ensurePrivateDirectory` (`storage-platform.ts:39-61`, invoked for each path by `ensureStorageLayout`, `storage.ts:41-47`). A pre-existing symlink at any of these paths is rejected (`storage-platform.ts:29-37,47-48`).

**R4**: The socket file is bound then `chmod`ed to `0o600` after `listen()` succeeds (`daemon-lifecycle.ts:183-199`, specifically `chmodSync(socketPath, 0o600)` at `daemon-lifecycle.ts:192`). Before binding, any pre-existing file at `socketPath` is unlinked (`daemon-lifecycle.ts:184`).

**R5**: `identity.json` is written via `durableReplaceFile`: a temp file created with `flag: "wx", mode: 0o600` then renamed into place, followed by `fsync` of the parent directory (`storage-platform.ts:72-93`, called at `daemon-lifecycle.ts:360`).

**R6**: `lock.db` (the daemon's writer-exclusivity lock, a SQLite file opened with `BEGIN IMMEDIATE`) is `chmod`ed to `0o600` immediately after opening (`daemon-lifecycle.ts:164-181`, specifically line 167). This is a **separate** file from the spool lock (`<spoolDir>/lock`); a reimplementation must not conflate the two.

### 1.3 Peer authorization model — filesystem DAC, not `SO_PEERCRED`

**R7**: The daemon does **not** authenticate RPC peers via `SO_PEERCRED`/`getsockopt`/`getpeereid` — no such call exists anywhere in `packages/core` or its consumers (verified by exhaustive grep of `SO_PEERCRED`, `peercred`, `getpeereid` across `packages/core/src`, `packages/mcp-server/src`, `packages/cli/src/commands`: zero matches). Authorization is enforced entirely by standard POSIX discretionary access control (DAC) on the filesystem path: only a process running as the owning UID (or root) can `stat`/traverse into `<dataDir>/control` (mode `0o700`, R3) to find the socket, and only the owning UID (or root) can `connect(2)` to a socket file with mode `0o600` (R4). There is no additional in-band credential exchange in the RPC protocol itself.

**R8**: When a client's `connect(2)` fails, `mapPeerConnectError` (`daemon-rpc-contract.ts:113-121`) maps the resulting `NodeJS.ErrnoException` to a `TypedRpcError`:

| `errno` code | Mapped RPC error code | `retryable` | Evidence |
|---|---|---|---|
| `EACCES` | `peer_denied` | `false` | `daemon-rpc-contract.ts:114-116` |
| `ECONNREFUSED`, `ENOENT` | `daemon_unavailable` | `true` | `daemon-rpc-contract.ts:117-119` |
| any other code | **not mapped** — the promise rejects with the original `Error`; no `TypedRpcError` is produced | — | `daemon-rpc-contract.ts:170-181` |

This mapping is purely client-side (invoked from `callDaemonRpc`'s socket `"error"` handler, `daemon-rpc-contract.ts:170-181`); the daemon process never sees or responds to these connection-level failures — they occur before any bytes are exchanged.

The handler applies the mapping to **exactly three** `errno` values: it calls `mapPeerConnectError` only for `EACCES`, `ECONNREFUSED`, and `ENOENT`, and calls `finish(peerError)` for everything else. `mapPeerConnectError`'s own trailing `peer_denied` fallback (`daemon-rpc-contract.ts:120`) is therefore **unreachable from `callDaemonRpc`** and applies only to direct calls of the exported function. Concretely: a regular file at the socket path yields `ENOTSOCK`, which surfaces as a thrown error, not `peer_denied` — a client that maps it to `peer_denied` would produce the wrong status and the wrong fail-open decision.

### 1.4 Platform preflight

**R9**: `assertSupportedStoragePlatform()` throws unless `process.platform === "linux"` (`storage-platform.ts:23-27`), called at the start of daemon startup (`daemon-lifecycle.ts:268`) and inside `ensurePrivateDirectory` (`storage-platform.ts:40`).

**R10**: `assertDataDirPreflight(dataDir)` (`storage-platform.ts:225-238`), called once at daemon startup before any storage is touched (`daemon-lifecycle.ts:269`):
1. Rejects a WSL Windows-share path: `resolve(dataDir)` matching `/^\/mnt\/[a-z](\/|$)/i` after backslash-to-slash normalization (`isWslWindowsSharePath`, `storage-platform.ts:174-177,227-229`).
2. Walks up to the nearest **existing** ancestor of `dataDir` (`existingAncestor`, `storage-platform.ts:179-187`) and rejects if `statfsSync(...).type` is one of the known network-filesystem magic numbers (`storage-platform.ts:132-146,164-167,230-233`): NFS, SMB/CIFS/SMB2, 9P, FUSE-backed (sshfs/rclone/gvfs/s3fs), virtiofs, Ceph, kAFS, AFS, Lustre, GFS2, OCFS2.
3. Additionally rejects if the mount containing that ancestor, resolved by parsing `/proc/self/mountinfo`, has a forbidden fstype string (`mountFstypeFor` + `isForbiddenMountFstype`, `storage-platform.ts:148-172,195-223,234-236`): `nfs`, `nfs4`, `cifs`, `smb3`, `smbfs`, `9p`, `drvfs`, `virtiofs`, `ceph`, `afs`, `lustre`, `gfs2`, `ocfs2`, or any fstype starting with `fuse`.

**R11**: `ensurePrivateDirectory` (R3) performs the **same** two network-filesystem checks again, per-directory, on every invocation (`storage-platform.ts:54-60`) — this is a second, redundant enforcement layer, not a distinct rule.

---

## 2. Framing

### 2.1 Newline-delimited JSON, one request per connection

**R12**: Each RPC request is a single JSON object serialized on one line, terminated by `\n` (`0x0a`). The client writes `` `${JSON.stringify(request)}\n` `` after `connect` (`daemon-rpc-contract.ts:147-149`). The daemon buffers incoming bytes per connection and looks for the first `0x0a`; everything before it is parsed as one request, everything after is retained for a subsequent request on the same connection (`daemon-rpc.ts:1640-1650`).

**R13**: In practice every client in this codebase (`callDaemonRpc`, the sole client entry point) opens exactly one connection per request and destroys the socket once the first response line is read (`daemon-rpc-contract.ts:128-186`). Server-side, the connection handler sets a `dispatching` guard (`daemon-rpc.ts:1639,1641`) that ignores further `"data"` events until the in-flight request finishes. `finish(payload)` (`daemon-rpc.ts:1612-1638`) is not unconditionally `connection.end(...)`: it returns early without writing anything if `done` is already `true` (`:1619-1622`) or if `connection.destroyed` (`:1625-1628`), and it calls `connection.destroy()` — not `.end()` — when invoked with no `payload` at all (`:1637`); only the live-socket, payload-supplied case calls `` `connection.end(`${JSON.stringify(payload)}\n`, requestStopAfterResponse)` `` (`:1630`). Every dispatch call site in this file (`dispatchDaemonRpc(...).then(finish, ...)`, `daemon-rpc.ts:1671-1673`) always passes a payload and `done` starts `false`, so in practice `finish()` does reach the `.end()` branch for every real request — the wire is effectively (and must be treated as) **one request, one response, one connection**, even though the line-buffering logic would in principle support pipelining.

**R14**: JSON key order within a request or response object is **not** contractual — both sides parse with a standard JSON parser. Only the newline-delimited line framing is contractual.

### 2.2 Request size bound

**R15**: The daemon enforces a hard **32768-byte** (`RPC_MAX_BYTES = 32 * 1024`, `daemon-rpc-contract.ts:5`) cap on the **cumulative buffered bytes of one in-flight request** (all chunks received since the last completed request on that connection, before a newline is found). If `buffer.length + chunk.length > RPC_MAX_BYTES`, the daemon immediately responds `payload_too_large` and terminates the connection, without waiting for a newline (`daemon-rpc.ts:1642-1645`). This bound applies to the **request**, not the response.

**R16**: On the response side, `callDaemonRpc` accepts an optional `maxResponseBytes`; if supplied, it is enforced by counting bytes as chunks arrive, before a newline is even seen, and finishing with a plain `Error` (not a `TypedRpcError`) if exceeded (`daemon-rpc-contract.ts:150-159`). This is **client-chosen policy, not a daemon-enforced or protocol-level bound**: the CLI hook client passes `256 * 1024` for `POST /v1/context/pack` and `RPC_MAX_BYTES` (32768) for every other method (`hook-rpc-client.ts:73,316-317`); the MCP server client (`rpc-client.ts:274-290`) passes **no** `maxResponseBytes` at all, i.e. unbounded on the client side.

### 2.3 `STOP` control frame

**R17**: A line beginning with the literal string `"STOP"` is not JSON-RPC; it is a separate control frame recognized **before** JSON parsing is attempted (`daemon-rpc.ts:1651-1661`). Format: `` `STOP ${nonce}\n` ``, where `nonce` is everything after `STOP` (index 4), trimmed.
- If `nonce` is non-empty and equals `ctx.identity.nonce`, the daemon responds `` `${JSON.stringify({ status: "stopping" })}\n` ``, calls `ctx.onStop()`, and ends the connection (`daemon-rpc.ts:1652-1656`).
- Otherwise it responds `` `${JSON.stringify({ status: "mismatch" })}\n` `` and ends the connection without stopping (`daemon-rpc.ts:1657`).
- Either way, the connection's `done` flag is set and no further RPC dispatch occurs on it (`daemon-rpc.ts:1659-1660`).

**R18**: This is how `stopDaemon()` requests a clean shutdown: it connects, writes `` `STOP ${nonce}\n` ``, and treats the connection's `"close"` event as success regardless of which of the two response bodies was returned (`daemon-lifecycle.ts:242-261,573-575`) — i.e. the caller of `stopDaemon` does not itself validate the `{status:...}` body; it relies on `ctx.onStop()` having been triggered daemon-side when the nonce matched.

### 2.4 Daemon self-restart after a successful restore

**R52**: `STOP` (R17-R18) is not the only path that triggers `ctx.onStop()`. In `attachDaemonRpc`'s `finish(payload)` (`daemon-rpc.ts:1612-1638`), immediately before the `done`/`connection.destroyed` early-return checks, the daemon evaluates:

```ts
stopAfterResponse ||=
  ctx.restoreState?.active === true &&
  "result" in payload &&
  payload.result.restartRequired === true;
```

(`daemon-rpc.ts:1613-1618`). If this is `true`, `requestStopAfterResponse()` — which calls `ctx.onStop()` exactly once, guarded by a `stopRequested` flag (`daemon-rpc.ts:1603-1607`) — fires as the completion callback of `connection.end(...)` once the response bytes have actually been flushed (`daemon-rpc.ts:1630`), or immediately on the error/already-closed paths (`daemon-rpc.ts:1608-1611,1620,1626,1634`) if the connection drops before then. Concretely: `POST /v1/backup/restore` sets `restoreState.active = true` before running the restore and — unlike the failure path, which resets it — leaves it `true` after a **successful** restore (`daemon-rpc.ts:512-525`); if the restore's result payload carries `restartRequired: true` (set at `online-backup.ts:1071,1246`), the daemon shuts itself down right after answering that response. This is a second, response-content-triggered daemon-initiated-stop mechanism, distinct from the nonce-driven `STOP` control frame of R17-R18, and it applies **only** to `POST /v1/backup/restore` responses (the only method whose result can carry `restartRequired` while `restoreState.active` is `true`). A Rust daemon that omits this self-restart observably diverges: it would stay alive after a restore the current daemon exits for.

---

## 3. Handshake

### 3.1 Envelope shape

```ts
type RpcRequest = {
  id: string;
  method: string;
  adapter_version: string;
  native_cli_version: string;
  normalized_schema_version: number;
  local_api_version: number;
  capability_hash: string;
  body?: Record<string, unknown>;
};

type RpcSuccess = { id: string; result: Record<string, unknown> };
type TypedRpcError = { error: { code: string; message: string; retryable: boolean } };
```
(`daemon-rpc-contract.ts:89-111`)

**R19**: A success response carries `id` (echoing the request's `id`); an error response is `{ error: {...} }` **with no `id` field at all** (`daemon-rpc-contract.ts:89-91,108-111`; constructed at `daemon-rpc.ts:1571-1580` vs. every `typedError(...)` call site). A reimplementation must not add an `id` to error responses.

**R20**: The only top-level fields the daemon accepts are `id, method, adapter_version, native_cli_version, normalized_schema_version, local_api_version, capability_hash, body` (`TOP_LEVEL_FIELDS`, `daemon-rpc.ts:110-119`). Any other top-level key is rejected (see §4).

### 3.2 Constants and the exact-match gate

| Constant | Value | Evidence |
|---|---|---|
| `LOCAL_API_VERSION` | `1` | `daemon-rpc-contract.ts:4` |
| `NORMALIZED_SCHEMA_VERSION` | `1` | `normalized-event.ts:7` |
| `RPC_CAPABILITY_HASH` | `de8a44532a1709090d41168514d4589a95f5023f69abca0e69cfc5d941aceba4` | computed at module load as `sha256(RPC_METHODS.join("\n")).digest("hex")` (`daemon-rpc-contract.ts:27-57,87-89`); mechanically re-derived from the exact `RPC_METHODS` array in §5 — see Rust parity requirements |

**R21**: `handshakeError(request)` (`daemon-rpc.ts:411-422`) is an **exact-equality** gate checked in this order, returning `protocol_mismatch` on the first mismatch:
1. `request.local_api_version !== LOCAL_API_VERSION` (`daemon-rpc.ts:412-414`)
2. `request.normalized_schema_version !== NORMALIZED_SCHEMA_VERSION` (`daemon-rpc.ts:415-417`)
3. `request.capability_hash !== RPC_CAPABILITY_HASH` (`daemon-rpc.ts:418-420`)

**R22**: `adapter_version` and `native_cli_version` are **never checked for a specific value** anywhere in the daemon RPC layer — only their `typeof` is checked during structural parsing (R24 step 2f). `native_cli_version` does, however, change response shape for MCP clients (§8), and is echoed back into deadline/timeout selection on the client side only (`hook-rpc-client.ts:67-70,75-78`), not validated by the daemon.

**R23**: There is no capability **negotiation** — `capability_hash` must match byte-for-byte or the request is rejected outright. Because the hash is derived from the literal `RPC_METHODS` array (join order and exact method-string spelling both matter), a Rust daemon speaking to the existing TypeScript MCP-server/CLI clients (or vice versa) must reproduce this exact array, in this exact order, to compute the same hash — see §5 for the full ordered list.

---

## 4. Dispatch order (validation precedence)

**R24**: `dispatchDaemonRpc(raw, ctx)` (`daemon-rpc.ts:1509-1596`) validates in the following strict order; the **first** failure short-circuits and its error code is returned — later checks are never reached for a request that fails an earlier one:

1. `started = (ctx.now ?? Date.now)()` — a wall-clock timestamp taken before any parsing (`daemon-rpc.ts:1513`).
2. `parseRequest(raw)` (`daemon-rpc.ts:372-409`), itself ordered:
   a. `JSON.parse` — malformed JSON → `invalid_json` (`:375-377`).
   b. Parsed value must be a non-null, non-array object → `invalid_json` (`:379-381`).
   c. Any top-level key not in `TOP_LEVEL_FIELDS` (R20) → `unknown_field`, message names **only the first** such key found by `Object.keys()` iteration order (`:383-386`).
   d. `id` must be a non-empty string → `invalid_request` (`:387-389`).
   e. `method` must be `typeof "string"` (membership in the 29-method set is **not** checked here) → `unknown_method`.
   f. `adapter_version` and `native_cli_version` must be `typeof "string"`; `normalized_schema_version` and `local_api_version` must be `typeof "number"`; `capability_hash` must be `typeof "string"` — **any** failing → `protocol_mismatch` (`:393-401`).
   g. `body`, if present, must be a non-null, non-array object → `invalid_request` (`:402-407`).
3. `handshakeError(request)` (R21) — exact-value mismatch on any of the three handshake constants → `protocol_mismatch` (`daemon-rpc.ts:1516-1517`). **This runs before method-name validity is checked**, so a request with both a bad `capability_hash` and an unrecognized `method` string reports `protocol_mismatch`, never `unknown_method`.
4. `isRpcMethod(request.method)` — `method` must be exactly one of the 29 strings in `RPC_METHODS` (case-sensitive, exact string) → `unknown_method` otherwise.
5. `deadlineMs = ctx.deadlineMs ?? rpcDeadlineForMethod(request.method)` is computed (§6) but not yet enforced (`daemon-rpc.ts:1521`).
6. `body = request.body ?? {}`; any key not in `METHOD_BODY_FIELDS[method]` (§5) → `unknown_field`, naming only the first extra key (`daemon-rpc.ts:1522-1526`).
7. For each field name in `METHOD_REQUIRED_FIELDS[method]` (§5), **in the array's declared order**, the first of these that fails short-circuits the whole request:
   a. `undefined` → `invalid_request`. `null` also fails except for the
      `POST /v1/processing-jobs/:id/doctor-retry` expected provider/manifest pair when **both**
      fields are present and NULL, which is the explicit legacy-unknown snapshot (`:1529-1540`).
      A mixed NULL/string pair fails before handler dispatch.
   b. If the field name is one of `idempotencyKey, requestId, operationId, payloadHash, reason, backupId, kind, title, body, collection, mode, context` and the value is not `typeof "string"` → `invalid_request` (`:1532-1548`). Required fields outside this explicit list (e.g. `id`, `items`, `filters`, `event`, `nonce`, `session`) receive **no type check at all** at this layer — only the presence check of step 7a.
   c. If the value is a string of length `0` → `invalid_request` (`:1549-1551`).
   d. If the field name is one of `idempotencyKey, requestId, operationId, backupId` (`PERSISTED_ID_FIELDS`, `daemon-rpc.ts:255`) and it fails `isSafePersistedText(value, 256)` → `invalid_request` "`<field> is invalid`" (`:1552-1554`). **`payloadHash` is not in `PERSISTED_ID_FIELDS`** and receives no such check here (see R31/Known gaps).
   e. If the field name is `reason` and it fails `isSafePersistedText(value, 1024)` → `invalid_request` "reason is invalid" (`:1555-1557`).
8. `elapsed = (ctx.now ?? Date.now)() - started`; if `elapsed >= deadlineMs` → `deadline_exceeded`, `retryable: true` (`daemon-rpc.ts:1559-1561`). This measures only the cost of steps 1–7 (parsing plus structural/required-field validation); it is not a timeout around the handler itself (that enforcement is the connection-level socket timeout, R17-adjacent — see §6).
9. `isMaintenanceMode(ctx) && MAINTENANCE_BLOCKED_METHODS.has(method)` → `maintenance_mode`, `retryable: true` (`daemon-rpc.ts:1563-1568`; see §7).
10. Only after all of the above passes is `handleMethod(...)` invoked (`daemon-rpc.ts:1570-1580`).

**R25**: `isSafePersistedText(value, maxBytes)` (`daemon-rpc.ts:352-362`) requires, in addition to the string/non-empty/byte-length checks: running the value through the redaction-intake pipeline (`applyDaemonIntake({ id: value }, { allowlist: ["id"] })`) and requiring `!intake.degraded && intake.sensitivity === "normal" && intake.payload.id === value` — i.e. the value must not trip secret/PII detection and must survive the intake pipeline byte-for-byte unchanged. This couples ID-shaped field validation to the redaction subsystem; the redaction rule set itself is out of scope for this document (see Known gaps / scope fence).

---

## 5. RPC method surface

**R26**: The full, ordered `RPC_METHODS` array (`daemon-rpc-contract.ts:27-55`) — order and exact spelling are part of the contract because `RPC_CAPABILITY_HASH` is derived from this exact sequence (R21, R23):

```
GET /v1/health
GET /v1/doctor
POST /v1/events
POST /v1/events/batch
POST /v1/context/pack
POST /v1/search
POST /v1/retrieval/file-context
POST /v1/retrieval/file-context/delivery
GET /v1/memories/:id
POST /v1/memories/record
DELETE /v1/memories/:id
GET /v1/checkpoints
GET /v1/view
POST /v1/viewer/auth/nonce
POST /v1/viewer/auth/exchange
POST /v1/viewer/auth/verify
POST /v1/viewer/auth/logout
GET /v1/backup/list
POST /v1/backup/create
POST /v1/backup/verify
POST /v1/backup/restore
POST /v1/operations/export
POST /v1/operations/import
GET /v1/operations/:id
POST /v1/jobs
GET /v1/jobs
GET /v1/jobs/:id
GET /v1/processing-jobs/:id
POST /v1/processing-jobs/:id/doctor-retry
```

### 5.1 Body allow-lists and required fields

**R27**: `METHOD_BODY_FIELDS` (`daemon-rpc.ts:121-188`) is the daemon's per-method allow-list — any body key outside it is rejected with `unknown_field` (§4 step 6). `METHOD_REQUIRED_FIELDS` (`daemon-rpc.ts:190-218`) is the subset that must be present and non-empty (§4 step 7), except for the jointly-NULL legacy-unknown doctor fingerprint pair defined by R24/R58. Every field in the required set is necessarily also in the allowed set.

| Method | Body allow-list (`METHOD_BODY_FIELDS`) | Required (`METHOD_REQUIRED_FIELDS`) | Maintenance-blocked | Deadline |
|---|---|---|---|---|
| `GET /v1/health` | *(none)* | *(none)* | no | 2000 ms |
| `GET /v1/doctor` | *(none)* | *(none)* | no | 2000 ms |
| `POST /v1/events` | `idempotencyKey, event, adapterRedaction` | `idempotencyKey, event` | **yes** | 2000 ms |
| `POST /v1/events/batch` | `items` | `items` | **yes** | 2000 ms |
| `POST /v1/context/pack` | `requestId, context, limit, tokenBudget, filters, trace` | `requestId, context` | **yes** | 2000 ms |
| `POST /v1/search` | `requestId, mode, query, repositoryPath, ids, memoryId, depthBefore, depthAfter, includePackContext, filters, limit` | `requestId, mode` | **yes** | 2000 ms |
| `POST /v1/retrieval/file-context` | `attemptId, startedAt, completedAt, retrievalStatus, candidateIds, candidateCount, selectedIds, failureCode, failureStage, project, repositoryPath, sourceSessionId` | `attemptId, startedAt, completedAt, retrievalStatus` | **yes** | 2000 ms |
| `POST /v1/retrieval/file-context/delivery` | `attemptId, status` | `attemptId, status` | **yes** | 2000 ms |
| `GET /v1/memories/:id` | `id, requestId, project, kind` | `id, requestId` | **yes** | 2000 ms |
| `POST /v1/memories/record` | `idempotencyKey, kind, title, body, confidence, project, adapterRedaction` | `idempotencyKey, kind, title, body` | **yes** | 2000 ms |
| `DELETE /v1/memories/:id` | `id, requestId, expectedRevision` | `id, requestId` | **yes** | 2000 ms |
| `GET /v1/checkpoints` | `project, state, limit` | *(none)* | no | 2000 ms |
| `GET /v1/view` | `collection, sessionId, project, kind, scope, limit, offset` | `collection` | no | 2000 ms |
| `POST /v1/viewer/auth/nonce` | *(none)* | *(none)* | no | 2000 ms |
| `POST /v1/viewer/auth/exchange` | `nonce` | `nonce` | no | 2000 ms |
| `POST /v1/viewer/auth/verify` | `bearer, session` | *(none)* | no | 2000 ms |
| `POST /v1/viewer/auth/logout` | `session` | `session` | no | 2000 ms |
| `GET /v1/backup/list` | *(none)* | *(none)* | no | 1,800,000 ms |
| `POST /v1/backup/create` | `operationId, payloadHash, reason` | `operationId, payloadHash, reason` | **yes** | 1,800,000 ms |
| `POST /v1/backup/verify` | `backupId` | `backupId` | no | 1,800,000 ms |
| `POST /v1/backup/restore` | `operationId, payloadHash, backupId` | `operationId, payloadHash, backupId` | **yes** | 1,800,000 ms |
| `POST /v1/operations/export` | `operationId, payloadHash, outputPath, filters` | `operationId, payloadHash, outputPath, filters` | **yes** | 2000 ms |
| `POST /v1/operations/import` | `operationId, payloadHash, inputPath, remapProject, dryRun` | `operationId, payloadHash, inputPath` | **yes** | 2000 ms |
| `GET /v1/operations/:id` | `id` | `id` | no | 2000 ms |
| `POST /v1/jobs` | `kind, args, dryRun` | `kind` | **yes** | 2000 ms |
| `GET /v1/jobs` | `kind, state, submittedAfter` | *(none)* | no | 2000 ms |
| `GET /v1/jobs/:id` | `id` | `id` | no | 2000 ms |
| `GET /v1/processing-jobs/:id` | `id` | `id` | no | 2000 ms |
| `POST /v1/processing-jobs/:id/doctor-retry` | `id, producerReceiptId, expectedRole, expectedProviderFingerprint, expectedManifestFingerprint, expectedAttemptCount, expectedClaimGeneration` | all listed fields; expected fingerprint pair may be jointly NULL | **yes** | 2000 ms |

Health responses add one bounded `capability` object frozen at daemon startup. Doctor returns the
same object at `diagnostics.capability`. It contains only the safe manifest/provider identity,
runtime reason, feature gates, and explicit schema/pack pending codes; credential values are never
included. No runtime handler rereads legacy provider config or environment to build this projection.

(Body allow-lists/required fields: `daemon-rpc.ts:121-218`. Maintenance-blocked set: `daemon-rpc.ts:220-235`, see §7. Deadlines: `daemon-rpc-contract.ts:59-68`, see §6.)

### 5.2 Method-specific notes visible at the RPC layer

**R28**: `id` is **not** in the explicit string-typed-field list of §4 step 7b, so the top-level gate only checks it is present (not `undefined`/`null`). Memory get/delete and both processing-job methods call `requirePositiveInt(value)`, which accepts either a positive-integer JSON number or a string matching `/^[1-9][0-9]*$/`. Operations and daemon-job lookups instead validate their own bounded string IDs inside their services. A reimplementation must route by method rather than assume one `:id` shape.

**R29**: `POST /v1/events/batch`: `items` must be an array of 1–200 entries (`daemon-rpc.ts:773-778`); each entry allows only `idempotencyKey, event` (`:781-783`), `idempotencyKey` must be a non-empty string passing `isSafePersistedText(..., 256)` (`:785-790`), and `event` must be an object (`:791`, via `asObject`). Each item is dispatched through the same logic as `POST /v1/events`; a `MutationConflictError` on one item is caught **per item** and reported as `{ receiptId, status: "conflict" }` in that item's slot (`:804-809`) without failing the batch — but any **other** exception thrown while processing an item propagates uncaught out of `handleEventBatch`, becoming a single top-level RPC error for the whole request even though earlier items in the array may already have been durably persisted (`:794-812`; see Known gaps).

**R30**: `GET /v1/view`: `collection` must be one of a fixed 15-value set (`VIEW_COLLECTIONS`, `daemon-rpc.ts:237-253`): `sessions, projects, memories, observations, summaries, session, memory, artifacts, raw-events, raw-events-status, stats, runtime, usage, observer-status, config`. `limit` (if present) must be an integer in `[1, 1000]`; `offset` in `[0, 1000000]`; `sessionId` (if present) is validated with the same dual-type `requirePositiveInt` as R28; `project`/`kind` (if present) must each be a string of at most 4096 UTF-8 bytes; `scope` (if present) must be exactly `"mine"` or `"theirs"` (`daemon-rpc.ts:1385-1408`). The actual view content beyond this envelope is produced by `ctx.viewerRead(body)` (`:1409`), a component out of scope for this document — treat its result shape as opaque pass-through.

**R31**: `payloadHash` — required for `POST /v1/backup/create`, `POST /v1/backup/restore`, `POST /v1/operations/export`, `POST /v1/operations/import` — is validated at **two different layers with two different rules**:
- The generic RPC-gate loop (§4 step 7) only checks it is a non-empty string (via the explicit-field-list check, R24 step 7b) — it is **not** a member of `PERSISTED_ID_FIELDS` and so is **not** subject to `isSafePersistedText`'s 256-byte cap or redaction-intake check (`daemon-rpc.ts:255,1532-1554`).
- The operation-specific handler (`DaemonOperationService.submit`, `daemon-operations.ts:379-397`) separately requires it to be a `boundedString(..., 64)` (≤ 64 bytes, no NUL, non-empty; `daemon-operations.ts:92-102`) matching `/^[a-f0-9]{64}$/` exactly (`daemon-operations.ts:88,387-389`), **and** requires it to equal the daemon's own recomputation of the hash from the rest of the request body (`operationPayloadHash(kind, request)`, `daemon-operations.ts:189-195,392-397`) — `backupPayloadHash(reason)` for `backup-create` (sha256 hex of `reason`, `online-backup.ts:148-150`), `restorePayloadHash(backupId)` for `backup-restore`, or `hashMutationPayload(request)` for `export`/`import`. A client-supplied `payloadHash` that is syntactically a 64-hex-char string but does not match the server's recomputation is rejected as `invalid_request` "payloadHash does not match the operation request." — the field functions as **request-integrity binding**, not a client-opaque token.

**R32**: `POST /v1/jobs`: `kind` must be one of a fixed 28-value set (`JOB_KINDS`, `daemon-jobs.ts:56-85`); `args` allow-list is kind-specific (`JOB_ARGS`, `daemon-jobs.ts:86-127`; per-kind argument schemas are out of scope for this document — see scope fence below). `dryRun`, if `true`, is rejected with `invalid_request` for any kind in `MAINTENANCE_JOB_KINDS` (`daemon-jobs.ts:128-147`) that is **not** also in `DRY_RUN_JOB_KINDS` (`daemon-jobs.ts:158-170`) (`daemon-jobs.ts:552-554`).

**R33**: `GET /v1/jobs`: `kind` (if present) must be one of `JOB_KINDS`; `state` (if present) must be one of `queued, running, completed, failed` (`JOB_STATES`, `daemon-jobs.ts:55`); `submittedAfter` (if present) must be a string ≤ 64 chars parseable by `Date.parse` (`daemon-jobs.ts:576-605`).

**R34**: `GET /v1/checkpoints` — see Known gaps: the body allow-list (`project, state, limit`) accepts and structurally passes those fields through the RPC gate, but the handler ignores the body entirely and unconditionally returns `{ checkpoints: [] }` (`daemon-rpc.ts:567-569`). No value-level validation of `project`/`state`/`limit` occurs for this method at any layer.

**R53**: The `filters` object accepted by `POST /v1/context/pack` and `POST /v1/search` (§5.1 lists it as an allowed body key for both, without specifying its shape) has its own sub-schema, enforced by `parseMemoryFilters(value)` (`daemon-rpc.ts:1435-1475`, called from `handlePack` at `:1107` and `handleSearch` at `:1272`; a third call site at `:1361` inside `GET /v1/memories/:id`'s handler builds an internal filters object from `project`/`kind` and is not part of the client-supplied `filters` key documented here):
- Any key not in the 29-key `FILTER_FIELDS` allow-list (`daemon-rpc.ts:257-287`: `kind, session_id, since, working_set_paths, project, scope_id, include_scope_ids, exclude_scope_ids, visibility, include_visibility, exclude_visibility, include_workspace_ids, exclude_workspace_ids, include_workspace_kinds, exclude_workspace_kinds, include_actor_ids, exclude_actor_ids, include_trust_states, exclude_trust_states, ownership_scope, personal_first, trust_bias, widen_shared_when_weak, widen_shared_min_personal_results, widen_shared_min_personal_score, widen_project_when_weak, widen_project_min_results, widen_project_min_score, widen_project_max_results`) is rejected via `RpcRequestError`'s default code, i.e. **`invalid_request`** (`daemon-rpc.ts:1438-1439`) — **not** `unknown_field`, unlike an unrecognized top-level body key (§4 step 6).
- Each present key is then checked against its typed group (`daemon-rpc.ts:288-316`, all `invalid_request` on mismatch): `STRING_FILTER_FIELDS` (`kind, since, project, ownership_scope, trust_bias`) must be a string; `STRING_ARRAY_FILTER_FIELDS` (`working_set_paths, include_scope_ids, exclude_scope_ids, include_visibility, exclude_visibility, include_workspace_ids, exclude_workspace_ids, include_workspace_kinds, exclude_workspace_kinds, include_actor_ids, exclude_actor_ids, include_trust_states, exclude_trust_states`) must be an array of strings; `STRING_OR_ARRAY_FILTER_FIELDS` (`scope_id, visibility`) must be a string or an array of strings; `BOOLEAN_OR_STRING_FILTER_FIELDS` (`personal_first, widen_shared_when_weak, widen_project_when_weak`) must be a boolean or a string; `NUMBER_FILTER_FIELDS` (`widen_shared_min_personal_results, widen_shared_min_personal_score, widen_project_min_results, widen_project_min_score, widen_project_max_results`) must be a finite number. `session_id`, the one `FILTER_FIELDS` key in none of the five groups, is checked separately and must be a positive integer (`daemon-rpc.ts:1467-1472`).

**R54**: `POST /v1/memories/record`'s `kind` field is checked against a fixed 8-value enum via `validateMemoryKind(String(body.kind))` (`packages/core/src/memory-kinds.ts:1-10` for the set, `:12-20` for the function, called at `daemon-rpc.ts:821`): `discovery, change, feature, bugfix, refactor, decision, exploration, session_summary` — the value is `trim()`ed and lowercased before comparison, and rejected as `invalid_request` "kind is unsupported." otherwise (`daemon-rpc.ts:820-823`). `confidence` (default `0.5` when absent, `daemon-rpc.ts:848`) is bounds-checked to a finite number in `[0, 1]` and rejected as `invalid_request` "confidence must be a number between 0 and 1." otherwise (`daemon-rpc.ts:849-855`). Neither rule appears in §5.1's body allow-list table, though both `kind` and `confidence` are listed there as body fields for this method.

**R55**: `POST /v1/search`'s `mode` field is checked against a fixed 8-value enum (`daemon-rpc.ts:1280-1288`): `search, search_index, find_by_file, recent, timeline, get_many, explain, expand` — rejected as `invalid_request` "Unsupported search mode: `<mode>`" otherwise (`:1290`). Per-mode required-field rules follow (`:1291-1298`): `query` is required for `search`/`search_index`; `repositoryPath` (validated as a safe repository-relative path) is required for `find_by_file`; `ids` (non-empty) is required for `get_many`/`expand`. §5.1 only states `mode` must be a non-empty string; this rule is the normative mode enum and per-mode requirements. R44 (§8) separately lists 7 of these 8 modes when mapping search modes to MCP retrieval surfaces (it omits `find_by_file`, which has no MCP surface) — that list is scoped to §8's MCP-surface mapping and must not be read as the mode enum; this rule (R55) is the normative one.

**R56**: `POST /v1/retrieval/file-context` and `POST /v1/retrieval/file-context/delivery` have handler-level value rules beyond the presence checks of §4 step 7: `attemptId`, on both methods, must match `FILE_CONTEXT_ATTEMPT_ID`, a case-insensitive UUID-shaped pattern (`/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i`, `daemon-rpc.ts:1144-1145`), or it is rejected as `invalid_request` "attemptId is invalid." (`:1183-1184`, `:1250-1251`). For `POST /v1/retrieval/file-context`: `retrievalStatus` must be one of `succeeded, no_results, skipped, failed` (`FILE_CONTEXT_RETRIEVAL_STATUSES`, `:1146`); `completedAt` must not precede `startedAt` (`:1193`); `candidateIds` and `selectedIds` are each parsed as arrays of at most 200 positive integers (`parseIds`, `:1491-1497`); `candidateCount` is bounded to `[0, 200]`, defaulting to `candidateIds.length` when absent (`parseBoundedInteger`, `:1196-1202`). For `POST /v1/retrieval/file-context/delivery`: `status` must be exactly `"handed_off"` or `"failed"`, or it is rejected as `invalid_request` "status is invalid." (`:1253-1255`). None of this appears in §5, though both methods and their body fields are listed in the §5.1 table.

**R57**: Direct capture RPC admission is capped at two in-flight requests. Each `POST /v1/events` or `POST /v1/events/batch` request owns one slot; a validated batch keeps that one slot for its whole bounded, synchronous max-200 persistence loop. When `captureInFlight >= 2`, the daemon returns `capture_saturated` with `retryable: true` before any event write by the rejected request; clients may spool and retry. The counter is decremented in `finally`. Batch handling preserves R29/G2: saturation is a non-conflict exception and therefore becomes one top-level batch error, not a per-row receipt.

**R58**: `GET /v1/processing-jobs/:id` returns the bounded doctor projection or `not_found`, including a `retryTarget` from the daemon's frozen active manifest or NULL when no target is active. `POST /v1/processing-jobs/:id/doctor-retry` requires role `summary`, the displayed provider/manifest attempt pair, attempt count, claim generation, and a producer receipt ID. The pair may both be NULL only for an honest legacy-unknown attempt; mixed NULL/non-NULL is invalid. The server reuses its frozen `retryTarget` rather than trusting a client-supplied target. A replay of an already-recorded `(job, producerReceiptId)` returns the durable signal as `duplicate` before mutable attempt/grant snapshot checks, including after the grant is consumed. A new producer receipt accepts only the displayed current snapshot: stale confirmation returns `stale_snapshot`, while a different signal arriving during an already-pending grant returns retryable `grant_pending`. The method is maintenance-blocked.

---

## 6. Deadlines

**R35**: `rpcDeadlineForMethod(method)` (`daemon-rpc-contract.ts:66-68`) returns:
- **1,800,000 ms** (30 minutes; `RPC_BACKUP_DEADLINE_MS = 30 * 60 * 1_000`, `daemon-rpc-contract.ts:7`) if `method` is one of `GET /v1/backup/list, POST /v1/backup/create, POST /v1/backup/verify, POST /v1/backup/restore` (`BACKUP_RPC_METHODS`, `daemon-rpc-contract.ts:59-64`).
- **2000 ms** (`RPC_DEFAULT_DEADLINE_MS`, `daemon-rpc-contract.ts:6`) for every other method.

**R36**: This value is used in two independent ways:
1. As the **elapsed-time budget** checked once, in-process, after structural/required-field validation and before dispatch (§4 step 8) — this only measures request-parsing cost, not handler execution time.
2. As the **socket idle/response timeout** (`connection.setTimeout(...)`, `daemon-rpc.ts:1675-1677` at connection-attach time using the daemon's configured default, then re-armed at `daemon-rpc.ts:1663-1670` to the specific method's deadline once the method is parseable from the raw JSON line — this re-arming is a best-effort `JSON.parse` attempt that is separate from, and precedes, the canonical `parseRequest` call). If the socket timeout fires before a response is written, the daemon responds `deadline_exceeded` (`retryable: true`) and destroys the connection (`daemon-rpc.ts:1675-1677`).

**R37**: `ctx.deadlineMs`, when set on the `DaemonRpcContext` (an optional constructor-time override, not client-controlled per-request), overrides `rpcDeadlineForMethod(...)` for **every** method uniformly (`daemon-rpc.ts:1521,1675`) — there is no per-request client-supplied deadline field in `RpcRequest`.

**R38**: Client-side, `callDaemonRpc` and the hook/MCP clients independently choose their own `timeoutMs` (typically also `rpcDeadlineForMethod(method)`, `rpc-client.ts:269`; or the smaller of a hook-agent budget and remaining deadline, `hook-rpc-client.ts:295-300`) and wrap the call in an `AbortSignal.timeout(...)` — this is client policy layered on top of, not a substitute for, the daemon's own socket timeout.

---

## 7. Maintenance-mode blocking

**R39**: `isMaintenanceMode(ctx)` is `true` iff `ctx.jobs.isMaintenanceMode()` **or** `ctx.restoreState?.active === true` (`daemon-rpc.ts:348-350`). `DaemonJobService.isMaintenanceMode()` (`daemon-jobs.ts:531-533`) reflects an internal flag set to `true` for the duration of `runInMaintenance(...)` (`daemon-jobs.ts:782-794`), which wraps execution of any job whose `kind` is in `MAINTENANCE_JOB_KINDS` (18 kinds, `daemon-jobs.ts:128-147`). `restoreState.active` is set by the daemon while `POST /v1/backup/restore` is in flight (`daemon-rpc.ts:512-525`) — and, on a **successful** restore, is left `true` rather than reset, which is what makes the self-restart of R52 (§2.4) possible.

**R40**: `MAINTENANCE_BLOCKED_METHODS` contains exactly **15** methods:

```
POST /v1/events
POST /v1/events/batch
POST /v1/context/pack
POST /v1/search
POST /v1/retrieval/file-context
POST /v1/retrieval/file-context/delivery
GET /v1/memories/:id
POST /v1/memories/record
DELETE /v1/memories/:id
POST /v1/backup/create
POST /v1/backup/restore
POST /v1/operations/export
POST /v1/operations/import
POST /v1/jobs
POST /v1/processing-jobs/:id/doctor-retry
```

**R41**: When `isMaintenanceMode(ctx)` is true and `request.method` is in this set, the daemon returns `maintenance_mode` (`retryable: true`) **before** invoking the method's handler (§4 step 9, after required-field validation but before deadline-adjacent handler dispatch) — the check runs on every request to a blocked method regardless of whether that request would otherwise have succeeded.

**R42**: The remaining 14 methods, including `GET /v1/processing-jobs/:id`, remain servable during maintenance mode.

**R43**: `dispatchSpoolMutation` (`daemon-rpc.ts:881-897`), used by the daemon's own periodic spool-sweep to internally replay queued `POST /v1/events`/`POST /v1/memories/record` mutations (`daemon-lifecycle.ts:329-336,358,362-363`), independently throws a plain `Error("maintenance mode is active")` if `isMaintenanceMode(ctx)` at call time (`daemon-rpc.ts:885`) — the sweep itself is also suppressed at the call site while maintenance is active or a restore is in flight (`daemon-lifecycle.ts:330`). This is internal daemon behavior, not a distinct RPC error path, but it means queued mutations do not drain during maintenance.

---

## 8. MCP retrieval-surface augmentation (`native_cli_version === "mcp-stdio"`)

**R44**: `mcpRetrievalSurface(method, body, nativeCliVersion)` (`daemon-rpc.ts:937-946`) returns a non-null `RetrievalSurface` **only** when `nativeCliVersion === "mcp-stdio"` (the literal value the MCP server client sends as `native_cli_version`, `rpc-client.ts:280`) and `method` is one of `POST /v1/context/pack` (→ `"mcp_pack"`), `GET /v1/memories/:id` (→ `"mcp_get"`), or `POST /v1/search` (→ looked up from `body.mode` via `MCP_SEARCH_SURFACES`, `daemon-rpc.ts:927-935`: `search→mcp_search, search_index→mcp_search_index, recent→mcp_recent, timeline→mcp_timeline, get_many→mcp_get_observations, explain→mcp_explain, expand→mcp_expand`; any other `mode` value yields `null`). This 7-entry mapping is scoped to the MCP-surface lookup and omits `find_by_file` (which has no MCP surface); it is not the normative `mode` enum — that is R55 (§5.2).

**R45**: When a non-null surface applies, `handleRetrievalRpc` (`daemon-rpc.ts:1085-1104`) wraps the handler call: it records a retrieval-surface ledger entry (success/failure/no-results, timing, candidate/selected memory IDs extracted per-surface by `mcpRetrievalResult`, `daemon-rpc.ts:965-1001`), and — **only if the recording succeeded and at least one memory ID was returned** — adds a `retrievalAttemptId` field to the **result** object (`daemon-rpc.ts:1100-1103`). This field is not present for non-MCP clients (any other `native_cli_version` value) and not present when ledger recording fails or returns zero IDs.

**R46**: `retrievalAttemptId` is deterministic: `sha256(` `${surface}\0${requestId}` `)` hex digest, reformatted into UUID-v4-like dashes with fixed version/variant nibbles (`mcpAttemptId`, `daemon-rpc.ts:1003-1011`) — same `surface` + same `requestId` always yields the same `retrievalAttemptId`, independent of wall-clock time or result content.

**R47**: The MCP client strips `retrievalAttemptId` out of the returned result before handing it to its caller, and instead exposes a `finalizeDelivery(status)` closure that, when invoked, sends it back via `POST /v1/retrieval/file-context/delivery` with `{ attemptId: retrievalAttemptId, status }` (`rpc-client.ts:303-323`). A reimplementation of the **daemon** must reproduce the `retrievalAttemptId` derivation exactly (R46) for this round trip to function; a reimplementation of the **client** must reproduce the strip-and-finalize behavior of R47 to avoid leaking the field to tool callers.

---

## 9. Typed error envelope

**R48**: Every error response is `{ error: { code: string; message: string; retryable: boolean } }` (`daemon-rpc-contract.ts:89-95`), never carrying `id` (R19). `retryable` defaults to `false` whenever a caller of `typedError(code, message)` omits the third argument.

**R49**: Codes observed in the daemon RPC layer, with their `retryable` value as constructed in source:

| Code | `retryable` | Origin | Evidence |
|---|---|---|---|
| `invalid_json` | `false` | malformed/non-object request | `daemon-rpc.ts:377,380` |
| `unknown_field` | `false` | extra top-level or body key | `daemon-rpc.ts:385,1525` |
| `invalid_request` | `false` | missing/mistyped/oversized required field; also `RpcRequestError`'s default `code` for any application-level validation error not otherwise coded | `daemon-rpc.ts:318-326,388,406,1530,1547,1550,1553,1556` |
| `unknown_method` | `false` | `method` not typed as a string, or not one of the 29 `RPC_METHODS` | `daemon-rpc.ts` |
| `protocol_mismatch` | `false` | handshake field mistyped, or exact-value mismatch (§3.2) | `daemon-rpc.ts:400,413,416,419` |
| `payload_too_large` | `false` | cumulative request bytes exceed `RPC_MAX_BYTES` | `daemon-rpc.ts:1643` |
| `deadline_exceeded` | **`true`** | in-process elapsed budget exceeded (§4 step 8), or socket timeout fired (§6) | `daemon-rpc.ts:1561,1676` |
| `maintenance_mode` | **`true`** | blocked method while maintenance/restore active (§7) | `daemon-rpc.ts:1565` |
| `capture_saturated` | **`true`** | two direct singular/batch capture RPCs already own the request-level slots; no write occurs for the rejected request | `daemon-rpc.ts` |
| `grant_pending` | **`true`** | the processing job already owns an unconsumed resume grant from a different producer receipt | `ProcessingResumeError` (`store.ts`) |
| `stale_snapshot` | `false` | doctor retry confirmation no longer matches the displayed job attempt | `ProcessingResumeError` (`store.ts`) |
| `invalid_signal` | `false` | resume signal fails its closed shape/identity validation | `ProcessingResumeError` (`store.ts`) |
| `internal_error` | `false` | any thrown error not matching a known error class; or the socket-handler's own dispatch-promise rejection | `daemon-rpc.ts:1594,1672` |
| `not_found` | `false` | e.g. `DELETE /v1/memories/:id` for a nonexistent id (`RpcRequestError` with explicit code); also a `DaemonOperationRequestError`/`BackupRequestError` code | `daemon-rpc.ts:913`, `daemon-operations.ts:74-84`, `online-backup.ts:60` |
| `idempotency_conflict` | `false` | `MutationConflictError`'s fixed `code` property; also a `DaemonOperationRequestError` code (export/import operation-id reuse) | `mutation-dispatcher.ts:13-14`, `daemon-operations.ts:74-84,417` |
| `conflict` | `false` | `DaemonOperationRequestError`/`BackupRequestError` code (e.g. backup-id reuse with different payload, restore vs. active work) | `daemon-operations.ts:74-84,414-419`, `online-backup.ts:60`, `daemon-rpc.ts:516` |
| `peer_denied` | `false` | client-side connect failure mapping (R8), `EACCES` only — never produced by the daemon itself | `daemon-rpc-contract.ts:115`, `170-181` |
| `daemon_unavailable` | **`true`** | client-side connect failure mapping (R8) — never produced by the daemon itself | `daemon-rpc-contract.ts:118` |

**R50**: `dispatchDaemonRpc` preserves the explicit retryable bit from `RpcRequestError` and `ProcessingResumeError`; this is what makes `capture_saturated` and `grant_pending` retryable. Backup, operation, mutation-conflict, stale-snapshot, invalid-signal, and ordinary validation errors omit or set no retryable bit and therefore remain `false`.

**R51**: `retryable` is load-bearing for client behavior, not merely informational: the MCP client's `requestWithSpool` only falls back to the on-disk spool when `response.error.retryable` is `true` (`rpc-client.ts:366-367`). A reimplementation that flips a code's `retryable` value (in either direction) silently changes whether that class of failure gets spooled-and-retried or surfaced immediately to the tool caller.

---

## 10. Known gaps (documented, not fixed)

**G1 — `GET /v1/checkpoints` allow-lists fields it never reads.** The RPC gate accepts and structurally passes `project, state, limit` in the body (no required fields), but the handler ignores `body` entirely and always returns `{ checkpoints: [] }` (`daemon-rpc.ts:567-569`). No type or value validation of `project`/`state`/`limit` occurs anywhere for this method. **Must preserve**: this is the current wire-observable behavior — a reimplementation must accept those three body keys (rejecting any other key with `unknown_field`) and must return `{ checkpoints: [] }` unconditionally, exactly as today. A reimplementation must **not** attempt to infer or invent real checkpoint semantics from the field names.

**G2 — `POST /v1/events/batch` is not atomic and does not fully report partial failure.** A per-item `MutationConflictError` is caught and reported as `{ receiptId, status: "conflict" }` in that item's slot (R29), but any other exception thrown while processing item *k* propagates uncaught, producing a single top-level RPC error for the entire request — even though items `0..k-1` may already have been durably committed via their own `dispatchClassA` calls (`daemon-rpc.ts:794-812`). The client receives no indication of which prefix of the batch succeeded. **Must preserve** the non-atomic commit-then-fail behavior and the top-level-error-on-non-conflict-exception shape (a reimplementation must not silently make batches atomic, since existing clients build correctness on the current partial-commit-then-error semantics via idempotency keys and retries). A reimplementation **may not rely on** the batch response ever reporting which specific items committed before a mid-batch failure — that information is not communicated by the current protocol at all.

**G3 — `payloadHash` has no protocol-layer safety net; its only real validation is deep inside the operation handler.** The generic RPC-gate required-field check (§4 step 7b) accepts any non-empty string for `payloadHash`, without the 256-byte cap or redaction-intake safety check applied to `idempotencyKey`/`requestId`/`operationId`/`backupId` (R31). The real shape constraint (exactly 64 lowercase hex chars, and exact equality with the server's own recomputed hash of the rest of the body) lives entirely in `daemon-operations.ts:387-397`, reachable only for the four operation-submitting methods. **Must preserve**: a reimplementation must apply the same two-layer split — a permissive top-level gate (any non-empty string) and a strict, per-operation-kind recomputation-and-equality check inside the operations service — because clients (e.g. `rpc-client.ts:169-205`) compute `payloadHash` client-side before sending and expect a mismatch to be rejected as `invalid_request`, not silently accepted. A reimplementation **may not rely on** the top-level gate alone to bound or sanitize `payloadHash` — it provides no such guarantee today.

**G4 — `adapter_version` and `native_cli_version` are structurally required but never validated against any known-good value.** Only their `typeof` is checked (R22). `native_cli_version` does have one observable effect (`native_cli_version === "mcp-stdio"` toggles the MCP retrieval-ledger behavior of §8), but no other string comparison against it appears in `daemon-rpc.ts`, `daemon-rpc-contract.ts`, or `daemon-lifecycle.ts`. **Must preserve**: the exact-string check `=== "mcp-stdio"` for §8's behavior. A reimplementation **may not rely on** any other value of `native_cli_version` (or on `adapter_version` at all) triggering daemon-side behavior — none currently does, and inventing a check would be a new capability, not a preserved one.

**G5 — the `not_implemented` fallback in `handleMethod` is unreachable given the current 29-method set.** Every registered `RPC_METHODS` string has an explicit branch, and `isRpcMethod(request.method)` has already rejected any string outside that set before `handleMethod` runs. **A reimplementation must not rely on this fallback being reachable or on its shape** — it is dead code under the current method set, not a documented "unimplemented method" response contract.

**G6 — ID-shaped fields undergo secret/PII redaction-intake checks that are out of this document's scope.** `isSafePersistedText` (R25) runs `idempotencyKey`, `requestId`, `operationId`, `backupId`, and `reason` (and the top-level `id` on `POST /v1/events/batch` items) through `applyDaemonIntake`, the same redaction pipeline used on user content, and rejects values that trip secret/PII detection or get mutated by it. The specific detection rules (`secret_rules_version`, pattern set) are a separate subsystem not covered here. **Must preserve** the coupling itself — that these specific fields are subject to *some* redaction-intake pass and can be rejected as `invalid_request` on that basis, not just on shape — but this document does **not** specify the detection rules; a reimplementation needs the redaction/secret-detection contract (out of scope here) to reproduce which specific strings get rejected.

---

## Scope fence

The following are deliberately **not** specified by this document, though the RPC layer depends on or touches them:

- **Normalized event schema** — `validateNormalizedEvent(event, normalizedSchemaVersion)` is invoked for `POST /v1/events` (`daemon-rpc.ts:681,696`) and by both clients before sending; its field-level schema is a separate contract.
- **Redaction/secret-detection rules** — `applyDaemonIntake`/`preprocessAdapterEvent` and the `secret_rules_version` versioning scheme (referenced throughout §4, §5, G6) are a separate subsystem.
- **`viewerRead` (`GET /v1/view`) result content** beyond the envelope validated in R30 — treated as opaque pass-through.
- **Per-job-kind `args` schemas** for `POST /v1/jobs` — only the `kind` allow-list and the `dryRun`/maintenance interaction (R32) are specified; the 27 individual argument shapes in `JOB_ARGS` (`daemon-jobs.ts:86-127`) are not enumerated here.
- **`collectOperationalStatus(...)`** content inside `GET /v1/doctor`'s `diagnostics.operationalStatus` — cited as present (`daemon-rpc.ts:449-452,490`) but not itself specified.
- **The spool on-disk format** and its daemon-side sweep — covered by the sibling contract `spool-format-v1.md` in this directory.

---

## Rust parity requirements

### Must reproduce exactly (byte-for-byte or bit-for-bit observable)

1. Socket path derivation (R1–R2) and file/directory permission modes (R3–R6).
2. The absence of any peer-credential check beyond filesystem DAC (R7) — a Rust daemon must not add `SO_PEERCRED` (or equivalent) checks that the current daemon does not perform, since that would reject connections the current daemon accepts.
3. Newline-delimited JSON framing, one-response-per-connection behavior (R12–R14), the `STOP <nonce>` control frame and its two exact response bodies (R17–R18).
4. The 32768-byte request-size bound and the exact `payload_too_large` trigger condition (R15).
5. The handshake envelope's field set (R20), the exact-match semantics of `local_api_version`, `normalized_schema_version`, `capability_hash` (R21), and the constants `LOCAL_API_VERSION = 1`, `NORMALIZED_SCHEMA_VERSION = 1`, `RPC_CAPABILITY_HASH = de8a44532a1709090d41168514d4589a95f5023f69abca0e69cfc5d941aceba4` (R21, R26) — the hash specifically requires reproducing the 29-entry `RPC_METHODS` array in the exact order and spelling given in §5, joined with `\n`, sha256-hex-digested, for any two implementations (TS↔Rust) to interoperate.
6. The full validation precedence chain of §4 (R24) — which error code a malformed request receives depends on this exact order, and clients (and tests) may depend on receiving a specific code for a specific class of malformed input.
7. Every method's body allow-list and required-field set (§5.1 table, R27), including the field-name-specific type/safety checks of R24 step 7b–7e.
8. Deadline values and their dual application as an in-process elapsed budget and a socket timeout (R35–R37).
9. The 15-method maintenance-blocked set and the exact condition under which it applies (R39–R41).
10. The daemon self-restart-after-response condition of R52 (§2.4) — a Rust daemon must shut itself down after flushing a `POST /v1/backup/restore` response whose result carries `restartRequired: true` while a restore is active, or it observably diverges by staying alive when the current daemon exits.
11. The MCP `retrievalAttemptId` derivation (R46) and the conditions under which it is added to a result (R44–R45), for interoperability with the existing TypeScript MCP-server client's `finalizeDelivery` flow.
12. The error envelope shape — no `id` on errors (R19) — and every `code`/`retryable` pairing in R49, since `retryable` is load-bearing for client spool behavior (R51).
13. All behaviors listed as "must preserve" under Known gaps (G1–G6).

### Free to differ

- Internal code organization, the specific TypeScript error classes (`RpcRequestError`, `BackupRequestError`, `DaemonOperationRequestError`, `MutationConflictError`) — only their externally observable `code`/`message`/`retryable` triple (R49) is contractual, not the class hierarchy.
- The `message` string of any error, beyond it being a non-empty, human-readable string — no client in this codebase pattern-matches on error `message` text, only on `error.code` and `error.retryable`.
- Performance characteristics not covered by the deadline values themselves (e.g. actual handler latency under the 2000 ms/1,800,000 ms budgets, as long as the budgets are enforced per §6).
- The internal implementation of `isSafePersistedText`'s redaction-intake call (R25) — as long as the observable accept/reject boundary for the specific strings the existing test/production corpus exercises is preserved; the underlying secret-detection rule engine is out of this document's scope (see Scope fence).
- Whether the daemon process is written to support pipelining multiple requests per connection — since no client in this codebase does so (R13), a Rust daemon may implement strict one-request-per-connection without observable difference, even though the current line-buffering code would in principle allow more.
