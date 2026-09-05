# Error taxonomy and fail-open contract v1

## Status

- Contract version: **v1**
- Frozen from: `vendor/codemem` Phase 1 TypeScript implementation (the versions of the files read to produce this document)
- Authority: **this document is authoritative for reimplementation parity**. Where this document and the TypeScript source disagree after a vendor update, this document still governs what a Rust reimplementation must reproduce for v1; a new contract version must be cut to adopt any later vendor change.
- Scope: RPC transport and typed error codes (`daemon-rpc.ts`, `daemon-rpc-contract.ts`), daemon job errors (`daemon-jobs.ts`), daemon operation (export/import/backup) errors (`daemon-operations.ts`), operational-status aggregation (`operational-status.ts`), hook client fail-open behavior for both adapters (`hook-runtime.ts`, `commands/claude-hook-*.ts`, `commands/codex-hook-*.ts`, `commands/hook-rpc-client.ts`), and the `status` CLI surface (`commands/status.ts`). All evidence paths are relative to `vendor/codemem` unless stated otherwise. Short citations of the form `file.ts:LINE` (basename only, no directory) refer to whichever full path under the Scope list above contains that basename — e.g. `daemon-rpc.ts:377` means `packages/core/src/daemon-rpc.ts:377`.

This document describes only what the code does today. It does not propose changes.

---

## 1. RPC transport contract

### 1.1 Framing

The daemon RPC is **not HTTP**. It is newline-delimited JSON over a Unix domain socket at `resolveStorageLayout(dataDir).socketPath`. Each request is one JSON object followed by `\n` (`packages/core/src/daemon-rpc-contract.ts:148`); each response is one JSON object followed by `\n` (`packages/core/src/daemon-rpc.ts:1630`). The `"GET "` / `"POST "` / `"DELETE "` prefixes on method strings (Table 1.2) are naming convention only, carried as opaque string literals — there is no HTTP verb semantics, headers, or status line.

A special control line `STOP <nonce>` (not JSON) on an established connection requests daemon shutdown; the daemon replies `{"status":"stopping"}` if the nonce matches `ctx.identity.nonce`, else `{"status":"mismatch"}`, then closes the connection either way (`packages/core/src/daemon-rpc.ts:1651-1661`).

Only one request may be in flight per connection: while `dispatching` is true, any additional bytes that arrive on the same connection are silently ignored, not buffered for a follow-up request (`packages/core/src/daemon-rpc.ts:1640-1641`).

### 1.2 Request envelope

```ts
type RpcRequest = {
  id: string;
  method: string;               // one of RPC_METHODS, Table 1.4
  adapter_version: string;
  native_cli_version: string;
  normalized_schema_version: number;
  local_api_version: number;    // must equal LOCAL_API_VERSION = 1
  capability_hash: string;      // must equal RPC_CAPABILITY_HASH
  body?: Record<string, unknown>;
};
```
Evidence: `packages/core/src/daemon-rpc-contract.ts:97-106`.

- `LOCAL_API_VERSION = 1` (`daemon-rpc-contract.ts:4`).
- `RPC_CAPABILITY_HASH` is `sha256(RPC_METHODS.join("\n"))` (`daemon-rpc-contract.ts:85-87`) — it changes if the method list changes, and is checked byte-for-byte on every request (`daemon-rpc.ts:418-420`).
- Hook clients send `native_cli_version` as a fixed literal per agent, not the real host CLI version:

  | Agent | `native_cli_version` literal | Evidence |
  |---|---|---|
  | claude | `"2.1.228 (Claude Code)"` | `packages/cli/src/commands/hook-rpc-client.ts:68` |
  | codex | `"codex-cli 0.147.0"` | `packages/cli/src/commands/hook-rpc-client.ts:69` |

  A Rust reimplementation acting as a hook client must send these exact literals for wire compatibility with a TypeScript daemon peer; they are not derived from the actually-installed host version.

### 1.3 Size and deadline limits

| Limit | Value | Enforced where | Evidence |
|---|---|---|---|
| `RPC_MAX_BYTES` | 32 KiB (`32 * 1024`) | Server: accumulated request bytes per connection before the newline is seen | `daemon-rpc-contract.ts:5`, `daemon-rpc.ts:1642-1645` |
| `RPC_MAX_BYTES` (client) | 32 KiB, when the caller supplies it | `callDaemonRpc` itself has **no built-in default** — it only enforces `maxResponseBytes` when `options.maxResponseBytes !== undefined` (`daemon-rpc-contract.ts:154`); the 32 KiB figure comes from the hook client explicitly passing `RPC_MAX_BYTES` as that option | `daemon-rpc-contract.ts:126,154-157`, `hook-rpc-client.ts:316-317` |
| Hook `POST /v1/context/pack` response cap | 256 KiB (`HOOK_RPC_RESPONSE_MAX_BYTES`) | The hook client's own override of the 32 KiB it passes for every other method — not a protocol default | `packages/cli/src/commands/hook-rpc-client.ts:73`, `316-317` |
| `RPC_DEFAULT_DEADLINE_MS` | 2,000 ms | Per-request deadline for all methods except backup methods | `daemon-rpc-contract.ts:6` |
| Backup-method deadline | 1,800,000 ms (30 min) | `GET/POST /v1/backup/*` only, via `BACKUP_RPC_METHODS` | `daemon-rpc-contract.ts:7,59-68` |
| Socket timeout (real enforcement) | `rpcDeadlineForMethod(method)`, reset per request | `net.Socket#setTimeout`; fires `deadline_exceeded` and destroys the connection independent of handler progress | `daemon-rpc.ts:1666`, `1675-1677` |
| Pre-dispatch elapsed check | same deadline value | A *soft* check: computed once, before `handleMethod` runs; it only fires if parsing + validation already consumed the whole budget. It does **not** abort an in-flight async handler. | `daemon-rpc.ts:1559-1562` |

The client-side `callDaemonRpc` also enforces its own `options.timeoutMs` via `socket.setTimeout` (default `RPC_DEFAULT_DEADLINE_MS`) and an optional `AbortSignal` (`daemon-rpc-contract.ts:146`, `128-143`).

### 1.4 RPC methods

All 29 entries of `RPC_METHODS` are exhaustively handled by explicit branches in `handleMethod`. The final fallback branch —

```ts
return { operationId: body.operationId ?? body.id, state: "not_implemented" };
```

— is **unreachable** for any request that passed `isRpcMethod()`, because every one of the 29 registered methods has a preceding branch. A reimplementation is not required to reproduce this fallback's `"not_implemented"` shape; it is dead in the current method set.

| Method | Required body fields | Evidence (required) |
|---|---|---|
| `GET /v1/health` | — | `daemon-rpc.ts:191` |
| `GET /v1/doctor` | — | `daemon-rpc.ts:192` |
| `POST /v1/events` | `idempotencyKey`, `event` | `daemon-rpc.ts:193` |
| `POST /v1/events/batch` | `items` | `daemon-rpc.ts:194` |
| `POST /v1/context/pack` | `requestId`, `context` | `daemon-rpc.ts:195` |
| `POST /v1/search` | `requestId`, `mode` | `daemon-rpc.ts:196` |
| `POST /v1/retrieval/file-context` | `attemptId`, `startedAt`, `completedAt`, `retrievalStatus` | `daemon-rpc.ts:197` |
| `POST /v1/retrieval/file-context/delivery` | `attemptId`, `status` | `daemon-rpc.ts:198` |
| `GET /v1/memories/:id` | `id`, `requestId` | `daemon-rpc.ts:199` |
| `POST /v1/memories/record` | `idempotencyKey`, `kind`, `title`, `body` | `daemon-rpc.ts:200` |
| `DELETE /v1/memories/:id` | `id`, `requestId` | `daemon-rpc.ts:201` |
| `GET /v1/checkpoints` | — | `daemon-rpc.ts:202` |
| `GET /v1/view` | `collection` | `daemon-rpc.ts:203` |
| `POST /v1/viewer/auth/nonce` | — | `daemon-rpc.ts:204` |
| `POST /v1/viewer/auth/exchange` | `nonce` | `daemon-rpc.ts:205` |
| `POST /v1/viewer/auth/verify` | — | `daemon-rpc.ts:206` |
| `POST /v1/viewer/auth/logout` | `session` | `daemon-rpc.ts:207` |
| `GET /v1/backup/list` | — | `daemon-rpc.ts:208` |
| `POST /v1/backup/create` | `operationId`, `payloadHash`, `reason` | `daemon-rpc.ts:209` |
| `POST /v1/backup/verify` | `backupId` | `daemon-rpc.ts:210` |
| `POST /v1/backup/restore` | `operationId`, `payloadHash`, `backupId` | `daemon-rpc.ts:211` |
| `POST /v1/operations/export` | `operationId`, `payloadHash`, `outputPath`, `filters` | `daemon-rpc.ts:212` |
| `POST /v1/operations/import` | `operationId`, `payloadHash`, `inputPath` | `daemon-rpc.ts:213` |
| `GET /v1/operations/:id` | `id` | `daemon-rpc.ts:214` |
| `POST /v1/jobs` | `kind` | `daemon-rpc.ts:215` |
| `GET /v1/jobs` | — | `daemon-rpc.ts:216` |
| `GET /v1/jobs/:id` | `id` | `daemon-rpc.ts:217` |
| `GET /v1/processing-jobs/:id` | `id` | `daemon-rpc.ts` |
| `POST /v1/processing-jobs/:id/doctor-retry` | `id`, `producerReceiptId`, `expectedRole`, `expectedProviderFingerprint`, `expectedManifestFingerprint`, `expectedAttemptCount`, `expectedClaimGeneration` | `daemon-rpc.ts` |

