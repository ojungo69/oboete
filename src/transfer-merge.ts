import type { DatabaseSync } from 'node:sqlite';

import { contentHash, memoryIdFor } from './db/identity.js';
import { grantVisibility } from './db/queries.js';
import { sha256Hex, sha256Json } from './hash.js';
import { verifiedRepoContext } from './privacy/source-context.js';
import { isCanonicalRemoteIdentity } from './repo-identity.js';
import { cjkBigrams } from './retrieval/fts.js';
import type { ClaudeMemNormalizedRecord } from './transfer-claude-mem.js';
import { CLAUDE_MEM_FORMAT, EXPORT_FORMAT, SENSITIVITY_RANK, migrationPayloadRedaction, type ImportOptions, type ImportResult,
  type NativeMemory, type NativeRecord, type NativeSource } from './transfer-format.js';
import { TransferInputError, type TransferPlan } from './transfer-plan.js';

type Existing = { id: string; repo_id: string; sensitivity: keyof typeof SENSITIVITY_RANK;
  deleted_at: number | null; owned: number };

function mappings(plan: TransferPlan, db: DatabaseSync | undefined, options: ImportOptions) {
  const selectTarget = db?.prepare('SELECT id, identity_kind, normalized_identity FROM repos WHERE id = ?');
  const update = plan.db.prepare('UPDATE transfer_rows SET destination_repo_id = ?, destination_context_id = ? WHERE kind = ? AND origin = ?');
  const unresolved: { line: number; reason: string }[] = [];
  for (const row of plan.db.prepare("SELECT * FROM transfer_rows WHERE kind = 'repo' ORDER BY origin").iterate()) {
    const repo = JSON.parse(String(row.data)) as Extract<NativeRecord, { kind: 'repo' }>;
    const external = plan.format === CLAUDE_MEM_FORMAT;
    const exact = external ? options.mapProject?.[repo.normalized_identity] : undefined;
    const hashed = external ? options.mapProjectHash?.[repo.id.slice('project:'.length)] : undefined;
    if (exact !== undefined && hashed !== undefined) throw new TransferInputError('duplicate_project_mapping');
    const mapped = external ? exact ?? hashed : options.mapRepo?.[repo.id];
    const target = selectTarget?.get(mapped ?? repo.id);
    let targetId: string | null = null;
    if (mapped !== undefined) {
      if (target !== undefined) targetId = mapped;
    } else if (!external && target !== undefined && target.identity_kind === repo.identity_kind
      && target.normalized_identity === repo.normalized_identity) targetId = repo.id;
    else if (!external && repo.identity_kind === 'remote' && isCanonicalRemoteIdentity(repo.normalized_identity)
      && sha256Hex(repo.normalized_identity).slice(0, 16) === repo.id) {
      targetId = repo.id;
      if (target === undefined) plan.db.prepare("UPDATE transfer_rows SET effect = 'create_repo' WHERE kind = 'repo' AND origin = ?").run(repo.id);
    }
    if (targetId === null) {
      if (unresolved.length < 100) unresolved.push({ line: Number(row.source_line), reason: external ? 'map_project_required' : 'map_repo_required' });
      continue;
    }
    const context = db === undefined ? null : verifiedRepoContext(db, targetId, options.mapContext?.[targetId]);
    update.run(targetId, context?.id ?? null, 'repo', repo.id);
  }
  for (const [source, target] of Object.entries(options.mapRepo ?? {})) {
    if (plan.db.prepare("SELECT 1 FROM transfer_rows WHERE kind = 'repo' AND origin = ?").get(source) === undefined
      || selectTarget?.get(target) === undefined) unresolved.push({ line: 0, reason: 'invalid_repo_mapping' });
  }
  for (const [source, target] of Object.entries(options.mapProject ?? {})) {
    if (plan.external?.projects.some((project) => project.source === source) !== true
      || selectTarget?.get(target) === undefined) unresolved.push({ line: 0, reason: 'invalid_project_mapping' });
  }
  for (const [hash, target] of Object.entries(options.mapProjectHash ?? {})) {
    if (!/^[0-9a-f]{64}$/u.test(hash) || plan.external?.projects.filter((project) => project.id === `project:${hash}`).length !== 1
      || selectTarget?.get(target) === undefined) unresolved.push({ line: 0, reason: 'invalid_project_hash_mapping' });
  }
  for (const [repoId, contextId] of Object.entries(options.mapContext ?? {})) {
    if (db === undefined || verifiedRepoContext(db, repoId, contextId) === null) unresolved.push({ line: 0, reason: 'invalid_context_mapping' });
  }
  for (const [source, target] of Object.entries(options.mapWork ?? {})) {
    const localWork = db?.prepare('SELECT repo_id FROM work_items WHERE id = ?').get(target);
    const origin = plan.db.prepare(`SELECT destination_repo_id FROM transfer_rows WHERE kind = 'repo'
      AND origin IN (SELECT repo_id FROM transfer_rows WHERE kind = 'visibility' AND json_extract(data, '$.work_id') = ?
        UNION SELECT repo_id FROM transfer_rows WHERE kind = 'memory' AND json_extract(data, '$.work_id') = ?)`)
      .all(source, source);
    if (localWork === undefined || origin.length !== 1 || origin[0].destination_repo_id !== localWork.repo_id) {
      unresolved.push({ line: 0, reason: 'invalid_work_mapping' });
    }
  }
  plan.db.exec(`UPDATE transfer_rows AS m SET
    destination_repo_id = (SELECT r.destination_repo_id FROM transfer_rows r WHERE r.kind = 'repo' AND r.origin = m.repo_id),
    destination_context_id = (SELECT r.destination_context_id FROM transfer_rows r WHERE r.kind = 'repo' AND r.origin = m.repo_id)
    WHERE m.kind <> 'repo' AND m.repo_id IS NOT NULL`);
  return { unresolved: unresolved.slice(0, 100) };
}

