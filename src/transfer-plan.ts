import { createHash } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';

import { checkpointHash, contentHash, materialHash } from './db/identity.js';
import { openDatabase, sqliteErrorInfo } from './db/open.js';
import { sha256Hex, sha256Json } from './hash.js';
import { EXPORT_FORMAT, MAX_FILE_BYTES, MAX_LINE_BYTES, MAX_NATIVE_LINE_BYTES, SENSITIVITY_RANK,
  NATIVE_FORMAT, NATIVE_REVISION, CLAUDE_MEM_FORMAT, CLAUDE_MEM_REVISION, headerSchema, lineSchema, nativeHeaderSchema,
  nativeRecordSchema, migrationPayloadRedaction, migrationPayloadShape, type NativeRecord } from './transfer-format.js';

import { adaptClaudeMemQueryExport, ClaudeMemAdapterError, MAX_CLAUDE_MEM_BYTES,
  type ClaudeMemAdapterResult, type ClaudeMemNormalizedRecord } from './transfer-claude-mem.js';

export class TransferInputError extends Error {
  constructor(readonly reason: string, readonly line = 0) { super(reason); }
}

/** A structural work bound before JSON.parse; quoted punctuation is ordinary text. */
export function boundedJson(text: string, line: number): unknown {
  let quoted = false;
  let escaped = false;
  let members = 0;
  for (const char of text) {
    if (quoted) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') quoted = false;
    } else if (char === '"') quoted = true;
    else if ((char === '[' || char === '{' || char === ',' || char === ':') && ++members > 100_000) {
      throw new TransferInputError('source_structure_too_large', line);
    }
  }
  try { return JSON.parse(text); } catch { throw new TransferInputError('invalid_json', line); }
}

export type TransferPlan = {
  db: DatabaseSync;
  format: typeof EXPORT_FORMAT | typeof NATIVE_FORMAT | typeof CLAUDE_MEM_FORMAT;
  external?: ClaudeMemAdapterResult['header'];
  revision: string;
  originId: string | null;
  sourceHash: string;
  bytes: number;
  exportedAt: number | null;
  close(): void;
};

function validateMemory(record: Extract<NativeRecord, { kind: 'memory' }>, line: number): void {
  const absentText = record.deleted_at !== null || record.sensitivity === 'secret';
  if (absentText) {
    if ((record.title ?? '') !== '' || (record.body ?? '') !== '' || (record.concepts ?? '[]') !== '[]') {
      throw new TransferInputError('redacted_memory_text', line);
    }
  } else if (materialHash(record.title ?? '', record.body ?? '') !== record.material_hash) {
    throw new TransferInputError('material_hash_mismatch', line);
  }
  if (record.identity_domain === 'personal_projection') {
    if (record.work_id !== null || record.checkpoint_parent_id !== null || record.source_session_id !== null
      || record.source_batch_id !== null) throw new TransferInputError('personal_source_lineage', line);
    if (!absentText && sha256Json(['personal-projection-v1', record.title ?? '', record.body ?? '']) !== record.content_hash) {
      throw new TransferInputError('personal_identity_mismatch', line);
    }
  }
}

