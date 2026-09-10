// The apply step: what a summarizer's observations are allowed to change (T038).
// Sources: contracts/observer.md ("Worker rules after either path", "Session summary", "Call
// policy" item 5), research.md R10, R11 and R12, data-model.md (memories, memory_sources,
// sessions.summary_state), spec FR-014, FR-018, FR-029, FR-035, FR-042, amendments A11 and A13.
// Security-owned (plan.md "Structure Decision"): the target restriction, the tombstone check and
// the sensitivity lattice live here and nowhere else.
import type { DatabaseSync } from 'node:sqlite';

import { contentHash, materialHash, memoryIdFor } from '../db/identity.js';
import { visibilityScope, type NearbyCandidate } from '../db/queries.js';
import { promoteSensitivity, strictest } from '../privacy/classify.js';
import type { DetectorResult } from '../privacy/detect.js';
import type { Sensitivity } from '../privacy/egress.js';
import { cjkBigrams } from '../retrieval/fts.js';
import { payloadOf, sourceRetryAt, type RawEventRow } from '../worker/batches.js';
import { assertLease, transactionImmediate } from '../worker/lease.js';
import { INSERT_MEMORY, INSERT_SOURCE, rejectsDirectives, type DegradedReason } from './classify.js';
import type { Observation, ObserverOutput } from './contract.js';
import type { FallbackOutput } from './fallback.js';
import { applyCheckpoint, batchWork, prepareCheckpoint, type CheckpointResult, type PreparedCheckpoint } from './checkpoint.js';
import { generationPrivacy, nearbyUnchanged, retainGenerationPrivacy } from './provenance.js';
import type { ObserverRequest } from './request.js';
import { recordObservationSharing } from '../sharing.js';

export type ApplyDecision = 'add' | 'update' | 'delete' | 'noop';

export type ApplyInput = {
  batchId: string;
  repoId: string;
  sessionId: string;
  output: ObserverOutput | FallbackOutput | null;
  fallbackReason: DegradedReason | null;
  rows: RawEventRow[];
  nearby: NearbyCandidate[];
  detect: (text: string) => Promise<DetectorResult>;
  now: number;
  coverage?: ObserverRequest['coverage'];
  providedCheckpoint?: NearbyCandidate;
};

const PROCESSED_RETENTION_MS = 30 * 24 * 60 * 60 * 1000;
const UNPROCESSED_MEMORY_SOURCES = `SELECT 1 FROM memory_sources ms
  LEFT JOIN raw_events r ON r.id = ms.raw_event_id
  WHERE ms.memory_id = m.id AND ms.context_only = 0 AND ms.raw_event_id IS NOT NULL AND r.processing_state IS NOT 'processed'
    AND ms.source_processed_at IS NULL`;

export type ApplyResult = {
  applied: { index: number; decision: ApplyDecision; memoryId: string | null;
    historical?: { target: string; reason: 'capture_time_order' | 'target_unavailable' } }[];
  suppressed: { index: number; contentHash: string }[];
  dropped: { index: number; reason: 'detector_failed' | 'directive' | 'unknown_source' }[];
  leaseLost: boolean;
  checkpoint?: CheckpointResult;
  fallbackReason?: DegradedReason;
};

type Prepared = {
  index: number;
  observation: Observation;
  sourceIds: string[];
  detectorClass: Sensitivity;
  material: string;
  content: string;
  memoryId: string;
  capturedAt: number | null;
};

type ApplyStatements = ReturnType<typeof prepareApplyStatements>;
type ApplyLists = Pick<ApplyResult, 'applied' | 'suppressed'>;

type PreparedMutation = {
  decision: ApplyDecision;
  sensitivity: Sensitivity;
  supersedes: string | null;
  historicalTarget: { id: string; capturedAt: number | null } | null;
};

function prepareApplyStatements(db: DatabaseSync) {
  return {
    byContentHash: db.prepare('SELECT id, deleted_at, valid_to, review_state, sensitivity FROM memories WHERE content_hash = ?'),
    readTarget: db.prepare(
      'SELECT id, sensitivity, deleted_at, valid_to, review_state, source_captured_at FROM memories WHERE id = ? AND repo_id = ?',
    ),
    insertMemory: db.prepare(INSERT_MEMORY),
    insertSource: db.prepare(INSERT_SOURCE),
  };
}

