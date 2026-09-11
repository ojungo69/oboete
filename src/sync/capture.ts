// Local change capture (contracts/sync.md "Local change capture"): one pass over the tracked
// tables that gives every row an origin, converts it to a revision payload in origin form and
// records a new revision authored by this replica wherever the row's canonical state differs
// from what was last materialized. Nothing outside sync is instrumented. Security-owned.
import type { DatabaseSync } from 'node:sqlite';

import { prepared } from '../db/statements.js';
import { sha256Json } from '../hash.js';
import {
  contextRecords, memoryRecords, proposalRecords, sourceRecords, visibilityRecords, workRecords, type Row,
} from '../transfer-records.js';
import { BOUNDS, ENTITY_REFERENCES, type Control } from './format.js';
import { canonicalJson, payloadHash, revisionId, type Sensitivity, type SyncKind } from './identity.js';
import {
  bindOrigin, canonicalOf, createOrigin, effectiveControl, headsOf, originsOfRow, readOrigin, readRevision, registerLocalRepos,
  replicaOriginId, setSelectedHead, sourceTupleOf, stateHash, storeRevision, unionOrigins, type Origin, type Revision,
} from './store.js';

export type CaptureResult = { revisions: number; tombstones: number };

/** The wire fields of a source row, in the order the writer binds them. */
export const SOURCE_FIELDS = ['raw_event_id', 'citation_kind', 'citation_value', 'source_agent', 'portion_start', 'portion_end', 'source_total',
  'source_hash', 'evidence', 'captured_at', 'source_processed_at', 'capture_root', 'source_paths_json', 'source_context_id', 'context_only',
  'source_memory_id'] as const;

/**
 * The local identifier of a source row is `source:<sync_key>` (contracts/sync.md): a key stored
 * on the row at its first capture and never recomputed, so in-place field changes and redaction
 * are revisions of the same origin. The key is the hash of the row's wire fields (references in
 * origin form) and its memory's material, so an identical row takes the same key: one this device
 * deletes and inserts again (the observer rewrites flat provenance rows on every batch) finds its
 * origin, revived if a capture saw it gone, and records nothing when it is back as it was; one
 * another device captured independently aliases by natural key. A key a live row holds (an
 * identical duplicate) moves to the next ordinal. A row moved under another memory is a new
 * source (new key, new origin): the origin's natural key names the memory the source belongs to,
 * and the old origin records a tombstone in the closing pass. `memory_sources.id` is a reusable
 * rowid and is never used.
 */
export function sourceLocalId(db: DatabaseSync, rowid: string, resolve: Resolver): string {
  const raw = prepared(db, 'SELECT sync_key, memory_id FROM memory_sources WHERE id = ?').get(rowid);
  if (raw === undefined) throw new Error(`memory_sources ${rowid} vanished during capture`);
  if (raw.sync_key !== null) {
    const origin = readOrigin(db, resolve.originOf('source', `source:${String(raw.sync_key)}`));
    const memory = origin === undefined ? undefined : readOrigin(db, String(origin.natural.memory));
    // The row moved only when its origin's memory is a row this device holds and it is another row.
    const owner = memory === undefined ? null : canonicalOf(db, memory.origin_id).local_id;
    if (owner === null || owner === String(raw.memory_id)) return `source:${String(raw.sync_key)}`;
  }
  const record = toOriginForm('source', [...sourceRecords(db, Number(rowid))][0]!, '', resolve);
  const memory = prepared(db, 'SELECT material_hash, content_hash FROM memories WHERE id = ?').get(String(raw.memory_id));
  const content = [memory?.material_hash ?? memory?.content_hash ?? null, SOURCE_FIELDS.map((field) => record[field] ?? null)];
  for (let ordinal = 0; ; ordinal += 1) {
    const key = sha256Json([content, ordinal]);
    if (prepared(db, 'SELECT 1 FROM memory_sources WHERE sync_key = ?').get(key) !== undefined) continue;
    // A key whose origin names another memory (the same material elsewhere), or whose own origin
    // moved onto another row, is not this row's: the origin the key would find is not its own.
    const held = readOrigin(db, resolve.originOf('source', `source:${key}`));
    if (held !== undefined) {
      const memory = readOrigin(db, String(held.natural.memory));
      const owner = memory === undefined ? null : canonicalOf(db, memory.origin_id).local_id;
      if ((owner !== null && owner !== String(raw.memory_id)) || (held.local_id !== null && held.local_id !== `source:${key}`)) continue;
    }
    prepared(db, 'UPDATE memory_sources SET sync_key = ? WHERE id = ?').run(key, rowid);
    return `source:${key}`;
  }
}