`GET /v1/checkpoints` always returns `{ checkpoints: [] }` regardless of body (`daemon-rpc.ts:568`) — a stub, not a functioning checkpoint listing.

### 1.5 Maintenance-mode blocking

While `isMaintenanceMode(ctx)` is true (`ctx.jobs.isMaintenanceMode()` or `ctx.restoreState?.active === true`), the existing mutation set plus `POST /v1/processing-jobs/:id/doctor-retry` — 15 methods total — is rejected with retryable `maintenance_mode` before dispatch.

`POST /v1/events`, `POST /v1/events/batch`, `POST /v1/context/pack`, `POST /v1/search`, `POST /v1/retrieval/file-context`, `POST /v1/retrieval/file-context/delivery`, `GET /v1/memories/:id`, `POST /v1/memories/record`, `DELETE /v1/memories/:id`, `POST /v1/backup/create`, `POST /v1/backup/restore`, `POST /v1/operations/export`, `POST /v1/operations/import`, `POST /v1/jobs`, `POST /v1/processing-jobs/:id/doctor-retry`.

All other methods, including `GET /v1/processing-jobs/:id`, remain callable during maintenance mode.

---

## 2. Typed RPC error codes

Every RPC response is either `{ id, result }` or `{ error: { code, message, retryable } }` (`TypedRpcError`, `daemon-rpc-contract.ts:89-95`). `code` is an untyped `string` — there is no closed enum enforced by the type system; the set below is the complete set actually produced by the read code paths.

### 2.1 Codes produced by the transport/dispatch layer itself

| Code | Retryable | Trigger | Evidence |
|---|---|---|---|
| `invalid_json` | false | Request line is not valid JSON, or top-level value is not an object | `daemon-rpc.ts:377,380` |
| `unknown_field` | false | Unknown top-level field, or unknown field for the resolved method's body | `daemon-rpc.ts:385`, `1525` |
| `invalid_request` | false | Missing `id`; `body` present but not an object; a required field missing/empty/wrong-typed; a persisted-ID field failing `isSafePersistedText`; `reason` failing its length/safety check | `daemon-rpc.ts:388,406,1530,1547,1550,1553,1556` |
| `unknown_method` | false | `method` not a string, or not a member of `RPC_METHODS` | `daemon-rpc.ts:391`, `1519` |
| `protocol_mismatch` | false | Handshake fields missing/mistyped, or `local_api_version` / `normalized_schema_version` / `capability_hash` mismatch | `daemon-rpc.ts:400,413,416,419` |
| `deadline_exceeded` | **true** | Pre-dispatch elapsed check exceeds the method's deadline (soft), or the real socket timeout fires (hard) | `daemon-rpc.ts:1561`, `1676` |
| `maintenance_mode` | **true** | Method is in `MAINTENANCE_BLOCKED_METHODS` while maintenance mode is active | `daemon-rpc.ts:1564-1568` |
| `capture_saturated` | **true** | Two direct capture RPCs (`POST /v1/events` or `POST /v1/events/batch`) already own the request-level slots; no event write is attempted by the rejected request | `daemon-rpc.ts` |
| `payload_too_large` | false | Accumulated connection bytes exceed `RPC_MAX_BYTES` before a newline is seen | `daemon-rpc.ts:1643` |
| `internal_error` | false | Catch-all: any thrown error not an instance of a recognized RPC/backup/operation/mutation/resume error class; also the JSON-parse-failure branch of the per-connection dispatch promise | `daemon-rpc.ts` |

### 2.2 Codes produced by client-side connection mapping (never sent by the daemon over the wire)

These are synthesized locally by `mapPeerConnectError` when the *client* fails to reach the socket — the daemon process never emits them, because if the daemon can't be reached, it can't respond at all.

**The mapping is applied to exactly three `errno` values.** `callDaemonRpc`'s socket `error` handler calls `mapPeerConnectError` only when `error.code` is `EACCES`, `ECONNREFUSED`, or `ENOENT`; every other socket error rejects the promise with the original `Error` and never becomes a `TypedRpcError` (`daemon-rpc-contract.ts:170-181`). `mapPeerConnectError`'s own trailing `peer_denied` fallback (`daemon-rpc-contract.ts:120`) is therefore unreachable from `callDaemonRpc` and only applies to direct calls of the exported function. Concretely: a regular file at the socket path yields `ENOTSOCK`, which surfaces as a thrown error, **not** `peer_denied`.

