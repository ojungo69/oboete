import type { DatabaseSync } from 'node:sqlite';

import {
  PRESET_CATALOG,
  readCredentials,
  type ChainTarget,
  type OboeteConfig,
  type PresetName,
} from '../config.js';
import { nearbyUnchanged } from '../observer/provenance.js';
import { currentWorkCheckpoint, memoryScope, memoryVisibility, nearbyCandidates, type NearbyCandidate } from '../db/queries.js';
import { contentHash } from '../events.js';
import { checkpointHash, materialHash, memoryIdFor } from '../db/identity.js';
import { promoteSensitivity } from '../privacy/classify.js';
import { applyObservations, type ApplyResult } from '../observer/apply.js';
import {
  CHAIN_STOPS,
  checkLanguage,
  deferralOutcome,
  mostSevereReason,
  rejectsDirectives,
  type DegradedReason,
  type SourceReason,
} from '../observer/classify.js';
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

/** The run's dependencies plus the worker's own control check, which the batches consult. */
export type BatchDeps = ObserveDeps & { shouldStop: () => string | undefined };

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

/**
 * One target's turn in the chain, for the observe log: the batch's single `degraded_reason` keeps
 * only the most severe reason, so the rest are only visible here (contracts/provider-fallback.md
 * "Diagnostics"). `detail` picks the message for the reason that wins and is sanitized there.
 */
export type ProviderAttempt = {
  position: number;
  preset: PresetName;
  model: string;
  reason: DegradedReason;
  detail: string;
};

export type BatchResult = {
  detail?: string;
  memoryIds: string[];
  attempts?: ProviderAttempt[];
} & (
  | { state: 'applied' | 'fallback' | 'lease_lost' | 'requeued'; reason: DegradedReason | null }
  | { state: 'done'; reason: string }
);

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
  deps: BatchDeps;
  preset: PresetName;
  model: string;
  consentOk: () => boolean;
};

