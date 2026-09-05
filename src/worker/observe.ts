import { spawn as nodeSpawn, type spawn } from 'node:child_process';
import { readdirSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';

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
import { nearbyCandidates, type NearbyCandidate } from '../db/queries.js';
import {
  checkCommits,
  checkPaths,
  createAncestorCache,
  type AncestorCache,
  updateCitationState,
} from '../injection/staleness.js';
import { appendLog, appendLogQuietly, credentialValues, errorCode } from '../log.js';
import { refreshWorkersAiCatalog } from '../observer/catalog.js';
import {
  applyObservations,
  checkLanguage,
  rejectsDirectives,
  sessionSummary,
  type ApplyResult,
  type DegradedReason,
} from '../observer/classify.js';
import { fallbackObserve, type FallbackEvent } from '../observer/fallback.js';
import { summarizeWithProvider, type CallOutcome } from '../observer/llm.js';
import { resolveModel } from '../observer/providers.js';
import { buildObserverRequest } from '../observer/request.js';
import { recordExhausted, reserveAttempt } from '../observer/reservation.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from '../paths.js';
import { reclassifyImportedRow } from '../privacy/classify.js';
import { cjkBigrams } from '../retrieval/fts.js';
import { detectSync, type DetectorInput, type DetectorResult } from '../privacy/detect.js';
import { loadDestinationRules } from '../privacy/egress.js';
import {
  classifyPending,
  createBatches,
  isSummarizableRow,
  loadBatchInput,
  payloadOf,
  reclaimStale,
  recoverSpool,
  toolInputOf,
  toolInputText,
  BLANK_CHARACTERS_SQL,
  SUMMARIZABLE_KINDS_SQL,
  type BatchInput,
  type BatchRow,
} from './batches.js';
import { assertLease, claimLease, heartbeat, releaseLease, transactionImmediate } from './lease.js';
import {
  checkpoint,
  cleanupPiAck,
  purgeExpiredEvents,
  runtimeStateSet,
} from './purge.js';

const DEFAULT_HEARTBEAT_MS = 2_000;
const DEFAULT_MAX_RUN_MS = 20 * 60 * 1_000;
const BUSY_RETRY_MS = 200;
/** Imported rows per fenced write: two detector runs each, so the lease's `now` stays fresh. */
const RECLASSIFY_LIMIT = 50;

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
 * contracts/cli.md line 35: a log never carries provider content. llm.ts's fixed unusable-output
 * messages are safe verbatim; a validation detail from contract.ts can echo provider-owned keys or
 * source ids, so it is replaced. Kept in step with src/observer/llm.ts.
 */
const SAFE_UNUSABLE_DETAILS = new Set([
  'provider response was not valid JSON',
  'provider output reached its length limit',
  'provider response contained no text',
  'provider response exceeded 1 MB',
  'the agent CLI did not return its documented JSON output',
  'the agent CLI response was unusable',
  'provider output was unusable',
]);

function loggableDetail(reason: DegradedReason, detail: string): string {
  if (reason !== 'unusable_output' || SAFE_UNUSABLE_DETAILS.has(detail)) return detail;
  return 'provider response failed observation validation';
}

type BatchResult = {
  state: 'applied' | 'fallback' | 'lease_lost';
  reason: DegradedReason | null;
  detail?: string;
  memoryIds: string[];
};

class LeaseLostError extends Error {
  constructor() {
    super('worker lease lost');
    this.name = 'LeaseLostError';
  }
}

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

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function retryBusy<T>(work: () => T | Promise<T>): Promise<T> {
  try {
    return await work();
  } catch (error) {
    if (!isBusyError(error)) throw error;
    await sleep(BUSY_RETRY_MS);
    return await work();
  }
}

function fallbackEvents(input: BatchInput): FallbackEvent[] {
  const turns = new Map(input.turns.map((turn) => [turn.id, turn.ordinal]));
  const events: FallbackEvent[] = [];

  for (const row of input.rows) {
    const payload = payloadOf(row) ?? {};
    const base = {
      id: row.id,
      turn_index: row.turn_id === null ? 0 : (turns.get(row.turn_id) ?? 0),
      sensitivity: row.sensitivity,
      classification_state: row.classification_state === 'partial' ? 'partial' : 'done',
    } as const;
    const toolCallId = typeof payload.tool_call_id === 'string' ? payload.tool_call_id : undefined;

    switch (row.kind) {
      case 'prompt':
      case 'last_assistant_message':
      case 'compaction_summary':
        events.push({ ...base, kind: row.kind, text: row.content ?? '' });
        break;
      case 'tool_call':
        events.push({
          ...base,
          kind: 'tool_call',
          ...(toolCallId === undefined ? {} : { tool_call_id: toolCallId }),
          tool_name: typeof payload.tool_name === 'string' ? payload.tool_name : 'other',
          input: toolInputOf(row),
        });
        break;
      case 'tool_result':
        events.push({
          ...base,
          kind: 'tool_result',
          ...(toolCallId === undefined ? {} : { tool_call_id: toolCallId }),
          output: row.content ?? '',
          is_error: payload.is_error === true,
        });
        break;
      case 'tool_failure':
        events.push({
          ...base,
          kind: 'tool_failure',
          ...(toolCallId === undefined ? {} : { tool_call_id: toolCallId }),
          error: row.content ?? '',
        });
        break;
      default:
        break;
    }
  }
  return events;
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

function markExcerpted(
  db: DatabaseSync,
  token: string,
  batchId: string,
  excerpted: boolean,
  now: number,
): boolean {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return false;
    }
    const result = db
      .prepare('UPDATE observation_batches SET excerpted = ? WHERE id = ? AND owner_token = ?')
      .run(excerpted ? 1 : 0, batchId, token);
    return Number(result.changes) !== 0;
  });
}

