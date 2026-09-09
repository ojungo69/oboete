import type { DatabaseSync } from 'node:sqlite';

import {
  PRESET_CATALOG,
  readCredentials,
  type OboeteConfig,
  type PresetName,
} from '../config.js';
import { nearbyCandidates, type NearbyCandidate } from '../db/queries.js';
import { applyObservations, type ApplyResult } from '../observer/apply.js';
import { checkLanguage, type DegradedReason } from '../observer/classify.js';
import { fallbackObserve, type FallbackEvent } from '../observer/fallback.js';
import { summarizeWithProvider, type CallOutcome } from '../observer/llm.js';
import { buildObserverRequest } from '../observer/request.js';
import { recordExhausted, reserveAttempt } from '../observer/reservation.js';
import type { DetectorResult } from '../privacy/detect.js';
import { loadDestinationRules } from '../privacy/egress.js';
import {
  loadBatchInput,
  payloadOf,
  toolInputOf,
  toolInputText,
  type BatchInput,
  type BatchRow,
} from './batches.js';
import { assertLease, transactionImmediate } from './lease.js';
import type { ObserveDeps } from './observe.js';

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

export type BatchResult = {
  state: 'applied' | 'fallback' | 'lease_lost';
  reason: DegradedReason | null;
  detail?: string;
  memoryIds: string[];
};

export class LeaseLostError extends Error {
  constructor() {
    super('worker lease lost');
    this.name = 'LeaseLostError';
  }
}
function fallbackEventBase(row: BatchInput['rows'][number], turns: Map<string, number>) {
  return {
    id: row.id,
    turn_index: row.turn_id === null ? 0 : (turns.get(row.turn_id) ?? 0),
    sensitivity: row.sensitivity,
    classification_state: row.classification_state === 'partial' ? 'partial' : 'done',
  } as const;
}

function fallbackToolCall(
  base: ReturnType<typeof fallbackEventBase>,
  toolCallId: string | undefined,
  payload: Record<string, unknown>,
  row: BatchInput['rows'][number],
): FallbackEvent {
  return {
    ...base,
    kind: 'tool_call',
    ...(toolCallId === undefined ? {} : { tool_call_id: toolCallId }),
    tool_name: typeof payload.tool_name === 'string' ? payload.tool_name : 'other',
    input: toolInputOf(row),
  };
}

function fallbackToolResult(
  base: ReturnType<typeof fallbackEventBase>,
  toolCallId: string | undefined,
  payload: Record<string, unknown>,
  row: BatchInput['rows'][number],
): FallbackEvent {
  return {
    ...base,
    kind: 'tool_result',
    ...(toolCallId === undefined ? {} : { tool_call_id: toolCallId }),
    output: row.content ?? '',
    is_error: payload.is_error === true,
  };
}

function fallbackToolFailure(
  base: ReturnType<typeof fallbackEventBase>,
  toolCallId: string | undefined,
  row: BatchInput['rows'][number],
): FallbackEvent {
  return {
    ...base,
    kind: 'tool_failure',
    ...(toolCallId === undefined ? {} : { tool_call_id: toolCallId }),
    error: row.content ?? '',
  };
}

function appendFallbackEvent(
  row: BatchInput['rows'][number],
  turns: Map<string, number>,
  events: FallbackEvent[],
): void {
  const payload = payloadOf(row) ?? {};
  const base = fallbackEventBase(row, turns);
  const toolCallId = typeof payload.tool_call_id === 'string' ? payload.tool_call_id : undefined;

  switch (row.kind) {
    case 'prompt':
    case 'last_assistant_message':
    case 'compaction_summary':
      events.push({ ...base, kind: row.kind, text: row.content ?? '' });
      break;
    case 'tool_call':
      events.push(fallbackToolCall(base, toolCallId, payload, row));
      break;
    case 'tool_result':
      events.push(fallbackToolResult(base, toolCallId, payload, row));
      break;
    case 'tool_failure':
      events.push(fallbackToolFailure(base, toolCallId, row));
      break;
    default:
      break;
  }
}