function applyDelete(
  db: DatabaseSync,
  input: ApplyInput,
  item: Prepared,
  target: string | null,
  classification: Observation['classification'],
): ApplyResult['applied'][number] {
  // contracts/observer.md: a delete needs a reason, otherwise nothing happens.
  if (target === null || classification.reason.trim() === '') {
    return { index: item.index, decision: 'noop', memoryId: null };
  }
  const changes = Number(
    db
      .prepare(
        'UPDATE memories SET deleted_at = ? WHERE id = ? AND repo_id = ? AND deleted_at IS NULL',
      )
      .run(input.now, target, input.repoId).changes,
  );
  return {
    index: item.index,
    decision: changes === 0 ? 'noop' : 'delete',
    memoryId: changes === 0 ? null : target,
  };
}

function recordExisting(
  db: DatabaseSync,
  input: ApplyInput,
  item: Prepared,
  rowsById: Map<string, RawEventRow>,
  byContentHash: ApplyStatements['byContentHash'],
  result: ApplyLists,
): boolean {
  const existing = byContentHash.get(item.content);
  if (existing === undefined) return false;
  // FR-035: the same content never returns once it was deleted; the reason is kept for `why`.
  if (existing.deleted_at !== null) {
    result.suppressed.push({ index: item.index, contentHash: item.content });
    return true;
  }
  if (existing.valid_to !== null || existing.review_state === 'imported' || existing.sensitivity === 'secret') {
    result.applied.push({ index: item.index, decision: 'noop', memoryId: null });
    return true;
  }
  if (input.fallbackReason === null) confirmExistingMemory(db, input, item, rowsById, String(existing.id));
  result.applied.push({ index: item.index, decision: 'noop', memoryId: String(existing.id) });
  return true;
}