function recordProviderResult(
  db: DatabaseSync,
  token: string,
  preset: PresetName,
  outcome: Extract<CallOutcome, { ok: true }>,
  now: number,
): boolean {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return false;
    }
    db.prepare(
      `UPDATE provider_usage SET
         neurons_estimate = COALESCE(neurons_estimate, 0) + COALESCE(?, 0),
         resolved_model = COALESCE(?, resolved_model)
       WHERE rowid = (
         SELECT rowid FROM provider_usage WHERE preset = ? ORDER BY reset_at DESC LIMIT 1
       )`,
    ).run(outcome.neurons, outcome.resolvedModel, preset);
    return true;
  });
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

async function providerCall(
  db: DatabaseSync,
  token: string,
  input: ReturnType<typeof buildObserverRequest>['input'],
  batch: BatchRow,
  config: OboeteConfig,
  deps: ObserveDeps,
  preset: PresetName,
  model: string,
  consentOk: () => boolean,
): Promise<CallOutcome> {
  const entry = PRESET_CATALOG[preset];
  return await summarizeWithProvider(input, {
    preset,
    model,
    agentCli: config.observer.agent_cli,
    credentials: readCredentials(preset, deps.env, config.observer.agent_cli),
    consentOk,
    reserve: () => {
      const result = reserveAttempt(db, {
        preset,
        capped: entry.capped,
        trigger: batch.trigger,
        batchId: batch.id,
        token,
        now: deps.now(),
      });
      if (!result.ok) {
        if (result.reason === 'lease_lost') throw new LeaseLostError();
        return { ok: false, reason: result.reason };
      }
      return result;
    },
    onExhausted: (reservationId) =>
      recordExhausted(db, { preset, reservationId, now: deps.now() }),
    fetch: deps.fetch,
    spawn: deps.spawn,
    now: deps.now,
  });
}

async function applyFallback(
  db: DatabaseSync,
  token: string,
  input: BatchInput,
  nearby: NearbyCandidate[],
  reason: DegradedReason,
  detect: (text: string) => Promise<DetectorResult>,
  now: number,
): Promise<BatchResult> {
  const applied = await applyObservations(db, token, {
    batchId: input.batch.id,
    repoId: input.session.repo_id,
    sessionId: input.session.id,
    output: fallbackObserve({
      repoId: input.session.repo_id,
      events: fallbackEvents(input),
      nearby: nearby.map((row) => ({
        id: row.id,
        content_hash: row.content_hash,
        deleted: row.deleted,
      })),
    }),
    fallbackReason: reason,
    rows: input.rows,
    nearby,
    detect,
    now,
  });
  return {
    state: applied.leaseLost ? 'lease_lost' : 'fallback',
    reason,
    memoryIds: appliedMemoryIds(applied),
  };
}

