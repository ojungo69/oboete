import type { DatabaseSync } from 'node:sqlite';

import {
  PRESET_CATALOG,
  readCredentials,
  type OboeteConfig,
  type PresetName,
} from '../config.js';
import { nearbyUnchanged } from '../observer/provenance.js';
import { currentWorkCheckpoint, memoryScope, memoryVisibility, nearbyCandidates, type NearbyCandidate } from '../db/queries.js';
import { contentHash } from '../events.js';
import { checkpointHash, materialHash, memoryIdFor } from '../db/identity.js';
import { promoteSensitivity } from '../privacy/classify.js';
import { applyObservations, type ApplyResult } from '../observer/apply.js';
import { checkLanguage, rejectsDirectives, type DegradedReason } from '../observer/classify.js';
import { fallbackObserve, type FallbackEvent } from '../observer/fallback.js';
import { summarizeWithProvider, type CallOutcome } from '../observer/llm.js';
import { buildObserverRequest } from '../observer/request.js';
import { recordExhausted, reserveAttempt } from '../observer/reservation.js';
import type { DetectorResult } from '../privacy/detect.js';
import { memoryContexts, readSourcePrivacy, sourceContext, type SourceContext, type RootCache } from '../privacy/provenance.js';
import { loadDestinationRules } from '../privacy/egress.js';
import {
  loadBatchInput,
  excludeSecretSource,
  payloadOf,
  reconcilePendingDestinations,
  sourceRetryAt,
  toolInputOf,
  toolInputText,
  type BatchInput,
  type BatchRow,
  type RawEventRow,
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
  state: 'applied' | 'fallback' | 'lease_lost' | 'requeued';
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
function markRequest(
  db: DatabaseSync,
  token: string,
  batchInput: BatchInput,
  request: ReturnType<typeof buildObserverRequest>,
  now: number,
  parentId: string | null,
): boolean {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return false;
    }
    const result = db
      .prepare(`UPDATE observation_batches SET excerpted = ?, checkpoint_parent_id = ?, checkpoint_decision = 'pending'
        WHERE id = ? AND owner_token = ? AND EXISTS (SELECT 1 FROM work_bindings binding
          JOIN work_items w ON w.id = binding.work_id WHERE binding.id = observation_batches.work_binding_id
          AND w.current_checkpoint_memory_id IS ?)`)
      .run(request.excerpted ? 1 : 0, parentId, batchInput.batch.id, token, parentId);
    if (Number(result.changes) === 0) {
      db.exec('ROLLBACK');
      return false;
    }
    const source = db.prepare(`UPDATE raw_events SET processing_hash = ?, processing_offset = ?
      WHERE id = ? AND batch_id = ? AND processing_hash IS ? AND processing_offset = ?`);
    const receipt = db.prepare(`UPDATE observation_batch_sources SET
      portion_start = ?, portion_end = ?, source_total = ?, source_hash = ? WHERE batch_id = ? AND raw_event_id = ?`);
    for (const portion of request.coverage) {
      const row = batchInput.rows.find((row) => row.id === portion.rowId)!;
      if (portion.end > portion.start && Number(source.run(portion.sourceHash, portion.start,
        row.id, batchInput.batch.id, row.processing_hash ?? null, row.processing_offset ?? 0).changes) !== 1) {
        db.exec('ROLLBACK');
        return false;
      }
      receipt.run(portion.start, portion.end, portion.total, portion.sourceHash, batchInput.batch.id, portion.rowId);
    }
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
  coverage?: ReturnType<typeof buildObserverRequest>['coverage'],
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
    coverage,
  });
  return {
    state: applied.leaseLost ? 'lease_lost' : 'fallback',
    reason,
    memoryIds: appliedMemoryIds(applied),
  };
}

function appliedMemoryIds(result: ApplyResult): string[] {
  const ids = result.applied.flatMap((row) =>
    row.memoryId !== null && (row.decision === 'add' || row.decision === 'update')
      ? [row.memoryId]
      : [],
  );
  if (result.checkpoint?.memoryId !== null && result.checkpoint?.memoryId !== undefined
    && ['replaced', 'confirmed'].includes(result.checkpoint.decision)) ids.push(result.checkpoint.memoryId);
  return ids;
}