| Code | Retryable | Trigger | Evidence |
|---|---|---|---|
| `peer_denied` | false | `EACCES` (via the handler's three-code allowlist); the function's catch-all branch is unreachable from `callDaemonRpc` | `daemon-rpc-contract.ts:114-115`, `120`, `169-180` |
| `daemon_unavailable` | **true** | `ECONNREFUSED` or `ENOENT` connecting to the socket | `daemon-rpc-contract.ts:117-118`, `169-180` |

`status.ts` depends on this distinction directly: it treats `health.error.code === "daemon_unavailable"` as "daemon not running" (the default assumption) and *any other* health error code as `daemon: "unavailable"` (a harder failure) — see §6.1.

### 2.3 `RpcRequestError` (`daemon-rpc.ts:318-326`)

```ts
class RpcRequestError extends Error {
  constructor(message: string, readonly code = "invalid_request") { ... }
}
```

Default code is `"invalid_request"`. Explicit non-default uses include `not_found` for missing memories and processing jobs, plus retryable `capture_saturated` for direct event admission at the max-two limit. Other validation sites use the default code.

### 2.3.1 `ProcessingResumeError`

| Code | Retryable | Trigger | Evidence |
|---|---|---|---|
| `not_found` | false | The exact processing job does not exist | `store.ts` |
| `stale_snapshot` | false | Doctor retry confirmation no longer matches the displayed attempt snapshot | `store.ts` |
| `grant_pending` | **true** | A processing job already owns an unconsumed one-shot resume grant | `daemon-rpc.ts`, `store.ts` |
| `invalid_signal` | false | A resume signal fails its closed shape or identity validation | `store.ts` |

`dispatchDaemonRpc` preserves the explicit code and retryable bit in the typed error envelope; doctor retry therefore cannot collapse a stale confirmation into `internal_error`.

### 2.4 `BackupRequestError` (`packages/core/src/online-backup.ts:60,138-146`)

```ts
type BackupErrorCode = "invalid_request" | "conflict" | "not_found";
```

All three codes are used by `online-backup.ts` (invalid operation/payload IDs and mismatched hashes → `invalid_request`; malformed/missing/unreadable restore results → `conflict`; no canonical database to back up → `not_found`). `daemon-rpc.ts:516` additionally throws `BackupRequestError("conflict", ...)` when a restore is requested while another restore is active or work is pending.

### 2.5 `DaemonOperationRequestError` (`packages/core/src/daemon-operations.ts:72-85`)

```ts
class DaemonOperationRequestError extends Error {
  constructor(
    readonly code: "invalid_request" | "idempotency_conflict" | "not_found" | "conflict" | "internal_error",
    message: string,
  ) { ... }
}
```

All 5 codes are reachable: `invalid_request` for malformed submit fields (`daemon-operations.ts:99,107,110,115,123,133,141,152,160,174,206,385,389,393,405,520`) — including `submit()`'s payload-hash-mismatch check `payloadHash does not match the operation request.` at `daemon-operations.ts:393-396`, distinct from the field-shape checks at `:385,389`; the `submit()` idempotency-conflict branch picks `"conflict"` for `backup-create`/`backup-restore` kinds and `"idempotency_conflict"` for `export`/`import` when an existing operation ID has a different kind or payload hash (`daemon-operations.ts:413-419`); `not_found` when `get()` is called for an unknown operation ID (`daemon-operations.ts:524`); `internal_error` as `runBackup`'s fallback when a failed journal's error code is not in the sanitized allowlist, or when a backup/restore never produced a durable result (`daemon-operations.ts:499-514`, see §4.3).

### 2.6 `MutationConflictError` (`packages/core/src/mutation-dispatcher.ts:13-25`)

```ts
class MutationConflictError extends Error {
  readonly code = "idempotency_conflict";
}
```

Single fixed code. Raised by `dispatchClassA` when an idempotency key is reused with a different payload. Surfaced two different ways depending on entry point:

- `POST /v1/events` (single event): propagates to `dispatchDaemonRpc`'s catch chain and becomes a **typed error response** `{ error: { code: "idempotency_conflict", ... } }` (`daemon-rpc.ts:1591-1592`).
- `POST /v1/events/batch` (per item): caught **per row** inside `handleEventBatch` and converted into a **success-shaped receipt** `{ receiptId: error.receipt.receiptId, status: "conflict" }` — the batch RPC call itself still returns `result`, not `error` (`daemon-rpc.ts:804-809`). A Rust reimplementation must preserve this asymmetry: the same underlying conflict is a top-level RPC error for the singular method and an in-band per-item status string for the batch method.

### 2.7 `DaemonJobRequestError` (`packages/core/src/daemon-jobs.ts:206-211`)

```ts
class DaemonJobRequestError extends Error {
  constructor(message: string) { super(message); this.name = "DaemonJobRequestError"; }
}
```

This class carries **no typed `code` field** at all (unlike §2.4–2.6). It has exactly **20 throw sites**, all inside synchronous request-validation helpers reachable only from `submit()`, `list()`, and `get()` (`daemon-jobs.ts:222,235,249,258,275,299,307,333,346,349,354,358,364,376,550,553,567,583,590,601` — verified by `grep -c "throw new DaemonJobRequestError" daemon-jobs.ts` = 20).

Every one of the three job RPC branches in `daemon-rpc.ts` catches it explicitly and rewraps it as a **plain `RpcRequestError` with no explicit code argument**:

```ts
if (error instanceof DaemonJobRequestError) throw new RpcRequestError(error.message);
```
(`daemon-rpc.ts:612`, `626`, `634`, one per branch for `POST /v1/jobs`, `GET /v1/jobs`, `GET /v1/jobs/:id`.)

Because `RpcRequestError`'s `code` parameter defaults to `"invalid_request"` (§2.3, `daemon-rpc.ts:321`), and `dispatchDaemonRpc`'s catch chain maps `RpcRequestError` to `typedError(error.code, error.message)` (`daemon-rpc.ts:1582-1583`), **all 20 distinct job-validation failures collapse to the single wire-level code `"invalid_request"`**, distinguishable to a caller only by the free-text `message`. `DaemonJobRequestError` is never listed in the dispatch catch chain itself (`daemon-rpc.ts:1581-1594`) — it only reaches the wire because every call site pre-wraps it; there is no path in the read code where it reaches the top-level `internal_error` catch-all. See "Known gaps" for the correction this supersedes.

Job *execution* failures (inside the async `run()`, not `submit`/`list`/`get`) are unrelated to this class: any thrown error there becomes job state `"failed"` with `error_code` set to `"redaction_degraded"` (if `RedactionWorkerError`) or `"job_failed"` (all other errors) — a **job-row field**, not an RPC-level typed error (`daemon-jobs.ts:732-744`). The original error's identity and message are discarded entirely; the persisted snapshot always reports the fixed string `"Daemon job failed; submit a new job to retry."` when `error_code` is set (`daemon-jobs.ts:503-505`). A Rust reimplementation must reproduce this exact constant string for job-row parity, not attempt to recover or forward the real failure text.

A third job-row `error_code` value, `"daemon_restarted"`, is assigned not from inside `run()` but from the `DaemonJobService` constructor: on every daemon startup, `UPDATE daemon_jobs SET state = 'failed', error_code = 'daemon_restarted', ... WHERE state IN ('queued', 'running')` (`daemon-jobs.ts:522-528`) transitions any job that was still `queued` or `running` when the previous daemon process stopped into `failed`/`daemon_restarted`. This is the same literal string as the `daemon_restarted` operations-journal code in §4.2 but a distinct occurrence in a distinct subsystem (a `daemon_jobs` row `error_code`, not a `daemon-operations.ts` journal `error.code`) — the two do not share code or a single source of truth. This snapshot also reports the fixed `"Daemon job failed; submit a new job to retry."` message, since the snapshot logic keys only on `error_code` being non-null (`daemon-jobs.ts:503-505`), not on which of the three codes it is. §3.6's internal-backfill re-enqueue logic reads this code directly to decide whether a prior failure was a restart artifact rather than a real failure (`latest.error_code !== "daemon_restarted"`, `daemon-jobs.ts:677`). A Rust reimplementation needs this startup-time restart-recovery transition (orphaned `queued`/`running` rows → `failed`/`daemon_restarted`) for job-row parity.

---

## 3. Daemon job service (`daemon-jobs.ts`)

### 3.1 Job kinds

All 28 members of `JOB_KINDS` (`daemon-jobs.ts:56-85`):

`db.init`, `db.vacuum`, `dedup-keys.backfill`, `gate.raw-events`, `memories.dedup`, `memories.prune`, `narrative.backfill`, `observations.prune`, `plan.relink`, `projects.normalize`, `projects.rename`, `raw-events.prune`, `raw-events.retry`, `refs.backfill`, `report.artifact`, `report.db-size`, `report.extraction`, `report.memory-role`, `report.raw-events`, `report.relink`, `report.role-compare`, `scopes.backfill`, `secrets.scan`, `session-context.backfill`, `structured.backfill`, `summary-dedup.backfill`, `tags.backfill`, `vectors.migrate`.

Submitting any other string as `kind` throws `DaemonJobRequestError("job kind is unsupported.")` (`daemon-jobs.ts:375-377`).

### 3.2 Job states

`"queued" | "running" | "completed" | "failed"` (`daemon-jobs.ts:174`, `JOB_STATES` at `:55`). `maxAttempts` is always `1` (`daemon-jobs.ts:183,198`) — there is no automatic retry of a failed job; the caller must submit a new job.

### 3.3 Maintenance-classified kinds

`MAINTENANCE_JOB_KINDS` (18 kinds, `daemon-jobs.ts:128-147`) run inside `runInMaintenance` (sets `isMaintenanceMode()` true for the duration, blocking the 15 methods in §1.5) **only when `dry_run === 0`** (`daemon-jobs.ts:751`) — a `dryRun: true` submission of a maintenance-classified kind does **not** enter maintenance mode.

`BACKUP_REQUIRED_JOB_KINDS` (8 kinds: `db.vacuum`, `memories.dedup`, `memories.prune`, `observations.prune`, `projects.normalize`, `projects.rename`, `raw-events.prune`, `secrets.scan`, `daemon-jobs.ts:148-157`) does **not** gate the backup requirement on its own. The guard inside `runInMaintenance` (entered only for non-dry-run maintenance-classified jobs, `daemon-jobs.ts:751-752`) is `if (BACKUP_REQUIRED_JOB_KINDS.has(row.kind) || args.internal !== true)` (`daemon-jobs.ts:756`) — an **OR**, not a subset restriction. `internal` is not in any kind's `JOB_ARGS` allowlist (`daemon-jobs.ts:86-127`), and `validateJobArgs` rejects unknown argument names (`daemon-jobs.ts:307-308`), so `submit()` — the only entry point for externally-submitted (RPC) jobs — always rejects an `internal` argument; consequently `args.internal !== true` is always true for any job that reached `runInMaintenance` via `submit()`, regardless of `kind`. The only jobs that ever carry `args: { internal: true }` are the six kinds `scanInternalBackfills` enqueues via a direct `this.enqueue(kind, { internal: true }, false)` call that bypasses `submit()`/`validateJobArgs` entirely (`daemon-jobs.ts:681`; scan loop `daemon-jobs.ts:653-682`) — and those six kinds (`scopes.backfill`, `dedup-keys.backfill`, `session-context.backfill`, `refs.backfill`, `summary-dedup.backfill`, `vectors.migrate`) are disjoint from the 8-kind `BACKUP_REQUIRED_JOB_KINDS` set. Net effect in the currently-reachable code paths: **every non-dry-run maintenance-classified job submitted through `submit()` requires a verified backup**, not only the 8 named in `BACKUP_REQUIRED_JOB_KINDS`; the internal scheduler's 6 kinds never require one (their `args.internal === true` and none of them is in the 8-kind set); `BACKUP_REQUIRED_JOB_KINDS` has zero observable gating effect in the reachable paths — a job's kind being in that set changes nothing, since the `args.internal !== true` disjunct already forces the backup for every reachable non-dry-run maintenance job. Backup verification failure throws a plain `Error`, which becomes job state `"failed"` with `error_code: "job_failed"` (§2.7).

`DRY_RUN_JOB_KINDS` (11 kinds, `daemon-jobs.ts:158-170`) are the only maintenance-classified kinds that accept `dryRun: true`; submitting `dryRun: true` for any other maintenance-classified kind throws `DaemonJobRequestError("${kind} does not support dryRun.")` (`daemon-jobs.ts:552-554`).

### 3.4 Job argument validation

`validateJobArgs` (`daemon-jobs.ts:304-369`) rejects unknown argument names per-kind against `JOB_ARGS` (`daemon-jobs.ts:86-127`), and enforces per-field bounds (integers 1–10,000 by default, `maxAgeDays` up to 36,500, `windowMs` up to 31,536,000,000, numeric rate fields 0–1, `windowHours` 0.01–8,760, string fields byte-capped, `since` must parse as an ISO date). Four kinds have cross-field requirements beyond per-field typing: `report.role-compare` requires both `baselineDbPath` and `candidateDbPath`; `projects.rename` requires both `oldName` and `newName`; `raw-events.prune` requires `maxAgeDays`; `report.extraction` requires `scenarioId` and exactly one of `sessionId`/`batchId` (`daemon-jobs.ts:344-368`).

### 3.5 Job result size cap

`MAX_RESULT_BYTES = 512 * 1024`. A job whose serialized result exceeds this throws a plain `Error("job result exceeds the persisted result limit")` **after** the job's work has already run, converting an otherwise-successful job into state `"failed"` / `error_code: "job_failed"` (`daemon-jobs.ts:171`, `721-724`).

### 3.6 Internal backfill self-scheduling

`scanInternalBackfills` (started once via `startInternalBackfills`, polled every `INTERNAL_SCAN_MS = 5,000` ms, `daemon-jobs.ts:172`, `558-563`, `653-683`) auto-enqueues six backfill kinds (`scopes.backfill`, `dedup-keys.backfill`, `session-context.backfill`, `refs.backfill`, `summary-dedup.backfill`, `vectors.migrate`) whenever their respective `hasPending*` check is true and the most recent job of that kind is not currently `queued`/`running`, and — if the most recent job of that kind is `failed` — that failure is not itself a prior internal-scheduler restart artifact: the skip condition is `latest.state === "queued" || latest.state === "running" || (latest.state === "failed" && (latest.error_code !== "daemon_restarted" || latest.internal !== 1))` (`daemon-jobs.ts:673-678`) — i.e. a `failed`/`daemon_restarted` row from a job that itself carried `args.internal === true` (see §2.7) does *not* block re-enqueue, but any other `failed` row does. This bypasses `submit()` entirely: these six jobs are enqueued directly via `this.enqueue(kind, { internal: true }, false)` (`daemon-jobs.ts:681`), so `validateJobArgs`'s unknown-argument rejection of `internal` (§3.3) never applies to them. These are non-dry-run, non-backup-required internal jobs submitted with `args: { internal: true }`.

---

## 4. Daemon operations (`daemon-operations.ts`)

### 4.1 Operation kinds and states

Kinds: `"export" | "import" | "backup-create" | "backup-restore"` (`daemon-operations.ts:44`). States: `"prepared" | "writing" | "verified" | "backup_verified" | "applying" | "committed" | "failed"` (`daemon-operations.ts:45-52`). Only `"committed"` and `"failed"` are terminal (`TERMINAL_STATES`, `:89`).

### 4.2 `GET /v1/operations/:id` — raw journal error codes

`get()` returns the durable journal's `error` field verbatim when present, without sanitization. Codes observed in the read code:

| Code | Meaning | Evidence |
|---|---|---|
| `daemon_restarted` | Operation was non-terminal when the daemon last started; export cleanup verification status is embedded in the message | `daemon-operations.ts:368-371` |
| `daemon_stopping` | `schedule()` threw synchronously (service stopping) right after `submit()` persisted the journal | `daemon-operations.ts:441`, `464` |
| `operation_failed` | The scheduled work's promise rejected for a reason other than a recognized typed error | `daemon-operations.ts:557-559` |
| `export_failed` | Any exception during `executeExport` | `daemon-operations.ts:681-683` |
| `invalid_import` | `readImportPayload`/initial dry-run preview failed, before any backup was attempted | `daemon-operations.ts:688-733` (`failureCode` starts as `"invalid_import"`) |
| `backup_failed` | The pre-import verified backup could not be created/verified | same, `failureCode` reassigned at `:701` |
| `import_failed` | The import itself failed after a successful backup | same, `failureCode` reassigned at `:719` |
| any `BackupRequestError` code (`invalid_request`/`conflict`/`not_found`) | Backup-create/backup-restore execution threw a typed backup error that wasn't recoverable | `daemon-operations.ts:622-628` |

### 4.3 `runBackup()` — sanitized error surface

`runBackup` (used only by the RPC methods `POST /v1/backup/create` and `POST /v1/backup/restore`, `daemon-rpc.ts:497-499`, `512-525`) does **not** return the raw journal code from §4.2. It maps a terminal-failed journal's error code through an explicit allowlist and defaults everything else to `internal_error`:

```ts
const code = ["invalid_request", "conflict", "idempotency_conflict", "not_found"].includes(journal.error.code)
  ? journal.error.code
  : "internal_error";
throw new DaemonOperationRequestError(code, journal.error.message);
```
(`daemon-operations.ts:500-509`.) Note `idempotency_conflict` is in this allowlist even though §4.2's backup-create/backup-restore failure codes never actually produce it (that code is reserved for export/import's `submit()`-time conflict, §2.5) — it is allowlisted defensively but not reachable via the backup path in the read code. If the operation never produced a durable result at all (no journal, or journal not failed/committed), `runBackup` throws `DaemonOperationRequestError("internal_error", "The daemon backup operation did not produce a durable result.")` (`daemon-operations.ts:511-514`).