function referencesValid(db: DatabaseSync): void {
  const invalid = (sql: string, reason: string): void => {
    const row = db.prepare(sql).get();
    if (row !== undefined) throw new TransferInputError(reason, Number(row.sequence));
  };
  invalid(`SELECT m.source_line AS sequence FROM transfer_rows m LEFT JOIN transfer_rows r
    ON r.kind = 'repo' AND r.origin = m.repo_id WHERE m.kind = 'memory' AND r.origin IS NULL LIMIT 1`, 'unknown_source_repo');
  invalid(`SELECT s.source_line AS sequence FROM transfer_rows s LEFT JOIN transfer_rows m
    ON m.kind = 'memory' AND m.origin = s.memory_id WHERE s.kind IN ('source', 'visibility', 'sharing_proposal')
    AND m.origin IS NULL LIMIT 1`, 'unknown_source_memory');
  invalid(`SELECT s.source_line AS sequence FROM transfer_rows s JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = s.memory_id
    WHERE s.kind = 'source' AND (json_extract(m.data, '$.identity_domain') = 'personal_projection'
      OR ((json_extract(m.data, '$.deleted_at') IS NOT NULL OR json_extract(m.data, '$.sensitivity') = 'secret')
        AND (json_extract(s.data, '$.evidence') IS NOT NULL OR json_extract(s.data, '$.citation_value') IS NOT NULL
          OR json_extract(s.data, '$.capture_root') IS NOT NULL OR json_extract(s.data, '$.source_paths_json') IS NOT NULL
          OR json_extract(s.data, '$.source_agent') IS NOT NULL))) LIMIT 1`,
  'source_on_redacted_or_personal_memory');
  invalid(`SELECT s.source_line AS sequence FROM transfer_rows s LEFT JOIN transfer_rows m ON m.kind = 'memory'
    AND m.origin = json_extract(s.data, '$.source_memory_id') WHERE s.kind = 'source'
    AND json_extract(s.data, '$.source_memory_id') IS NOT NULL AND m.origin IS NULL LIMIT 1`, 'unknown_source_dependency');
  // A local dependency row is the edge alone (observer/provenance.ts retainGenerationPrivacy), so anything
  // else riding on it has no local shape and would escape the parent's redaction.
  const dependencyOnly = ['raw_event_id', 'source_context_id', 'citation_kind', 'citation_value', 'source_agent', 'portion_start',
    'portion_end', 'source_total', 'source_hash', 'evidence', 'capture_root', 'source_paths_json']
    .map((field) => `json_extract(s.data, '$.${field}') IS NOT NULL`).join(' OR ');
  invalid(`SELECT s.source_line AS sequence FROM transfer_rows s WHERE s.kind = 'source'
    AND json_extract(s.data, '$.source_memory_id') IS NOT NULL
    AND (json_extract(s.data, '$.context_only') <> 1 OR ${dependencyOnly}) LIMIT 1`, 'dependency_source_has_text');
  // The memories_provenance_privacy trigger keeps every descendant at least as strict as its ancestor; a
  // file that claims otherwise describes a state the database can never hold, so it is refused, not repaired.
  invalid(`SELECT m.source_line AS sequence FROM transfer_rows m JOIN (${LINEAGE_EDGES}) e ON e.child = m.origin
    JOIN transfer_rows p ON p.kind = 'memory' AND p.origin = e.parent
    WHERE m.kind = 'memory' AND ${rankSql("json_extract(m.data, '$.sensitivity')")} < ${rankSql("json_extract(p.data, '$.sensitivity')")}
    LIMIT 1`, 'dependency_sensitivity_below_parent');
  invalid(`SELECT s.source_line AS sequence FROM transfer_rows s LEFT JOIN transfer_rows m ON m.kind = 'memory'
    AND m.origin = json_extract(s.data, '$.superseded_by') WHERE s.kind = 'memory'
    AND json_extract(s.data, '$.superseded_by') IS NOT NULL AND m.origin IS NULL LIMIT 1`, 'unknown_supersession');
  invalid(`SELECT p.source_line AS sequence FROM transfer_rows p LEFT JOIN transfer_rows m ON m.kind = 'memory'
    AND m.origin = json_extract(p.data, '$.projected_memory_id') WHERE p.kind = 'sharing_proposal'
    AND json_extract(p.data, '$.state') = 'approved' AND (m.origin IS NULL
      OR json_extract(m.data, '$.identity_domain') <> 'personal_projection'
      OR (json_extract(p.data, '$.redacted') = 0 AND (json_extract(m.data, '$.title') IS NOT json_extract(p.data, '$.candidate_title')
        OR json_extract(m.data, '$.body') IS NOT json_extract(p.data, '$.candidate_body')
        OR json_extract(m.data, '$.material_hash') IS NOT json_extract(p.data, '$.candidate_material_hash')))) LIMIT 1`,
  'proposal_projection_mismatch');
  invalid(`SELECT p.source_line AS sequence FROM transfer_rows p
    JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = p.memory_id
    LEFT JOIN transfer_rows projection ON projection.kind = 'memory'
      AND projection.origin = json_extract(p.data, '$.projected_memory_id')
    WHERE p.kind = 'sharing_proposal' AND json_extract(p.data, '$.redacted') = 0
      AND (json_extract(m.data, '$.deleted_at') IS NOT NULL OR json_extract(m.data, '$.sensitivity') = 'secret'
        OR json_extract(projection.data, '$.deleted_at') IS NOT NULL
        OR json_extract(projection.data, '$.sensitivity') = 'secret') LIMIT 1`, 'proposal_parent_redaction');
  invalid(`SELECT v.source_line AS sequence FROM transfer_rows v LEFT JOIN transfer_rows p ON p.kind = 'sharing_proposal'
    AND p.origin = json_extract(v.data, '$.proposal_id') WHERE v.kind = 'visibility'
    AND json_extract(v.data, '$.audience') = 'personal' AND (p.origin IS NULL
      OR json_extract(p.data, '$.state') <> 'approved'
      OR json_extract(p.data, '$.projected_memory_id') <> v.memory_id) LIMIT 1`, 'personal_approval_mismatch');
  invalid(`SELECT v.source_line AS sequence FROM transfer_rows v JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = v.memory_id
    WHERE v.kind = 'visibility' AND json_extract(v.data, '$.audience') <> 'personal'
      AND json_extract(v.data, '$.repo_id') <> m.repo_id LIMIT 1`, 'visibility_repo_mismatch');
  invalid(`SELECT p.source_line AS sequence FROM transfer_rows p JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = p.memory_id
    WHERE p.kind = 'sharing_proposal' AND json_extract(p.data, '$.origin_repo_id') <> m.repo_id LIMIT 1`, 'proposal_repo_mismatch');
  invalid(`SELECT h.source_line AS sequence FROM transfer_rows h LEFT JOIN transfer_rows r ON r.kind = 'repo' AND r.origin = h.repo_id
    WHERE h.kind IN ('work', 'context') AND r.origin IS NULL LIMIT 1`, 'historical_repo_mismatch');
  invalid(`SELECT w.source_line AS sequence FROM transfer_rows w LEFT JOIN transfer_rows c ON c.kind = 'context'
    AND c.origin = json_extract(w.data, '$.origin_context_id') WHERE w.kind = 'work'
    AND (c.origin IS NULL OR c.repo_id <> w.repo_id) LIMIT 1`, 'work_context_mismatch');
  invalid(`SELECT m.source_line AS sequence FROM transfer_rows m LEFT JOIN transfer_rows w ON w.kind = 'work'
    AND w.origin = json_extract(m.data, '$.work_id') WHERE m.kind = 'memory'
    AND json_extract(m.data, '$.work_id') IS NOT NULL AND (w.origin IS NULL OR w.repo_id <> m.repo_id) LIMIT 1`, 'memory_work_mismatch');
  invalid(`SELECT v.source_line AS sequence FROM transfer_rows v LEFT JOIN transfer_rows w ON w.kind = 'work'
    AND w.origin = json_extract(v.data, '$.work_id') WHERE v.kind = 'visibility'
    AND json_extract(v.data, '$.audience') = 'work' AND (w.origin IS NULL OR w.repo_id <> v.repo_id) LIMIT 1`, 'grant_work_mismatch');
  invalid(`SELECT p.source_line AS sequence FROM transfer_rows p LEFT JOIN transfer_rows w ON w.kind = 'work'
    AND w.origin = json_extract(p.data, '$.origin_work_id') WHERE p.kind = 'sharing_proposal'
    AND (w.origin IS NULL OR w.repo_id <> p.repo_id OR json_extract(p.data, '$.projected_memory_id') = p.memory_id) LIMIT 1`,
  'proposal_work_mismatch');
  invalid(`SELECT s.source_line AS sequence FROM transfer_rows s JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = s.memory_id
    LEFT JOIN transfer_rows c ON c.kind = 'context' AND c.origin = json_extract(s.data, '$.source_context_id')
    WHERE s.kind = 'source' AND json_extract(s.data, '$.source_context_id') IS NOT NULL
      AND (c.origin IS NULL OR c.repo_id <> m.repo_id) LIMIT 1`, 'source_context_mismatch');
  invalid(`SELECT s.source_line AS sequence FROM transfer_rows s JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = s.memory_id
    JOIN transfer_rows parent ON parent.kind = 'memory' AND parent.origin = json_extract(s.data, '$.source_memory_id')
    WHERE s.kind = 'source' AND parent.repo_id <> m.repo_id LIMIT 1`, 'source_dependency_repo_mismatch');
  invalid(`SELECT m.source_line AS sequence FROM transfer_rows m LEFT JOIN transfer_rows parent ON parent.kind = 'memory'
    AND parent.origin = json_extract(m.data, '$.checkpoint_parent_id') WHERE m.kind = 'memory'
    AND json_extract(m.data, '$.checkpoint_parent_id') IS NOT NULL AND (parent.origin IS NULL
      OR parent.repo_id <> m.repo_id OR json_extract(parent.data, '$.work_id') IS NOT json_extract(m.data, '$.work_id')) LIMIT 1`,
  'checkpoint_parent_mismatch');
  invalid(`SELECT w.source_line AS sequence FROM transfer_rows w LEFT JOIN transfer_rows m ON m.kind = 'memory'
    AND m.origin = json_extract(w.data, '$.current_checkpoint_memory_id') WHERE w.kind = 'work'
    AND json_extract(w.data, '$.current_checkpoint_memory_id') IS NOT NULL AND (m.origin IS NULL
      OR m.repo_id <> w.repo_id OR json_extract(m.data, '$.work_id') IS NOT w.origin) LIMIT 1`, 'historical_checkpoint_mismatch');
  invalid(`SELECT o.source_line AS sequence FROM transfer_rows o LEFT JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = o.memory_id
    WHERE o.kind = 'migration_origin' AND o.memory_id IS NOT NULL AND (m.origin IS NULL OR m.repo_id IS NOT o.repo_id) LIMIT 1`,
  'migration_origin_target_mismatch');
  invalid(`SELECT o.source_line AS sequence FROM transfer_rows o LEFT JOIN transfer_rows r ON r.kind = 'repo' AND r.origin = o.repo_id
    WHERE o.kind = 'migration_origin' AND o.repo_id IS NOT NULL AND r.origin IS NULL LIMIT 1`, 'migration_origin_repo_mismatch');
  // A retainable payload with no memory in this file would be stored with no identity to redact it by
  // later; a terminal-labelled payload is never retained, so it may travel as a hash-only trace.
  // The terminal label is the one migrationPayloadRedaction reads for that kind, nothing else.
  invalid(`SELECT o.source_line AS sequence FROM transfer_rows o WHERE o.kind = 'migration_origin' AND o.memory_id IS NULL
    AND json_type(o.data, '$.payload') <> 'null' AND CASE json_extract(o.data, '$.record_kind')
      WHEN 'memory' THEN json_extract(o.data, '$.payload.sensitivity') IS NOT 'secret' AND json_extract(o.data, '$.payload.deleted_at') IS NULL
      WHEN 'sharing_proposal' THEN json_extract(o.data, '$.payload.candidate_sensitivity') IS NOT 'secret'
        AND json_extract(o.data, '$.payload.redacted') IS NOT 1
      WHEN 'source' THEN 1 WHEN 'visibility' THEN 1 ELSE 0 END LIMIT 1`, 'orphan_origin_payload');
  invalid(`SELECT o.source_line AS sequence FROM transfer_rows o JOIN transfer_rows m ON m.kind = 'memory' AND m.origin = o.memory_id
    WHERE o.kind = 'migration_origin' AND json_type(o.data, '$.payload') <> 'null'
      AND (json_extract(m.data, '$.deleted_at') IS NOT NULL OR json_extract(m.data, '$.sensitivity') = 'secret') LIMIT 1`,
  'migration_origin_parent_redaction');
  acyclic(db);
}

