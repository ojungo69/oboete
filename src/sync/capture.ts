// Local change capture (contracts/sync.md "Local change capture"): one pass over the tracked
// tables that gives every row an origin, converts it to a revision payload in origin form and
// records a new revision authored by this replica wherever the row's canonical state differs
// from what was last materialized. Nothing outside sync is instrumented. Security-owned.
import type { DatabaseSync } from 'node:sqlite';

import { prepared } from '../db/statements.js';
import { sha256Hex } from '../hash.js';
import {
  contextRecords, memoryRecords, proposalRecords, sourceRecords, visibilityRecords, workRecords, type Row,
} from '../transfer-records.js';
import { ENTITY_REFERENCES, type Control } from './format.js';
import { canonicalJson, payloadHash, revisionId, type Sensitivity, type SyncKind } from './identity.js';
import {
  canonicalOf, createOrigin, effectiveControl, originsOfRow, readOrigin, registerLocalRepos, replicaOriginId,
  setSelectedHead, stateHash, storeRevision, type Origin, type Revision,
} from './store.js';

export type CaptureResult = { revisions: number; tombstones: number };

/**
 * The local identifier of a source row is `source:<sync_key>` (contracts/sync.md): the key is
 * stored on the row at its first capture and never recomputed, so in-place field changes,
 * redaction and a move under another memory are revisions of the same origin. The key is the
 * hash of the row's fields at that moment, named by device-independent identities (the owning
 * and parent memories' material, the context's local key), so an identical independent capture
 * on another device aliases onto the same origin. `memory_sources.id` is a reusable rowid and
 * is never used.
 */
export function sourceLocalId(db: DatabaseSync, rowid: string): string {
  const raw = prepared(db, `SELECT s.sync_key, s.citation_kind, s.citation_value, s.source_agent, s.portion_start, s.portion_end,
    s.source_total, s.source_hash, s.captured_at, s.capture_root, s.source_paths_json, s.context_only, s.raw_event_id,
    m.material_hash AS owner_material, p.material_hash AS parent_material, c.local_key AS context_key
    FROM memory_sources s JOIN memories m ON m.id = s.memory_id
    LEFT JOIN memories p ON p.id = s.source_memory_id LEFT JOIN work_contexts c ON c.id = s.source_context_id
    WHERE s.id = ?`).get(rowid);
  if (raw === undefined) throw new Error(`memory_sources ${rowid} vanished during capture`);
  if (raw.sync_key !== null) return `source:${String(raw.sync_key)}`;
  const fields = [
    raw.owner_material, raw.citation_kind, raw.citation_value, raw.source_agent, raw.portion_start, raw.portion_end, raw.source_total,
    raw.source_hash, raw.captured_at, raw.capture_root, raw.source_paths_json, raw.context_only, raw.raw_event_id,
    raw.parent_material ?? null, raw.context_key ?? null,
  ];
  // Two identical rows under one memory (nothing UNIQUE tells them apart) get distinct keys.
  let key = sha256Hex(canonicalJson(fields));
  while (prepared(db, 'SELECT 1 FROM memory_sources WHERE sync_key = ?').get(key) !== undefined) key = sha256Hex(canonicalJson([key, rowid]));
  prepared(db, 'UPDATE memory_sources SET sync_key = ? WHERE id = ?').run(key, rowid);
  return `source:${key}`;
}

export function controlOf(kind: SyncKind, record: Row): Control {
  const floor = (value: unknown): Sensitivity => value as Sensitivity;
  switch (kind) {
    case 'memory': return { tombstone: record.deleted_at !== null, sensitivity_floor: floor(record.sensitivity) };
    case 'work': return { tombstone: false, sensitivity_floor: floor(record.purpose_sensitivity) };
    case 'sharing_proposal': return { tombstone: false, sensitivity_floor: floor(record.candidate_sensitivity) };
    default: return { tombstone: false, sensitivity_floor: 'eligible' };
  }
}

type Resolver = {
  originOf: (kind: SyncKind, localId: string) => string;
  repoKey: (localRepoId: string) => string;
};

/** Converts a native record's entity references to origin form; everything else stays verbatim. */
export function toOriginForm(kind: SyncKind, record: Row, originId: string, resolve: Resolver): Row {
  const payload: Row = { ...record, id: originId };
  for (const { field, kind: target } of ENTITY_REFERENCES[kind]) {
    const value = record[field];
    if (value === null || value === undefined) continue;
    payload[field] = target === 'repo' ? resolve.repoKey(String(value)) : resolve.originOf(target, String(value));
  }
  return payload;
}

/** Payload fields that name what the natural key names: the payload follows the natural key. */
const NATURAL_REFERENCES: Record<SyncKind, readonly [payloadField: string, naturalField: string][]> = {
  memory: [['repo_id', 'repo'], ['work_id', 'work'], ['checkpoint_parent_id', 'parent']],
  source: [['memory_id', 'memory']],
  visibility: [['memory_id', 'memory'], ['repo_id', 'repo'], ['work_id', 'work']],
  sharing_proposal: [['origin_memory_id', 'origin_memory']],
  context: [['repo_id', 'repo']],
  work: [],
};

/**
 * A row's natural key is frozen when its origin is created, while the origins and repository
 * keys its references resolve to can change later (an alias, a mapping). Every revision names
 * its references the way its natural key does, so a reader can check one against the other.
 */
export function alignToNatural(kind: SyncKind, payload: Row, natural: Row): Row {
  const aligned = { ...payload };
  for (const [field, key] of NATURAL_REFERENCES[kind]) if (natural[key] !== undefined) aligned[field] = natural[key];
  return aligned;
}

