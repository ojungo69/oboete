import { spawn as nodeSpawn, type spawn } from 'node:child_process';
import { lstatSync, readdirSync, realpathSync, statSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';
import { setTimeout as timerSleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

import {
  PRESET_CATALOG,
  consentHash,
  consentMatches,
  consentTuple,
  isPaused,
  loadConfig,
  readCredentials,
  type OboeteConfig,
  type PresetName,
} from '../config.js';
import { openDatabase, isBusyError } from '../db/open.js';
import { createAncestorCache } from '../injection/staleness.js';
import { appendLog, appendLogQuietly, credentialValues, errorCode } from '../log.js';
import { refreshWorkersAiCatalog } from '../observer/catalog.js';
import { sessionSummary, type DegradedReason } from '../observer/classify.js';
import { resolveModel } from '../observer/providers.js';
import { clearWorkerStop, isWorkerStopped, writeWorkerStop } from '../pause.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from '../paths.js';
import { detectSync, type DetectorInput, type DetectorResult } from '../privacy/detect.js';
import {
  classifyPending,
  DUE_SOURCE_SQL,
  RECLAIM_AFTER_MS,
  SUMMARIZABLE_ROW_SQL,
  createBatches,
  hasBatchableSources,
  isSummarizableRow,
  reclaimStale,
  reconcilePendingDestinations,
  type BatchRow,
  type RawEventRow,
} from './batches.js';
import { updateBatchCitations } from './citations.js';
import { reclassifyImported } from './imported.js';
import { assertLease, claimLease, heartbeat, releaseLease, rotateLease, transactionImmediate } from './lease.js';
import {
  LeaseLostError,
  processBatch,
  type BatchDeps,
  type BatchResult,
} from './observe-batch.js';
import {
  checkpoint,
  cleanupPiAck,
  purgeExpiredEvents,
  runtimeStateSet,
} from './purge.js';
import { recoverSpool } from './spool-recovery.js';

const DEFAULT_HEARTBEAT_MS = 2_000;
const DEFAULT_MAX_RUN_MS = 20 * 60 * 1_000;
const BUSY_RETRY_MS = 200;
const RESIDENT_POLL_MS = 2_000;
/** An idle resident still opens an epoch this often so expiry and reclaim keep running. */
const MAINTENANCE_MS = 60_000;
const OBSERVE_USAGE = 'Usage: oboete observe [--reprocess-source <source-id>] [--resident] [--stop]\n';

export type ObserveDeps = {
  now: () => number;
  fetch: typeof globalThis.fetch;
  spawn: typeof spawn;
  detect: (input: DetectorInput) => Promise<DetectorResult>;
  env: NodeJS.ProcessEnv;
  heartbeatMs: number;
  maxRunMs: number;
  /** Test seam for the A11 crash window after a response and before its fenced apply. */
  applyHook: () => void | Promise<void>;
  writeError: (text: string) => void;
  /** Monotonic elapsed ms from an arbitrary origin; idle and epoch budgets use this. */
  elapsedMs: () => number;
  sleep: (ms: number, signal?: AbortSignal) => Promise<void>;
  /** Resolved engine artifact; tests point this at a file they can mutate. */
  engineArtifact: string;
};

type Counts = {
  recovered: number;
  classified: number;
  reclassified: number;
  batches: number;
  applied: number;
  fallback: number;
  purged: number;
};


/**
 * Whether the run must exit 3 (contracts/cli.md: storage unavailable). A constraint violation is a
 * defect in what was written, not an unwritable data directory, so it ends the run with the ordinary
 * worker error and the next run starts again rather than reporting broken storage on every pass.
 */
export function isStorageError(error: unknown): boolean {
  if (isBusyError(error)) return true;
  if (typeof error !== 'object' || error === null) return false;
  // SQLITE_CONSTRAINT is 19; its extended codes (UNIQUE 2067, FOREIGN KEY 787, ...) keep it in the
  // low byte.
  if ('errcode' in error && typeof error.errcode === 'number' && (error.errcode & 0xff) === 19) {
    return false;
  }
  if ('errcode' in error && typeof error.errcode === 'number') return true;
  if (!('code' in error) || typeof error.code !== 'string') return false;
  return (
    error.code.startsWith('ERR_SQLITE') ||
    error.code.startsWith('SQLITE_') ||
    ['EACCES', 'EBADF', 'EIO', 'EMFILE', 'ENFILE', 'ENOSPC', 'EROFS'].includes(error.code)
  );
}

function defaultSleep(ms: number, signal?: AbortSignal): Promise<void> {
  return timerSleep(ms, undefined, { signal });
}

async function retryBusy<T>(work: () => T | Promise<T>): Promise<T> {
  try {
    return await work();
  } catch (error) {
    if (!isBusyError(error)) throw error;
    await defaultSleep(BUSY_RETRY_MS);
    return await work();
  }
}


function ownsLease(db: DatabaseSync, token: string): boolean {
  return db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token === token;
}

function adoptPendingBatches(db: DatabaseSync, token: string, now: number): boolean {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return false;
    }
    db.prepare(
      `UPDATE observation_batches
       SET owner_token = ?, claimed_at = COALESCE(claimed_at, ?)
       WHERE state = 'pending' AND owner_token IS NOT ?`,
    ).run(token, now, token);
    return true;
  });
}

