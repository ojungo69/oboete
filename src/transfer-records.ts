// Native v2 records read from the store, one iterator per kind, with the redaction rules the
// export contract fixes (secret and deleted rows travel without text). Shared by `oboete export`
// and by sync publishing, which converts the local ids to origin ids before hashing.
import type { DatabaseSync } from 'node:sqlite';

const MEMORY_COLUMNS = `id, repo_id, type, title, body, concepts, material_hash, content_hash, sensitivity,
  review_state, degraded_reason, source_session_id, source_batch_id, valid_from, valid_to,
  superseded_by, pinned_at, pin_order, deleted_at, created_at, work_id, checkpoint_parent_id,
  provenance_complete, source_captured_at`;

export type Row = Record<string, unknown>;

export function* memoryRecords(db: DatabaseSync, only?: string): Generator<Row> {
  const personal = db.prepare(`SELECT 1 FROM memory_visibility WHERE memory_id = ? AND audience = 'personal'
    UNION SELECT 1 FROM migration_records WHERE destination_memory_id = ? AND identity_domain = 'personal_projection' LIMIT 1`);
  for (const row of db.prepare(`SELECT ${MEMORY_COLUMNS} FROM memories WHERE ? IS NULL OR id = ? ORDER BY created_at, id`).iterate(only ?? null, only ?? null)) {
    const withoutText = row.deleted_at !== null || row.sensitivity === 'secret';
    yield { kind: 'memory', ...row, identity_domain: personal.get(row.id, row.id) ? 'personal_projection' : 'ordinary',
      title: withoutText ? '' : row.title, body: withoutText ? '' : row.body,
      concepts: withoutText ? '[]' : row.concepts, source_agent: null };
  }
}

export function* sourceRecords(db: DatabaseSync, only?: number): Generator<Row> {
  for (const row of db.prepare(`SELECT s.*, m.deleted_at AS parent_deleted_at, m.sensitivity AS parent_sensitivity
    FROM memory_sources s JOIN memories m ON m.id = s.memory_id WHERE ? IS NULL OR s.id = ? ORDER BY s.id`).iterate(only ?? null, only ?? null)) {
    const { parent_deleted_at, parent_sensitivity, ...source } = row;
    const redacted = parent_deleted_at !== null || parent_sensitivity === 'secret';
    yield { kind: 'source', ...source, id: String(row.id), evidence: redacted ? null : row.evidence,
      citation_value: redacted ? null : row.citation_value, source_agent: redacted ? null : row.source_agent,
      capture_root: redacted ? null : row.capture_root, source_paths_json: redacted ? null : row.source_paths_json };
  }
}

export function* contextRecords(db: DatabaseSync, only?: string): Generator<Row> {
  for (const row of db.prepare('SELECT * FROM work_contexts WHERE ? IS NULL OR id = ? ORDER BY id').iterate(only ?? null, only ?? null)) yield { kind: 'context', ...row, redacted: false };
}

export function* workRecords(db: DatabaseSync, only?: string): Generator<Row> {
  for (const row of db.prepare('SELECT * FROM work_items WHERE ? IS NULL OR id = ? ORDER BY id').iterate(only ?? null, only ?? null)) {
    const redacted = row.purpose_sensitivity === 'secret';
    yield { kind: 'work', ...row, purpose: redacted ? null : row.purpose, redacted };
  }
}

export function* visibilityRecords(db: DatabaseSync, only?: string): Generator<Row> {
  for (const row of db.prepare('SELECT * FROM memory_visibility WHERE ? IS NULL OR id = ? ORDER BY id').iterate(only ?? null, only ?? null)) yield { kind: 'visibility', ...row };
}

export function* proposalRecords(db: DatabaseSync, only?: string): Generator<Row> {
  for (const row of db.prepare(`SELECT p.*, m.deleted_at AS origin_deleted_at, m.sensitivity AS origin_sensitivity,
    projected.deleted_at AS projection_deleted_at, projected.sensitivity AS projection_sensitivity
    FROM sharing_proposals p JOIN memories m ON m.id = p.origin_memory_id
    LEFT JOIN memories projected ON projected.id = p.projected_memory_id WHERE ? IS NULL OR p.id = ? ORDER BY p.id`).iterate(only ?? null, only ?? null)) {
    const { origin_deleted_at, origin_sensitivity, projection_deleted_at, projection_sensitivity, ...proposal } = row;
    const redacted = origin_deleted_at !== null || origin_sensitivity === 'secret' || row.candidate_sensitivity === 'secret'
      || projection_deleted_at !== null || projection_sensitivity === 'secret';
    yield { kind: 'sharing_proposal', ...proposal, redacted,
      candidate_title: redacted ? '' : row.candidate_title, candidate_body: redacted ? '' : row.candidate_body,
      source_event_ids_json: redacted ? '[]' : row.source_event_ids_json };
  }
}
