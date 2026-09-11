// The 0008 tables: origins, stored revisions, derived heads, selected heads and repository keys
// (contracts/sync.md "Merge rules", "Repository identity", "Schema"). Every revision graph rule
// that other sync modules rely on is implemented here once. Security-owned with the rest of sync.
import type { DatabaseSync } from 'node:sqlite';

import { prepared } from '../db/statements.js';
import { compareCodeUnits, sha256Hex } from '../hash.js';
import { isCanonicalRemoteIdentity } from '../repo-identity.js';
import { BOUNDS, type Control } from './format.js';
import { canonicalJson, revisionId, type SyncKind } from './identity.js';

export type Row = Record<string, unknown>;

export type Revision = {
  revision_id: string;
  origin_id: string;
  kind: SyncKind;
  author: string;
  parents: string[];
  control: Control;
  natural: Row;
  payload_hash: string | null;
  payload: Row | null;
  /** A source revision's UNIQUE-tuple members in origin form (see 0008); null for other kinds or when unknown. */
  tuple?: Row | null;
};

export type Origin = {
  origin_id: string;
  kind: SyncKind;
  local_id: string | null;
  natural: Row;
  canonical_origin_id: string;
  selected_head: string | null;
  materialized_revision: string | null;
  materialized_hash: string | null;
  withheld_reason: string | null;
};

export class SyncStoreError extends Error {
  constructor(readonly code: string, message = code) {
    super(message);
    this.name = 'SyncStoreError';
  }
}

export function replicaOriginId(db: DatabaseSync): string {
  const id = prepared(db, 'SELECT origin_id FROM replica_identity WHERE id = 1').get()?.origin_id;
  if (typeof id !== 'string') throw new SyncStoreError('missing_replica_identity');
  return id;
}

/** Fixed-size repository key: global for canonical remotes, replica-scoped for local paths. */
export function repoKeyFor(replica: string, identityKind: string, normalizedIdentity: string): string {
  const hash = sha256Hex(normalizedIdentity);
  return identityKind === 'remote' && isCanonicalRemoteIdentity(normalizedIdentity) ? `remote:${hash}` : `${replica}:common_dir:${hash}`;
}

/** Records every local repository's key so references can be converted in both directions. */
export function registerLocalRepos(db: DatabaseSync, replica: string): Map<string, string> {
  const keys = new Map<string, string>();
  for (const row of prepared(db, 'SELECT id, identity_kind, normalized_identity FROM repos ORDER BY id').iterate()) {
    const key = repoKeyFor(replica, String(row.identity_kind), String(row.normalized_identity));
    prepared(db, `INSERT INTO sync_repo_mappings (repo_key, identity_kind, normalized_identity, local_repo_id)
      VALUES (?, ?, ?, ?) ON CONFLICT(repo_key) DO UPDATE SET local_repo_id = excluded.local_repo_id`)
      .run(key, String(row.identity_kind), String(row.normalized_identity), String(row.id));
    keys.set(String(row.id), key);
  }
  return keys;
}

export function repoKeyOf(db: DatabaseSync, localRepoId: string): string | undefined {
  const row = prepared(db, 'SELECT repo_key FROM sync_repo_mappings WHERE local_repo_id = ?').get(localRepoId);
  return row === undefined ? undefined : String(row.repo_key);
}

export function localRepoOf(db: DatabaseSync, repoKey: string): string | null {
  const row = prepared(db, 'SELECT local_repo_id FROM sync_repo_mappings WHERE repo_key = ?').get(repoKey);
  return row === undefined || row.local_repo_id === null ? null : String(row.local_repo_id);
}