function pendingBatches(db: DatabaseSync): BatchRow[] {
  return db
    .prepare(
      `SELECT id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, claimed_at
       FROM observation_batches WHERE state = 'pending'
       ORDER BY claimed_at,
         CASE destination WHEN 'remote_observer' THEN 0 WHEN 'local_observer' THEN 0 ELSE 1 END,
         id`,
    )
    .all() as unknown as BatchRow[];
}


/**
 * contracts/observer.md call policy 6 and R8: the consent tuple is recomputed from the configuration
 * as it is on disk before every reservation and again immediately before every send, not from the
 * snapshot the run started with -- a run lasts up to twenty minutes (DEFAULT_MAX_RUN_MS). The live
 * tuple must still be the one consent records and must still be the destination this run reserved
 * against, so choosing another preset, choosing `none` or revoking consent stops the next batch. A
 * file that cannot be read or parsed is a mismatch, never a licence to keep sending. The cost is two
 * re-reads of one small TOML file per attempt, so nothing is cached.
 */
function liveConsentOk(paths: OboetePaths, env: NodeJS.ProcessEnv, startedHash: string): boolean {
  try {
    const live = loadConfig(paths);
    return consentMatches(live, env) && consentHash(consentTuple(live, env)) === startedHash;
  } catch {
    return false;
  }
}




function pendingSummaries(db: DatabaseSync, token: string, now: number): string[] {
  return db
    .prepare(
      `SELECT id FROM sessions s
       WHERE status = 'ended' AND summary_state = 'pending'
         AND NOT EXISTS (
           SELECT 1 FROM observation_batches b
           WHERE b.session_id = s.id AND b.state NOT IN ('applied', 'fallback')
         )
         AND NOT EXISTS (SELECT 1 FROM raw_events WHERE session_id = s.id AND batch_id IS NULL
           AND ${DUE_SOURCE_SQL} AND ${SUMMARIZABLE_ROW_SQL})
         AND (s.summary_updated_at IS NULL OR EXISTS (
           SELECT 1 FROM observation_batches b
           WHERE b.session_id = s.id AND b.completed_at > s.summary_updated_at
         ))
       ORDER BY ended_at, id`,
    )
    .all(now, token)
    .map((row) => String(row.id));
}

/** Idle probe: pass a token that matches no attempt (the empty string). */
export function queueIsEmpty(db: DatabaseSync, paths: OboetePaths, token: string, now: number): boolean {
  if (
    db.prepare("SELECT 1 AS work FROM observation_batches WHERE state IN ('pending', 'running') LIMIT 1").get() !==
    undefined
  ) {
    return false;
  }
  if (hasBatchableSources(db, token, now)) return false;
  try {
    if (
      readdirSync(paths.spool, { withFileTypes: true }).some(
        (entry) => entry.isFile() && entry.name.endsWith('.json'),
      )
    ) {
      return false;
    }
  } catch {
    // R6: a missing spool directory contains no queued entry.
  }
  return pendingSummaries(db, token, now).length === 0;
}

function logEnd(paths: OboetePaths, result: Counts, exit: number, reason: string): number {
  try {
    appendLog(paths.observeLog, 'info', 'run end', { ...result, exit, reason });
    return exit;
  } catch {
    return 3;
  }
}

function releaseForExit(
  db: DatabaseSync,
  paths: OboetePaths,
  token: string,
  now: number,
  result: Counts,
  reason: string,
  releaseWithPending: boolean,
): 'released' | 'kept' | 'lost' {
  return releaseLease(db, token, () => {
    // Inside the transaction, after `releaseLease` has confirmed this row still carries this token:
    // a process suspended long enough to lose the lease must leave the stop request for the owner
    // that replaced it. Only a resident shutdown reaches here with `stopped`.
    if (reason === 'stopped') clearWorkerStop(paths);
    const empty = releaseWithPending || queueIsEmpty(db, paths, token, now);
    if (empty) runtimeStateSet(db, 'last_run', JSON.stringify({ at: now, reason, ...result }), now);
    return empty;
  });
}

function observeDependencies(overrides: Partial<ObserveDeps>): ObserveDeps {
  return {
    now: overrides.now ?? Date.now,
    fetch: overrides.fetch ?? globalThis.fetch,
    spawn: overrides.spawn ?? nodeSpawn,
    detect: overrides.detect ?? detectSync,
    env: overrides.env ?? process.env,
    heartbeatMs: overrides.heartbeatMs ?? DEFAULT_HEARTBEAT_MS,
    maxRunMs: overrides.maxRunMs ?? DEFAULT_MAX_RUN_MS,
    applyHook: overrides.applyHook ?? (() => undefined),
    writeError: overrides.writeError ?? ((text) => { process.stderr.write(text); }),
    elapsedMs: overrides.elapsedMs ?? (() => performance.now()),
    sleep: overrides.sleep ?? defaultSleep,
    engineArtifact: overrides.engineArtifact ?? engineArtifactPath(),
  };
}