function nearbyForBatch(db: DatabaseSync, input: BatchInput): NearbyCandidate[] {
  let preview = '';
  for (const row of input.rows) {
    preview += `${row.content?.slice(0, 500) ?? ''}\n${toolInputText(row).slice(0, 500)}\n`;
    if (preview.length >= 4_000) break;
  }
  return nearbyCandidates(db, {
    repoId: input.session.repo_id,
    workId: db.prepare('SELECT work_id FROM work_bindings WHERE id = ?').get(input.batch.work_binding_id ?? null)?.work_id as string | null,
    text: preview.slice(0, 4_000),
    limit: 8,
  });
}

type PrivacyReader = (context: SourceContext | null, projectMemoryId?: string) => ReturnType<typeof readSourcePrivacy>;

async function revalidateSources(options: ProcessBatchOptions, input: BatchInput,
  privacyFor: PrivacyReader): Promise<boolean> {
  const { db, token, deps } = options;
  const checked: { row: RawEventRow; result: DetectorResult; context: SourceContext; reason: string }[] = [];
  for (const selected of input.rows) {
    // Generation receives only partial metadata; privacy also checks the retained prefix itself.
    const row = selected.classification_state === 'partial'
      ? db.prepare('SELECT * FROM raw_events WHERE id = ?').get(selected.id) as unknown as RawEventRow : selected;
    const payload = payloadOf(row);
    const context = sourceContext(db, row);
    const privacy = privacyFor(context);
    const paths = context.paths ?? [];
    const available = privacy !== null;
    const result: DetectorResult = !available ? { ok: false, reason: 'detector_error' }
      : await deps.detect({ ...privacy.detector, text: row.content ?? '',
        fields: [row.id, row.kind, typeof payload?.tool_name === 'string' ? payload.tool_name : '', toolInputText(row), ...paths] });
    checked.push({ row, result, context, reason: privacyFor(null) === null ? 'consent_changed'
      : available ? 'detector_failed' : 'source_context_unknown' });
  }
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, deps.now())) {
      db.exec('ROLLBACK');
      return false;
    }
    const receipt = db.prepare(`UPDATE observation_batch_sources SET outcome = ?, reason = ?, recorded_at = ?
      WHERE batch_id = ? AND raw_event_id = ?`);
    for (const { row, result, context, reason } of checked) {
      const at = deps.now();
      if (!result.ok) {
        db.prepare(`UPDATE raw_events SET processing_state = 'waiting', retry_after = ?,
          processing_attempts = processing_attempts + 1, batch_id = NULL WHERE id = ? AND batch_id = ?`)
          .run(row.classification_state === 'partial' ? null : sourceRetryAt(at, row.processing_attempts ?? 0), row.id, input.batch.id);
        receipt.run('deferred', reason, at, input.batch.id, row.id);
      } else if (result.sensitivity === 'secret' || result.privateRemoved > 0) {
        excludeSecretSource(db, row.id, at);
        db.prepare('UPDATE raw_events SET batch_id = NULL WHERE id = ?').run(row.id);
        receipt.run('rejected', 'secret', at, input.batch.id, row.id);
      } else {
        const sensitivity = promoteSensitivity(row.sensitivity, result,
          row.classification_state === 'partial' ? 'partial' : 'done');
        const payload = { ...payloadOf(row), capture_root: context.root,
          ...(context.paths === null ? {} : { source_paths: context.paths }) };
        db.prepare('UPDATE raw_events SET sensitivity = ?, payload_json = ? WHERE id = ? AND batch_id = ?')
          .run(sensitivity, JSON.stringify(payload), row.id, input.batch.id);
      }
    }
    return true;
  });
}


