import type { DatabaseSync } from 'node:sqlite';

import { visibilityUnchanged, type NearbyCandidate } from '../db/queries.js';
import { contentHash } from '../events.js';
import { strictest } from '../privacy/classify.js';
import type { Sensitivity } from '../privacy/egress.js';
import { canonicalContexts, memoryContexts, sourceContext, type SourceContext } from '../privacy/source-context.js';
import type { ApplyInput } from './apply.js';

export function nearbyUnchanged(db: DatabaseSync, candidate: NearbyCandidate): boolean {
  const current = db.prepare(`SELECT type, content_hash, material_hash, title, body, sensitivity, review_state, deleted_at, valid_to,
    work_id, checkpoint_parent_id, provenance_complete
    FROM memories WHERE id = ? AND repo_id = ?`).get(candidate.id, candidate.repo_id);
  return current !== undefined && current.type === candidate.type && current.content_hash === candidate.content_hash && current.title === candidate.title
    && current.body === candidate.body && current.sensitivity === candidate.sensitivity
    && current.review_state === candidate.review_state && (current.deleted_at !== null) === candidate.deleted
    && (candidate.valid_to === undefined || current.valid_to === candidate.valid_to)
    && (candidate.work_id === undefined || (current.work_id === candidate.work_id
      && current.checkpoint_parent_id === candidate.checkpoint_parent_id
      && current.provenance_complete === candidate.provenance_complete
      && current.material_hash === candidate.material_hash))
    && (candidate.privacy_stamp === undefined || candidate.privacy_stamp === contentHash(JSON.stringify(memoryContexts(db, candidate))))
    && visibilityUnchanged(db, candidate);
}


type SourceMemory = { id: string; sensitivity: Sensitivity; deleted_at: number | null;
  work_id: string | null; provenance_complete: number | null; source_batch_id: string | null };
type GenerationPrivacy = { sensitivity: Sensitivity; contexts: SourceContext[] | null;
  rawIds: string[]; memoryIds: string[] };

function readMemory(db: DatabaseSync, repoId: string, id: string): SourceMemory | undefined {
  return db.prepare(`SELECT id, sensitivity, deleted_at, work_id, provenance_complete, source_batch_id
    FROM memories WHERE id = ? AND repo_id = ?`).get(id, repoId) as SourceMemory | undefined;
}

/** Called inside apply's transaction: citations cannot downgrade any text the provider received. */
export function generationPrivacy(db: DatabaseSync, input: ApplyInput): GenerationPrivacy {
  const contexts: SourceContext[] = [];
  let complete = true;
  let sensitivity: Sensitivity = 'eligible';
  const rawIds: string[] = [];
  const memoryIds: string[] = [];
  const raw = db.prepare('SELECT sensitivity, classification_state FROM raw_events WHERE id = ? AND repo_id = ? AND batch_id = ?');
  for (const portion of input.coverage ?? []) {
    if (portion.end <= portion.start) continue;
    const row = input.rows.find((row) => row.id === portion.rowId && row.repo_id === input.repoId);
    const current = raw.get(portion.rowId, input.repoId, input.batchId);
    if (row === undefined || current === undefined) { sensitivity = 'secret'; complete = false; continue; }
    rawIds.push(row.id);
    sensitivity = strictest(sensitivity, row.sensitivity,
      current.classification_state === 'done' ? current.sensitivity as Sensitivity : 'secret');
    contexts.push(sourceContext(db, row));
  }
  const ids = new Set(input.nearby.map((memory) => memory.id));
  if (input.providedCheckpoint !== undefined) ids.add(input.providedCheckpoint.id);
  for (const id of ids) {
    const memory = readMemory(db, input.repoId, id);
    if (memory === undefined || memory.deleted_at !== null) { sensitivity = 'secret'; complete = false; continue; }
    memoryIds.push(id);
    sensitivity = strictest(sensitivity, memory.sensitivity,
      input.nearby.find((candidate) => candidate.id === id)?.sensitivity ?? 'eligible');
    let inherited = memoryContexts(db, memory);
    // Legacy source-less facts already use the verified request root for admission. Linked
    // incomplete sources and new provenance_complete=0 memories never enter this allowance.
    if (memory.work_id === null && inherited?.length === 1 && inherited[0].root === null
      && db.prepare('SELECT 1 FROM memory_sources WHERE memory_id = ? AND raw_event_id IS NOT NULL LIMIT 1').get(id) === undefined) {
      const current = db.prepare(`SELECT c.id, c.root FROM observation_batches batch
        JOIN work_bindings b ON b.id = batch.work_binding_id JOIN work_contexts c ON c.id = b.context_id
        WHERE batch.id = ? AND c.repo_id = ?`).get(input.batchId, input.repoId);
      if (current !== undefined) inherited = [{ root: String(current.root), contextId: String(current.id), paths: inherited[0].paths }];
    }
    if (inherited === null) complete = false;
    else contexts.push(...inherited);
  }
  return { sensitivity, contexts: complete && contexts.length > 0 ? canonicalContexts(contexts) : null, rawIds, memoryIds };
}