type FileIdentity = {
  exists: boolean;
  dev: number;
  ino: number;
  size: number;
  mtimeMs: number;
  target: string | null;
};

function readFileIdentity(path: string): FileIdentity | 'unreadable' {
  try {
    const link = lstatSync(path);
    const target = link.isSymbolicLink() ? realpathSync(path) : null;
    const st = statSync(path);
    return {
      exists: true,
      dev: st.dev,
      ino: st.ino,
      size: st.size,
      mtimeMs: st.mtimeMs,
      target,
    };
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'ENOENT'
      ? { exists: false, dev: 0, ino: 0, size: 0, mtimeMs: 0, target: null }
      : 'unreadable';
  }
}

function sameIdentity(left: FileIdentity, right: FileIdentity): boolean {
  return (
    left.exists === right.exists &&
    left.dev === right.dev &&
    left.ino === right.ino &&
    left.size === right.size &&
    left.mtimeMs === right.mtimeMs &&
    left.target === right.target
  );
}

function engineArtifactPath(): string {
  return fileURLToPath(import.meta.url);
}

type ConfigStamp = { identity: FileIdentity; idleExitMs: number };

/** One parse per control check: the stamp carries the settings the check needs. */
function readConfigStamp(paths: OboetePaths): ConfigStamp | 'unreadable' {
  let idleExitMs: number;
  try {
    idleExitMs = loadConfig(paths).worker.idle_exit_ms;
  } catch {
    return 'unreadable';
  }
  const identity = readFileIdentity(paths.config);
  return identity === 'unreadable' ? 'unreadable' : { identity, idleExitMs };
}

function nextWakeDelay(db: DatabaseSync, now: number): number {
  let delay = RESIDENT_POLL_MS;
  const retry = db
    .prepare(
      `SELECT MIN(retry_after) AS t FROM raw_events
       WHERE processing_state = 'waiting' AND retry_after IS NOT NULL AND retry_after > ?`,
    )
    .get(now)?.t;
  if (typeof retry === 'number') delay = Math.min(delay, Math.max(1, retry - now));
  const claimed = db
    .prepare("SELECT MIN(claimed_at) AS t FROM observation_batches WHERE state = 'running'")
    .get()?.t;
  if (typeof claimed === 'number') {
    const reclaimAt = claimed + RECLAIM_AFTER_MS;
    if (reclaimAt > now) delay = Math.min(delay, Math.max(1, reclaimAt - now));
  }
  return delay;
}

function countDelta(before: Counts, after: Counts): Counts {
  return {
    recovered: after.recovered - before.recovered,
    classified: after.classified - before.classified,
    reclassified: after.reclassified - before.reclassified,
    batches: after.batches - before.batches,
    applied: after.applied - before.applied,
    fallback: after.fallback - before.fallback,
    purged: after.purged - before.purged,
  };
}

function openObserveDatabase(paths: OboetePaths): DatabaseSync | null {
  let db: DatabaseSync | null = null;
  try {
    ensureDirectories(paths);
    appendLog(paths.observeLog, 'info', 'run start', { pid: process.pid });
    db = openDatabase({ path: paths.db, timeoutMs: 2_000 }).db;
  } catch (error) {
    try {
      appendLog(paths.observeLog, 'error', 'storage unavailable', { code: errorCode(error) });
    } catch {
      // contracts/cli.md: the original storage failure still requires exit 3.
    }
    if (db?.isOpen) db.close();
    return null;
  }

  return db;
}

/** The lease token, or the exit code this run ends with; tagged so a caller cannot confuse them. */
type LeaseClaim = { ok: true; token: string } | { ok: false; exit: number };

function claimObserveLease(
  db: DatabaseSync, paths: OboetePaths, result: Counts, startedAt: number,
): LeaseClaim {
  let token: string | null;
  try {
    token = claimLease(db, { pid: process.pid, now: startedAt });
  } catch (error) {
    try {
      appendLog(paths.observeLog, 'error', 'lease claim failed', { code: errorCode(error) });
    } catch {
      // contracts/cli.md: either failure is a storage exit.
    }
    db.close();
    return { ok: false, exit: logEnd(paths, result, 3, 'storage_error') };
  }
  if (token === null) {
    db.close();
    return { ok: false, exit: logEnd(paths, result, 0, 'another_worker') };
  }

  return { ok: true, token };
}

function resolveObserveModel(config: OboeteConfig): { preset: PresetName | 'none'; model: string } {
  let resolved: { preset: PresetName | 'none'; model: string };
  try {
    resolved = resolveModel(config);
  } catch {
    resolved = { preset: config.observer.preset, model: '' };
  }
  return resolved;
}