function appliedMemoryIds(result: ApplyResult): string[] {
  return result.applied.flatMap((row) =>
    row.memoryId !== null && (row.decision === 'add' || row.decision === 'update')
      ? [row.memoryId]
      : [],
  );
}

async function processBatch(
  db: DatabaseSync,
  token: string,
  batch: BatchRow,
  config: OboeteConfig,
  deps: ObserveDeps,
  detect: (text: string) => Promise<DetectorResult>,
  providerState: Map<string, DegradedReason | null>,
  initialProviderReason: DegradedReason | null,
  resolved: { preset: PresetName | 'none'; model: string },
  consentOk: () => boolean,
): Promise<BatchResult> {
  const input = loadBatchInput(db, batch.id);
  if (input === null) throw new Error('batch input missing');
  const nearby = nearbyCandidates(db, {
    repoId: input.session.repo_id,
    text: input.rows.map((row) => `${row.content ?? ''}\n${toolInputText(row)}`).join('\n'),
    limit: 8,
  });

  if (batch.destination === 'fallback') {
    const sessionState = providerState.has(batch.session_id)
      ? providerState.get(batch.session_id)
      : initialProviderReason;
    const reason = sessionState ?? 'rule_based';
    return await applyFallback(db, token, input, nearby, reason, detect, deps.now());
  }

  if (resolved.preset === 'none') {
    providerState.set(batch.session_id, 'no_provider');
    return await applyFallback(db, token, input, nearby, 'no_provider', detect, deps.now());
  }

  const request = buildObserverRequest({
    rows: input.rows,
    session: input.session,
    turns: input.turns,
    destination: batch.destination,
    repoId: input.session.repo_id,
    nearby,
    rules: loadDestinationRules(db),
  });
  if (!markExcerpted(db, token, batch.id, request.excerpted, deps.now())) {
    return { state: 'lease_lost', reason: null, memoryIds: [] };
  }

  let outcome = await providerCall(
    db,
    token,
    request.input,
    batch,
    config,
    deps,
    resolved.preset,
    resolved.model,
    consentOk,
  );
  if (outcome.ok) {
    if (!recordProviderResult(db, token, resolved.preset, outcome, deps.now())) {
      return { state: 'lease_lost', reason: null, memoryIds: [] };
    }
    if (checkLanguage(request.input, outcome.output) === 'mismatch') {
      outcome = await providerCall(
        db,
        token,
        request.input,
        batch,
        config,
        deps,
        resolved.preset,
        resolved.model,
        consentOk,
      );
      if (outcome.ok && !recordProviderResult(db, token, resolved.preset, outcome, deps.now())) {
        return { state: 'lease_lost', reason: null, memoryIds: [] };
      }
      if (outcome.ok && checkLanguage(request.input, outcome.output) === 'mismatch') {
        providerState.set(batch.session_id, 'language_mismatch');
        return await applyFallback(
          db,
          token,
          input,
          nearby,
          'language_mismatch',
          detect,
          deps.now(),
        );
      }
    }
  }

  if (!outcome.ok) {
    providerState.set(batch.session_id, outcome.reason);
    return {
      ...(await applyFallback(db, token, input, nearby, outcome.reason, detect, deps.now())),
      detail: loggableDetail(outcome.reason, outcome.detail),
    };
  }

  providerState.set(batch.session_id, null);
  await deps.applyHook();
  const applied = await applyObservations(db, token, {
    batchId: input.batch.id,
    repoId: input.session.repo_id,
    sessionId: input.session.id,
    output: outcome.output,
    fallbackReason: null,
    rows: input.rows,
    nearby,
    detect,
    now: deps.now(),
  });
  return {
    state: applied.leaseLost ? 'lease_lost' : 'applied',
    reason: null,
    memoryIds: appliedMemoryIds(applied),
  };
}

function gitEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const clean: NodeJS.ProcessEnv = { ...env, LC_ALL: 'C' };
  for (const key of Object.keys(clean)) {
    if (key.startsWith('GIT_')) delete clean[key];
  }
  return clean;
}