export function naturalOf(kind: SyncKind, record: Row, originId: string, resolve: Resolver): Row {
  switch (kind) {
    case 'memory':
      if (record.identity_domain === 'personal_projection') return { domain: 'personal_projection', projection_hash: record.content_hash };
      if (record.work_id !== null) {
        return {
          domain: 'checkpoint', repo: resolve.repoKey(String(record.repo_id)), work: resolve.originOf('work', String(record.work_id)),
          parent: record.checkpoint_parent_id === null ? null : resolve.originOf('memory', String(record.checkpoint_parent_id)),
          material_hash: record.material_hash,
        };
      }
      return { domain: 'ordinary', repo: resolve.repoKey(String(record.repo_id)), material_hash: record.material_hash };
    case 'source': return { memory: resolve.originOf('memory', String(record.memory_id)), key: originId.slice(originId.lastIndexOf(':') + 1) };
    case 'visibility':
      return {
        memory: resolve.originOf('memory', String(record.memory_id)), audience: record.audience,
        repo: record.repo_id === null ? null : resolve.repoKey(String(record.repo_id)),
        work: record.work_id === null ? null : resolve.originOf('work', String(record.work_id)),
      };
    case 'sharing_proposal': return { candidate: record.candidate_material_hash, origin_memory: resolve.originOf('memory', String(record.origin_memory_id)) };
    case 'context': return { repo: resolve.repoKey(String(record.repo_id)), local_key: record.local_key };
    case 'work': return { work: originId };
  }
}

/**
 * Runs the change pass inside the caller's transaction and returns how many revisions it created.
 * A row whose canonical state differs from `materialized_hash` becomes a successor of the
 * selected head; an origin whose row no longer exists becomes a tombstone.
 */
export function captureLocalChanges(db: DatabaseSync, now: number): CaptureResult {
  const replica = replicaOriginId(db);
  const repoKeys = registerLocalRepos(db, replica);
  const originCache = new Map<string, string>();
  const resolve: Resolver = {
    originOf: (kind, localId) => {
      const cacheKey = `${kind} ${localId}`;
      let origin = originCache.get(cacheKey);
      if (origin === undefined) {
        origin = originsOfRow(db, kind, localId)[0]?.canonical_origin_id ?? `${replica}:${localId}`;
        originCache.set(cacheKey, origin);
      }
      return origin;
    },
    repoKey: (localRepoId) => {
      const key = repoKeys.get(localRepoId);
      if (key === undefined) throw new Error(`repository ${localRepoId} has no sync key`);
      return key;
    },
  };
  const result: CaptureResult = { revisions: 0, tombstones: 0 };
  const seen = new Set<string>();

  const record = (kind: SyncKind, row: Row, localId: string): void => {
    seen.add(`${kind} ${localId}`);
    const originId = resolve.originOf(kind, localId);
    const natural = naturalOf(kind, row, originId, resolve);
    let origin = readOrigin(db, originId);
    if (origin === undefined) origin = createOrigin(db, { origin_id: originId, kind, local_id: localId, natural });
    const canonical = canonicalOf(db, origin.origin_id);
    const payload = alignToNatural(kind, toOriginForm(kind, row, originId, resolve), canonical.natural);
    const control = controlOf(kind, row);
    const hash = payloadHash(payload);
    const state = stateHash(hash, control);
    if (canonical.materialized_hash === state) return;
    const parents = canonical.materialized_revision === null ? [] : [canonical.materialized_revision];
    const revision: Revision = {
      revision_id: '', origin_id: canonical.origin_id, kind, author: replica, parents, control,
      natural: canonical.natural, payload_hash: hash, payload,
    };
    revision.revision_id = revisionId(revision);
    if (storeRevision(db, revision, null, now)) result.revisions += 1;
    setSelectedHead(db, canonical.origin_id, revision.revision_id, revision.revision_id, state);
  };

  for (const row of contextRecords(db)) record('context', row, String(row.id));
  for (const row of workRecords(db)) record('work', row, String(row.id));
  for (const row of memoryRecords(db)) record('memory', row, String(row.id));
  for (const row of sourceRecords(db)) record('source', row, sourceLocalId(db, String(row.id)));
  for (const row of visibilityRecords(db)) record('visibility', row, String(row.id));
  for (const row of proposalRecords(db)) record('sharing_proposal', row, String(row.id));

  // Rows that vanished (sources and grants are physically deleted) become tombstones once.
  for (const stored of prepared(db, `SELECT * FROM sync_origins WHERE local_id IS NOT NULL AND origin_id = canonical_origin_id`).all()) {
    const origin: Origin = { ...stored, natural: JSON.parse(String(stored.natural_json)) as Row } as unknown as Origin;
    const key = `${String(stored.kind)} ${String(stored.local_id)}`;
    if (seen.has(key)) continue;
    const others = originsOfRow(db, stored.kind as SyncKind, String(stored.local_id)).map((sibling) => `${sibling.kind} ${sibling.local_id}`);
    if (others.some((sibling) => seen.has(sibling))) continue;
    const control = effectiveControl(db, origin.origin_id);
    if (control.tombstone) continue;
    const tombstone: Control = { tombstone: true, sensitivity_floor: control.sensitivity_floor };
    const state = stateHash(null, tombstone);
    if (origin.materialized_hash === state) continue;
    const revision: Revision = {
      revision_id: '', origin_id: origin.origin_id, kind: origin.kind, author: replica,
      parents: origin.materialized_revision === null ? [] : [origin.materialized_revision], control: tombstone,
      natural: origin.natural, payload_hash: null, payload: null,
    };
    revision.revision_id = revisionId(revision);
    if (storeRevision(db, revision, null, now)) result.tombstones += 1;
    setSelectedHead(db, origin.origin_id, revision.revision_id, revision.revision_id, state);
  }
  return result;
}