function fallbackEvents(input: BatchInput): FallbackEvent[] {
  const turns = new Map(input.turns.map((turn) => [turn.id, turn.ordinal]));
  const events: FallbackEvent[] = [];

  for (const row of input.rows) {
    appendFallbackEvent(row, turns, events);
  }
  return events;
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
type ProviderCallOptions = {
  db: DatabaseSync;
  token: string;
  input: ReturnType<typeof buildObserverRequest>['input'];
  batch: BatchRow;
  config: OboeteConfig;
  deps: ObserveDeps;
  preset: PresetName;
  model: string;
  consentOk: () => boolean;
};

async function providerCall(options: ProviderCallOptions): Promise<CallOutcome> {
  const { db, token, input, batch, config, deps, preset, model, consentOk } = options;
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

function nearbyForBatch(db: DatabaseSync, input: BatchInput): NearbyCandidate[] {
  return nearbyCandidates(db, {
    repoId: input.session.repo_id,
    text: input.rows.map((row) => `${row.content ?? ''}\n${toolInputText(row)}`).join('\n'),
    limit: 8,
  });
}

type ProcessBatchOptions = {
  db: DatabaseSync;
  token: string;
  batch: BatchRow;
  config: OboeteConfig;
  deps: ObserveDeps;
  detect: (text: string) => Promise<DetectorResult>;
  providerState: Map<string, DegradedReason | null>;
  initialProviderReason: DegradedReason | null;
  resolved: { preset: PresetName | 'none'; model: string };
  consentOk: () => boolean;
};

/** The reason a fallback records: this session's own degraded state, else the worker's, else rules. */
function fallbackReason(
  providerState: Map<string, DegradedReason | null>,
  sessionId: string,
  initialProviderReason: DegradedReason | null,
): DegradedReason {
  const sessionState = providerState.has(sessionId)
    ? providerState.get(sessionId)
    : initialProviderReason;
  return sessionState ?? 'rule_based';
}

/** The observer request for one batch, with the destination rules read at call time. */
function requestForProvider(
  db: DatabaseSync,
  nearby: ReturnType<typeof nearbyForBatch>,
  destination: 'remote_observer' | 'local_observer',
  input: BatchInput,
) {
  return buildObserverRequest({
    rows: input.rows,
    session: input.session,
    turns: input.turns,
    destination,
    repoId: input.session.repo_id,
    nearby,
    rules: loadDestinationRules(db),
  });
}

type LanguageRetry = { done: BatchResult } | { outcome: CallOutcome };

/**
 * Records a successful provider answer and retries once if it came back in the wrong language, in
 * the order the inline form used. A failed call is passed through untouched for the caller's own
 * fallback branch.
 */
async function settleProviderOutcome(args: {
  options: ProcessBatchOptions;
  request: ReturnType<typeof buildObserverRequest>;
  input: BatchInput;
  nearby: ReturnType<typeof nearbyForBatch>;
  preset: PresetName;
  model: string;
  outcome: CallOutcome;
}): Promise<LanguageRetry> {
  const { options, request, preset, outcome } = args;
  const { db, token, deps } = options;
  if (!outcome.ok) return { outcome };
  if (!recordProviderResult(db, token, preset, outcome, deps.now())) {
    return { done: { state: 'lease_lost', reason: null, memoryIds: [] } };
  }
  if (checkLanguage(request.input, outcome.output) !== 'mismatch') return { outcome };
  return await retryOnLanguageMismatch(args);
}


/**
 * One retry after the provider answered in the wrong language, in the order the inline form used:
 * the retry's own result is recorded first, and only a second mismatch marks the session degraded
 * and falls back. Returns the result the caller must return, or the outcome to carry on with.
 */
async function retryOnLanguageMismatch(args: {
  options: ProcessBatchOptions;
  request: ReturnType<typeof buildObserverRequest>;
  input: BatchInput;
  nearby: ReturnType<typeof nearbyForBatch>;
  preset: PresetName;
  model: string;
}): Promise<LanguageRetry> {
  const { options, request, input, nearby, preset, model } = args;
  const { db, token, batch, config, deps, detect, providerState } = options;
  const outcome = await providerCall({
    db, token, input: request.input, batch, config, deps, preset, model, consentOk: options.consentOk,
  });
  if (outcome.ok && !recordProviderResult(db, token, preset, outcome, deps.now())) {
    return { done: { state: 'lease_lost', reason: null, memoryIds: [] } };
  }
  if (outcome.ok && checkLanguage(request.input, outcome.output) === 'mismatch') {
    providerState.set(batch.session_id, 'language_mismatch');
    return {
      done: await applyFallback(db, token, input, nearby, 'language_mismatch', detect, deps.now()),
    };
  }
  return { outcome };
}

export async function processBatch(options: ProcessBatchOptions): Promise<BatchResult> {
  const { db, token, batch, config, deps, detect, providerState,
    initialProviderReason, resolved, consentOk } = options;
  const input = loadBatchInput(db, batch.id);
  if (input === null) throw new Error('batch input missing');
  const nearby = nearbyForBatch(db, input);

  if (batch.destination === 'fallback') {
    const reason = fallbackReason(providerState, batch.session_id, initialProviderReason);
    return await applyFallback(db, token, input, nearby, reason, detect, deps.now());
  }

  if (resolved.preset === 'none') {
    providerState.set(batch.session_id, 'no_provider');
    return await applyFallback(db, token, input, nearby, 'no_provider', detect, deps.now());
  }

  const request = requestForProvider(db, nearby, batch.destination, input);
  if (!markExcerpted(db, token, batch.id, request.excerpted, deps.now())) {
    return { state: 'lease_lost', reason: null, memoryIds: [] };
  }

  let outcome = await providerCall({
    db, token, input: request.input, batch, config, deps,
    preset: resolved.preset, model: resolved.model, consentOk,
  });
  const settled = await settleProviderOutcome({
    options,
    request,
    input,
    nearby,
    preset: resolved.preset,
    model: resolved.model,
    outcome,
  });
  if ('done' in settled) return settled.done;
  outcome = settled.outcome;

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