So a Rust reimplementation must expose **two different error surfaces for the same underlying failure**: `GET /v1/operations/:id` (raw, kind-specific codes) vs. the synchronous `POST /v1/backup/create|restore` RPC result (sanitized 5-code set: `invalid_request`, `conflict`, `idempotency_conflict`, `not_found`, `internal_error`).

### 4.4 Export/import specifics

- Export output path must resolve **outside** the daemon data directory both at submit time (`daemon-operations.ts:398-409`) and again at execute time (`:664-666`) — double-checked because the resolved realpath can differ between the two moments (e.g., symlink changes).
- Export writes to a `.{hash}.tmp` sibling file with `wx` (exclusive create) + `mode 0o600`, renames into place, then `fsync`s the parent directory, and re-hashes the output file to confirm it matches the pre-computed SHA-256 before marking `"committed"` (`daemon-operations.ts:223-247`, `662-680`).
- Import always runs a dry-run preview first (even for a non-dry-run request) to validate the payload before creating a pre-import backup; only after `requireVerifiedBackup` succeeds does it apply the real import (`daemon-operations.ts:690-724`).

---

## 5. Hook client fail-open behavior

`hook-runtime.ts` dispatches exactly 5 commands: `claude-hook-file-context`, `claude-hook-ingest`, `claude-hook-inject`, `codex-hook-ingest`, `codex-hook-inject` (`packages/cli/src/hook-runtime.ts:21-27`).

### 5.1 Runtime-level skip paths (apply to all 5 commands, before any adapter-specific logic runs)