function originFromRow(row: Row): Origin {
  return {
    origin_id: String(row.origin_id), kind: row.kind as SyncKind,
    local_id: row.local_id === null ? null : String(row.local_id),
    natural: JSON.parse(String(row.natural_json)) as Row,
    canonical_origin_id: String(row.canonical_origin_id),
    selected_head: row.selected_head === null ? null : String(row.selected_head),
    materialized_revision: row.materialized_revision === null ? null : String(row.materialized_revision),
    materialized_hash: row.materialized_hash === null ? null : String(row.materialized_hash),
    withheld_reason: row.withheld_reason === null ? null : String(row.withheld_reason),
  };
}

export function readOrigin(db: DatabaseSync, originId: string): Origin | undefined {
  const row = prepared(db, 'SELECT * FROM sync_origins WHERE origin_id = ?').get(originId);
  return row === undefined ? undefined : originFromRow(row);
}

/** The canonical origin of the local row an origin maps to (itself when not aliased). */
export function canonicalOf(db: DatabaseSync, originId: string): Origin {
  const origin = readOrigin(db, originId);
  if (origin === undefined) throw new SyncStoreError('unknown_origin', originId);
  return origin.canonical_origin_id === originId ? origin : canonicalOf(db, origin.canonical_origin_id);
}

export function originsOfRow(db: DatabaseSync, kind: SyncKind, localId: string): Origin[] {
  return prepared(db, 'SELECT * FROM sync_origins WHERE kind = ? AND local_id = ? ORDER BY origin_id').all(kind, localId).map(originFromRow);
}

export function originByNatural(db: DatabaseSync, kind: SyncKind, natural: Row): Origin | undefined {
  const row = prepared(db, 'SELECT * FROM sync_origins WHERE kind = ? AND natural_json = ? ORDER BY origin_id LIMIT 1')
    .get(kind, canonicalJson(natural));
  return row === undefined ? undefined : originFromRow(row);
}

/**
 * Creates an origin row, aliasing it onto an existing local row when another origin of the same
 * kind already maps to `localId`; the canonical origin is the smallest origin id of the row.
 */
export function createOrigin(
  db: DatabaseSync,
  input: { origin_id: string; kind: SyncKind; local_id: string | null; natural: Row; withheld_reason?: string | null },
): Origin {
  const siblings = input.local_id === null ? [] : originsOfRow(db, input.kind, input.local_id);
  const canonical = [input.origin_id, ...siblings.map((origin) => origin.canonical_origin_id)].sort(compareCodeUnits)[0]!;
  prepared(db, `INSERT INTO sync_origins (origin_id, kind, local_id, natural_json, canonical_origin_id, selected_head,
    materialized_revision, materialized_hash, withheld_reason) VALUES (?, ?, ?, ?, ?, NULL, NULL, NULL, ?)`)
    .run(input.origin_id, input.kind, input.local_id, canonicalJson(input.natural), canonical, input.withheld_reason ?? null);
  if (canonical === input.origin_id && siblings.length > 0) {
    // The new origin sorts first: it becomes the row's canonical origin and inherits the row state.
    const previous = readOrigin(db, siblings[0]!.canonical_origin_id)!;
    prepared(db, 'UPDATE sync_origins SET canonical_origin_id = ? WHERE canonical_origin_id = ?').run(canonical, previous.origin_id);
    setSelectedHead(db, canonical, previous.selected_head, previous.materialized_revision, previous.materialized_hash);
    setSelectedHead(db, previous.origin_id, null, null, null);
  }
  return readOrigin(db, input.origin_id)!;
}

/**
 * Joins the groups of two origins of one kind: onto the local row either has (both rows: the
 * caller decides which survives and moves the other first), or by canonical id alone. A parent
 * link between two source origins says their author saw one row (contracts/sync.md "Merge rules").
 */