/** Flat admission proof plus direct dependency IDs; no ancestor evidence or raw-ID copies. */
export function retainGenerationPrivacy(db: DatabaseSync, input: ApplyInput, privacy: GenerationPrivacy, outputIds: string[]): void {
  const raw = db.prepare('INSERT OR IGNORE INTO memory_sources (memory_id, raw_event_id, context_only) VALUES (?, ?, 1)');
  const memory = db.prepare('INSERT OR IGNORE INTO memory_sources (memory_id, source_memory_id, context_only) VALUES (?, ?, 1)');
  const clearFlat = db.prepare(`DELETE FROM memory_sources WHERE memory_id = ?
    AND raw_event_id IS NULL AND source_memory_id IS NULL AND citation_kind IS NULL`);
  const flat = db.prepare(`INSERT INTO memory_sources
    (memory_id, capture_root, source_paths_json, source_context_id, context_only) VALUES (?, ?, ?, ?, 1)`);
  const update = db.prepare(`UPDATE memories SET sensitivity = ?,
    review_state = CASE WHEN ? = 'secret' THEN 'imported' ELSE review_state END WHERE id = ?`);
  const clearSecret = db.prepare(`UPDATE memory_sources SET evidence = NULL, capture_root = NULL,
    source_paths_json = NULL, citation_value = NULL WHERE memory_id = ?`);
  const descendants = db.prepare(`WITH RECURSIVE descendants(id) AS (
    SELECT ? UNION SELECT m.id FROM memories m JOIN descendants d ON m.checkpoint_parent_id = d.id
    UNION SELECT ms.memory_id FROM memory_sources ms JOIN descendants d ON ms.source_memory_id = d.id
  ) SELECT m.id, m.work_id, m.provenance_complete FROM memories m JOIN descendants d ON m.id = d.id
    WHERE m.repo_id = ? AND m.id <> ? AND m.provenance_complete IS NOT 0`);
  const setComplete = db.prepare('UPDATE memories SET provenance_complete = ? WHERE id = ?');
  const storeContexts = (id: string, contexts: SourceContext[] | null) => {
    clearFlat.run(id);
    if (contexts !== null) for (const context of contexts) flat.run(id, context.root, JSON.stringify(context.paths), context.contextId);
    setComplete.run(contexts === null ? 0 : 1, id);
  };
  for (const id of new Set(outputIds)) {
    const output = readMemory(db, input.repoId, id);
    if (output === undefined || output.deleted_at !== null) continue;
    const prior = output.provenance_complete === null && output.source_batch_id === input.batchId
      ? [] : memoryContexts(db, output);
    const contexts = privacy.contexts === null || prior === null ? null : canonicalContexts([...privacy.contexts, ...prior]);
    const sensitivity = strictest(output.sensitivity, privacy.sensitivity);
    for (const rawId of privacy.rawIds) raw.run(id, rawId);
    for (const memoryId of privacy.memoryIds) if (memoryId !== id) memory.run(id, memoryId);
    storeContexts(id, sensitivity === 'secret' ? null : contexts);
    update.run(sensitivity, sensitivity, id);
    // A newly inserted secret already has that label, so the UPDATE trigger may not run.
    if (sensitivity === 'secret') clearSecret.run(id);
    if (JSON.stringify(prior) !== JSON.stringify(contexts)) {
      // Confirmation can extend an already referenced memory. Update its dependent component
      // here, inside the transaction, so readers keep using bounded flat proof without recursion.
      for (const row of descendants.iterate(id, input.repoId, id)) {
        const child = row as Pick<SourceMemory, 'id' | 'work_id' | 'provenance_complete'>;
        const previous = memoryContexts(db, child);
        storeContexts(child.id, contexts === null || previous === null ? null : canonicalContexts([...contexts, ...previous]));
      }
    }
  }
}