| Condition | Behavior | Evidence |
|---|---|---|
| `CODEMEM_PLUGIN_IGNORE` truthy (`1`/`true`/`yes`/`on`, case-insensitive, trimmed) | Returns `fallback(command)` immediately — `""` for `claude-hook-ingest`, else `{"continue":true}` — without parsing stdin as the adapter payload | `hook-runtime.ts:40-46,54-56` |
| stdin exceeds `HOOK_RUNTIME_INPUT_MAX_BYTES` (256 KiB) | Same fallback | `hook-runtime.ts:17,54` |
| stdin is not valid JSON, or top-level value is not a plain object | Same fallback | `hook-runtime.ts:58-64` |
| Any exception inside command dispatch (`ingestClaudeHookPayload`, etc.) | Same fallback (outer `try/catch`) | `hook-runtime.ts:74-100` |
| Worker (spawned per invocation) dies, times out, sends a malformed/oversized message (>256 KiB `HOOK_RUNTIME_OUTPUT_MAX_BYTES`), or fails to spawn | `finish(fallbackOutput)` | `hook-runtime.ts:161-188` |
| `readStdin` throws or is destroyed by its own deadline timer | `readStdin` returns `null` → `main()` uses `fallback(command)` directly, bypassing the worker entirely | `hook-runtime.ts:109-129`, `199-201` |
| Command string not one of the 5 registered | `process.exitCode = 2`, nothing written to stdout | `hook-runtime.ts:193-195` — unreachable via the shipped `hooks.json` files, which always pass a valid command |

### 5.2 Two supervision layers, not one

- **`hook-runtime.mjs` (worker) path** — the shipped integration. `main()` computes `deadlineAtMs = clientHardCapMs(command) - 50` (`hook-runtime.ts:198`), reads stdin under that deadline, then runs the adapter logic inside a `Worker` thread that is force-terminated and replaced with `fallbackOutput` if it does not post a message before the remaining budget elapses (`hook-runtime.ts:142-189`). This is the actual in-process enforcement of `clientHardCapMs`.
- **Commander CLI actions** (`claudeHookInjectCommand`, `codexHookInjectCommand`, `claudeHookIngestCommand`, `codexHookIngestCommand`) — invoked directly (not through `hook-runtime.mjs`), e.g. for manual/CLI use. These have **no worker, no outer hard-cap enforcement, and no `deadlineAtMs` unless the caller supplies one** — `prepareHookEvent`'s `deadlineAtMs` parameter defaults to `performance.now() + HOOK_DELIVERY_BUDGETS[agent].clientHardCapMs` only inside `prepareHookEvent` itself (`hook-rpc-client.ts:246`), with no external watchdog killing the process if that budget is exceeded.

### 5.3 Per-adapter early-return matrix

All 5 adapter functions take `payload` and (except `claude-hook-file-context`, which has its own dedicated logic) call `prepareHookEvent(agent, payload, deadlineAtMs)` (`hook-rpc-client.ts:243-287`) to build the normalized/redacted event, then decide whether and how to call `deliverHookEvent`.

| Adapter | `CODEMEM_PLUGIN_IGNORE` check | `hook_event_name` gate | `prepared.status === "skipped"` handling | Other skip conditions before delivery | Evidence |
|---|---|---|---|---|---|
| `claude-hook-inject` (`buildClaudeHookInjection`) | Returns `continueResult()` **without calling `prepareHookEvent` or `deliver()` at all** | none | Returns `continueResult()` **without calling `deliver()`** | empty `prompt`, `CODEMEM_INJECT_CONTEXT` disabled, or `redaction.local_only` → each calls `deliver()` (fire-and-forget, errors swallowed) then returns `continueResult()` | `claude-hook-inject.ts:87,89,107-118` |
| `codex-hook-inject` (`buildCodexHookInjection`) | Same — returns before `prepareHookEvent`/`deliver()` | **Extra gate absent from Claude adapter**: `if (payload.hook_event_name !== "UserPromptSubmit") return continueResult();` — also skips **without calling `deliver()`** | Returns `continueResult()` **without calling `deliver()`** | same three conditions as claude, each calling `deliver()` before returning | `codex-hook-inject.ts:101,102,104,108-119` |
| `claude-hook-ingest` (`ingestClaudeHookPayload`) | Checked only in the **command action wrapper**, before the function is even called; the exported function itself has no such check | none | **Always calls `deliverHookEvent(...)` regardless of `prepared.status`** — `deliverHookEvent` internally short-circuits to `{via:"skipped"}` for a skipped `prepared` without an RPC call, but the call site does not skip calling it | none — session-state enrichment failures are caught and ignored, delivery is unconditional | `claude-hook-ingest.ts:27,57-60`, `hook-rpc-client.ts:335` |
| `codex-hook-ingest` (`ingestCodexHookPayload`) | Same pattern — action wrapper only | none | Same as claude-hook-ingest: always calls `deliverHookEvent` | none | `codex-hook-ingest.ts:50-54,72-75` |
| `claude-hook-file-context` (`buildClaudeFileContext`) | Returns `continueResult()` **before `prepareHookEvent` runs, and before `deliver()`** | none | Fires `deliverHookEvent(...)` **unconditionally right after `prepareHookEvent` succeeds** (line 298), *then* separately checks `prepared.status === "skipped"` afterward only to short-circuit its own file-context retrieval logic — delivery already started either way | `prepareHookEvent` throwing synchronously also returns before `deliver()` is ever called | `claude-hook-file-context.ts:290,292-296,298-308` |

**The behavioral split the corrections flagged is real and confirmed**: the two *inject* adapters (`claude-hook-inject`, `codex-hook-inject`) are the only two of the five commands whose `CODEMEM_PLUGIN_IGNORE` path and `skipped`-status path return **without ever calling `deliverHookEvent`** — meaning delivery of the underlying normalized event to the daemon (or spool) is skipped entirely on those paths, not merely short-circuited inside the delivery function. The other three commands (`claude-hook-ingest`, `codex-hook-ingest`, `claude-hook-file-context`) always invoke `deliverHookEvent` once `prepareHookEvent` has run without throwing, and rely on `deliverHookEvent`'s own internal `prepared.status === "skipped"` check (`hook-rpc-client.ts:335`) to suppress the RPC call. The net effect (no RPC event sent) is the same in both patterns for the "skipped" case, but the `CODEMEM_PLUGIN_IGNORE` case differs meaningfully: for `claude-hook-ingest`/`codex-hook-ingest`, the ignore check lives only in the Commander action (outside the exported function), so a caller of the exported function directly (as `hook-runtime.ts` does) does **not** get the ignore short-circuit at all for those two commands — only the inject adapters and `claude-hook-file-context` check `CODEMEM_PLUGIN_IGNORE` inside the function that `hook-runtime.ts` actually calls. `hook-runtime.ts`'s own top-level `disabled()` check (`hook-runtime.ts:40-46`) covers all 5 commands regardless, so this only matters for direct/CLI invocation outside the worker runtime.

### 5.4 `deliverHookEvent` outcomes

`deliverHookEvent` returns `{ via: "rpc" | "spool" | "skipped" | "dropped" }` (`hook-rpc-client.ts:324-373`):

| `via` | Meaning | Evidence |
|---|---|---|
| `"skipped"` | `prepared.status === "skipped"` (policy-ignored path, or normalization failed) | `hook-rpc-client.ts:335` |
| `"dropped"` | `prepareHookEvent` threw synchronously, **or** the RPC attempt failed and the remaining time budget (`prepared.deadlineAtMs - now`) is `<= budget.fsyncMarginMs` (400 ms), **or** the spool write itself reports `status: "dropped"` | `hook-rpc-client.ts:330-334`, `351-352`, `371` |
| `"rpc"` | `POST /v1/events` RPC succeeded within `min(rpcTimeoutMs, budget.rpcCutoffMs)` | `hook-rpc-client.ts:344-349` |
| `"spool"` | RPC failed but time remained above the fsync margin; local spool write returned `"queued"` or `"duplicate"` (both map to `"spool"`, only `"dropped"` from the spool maps to `"dropped"`) | `hook-rpc-client.ts:353-371` |

### 5.5 Hook delivery budgets (`HOOK_DELIVERY_BUDGETS`, `daemon-rpc-contract.ts:8-25`)