export function unionOrigins(db: DatabaseSync, a: string, b: string): void {
  const [left, right] = [canonicalOf(db, a), canonicalOf(db, b)];
  if (left.origin_id === right.origin_id) return;
  if (left.local_id !== null || right.local_id !== null) {
    const localId = (left.local_id ?? right.local_id)!;
    for (const origin of [left, right]) if (origin.local_id === null) bindOrigin(db, origin.origin_id, localId);
    return;
  }
  const [canonical, other] = [left.origin_id, right.origin_id].sort(compareCodeUnits) as [string, string];
  prepared(db, 'UPDATE sync_origins SET canonical_origin_id = ? WHERE canonical_origin_id = ?').run(canonical, other);
  setSelectedHead(db, other, null, null, null);
}

/** Points an origin at a local row (after allocation or a later alias) and re-canonicalizes. */
export function bindOrigin(db: DatabaseSync, originId: string, localId: string): Origin {
  const origin = readOrigin(db, originId);
  if (origin === undefined) throw new SyncStoreError('unknown_origin', originId);
  const siblings = originsOfRow(db, origin.kind, localId).filter((sibling) => sibling.origin_id !== originId);
  const canonical = [origin.canonical_origin_id, ...siblings.map((sibling) => sibling.canonical_origin_id)].sort(compareCodeUnits)[0]!;
  prepared(db, 'UPDATE sync_origins SET local_id = ?, withheld_reason = NULL WHERE origin_id = ?').run(localId, originId);
  for (const group of new Set([origin.canonical_origin_id, ...siblings.map((sibling) => sibling.canonical_origin_id)])) {
    if (group === canonical) continue;
    const previous = readOrigin(db, group)!;
    prepared(db, 'UPDATE sync_origins SET canonical_origin_id = ? WHERE canonical_origin_id = ?').run(canonical, group);
    // The canonical keeps what it has and inherits what it lacks: a selected head from a group that
    // had one, and, independently, the materialized base of the row (the group that was applied to
    // the row carries it; an origin that only arrived has a head but no base).
    const current = readOrigin(db, canonical)!;
    if (current.selected_head === null || (current.materialized_revision === null && previous.materialized_revision !== null)) {
      setSelectedHead(db, canonical, current.selected_head ?? previous.selected_head,
        current.materialized_revision ?? previous.materialized_revision, current.materialized_hash ?? previous.materialized_hash);
    }
    setSelectedHead(db, group, null, null, null);
  }
  return readOrigin(db, originId)!;
}

export function revisionFromRow(row: Row): Revision {
  return {
    revision_id: String(row.revision_id), origin_id: String(row.origin_id), kind: row.kind as SyncKind,
    author: String(row.author), parents: JSON.parse(String(row.parents_json)) as string[],
    control: JSON.parse(String(row.control_json)) as Control, natural: JSON.parse(String(row.natural_json)) as Row,
    payload_hash: row.payload_hash === null ? null : String(row.payload_hash),
    payload: row.payload_json === null ? null : JSON.parse(String(row.payload_json)) as Row,
    tuple: row.tuple_json === null || row.tuple_json === undefined ? null : JSON.parse(String(row.tuple_json)) as Row,
  };
}

export function readRevision(db: DatabaseSync, revisionId: string): Revision | undefined {
  const row = prepared(db, 'SELECT * FROM sync_revisions WHERE revision_id = ?').get(revisionId);
  return row === undefined ? undefined : revisionFromRow(row);
}

export function revisionsOfOrigin(db: DatabaseSync, originId: string): Revision[] {
  return prepared(db, 'SELECT * FROM sync_revisions WHERE origin_id = ? ORDER BY stored_at, revision_id').all(originId).map(revisionFromRow);
}

/** Stores identity fields verbatim; returns false when the revision was already stored. */
/** The UNIQUE-tuple members of a source payload, in origin form. */
export function sourceTupleOf(payload: Row): Row {
  return { raw_event_id: payload.raw_event_id, source_hash: payload.source_hash, portion_start: payload.portion_start,
    portion_end: payload.portion_end, context_only: payload.context_only, source_memory_id: payload.source_memory_id };
}