/** child -> parent edges of the dependency graph: source dependencies and checkpoint parents. */
const LINEAGE_EDGES = `SELECT memory_id AS child, json_extract(data, '$.source_memory_id') AS parent FROM transfer_rows
    WHERE kind = 'source' AND json_extract(data, '$.source_memory_id') IS NOT NULL
  UNION SELECT origin, json_extract(data, '$.checkpoint_parent_id') FROM transfer_rows
    WHERE kind = 'memory' AND json_extract(data, '$.checkpoint_parent_id') IS NOT NULL`;

export const rankSql = (expression: string): string =>
  `CASE ${expression} ${Object.entries(SENSITIVITY_RANK).map(([name, rank]) => `WHEN '${name}' THEN ${rank}`).join(' ')} END`;

/**
 * Kahn's algorithm on scratch tables: bounded pages, no recursive ancestor expansion or JS graph.
 * The dependency edges stay in `transfer_lineage` for the merge's sensitivity propagation.
 */
function acyclic(db: DatabaseSync): void {
  db.exec(`CREATE TABLE transfer_edges (from_id TEXT NOT NULL, to_id TEXT NOT NULL, PRIMARY KEY(from_id, to_id)) WITHOUT ROWID;
    CREATE INDEX transfer_edges_target ON transfer_edges(to_id);
    CREATE TABLE transfer_degree (id TEXT PRIMARY KEY, incoming INTEGER NOT NULL) STRICT;
    CREATE INDEX transfer_degree_ready ON transfer_degree(incoming, id);
    CREATE TABLE transfer_lineage (child TEXT NOT NULL, parent TEXT NOT NULL, PRIMARY KEY(child, parent)) WITHOUT ROWID;
    INSERT OR IGNORE INTO transfer_lineage ${LINEAGE_EDGES};`);
  for (const relation of ['dependencies', 'superseded_by'] as const) {
    db.exec('DELETE FROM transfer_edges; DELETE FROM transfer_degree');
    if (relation === 'dependencies') db.exec(`INSERT OR IGNORE INTO transfer_edges ${LINEAGE_EDGES}`);
    else db.exec(`INSERT OR IGNORE INTO transfer_edges SELECT origin, json_extract(data, '$.superseded_by') FROM transfer_rows
      WHERE kind = 'memory' AND json_extract(data, '$.superseded_by') IS NOT NULL`);
    db.exec(`
      INSERT OR IGNORE INTO transfer_degree SELECT from_id, 0 FROM transfer_edges;
      INSERT OR IGNORE INTO transfer_degree SELECT to_id, 0 FROM transfer_edges;
      UPDATE transfer_degree SET incoming = (SELECT COUNT(*) FROM transfer_edges WHERE to_id = transfer_degree.id);`);
    const decrement = db.prepare('UPDATE transfer_degree SET incoming = incoming - 1 WHERE id = ?');
    for (;;) {
      const ready = db.prepare('SELECT id FROM transfer_degree WHERE incoming = 0 ORDER BY id LIMIT 1000').all();
      if (ready.length === 0) break;
      for (const node of ready) {
        for (const edge of db.prepare('SELECT to_id FROM transfer_edges WHERE from_id = ?').iterate(node.id)) decrement.run(edge.to_id);
        db.prepare('DELETE FROM transfer_edges WHERE from_id = ?').run(node.id);
        db.prepare('DELETE FROM transfer_degree WHERE id = ?').run(node.id);
      }
    }
    if (db.prepare('SELECT 1 FROM transfer_degree LIMIT 1').get() !== undefined) throw new TransferInputError('source_lineage_cycle');
  }
  db.exec('DROP TABLE transfer_edges; DROP TABLE transfer_degree');
}