async function providerCall(options: ProviderCallOptions): Promise<ProviderResult> {
  const { db, token, input, batch, config, deps, preset, model, consentOk } = options;
  const entry = PRESET_CATALOG[preset];
  let stopReason: string | undefined;
  const outcome = await summarizeWithProvider(input, {
    preset,
    model,
    agentCli: config.observer.agent_cli,
    credentials: readCredentials(preset, deps.env, config.observer.agent_cli),
    consentOk: () => {
      // This boundary also runs before output retries and agent CLI attempts.
      stopReason ??= deps.shouldStop();
      return stopReason === undefined && consentOk();
    },
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
  return stopReason === undefined ? { outcome }
    : { done: { state: 'done', reason: stopReason, memoryIds: [] } };
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

/** The reasons of this batch's deferred sources, for a pass that re-checked none of them itself. */
function recordedDeferrals(db: DatabaseSync, batchId: string): string[] {
  return db.prepare(`SELECT DISTINCT reason FROM observation_batch_sources
    WHERE batch_id = ? AND outcome = 'deferred'`).all(batchId).map((row) => String(row.reason));
}

/** The reasons this pass deferred, or null when the lease was lost before they could be recorded. */
async function revalidateSources(options: ProcessBatchOptions, input: BatchInput,
  privacyFor: PrivacyReader): Promise<SourceReason[] | null> {
  const { db, token, deps } = options;
  const checked: { row: RawEventRow; result: DetectorResult; context: SourceContext; reason: SourceReason }[] = [];
  // Consent is a property of the repository, not of one source, and reading it parses the config
  // file: one answer for this batch, taken only if some source needs it.
  let consentHolds: boolean | null = null;
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
    // An unreadable policy is a consent change only when the caller's `consentOk` says consent no
    // longer holds; an unresolvable binding or root is a held origin (contracts/memory-core.md).
    // What that callback covers is the caller's: the worker re-reads the file, so an unparsable
    // config reads as a consent change there, while doctor closes over an already-parsed config.
    let reason: SourceReason = 'detector_failed';
    if (!available) {
      consentHolds ??= options.consentOk();
      reason = consentHolds ? 'source_context_unknown' : 'consent_changed';
    }
    checked.push({ row, result, context, reason });
  }
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, deps.now())) {
      db.exec('ROLLBACK');
      return null;
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
    return checked.filter((entry) => !entry.result.ok).map((entry) => entry.reason);
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
  deps: BatchDeps;
  detect: (text: string) => Promise<DetectorResult>;
  providerState: Map<string, DegradedReason | null>;
  initialProviderReason: DegradedReason | null;
  resolved: { preset: PresetName | 'none'; model: string; chain: ChainTarget[] };
  consentOk: () => boolean;
  /**
   * Caller-owned so providerCall's LeaseLostError or an applyObservations storage error cannot
   * discard earlier attempts required by contracts/provider-fallback.md "Diagnostics".
   */
  attempts: ProviderAttempt[];
};

/**
 * How a pass through the chain ended. `outcome` exists only on the variant that has an answer to
 * apply, and `answered` is null on the other, so the pairing "a target is named exactly when the
 * call succeeded" is the type's rather than a comment's. The caller discriminates on
 * `answered === null`, never on a key being present; the failure path reads `attempts`, which is
 * the caller's own array.
 */
type ChainResult =
  | { done: BatchResult }
  | { answered: Pick<ProviderAttempt, 'position' | 'preset' | 'model'>; outcome: Extract<CallOutcome, { ok: true }> }
  | { answered: null };

async function attemptTargets(
  options: ProcessBatchOptions, input: BatchInput, nearby: NearbyCandidate[],
  request: ReturnType<typeof buildObserverRequest>, targets: ChainTarget[],
): Promise<ChainResult> {
  const { db, token, batch, config, deps, consentOk, attempts } = options;
  for (const [position, target] of targets.entries()) {
    const between = deps.shouldStop();
    if (between !== undefined) return { done: { state: 'done', reason: between, memoryIds: [], attempts } };
    // Step 2 of "The attempt sequence", and it has to be the loop's own: `summarizeWithProvider`
    // answers `no_provider` for a target with no credentials before it ever asks whether consent
    // still holds, so a chain that ends on such a target would keep an earlier target's reason by
    // precedence and send the user to fix a credential when consent is what they must act on.
    if (!consentOk()) {
      attempts.push({ position, preset: target.preset, model: target.model,
        reason: 'consent_changed', detail: '' });
      return { answered: null };
    }
    const called = await providerCall({
      db, token, input: request.input, batch, config, deps,
      preset: target.preset, model: target.model, consentOk,
    });
    if ('done' in called) return { done: { ...called.done, attempts } };
    const settled = await settleProviderOutcome({
      options,
      request,
      input,
      nearby,
      position,
      preset: target.preset,
      model: target.model,
      outcome: called.outcome,
    });
    // A target whose answer arrived and was then refused records its own line where the refusal is
    // decided (`retryOnLanguageMismatch`), not here: the fallback that follows it can come back
    // `lease_lost` or throw, and the line would be lost on the one path that already spent two
    // allowances on this target.
    if ('done' in settled) return { done: { ...settled.done, attempts } };
    const outcome = settled.outcome;
    if (outcome.ok) return { answered: { position, preset: target.preset, model: target.model }, outcome };
    attempts.push({ position, preset: target.preset, model: target.model,
      reason: outcome.reason, detail: outcome.detail });
    if (CHAIN_STOPS.has(outcome.reason)) break;
  }

  return { answered: null };
}

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

type ProviderResult = { done: BatchResult } | { outcome: CallOutcome };

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
  position: number;
  preset: PresetName;
  model: string;
  outcome: CallOutcome;
}): Promise<ProviderResult> {
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
  position: number;
  preset: PresetName;
  model: string;
}): Promise<ProviderResult> {
  const { options, request, input, nearby, position, preset, model } = args;
  const { db, token, batch, config, deps, detect, providerState } = options;
  const called = await providerCall({
    db, token, input: request.input, batch, config, deps, preset, model, consentOk: options.consentOk,
  });
  if ('done' in called) return called;
  const { outcome } = called;
  if (outcome.ok && !recordProviderResult(db, token, preset, outcome, deps.now())) {
    return { done: { state: 'lease_lost', reason: null, memoryIds: [] } };
  }
  if (outcome.ok && checkLanguage(request.input, outcome.output) === 'mismatch') {
    providerState.set(batch.session_id, 'language_mismatch');
    // Before the fallback, because this target has now spent two allowances and `applyFallback`
    // can return `lease_lost` or throw — and the reason is already decided here
    // (contracts/provider-fallback.md "Diagnostics": one line per target that failed).
    options.attempts.push({ position, preset, model, reason: 'language_mismatch', detail: '' });
    return {
      done: await applyFallback(db, token, input, nearby, 'language_mismatch', detect, deps.now(), request.coverage),
    };
  }
  return { outcome };
}