| Field | claude | codex | Enforced by | In-process? |
|---|---|---|---|---|
| `clientHardCapMs` | 2,000 | 1,500 | Worker termination deadline in `hook-runtime.ts` (`main()`: `clientHardCapMs(command) - 50`) | **Yes** |
| `rpcCutoffMs` | 1,500 | 1,000 | `min(options.rpcTimeoutMs, budget.rpcCutoffMs)` passed as the RPC socket timeout | **Yes** |
| `spoolReserveMs` | 500 | 500 | Subtracted from `prepared.deadlineAtMs` to compute the RPC attempt's own deadline, reserving time for a spool fallback | **Yes** |
| `spoolLockWaitMs` | 100 | 100 | Upper bound on the spool file-lock wait budget after an RPC failure | **Yes** |
| `fsyncMarginMs` | 400 | 400 | Threshold below which a failed RPC gives up and reports `"dropped"` instead of attempting the spool write | **Yes** |
| `outerWatchdogMs` | 3,000 | 5,000 | **Not enforced anywhere in the read TypeScript.** It is only consumed by a test asserting that the *host's* hook manifest (`plugins/claude/hooks/hooks.json`, `plugins/codex/hooks/hooks.json`) declares a `"timeout"` (in seconds) equal to `outerWatchdogMs / 1000` for every hook entry pointing at `hook-runtime.mjs` | **No** — enforced externally by the Claude Code / Codex CLI host process killing the hook subprocess after its own configured timeout. Evidence: `packages/cli/src/commands/hook-thin-client.test.ts:56-77`, `plugins/claude/hooks/hooks.json` (`"timeout": 3`), `plugins/codex/hooks/hooks.json` (`"timeout": 5`) |

`hook-runtime.ts` also enforces `REDACTION_WORKER_DEADLINE_MS = 100` as a warm-up deadline passed to `warmRedactionWorker` (`hook-runtime.ts:68-72`) — this is a soft warm-start hint, not a hard cutoff on redaction itself (redaction worker enforcement is out of the read scope for this document).

---

## 6. `status` CLI and doctor/health surfaces

### 6.1 `GET /v1/health` and `GET /v1/doctor` result shapes

`GET /v1/health` result: `{ status: "ok", instanceId, maintenanceMode, protocolVersion, spool, capability }` (`daemon-rpc.ts:441-449`). `capability` is the bounded frozen safe projection selected at daemon startup: capture-only when no current manifest exists, or the validated manifest/provider identity, runtime reason, provider health, feature gates, and explicit `pending_schema_v21` / `pending_pack_boundary` readiness. It contains no credential value. `status` is always the literal `"ok"` if the RPC succeeds at all — a reachable-but-unhealthy daemon is not distinguished from a healthy one by this field; distress is visible in `spool.status`, `capability.runtimeReason`, `capability.providerHealth`, or by the RPC failing entirely.

`GET /v1/doctor` result adds `diagnostics: { pid, dataDir, lock: "held", socket: "listening", platform, spool, hookDelivery: { implementation: "node-fallback", p95TargetMs: 150, budgets: HOOK_DELIVERY_BUDGETS }, redaction: { status: degradedDeliveries > 0 ? "warning" : "ok", degradedDeliveries, workerDeadlineMs }, operationalStatus, capability }` (`daemon-rpc.ts:451-496`). The nested `capability` is the same frozen object returned by health. `degradedDeliveries` is a point-in-time count across `raw_events`, `memory_items`, and `daemon_jobs`.

### 6.2 `status` command daemon/database resolution logic

```ts
const health = await deps.requestRpc(dataDir, "GET /v1/health");
if (health.ok) {
  daemonState = "running";
  const doctor = await deps.requestRpc(dataDir, "GET /v1/doctor");
  databaseState = doctor.ok ? "ready" : "unavailable";
  snapshot = doctor.ok ? doctorOperationalStatus(doctor.result) : null;
} else if (health.error.code !== "daemon_unavailable") {
  daemonState = "unavailable";
  databaseState = "unavailable";
}
// else: daemonState stays "not_running" (its initial value), databaseState stays "unknown"
```
(`status.ts:330-346`.) So `daemon_unavailable` (the client-synthesized code from §2.2, meaning "could not connect at all") is treated as the *ordinary* "daemon not running" case, while every other health-RPC failure code (e.g. `peer_denied`, `protocol_mismatch`, `deadline_exceeded`, or a genuine daemon-side `internal_error`) is treated as the *harder* `daemon: "unavailable"` failure.

### 6.3 `OperationalStatusReport` fields and states

```ts
type DaemonState = "running" | "not_running" | "unavailable";
type DatabaseState = "ready" | "missing" | "unavailable" | "unknown";
type MaintenanceState = "idle" | "running" | "failed" | "unknown";
type SemanticIndexState = "healthy" | "pending" | "degraded" | "failed" | "unknown";
type RawEventsState = "healthy" | "backlogged" | "failing" | "unknown";
type ObserverState = "healthy" | "idle" | "pending" | "backoff" | "failed" | "unconfigured" | "unknown";
type ProcessingNextAction = "none" | "activate_valid_manifest" | "configure_credential" |
  "wait_for_capacity" | "confirm_retry" | "restart_daemon" | "upgrade_runtime";
```

The doctor snapshot carries `raw_events: { available, pending, source_gaps, failed_batches }`; the CLI projects it as `raw_events: { state, pending, source_gaps }`. `source_gaps` is a content-free count over at most the first 25 pending streams ordered by session update time and stable source/stream keys, so its observable range is 0–25 and it is not an unbounded total. A positive count forces processing `next_action: "upgrade_runtime"`; stream IDs, event IDs, payloads, and paths are never projected. If either the pending aggregate or source-gap scan cannot run, `available=false`; a successful pending count remains visible, but the CLI reports raw events `unknown`, emits `raw_events_unavailable`, and forces `next_action=upgrade_runtime`. A missing, non-integer, or negative `source_gaps` in an older/partial doctor snapshot is likewise unavailable, not zero.

The report also carries the exact safe `capability` projection or `null` when doctor is unavailable, plus `processing_jobs: { capacity, uncompleted, processing, failed, exhausted, pending_grants, max_attempt, legacy_unrecoverable, retry_exhausted_job_ids, next_action }`. Counts are bounded non-negative integers; `retry_exhausted_job_ids` is an ascending content-free list capped at 25 and contains only exact retained doctor-retry targets. If any exhausted job lacks an exact retained range, `next_action=upgrade_runtime`; the ID list is empty whenever that action applies. Capacity defaults to 25 and unknown/malformed next action fails closed to `upgrade_runtime`. In that state, exhausted attention instructs upgrade before retrying rather than confirmation. A configured manifest whose provider gate is disabled reports observer `"pending"`, not `"idle"`; its manifest/provider fingerprints and privacy/schema/pack reasons remain visible in JSON and human output. A missing doctor snapshot reports observer `"unknown"` because status does not reread mutable legacy config.

`projectDatabaseSubsystems` maps a missing `OperationalStatusSnapshot` (no doctor response) to `maintenance/semantic_index: "unknown"`, `raw_events: {state:"unknown", pending:0, source_gaps:0}`, zeroed processing-job counts with capacity 25 and `next_action: "upgrade_runtime"`, and `observer: {state:"unknown"}`.

### 6.4 Attention codes (`StatusAttention`)