function initialProviderFailure(
  resolved: { preset: PresetName | 'none'; model: string },
  credentials: ReturnType<typeof readCredentials> | null,
  config: OboeteConfig,
  env: NodeJS.ProcessEnv,
): DegradedReason | null {
  let initialProviderReason: DegradedReason | null;
  if (resolved.preset === 'none' || credentials?.present !== true || resolved.model === '') {
    initialProviderReason = 'no_provider';
  } else if (!consentMatches(config, env)) {
    initialProviderReason = 'consent_changed';
  } else {
    initialProviderReason = null;
  }
  return initialProviderReason;
}

/**
 * A run that fell back reports it as exit 1, unless the storage exit already won or the run ended
 * for a reason that is not about the provider: a lost lease, the max-run yield, or a batch error.
 */
function reportsFallbackExit(exit: number, usedFallback: boolean, endReason: string): boolean {
  return exit !== 3 && usedFallback
    && endReason !== 'lease_lost' && endReason !== 'max_run' && endReason !== 'batch_error';
}

/**
 * Folds one batch's outcome into the run counters, and reports the two run-level facts the caller
 * latches: the lease is gone, and a fallback happened for a reason other than there being no
 * provider configured. Neither flag is ever cleared, so the caller only ever sets them.
 */
function recordBatchResult(
  result: Counts,
  batchResult: BatchResult,
): { leaseLost: boolean; usedFallback: boolean } {
  if (batchResult.state === 'lease_lost') return { leaseLost: true, usedFallback: false };
  if (batchResult.state === 'requeued' || batchResult.state === 'done') return { leaseLost: false, usedFallback: false };
  result.batches += 1;
  result[batchResult.state] += 1;
  return {
    leaseLost: false,
    usedFallback: batchResult.state === 'fallback' && batchResult.reason !== 'rule_based',
  };
}

/** The run counters a worker run starts from; every phase adds to these. */
function emptyCounts(): Counts {
  return {
    recovered: 0,
    classified: 0,
    reclassified: 0,
    batches: 0,
    applied: 0,
    fallback: 0,
    purged: 0,
  };
}

const COOPERATIVE_REASONS = new Set([
  'paused',
  'stopped',
  'config_changed',
  'upgraded',
  'idle_exit',
  'lease_lost',
  'signal',
]);