function confirmExistingMemory(
  db: DatabaseSync,
  input: ApplyInput,
  item: Prepared,
  rowsById: Map<string, RawEventRow>,
  id: string,
): void {
  const existing = db.prepare('SELECT sensitivity FROM memories WHERE id = ? AND repo_id = ? AND deleted_at IS NULL')
    .get(id, input.repoId);
  if (existing === undefined) return;
  const sensitivity = strictest(existing.sensitivity as Sensitivity, item.detectorClass,
    ...item.sourceIds.map((sourceId) => rowsById.get(sourceId)?.sensitivity ?? 'secret'));
  db.prepare(`UPDATE memories SET sensitivity = ?,
    source_captured_at = MAX(COALESCE(source_captured_at, ?), COALESCE(?, source_captured_at)) WHERE id = ?`)
    .run(sensitivity, item.capturedAt, item.capturedAt, id);
  const source = db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, source_agent)
    SELECT ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM memory_sources WHERE memory_id = ? AND raw_event_id = ? AND context_only = 0)`);
  for (const sourceId of item.sourceIds) {
    source.run(id, sourceId, rowsById.get(sourceId)?.agent ?? null, id, sourceId);
  }
}

function prepareMutation(
  input: ApplyInput,
  item: Prepared,
  rowsById: Map<string, RawEventRow>,
  target: string | null,
  decision: ApplyDecision,
  readTarget: ApplyStatements['readTarget'],
): PreparedMutation {
  let sensitivity = strictest(
    item.detectorClass,
    ...item.sourceIds.map((id) => rowsById.get(id)?.sensitivity ?? 'secret'),
  );
  let supersedes: string | null = null;
  let historicalTarget: PreparedMutation['historicalTarget'] = null;
  if (decision === 'update' && target !== null) {
    const targetRow = readTarget.get(target, input.repoId);
    if (targetRow?.deleted_at !== null) {
      // The target is gone or tombstoned: the content is still worth keeping, but it
      // supersedes nothing and a tombstone stays a tombstone.
      decision = 'add';
    } else {
      if (sourceIsOlder(item, targetRow.source_captured_at)) {
        historicalTarget = { id: target, capturedAt: typeof targetRow.source_captured_at === 'number' ? targetRow.source_captured_at : null };
      } else supersedes = target;
      // max(target, every source row, detector): an eligible update cannot relax a stricter
      // target (contracts/observer.md, tested against the outbound body).
      sensitivity = strictest(sensitivity, targetRow.sensitivity as Sensitivity);
    }
  }
  return { decision, sensitivity, supersedes, historicalTarget };
}

function sourceIsOlder(item: Prepared, targetTime: unknown): boolean {
  return item.capturedAt === null || typeof targetTime !== 'number' || item.capturedAt < targetTime;
}

function insertObservationCitations(
  insertSource: ApplyStatements['insertSource'],
  item: Prepared,
  agent: string | null,
): void {
  // FR-029: the full path is kept here for the staleness check, never the shortened form.
  for (const path of item.observation.citations.files_read) {
    insertSource.run(item.memoryId, null, 'file_read', path, agent);
  }
  for (const path of item.observation.citations.files_modified) {
    insertSource.run(item.memoryId, null, 'file_modified', path, agent);
  }
  for (const commit of item.observation.citations.commits) {
    insertSource.run(item.memoryId, null, 'commit', commit, agent);
  }
}

function insertObservationSources(
  statements: ApplyStatements,
  item: Prepared,
  rowsById: Map<string, RawEventRow>,
): void {
  const agent = rowsById.get(item.sourceIds[0])?.agent ?? null;
  for (const id of item.sourceIds) {
    // FR-005: the agent is recorded as provenance and decides nothing.
    statements.insertSource.run(item.memoryId, id, null, null, rowsById.get(id)?.agent ?? null);
  }
  insertObservationCitations(statements.insertSource, item, agent);
}

function insertPreparedObservation(
  db: DatabaseSync,
  input: ApplyInput,
  item: Prepared,
  rowsById: Map<string, RawEventRow>,
  statements: ApplyStatements,
  mutation: PreparedMutation,
): ApplyResult['applied'][number] {
  statements.insertMemory.run(
    item.memoryId,
    input.repoId,
    item.observation.type,
    item.observation.title,
    item.observation.body,
    JSON.stringify(item.observation.concepts),
    cjkBigrams(`${item.observation.title} ${item.observation.body}`),
    item.material,
    item.content,
    mutation.sensitivity,
    // NULL only for provider output (data-model.md memories.degraded_reason).
    input.fallbackReason,
    input.sessionId,
    input.batchId,
    item.capturedAt,
    input.now,
  );

  db.prepare('UPDATE memories SET source_captured_at = ? WHERE id = ?').run(item.capturedAt, item.memoryId);

  insertObservationSources(statements, item, rowsById);

  if (mutation.supersedes !== null) {
    db.prepare('UPDATE memories SET valid_to = ?, superseded_by = ? WHERE id = ?').run(
      item.capturedAt,
      item.memoryId,
      mutation.supersedes,
    );
  }
  if (mutation.historicalTarget !== null) {
    db.prepare('UPDATE memories SET valid_to = ?, superseded_by = ? WHERE id = ?').run(
      mutation.historicalTarget.capturedAt ?? input.now, mutation.historicalTarget.id, item.memoryId,
    );
    return { index: item.index, decision: 'add', memoryId: item.memoryId,
      historical: { target: mutation.historicalTarget.id, reason: 'capture_time_order' } };
  }
  return { index: item.index, decision: mutation.decision, memoryId: item.memoryId };
}

function applyPreparedObservation(
  db: DatabaseSync,
  input: ApplyInput,
  item: Prepared,
  rowsById: Map<string, RawEventRow>,
  offered: Set<string>,
  statements: ApplyStatements,
  result: ApplyLists,
): void {
  const { classification } = item.observation;
  // R10: a target that was not among the supplied nearby ids is not a target at all.
  const target = classification.target !== null && offered.has(classification.target)
    ? classification.target
    : null;
  let decision: ApplyDecision = classification.decision;
  if (target === null && (decision === 'update' || decision === 'delete')) {
    decision = decision === 'update' ? 'add' : 'noop';
  }

  if (decision === 'delete') {
    const targetRow = statements.readTarget.get(target!, input.repoId);
    if (targetRow === undefined || targetRow.deleted_at !== null || sourceIsOlder(item, targetRow.source_captured_at)) {
      result.applied.push({ index: item.index, decision: 'noop', memoryId: null,
        historical: { target: target!, reason: targetRow === undefined || targetRow.deleted_at !== null ? 'target_unavailable' : 'capture_time_order' } });
      return;
    }
    result.applied.push(applyDelete(db, input, item, target, classification));
    return;
  }
  if (decision === 'noop') {
    const existing = target === null ? undefined : statements.readTarget.get(target, input.repoId);
    const confirmed = existing !== undefined && existing.deleted_at === null && existing.valid_to === null
      && existing.review_state !== 'imported' && existing.sensitivity !== 'secret' && classification.reason.trim() !== '';
    if (confirmed && input.fallbackReason === null) confirmExistingMemory(db, input, item, rowsById, target!);
    result.applied.push({ index: item.index, decision: 'noop', memoryId: confirmed ? target : null });
    return;
  }
  if (recordExisting(db, input, item, rowsById, statements.byContentHash, result)) return;

  const mutation = prepareMutation(input, item, rowsById, target, decision, statements.readTarget);
  result.applied.push(insertPreparedObservation(db, input, item, rowsById, statements, mutation));
}

function applyPreparedObservations(
  db: DatabaseSync,
  token: string,
  input: ApplyInput,
  prepared: Prepared[],
  rowsById: Map<string, RawEventRow>,
  dropped: ApplyResult['dropped'],
  checkpoint: PreparedCheckpoint | null,
): ApplyResult {
  if (!assertLease(db, token, input.now)) {
    db.exec('ROLLBACK');
    return { applied: [], suppressed: [], dropped: [], leaseLost: true };
  }

  const source = db.prepare('SELECT processing_offset, processing_hash FROM raw_events WHERE id = ? AND batch_id = ?');
  for (const portion of input.coverage ?? []) {
    if (portion.end === portion.start) continue;
    const current = source.get(portion.rowId, input.batchId);
    if (current?.processing_offset !== portion.start || current.processing_hash !== portion.sourceHash) {
      throw new Error('source_range_changed');
    }
  }

  const work = batchWork(db, input);
  if (work === undefined || db.prepare(`SELECT 1 FROM observation_batches WHERE id = ?
    AND owner_token = ? AND state IN ('pending', 'running')`).get(input.batchId, token) === undefined) {
    throw new Error('sharing_context_changed');
  }
  const sourceBinding = db.prepare('SELECT repo_id, session_id, work_binding_id FROM raw_events WHERE id = ?');
  for (const row of input.rows) {
    const current = sourceBinding.get(row.id);
    if (current?.repo_id !== input.repoId || current.session_id !== input.sessionId
      || current.work_binding_id !== work.work_binding_id) throw new Error('sharing_source_changed');
  }
  const audience = visibilityScope({ repoId: input.repoId, workId: work.work_id, personal: false });
  const target = db.prepare(`SELECT 1 FROM memories m WHERE m.id = ? AND ${audience.where}
    AND m.type <> 'session_summary' AND NOT EXISTS (SELECT 1 FROM memory_visibility p
      WHERE p.memory_id = m.id AND p.audience = 'personal')`);
  const offered = new Set<string>();
  for (const candidate of input.nearby) {
    if (candidate.repo_id !== input.repoId || !nearbyUnchanged(db, candidate)
      || target.get(candidate.id, ...audience.params) === undefined) throw new Error('sharing_context_changed');
    offered.add(candidate.id);
  }
  const parent = input.providedCheckpoint;
  if (parent !== undefined && (parent.repo_id !== input.repoId || parent.type !== 'session_summary'
    || parent.work_id !== work.work_id || parent.id !== work.checkpoint_parent_id || !nearbyUnchanged(db, parent)
    || db.prepare(`SELECT 1 FROM memory_visibility WHERE memory_id = ? AND audience = 'work'
      AND repo_id = ? AND work_id = ?`).get(parent.id, input.repoId, work.work_id) === undefined)) {
    throw new Error('sharing_context_changed');
  }

  const privacy = input.fallbackReason === null ? generationPrivacy(db, input) : null;
  if (checkpoint?.replacement !== undefined && privacy !== null) {
    checkpoint.replacement.sensitivity = strictest(checkpoint.replacement.sensitivity, privacy.sensitivity);
  }
  const checkpointResult = applyCheckpoint(db, input, checkpoint);
  // An unusable required progress decision must retain its retry opportunity. Ordinary source
  // accounting stays independent for valid, historical, conflicting and tombstoned checkpoints.
  const checkpointFailed = checkpointResult?.decision === 'rejected' && checkpointResult.reason !== 'tombstoned';
  if (checkpointFailed) input = { ...input, fallbackReason: 'unusable_output' };
  const result: ApplyLists = { applied: [], suppressed: [] };
  const statements = prepareApplyStatements(db);
  for (const item of checkpointFailed ? [] : prepared) {
    applyPreparedObservation(db, input, item, rowsById, offered, statements, result);
  }

  // Call policy 5: the batch reaches its terminal state in the same transaction as the mutations.
  db.prepare(
    'UPDATE observation_batches SET state = ?, completed_at = ?, degraded_reason = ? WHERE id = ?',
  ).run(
    input.fallbackReason === null ? 'applied' : 'fallback',
    input.now,
    input.fallbackReason,
    input.batchId,
  );

  recordSourceEvidence(db, input, result, checkpointResult);
  if (privacy !== null && !checkpointFailed) retainGenerationPrivacy(db, input, privacy, [
    ...result.applied.filter((item) => item.memoryId !== null && item.decision !== 'delete').map((item) => item.memoryId!),
    ...(checkpointResult?.memoryId == null ? [] : [checkpointResult.memoryId]),
  ]);
  settleSources(db, input, { ...result, dropped, leaseLost: false }, checkpointResult);
  recordObservationSharing(db, input, result.applied, prepared);

  return { ...result, dropped, leaseLost: false, ...(checkpointResult === null ? {} : { checkpoint: checkpointResult }),
    ...(checkpointFailed ? { fallbackReason: 'unusable_output' as const } : {}) };
}

type SourceOutcome = {
  outcome: 'processed' | 'deferred' | 'uncovered' | 'rejected';
  reason: string;
};

function outcomeForSource(input: ApplyInput, result: ApplyResult, id: string): SourceOutcome {
  const coverage = input.coverage?.find((row) => row.rowId === id);
  if (coverage?.state === 'omitted') return { outcome: 'uncovered', reason: 'not_sent' };
  if (input.fallbackReason !== null) return { outcome: 'deferred', reason: input.fallbackReason };
  if (coverage === undefined || coverage.end <= coverage.start) return { outcome: 'uncovered', reason: 'unaccounted' };

  const cited = new Set((input.output?.observations ?? []).flatMap((observation, index) =>
    observation.source_event_ids.includes(id) ? [index] : [],
  ));
  const rejected = result.dropped.find((item) => cited.has(item.index));
  if (rejected !== undefined) return { outcome: 'rejected', reason: rejected.reason };
  if (result.suppressed.some((item) => cited.has(item.index))) {
    return { outcome: 'processed', reason: 'tombstoned' };
  }
  for (const item of result.applied) {
    if (!cited.has(item.index)) continue;
    if (item.historical !== undefined) return { outcome: 'processed',
      reason: `historical_${input.output!.observations[item.index].classification.decision}` };
    if (item.decision !== 'noop' || item.memoryId !== null) {
      return { outcome: 'processed', reason: item.decision === 'noop' ? 'deduplicated' : item.decision };
    }
    const classification = input.output?.observations[item.index].classification;
    if (classification?.decision === 'noop' && classification.target === null && classification.reason.trim() !== '') {
      return { outcome: 'processed', reason: 'no_memory' };
    }
  }
  return { outcome: 'uncovered', reason: 'unaccounted' };
}

/** Store the exact admitted source, never a provider's response, beside each accepted citation. */
function recordSourceEvidence(db: DatabaseSync, input: ApplyInput, result: ApplyLists, checkpoint: CheckpointResult | null): void {
  if (input.fallbackReason !== null) return;
  const live = db.prepare('SELECT 1 FROM memories WHERE id = ? AND repo_id = ? AND deleted_at IS NULL');
  const existing = db.prepare(`SELECT 1 FROM memory_sources WHERE memory_id = ? AND raw_event_id = ? AND context_only = 0
    AND source_hash = ? AND portion_start = ? AND portion_end = ?`);
  const empty = db.prepare(`SELECT id FROM memory_sources WHERE memory_id = ? AND raw_event_id = ? AND context_only = 0
    AND evidence IS NULL ORDER BY id LIMIT 1`);
  const update = db.prepare(`UPDATE memory_sources SET portion_start = ?, portion_end = ?, source_total = ?,
    source_hash = ?, evidence = ?, captured_at = ?, capture_root = ?, source_paths_json = ?, source_context_id = ? WHERE id = ?`);
  const insert = db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, source_agent,
    portion_start, portion_end, source_total, source_hash, evidence, captured_at, capture_root, source_paths_json, source_context_id)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`);
  const context = db.prepare('SELECT context_id FROM work_bindings WHERE id = ?');
  const accepted = result.applied.filter((item) => item.memoryId !== null && item.decision !== 'delete')
    .map((item) => ({ memoryId: item.memoryId!, sourceIds: input.output!.observations[item.index].source_event_ids }));
  if (checkpoint?.memoryId !== null && checkpoint?.memoryId !== undefined) accepted.push({
    memoryId: checkpoint.memoryId, sourceIds: checkpoint.sourceIds,
  });
  for (const item of accepted) {
    // A later action in the same response may already have deleted this earlier confirmation.
    if (live.get(item.memoryId, input.repoId) === undefined) continue;
    for (const portion of input.coverage ?? []) {
      if (portion.end <= portion.start || !item.sourceIds.includes(portion.rowId)) continue;
      if (existing.get(item.memoryId, portion.rowId, portion.sourceHash, portion.start, portion.end) !== undefined) continue;
      const row = input.rows.find((row) => row.id === portion.rowId)!;
      const payload = payloadOf(row);
      const values = [portion.start, portion.end, portion.total, portion.sourceHash, portion.text, row.captured_at,
        typeof payload?.capture_root === 'string' ? payload.capture_root : null,
        Array.isArray(payload?.source_paths) ? JSON.stringify(payload.source_paths) : null,
        context.get(row.work_binding_id ?? null)?.context_id ?? null];
      const link = empty.get(item.memoryId, portion.rowId);
      if (link !== undefined) update.run(...values, link.id);
      else insert.run(item.memoryId, portion.rowId, row.agent, ...values);
    }
  }
}