/** The UNIQUE tuples of memory_sources a tuple record falls under (NULL members never match, as in the index). */
export function sourceTuples(tuple: Row): string[] {
  const tuples: string[] = [];
  if (tuple.source_hash !== null && tuple.raw_event_id !== null && tuple.portion_start !== null && tuple.portion_end !== null) {
    tuples.push(canonicalJson(['portion', tuple.raw_event_id, tuple.source_hash, tuple.portion_start, tuple.portion_end]));
  }
  if (tuple.context_only === 1 && tuple.raw_event_id !== null) tuples.push(canonicalJson(['raw', tuple.raw_event_id]));
  if (tuple.context_only === 1 && tuple.source_memory_id !== null) tuples.push(canonicalJson(['memory', tuple.source_memory_id]));
  return tuples;
}

/**
 * Records the tombstone of a bound origin whose row is gone (once: an origin already deleted, or
 * whose base is the deletion, records nothing) and returns the revision that carries it.
 */
function recordTombstone(db: DatabaseSync, origin: Origin, replica: string, now: number, result: CaptureResult): string | null {
  const control = effectiveControl(db, origin.origin_id);
  if (control.tombstone) return headsOf(db, origin.origin_id).find((head) => readRevision(db, head)!.control.tombstone) ?? null;
  const tombstone: Control = { tombstone: true, sensitivity_floor: control.sensitivity_floor };
  const state = stateHash(null, tombstone);
  if (origin.materialized_hash === state) return origin.materialized_revision;
  const revision: Revision = {
    revision_id: '', origin_id: origin.origin_id, kind: origin.kind, author: replica,
    parents: origin.materialized_revision === null ? [] : [origin.materialized_revision], control: tombstone,
    natural: origin.natural, payload_hash: null, payload: null,
  };
  revision.revision_id = revisionId(revision);
  if (storeRevision(db, revision, null, now)) result.tombstones += 1;
  setSelectedHead(db, origin.origin_id, revision.revision_id, revision.revision_id, state);
  return revision.revision_id;
}

/**
 * The tombstone heads of the source groups of this memory that ever held one of the tuples a
 * row takes and whose rows are gone (recorded here when the closing pass has not reached them
 * yet), other than the row's own group: a revision that revives or re-creates the source names
 * them as parents, so it supersedes the deletions wherever they all arrive, never meets them as
 * a sibling, and joins their groups on this device as it will on every other.
 * ponytail: scans the memory's source revisions per changed row; index tuple_json if it shows.
 */