async function observeLifecycle(
  overrides: Partial<ObserveDeps>,
  options: { reprocessSource?: string; resident: boolean },
): Promise<number> {
  const { reprocessSource, resident } = options;
  const deps = observeDependencies(overrides);
  const paths = oboetePaths(resolveHome(deps.env));
  if (isPaused(paths)) {
    if (reprocessSource === undefined) return 0;
    deps.writeError('Oboete is paused. Resume it before requesting source reprocessing.\n');
    return 1;
  }

  const result = emptyCounts();
  const opened = openObserveDatabase(paths);
  if (opened === null) return 3;
  const db: DatabaseSync = opened;

  const startedAt = deps.now();
  const deadline = startedAt + Math.max(0, deps.maxRunMs);
  const claim = claimObserveLease(db, paths, result, startedAt);
  if (!claim.ok) {
    if (reprocessSource !== undefined && claim.exit === 0) {
      deps.writeError('Another worker is running. Retry this reprocessing command after it finishes.\n');
      return 1;
    }
    return claim.exit;
  }
  let token = claim.token;

  let leaseLost = false;
  const heartbeatTimer = setInterval(heartbeatLease, Math.max(1, deps.heartbeatMs));
  heartbeatTimer.unref();

  let usedFallback = false;
  let catalogChecked = false;
  let yieldAfterPass = false;
  let exit: number | undefined;
  let endReason = 'empty';
  let stopReason: string | undefined;
  let wakeSleep: (() => void) | undefined;
  let epochDeadlineElapsed = deps.elapsedMs() + Math.max(0, deps.maxRunMs);
  const enginePath = deps.engineArtifact;
  const startedEngine = readFileIdentity(enginePath);
  const startedConfig = readConfigStamp(paths);
  let lastCaptureMark = 0;
  let lastActivityElapsed = deps.elapsedMs();

  function onSignal(): void {
    stopReason ??= 'signal';
    wakeSleep?.();
  }

  function recordRunFailure(error: unknown, db: DatabaseSync, token: string): void {
    let logFailed = false;
    try {
      appendLog(paths.observeLog, 'error', 'worker step failed', { code: errorCode(error) });
    } catch {
      logFailed = true;
    }
    const storageError = logFailed || isStorageError(error);
    exit = storageError ? 3 : 0;
    endReason = storageError ? 'storage_error' : 'worker_error';
    if (!resident) {
      try {
        // Inside the guard for the same reason as the resident's shutdown: on a failing handle the
        // probe itself throws, and this runs while the storage outcome is being recorded.
        if (ownsLease(db, token)) releaseForExit(db, paths, token, deps.now(), result, endReason, false);
      } catch {
        // R6: preserve the original storage outcome; a held lease becomes stale for takeover.
      }
    }
  }

  function heartbeatLease(): void {
    try {
      if (!heartbeat(db, token, deps.now())) leaseLost = true;
    } catch (error) {
      appendLogQuietly(paths.observeLog, 'warn', 'heartbeat failed', { code: errorCode(error) });
    }
  }

  function timedOut(): boolean {
    return resident ? deps.elapsedMs() >= epochDeadlineElapsed : deps.now() >= deadline;
  }

  function pollIdleActivity(): void {
    // `data_version` changes when another connection commits and never for this process's own
    // writes, so a capture — always another process — is always seen, while this resident's own
    // maintenance cannot look like one. The two obvious alternatives both hide a capture:
    // timestamps, because `last_captured_at` is written clamped and a maximum over rows hides a
    // batch that completes after a backward correction; and `MAX(rowid)`, because a purge that
    // deletes the newest row frees exactly the rowid the next insert takes. Completed processing is
    // counted in the epoch below, since the lease owner is the only process that completes a batch.
    const version = db.prepare('PRAGMA data_version').get()?.data_version;
    const mark = typeof version === 'number' ? version : 0;
    if (mark !== lastCaptureMark) {
      lastCaptureMark = mark;
      lastActivityElapsed = deps.elapsedMs();
    }
  }

  function controlReason(): string | undefined {
    if (stopReason !== undefined) return stopReason;
    if (!resident) return undefined;
    if (isPaused(paths)) return 'paused';
    if (isWorkerStopped(paths)) return 'stopped';
    const engineNow = readFileIdentity(enginePath);
    if (
      startedEngine === 'unreadable' ||
      engineNow === 'unreadable' ||
      !engineNow.exists ||
      !sameIdentity(startedEngine, engineNow)
    ) {
      return 'upgraded';
    }
    const configNow = readConfigStamp(paths);
    if (configNow === 'unreadable' || startedConfig === 'unreadable') return 'config_changed';
    if (!sameIdentity(startedConfig.identity, configNow.identity)) return 'config_changed';
    if (!ownsLease(db, token)) return 'lease_lost';
    pollIdleActivity();
    if (
      deps.elapsedMs() - lastActivityElapsed >= configNow.idleExitMs &&
      queueIsEmpty(db, paths, '', deps.now())
    ) {
      return 'idle_exit';
    }
    return undefined;
  }

  async function interruptibleSleep(ms: number): Promise<void> {
    if (ms <= 0 || stopReason !== undefined) return;
    const sleepAbort = new AbortController();
    try {
      await Promise.race([
        deps.sleep(ms, sleepAbort.signal),
        new Promise<void>((resolve) => { wakeSleep = resolve; }),
      ]);
    } finally {
      sleepAbort.abort();
      wakeSleep = undefined;
    }
  }

  async function observeClaimedLease(db: DatabaseSync): Promise<void> {
    const busyWaitMs = Math.min(Math.max(1, deps.heartbeatMs), BUSY_RETRY_MS);
    async function recoverAndClassify(): Promise<boolean> {
      const recovered = recoverSpool(db, paths, token, deps.now());
      result.recovered += recovered.inserted;
      // A spooled capture is still a capture, and this is the one that arrives on the resident's own
      // connection, which `data_version` deliberately does not see. The reset belongs here rather
      // than at the end of the epoch: the next pass checks the controls, and a recovered
      // `session_start` leaves nothing queued to hold the resident past that check.
      if (recovered.inserted > 0) lastActivityElapsed = deps.elapsedMs();
      if (leaseLost || !ownsLease(db, token)) return true;

      const classified = await retryBusy(() => classifyPending(db, token, deps.now(), detect));
      result.classified += classified.examined;
      if (classified.leaseLost || leaseLost) return true;

      const reclassified = await retryBusy(() => reclassifyImported(db, token, deps.now, deps.detect,
        { home: paths.home, env: deps.env, stop: timedOut }));
      result.reclassified += reclassified.examined;
      if (reclassified.leaseLost || leaseLost) return true;

      return false;
    }

    async function maintainQueue(): Promise<boolean> {
      const reclaimed = await retryBusy(() => reclaimStale(db, token, deps.now()));
      if (reclaimed.leaseLost || leaseLost) return true;

      const reconciled = await retryBusy(() =>
        reconcilePendingDestinations(db, token, deps.now(), presetEntry?.egress ?? 'none'));
      if (reconciled.leaseLost || leaseLost) return true;

      const purged = await retryBusy(() => purgeExpiredEvents(db, token, deps.now(), { clock: deps.now }));
      result.purged += purged.deleted;
      if (purged.leaseLost || leaseLost) return true;

      await retryBusy(() => cleanupPiAck(db, token, paths.piAck, deps.now()));
      if (leaseLost || !ownsLease(db, token)) return true;

      const created = await retryBusy(() =>
        createBatches(db, token, deps.now(), { preset: presetEntry?.egress ?? 'none' }),
      );
      if (created.leaseLost || leaseLost) return true;

      return false;
    }

    async function refreshCatalog(): Promise<boolean> {
      if (
        !catalogChecked &&
        resolved.preset === 'workers-ai' &&
        credentials?.present === true
      ) {
        catalogChecked = true;
        await retryBusy(() =>
          refreshWorkersAiCatalog(db, {
            env: deps.env, now: deps.now(),
            fetchImpl: async (...args) => {
              const reason = batchDeps.shouldStop();
              if (reason !== undefined) {
                stopReason = reason;
                throw new Error('worker stopped');
              }
              return await deps.fetch(...args);
            },
          }),
        );
        if (stopReason !== undefined || leaseLost || !ownsLease(db, token)) return true;
      }

      return false;
    }

    async function processPendingBatch(batch: BatchRow): Promise<void> {
      async function checkpointBatch(): Promise<void> {
        try {
          await retryBusy(() => checkpoint(db, 'PASSIVE'));
          if (
            batchResult !== null &&
            !(await updateBatchCitations(
              db,
              token,
              String(batch.repo_id ?? ''),
              batchResult.memoryIds,
              ancestorCache,
              deps,
            ))
          ) {
            leaseLost = true;
          }
        } catch (error) {
          if (isStorageError(error)) throw error;
          batchError = batchError ?? error;
          yieldAfterPass = true;
        }
      }

      function logBatch(): void {
        appendLog(paths.observeLog, batchError === undefined ? 'info' : 'error', 'batch', {
          id: batch.id,
          state: batchResult?.state ?? 'error',
          reason: batchResult?.reason ?? (batchError === undefined ? 'none' : errorCode(batchError)),
          ...(batchResult?.detail === undefined ? {} : { detail: batchResult.detail.split(/[\r\n]/)[0] }),
        });
      }

      let batchResult: BatchResult | null = null;
      let batchError: unknown;
      try {
        batchResult = (await processBatch({
          db, token, batch, config, deps: batchDeps, detect, providerState,
          initialProviderReason, resolved, consentOk,
        }));
        if (batchResult.state === 'done') {
          stopReason = batchResult.reason;
          return;
        }
        const recorded = recordBatchResult(result, batchResult);
        if (recorded.leaseLost) leaseLost = true;
        if (recorded.usedFallback) usedFallback = true;
      } catch (error) {
        if (error instanceof LeaseLostError) leaseLost = true;
        else {
          batchError = error;
          yieldAfterPass = true;
        }
      }

      if (!leaseLost) await checkpointBatch();

      logBatch();

    }

    async function processPendingBatches(): Promise<void> {
      const batches = pendingBatches(db);
      for (const batch of batches) {
        if (leaseLost || stopReason !== undefined || timedOut()) break;
        const reason = controlReason();
        if (reason !== undefined) {
          stopReason = reason;
          break;
        }
        await processPendingBatch(batch);
      }
    }

    async function summarizeSession(sessionId: string): Promise<boolean> {
      try {
        const summary = await retryBusy(() => sessionSummary(db, token, sessionId, deps.now()));
        if (summary.state === 'lease_lost') {
          leaseLost = true;
          return true;
        }
      } catch (error) {
        if (isStorageError(error)) throw error;
        appendLog(paths.observeLog, 'error', 'session summary failed', {
          session: sessionId,
          code: errorCode(error),
        });
        yieldAfterPass = true;
      }
      return false;
    }

    async function summarizePendingSessions(): Promise<void> {
      for (const sessionId of pendingSummaries(db, token, deps.now())) {
        if (leaseLost || timedOut() || stopReason !== undefined) break;
        if (await summarizeSession(sessionId)) break;
      }
    }

    async function releaseEmptyPass(): Promise<boolean> {
      const released = releaseForExit(db, paths, token, deps.now(), result, 'empty', false);
      if (released === 'lost') {
        leaseLost = true;
        return true;
      }
      if (released === 'released') {
        await retryBusy(() => checkpoint(db, 'TRUNCATE'));
        return true;
      }
      await deps.sleep(busyWaitMs);
      return false;
    }

    /** Resident-only: the epoch ends when the queue is drained, and waits out a busy row. */
    async function drainedOrWait(): Promise<boolean> {
      if (queueIsEmpty(db, paths, token, deps.now())) return true;
      await interruptibleSleep(busyWaitMs);
      return false;
    }

    async function releaseMaxRun(): Promise<void> {
      // FR-009: a bounded worker releases even with queued work so the next hook can respawn it.
      const released = releaseForExit(db, paths, token, deps.now(), result, endReason, true);
      if (released === 'released') await retryBusy(() => checkpoint(db, 'TRUNCATE'));
      else if (released === 'lost') endReason = 'lease_lost';
    }

    async function processPass(): Promise<boolean> {
      if (await recoverAndClassify()) return true;

      if (await maintainQueue()) return true;

      if (await refreshCatalog()) return true;

      if (!adoptPendingBatches(db, token, deps.now())) return true;
      await processPendingBatches();
      if (leaseLost || stopReason !== undefined) return true;

      if (!timedOut()) await summarizePendingSessions();
      if (leaseLost || stopReason !== undefined) return true;

      if (yieldAfterPass || timedOut()) {
        endReason = yieldAfterPass ? 'batch_error' : 'max_run';
        yieldAfterPass = true;
        return true;
      }

      return false;
    }

    async function finishLeaseRun(): Promise<void> {
      if (leaseLost) {
        exit = 0;
        endReason = 'lease_lost';
      } else if (yieldAfterPass) {
        if (endReason === 'max_run') {
          await releaseMaxRun();
        }
        exit = 0;
      } else {
        exit = usedFallback ? 1 : 0;
      }
    }

    async function runPasses(): Promise<void> {
      for (;;) {
        const reason = controlReason();
        if (reason !== undefined) {
          stopReason = reason;
          break;
        }
        if (timedOut()) {
          yieldAfterPass = true;
          endReason = 'max_run';
          break;
        }

        if (await processPass()) break;

        if (resident ? await drainedOrWait() : await releaseEmptyPass()) break;
      }

    }

    async function runResident(): Promise<void> {
      let lastEpochElapsed = deps.elapsedMs();
      for (;;) {
        const reason = controlReason();
        if (reason !== undefined) {
          endReason = reason;
          exit = 0;
          if (reason === 'lease_lost') leaseLost = true;
          return;
        }
        const now = deps.now();
        if (deps.elapsedMs() - lastEpochElapsed < MAINTENANCE_MS && queueIsEmpty(db, paths, '', now)) {
          await interruptibleSleep(nextWakeDelay(db, now));
          continue;
        }
        const rotated = await retryBusy(() => rotateLease(db, token, deps.now()));
        if (rotated === null) {
          leaseLost = true;
          endReason = 'lease_lost';
          exit = 0;
          return;
        }
        token = rotated;
        epochDeadlineElapsed = deps.elapsedMs() + Math.max(0, deps.maxRunMs);
        yieldAfterPass = false;
        endReason = 'empty';
        providerState.clear();
        ancestorCache = createAncestorCache();
        const before = { ...result };
        await runPasses();
        lastEpochElapsed = deps.elapsedMs();
        const ended = leaseLost ? 'lease_lost' : stopReason;
        if (ended !== undefined) {
          endReason = ended;
          exit = 0;
          return;
        }
        const delta = countDelta(before, result);
        if (delta.applied !== 0 || delta.fallback !== 0) lastActivityElapsed = deps.elapsedMs();
        if (Object.values(delta).some(Boolean)) appendLog(paths.observeLog, 'info', 'epoch', delta);
        if (endReason === 'batch_error') {
          exit = 0;
          return;
        }
        await interruptibleSleep(nextWakeDelay(db, deps.now()));
      }
    }

    const startupReason = controlReason();
    if (startupReason !== undefined) {
      endReason = startupReason;
      exit = 0;
      return;
    }
    const config = loadConfig(paths);
    const resolved = resolveObserveModel(config);
    const presetEntry = resolved.preset === 'none' ? null : PRESET_CATALOG[resolved.preset];
    const credentials =
      resolved.preset === 'none'
        ? null
        : readCredentials(resolved.preset, deps.env, config.observer.agent_cli);
    const initialProviderReason = initialProviderFailure(resolved, credentials, config, deps.env);
    const startedConsentHash = consentHash(consentTuple(config, deps.env));
    const consentOk = (): boolean => liveConsentOk(paths, deps.env, startedConsentHash);
    const providerState = new Map<string, DegradedReason | null>();
    let ancestorCache = createAncestorCache();
    const batchDeps: BatchDeps = { ...deps, shouldStop: controlReason };
    const detect = async (text: string) => {
      try { return await deps.detect({
        text,
        paths: [],
        repoRoot: null,
        secretPaths: loadConfig(paths).privacy.secret_paths,
        credentialValues: credentialValues(deps.env),
      }); } catch { return { ok: false, reason: 'detector_error' } as const; }
    };

    if (resident) await runResident();
    else {
      await runPasses();
      await finishLeaseRun();
    }
  }

  async function shutdownResident(): Promise<void> {
    if (!db.isOpen) return;
    try {
      // The probe is a statement on a handle that is open but may already be failing, which is the
      // state a storage fault leaves behind. This runs in the lifecycle's own `finally`, so a
      // throw escaping here would cost the run its `run end` record and leave the handle open.
      if (!ownsLease(db, token)) return;
      const released = releaseForExit(db, paths, token, deps.now(), result, endReason, true);
      if (released === 'released') await retryBusy(() => checkpoint(db, 'TRUNCATE'));
      else if (released === 'lost') endReason = 'lease_lost';
    } catch {
      // A held lease becomes stale for takeover.
    }
  }

  if (resident) {
    process.on('SIGTERM', onSignal);
    process.on('SIGINT', onSignal);
  }
  try {
    const queued = reprocessSource === undefined ? 'queued' : requeueSource(db, token, reprocessSource, deps.now());
    if (queued === 'queued') await observeClaimedLease(db);
    else {
      const lost = queued === 'lease_lost';
      deps.writeError(lost ? 'Another worker took over. Retry this reprocessing command after it finishes.\n'
        : queued === 'work_selection_required' ? 'Choose the source work first with oboete work choose-source <source-id> <work-id|new>.\n'
        : 'The source was not found or is not a complete, non-secret capture.\n');
      if (!lost) releaseLease(db, token, () => true);
      exit = lost ? 1 : 2;
      endReason = queued;
    }
  } catch (error) {
    recordRunFailure(error, db, token);
  } finally {
    try {
      wakeSleep?.();
      clearInterval(heartbeatTimer);
      if (resident) await shutdownResident();
      if (db.isOpen) db.close();
    } finally {
      if (resident) {
        process.off('SIGTERM', onSignal);
        process.off('SIGINT', onSignal);
      }
    }
  }

  // Every path above assigns it; the check is here so a future one that does not fails loudly
  // instead of reporting `undefined` as this run's exit code (contracts/cli.md pins 0, 1 and 3).
  if (exit === undefined) throw new Error('observe run produced no exit code');
  if (reportsFallbackExit(exit, usedFallback, endReason)) {
    exit = 1;
  }
  if (resident && COOPERATIVE_REASONS.has(endReason)) exit = 0;
  return logEnd(paths, result, exit, endReason);
}

