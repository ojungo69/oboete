import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { checkpointHash, materialHash, memoryIdFor } from '../db/identity.js';
import { grantVisibility } from '../db/queries.js';
import { promoteSensitivity, strictest } from '../privacy/classify.js';
import type { Sensitivity } from '../privacy/egress.js';
import { cjkBigrams } from '../retrieval/fts.js';
import type { ApplyInput } from './apply.js';
import { INSERT_MEMORY, rejectsDirectives } from './classify.js';
import { checkpointSchema, checkpointText } from './contract.js';

type WorkBatch = { work_id: string; work_binding_id: string; checkpoint_parent_id: string | null };
type Checkpoint = { id: string; material_hash: string; sensitivity: Sensitivity;
  source_captured_at: number | null; valid_from: number | null; deleted_at: number | null; valid_to: number | null };
type Replacement = { title: string; body: string; material: string; content: string; memoryId: string;
  sensitivity: Sensitivity; capturedAt: number | null };
export type PreparedCheckpoint = WorkBatch & { decision: 'replace' | 'unchanged' | 'rejected';
  reason: string; sourceIds: string[]; replacement?: Replacement };
export type CheckpointResult = { decision: string; reason: string; memoryId: string | null; sourceIds: string[] };
export function batchWork(db: DatabaseSync, input: Pick<ApplyInput, 'batchId' | 'repoId' | 'sessionId'>): WorkBatch | undefined {
  return db.prepare(`SELECT w.id AS work_id, b.work_binding_id, b.checkpoint_parent_id
    FROM observation_batches b JOIN work_bindings binding ON binding.id = b.work_binding_id
    JOIN work_items w ON w.id = binding.work_id JOIN sessions s ON s.id = binding.session_id
    JOIN work_contexts c ON c.id = binding.context_id
    WHERE b.id = ? AND b.repo_id = ? AND b.session_id = ? AND w.repo_id = b.repo_id
      AND binding.session_id = b.session_id AND s.repo_id = b.repo_id AND c.repo_id = b.repo_id`)
    .get(input.batchId, input.repoId, input.sessionId) as WorkBatch | undefined;
}

function readCheckpoint(db: DatabaseSync, workId: string, id: string | null): Checkpoint | undefined {
  return db.prepare(`SELECT id, material_hash, sensitivity, source_captured_at, valid_from, deleted_at, valid_to
    FROM memories WHERE id = ? AND work_id = ?`).get(id, workId) as Checkpoint | undefined;
}

/** Source membership, detection and formatting finish before the existing apply transaction. */
export async function prepareCheckpoint(db: DatabaseSync, input: ApplyInput): Promise<PreparedCheckpoint | null> {
  if (input.fallbackReason !== null || input.output === null || !('checkpoint' in input.output)) return null;
  const work = batchWork(db, input);
  if (work === undefined) return null;
  const parsed = checkpointSchema.safeParse(input.output.checkpoint);
  const rejected = (reason: string): PreparedCheckpoint => ({ ...work, decision: 'rejected', reason, sourceIds: [] });
  if (!parsed.success) return rejected('unusable_output');
  const choice = parsed.data;
  const sourceIds = [...new Set(choice.source_event_ids)];
  const rows = sourceIds.map((id) => input.rows.find((row) => row.id === id));
  if (rows.some((row) => row === undefined || row.work_binding_id !== work.work_binding_id
    || row.repo_id !== input.repoId || row.session_id !== input.sessionId)
    || sourceIds.some((id) => !input.coverage?.some((portion) => portion.rowId === id && portion.end > portion.start))) {
    return rejected('source_not_admitted');
  }
  if (choice.decision === 'unchanged') return { ...work, decision: 'unchanged', reason: 'provider_unchanged', sourceIds };
  const parent = readCheckpoint(db, work.work_id, work.checkpoint_parent_id);
  if (work.checkpoint_parent_id !== null && (parent === undefined || parent.deleted_at !== null)) return rejected('parent_unavailable');
  const rendered = checkpointText(choice);
  const title = await input.detect(rendered.title);
  const body = await input.detect(rendered.body);
  if (!title.ok || !body.ok) return rejected('detector_failed');
  if (rejectsDirectives(title.text) !== null || rejectsDirectives(body.text) !== null) return rejected('directive');
  const sensitivity = strictest(parent?.sensitivity ?? 'eligible',
    promoteSensitivity('eligible', title, 'done'), promoteSensitivity('eligible', body, 'done'),
    ...rows.map((row) => row!.sensitivity));
  if (sensitivity === 'secret') return rejected('secret');
  const material = materialHash(title.text, body.text);
  const content = checkpointHash(input.repoId, work.work_id, work.checkpoint_parent_id, material);
  const times = rows.map((row) => row!.captured_at);
  return { ...work, decision: 'replace', reason: 'provider_replacement', sourceIds, replacement: {
    title: title.text, body: body.text, material, content, memoryId: memoryIdFor(content), sensitivity,
    capturedAt: times.every((at): at is number => at !== null) ? Math.max(...times) : null,
  } };
}