function retiredSources(db: DatabaseSync, memory: string, own: string, tuple: Row, replica: string, now: number, result: CaptureResult): string[] {
  const wanted = new Set(sourceTuples(tuple));
  const retired: string[] = [];
  if (wanted.size === 0) return retired;
  const groups = new Set<string>();
  for (const stored of prepared(db, `SELECT o.canonical_origin_id AS canonical, r.tuple_json AS tuple FROM sync_revisions r
      JOIN sync_origins o ON o.origin_id = r.origin_id WHERE o.kind = 'source' AND r.tuple_json IS NOT NULL
      AND json_extract(o.natural_json, '$.memory') IN (SELECT origin_id FROM sync_origins WHERE kind = 'memory' AND local_id = ?)`).all(memory)) {
    if (sourceTuples(JSON.parse(String(stored.tuple)) as Row).some((held) => wanted.has(held))) groups.add(String(stored.canonical));
  }
  for (const id of [...groups].sort()) {
    const canonical = canonicalOf(db, id);
    if (canonical.origin_id === own) continue;
    if (canonical.local_id !== null && prepared(db, 'SELECT 1 FROM memory_sources WHERE sync_key = ?').get(canonical.local_id.slice('source:'.length)) !== undefined) continue;
    if (canonical.local_id !== null) recordTombstone(db, canonical, replica, now, result);
    for (const head of headsOf(db, canonical.origin_id)) {
      if (readRevision(db, head)!.control.tombstone && !retired.includes(head)) retired.push(head);
    }
  }
  return retired.slice(0, BOUNDS.parentsPerRevision);
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
    let canonical = canonicalOf(db, origin.origin_id);
    let payload = alignToNatural(kind, toOriginForm(kind, row, originId, resolve), canonical.natural);
    const control = controlOf(kind, row);
    let hash = payloadHash(payload);
    let state = stateHash(hash, control);
    if (canonical.materialized_hash === state) return;
    let parents = canonical.materialized_revision === null ? [] : [canonical.materialized_revision];
    // A source that is new, or comes back after its own deletion, names the deletions of the
    // groups that held its tuples: their revival, recorded once for every device. The retired
    // groups join this row's group (their origins rebound onto the row: a retired origin still
    // names the row it lost), and the canonical that results carries the revision.
    const base = canonical.materialized_revision === null ? undefined : readRevision(db, canonical.materialized_revision);
    const tuple = kind === 'source' ? sourceTupleOf(payload) : null;
    const moved = base !== undefined && !base.control.tombstone && canonicalJson(base.tuple ?? null) !== canonicalJson(tuple);
    if (kind === 'source' && (parents.length === 0 || base?.control.tombstone === true || moved)) {
      const retired = retiredSources(db, String(row.memory_id), canonical.origin_id, tuple!, replica, now, result);
      for (const head of retired) {
        const group = canonicalOf(db, readRevision(db, head)!.origin_id);
        if (group.local_id !== null && group.local_id !== localId) for (const member of originsOfRow(db, 'source', group.local_id)) bindOrigin(db, member.origin_id, localId);
        unionOrigins(db, canonical.origin_id, group.origin_id);
      }
      canonical = canonicalOf(db, origin.origin_id);
      payload = alignToNatural(kind, toOriginForm(kind, row, canonical.origin_id, resolve), canonical.natural);
      hash = payloadHash(payload);
      state = stateHash(hash, control);
      if (canonical.materialized_hash === state) return;
      parents = [...new Set([...(canonical.materialized_revision === null ? [] : [canonical.materialized_revision]), ...retired])].slice(0, BOUNDS.parentsPerRevision);
    }
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
  for (const row of sourceRecords(db)) record('source', row, sourceLocalId(db, String(row.id), resolve));
  for (const row of visibilityRecords(db)) record('visibility', row, String(row.id));
  for (const row of proposalRecords(db)) record('sharing_proposal', row, String(row.id));

  // Rows that vanished (sources and grants are physically deleted) become tombstones once.
  for (const stored of prepared(db, `SELECT * FROM sync_origins WHERE local_id IS NOT NULL AND origin_id = canonical_origin_id`).all()) {
    const origin: Origin = { ...stored, natural: JSON.parse(String(stored.natural_json)) as Row } as unknown as Origin;
    const key = `${String(stored.kind)} ${String(stored.local_id)}`;
    if (seen.has(key)) continue;
    const others = originsOfRow(db, stored.kind as SyncKind, String(stored.local_id)).map((sibling) => `${sibling.kind} ${sibling.local_id}`);
    if (others.some((sibling) => seen.has(sibling))) continue;
    recordTombstone(db, origin, replica, now, result);
  }
  return result;
}