| Code | Severity | Trigger | Evidence |
|---|---|---|---|
| `database_missing` | error | `database.state === "missing"` | `status.ts:157-162` — but `collectStatusReport` never assigns `databaseState = "missing"`; this branch is unreachable from the read entry point (see Known gaps) |
| `database_unavailable` | error | `database.state === "unavailable"` (doctor RPC failed after health succeeded, or health failed with a non-`daemon_unavailable` code) | `status.ts:163-169` |
| `daemon_not_running` | warning | `daemon.state === "not_running"` | `status.ts:173-178` |
| `daemon_unavailable` | error | `daemon.state === "unavailable"` | `status.ts:179-185` |
| `viewer_stopped` | warning | no viewer PID record | `status.ts:202-208` |
| `viewer_unreachable` | warning | viewer PID present but local health check failed | `status.ts:210-216` |
| `viewer_pid_malformed` / `viewer_non_loopback` / `viewer_not_ready` / `viewer_unexpected_response` / `viewer_wrong_service` | warning | per `runtime.attention_code` from `observeViewerRuntime` | `status.ts:218-231` |
| `maintenance_failed` | error | `snapshot.maintenance.state === "failed"` | `status.ts:247-252` |
| `maintenance_running` | warning | `snapshot.maintenance.state === "running"` | `status.ts:253-258` |
| `semantic_index_failed` | error | `snapshot.semantic_index.state === "failed"` | `status.ts:260-265` |
| `semantic_index_pending` | warning | `snapshot.semantic_index.state === "pending"` | `status.ts:266-271` |
| `semantic_index_degraded` | warning | `snapshot.semantic_index.state === "degraded"` | `status.ts:272-277` |
| `raw_events_source_gap` | error | bounded `snapshot.raw_events.source_gaps > 0`; fixed message reports only the bounded count and sets `next_action=upgrade_runtime` | `projectDatabaseSubsystems` |
| `raw_events_unavailable` | error | `snapshot.raw_events.available !== true` or `source_gaps` is missing, non-integer, or negative; fixed content-free message and `next_action=upgrade_runtime` | `projectDatabaseSubsystems` |
| `raw_events_failing` | error | `snapshot.raw_events.failed_batches > 0` | `status.ts:282-288` |
| `raw_events_backlogged` | warning | `pending > 0` and not failing | `status.ts:289-296` |
| `processing_jobs_legacy_unrecoverable` | error | `legacy_unrecoverable > 0` | `status.ts` |
| `processing_jobs_exhausted` | error | `exhausted > 0` and no legacy-unrecoverable range | `status.ts` |
| `processing_jobs_backoff` | warning | `failed > 0` and no stronger processing-job condition | `status.ts` |
| `processing_jobs_at_capacity` | warning | `next_action === "wait_for_capacity"` and no stronger processing-job condition | `status.ts` |
| `observer_failed` | error | `configuredObserver && snapshot.observer.failed_batches > 0` | `status.ts:300-306` |
| `observer_backoff` | warning | `configuredObserver && snapshot.observer.backoff_batches > 0` and not failed | `status.ts:307-314` |
| `observer_pending` | warning | frozen manifest is configured while `providerEnabled !== true` | `status.ts` capability projection |
| `legacy_config_ignored` | warning | compatibility `status --config` was supplied; runtime still uses the frozen manifest | `status.ts` compatibility branch |

`report.ok = !attention.some(item => item.severity === "error")` (`status.ts:370`). Attention is capped to `MAX_ATTENTION = 20` items, with `code` sanitized to `[a-z0-9_]` (case-insensitive) and truncated to 64 chars, and `message` truncated to 500 chars (`boundAttention`, `status.ts:74,148-154`).

### 6.5 `status` CLI exit codes

| Exit code | Condition | JSON mode output | Text mode output | Evidence |
|---|---|---|---|---|
| `0` | Report collected successfully — **regardless of `report.ok`**; an unhealthy-but-successfully-collected report still exits `0` | full `OperationalStatusReport` JSON | `renderStatusReport(report)` | `status.ts:424-427` |
| `1` | `collectStatusReport` (or its dependencies) threw | `{"error":"status_failed","message":"Unable to collect operational status"}` | the message, to stderr | `status.ts:428-433` |
| `2` | Positional CLI arguments were supplied (usage error) | `{"error":"usage_error","message":"status accepts only documented options and no positional arguments"}` | the message, to stderr | `status.ts:417-423` |

`status` never signals "unhealthy" (`ok:false`) via its process exit code — only true collection failure (`1`) or CLI misuse (`2`) do. A caller must parse the `ok` field of the JSON/text output to detect degraded operational state, not the exit code.

### 6.6 `hook-runtime.mjs` exit code

`main()` sets `process.exitCode = 2` only for an unrecognized command name (`hook-runtime.ts:193-195`); every other path (including all fail-open fallbacks) leaves the default exit code `0`. The 4 Commander-based hook command actions (`claude-hook-inject`, `codex-hook-inject`, `claude-hook-ingest`, `codex-hook-ingest`) never call `process.exitCode =` at all in the read code — they always exit `0`, even when their internal `try/catch` falls back to a fail-open JSON payload.

---

## 7. Known gaps (documented, not fixed)

