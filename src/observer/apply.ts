// The apply step: what a summarizer's observations are allowed to change (T038).
// Sources: contracts/observer.md ("Worker rules after either path", "Session summary", "Call
// policy" item 5), research.md R10, R11 and R12, data-model.md (memories, memory_sources,
// sessions.summary_state), spec FR-014, FR-018, FR-029, FR-035, FR-042, amendments A11 and A13.
// Security-owned (plan.md "Structure Decision"): the target restriction, the tombstone check and
// the sensitivity lattice live here and nowhere else.
import type { DatabaseSync } from 'node:sqlite';

import { contentHash, materialHash, memoryIdFor } from '../db/identity.js';
import type { NearbyCandidate } from '../db/queries.js';
import { promoteSensitivity, strictest } from '../privacy/classify.js';
import type { DetectorResult } from '../privacy/detect.js';
import type { Sensitivity } from '../privacy/egress.js';
import { cjkBigrams } from '../retrieval/fts.js';
import type { RawEventRow } from '../worker/batches.js';
import { assertLease, transactionImmediate } from '../worker/lease.js';
import { INSERT_MEMORY, INSERT_SOURCE, rejectsDirectives, type DegradedReason } from './classify.js';
import type { Observation, ObserverOutput } from './contract.js';

export type ApplyDecision = 'add' | 'update' | 'delete' | 'noop';

export type ApplyInput = {
  batchId: string;
  repoId: string;
  sessionId: string;
  output: ObserverOutput | null;
  fallbackReason: DegradedReason | null;
  rows: RawEventRow[];
  nearby: NearbyCandidate[];
  detect: (text: string) => Promise<DetectorResult>;
  now: number;
};

export type ApplyResult = {
  applied: { index: number; decision: ApplyDecision; memoryId: string | null }[];
  suppressed: { index: number; contentHash: string }[];
  dropped: { index: number; reason: 'detector_failed' | 'directive' | 'unknown_source' }[];
  leaseLost: boolean;
};

type Prepared = {
  index: number;
  observation: Observation;
  sourceIds: string[];
  detectorClass: Sensitivity;
  material: string;
  content: string;
  memoryId: string;
};

type ApplyStatements = ReturnType<typeof prepareApplyStatements>;
type ApplyLists = Pick<ApplyResult, 'applied' | 'suppressed'>;

type PreparedMutation = {
  decision: ApplyDecision;
  sensitivity: Sensitivity;
  supersedes: string | null;
};

function prepareApplyStatements(db: DatabaseSync) {
  return {
    byContentHash: db.prepare('SELECT id, deleted_at FROM memories WHERE content_hash = ?'),
    readTarget: db.prepare(
      'SELECT id, sensitivity, deleted_at FROM memories WHERE id = ? AND repo_id = ?',
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
  item: Prepared,
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
  result.applied.push({ index: item.index, decision: 'noop', memoryId: String(existing.id) });
  return true;
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
  if (decision === 'update' && target !== null) {
    const targetRow = readTarget.get(target, input.repoId);
    if (targetRow?.deleted_at !== null) {
      // The target is gone or tombstoned: the content is still worth keeping, but it
      // supersedes nothing and a tombstone stays a tombstone.
      decision = 'add';
    } else {
      supersedes = target;
      // max(target, every source row, detector): an eligible update cannot relax a stricter
      // target (contracts/observer.md, tested against the outbound body).
      sensitivity = strictest(sensitivity, targetRow.sensitivity as Sensitivity);
    }
  }
  return { decision, sensitivity, supersedes };
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
    input.now,
    input.now,
  );

  insertObservationSources(statements, item, rowsById);

  if (mutation.supersedes !== null) {
    db.prepare('UPDATE memories SET valid_to = ?, superseded_by = ? WHERE id = ?').run(
      input.now,
      item.memoryId,
      mutation.supersedes,
    );
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
    result.applied.push(applyDelete(db, input, item, target, classification));
    return;
  }
  if (decision === 'noop') {
    result.applied.push({ index: item.index, decision: 'noop', memoryId: null });
    return;
  }
  if (recordExisting(item, statements.byContentHash, result)) return;

  const mutation = prepareMutation(input, item, rowsById, target, decision, statements.readTarget);
  result.applied.push(insertPreparedObservation(db, input, item, rowsById, statements, mutation));
}

function applyPreparedObservations(
  db: DatabaseSync,
  token: string,
  input: ApplyInput,
  prepared: Prepared[],
  rowsById: Map<string, RawEventRow>,
  offered: Set<string>,
  dropped: ApplyResult['dropped'],
): ApplyResult {
  if (!assertLease(db, token, input.now)) {
    db.exec('ROLLBACK');
    return { applied: [], suppressed: [], dropped: [], leaseLost: true };
  }

  const result: ApplyLists = { applied: [], suppressed: [] };
  const statements = prepareApplyStatements(db);
  for (const item of prepared) {
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

  return { ...result, dropped, leaseLost: false };
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
  // R10: only the candidates that were actually offered, and only of this repository.
  const offered = new Set(
    input.nearby.filter((candidate) => candidate.repo_id === input.repoId).map((candidate) => candidate.id),
  );

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
    prepared.push({
      index,
      observation,
      sourceIds,
      detectorClass,
      material,
      content,
      memoryId: memoryIdFor(content),
    });
  }

  return transactionImmediate(db, () =>
    applyPreparedObservations(db, token, input, prepared, rowsById, offered, dropped),
  );
}