/** Attempt history stays attached to the batch; unresolved sources return to the due queue. */
function settleSources(db: DatabaseSync, input: ApplyInput, result: ApplyResult, checkpoint: CheckpointResult | null): void {
  const source = db.prepare('SELECT processing_attempts FROM raw_events WHERE id = ? AND batch_id = ?');
  const receipt = db.prepare(`INSERT INTO observation_batch_sources
    (batch_id, raw_event_id, turn_id, outcome, reason, recorded_at, historical_actions_json) VALUES (?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(batch_id, raw_event_id) DO UPDATE SET
      turn_id = excluded.turn_id, outcome = excluded.outcome, reason = excluded.reason,
      recorded_at = excluded.recorded_at, historical_actions_json = excluded.historical_actions_json`);
  const processed = db.prepare(`UPDATE raw_events SET processing_state = 'processed', processed_at = ?,
    expires_at = ?, retry_after = NULL, processing_offset = ?, processing_attempts = 0
    WHERE id = ? AND batch_id = ? AND processing_offset = ? AND processing_hash = ?`);
  const progress = db.prepare(`UPDATE raw_events SET processing_state = 'pending', batch_id = NULL,
    retry_after = NULL, processing_offset = ?, processing_attempts = 0
    WHERE id = ? AND batch_id = ? AND processing_offset = ? AND processing_hash = ?`);
  const unsent = db.prepare(`UPDATE raw_events SET processing_state = 'pending', batch_id = NULL,
    retry_after = NULL WHERE id = ? AND batch_id = ?`);
  const waiting = db.prepare(`UPDATE raw_events SET processing_state = 'waiting', batch_id = NULL,
    processed_at = NULL, retry_after = ?, processing_attempts = processing_attempts + 1
    WHERE id = ? AND batch_id = ? AND processing_state IN ('pending', 'waiting')`);

  for (const row of input.rows) {
    const stored = source.get(row.id, input.batchId);
    if (stored === undefined) continue;
    const outcome: SourceOutcome = row.classification_state === 'partial'
      ? { outcome: 'deferred', reason: 'partial_capture' } : outcomeForSource(input, result, row.id);
    const portion = input.coverage?.find((portion) => portion.rowId === row.id);
    const history = result.applied.flatMap((item) => {
      const observation = input.output?.observations[item.index];
      return item.historical === undefined || !observation?.source_event_ids.includes(row.id) ? []
        : [{ decision: observation.classification.decision, ...item.historical }];
    });
    receipt.run(input.batchId, row.id, row.turn_id, outcome.outcome, outcome.reason, input.now,
      history.length === 0 ? null : JSON.stringify(history));
    if (outcome.outcome === 'processed') {
      if (portion!.end === portion!.total) {
        processed.run(input.now, input.now + PROCESSED_RETENTION_MS, portion!.end,
          row.id, input.batchId, portion!.start, portion!.sourceHash);
        db.prepare('UPDATE memory_sources SET source_processed_at = ? WHERE raw_event_id = ?').run(input.now, row.id);
      } else {
        progress.run(portion!.end, row.id, input.batchId, portion!.start, portion!.sourceHash);
      }
    } else if (outcome.reason === 'not_sent') {
      unsent.run(row.id, input.batchId);
    } else {
      waiting.run(row.classification_state === 'partial' ? null : sourceRetryAt(input.now, Number(stored.processing_attempts)), row.id, input.batchId);
    }
  }
  if (input.fallbackReason === null) {
    const upgrade = db.prepare(`UPDATE memories AS m SET degraded_reason = NULL,
      source_session_id = ?, source_batch_id = ?
      WHERE m.id = ? AND m.repo_id = ? AND m.degraded_reason IS NOT NULL AND m.deleted_at IS NULL
        AND NOT EXISTS (${UNPROCESSED_MEMORY_SOURCES})`);
    const confirmed = result.applied.filter((item) => item.memoryId !== null && item.decision !== 'delete').map((item) => item.memoryId!);
    if (checkpoint?.decision === 'confirmed' && checkpoint.memoryId !== null) confirmed.push(checkpoint.memoryId);
    for (const id of confirmed) upgrade.run(input.sessionId, input.batchId, id, input.repoId);
    db.prepare(`UPDATE memories AS m SET valid_to = ?
      WHERE m.source_session_id = ? AND m.type <> 'session_summary' AND m.degraded_reason IS NOT NULL
        AND m.deleted_at IS NULL AND m.valid_to IS NULL
        AND EXISTS (SELECT 1 FROM memory_sources ms JOIN raw_events r ON r.id = ms.raw_event_id
          WHERE ms.memory_id = m.id AND ms.context_only = 0 AND r.processing_state = 'processed')
        AND NOT EXISTS (${UNPROCESSED_MEMORY_SOURCES})`)
      .run(input.now, input.sessionId);
  }
  db.prepare("UPDATE sessions SET summary_state = 'pending', summary_updated_at = NULL WHERE id = ?")
    .run(input.sessionId);
}