/** A disk-backed, fully validated input. Reading never opens the destination or reopens the source. */
export async function readTransferPlan(input: AsyncIterable<Uint8Array | string>, from?: 'claude-mem'): Promise<TransferPlan> {
  const directory = mkdtempSync(join(tmpdir(), 'oboete-transfer-'));
  let db: DatabaseSync | undefined;
  try {
    db = openDatabase({ path: join(directory, 'plan.db'), timeoutMs: 2_000 }).db;
    db.exec(`PRAGMA journal_mode = DELETE; PRAGMA synchronous = OFF; PRAGMA cache_size = -2048; PRAGMA temp_store = FILE;
      PRAGMA max_page_count = 131072;
      CREATE TABLE transfer_rows (sequence INTEGER PRIMARY KEY, source_line INTEGER NOT NULL, kind TEXT NOT NULL, origin TEXT NOT NULL,
        repo_id TEXT, memory_id TEXT, data TEXT NOT NULL, destination_repo_id TEXT, destination_memory_id TEXT,
        destination_context_id TEXT, effect TEXT, content_hash TEXT, UNIQUE(kind, origin)) STRICT;
      CREATE INDEX transfer_memory ON transfer_rows(kind, memory_id);
      BEGIN IMMEDIATE`);
    const insert = db.prepare('INSERT INTO transfer_rows(sequence, source_line, kind, origin, repo_id, memory_id, data) VALUES (?, ?, ?, ?, ?, ?, ?)');
    const digest = createHash('sha256');
    const decoder = new TextDecoder('utf-8', { fatal: true });
    let bytes = 0;
    let number = 0;
    let sequence = 0;
    let pending = '';
    let format: TransferPlan['format'] | null = null;
    let external: TransferPlan['external'];
    let exportedAt: number | null = null;
    let originId: string | null = null;
    const store = (record: NativeRecord | ClaudeMemNormalizedRecord): void => {
      if (sequence === 1_000_000) throw new TransferInputError('too_many_source_records', number);
      const repo = record.kind === 'repo' ? record.id : record.kind === 'memory' || record.kind === 'visibility'
        || record.kind === 'work' || record.kind === 'context' || record.kind === 'migration_origin'
        || record.kind === 'session' || record.kind === 'prompt' || record.kind === 'excluded'
        ? record.repo_id : record.kind === 'sharing_proposal' ? record.origin_repo_id : null;
      const memory = record.kind === 'source' || record.kind === 'visibility' || record.kind === 'migration_origin' ? record.memory_id
        : record.kind === 'sharing_proposal' ? record.origin_memory_id : null;
      try { insert.run(++sequence, number, record.kind, record.id, repo, memory, JSON.stringify(record)); }
      catch (error) {
        // node:sqlite reports extended codes (2067 for a UNIQUE violation); the primary code is the low byte.
        const code = (sqliteErrorInfo(error).errcode ?? 0) & 0xff;
        throw new TransferInputError(code === 13 ? 'scratch_storage_full'
          : code === 19 ? 'duplicate_source_origin' : 'scratch_storage_failed', number);
      }
    };
    const accept = (physical: string): void => {
      number += 1;
      if (number > 1_000_000) throw new TransferInputError('too_many_source_lines', number);
      if (Buffer.byteLength(physical) > (format === EXPORT_FORMAT ? MAX_LINE_BYTES : MAX_NATIVE_LINE_BYTES)) {
        throw new TransferInputError('source_line_too_large', number);
      }
      if (physical.trim() === '') return;
      const value = boundedJson(physical, number);
      if (format === null) {
        const legacy = headerSchema.safeParse(value);
        if (legacy.success) {
          if (Buffer.byteLength(physical) > MAX_LINE_BYTES) throw new TransferInputError('source_line_too_large', number);
          format = EXPORT_FORMAT;
          exportedAt = legacy.data.exported_at ?? null;
          for (const repo of legacy.data.repos) store({ kind: 'repo', id: repo.id,
            identity_kind: repo.identity_kind, normalized_identity: repo.normalized_identity });
          return;
        }
        const native = nativeHeaderSchema.safeParse(value);
        if (!native.success) throw new TransferInputError('unsupported_source_format', number);
        format = NATIVE_FORMAT;
        exportedAt = native.data.exported_at;
        originId = native.data.origin_id;
        return;
      }
      if (format === NATIVE_FORMAT) {
        const record = nativeRecordSchema.safeParse(value);
        if (!record.success) throw new TransferInputError('invalid_native_record', number);
        if (record.data.kind === 'memory') {
          validateMemory(record.data, number);
          const memory = record.data;
          const expected = memory.work_id === null ? contentHash(memory.repo_id, memory.material_hash)
            : checkpointHash(memory.repo_id, memory.work_id, memory.checkpoint_parent_id, memory.material_hash);
          if (memory.identity_domain === 'ordinary' && memory.content_hash !== expected) {
            throw new TransferInputError('content_hash_mismatch', number);
          }
        }
        if (record.data.kind === 'sharing_proposal' && !record.data.redacted
          && materialHash(record.data.candidate_title, record.data.candidate_body) !== record.data.candidate_material_hash) {
          throw new TransferInputError('candidate_hash_mismatch', number);
        }
        if (record.data.kind === 'migration_origin') {
          const origin = boundedJson(record.data.origin_json, number);
          const supported = Array.isArray(origin) && (
            (origin[1] === EXPORT_FORMAT && origin[2] === '1' && origin[3] === null)
            || (origin[1] === NATIVE_FORMAT && origin[2] === NATIVE_REVISION && typeof origin[3] === 'string' && /^[0-9a-f]{32}$/u.test(origin[3]))
            || (origin[1] === CLAUDE_MEM_FORMAT && origin[2] === CLAUDE_MEM_REVISION && origin[3] === null));
          if (!Array.isArray(origin) || origin.length !== 7 || origin[0] !== 'migration-origin-v1'
            || !supported || typeof origin[5] !== 'string' || !/^[0-9a-f]{64}$/u.test(origin[5])
            || origin[4] !== record.data.record_kind || origin[6] !== record.data.payload_hash
            || sha256Json(origin) !== record.data.id
            || (record.data.payload !== null && (record.data.payload.kind !== record.data.record_kind
              || sha256Hex(JSON.stringify(record.data.payload)) !== record.data.stored_payload_hash))) {
            throw new TransferInputError('invalid_migration_origin', number);
          }
          if (record.data.payload !== null && migrationPayloadRedaction(record.data.payload) === null
            && !migrationPayloadShape(record.data.payload)) throw new TransferInputError('invalid_origin_payload', number);
        }
        store(record.data);
      } else {
        const parsed = lineSchema.safeParse(value);
        if (!parsed.success) throw new TransferInputError('invalid_v1_record', number);
        const { sources, ...memory } = parsed.data;
        const record = { ...memory, kind: 'memory' as const, identity_domain: 'ordinary' as const,
          work_id: null, checkpoint_parent_id: null, provenance_complete: null, source_captured_at: null };
        validateMemory(record, number);
        store(record);
        for (const [index, source] of sources.entries()) store({ kind: 'source', id: `v1:${memory.id}:${index}`,
          memory_id: memory.id, raw_event_id: null, source_memory_id: null, source_context_id: null,
          citation_kind: source.citation_kind, citation_value: source.citation_value, source_agent: source.source_agent,
          portion_start: null, portion_end: null, source_total: null, source_hash: null,
          evidence: null, captured_at: null, source_processed_at: null, capture_root: null,
          source_paths_json: null, context_only: 0 });
      }
    };
    try {
      for await (const chunk of input) {
        const data = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
        bytes += data.byteLength;
        if (bytes > (from === 'claude-mem' ? MAX_CLAUDE_MEM_BYTES : MAX_FILE_BYTES)) throw new TransferInputError('source_file_too_large', number + 1);
        digest.update(data);
        pending += decoder.decode(data, { stream: true });
        if (from === 'claude-mem') continue;
        for (let end = pending.indexOf('\n'); end !== -1; end = pending.indexOf('\n')) {
          accept(pending.slice(0, end).replace(/\r$/u, ''));
          pending = pending.slice(end + 1);
        }
        if (Buffer.byteLength(pending) > (format === EXPORT_FORMAT ? MAX_LINE_BYTES : MAX_NATIVE_LINE_BYTES)) {
          throw new TransferInputError('source_line_too_large', number + 1);
        }
      }
      pending += decoder.decode();
    } catch (error) {
      if (error instanceof TransferInputError) throw error;
      throw new TransferInputError(error instanceof TypeError ? 'invalid_utf8' : 'source_read_failed', number + 1);
    }
    if (from === 'claude-mem') {
      try {
        const adapted = adaptClaudeMemQueryExport(boundedJson(pending, 0), bytes);
        external = adapted.header;
        format = CLAUDE_MEM_FORMAT;
        exportedAt = external.exported_at;
        for (const project of external.projects) store({ kind: 'repo', id: project.id,
          identity_kind: 'common_dir', normalized_identity: project.source });
        for (const record of adapted.records) store(record);
      } catch (error) {
        if (error instanceof ClaudeMemAdapterError) throw new TransferInputError(error.code, error.index + 1);
        throw error;
      }
    } else if (pending !== '') accept(pending.replace(/\r$/u, ''));
    if (format === null) throw new TransferInputError('source_empty');
    referencesValid(db);
    db.exec('COMMIT');
    const planned = db;
    const sourceFormat = format as TransferPlan['format'];
    return { db: planned, format: sourceFormat, revision: sourceFormat === NATIVE_FORMAT ? NATIVE_REVISION
      : sourceFormat === CLAUDE_MEM_FORMAT ? CLAUDE_MEM_REVISION : '1', external,
      originId, bytes, exportedAt, sourceHash: digest.digest('hex'), close: () => {
        try { planned.close(); } finally { rmSync(directory, { recursive: true, force: true }); }
      } };
  } catch (error) {
    try { db?.close(); } finally { rmSync(directory, { recursive: true, force: true }); }
    throw error;
  }
}