1. **`DaemonJobRequestError` has no typed error code; it collapses all 20 validation failures to the wire code `"invalid_request"`** (not `"internal_error"`), verified directly: every job-RPC branch (`daemon-rpc.ts:612,626,634`) wraps it as `new RpcRequestError(error.message)` with no second argument, and `RpcRequestError`'s `code` parameter defaults to `"invalid_request"` (`daemon-rpc.ts:321`). The class is never listed in `dispatchDaemonRpc`'s catch chain (`daemon-rpc.ts:1581-1594`), so it never reaches that function's `"internal_error"` catch-all directly — it always reaches the wire pre-wrapped. **A reimplementation targeting wire compatibility must emit `"invalid_request"` for all 20 job-validation failure conditions**, distinguishable only by message text, not a per-condition code. A reimplementation is free to introduce a richer internal error type as long as the RPC-visible code stays `"invalid_request"` for these cases; it must not emit `"internal_error"` for them, and must not attempt to invent distinct codes per validation rule, since no client of the real daemon can currently observe such a distinction.
2. **`outerWatchdogMs` (3,000 ms claude / 5,000 ms codex) is declared in `HOOK_DELIVERY_BUDGETS` but not enforced by any code under `packages/core` or `packages/cli`.** It is enforced only by the host (Claude Code / Codex CLI) reading the `"timeout"` field of `plugins/claude/hooks/hooks.json` / `plugins/codex/hooks/hooks.json` and killing the hook subprocess itself. A reimplementation that is invoked the same way (as an external hook binary under a host-imposed process timeout) may rely on the host for this budget and need not implement its own outer watchdog; a reimplementation invoked any other way (e.g., embedded, or without a host timeout) must implement this cutoff itself to match observed behavior, since nothing else in the process enforces it.
3. **The pre-dispatch elapsed-deadline check in `dispatchDaemonRpc` (`daemon-rpc.ts:1559-1562`) is not a real handler-execution timeout.** It only fires if request parsing/validation itself consumed the whole per-method deadline before the handler starts; it cannot and does not abort a slow in-flight async handler. The only mechanism that can end a slow request is the socket-level `setTimeout` at the connection layer (`daemon-rpc.ts:1666,1675-1677`), which destroys the connection and returns `deadline_exceeded` without waiting for or canceling the in-flight handler's own database/filesystem work. A reimplementation must preserve this: the deadline is a wall-clock socket timeout on the connection, not a cancellation signal delivered to request-handling logic.
4. **`ObserverState` declares a `"healthy"` variant that `projectDatabaseSubsystems` never assigns** (`status.ts:32` vs. `298-314`) — the type is broader than the function that produces it; it only ever yields `"unconfigured"`, `"unknown"`, `"pending"`, `"idle"`, `"failed"`, or `"backoff"`. (`RawEventsState`'s `"healthy"` is **not** in this category — it is the ordinary case when `raw_events.available` is true and no failures/backlog are present, `status.ts:281`.) A reimplementation does not need to reproduce the unreachable `observer: "healthy"` state; matching the *reachable* subset (`observer`: `unknown`/`unconfigured`/`pending`/`idle`/`backoff`/`failed`) is sufficient for behavioral parity there, since no external caller can currently observe `observer: "healthy"` via `status`.
5. **`DatabaseState` declares `"missing"` and has a dedicated attention message for it** (`status.ts:27`, `156-162`), but `collectStatusReport`'s only assignments to `databaseState` are `"ready"` and `"unavailable"` (success/failure of the doctor RPC) or the initial `"unknown"` — there is no code path in the read `status.ts` that sets `"missing"`. A reimplementation is not required to produce `database_missing`; it is dead in the current `status` implementation.
6. **The `not_implemented` fallback branch in `handleMethod`** is unreachable for every one of the 29 registered `RPC_METHODS` under the `isRpcMethod()` gate. A reimplementation is not required to reproduce this shape for any currently-registered method; it exists only as defensive code for a method that could theoretically be added without a matching branch.
7. **`mapPeerConnectError`'s `peer_denied` catch-all branch (`daemon-rpc-contract.ts:120`) is dead code on the `callDaemonRpc` path.** The socket `error` handler applies the mapping only to `EACCES`, `ECONNREFUSED`, and `ENOENT`, and calls `finish(peerError)` for everything else (`daemon-rpc-contract.ts:170-181`), so an `ENOTSOCK` (regular file at the socket path), `EPERM`, or any other errno rejects with the original `Error` and never becomes a `TypedRpcError`. The catch-all is reachable only by calling the exported `mapPeerConnectError` directly, which no production caller does. A reimplementation must preserve the two-way mapping (`EACCES` → `peer_denied`, `ECONNREFUSED`/`ENOENT` → `daemon_unavailable`) **and** the "everything else stays an untyped error" behavior — widening the mapping to a `peer_denied` catch-all would change both the reported status and the fail-open decision for a malformed socket path.
8. **`GET /v1/checkpoints` is a permanent stub** returning `{ checkpoints: [] }` regardless of request body (`daemon-rpc.ts:568`), despite being a fully specified RPC method with required-field validation (none, per Table 1.4) that a client could reasonably expect to be functional. A reimplementation must preserve the empty-array stub response for wire compatibility, not implement real checkpoint listing, unless the contract is explicitly revised in a later version.
9. **`idempotency_conflict` is allowlisted in `runBackup`'s sanitized error-code mapping (`daemon-operations.ts:500-509`) but is not reachable through the backup-create/backup-restore code paths that feed it** — that code is only produced by `submit()`'s idempotency check for `export`/`import` kinds (`:413-419`), which never call `runBackup`. It is dead defensive code in that specific allowlist. A reimplementation may keep or drop this defensive branch; no observable behavior depends on it being present.
10. **`BACKUP_REQUIRED_JOB_KINDS` (8 kinds, `daemon-jobs.ts:148-157`) is declared but has no observable gating effect distinct from the maintenance classification itself in the reachable code paths.** The backup guard it feeds is `if (BACKUP_REQUIRED_JOB_KINDS.has(row.kind) || args.internal !== true)` (`daemon-jobs.ts:756`); since `internal` is rejected as an unknown argument by `validateJobArgs` for every externally-submitted job (§3.3), the right-hand disjunct alone already forces a backup for every non-dry-run maintenance-classified job reached via `submit()`, making the left-hand membership test redundant on that path. The set only matters for jobs carrying `args.internal === true`, and the only such jobs (`scanInternalBackfills`'s six kinds, `daemon-jobs.ts:653-682`) are disjoint from it. A reimplementation must still reproduce the *observable* rule — backup required before every non-dry-run maintenance-classified job submitted externally, not required for the six internally-scheduled backfill kinds — but is not required to reproduce `BACKUP_REQUIRED_JOB_KINDS` as a distinct 8-kind gate, since no reachable code path currently lets that set's membership change the outcome.

---

## 8. Rust parity requirements

### 8.1 Must reproduce byte-for-byte or exact-code-for-code (wire/observable contract)

- The newline-delimited JSON framing over the Unix domain socket, including the bare `STOP <nonce>` control line and its two literal replies (§1.1).
- The handshake fields and their exact failure code (`protocol_mismatch`) for any mismatch of `local_api_version`, `normalized_schema_version`, or `capability_hash` (§1.2) — including that `capability_hash` is a hash of the exact `RPC_METHODS` list, so any method-set change must be treated as a protocol-breaking change requiring a new contract version.
- The complete set of 29 `RPC_METHODS` strings and their required-field lists (Table 1.4), including the `GET /v1/checkpoints` empty-array stub (Known gap 8) and the maintenance-blocked method set (§1.5).
- Every typed error code in §2.1–2.7 and its `retryable` flag, for the exact trigger conditions cited — including the `DaemonJobRequestError` → `"invalid_request"` collapse (Known gap 1) and the `POST /v1/events` vs `POST /v1/events/batch` asymmetry for `idempotency_conflict` (§2.6).
- `RPC_MAX_BYTES` (32 KiB) as the server's per-connection request cap (§1.3). The response side is **not** part of the wire contract: `callDaemonRpc` has no default `maxResponseBytes`, the MCP client supplies none, and only the hook client chooses a cap (32 KiB, raised to 256 KiB for `POST /v1/context/pack`). A reimplemented client must keep that per-client freedom — imposing 32 KiB as a global default would reject MCP responses the current client accepts.
- `RPC_DEFAULT_DEADLINE_MS` (2,000 ms) and the 30-minute backup-method deadline, enforced as a real socket-level timeout that can fire independent of handler completion, not merely as a pre-dispatch check (§1.3, Known gap 3).
- The two-tier operations error surface: raw journal codes via `GET /v1/operations/:id` vs. the 5-code sanitized set via `POST /v1/backup/create|restore` (§4.3).
- All 28 job kinds, their argument allowlists and bounds, the maintenance (18 kinds) / dry-run-eligible (11 kinds) classifications, `maxAttempts: 1` with no automatic retry, the 512 KiB result-size cap, and the fixed job-failure message string `"Daemon job failed; submit a new job to retry."` (§3, §2.7) for all three `error_code` values (`redaction_degraded`, `job_failed`, `daemon_restarted`) — including the startup-time transition of orphaned `queued`/`running` jobs to `failed`/`daemon_restarted` (§2.7, §3.6).
- The observable backup-before-execution rule: every non-dry-run maintenance-classified job submitted through the external RPC path requires a verified online backup before execution, regardless of whether its kind is in the 8-kind `BACKUP_REQUIRED_JOB_KINDS` set — that set has no gating effect distinct from the maintenance classification itself in the reachable code paths (§3.3, Known gap 10). The six internally-scheduled backfill kinds never require a backup.
- Hook fail-open behavior exactly as tabulated in §5.3 and §5.4, per adapter — in particular, which of the 5 commands skip calling delivery entirely versus which always call it and rely on delivery's internal short-circuit, and the codex-only `hook_event_name` gate. A reimplementation of these adapters must reproduce the same net delivery outcome (no RPC event sent) for each skip condition; it is not required to preserve the internal code-path distinction between "returned before calling deliver" and "called deliver, which short-circuited," since that distinction is not observable from outside the process in the current code (Known gap discussion in §5.3 applies only to *direct* invocation of the exported functions bypassing `hook-runtime.ts`'s own `CODEMEM_PLUGIN_IGNORE` check, which is out of scope for a socket/RPC-level reimplementation).
- The five `HOOK_DELIVERY_BUDGETS` fields that are actually enforced in-process (`clientHardCapMs`, `rpcCutoffMs`, `spoolReserveMs`, `spoolLockWaitMs`, `fsyncMarginMs`), per agent, at their exact millisecond values (§5.5).
- The `native_cli_version` literals per agent (§1.2) if the reimplementation needs to interoperate on the wire with a TypeScript daemon peer.
- `status`'s exit-code contract: `0` even when `ok:false`, `1` only for report-collection failure, `2` only for a usage error, and that operational health must be read from the `ok` field, never inferred from the exit code (§6.5).
- The attention-code table (§6.4), its severity levels, the `ok = no error-severity attention` rule, and the `MAX_ATTENTION = 20` / 64-char code / 500-char message truncation (`boundAttention`).
- The `daemon_unavailable` vs. other-health-error-code branch in `status`'s daemon-state resolution (§6.2), and that `daemon_unavailable`/`peer_denied` are client-synthesized codes the daemon itself never emits (§2.2).

### 8.2 Free to differ (implementation detail, not part of the observable contract)

- The internal Node.js `Worker`-thread supervision mechanism in `hook-runtime.ts` (§5.2) — any mechanism that enforces `clientHardCapMs` and falls back to the same output on timeout/crash is equivalent; Rust need not use OS threads/worker isolation the same way.
- Whether `outerWatchdogMs` is enforced by the reimplementation itself or left to an external host process timeout (Known gap 2) — this is a deployment-topology decision, not an internal-behavior requirement, provided the effective observed cutoff matches.
- The exact SQL used to compute `degradedDeliveries`, `pending`, `failed_batches`, `backoff_batches`, etc. in `operational-status.ts` — only the resulting state values and thresholds (`§6.3`) are part of the contract, not the queries that produce them.
- The unreachable enum member `ObserverState`'s `"healthy"` and `DatabaseState`'s `"missing"` (note: `RawEventsState`'s `"healthy"` is reachable and normal — it must be reproduced, not omitted) and the dead `not_implemented`/`idempotency_conflict`-in-backup-allowlist branches (Known gaps 4, 5, 6, 9) — a reimplementation may omit the genuinely-unreachable ones without any observable behavioral difference from the current TypeScript daemon.
- Internal job scheduling implementation (in-memory promise queue vs. any other single-flight scheduler), provided the observable ordering guarantee holds: jobs run strictly one at a time in submission order, and a maintenance-classified non-dry-run job blocks the 15 maintenance-blocked RPC methods for its duration (§1.5, §3.3).
- The internal representation of the operation/job journals on disk (JSON files under `<dataDir>/control/operations/`, SQLite rows for jobs) — only the RPC-visible state machine and codes are part of the contract, not the storage format, unless a separate storage-compatibility contract says otherwise.