async function repositoryHead(
  root: string,
  deps: ObserveDeps,
): Promise<string | null> {
  return await new Promise((resolve) => {
    let settled = false;
    const finish = (head: string | null): void => {
      if (settled) return;
      settled = true;
      resolve(head);
    };
    try {
      const child = deps.spawn('git', ['-C', root, 'rev-parse', 'HEAD'], {
        env: gitEnvironment(deps.env),
        stdio: ['ignore', 'pipe', 'ignore'],
        signal: AbortSignal.timeout(500),
      });
      let stdout = '';
      child.stdout?.setEncoding('utf8');
      child.stdout?.on('data', (chunk: string) => {
        if (stdout.length < 256) stdout += chunk;
      });
      child.once('error', () => finish(null));
      child.once('close', (code) => {
        const head = stdout.trim();
        finish(code === 0 && /^[0-9a-f]{40,64}$/iu.test(head) ? head : null);
      });
    } catch {
      finish(null);
    }
  });
}

async function updateBatchCitations(
  db: DatabaseSync,
  token: string,
  repoId: string,
  memoryIds: string[],
  ancestorCache: AncestorCache,
  deps: ObserveDeps,
): Promise<boolean> {
  if (memoryIds.length === 0) return true;
  const root = db.prepare('SELECT display_root FROM repos WHERE id = ?').get(repoId)?.display_root;
  if (typeof root !== 'string' || root === '') return true;
  const head = await repositoryHead(root, deps);
  if (head === null) return true;
  const citationState = new Map<string, boolean>();
  for (const memoryId of new Set(memoryIds)) {
    const citations = db
      .prepare(
        `SELECT citation_kind, citation_value FROM memory_sources
         WHERE memory_id = ? AND citation_kind IS NOT NULL AND citation_value IS NOT NULL`,
      )
      .all(memoryId);
    const paths = citations.flatMap((row) =>
      (row.citation_kind === 'file_read' || row.citation_kind === 'file_modified') &&
      typeof row.citation_value === 'string'
        ? [row.citation_value]
        : [],
    );
    const commits = citations.flatMap((row) =>
      row.citation_kind === 'commit' && typeof row.citation_value === 'string'
        ? [row.citation_value]
        : [],
    );
    citationState.set(
      memoryId,
      [...checkPaths(paths, root).values(), ...checkCommits(commits, root, ancestorCache).values()].every(
        Boolean,
      ),
    );
  }
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, deps.now())) {
      db.exec('ROLLBACK');
      return false;
    }
    for (const [memoryId, ok] of citationState) updateCitationState(db, memoryId, head, ok);
    return true;
  });
}

/**
 * R12 "Export/import": every quarantined row (`review_state = imported`) goes through the detector
 * and the directive check; the decision table is privacy/classify.ts reclassifyImportedRow. The
 * writes are one fenced transaction so a lost lease changes nothing. A tombstoned row keeps its
 * hashes (FR-035) and loses the secret (FR-018).
 */
async function reclassifyImported(
  db: DatabaseSync,
  token: string,
  now: () => number,
  detect: (text: string) => Promise<DetectorResult>,
): Promise<{ examined: number; leaseLost: boolean }> {
  const select = db.prepare(
    `SELECT id, title, body FROM memories
     WHERE review_state = 'imported' AND deleted_at IS NULL AND id > ? ORDER BY id LIMIT ?`,
  );
  // The CJK index is a column of its own (0002_memory_search.sql), so it follows the text here.
  const release = db.prepare(
    `UPDATE memories SET review_state = 'unreviewed', title = ?, body = ?, cjk_bigrams = ?
     WHERE id = ? AND review_state = 'imported'`,
  );
  const tombstone = db.prepare(
    `UPDATE memories SET sensitivity = 'secret', deleted_at = ?, title = ?, body = ?, cjk_bigrams = ?
     WHERE id = ? AND review_state = 'imported'`,
  );
  let examined = 0;
  // Keyset pages: a row the detector could not finish stays quarantined and is passed over, so
  // the pass ends even when the detector keeps failing on it (the next run tries it again).
  let after = '';
  for (;;) {
    const rows = select
      .all(after, RECLASSIFY_LIMIT)
      .map((row) => ({ id: String(row.id), title: String(row.title ?? ''), body: String(row.body ?? '') }));
    if (rows.length === 0) return { examined, leaseLost: false };
    examined += rows.length;
    after = rows[rows.length - 1]?.id ?? after;

    const decided: { id: string; decision: 'unreviewed' | 'secret'; title: string; body: string }[] = [];
    for (const row of rows) {
      const directive = rejectsDirectives(row.title) !== null || rejectsDirectives(row.body) !== null;
      const verdict = reclassifyImportedRow(await detect(row.title), await detect(row.body), directive);
      if (verdict.decision === 'retry') continue;
      decided.push({ id: row.id, ...verdict });
    }

    // The clock is read after the detector ran, so the lease heartbeat this write leaves is current.
    const at = now();
    const leaseLost = transactionImmediate(db, () => {
      if (!assertLease(db, token, at)) {
        db.exec('ROLLBACK');
        return true;
      }
      for (const row of decided) {
        const bigrams = cjkBigrams(`${row.title} ${row.body}`);
        if (row.decision === 'unreviewed') release.run(row.title, row.body, bigrams, row.id);
        else tombstone.run(at, row.title, row.body, bigrams, row.id);
      }
      return false;
    });
    if (leaseLost) return { examined, leaseLost: true };
    if (rows.length < RECLASSIFY_LIMIT) return { examined, leaseLost: false };
  }
}