async function revalidateNearby(options: ProcessBatchOptions, nearby: NearbyCandidate[],
  privacyFor: PrivacyReader): Promise<NearbyCandidate[]> {
  const kept: NearbyCandidate[] = [];
  for (const candidate of nearby) {
    if (rejectsDirectives(candidate.title) !== null || rejectsDirectives(candidate.body) !== null) continue;
    if (candidate.work_id !== null && candidate.work_id !== undefined) {
      const material = materialHash(candidate.title, candidate.body);
      const content = checkpointHash(candidate.repo_id, candidate.work_id, candidate.checkpoint_parent_id ?? null, material);
      if (candidate.material_hash !== material || candidate.content_hash !== content || candidate.id !== memoryIdFor(content)) continue;
    }
    const contexts = memoryContexts(options.db, candidate);
    if (contexts === null) continue;
    const checked = { ...candidate, privacy_stamp: contentHash(JSON.stringify(contexts)),
      visibility_stamp: contentHash(JSON.stringify(memoryVisibility(options.db, candidate.id))) };
    let result: DetectorResult | undefined;
    for (const context of contexts) {
      const privacy = privacyFor(context, candidate.id);
      if (privacy === null) { result = undefined; break; }
      result = await options.deps.detect({ ...privacy.detector, text: candidate.title,
        fields: [candidate.body, candidate.id, candidate.type, ...(context.paths ?? [])] });
      if (!result.ok || result.sensitivity === 'secret' || result.privateRemoved > 0) break;
    }
    if (result === undefined) continue;
    if (result.ok && result.sensitivity !== 'secret' && result.privateRemoved === 0) {
      if (nearbyUnchanged(options.db, checked)) kept.push(checked);
    } else {
      const secretFound = result.ok;
      transactionImmediate(options.db, () => {
        if (!assertLease(options.db, options.token, options.deps.now())) throw new LeaseLostError();
        options.db.prepare("UPDATE memories SET review_state = 'imported' WHERE id = ?").run(candidate.id);
        if (secretFound) {
          options.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = ?").run(candidate.id);
          options.db.prepare('UPDATE memory_sources SET evidence = NULL, capture_root = NULL, source_paths_json = NULL, citation_value = NULL WHERE memory_id = ?').run(candidate.id);
        }
      });
    }
  }
  return kept;
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
      done: await applyFallback(db, token, input, nearby, 'language_mismatch', detect, deps.now(), request.coverage),
    };
  }
  return { outcome };
}