/** Called only inside the lease-fenced apply transaction; the provider never chooses a work ID. */
export function applyCheckpoint(db: DatabaseSync, input: ApplyInput, prepared: PreparedCheckpoint | null): CheckpointResult | null {
  if (prepared === null) return null;
  const work = batchWork(db, input);
  if (work?.work_id !== prepared.work_id || work.work_binding_id !== prepared.work_binding_id
    || work.checkpoint_parent_id !== prepared.checkpoint_parent_id) throw new Error('checkpoint_context_changed');
  const receipt = (decision: string, reason: string, memoryId: string | null = null): CheckpointResult => {
    if (memoryId !== null) grantVisibility(db, memoryId,
      { audience: 'work', repoId: input.repoId, workId: work.work_id }, 'observer', input.now);
    db.prepare(`UPDATE observation_batches SET checkpoint_decision = ?, checkpoint_reason = ?, checkpoint_memory_id = ?,
      checkpoint_source_ids_json = ? WHERE id = ?`)
      .run(decision, reason, memoryId, JSON.stringify(prepared.sourceIds), input.batchId);
    return { decision, reason, memoryId, sourceIds: prepared.sourceIds };
  };
  if (prepared.replacement === undefined) return receipt(prepared.decision, prepared.reason);
  const item = prepared.replacement;
  const currentId = db.prepare('SELECT current_checkpoint_memory_id FROM work_items WHERE id = ?').get(work.work_id)?.current_checkpoint_memory_id as string | null;
  const current = readCheckpoint(db, work.work_id, currentId);
  if (db.prepare('SELECT 1 FROM memories WHERE work_id = ? AND material_hash = ? AND deleted_at IS NOT NULL LIMIT 1')
    .get(work.work_id, item.material) !== undefined) return receipt('rejected', 'tombstoned');
  const parent = readCheckpoint(db, work.work_id, work.checkpoint_parent_id);
  if (work.checkpoint_parent_id !== null && (parent === undefined || parent.deleted_at !== null)) return receipt('rejected', 'parent_unavailable');
  item.sensitivity = strictest(item.sensitivity, parent?.sensitivity ?? 'eligible');
  if (item.sensitivity === 'secret') return receipt('rejected', 'secret');
  if (current !== undefined && current.deleted_at === null && current.valid_to === null && current.material_hash === item.material) {
    db.prepare(`UPDATE memories SET sensitivity = ?, source_captured_at = MAX(COALESCE(source_captured_at, ?), COALESCE(?, source_captured_at)) WHERE id = ?`)
      .run(strictest(current.sensitivity, item.sensitivity), item.capturedAt, item.capturedAt, current.id);
    return receipt('confirmed', 'same_content', current.id);
  }

  const existing = readCheckpoint(db, work.work_id, item.memoryId);
  if (existing === undefined) {
    db.prepare(INSERT_MEMORY).run(item.memoryId, input.repoId, 'session_summary', item.title, item.body, '[]',
      cjkBigrams(`${item.title} ${item.body}`), item.material, item.content, item.sensitivity,
      input.fallbackReason, input.sessionId, input.batchId, item.capturedAt, input.now);
    db.prepare('UPDATE memories SET work_id = ?, checkpoint_parent_id = ?, source_captured_at = ? WHERE id = ?')
      .run(work.work_id, work.checkpoint_parent_id, item.capturedAt, item.memoryId);
  }
  const floor = current?.source_captured_at ?? current?.valid_from ?? null;
  if (current !== undefined && (item.capturedAt === null || floor === null || item.capturedAt < floor)) {
    db.prepare('UPDATE memories SET valid_to = ?, superseded_by = ? WHERE id = ?')
      .run(floor ?? input.now, current.id, item.memoryId);
    return receipt('historical', 'capture_time_order', item.memoryId);
  }
  if (currentId !== work.checkpoint_parent_id) {
    const local = JSON.stringify({ work_id: work.work_id, checkpoint_memory_id: currentId });
    const remote = JSON.stringify({ work_id: work.work_id, checkpoint_memory_id: item.memoryId, parent_memory_id: work.checkpoint_parent_id });
    db.prepare(`INSERT INTO sync_conflicts (id, repo_id, content_hash, local_state_json, remote_state_json, status, created_at)
      SELECT ?, ?, ?, ?, ?, 'open', ? WHERE NOT EXISTS (SELECT 1 FROM sync_conflicts WHERE repo_id = ?
        AND status = 'open' AND local_state_json = ? AND remote_state_json = ?)`)
      .run(randomUUID(), input.repoId, item.content, local, remote, input.now, input.repoId, local, remote);
    return receipt('conflict', 'parent_changed', item.memoryId);
  }
  if (existing !== undefined && existing.valid_to !== null) return receipt('historical', 'already_retired', item.memoryId);
  const changed = db.prepare(`UPDATE work_items SET current_checkpoint_memory_id = ?, updated_at = ?
    WHERE id = ? AND repo_id = ? AND current_checkpoint_memory_id IS ?`)
    .run(item.memoryId, input.now, work.work_id, input.repoId, work.checkpoint_parent_id);
  if (Number(changed.changes) !== 1) throw new Error('checkpoint_context_changed');
  if (parent !== undefined) db.prepare('UPDATE memories SET valid_to = ?, superseded_by = ? WHERE id = ?')
    .run(item.capturedAt ?? input.now, item.memoryId, parent.id);
  return receipt('replaced', 'provider_replacement', item.memoryId);
}