function pendingSummaries(db: DatabaseSync): string[] {
  return db
    .prepare(
      `SELECT id FROM sessions s
       WHERE status = 'ended' AND summary_state = 'pending'
         AND NOT EXISTS (
           SELECT 1 FROM observation_batches b
           WHERE b.session_id = s.id AND b.state NOT IN ('applied', 'fallback')
         )
       ORDER BY ended_at, id`,
    )
    .all()
    .map((row) => String(row.id));
}

// `isSummarizableRow` over the unbatched rows, in SQL: a pass asks whether one exists instead of
// reading every unbatched row (FR-002, the worker runs beside the hooks). The one part SQL cannot
// see is a tool call's input, which lives in `payload_json`, so only content-less tool calls are
// read back — the rest of the rule is the indexed test below.
const UNBATCHED_WITH_CONTENT = `SELECT 1 AS work FROM raw_events
   WHERE batch_id IS NULL AND kind IN (${SUMMARIZABLE_KINDS_SQL})
     AND classification_state IS NOT 'failed' AND sensitivity <> 'secret'
     AND TRIM(COALESCE(content, ''), ${BLANK_CHARACTERS_SQL}) <> '' LIMIT 1`;
const UNBATCHED_EMPTY_TOOL_CALLS = `SELECT kind, content, payload_json, classification_state, sensitivity
   FROM raw_events
   WHERE batch_id IS NULL AND kind = 'tool_call'
     AND classification_state IS NOT 'failed' AND sensitivity <> 'secret'
     AND TRIM(COALESCE(content, ''), ${BLANK_CHARACTERS_SQL}) = ''`;

function queueIsEmpty(db: DatabaseSync, paths: OboetePaths): boolean {
  if (
    db.prepare("SELECT 1 AS work FROM observation_batches WHERE state IN ('pending', 'running') LIMIT 1").get() !==
    undefined
  ) {
    return false;
  }
  if (db.prepare(UNBATCHED_WITH_CONTENT).get() !== undefined) return false;
  const toolCalls = db
    .prepare(UNBATCHED_EMPTY_TOOL_CALLS)
    .all() as unknown as Parameters<typeof isSummarizableRow>[0][];
  if (toolCalls.some(isSummarizableRow)) return false;
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
  return (
    db.prepare("SELECT 1 AS work FROM sessions WHERE status = 'ended' AND summary_state = 'pending' LIMIT 1").get() ===
    undefined
  );
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
    const empty = releaseWithPending || queueIsEmpty(db, paths);
    if (empty) runtimeStateSet(db, 'last_run', JSON.stringify({ at: now, reason, ...result }), now);
    return empty;
  });
}