/** Detached `oboete observe`: one bounded worker run, or `--resident` across idle epochs. */
export async function runObserve(argv: string[], overrides: Partial<ObserveDeps> = {}): Promise<number> {
  let parsed: { reprocessSource?: string; resident: boolean; stop: boolean };
  try {
    const { values } = parseArgs({ args: argv, allowPositionals: false, strict: true,
      options: {
        'reprocess-source': { type: 'string' },
        resident: { type: 'boolean' },
        stop: { type: 'boolean' },
      } });
    const reprocessSource = values['reprocess-source'];
    const resident = values.resident === true;
    const stop = values.stop === true;
    if (reprocessSource !== undefined && !/^[a-zA-Z0-9:_-]{1,200}$/u.test(reprocessSource)) throw new Error('invalid_source');
    if (stop && (resident || reprocessSource !== undefined)) throw new Error('invalid_flags');
    parsed = { reprocessSource, resident, stop };
  } catch {
    (overrides.writeError ?? ((text: string) => { process.stderr.write(text); }))(OBSERVE_USAGE);
    return 2;
  }
  if (parsed.stop) {
    const paths = oboetePaths(resolveHome(overrides.env ?? process.env));
    writeWorkerStop(paths);
    return 0;
  }
  return await observeLifecycle(overrides, {
    reprocessSource: parsed.reprocessSource,
    resident: parsed.resident && parsed.reprocessSource === undefined,
  });
}