/**
 * The whole result of one batch in one fenced transaction: every memory mutation and the batch's
 * terminal state commit together, so a lost lease discards everything and a repeated provider call
 * can never apply twice (A11, call policy 5).
 */
export async function applyObservations(
  db: DatabaseSync,
  token: string,
  input: ApplyInput,
): Promise<ApplyResult> {
  const rowsById = new Map(input.rows.map((row) => [row.id, row]));
  const dropped: ApplyResult['dropped'] = [];
  const prepared: Prepared[] = [];

  // The detector is asynchronous, so every observation is prepared before the transaction opens.
  for (const [index, raw] of (input.output?.observations ?? []).entries()) {
    const title = await input.detect(raw.title);
    const body = await input.detect(raw.body);
    // FR-018: a detector failure drops the observation; it is never stored unredacted.
    if (!title.ok || !body.ok) {
      dropped.push({ index, reason: 'detector_failed' });
      continue;
    }
    const observation: Observation = { ...raw, title: title.text, body: body.text };
    if (rejectsDirectives(observation.title) !== null || rejectsDirectives(observation.body) !== null) {
      dropped.push({ index, reason: 'directive' });
      continue;
    }
    // contracts/observer.md: the ids must belong to this batch; the fallback path never went
    // through validateObserverOutput, so the check is repeated here against the batch's own rows.
    const sourceIds = raw.source_event_ids.filter((id) => rowsById.has(id));
    if (sourceIds.length === 0) {
      dropped.push({ index, reason: 'unknown_source' });
      continue;
    }
    // A clean detector run contributes the loosest class; only a finding makes the memory secret.
    const detectorClass = strictest(
      promoteSensitivity('eligible', title, 'done'),
      promoteSensitivity('eligible', body, 'done'),
    );
    const material = materialHash(observation.title, observation.body);
    const content = contentHash(input.repoId, material);
    const times = sourceIds.map((id) => rowsById.get(id)?.captured_at ?? null);
    prepared.push({
      index,
      observation,
      sourceIds,
      detectorClass,
      material,
      content,
      memoryId: memoryIdFor(content),
      capturedAt: times.every((time): time is number => time !== null) ? Math.max(...times) : null,
    });
  }

  const checkpoint = await prepareCheckpoint(db, input);
  return transactionImmediate(db, () =>
    applyPreparedObservations(db, token, input, prepared, rowsById, dropped, checkpoint),
  );
}