export async function processBatch(options: ProcessBatchOptions): Promise<BatchResult> {
  const { db, token, batch, deps, detect, providerState,
    initialProviderReason, resolved, consentOk, attempts } = options;
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
  const deferred = await revalidateSources(options, input, privacyFor);
  if (deferred === null) return { state: 'lease_lost', reason: null, memoryIds: [] };
  const reconciled = reconcilePendingDestinations(db, token, deps.now(),
    resolved.preset === 'none' ? 'none' : PRESET_CATALOG[resolved.preset].egress);
  if (reconciled.leaseLost) return { state: 'lease_lost', reason: null, memoryIds: [] };
  input = loadBatchInput(db, batch.id)!;
  if (input.batch.state !== 'pending') return { state: 'requeued', reason: null, memoryIds: [] };
  if (input.rows.length === 0) {
    // What this pass did to the sources decides the batch's own outcome, so an earlier pass over the
    // same batch — whose cause the user may since have fixed — does not. When this pass deferred
    // nothing there is nothing of its own to read: either an earlier pass emptied the batch and its
    // receipts are the only record, or the sources left for something that is not a deferral at all
    // (quarantined as secret), which those receipts also say.
    const reason = deferralOutcome(deferred.length > 0 ? deferred : recordedDeferrals(db, batch.id));
    transactionImmediate(db, () => {
      if (!assertLease(db, token, deps.now())) throw new LeaseLostError();
      db.prepare("UPDATE observation_batches SET state = 'fallback', completed_at = ?, degraded_reason = ? WHERE id = ? AND owner_token = ?")
        .run(deps.now(), reason, batch.id, token);
    });
    // The row is terminal either way. A batch that emptied without a reason did no work and must not
    // read as activity: counting it would keep a resident worker awake across a held source's own
    // retries (#279), so it is reported as requeued, with the detail on the log line.
    return reason === null
      ? { state: 'requeued', reason: null, detail: 'held', memoryIds: [] }
      : { state: 'fallback', reason, memoryIds: [] };
  }
  const nearby = privacy === null ? [] : await revalidateNearby(options, nearbyForBatch(db, input), privacyFor);

  if (batch.destination === 'fallback') {
    const reason = fallbackReason(providerState, batch.session_id, initialProviderReason);
    return await applyFallback(db, token, input, nearby, reason, detect, deps.now());
  }

  // Both ways a run can have no provider at all, answered here rather than inside the target loop.
  // `resolveObserveModel` turns a configuration the resolver refuses into an empty model and an
  // empty chain, which `initialProviderFailure` has already read as `no_provider` before the first
  // batch: building a target from it would spend the whole pipeline — the detector pass over the
  // request included — on a call `summarizeWithProvider` refuses at its first line, and would reach
  // the consent guard, whose `consent_changed` sends the user to accept an egress that leaves the
  // resolver error exactly where it was.
  if (resolved.preset === 'none' || resolved.model === '') {
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
  const stopReason = deps.shouldStop();
  if (stopReason !== undefined) return { state: 'done', reason: stopReason, memoryIds: [] };
  if (!markRequest(db, token, input, request, deps.now(), parentId)) {
    return { state: 'lease_lost', reason: null, memoryIds: [] };
  }

  const chain = await attemptTargets({ ...options, consentOk: currentConsent }, input, nearby, request,
    chainTargets(resolved.preset, resolved.model, resolved.chain, batch.destination));
  if ('done' in chain) return chain.done;

  if (chain.answered === null) {
    // Every failed target is in `attempts`, so attemptTargets ran at least once. The reason a stop
    // ended the chain on wins, because that is the one the user has to act on; otherwise the batch
    // keeps the most severe of the reasons the chain actually met. The kept reason and the kept
    // detail always come from the same attempt, which is what lets `loggableDetail` decide by
    // reason whether the text is the provider's (contracts/provider-fallback.md "Advance and stop").
    const last = attempts.at(-1)!;
    const reason = CHAIN_STOPS.has(last.reason)
      ? last.reason
      : mostSevereReason(attempts.map((attempt) => attempt.reason))!;
    const worst = attempts.find((attempt) => attempt.reason === reason)!;
    providerState.set(batch.session_id, reason);
    return {
      ...(await applyFallback(db, token, input, nearby, reason, detect, deps.now(), request.coverage)),
      detail: loggableDetail(reason, worst.detail),
      attempts,
    };
  }

  const { answered, outcome } = chain;
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
  // `applyObservations` can refuse the answer it was given — a required progress decision the
  // detector rejects mints `unusable_output` *after* the call — and that reason is created too late
  // for `reserveAttempt` to have tied it to a target. Without this line the log names every target
  // that failed to answer and then a batch reason nothing accounts for
  // (contracts/provider-fallback.md "Diagnostics": one line per target that failed).
  const refused = applied.leaseLost ? null : applied.fallbackReason ?? null;
  if (refused !== null) attempts.push({ ...answered, reason: refused, detail: '' });
  return {
    state: applied.leaseLost ? 'lease_lost' : applied.fallbackReason === undefined ? 'applied' : 'fallback',
    reason: applied.fallbackReason ?? null,
    memoryIds: appliedMemoryIds(applied),
    attempts,
  };
}

/**
 * The primary first, then each admitted target the batch's destination label already allows:
 * `remote_observer` admits a local or a remote target, `local_observer` a local one only
 * (contracts/provider-fallback.md "The destination label, and per-attempt eligibility").
 */
function chainTargets(
  preset: PresetName,
  model: string,
  chain: ChainTarget[],
  destination: BatchRow['destination'],
): ChainTarget[] {
  return [
    { preset, model },
    ...chain.filter((target) =>
      destination === 'local_observer'
        ? PRESET_CATALOG[target.preset].egress === 'local'
        : destination === 'remote_observer'),
  ];
}