/** The tuples a source group's heads hold: what a deletion or a late alias names. */
export function headTuples(db: DatabaseSync, canonicalOriginId: string): Row[] {
  return headsOf(db, canonicalOriginId).map((head) => readRevision(db, head)?.tuple ?? null).filter((tuple): tuple is Row => tuple !== null);
}

export function storeRevision(db: DatabaseSync, revision: Revision, receivedFrom: string | null, now: number): boolean {
  if (revision.revision_id !== revisionId(revision)) throw new SyncStoreError('revision_id_mismatch', revision.revision_id);
  if (readRevision(db, revision.revision_id) !== undefined) return false;
  const count = Number(prepared(db, 'SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ?').get(revision.origin_id)?.n ?? 0);
  if (count >= BOUNDS.revisionsPerOrigin) throw new SyncStoreError('revisions_per_origin', revision.origin_id);
  // A source revision's tuple: its payload's, the one it arrived with, or, for a control-only
  // revision, its first parent's (it names the row its parent named). A revision whose payload
  // is withheld has no tuple until the payload arrives (storePayload).
  let tuple = revision.tuple ?? null;
  if (revision.kind === 'source') {
    if (revision.payload !== null) tuple = sourceTupleOf(revision.payload);
    if (tuple === null && revision.payload_hash === null) {
      for (const parent of revision.parents) { tuple = readRevision(db, parent)?.tuple ?? null; if (tuple !== null) break; }
    }
  }
  prepared(db, `INSERT INTO sync_revisions (revision_id, origin_id, kind, author, parents_json, control_json, natural_json,
    payload_hash, payload_json, tuple_json, received_from, stored_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(revision.revision_id, revision.origin_id, revision.kind, revision.author, canonicalJson(revision.parents),
      canonicalJson(revision.control), canonicalJson(revision.natural), revision.payload_hash,
      revision.payload === null ? null : canonicalJson(revision.payload), tuple === null ? null : canonicalJson(tuple), receivedFrom, now);
  for (const parent of revision.parents) {
    prepared(db, 'INSERT INTO sync_revision_parents (child, parent) VALUES (?, ?)').run(revision.revision_id, parent);
  }
  return true;
}

/** Fills a payload for a stored identity-only revision (the caller has verified its hash). */
export function storePayload(db: DatabaseSync, revisionIdValue: string, payload: Row): void {
  const kind = prepared(db, 'SELECT kind FROM sync_revisions WHERE revision_id = ?').get(revisionIdValue)?.kind;
  if (kind !== 'source') {
    prepared(db, 'UPDATE sync_revisions SET payload_json = ? WHERE revision_id = ? AND payload_json IS NULL').run(canonicalJson(payload), revisionIdValue);
    return;
  }
  const tuple = canonicalJson(sourceTupleOf(payload));
  prepared(db, 'UPDATE sync_revisions SET payload_json = ?, tuple_json = COALESCE(?, tuple_json) WHERE revision_id = ? AND payload_json IS NULL')
    .run(canonicalJson(payload), tuple, revisionIdValue);
  // A control-only descendant (a tombstone) that inherited a null tuple from this revision while
  // its payload was withheld takes the tuple now, so a deletion still names the row.
  for (let frontier = [revisionIdValue]; frontier.length > 0; ) {
    const next: string[] = [];
    for (const parent of frontier) {
      for (const child of prepared(db, `SELECT c.revision_id FROM sync_revision_parents p JOIN sync_revisions c ON c.revision_id = p.child
        WHERE p.parent = ? AND c.kind = 'source' AND c.payload_json IS NULL AND c.tuple_json IS NULL`).all(parent)) {
        prepared(db, 'UPDATE sync_revisions SET tuple_json = ? WHERE revision_id = ?').run(tuple, String(child.revision_id));
        next.push(String(child.revision_id));
      }
    }
    frontier = next;
  }
}

/** Erases stored payload bytes of every revision of every origin aliased to a row. */
export function erasePayloads(db: DatabaseSync, canonicalOriginId: string): void {
  prepared(db, `UPDATE sync_revisions SET payload_json = NULL WHERE origin_id IN
    (SELECT origin_id FROM sync_origins WHERE canonical_origin_id = ?)`).run(canonicalOriginId);
}

/**
 * Heads of a local row: stored revisions of any aliased origin that no stored revision of the
 * same row names as a parent. A parent link into another row counts only once the two rows are
 * aliased here (contracts/sync.md "Identity").
 */
export function headsOf(db: DatabaseSync, canonicalOriginId: string): string[] {
  return prepared(db, `SELECT r.revision_id FROM sync_revisions r JOIN sync_origins o ON o.origin_id = r.origin_id
    WHERE o.canonical_origin_id = ? AND NOT EXISTS (
      SELECT 1 FROM sync_revision_parents p JOIN sync_revisions c ON c.revision_id = p.child
      JOIN sync_origins co ON co.origin_id = c.origin_id
      WHERE p.parent = r.revision_id AND co.canonical_origin_id = o.canonical_origin_id)
    ORDER BY r.revision_id`).all(canonicalOriginId).map((row) => String(row.revision_id));
}

/** True when `descendant` reaches `ancestor` through stored parent links (bounded by the origin cap). */
export function descendsFrom(db: DatabaseSync, descendant: string, ancestor: string): boolean {
  if (descendant === ancestor) return true;
  const seen = new Set<string>();
  const stack = [descendant];
  while (stack.length > 0) {
    const current = stack.pop()!;
    for (const row of prepared(db, 'SELECT parent FROM sync_revision_parents WHERE child = ?').all(current)) {
      const parent = String(row.parent);
      if (parent === ancestor) return true;
      if (!seen.has(parent)) { seen.add(parent); stack.push(parent); }
    }
  }
  return false;
}

/** The control a row is under: any stored tombstone dominates; floors merge upward only. */
export function effectiveControl(db: DatabaseSync, canonicalOriginId: string): Control {
  const RANK = { eligible: 0, local_only: 1, private: 2, secret: 3 } as const;
  let control: Control = { tombstone: false, sensitivity_floor: 'eligible' };
  for (const row of prepared(db, `SELECT r.control_json FROM sync_revisions r JOIN sync_origins o ON o.origin_id = r.origin_id
    WHERE o.canonical_origin_id = ?`).iterate(canonicalOriginId)) {
    const stored = JSON.parse(String(row.control_json)) as Control;
    control = {
      tombstone: control.tombstone || stored.tombstone,
      sensitivity_floor: RANK[stored.sensitivity_floor] > RANK[control.sensitivity_floor] ? stored.sensitivity_floor : control.sensitivity_floor,
    };
  }
  // A source's deletion is a state, not a fate: it holds while a head carries it, and a later
  // revision of the row (an identical row inserted again, a row re-created under the old
  // tuple that names the tombstone as parent) revives it. Rows of the other kinds are never
  // physically re-created, so their tombstone is final.
  if (control.tombstone && readOrigin(db, canonicalOriginId)?.kind === 'source') {
    control.tombstone = headsOf(db, canonicalOriginId).some((head) => readRevision(db, head)!.control.tombstone);
  }
  return control;
}

export function setSelectedHead(
  db: DatabaseSync, canonicalOriginId: string, selected: string | null, materializedRevision: string | null, materializedHash: string | null,
): void {
  prepared(db, 'UPDATE sync_origins SET selected_head = ?, materialized_revision = ?, materialized_hash = ? WHERE origin_id = ?')
    .run(selected, materializedRevision, materializedHash, canonicalOriginId);
}

/** The state hash a row's materialization records: payload hash plus the control the row implies. */
export function stateHash(payloadHashValue: string | null, control: Control): string {
  return sha256Hex(canonicalJson([payloadHashValue, control]));
}