/** Historical processing starts only when the user names a surviving source explicitly. */
function requeueSource(db: DatabaseSync, token: string, id: string, now: number): 'queued' | 'invalid_source' | 'lease_lost' | 'work_selection_required' {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return 'lease_lost';
    }
    const stored = db.prepare('SELECT * FROM raw_events WHERE id = ?').get(id);
    if (stored === undefined) return 'invalid_source';
    const row = stored as unknown as RawEventRow;
    if (!isSummarizableRow(row) || row.classification_state === 'partial' || row.processing_state === 'excluded') return 'invalid_source';
    if (db.prepare('SELECT 1 FROM work_bindings WHERE id = ? AND work_id IS NOT NULL')
      .get(row.work_binding_id ?? null) === undefined) return 'work_selection_required';
    const batch = row.batch_id === null ? undefined
      : db.prepare('SELECT state FROM observation_batches WHERE id = ?').get(row.batch_id);
    if (batch?.state === 'pending' || batch?.state === 'running') return 'queued';
    // An explicit request is itself due now, even before the automatic ten-turn trigger.
    db.prepare(`UPDATE raw_events SET processing_state = 'waiting', processing_offset = 0,
      processing_hash = NULL, processed_at = NULL, retry_after = ?, processing_attempts = 0, batch_id = NULL WHERE id = ?`).run(now, id);
    db.prepare('UPDATE memory_sources SET source_processed_at = NULL WHERE raw_event_id = ?').run(id);
    db.prepare("UPDATE sessions SET summary_state = 'pending', summary_updated_at = NULL WHERE id = ?").run(row.session_id);
    return 'queued';
  });
}