/** Detached `oboete observe`: one bounded worker run, never a resident service (FR-009). */
export async function runObserve(argv: string[], overrides: Partial<ObserveDeps> = {}): Promise<number> {
  void argv;
  const deps: ObserveDeps = {
    now: overrides.now ?? Date.now,
    fetch: overrides.fetch ?? globalThis.fetch,
    spawn: overrides.spawn ?? nodeSpawn,
    detect: overrides.detect ?? detectSync,
    env: overrides.env ?? process.env,
    heartbeatMs: overrides.heartbeatMs ?? DEFAULT_HEARTBEAT_MS,
    maxRunMs: overrides.maxRunMs ?? DEFAULT_MAX_RUN_MS,
    applyHook: overrides.applyHook ?? (() => undefined),
  };
  const paths = oboetePaths(resolveHome(deps.env));
  if (isPaused(paths)) return 0;

  const result: Counts = {
    recovered: 0,
    classified: 0,
    reclassified: 0,
    batches: 0,
    applied: 0,
    fallback: 0,
    purged: 0,
  };
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
    return 3;
  }

  const startedAt = deps.now();
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
    return logEnd(paths, result, 3, 'storage_error');
  }
  if (token === null) {
    db.close();
    return logEnd(paths, result, 0, 'another_worker');
  }

  let leaseLost = false;
  const heartbeatTimer = setInterval(() => {
    try {
      if (!heartbeat(db as DatabaseSync, token as string, deps.now())) leaseLost = true;
    } catch (error) {
      appendLogQuietly(paths.observeLog, 'warn', 'heartbeat failed', { code: errorCode(error) });
    }
  }, Math.max(1, deps.heartbeatMs));
  heartbeatTimer.unref();

  let usedFallback = false;
  let catalogChecked = false;
  let yieldAfterPass = false;
  let exit: number;
  let endReason = 'empty';

  try {
    const config = loadConfig(paths);
    let resolved: { preset: PresetName | 'none'; model: string };
    try {
      resolved = resolveModel(config);
    } catch {
      resolved = { preset: config.observer.preset, model: '' };
    }
    const presetEntry = resolved.preset === 'none' ? null : PRESET_CATALOG[resolved.preset];
    const credentials =
      resolved.preset === 'none'
        ? null
        : readCredentials(resolved.preset, deps.env, config.observer.agent_cli);
    const initialProviderReason: DegradedReason | null =
      resolved.preset === 'none' || credentials?.present !== true || resolved.model === ''
        ? 'no_provider'
        : !consentMatches(config, deps.env)
          ? 'consent_changed'
          : null;
    const startedConsentHash = consentHash(consentTuple(config, deps.env));
    const consentOk = (): boolean => liveConsentOk(paths, deps.env, startedConsentHash);
    const providerState = new Map<string, DegradedReason | null>();
    const ancestorCache = createAncestorCache();
    const detect = (text: string) =>
      deps.detect({
        text,
        paths: [],
        repoRoot: null,
        secretPaths: config.privacy.secret_paths,
        credentialValues: credentialValues(deps.env),
      });

    for (;;) {
      if (deps.now() - startedAt >= Math.max(0, deps.maxRunMs)) {
        yieldAfterPass = true;
        endReason = 'max_run';
        break;
      }

      const recovered = await retryBusy(() => recoverSpool(db, paths, token, deps.now()));
      result.recovered += recovered.inserted;
      if (leaseLost || !ownsLease(db, token)) break;

      const classified = await retryBusy(() => classifyPending(db, token, deps.now(), detect));
      result.classified += classified.examined;
      if (classified.leaseLost || leaseLost) break;

      const reclassified = await retryBusy(() => reclassifyImported(db, token, deps.now, detect));
      result.reclassified += reclassified.examined;
      if (reclassified.leaseLost || leaseLost) break;

      const reclaimed = await retryBusy(() => reclaimStale(db, token, deps.now()));
      if (reclaimed.leaseLost || leaseLost) break;

      const purged = await retryBusy(() => purgeExpiredEvents(db, token, deps.now()));
      result.purged += purged.deleted;
      if (purged.leaseLost || leaseLost) break;

      await retryBusy(() => cleanupPiAck(db, token, paths.piAck, deps.now()));
      if (leaseLost || !ownsLease(db, token)) break;

      const created = await retryBusy(() =>
        createBatches(db, token, deps.now(), { preset: presetEntry?.egress ?? 'none' }),
      );
      if (created.leaseLost || leaseLost) break;

      if (
        !catalogChecked &&
        resolved.preset === 'workers-ai' &&
        credentials?.present === true
      ) {
        catalogChecked = true;
        await retryBusy(() =>
          refreshWorkersAiCatalog(db, { env: deps.env, now: deps.now(), fetchImpl: deps.fetch }),
        );
        if (leaseLost || !ownsLease(db, token)) break;
      }

      if (!adoptPendingBatches(db, token, deps.now())) break;
      const batches = pendingBatches(db);
      for (const batch of batches) {
        if (leaseLost) break;
        let batchResult: BatchResult | null = null;
        let batchError: unknown;
        try {
          batchResult = await processBatch(
            db,
            token,
            batch,
            config,
            deps,
            detect,
            providerState,
            initialProviderReason,
            resolved,
            consentOk,
          );
          if (batchResult.state === 'lease_lost') {
            leaseLost = true;
          } else {
            result.batches += 1;
            result[batchResult.state] += 1;
            if (batchResult.state === 'fallback' && batchResult.reason !== 'rule_based') {
              usedFallback = true;
            }
          }
        } catch (error) {
          if (error instanceof LeaseLostError) leaseLost = true;
          else {
            batchError = error;
            yieldAfterPass = true;
          }
        }

        if (!leaseLost) {
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

        appendLog(paths.observeLog, batchError === undefined ? 'info' : 'error', 'batch', {
          id: batch.id,
          state: batchResult?.state ?? 'error',
          reason: batchResult?.reason ?? (batchError === undefined ? 'none' : errorCode(batchError)),
          ...(batchResult?.detail === undefined ? {} : { detail: batchResult.detail.split(/[\r\n]/)[0] }),
        });
      }
      if (leaseLost) break;

      for (const sessionId of pendingSummaries(db)) {
        try {
          const summary = await retryBusy(() => sessionSummary(db, token, sessionId, deps.now()));
          if (summary.state === 'lease_lost') {
            leaseLost = true;
            break;
          }
        } catch (error) {
          if (isStorageError(error)) throw error;
          appendLog(paths.observeLog, 'error', 'session summary failed', {
            session: sessionId,
            code: errorCode(error),
          });
          yieldAfterPass = true;
        }
      }
      if (leaseLost) break;

      if (yieldAfterPass || deps.now() - startedAt >= Math.max(0, deps.maxRunMs)) {
        endReason = yieldAfterPass ? 'batch_error' : 'max_run';
        yieldAfterPass = true;
        break;
      }

      const released = releaseForExit(db, paths, token, deps.now(), result, 'empty', false);
      if (released === 'lost') {
        leaseLost = true;
        break;
      }
      if (released === 'released') {
        await retryBusy(() => checkpoint(db, 'TRUNCATE'));
        endReason = 'empty';
        break;
      }
      await sleep(Math.min(Math.max(1, deps.heartbeatMs), BUSY_RETRY_MS));
    }

    if (leaseLost) {
      exit = 0;
      endReason = 'lease_lost';
    } else if (yieldAfterPass) {
      if (endReason === 'max_run') {
        // FR-009: a bounded worker releases even with queued work so the next hook can respawn it.
        const released = releaseForExit(db, paths, token, deps.now(), result, endReason, true);
        if (released === 'released') await retryBusy(() => checkpoint(db, 'TRUNCATE'));
        else if (released === 'lost') endReason = 'lease_lost';
      }
      exit = 0;
    } else {
      exit = usedFallback ? 1 : 0;
    }
  } catch (error) {
    let logFailed = false;
    try {
      appendLog(paths.observeLog, 'error', 'worker step failed', { code: errorCode(error) });
    } catch {
      logFailed = true;
    }
    const storageError = logFailed || isStorageError(error);
    exit = storageError ? 3 : 0;
    endReason = storageError ? 'storage_error' : 'worker_error';
    if (ownsLease(db, token)) {
      try {
        releaseForExit(db, paths, token, deps.now(), result, endReason, false);
      } catch {
        // R6: preserve the original storage outcome; a held lease becomes stale for takeover.
      }
    }
  } finally {
    clearInterval(heartbeatTimer);
    if (db.isOpen) db.close();
  }

  if (exit !== 3 && usedFallback && endReason !== 'lease_lost' && endReason !== 'max_run' && endReason !== 'batch_error') {
    exit = 1;
  }
  return logEnd(paths, result, exit, endReason);
}