function identityOf(memory: NativeMemory, repoId: string) {
  const material = memory.material_hash;
  const content = memory.identity_domain === 'personal_projection'
    ? memory.content_hash : contentHash(repoId, material);
  return { content, id: memoryIdFor(content) };
}

function insertMemory(db: DatabaseSync, memory: NativeMemory, repoId: string, id: string, content: string): void {
  const withoutText = memory.deleted_at !== null || memory.sensitivity === 'secret';
  const title = withoutText ? '' : (memory.title ?? '');
  const body = withoutText ? '' : (memory.body ?? '');
  const sensitivity = SENSITIVITY_RANK[memory.sensitivity] > SENSITIVITY_RANK.local_only ? memory.sensitivity : 'local_only';
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, material_hash, content_hash,
    sensitivity, review_state, degraded_reason, valid_from, valid_to, deleted_at, created_at,
    cjk_bigrams, source_captured_at, provenance_complete)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, ?, ?, ?, ?, ?, 0)`)
    .run(id, repoId, memory.type, title, body, withoutText ? '[]' : memory.concepts, memory.material_hash,
      content, sensitivity, memory.degraded_reason, memory.valid_from, memory.valid_to, memory.deleted_at,
      memory.created_at, cjkBigrams(`${title} ${body}`), memory.source_captured_at);
}

function recordOrigin(plan: TransferPlan, record: NativeRecord | ClaudeMemNormalizedRecord) {
  const payload = JSON.stringify(record);
  const payloadHash = 'external_payload_hash' in record ? record.external_payload_hash : sha256Hex(payload);
  const origin = ['migration-origin-v1', plan.format, plan.revision, plan.originId, record.kind, sha256Hex(record.id), payloadHash];
  return { payload, payloadHash, originJson: JSON.stringify(origin), key: sha256Json(origin) };
}

function grantImported(db: DatabaseSync, plan: TransferPlan, memory: NativeMemory,
  repoId: string, id: string, options: ImportOptions): void {
  if (memory.identity_domain === 'personal_projection') return;
  if (plan.format === EXPORT_FORMAT || plan.format === CLAUDE_MEM_FORMAT) grantVisibility(db, id, { audience: 'project', repoId }, 'migration', options.now);
  for (const row of plan.db.prepare("SELECT data FROM transfer_rows WHERE kind = 'visibility' AND memory_id = ?").iterate(memory.id)) {
    const grant = JSON.parse(String(row.data)) as Extract<NativeRecord, { kind: 'visibility' }>;
    if (grant.audience === 'project') grantVisibility(db, id, { audience: 'project', repoId }, 'migration', options.now);
    else if (grant.audience === 'work' && grant.work_id !== null) {
      const workId = options.mapWork?.[grant.work_id];
      if (workId !== undefined) grantVisibility(db, id, { audience: 'work', repoId, workId }, 'migration', options.now);
    }
  }
}

function mergeMemory(db: DatabaseSync | undefined, plan: TransferPlan, row: Record<string, unknown>,
  options: ImportOptions, result: ImportResult): void {
  const memory = JSON.parse(String(row.data)) as NativeMemory;
  const repoId = String(row.destination_repo_id);
  const { id, content } = identityOf(memory, repoId);
  const existing = (plan.db.prepare('SELECT * FROM transfer_targets WHERE content_hash = ?').get(content)
    ?? db?.prepare('SELECT id, repo_id, sensitivity, deleted_at, 0 AS owned FROM memories WHERE content_hash = ?').get(content)) as Existing | undefined;
  let effect = 'inserted';
  let destinationId = id;
  const apply = options.dryRun !== true && db !== undefined;
  if (existing !== undefined) {
    destinationId = existing.id;
    effect = existing.deleted_at !== null ? 'held_by_tombstone' : existing.owned === 1 ? 'inserted' : 'matched_existing';
    if (existing.deleted_at !== null) result.unchanged += 1;
    else if (memory.deleted_at !== null) {
      if (apply) db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(memory.deleted_at, existing.id);
      result.tombstones += 1;
      effect = 'held_by_tombstone';
    } else if (SENSITIVITY_RANK[memory.sensitivity] > SENSITIVITY_RANK[existing.sensitivity]) {
      if (apply) db.prepare('UPDATE memories SET sensitivity = ? WHERE id = ?').run(memory.sensitivity, existing.id);
      result.updated += 1;
    } else result.unchanged += 1;
  } else {
    if (apply) {
      insertMemory(db, memory, repoId, id, content);
      if (memory.deleted_at === null && memory.sensitivity !== 'secret') grantImported(db, plan, memory, repoId, id, options);
    }
    if (memory.deleted_at === null) result.inserted += 1;
    else { result.tombstones += 1; effect = 'held_by_tombstone'; }
  }
  const sensitivity = existing === undefined ? (SENSITIVITY_RANK[memory.sensitivity] > SENSITIVITY_RANK.local_only ? memory.sensitivity : 'local_only')
    : SENSITIVITY_RANK[memory.sensitivity] > SENSITIVITY_RANK[existing.sensitivity] ? memory.sensitivity : existing.sensitivity;
  const deletedAt = existing?.deleted_at ?? memory.deleted_at;
  plan.db.prepare(`INSERT OR REPLACE INTO transfer_targets (content_hash, id, repo_id, sensitivity, deleted_at, owned)
    VALUES (?, ?, ?, ?, ?, ?)`).run(content, destinationId, existing?.repo_id ?? repoId, sensitivity, deletedAt, existing?.owned ?? 1);
  if (apply && existing?.owned === 1 && deletedAt === null && sensitivity !== 'secret') {
    grantImported(db, plan, memory, repoId, destinationId, options);
  }
  // A source-free personal identity is global. A foreign-repo duplicate keeps history only.
  const privateTarget = memory.identity_domain === 'personal_projection' && existing !== undefined && existing.repo_id !== repoId;
  plan.db.prepare('UPDATE transfer_rows SET destination_memory_id = ?, effect = ? WHERE sequence = ?')
    .run(privateTarget ? null : destinationId, privateTarget ? 'historical_held' : effect, Number(row.sequence));
}

function insertSource(db: DatabaseSync, id: string, source: NativeSource): void {
  // Native source identifiers and roots are retained in the private receipt, never local lineage.
  db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value,
    source_agent, portion_start, portion_end, source_total, source_hash, evidence, captured_at,
    source_processed_at, context_only) VALUES (?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(id, source.citation_kind, source.citation_value, source.source_agent, source.portion_start,
      source.portion_end, source.source_total, source.source_hash, source.evidence, source.captured_at,
      source.source_processed_at, source.context_only);
}

function saveOrigins(db: DatabaseSync, plan: TransferPlan, importId: string, apply: boolean): void {
  const insert = db.prepare(`INSERT OR IGNORE INTO migration_records (id, origin_key, origin_json, target_key, first_import_id,
    record_kind, payload_hash, payload_json, destination_repo_id, destination_memory_id, destination_context_id,
    effect, classification_state, detail_code, identity_domain, destination_context_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`);
  for (const row of plan.db.prepare("SELECT * FROM transfer_rows WHERE kind <> 'repo' ORDER BY sequence").iterate()) {
    const record = JSON.parse(String(row.data)) as NativeRecord | ClaudeMemNormalizedRecord;
    const memory = row.kind === 'memory' ? row : row.memory_id === null ? undefined
      : plan.db.prepare("SELECT * FROM transfer_rows WHERE kind = 'memory' AND origin = ?").get(row.memory_id);
    const repoId = typeof memory?.destination_repo_id === 'string' ? memory.destination_repo_id
      : typeof row.destination_repo_id === 'string' ? row.destination_repo_id : null;
    const memoryId = typeof memory?.destination_memory_id === 'string' ? memory.destination_memory_id : null;
    const contextId = typeof memory?.destination_context_id === 'string' ? memory.destination_context_id
      : typeof row.destination_context_id === 'string' ? row.destination_context_id : null;
    const effect = row.kind === 'excluded' ? 'excluded' : String(memory?.effect ?? 'support_only');
    const identityDomain = memory === undefined ? null : (JSON.parse(String(memory.data)) as NativeMemory).identity_domain;
    const target = memoryId === null ? undefined : db.prepare('SELECT deleted_at, sensitivity FROM memories WHERE id = ?').get(memoryId);
    const payloadState = record.kind === 'migration_origin' ? migrationPayloadRedaction(record.payload) : migrationPayloadRedaction(record);
    const redacted = target?.deleted_at != null || target?.sensitivity === 'secret'
      || payloadState !== null;
    const inherited = record.kind === 'migration_origin';
    const origin = inherited ? { key: record.id, originJson: record.origin_json, payloadHash: record.payload_hash,
      payload: record.payload === null ? null : JSON.stringify(record.payload) } : recordOrigin(plan, record);
    const withoutPayload = redacted || origin.payload === null;
    const targetKey = repoId === null ? 'unresolved' : `repo:${repoId}`;
    const prior = db.prepare('SELECT target_key, payload_hash FROM migration_records WHERE origin_key = ?').get(origin.key);
    if (prior !== undefined && (prior.target_key !== targetKey || prior.payload_hash !== origin.payloadHash)) {
      throw new TransferInputError('origin_mapping_changed', Number(row.source_line));
    }
    if (!apply) continue;
    const recordId = sha256Json(['migration-record-v1', origin.key, targetKey]);
    const secret = target?.sensitivity === 'secret' || payloadState === 'secret' || (inherited && record.classification_state === 'secret');
    const inserted = insert.run(recordId, origin.key, origin.originJson, targetKey, importId,
      inherited ? record.record_kind : record.kind, origin.payloadHash, withoutPayload ? null : origin.payload,
      repoId, memoryId, contextId, effect, secret ? 'secret' : withoutPayload ? 'not_applicable' : 'pending',
      withoutPayload ? 'source_redacted' : 'classification_pending', identityDomain,
      contextId === null ? null : db.prepare('SELECT local_key FROM work_contexts WHERE id = ?').get(contextId)?.local_key ?? null);
    if (withoutPayload) db.prepare(`UPDATE migration_records SET payload_json = NULL,
      classification_state = CASE WHEN classification_state = 'secret' OR ? THEN 'secret' ELSE 'not_applicable' END,
      detail_code = 'source_redacted' WHERE id = ?`).run(secret ? 1 : 0, recordId);
    if (inserted.changes > 0 && record.kind === 'source' && effect === 'inserted' && memoryId !== null && !redacted
      && record.context_only === 0 && record.citation_kind !== null) {
      insertSource(db, memoryId, record);
    }
  }
}

/** Preview and apply share the same merge decisions; only apply opens an immediate write unit. */
export function mergeTransferPlan(db: DatabaseSync | undefined, plan: TransferPlan, options: ImportOptions): ImportResult {
  const result: ImportResult = { applied: false, inserted: 0, updated: 0, tombstones: 0, unchanged: 0, rejected: [] };
  const apply = options.dryRun !== true && db !== undefined;
  const pairs = (values: Record<string, string> | undefined) => Object.entries(values ?? {}).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  const mapping = JSON.stringify([pairs(options.mapRepo), pairs(options.mapWork), pairs(options.mapContext),
    pairs(options.mapProject), pairs(options.mapProjectHash)]);
  const mappingHash = sha256Hex(mapping);
  const importId = sha256Json(['migration-import-v1', plan.format, plan.revision, plan.sourceHash]);
  if (apply) db.exec('BEGIN IMMEDIATE');
  else db?.exec('BEGIN');
  try {
    const prior = db?.prepare('SELECT mapping_hash FROM migration_imports WHERE id = ?').get(importId);
    if (prior !== undefined && prior.mapping_hash !== mappingHash) throw new TransferInputError('import_mapping_changed');
    if (prior !== undefined) return { ...result, duplicate: true,
      unchanged: Number(plan.db.prepare("SELECT COUNT(*) AS n FROM transfer_rows WHERE kind = 'memory'").get()?.n ?? 0) };
    const resolved = mappings(plan, db, options);
    if (resolved.unresolved.length > 0) return { ...result, rejected: resolved.unresolved };
    plan.db.exec(`CREATE TABLE IF NOT EXISTS transfer_targets (content_hash TEXT PRIMARY KEY, id TEXT NOT NULL,
      repo_id TEXT NOT NULL, sensitivity TEXT NOT NULL, deleted_at INTEGER, owned INTEGER NOT NULL) STRICT;
      DELETE FROM transfer_targets`);
    if (apply) {
      for (const row of plan.db.prepare("SELECT origin, data FROM transfer_rows WHERE kind = 'repo' AND effect = 'create_repo'").iterate()) {
        const repo = JSON.parse(String(row.data)) as Extract<NativeRecord, { kind: 'repo' }>;
        db.prepare(`INSERT OR IGNORE INTO repos
        (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at) VALUES (?, 'remote', ?, ?, ?, ?)`)
          .run(repo.id, repo.normalized_identity, repo.normalized_identity, options.now, options.now);
      }
    }
    for (const row of plan.db.prepare("SELECT * FROM transfer_rows WHERE kind = 'memory' ORDER BY sequence").iterate()) {
      mergeMemory(db, plan, row, options, result);
    }
    if (apply) {
      db.prepare(`INSERT OR IGNORE INTO migration_imports (id, source_format, source_revision, source_sha256,
        exported_at, mapping_json, mapping_hash, counts_json, imported_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`)
        .run(importId, plan.format, plan.revision, plan.sourceHash, plan.exportedAt, mapping,
          mappingHash, JSON.stringify(result), options.now);
      saveOrigins(db, plan, importId, true);
      db.exec('COMMIT');
      result.applied = true;
    } else if (db !== undefined) saveOrigins(db, plan, importId, false);
    return result;
  } finally { if (db?.isTransaction) db.exec('ROLLBACK'); }
}