export async function processBatch(options: ProcessBatchOptions): Promise<BatchResult> {
  const { db, token, batch, config, deps, detect, providerState,
    initialProviderReason, resolved, consentOk } = options;
  let input = loadBatchInput(db, batch.id);
  if (input === null) throw new Error('batch input missing');
  const repoId = input.session.repo_id;
  if (input.batch.state !== 'pending') return { state: 'requeued', reason: null, memoryIds: [] };
  const location = { repoId, bindingId: input.batch.work_binding_id ?? null };
  const policies = new Map<string, { context: SourceContext | null; projectMemoryId?: string; policy: ReturnType<typeof readSourcePrivacy> }>();
  const roots: RootCache = new Map();
  const privacyFor: PrivacyReader = (context, projectMemoryId) => {
    const key = JSON.stringify([context, projectMemoryId]);
    if (!policies.has(key)) {
      let policy: ReturnType<typeof readSourcePrivacy> = null;
      try { policy = readSourcePrivacy(db, location, context, deps.env, roots, undefined, projectMemoryId); } catch { /* Fail closed. */ }
      policies.set(key, { context, projectMemoryId, policy });
    }
    return policies.get(key)!.policy;
  };
  const privacy = privacyFor(null);
  if (!(await revalidateSources(options, input, privacyFor))) return { state: 'lease_lost', reason: null, memoryIds: [] };
  const reconciled = reconcilePendingDestinations(db, token, deps.now(),
    resolved.preset === 'none' ? 'none' : PRESET_CATALOG[resolved.preset].egress);
  if (reconciled.leaseLost) return { state: 'lease_lost', reason: null, memoryIds: [] };
  input = loadBatchInput(db, batch.id)!;
  if (input.batch.state !== 'pending') return { state: 'requeued', reason: null, memoryIds: [] };
  if (input.rows.length === 0) {
    const deferred = db.prepare("SELECT 1 FROM observation_batch_sources WHERE batch_id = ? AND outcome = 'deferred' LIMIT 1").get(batch.id);
    const reason = deferred === undefined ? null : privacy === null ? 'consent_changed' : 'unusable_output';
    transactionImmediate(db, () => {
      if (!assertLease(db, token, deps.now())) throw new LeaseLostError();
      db.prepare("UPDATE observation_batches SET state = 'fallback', completed_at = ?, degraded_reason = ? WHERE id = ? AND owner_token = ?")
        .run(deps.now(), reason, batch.id, token);
    });
    return { state: reason === null ? 'requeued' : 'fallback', reason, memoryIds: [] };
  }
  const nearby = privacy === null ? [] : await revalidateNearby(options, nearbyForBatch(db, input), privacyFor);

  if (batch.destination === 'fallback') {
    const reason = fallbackReason(providerState, batch.session_id, initialProviderReason);
    return await applyFallback(db, token, input, nearby, reason, detect, deps.now());
  }

  if (resolved.preset === 'none') {
    providerState.set(batch.session_id, 'no_provider');
    return await applyFallback(db, token, input, nearby, 'no_provider', detect, deps.now());
  }

  const selectedDestination = PRESET_CATALOG[resolved.preset].egress === 'local'
    ? 'local_observer' : 'remote_observer';
  if (batch.destination !== selectedDestination) {
    return await applyFallback(db, token, input, nearby, 'consent_changed', detect, deps.now());
  }

  const boundWork = db.prepare(`SELECT w.id, w.current_checkpoint_memory_id FROM work_bindings binding
    JOIN work_items w ON w.id = binding.work_id WHERE binding.id = ? AND w.repo_id = ?`)
    .get(input.batch.work_binding_id ?? null, repoId);
  if (boundWork === undefined) return { state: 'requeued', reason: null, memoryIds: [] };
  const parentId = typeof boundWork.current_checkpoint_memory_id === 'string' ? boundWork.current_checkpoint_memory_id : null;
  const parentRow = currentWorkCheckpoint(db, String(boundWork.id), memoryScope(db, { repoId, destination: batch.destination,
    workId: String(boundWork.id) }));
  const parentCandidates: NearbyCandidate[] = parentRow === null ? [] : [{ ...parentRow,
    title: parentRow.title ?? '', body: parentRow.body ?? '', deleted: parentRow.deleted_at !== null }];
  const parent = (await revalidateNearby(options, parentCandidates, privacyFor))[0];
  const checkpointContext: ReturnType<typeof buildObserverRequest>['input']['checkpoint_context'] = parentId === null
    ? { state: 'none' } : parent === undefined ? { state: 'withheld' }
      : { state: 'provided', id: parent.id, title: parent.title, body: parent.body };

  const request = buildObserverRequest({
    rows: input.rows, session: input.session, turns: input.turns,
    destination: batch.destination, repoId, nearby, rules: loadDestinationRules(db), checkpointContext,
  });
  const currentConsent = () => {
    try {
      const currentRoots: RootCache = new Map();
      return consentOk() && [...policies.values()].every(({ context, projectMemoryId, policy }) => policy === null
        || readSourcePrivacy(db, location, context, deps.env, currentRoots, undefined, projectMemoryId)?.stamp === policy.stamp)
        && db.prepare('SELECT current_checkpoint_memory_id FROM work_items WHERE id = ?').get(boundWork.id)?.current_checkpoint_memory_id === parentId
        && (parent === undefined || nearbyUnchanged(db, parent))
        && nearby.filter((candidate) => request.input.nearby.some((sent) => sent.id === candidate.id))
          .every((candidate) => nearbyUnchanged(db, candidate));
    }
    catch { return false; }
  };
  const finalCheck = privacy === null ? null : await deps.detect({ ...privacy.detector, paths: [], text: JSON.stringify(request.input) });
  if (finalCheck?.ok !== true || finalCheck.sensitivity === 'secret' || finalCheck.privateRemoved > 0) {
    return await applyFallback(db, token, input, [], 'unusable_output', detect, deps.now());
  }
  if (!markRequest(db, token, input, request, deps.now(), parentId)) {
    return { state: 'lease_lost', reason: null, memoryIds: [] };
  }

  let outcome = await providerCall({
    db, token, input: request.input, batch, config, deps,
    preset: resolved.preset, model: resolved.model, consentOk: currentConsent,
  });
  const settled = await settleProviderOutcome({
    options: { ...options, consentOk: currentConsent },
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
      ...(await applyFallback(db, token, input, nearby, outcome.reason, detect, deps.now(), request.coverage)),
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
    coverage: request.coverage,
    providedCheckpoint: request.input.checkpoint_context.state === 'provided' ? parent : undefined,
    rows: input.rows,
    nearby: nearby.filter((candidate) => request.input.nearby.some((sent) => sent.id === candidate.id)),
    detect,
    now: deps.now(),
  });
  return {
    state: applied.leaseLost ? 'lease_lost' : applied.fallbackReason === undefined ? 'applied' : 'fallback',
    reason: applied.fallbackReason ?? null,
    memoryIds: appliedMemoryIds(applied),
  };
}
