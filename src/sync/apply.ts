// Pull apply (contracts/sync.md "Pull" step 6 and "Merge rules"): inside the caller's
// `BEGIN IMMEDIATE`, record local changes, store the staged revisions (aliasing origins onto
// rows the store already recognizes), then materialize every touched row from its selected
// head under the effective control, in dependency order, with the checkpoint, lineage and
// approval rules the contract fixes. Security-owned: nothing pulled ever widens authority.
import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { grantVisibility } from '../db/queries.js';
import { prepared } from '../db/statements.js';
import { sha256Hex, sha256Json } from '../hash.js';
import { cjkBigrams } from '../retrieval/fts.js';
import { contextRecords, memoryRecords, proposalRecords, sourceRecords, visibilityRecords, workRecords } from '../transfer-records.js';
import { captureLocalChanges, controlOf, sourceLocalId, toOriginForm } from './capture.js';
import { BOUNDS, type Control } from './format.js';
import { payloadHash, revisionId, type Sensitivity, type SyncKind } from './identity.js';
import { BundleRejected, stagedLines, stagedOrigins, stagedRepos, type Staged } from './stage.js';
import {
  bindOrigin, canonicalOf, createOrigin, descendsFrom, effectiveControl, erasePayloads, headsOf, localRepoOf, originsOfRow,
  readOrigin, readRevision, registerLocalRepos, replicaOriginId, setSelectedHead, stateHash, storePayload,
  storeRevision, type Origin, type Revision, type Row,
} from './store.js';

const RANK: Record<Sensitivity, number> = { eligible: 0, local_only: 1, private: 2, secret: 3 };
const KIND_ORDER: SyncKind[] = ['context', 'work', 'memory', 'source', 'visibility', 'sharing_proposal'];

export type ApplyResult = {
  stored: number; filled: number; materialized: number; withheldOnApply: number; conflicts: number;
};

function stricter(a: Sensitivity, b: Sensitivity): Sensitivity { return RANK[a] >= RANK[b] ? a : b; }

class Unresolved extends Error {
  constructor(readonly reason: string) { super(reason); }
}

type Resolver = {
  replica: string;
  localOf(kind: SyncKind, originId: string): string;
  repo(key: string): string;
};

/** Repository lines: canonical remotes resolve everywhere; foreign paths wait for `map-repo`. */
function applyRepoLines(db: DatabaseSync, staged: Staged, replica: string, now: number): void {
  for (const repo of stagedRepos(staged)) {
    const existing = prepared(db, 'SELECT local_repo_id FROM sync_repo_mappings WHERE repo_key = ?').get(repo.origin_id);
    if (existing?.local_repo_id != null) continue;
    let localRepoId: string | null = null;
    if (repo.origin_id.startsWith('remote:')) {
      if (`remote:${sha256Hex(repo.normalized_identity)}` !== repo.origin_id) throw new BundleRejected('repo_key_mismatch', repo.origin_id);
      const id = sha256Hex(repo.normalized_identity).slice(0, 16);
      prepared(db, `INSERT OR IGNORE INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
        VALUES (?, 'remote', ?, ?, ?, ?)`).run(id, repo.normalized_identity, repo.normalized_identity, now, now);
      localRepoId = String(prepared(db, 'SELECT id FROM repos WHERE normalized_identity = ?').get(repo.normalized_identity)!.id);
    } else if (repo.origin_id.startsWith(`${replica}:common_dir:`)) {
      const row = prepared(db, 'SELECT id FROM repos WHERE normalized_identity = ?').get(repo.normalized_identity);
      localRepoId = row === undefined ? null : String(row.id);
    }
    prepared(db, `INSERT INTO sync_repo_mappings (repo_key, identity_kind, normalized_identity, local_repo_id) VALUES (?, ?, ?, ?)
      ON CONFLICT(repo_key) DO UPDATE SET local_repo_id = COALESCE(sync_repo_mappings.local_repo_id, excluded.local_repo_id)`)
      .run(repo.origin_id, repo.identity_kind, repo.normalized_identity, localRepoId);
  }
}

/** The local row a natural key already names here, if any (contracts/sync.md "Identity"). */
function aliasTarget(db: DatabaseSync, kind: SyncKind, natural: Row): string | null {
  const local = (originId: unknown): string | null =>
    originId === null ? null : (readOrigin(db, String(originId)) === undefined ? null : canonicalOf(db, String(originId)).local_id);
  switch (kind) {
    case 'memory': {
      if (natural.domain === 'personal_projection') {
        return prepared(db, 'SELECT id FROM memories WHERE content_hash = ?').get(String(natural.projection_hash))?.id as string ?? null;
      }
      const repo = localRepoOf(db, String(natural.repo));
      if (repo === null) return null;
      if (natural.domain === 'checkpoint') {
        const work = local(natural.work);
        if (work === null) return null;
        const parent = natural.parent === null ? null : local(natural.parent);
        if (natural.parent !== null && parent === null) return null;
        return prepared(db, 'SELECT id FROM memories WHERE work_id = ? AND material_hash = ? AND checkpoint_parent_id IS ? ORDER BY id LIMIT 1')
          .get(work, String(natural.material_hash), parent)?.id as string ?? null;
      }
      return prepared(db, 'SELECT id FROM memories WHERE content_hash = ?').get(sha256Json([repo, String(natural.material_hash)]))?.id as string ?? null;
    }
    case 'source': {
      const memory = local(natural.memory);
      return memory === null ? null : `source:${memory}:${String(natural.source_hash)}`;
    }
    case 'visibility': {
      const memory = local(natural.memory);
      if (memory === null) return null;
      const repo = natural.repo === null ? null : localRepoOf(db, String(natural.repo));
      const work = natural.work === null ? null : local(natural.work);
      if ((natural.repo !== null && repo === null) || (natural.work !== null && work === null)) return null;
      return prepared(db, 'SELECT id FROM memory_visibility WHERE memory_id = ? AND audience = ? AND repo_id IS ? AND work_id IS ?')
        .get(memory, String(natural.audience), repo, work)?.id as string ?? null;
    }
    case 'sharing_proposal': {
      const memory = local(natural.origin_memory);
      return memory === null ? null : prepared(db, 'SELECT id FROM sharing_proposals WHERE origin_memory_id = ? AND candidate_material_hash = ? ORDER BY id LIMIT 1')
        .get(memory, String(natural.candidate))?.id as string ?? null;
    }
    case 'context': {
      const repo = localRepoOf(db, String(natural.repo));
      return repo === null ? null : prepared(db, 'SELECT id FROM work_contexts WHERE repo_id = ? AND local_key = ?')
        .get(repo, String(natural.local_key))?.id as string ?? null;
    }
    case 'work': return null;
  }
}

function lineToRevision(line: ReturnType<typeof stagedLines>[number]): Revision {
  return {
    revision_id: line.revision_id, origin_id: line.origin_id, kind: line.kind, author: line.author, parents: line.parents,
    control: line.control, natural: line.natural, payload_hash: line.payload_hash, payload: line.payload,
  };
}

/** Phase one: origins (aliased where the store recognizes them) and revisions in parents order. */
function storeStaged(db: DatabaseSync, staged: Staged, sender: string, now: number, result: ApplyResult): Set<string> {
  const touched = new Set<string>();
  const origins = stagedOrigins(staged);
  // Dependency order for aliasing: a checkpoint's natural names its work and parent; a source's
  // names its memory. Kinds first, then repeated passes until every natural resolves or stalls.
  const ordered = [...origins].sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind));
  for (const origin of ordered) {
    if (readOrigin(db, origin.origin_id) !== undefined) continue;
    const localId = aliasTarget(db, origin.kind, origin.natural);
    createOrigin(db, { origin_id: origin.origin_id, kind: origin.kind, local_id: localId, natural: origin.natural,
      withheld_reason: localId === null ? 'identity_only' : null });
  }
  for (const origin of ordered) {
    const lines = new Map(stagedLines(staged, origin.origin_id).map((line) => [line.revision_id, line]));
    const done = new Set<string>();
    const visit = (id: string): void => {
      if (done.has(id)) return;
      const line = lines.get(id);
      if (line === undefined) return; // a stored parent
      done.add(id);
      for (const parent of line.parents) visit(parent);
      const revision = lineToRevision(line);
      if (storeRevision(db, revision, sender, now)) result.stored += 1;
      else if (line.payload !== null && readRevision(db, id)?.payload === null) {
        const control = effectiveControl(db, canonicalOf(db, origin.origin_id).origin_id);
        if (!control.tombstone && control.sensitivity_floor !== 'secret') { storePayload(db, id, line.payload); result.filled += 1; }
      }
    };
    for (const id of lines.keys()) visit(id);
    touched.add(canonicalOf(db, origin.origin_id).origin_id);
  }
  return touched;
}

/** Selected-head rule: descend along the local line, accept a full resolution, never break ties. */
function selectHead(db: DatabaseSync, row: Origin, heads: string[]): string | null {
  if (heads.length === 0) return row.selected_head;
  if (row.selected_head === null) {
    const first = prepared(db, `SELECT revision_id FROM sync_revisions WHERE revision_id IN (${heads.map(() => '?').join(', ')})
      ORDER BY stored_at, revision_id LIMIT 1`).get(...heads);
    return String(first!.revision_id);
  }
  if (heads.includes(row.selected_head)) return row.selected_head;
  if (heads.length === 1) {
    const only = readRevision(db, heads[0]!)!;
    if (only.parents.length > 1 || descendsFrom(db, only.revision_id, row.selected_head)) return only.revision_id;
    return row.selected_head;
  }
  const descendants = heads.filter((head) => descendsFrom(db, head, row.selected_head!));
  return descendants.length === 1 ? descendants[0]! : row.selected_head;
}

function reportConflict(db: DatabaseSync, row: Origin, heads: string[], selected: string | null, now: number, result: ApplyResult): void {
  const id = `sync:${row.origin_id}`;
  if (heads.length <= 1) {
    prepared(db, "UPDATE sync_conflicts SET status = 'resolved' WHERE id = ? AND status = 'open'").run(id);
    return;
  }
  const detail = heads.map((head) => { const r = readRevision(db, head)!; return { revision: head, origin: r.origin_id, author: r.author }; });
  const repo = row.natural.repo === undefined ? null : localRepoOf(db, String(row.natural.repo));
  prepared(db, `INSERT INTO sync_conflicts (id, repo_id, content_hash, local_state_json, remote_state_json, status, created_at)
    VALUES (?, ?, NULL, ?, ?, 'open', ?) ON CONFLICT(id) DO UPDATE SET local_state_json = excluded.local_state_json,
    remote_state_json = excluded.remote_state_json, status = 'open'`)
    .run(id, repo, JSON.stringify({ origin: row.origin_id, kind: row.kind, selected }), JSON.stringify({ heads: detail }), now);
  result.conflicts += 1;
}

type Writer = (db: DatabaseSync, row: Origin, payload: Row | null, control: Control, resolve: Resolver, now: number) => string | null;

function memoryWriter(db: DatabaseSync, row: Origin, payload: Row | null, control: Control, resolve: Resolver, now: number): string | null {
  if (payload === null) {
    if (row.local_id === null) return null;
    applyControlToMemory(db, row.local_id, control, now);
    return row.local_id;
  }
  const repo = resolve.repo(String(payload.repo_id));
  const personal = payload.identity_domain === 'personal_projection';
  if (personal && !approvedProjection(db, String(payload.content_hash))) throw new Unresolved('approval_missing');
  const contentHash = personal ? String(payload.content_hash) : sha256Json([repo, String(payload.material_hash)]);
  const workId = payload.work_id === null ? null : resolve.localOf('work', String(payload.work_id));
  const parentId = payload.checkpoint_parent_id === null ? null : resolve.localOf('memory', String(payload.checkpoint_parent_id));
  const supersededBy = payload.superseded_by === null ? null : resolve.localOf('memory', String(payload.superseded_by));
  const localId = row.local_id ?? (prepared(db, 'SELECT id FROM memories WHERE content_hash = ?').get(contentHash)?.id as string | undefined)
    ?? `m_${contentHash.slice(0, 24)}`;
  const sensitivity = stricter(payload.sensitivity as Sensitivity, control.sensitivity_floor);
  const secret = sensitivity === 'secret';
  const deletedAt = control.tombstone ? (payload.deleted_at as number | null) ?? now : null;
  const title = secret || control.tombstone ? '' : payload.title;
  const body = secret || control.tombstone ? '' : payload.body;
  prepared(db, `INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash, sensitivity,
      review_state, degraded_reason, source_session_id, source_batch_id, valid_from, valid_to, superseded_by, pinned_at, pin_order,
      deleted_at, created_at, work_id, checkpoint_parent_id, provenance_complete, source_captured_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET type = excluded.type, title = excluded.title, body = excluded.body, concepts = excluded.concepts,
      cjk_bigrams = excluded.cjk_bigrams, sensitivity = excluded.sensitivity, review_state = excluded.review_state,
      degraded_reason = excluded.degraded_reason, valid_from = excluded.valid_from, valid_to = excluded.valid_to,
      superseded_by = excluded.superseded_by, pinned_at = excluded.pinned_at, pin_order = excluded.pin_order,
      deleted_at = COALESCE(memories.deleted_at, excluded.deleted_at), provenance_complete = excluded.provenance_complete,
      source_captured_at = excluded.source_captured_at`)
    .run(localId, repo, payload.type as string, title as string | null, body as string | null,
      secret || control.tombstone ? '[]' : payload.concepts as string | null, cjkBigrams(`${String(title ?? '')} ${String(body ?? '')}`),
      payload.material_hash as string, contentHash, sensitivity, payload.review_state as string, payload.degraded_reason as string | null,
      payload.source_session_id as string | null, payload.source_batch_id as string | null, payload.valid_from as number | null,
      payload.valid_to as number | null, supersededBy, payload.pinned_at as number | null, payload.pin_order as number | null,
      deletedAt, payload.created_at as number | null, workId, parentId, payload.provenance_complete as number | null,
      payload.source_captured_at as number | null);
  if (row.local_id === null) bindOrigin(db, row.origin_id, localId);
  return localId;
}

function applyControlToMemory(db: DatabaseSync, localId: string, control: Control, now: number): void {
  if (control.tombstone) prepared(db, 'UPDATE memories SET deleted_at = COALESCE(deleted_at, ?) WHERE id = ?').run(now, localId);
  prepared(db, `UPDATE memories SET sensitivity = ? WHERE id = ? AND CASE sensitivity WHEN 'eligible' THEN 0 WHEN 'local_only' THEN 1
    WHEN 'private' THEN 2 ELSE 3 END < ?`).run(control.sensitivity_floor, localId, RANK[control.sensitivity_floor]);
  if (control.sensitivity_floor === 'secret' || control.tombstone) {
    prepared(db, "UPDATE memories SET title = '', body = '', concepts = '[]', cjk_bigrams = '' WHERE id = ?").run(localId);
  }
}

function approvedProjection(db: DatabaseSync, projectionHash: string): boolean {
  return prepared(db, 'SELECT 1 FROM sync_approvals WHERE projection_hash = ?').get(projectionHash) !== undefined;
}

function sourceWriter(db: DatabaseSync, row: Origin, payload: Row | null, control: Control, resolve: Resolver, now: number): string | null {
  void now;
  const memory = row.natural.memory === undefined ? null : resolve.localOf('memory', String(row.natural.memory));
  if (memory === null) throw new Unresolved('unresolved_reference');
  const localId = `source:${memory}:${String(row.natural.source_hash)}`;
  const existing = findSourceRow(db, memory, localId, resolve);
  if (control.tombstone) {
    if (existing !== null) prepared(db, 'DELETE FROM memory_sources WHERE id = ?').run(existing);
    return localId;
  }
  if (payload === null) return existing === null ? null : localId;
  const sourceMemory = payload.source_memory_id === null ? null : resolve.localOf('memory', String(payload.source_memory_id));
  const sourceContext = payload.source_context_id === null ? null : resolve.localOf('context', String(payload.source_context_id));
  if (existing !== null) prepared(db, 'DELETE FROM memory_sources WHERE id = ?').run(existing);
  prepared(db, `INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value, source_agent, portion_start, portion_end,
      source_total, source_hash, evidence, captured_at, source_processed_at, capture_root, source_paths_json, source_context_id,
      context_only, source_memory_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(memory, payload.raw_event_id as string | null, payload.citation_kind as string | null, payload.citation_value as string | null,
      payload.source_agent as string | null, payload.portion_start as number | null, payload.portion_end as number | null,
      payload.source_total as number | null, payload.source_hash as string | null, payload.evidence as string | null,
      payload.captured_at as number | null, payload.source_processed_at as number | null, payload.capture_root as string | null,
      payload.source_paths_json as string | null, sourceContext, payload.context_only as number, sourceMemory);
  if (row.local_id === null) bindOrigin(db, row.origin_id, localId);
  return localId;
}

function findSourceRow(db: DatabaseSync, memory: string, localId: string, resolve: Resolver): number | null {
  const originOf = (kind: SyncKind, id: string): string => originsOfRow(db, kind, id)[0]?.canonical_origin_id ?? `${resolve.replica}:${id}`;
  for (const row of prepared(db, 'SELECT id FROM memory_sources WHERE memory_id = ?').iterate(memory)) {
    if (sourceLocalId(db, String(row.id), originOf) === localId) return Number(row.id);
  }
  return null;
}

function contextWriter(db: DatabaseSync, row: Origin, payload: Row | null, control: Control, resolve: Resolver, now: number): string | null {
  void control; void now;
  if (payload === null) return row.local_id;
  const repo = resolve.repo(String(payload.repo_id));
  const localId = row.local_id ?? (prepared(db, 'SELECT id FROM work_contexts WHERE repo_id = ? AND local_key = ?')
    .get(repo, String(payload.local_key))?.id as string | undefined) ?? randomUUID();
  prepared(db, `INSERT INTO work_contexts (id, repo_id, local_key, root, repo_secret_paths_json, created_at, last_seen_at)
    VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET root = excluded.root,
    repo_secret_paths_json = excluded.repo_secret_paths_json, last_seen_at = MAX(work_contexts.last_seen_at, excluded.last_seen_at)`)
    .run(localId, repo, String(payload.local_key), (payload.root as string | null) ?? '', payload.repo_secret_paths_json as string | null,
      payload.created_at as number, payload.last_seen_at as number);
  if (row.local_id === null) bindOrigin(db, row.origin_id, localId);
  return localId;
}

function workWriter(db: DatabaseSync, row: Origin, payload: Row | null, control: Control, resolve: Resolver, now: number): string | null {
  void now;
  if (payload === null) return row.local_id;
  const repo = resolve.repo(String(payload.repo_id));
  const context = resolve.localOf('context', String(payload.origin_context_id));
  const localId = row.local_id ?? randomUUID();
  const sensitivity = stricter(payload.purpose_sensitivity as Sensitivity, control.sensitivity_floor);
  prepared(db, `INSERT INTO work_items (id, repo_id, origin_context_id, purpose, purpose_source_event_id, purpose_sensitivity, state,
      created_at, updated_at, completed_at, current_checkpoint_memory_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
    ON CONFLICT(id) DO UPDATE SET purpose = excluded.purpose, purpose_source_event_id = excluded.purpose_source_event_id,
      purpose_sensitivity = excluded.purpose_sensitivity, state = excluded.state, updated_at = excluded.updated_at,
      completed_at = excluded.completed_at`)
    .run(localId, repo, context, sensitivity === 'secret' ? null : payload.purpose as string | null,
      payload.purpose_source_event_id as string | null, sensitivity, payload.state as string, payload.created_at as number,
      payload.updated_at as number, payload.completed_at as number | null);
  if (row.local_id === null) bindOrigin(db, row.origin_id, localId);
  return localId;
}

function visibilityWriter(db: DatabaseSync, row: Origin, payload: Row | null, control: Control, resolve: Resolver, now: number): string | null {
  void now;
  if (control.tombstone) {
    if (row.local_id !== null) prepared(db, 'DELETE FROM memory_visibility WHERE id = ?').run(row.local_id);
    return row.local_id;
  }
  if (payload === null) return row.local_id;
  const memory = resolve.localOf('memory', String(payload.memory_id));
  const repo = payload.repo_id === null ? null : resolve.repo(String(payload.repo_id));
  const work = payload.work_id === null ? null : resolve.localOf('work', String(payload.work_id));
  const proposal = payload.proposal_id === null ? null : resolve.localOf('sharing_proposal', String(payload.proposal_id));
  if (payload.audience === 'personal' && (proposal === null || !locallyApproved(db, proposal))) throw new Unresolved('approval_missing');
  const grant = payload.audience === 'work' ? { audience: 'work' as const, repoId: repo!, workId: work! }
    : payload.audience === 'project' ? { audience: 'project' as const, repoId: repo! } : { audience: 'personal' as const, proposalId: proposal! };
  grantVisibility(db, memory, grant, payload.grant_kind as 'migration' | 'observer' | 'explicit_adoption' | 'proposal_approval', payload.created_at as number);
  const localId = row.local_id ?? `v_${sha256Json([memory, payload.audience, repo, work])}`;
  if (row.local_id === null) bindOrigin(db, row.origin_id, localId);
  return localId;
}

function locallyApproved(db: DatabaseSync, proposalLocalId: string): boolean {
  return prepared(db, 'SELECT 1 FROM sync_approvals WHERE proposal_id = ?').get(proposalLocalId) !== undefined;
}

function proposalWriter(db: DatabaseSync, row: Origin, payload: Row | null, control: Control, resolve: Resolver, now: number): string | null {
  void now;
  if (payload === null) return row.local_id;
  const memory = resolve.localOf('memory', String(payload.origin_memory_id));
  const repo = resolve.repo(String(payload.origin_repo_id));
  const work = resolve.localOf('work', String(payload.origin_work_id));
  const localId = row.local_id ?? randomUUID();
  const sensitivity = stricter(payload.candidate_sensitivity as Sensitivity, control.sensitivity_floor);
  const redacted = payload.redacted === true || sensitivity === 'secret';
  // An approval binds only to what this device's user approved; otherwise the decision is visible as pending.
  const approvedHere = payload.state === 'approved' && locallyApproved(db, localId)
    && prepared(db, 'SELECT 1 FROM sync_approvals WHERE proposal_id = ? AND candidate_hash = ?').get(localId, String(payload.candidate_material_hash)) !== undefined;
  const state = payload.state === 'approved' && !approvedHere ? 'pending' : payload.state as string;
  const projected = state === 'approved' && payload.projected_memory_id !== null ? resolve.localOf('memory', String(payload.projected_memory_id)) : null;
  prepared(db, `INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title, candidate_body,
      candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, decision_channel, projected_memory_id, created_at, decided_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET candidate_title = excluded.candidate_title, candidate_body = excluded.candidate_body,
      candidate_sensitivity = excluded.candidate_sensitivity, source_event_ids_json = excluded.source_event_ids_json,
      state = excluded.state, decision_channel = excluded.decision_channel, projected_memory_id = excluded.projected_memory_id,
      decided_at = excluded.decided_at`)
    .run(localId, memory, repo, work, redacted ? '' : payload.candidate_title as string, redacted ? '' : payload.candidate_body as string,
      payload.candidate_material_hash as string, sensitivity, redacted ? '[]' : payload.source_event_ids_json as string,
      payload.basis as string, state, state === 'pending' ? null : payload.decision_channel as string | null, projected,
      payload.created_at as number, state === 'pending' ? null : payload.decided_at as number | null);
  if (row.local_id === null) bindOrigin(db, row.origin_id, localId);
  return localId;
}

const WRITERS: Record<SyncKind, Writer> = {
  memory: memoryWriter, source: sourceWriter, context: contextWriter, work: workWriter, visibility: visibilityWriter,
  sharing_proposal: proposalWriter,
};

/** What the capture pass would record for this row now: the materialized state after corrections. */
function materializedState(db: DatabaseSync, row: Origin, localId: string, resolve: Resolver): string | null {
  const record = localRecord(db, row.kind, localId, resolve);
  if (record === null) return null;
  const originOf = (kind: SyncKind, id: string): string => originsOfRow(db, kind, id)[0]?.canonical_origin_id ?? `${resolve.replica}:${id}`;
  const repoKeys = registerLocalRepos(db, resolve.replica);
  const captureResolver = { originOf, repoKey: (id: string) => repoKeys.get(id) ?? resolve.replica };
  const payload = toOriginForm(row.kind, record, row.origin_id, captureResolver);
  return stateHash(payloadHash(payload), controlOf(row.kind, record));
}

function localRecord(db: DatabaseSync, kind: SyncKind, localId: string, resolve: Resolver): Row | null {
  const one = (records: Iterable<Row>): Row | null => { for (const record of records) return record; return null; };
  switch (kind) {
    case 'memory': return one(memoryRecords(db, localId));
    case 'context': return one(contextRecords(db, localId));
    case 'work': return one(workRecords(db, localId));
    case 'visibility': return one(visibilityRecords(db, localId));
    case 'sharing_proposal': return one(proposalRecords(db, localId));
    case 'source': {
      const memory = localId.split(':')[1]!;
      const rowid = findSourceRow(db, memory, localId, resolve);
      return rowid === null ? null : one(sourceRecords(db, rowid));
    }
  }
}

/** Lineage inheritance: a child applied under a stricter parent is raised in the same transaction. */
function raiseToParents(db: DatabaseSync, localId: string): void {
  let floor: Sensitivity = 'eligible';
  for (const parent of prepared(db, `SELECT p.sensitivity FROM memories m JOIN memories p ON p.id = m.checkpoint_parent_id WHERE m.id = ?
    UNION ALL SELECT p.sensitivity FROM memory_sources ms JOIN memories p ON p.id = ms.source_memory_id WHERE ms.memory_id = ?`).all(localId, localId)) {
    floor = stricter(floor, parent.sensitivity as Sensitivity);
  }
  if (floor !== 'eligible') applyControlToMemory(db, localId, { tombstone: false, sensitivity_floor: floor }, 0);
}

/** Checkpoint deletion unit: a tombstone for {work, material} deletes every stored checkpoint of it. */
function sweepCheckpoint(db: DatabaseSync, row: Origin, now: number): void {
  if (row.natural.domain !== 'checkpoint') return;
  const work = readOrigin(db, String(row.natural.work)) === undefined ? null : canonicalOf(db, String(row.natural.work)).local_id;
  if (work === null) return;
  prepared(db, 'UPDATE memories SET deleted_at = ? WHERE work_id = ? AND material_hash = ? AND deleted_at IS NULL')
    .run(now, work, String(row.natural.material_hash));
}

/** The work's pointer follows a pulled head only when the chain reaches the local checkpoint. */
function advanceWorkCheckpoint(db: DatabaseSync, row: Origin, selected: string): boolean {
  const revision = readRevision(db, selected);
  if (revision?.payload == null || row.local_id === null) return true;
  const target = revision.payload.current_checkpoint_memory_id === null ? null : resolveLocal(db, 'memory', String(revision.payload.current_checkpoint_memory_id));
  if (revision.payload.current_checkpoint_memory_id !== null && target === null) return false;
  const current = prepared(db, 'SELECT current_checkpoint_memory_id FROM work_items WHERE id = ?').get(row.local_id)?.current_checkpoint_memory_id as string | null;
  const reaches = (from: string | null): boolean => {
    let cursor = from;
    for (let steps = 0; cursor !== null && steps < BOUNDS.revisionsPerOrigin; steps += 1) {
      if (cursor === current) return true;
      cursor = prepared(db, 'SELECT checkpoint_parent_id FROM memories WHERE id = ?').get(cursor)?.checkpoint_parent_id as string | null ?? null;
    }
    return current === null;
  };
  if (!(revision.parents.length > 1 || reaches(target))) return false;
  prepared(db, 'UPDATE work_items SET current_checkpoint_memory_id = ? WHERE id = ?').run(target, row.local_id);
  return true;
}

function resolveLocal(db: DatabaseSync, kind: SyncKind, originId: string): string | null {
  const origin = readOrigin(db, originId);
  if (origin === undefined || origin.kind !== kind) return null;
  return canonicalOf(db, originId).local_id;
}

function resolverFor(db: DatabaseSync, replica: string): Resolver {
  return {
    replica,
    localOf: (kind, originId) => {
      const local = resolveLocal(db, kind, originId);
      if (local === null) throw new Unresolved('unresolved_reference');
      return local;
    },
    repo: (key) => {
      const local = localRepoOf(db, key);
      if (local === null) throw new Unresolved('unmapped_repo');
      return local;
    },
  };
}

/**
 * Phase two for a set of local rows (canonical origin ids): selected head, effective control,
 * writer, materialized state, conflict report, checkpoint and lineage rules, then the closing
 * change pass. Shared by pull and by `map-repo`, which re-evaluates withheld rows without a bundle.
 */
export function materializeRows(db: DatabaseSync, rowIds: readonly string[], now: number): ApplyResult {
  const result: ApplyResult = { stored: 0, filled: 0, materialized: 0, withheldOnApply: 0, conflicts: 0 };
  const resolve = resolverFor(db, replicaOriginId(db));
  const rows = rowIds.map((id) => readOrigin(db, id)!).sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind));
  const pending = new Set(rows.map((row) => row.origin_id));
  const workRows: Origin[] = [];
  let progress = true;
  while (progress && pending.size > 0) {
    progress = false;
    for (const stale of rows) {
      if (!pending.has(stale.origin_id)) continue;
      const row = readOrigin(db, stale.origin_id)!;
      const heads = headsOf(db, row.origin_id);
      if (heads.length > BOUNDS.headsPerOrigin) throw new BundleRejected('heads_per_row', row.origin_id);
      const selected = selectHead(db, row, heads);
      const control = effectiveControl(db, row.origin_id);
      if (control.tombstone || control.sensitivity_floor === 'secret') erasePayloads(db, row.origin_id);
      const revision = selected === null ? undefined : readRevision(db, selected);
      const payload = revision?.payload ?? null;
      try {
        const localId = WRITERS[row.kind](db, row, payload, control, resolve, now);
        pending.delete(row.origin_id);
        progress = true;
        const after = readOrigin(db, row.origin_id)!;
        const fallback = stateHash(revision?.payload_hash ?? null, control);
        const state = localId === null ? fallback : materializedState(db, after, localId, resolve) ?? fallback;
        const materializedRevision = payload !== null ? selected : after.materialized_revision ?? selected;
        setSelectedHead(db, row.origin_id, selected, materializedRevision, state);
        prepared(db, 'UPDATE sync_origins SET withheld_reason = NULL WHERE origin_id = ? AND local_id IS NOT NULL').run(row.origin_id);
        reportConflict(db, after, heads, selected, now, result);
        result.materialized += 1;
        if (row.kind === 'work') workRows.push(after);
        if (row.kind === 'memory' && localId !== null) {
          if (control.tombstone) sweepCheckpoint(db, after, now);
          raiseToParents(db, localId);
        }
      } catch (error) {
        if (!(error instanceof Unresolved)) throw error;
        prepared(db, 'UPDATE sync_origins SET withheld_reason = ? WHERE origin_id = ?').run(error.reason, row.origin_id);
        setSelectedHead(db, row.origin_id, selected, row.materialized_revision, row.materialized_hash);
        reportConflict(db, row, heads, selected, now, result);
      }
    }
  }
  result.withheldOnApply = pending.size;
  for (const work of workRows) {
    const current = readOrigin(db, work.origin_id)!;
    if (current.selected_head === null) continue;
    if (!advanceWorkCheckpoint(db, current, current.selected_head)) {
      // A checkpoint the chain does not reach is a sibling for the pointer: keep the previous line.
      const kept = current.materialized_revision;
      setSelectedHead(db, current.origin_id, kept, kept, current.materialized_hash);
      const heads = headsOf(db, current.origin_id);
      reportConflict(db, current, heads.length > 1 ? heads : [...new Set([current.selected_head, kept ?? current.selected_head])], kept, now, result);
    }
  }
  captureLocalChanges(db, now);
  return result;
}

/**
 * Applies a staged bundle inside the caller's `BEGIN IMMEDIATE`. Throws `BundleRejected` when a
 * row would exceed the head bound; the caller rolls back.
 */
export function applyStaged(db: DatabaseSync, staged: Staged, input: { senderOriginId: string; now: number }): ApplyResult {
  const replica = replicaOriginId(db);
  captureLocalChanges(db, input.now);
  applyRepoLines(db, staged, replica, input.now);
  const stored: ApplyResult = { stored: 0, filled: 0, materialized: 0, withheldOnApply: 0, conflicts: 0 };
  const touched = storeStaged(db, staged, input.senderOriginId, input.now, stored);
  const result = materializeRows(db, [...touched], input.now);
  return { ...result, stored: stored.stored, filled: stored.filled };
}

export class ResolveError extends Error {
  constructor(readonly code: string) { super(code); this.name = 'ResolveError'; }
}

/**
 * `oboete sync resolve <origin> --keep <revision | checkpoint memory origin>`: one successor with
 * every current head as parent whose content is the kept head (or, for a work, the current
 * payload pointing at the kept checkpoint); closes the row's conflict rows. Runs inside the
 * caller's `BEGIN IMMEDIATE`.
 */
export function resolveRow(db: DatabaseSync, originId: string, keep: string, now: number): { revision_id: string } {
  const replica = replicaOriginId(db);
  captureLocalChanges(db, now);
  if (readOrigin(db, originId) === undefined) throw new ResolveError('unknown_origin');
  const row = canonicalOf(db, originId);
  const heads = headsOf(db, row.origin_id);
  const control = effectiveControl(db, row.origin_id);
  let payload: Row | null;
  let payloadHashValue: string | null;
  const kept = readRevision(db, keep);
  if (kept !== undefined && heads.includes(keep)) {
    payload = kept.payload;
    payloadHashValue = kept.payload_hash;
  } else if (row.kind === 'work' && row.local_id !== null) {
    const checkpoint = readOrigin(db, keep);
    const checkpointLocal = checkpoint?.kind === 'memory' ? canonicalOf(db, keep).local_id : null;
    if (checkpointLocal === null || prepared(db, 'SELECT 1 FROM memories WHERE id = ? AND work_id = ? AND deleted_at IS NULL')
      .get(checkpointLocal, row.local_id) === undefined) throw new ResolveError('unknown_head');
    prepared(db, 'UPDATE work_items SET current_checkpoint_memory_id = ?, updated_at = ? WHERE id = ?').run(checkpointLocal, now, row.local_id);
    const repoKeys = registerLocalRepos(db, replica);
    const originOf = (kind: SyncKind, id: string): string => originsOfRow(db, kind, id)[0]?.canonical_origin_id ?? `${replica}:${id}`;
    const record = localRecord(db, 'work', row.local_id, { replica, localOf: () => '', repo: () => '' })!;
    payload = toOriginForm('work', record, row.origin_id, { originOf, repoKey: (id) => repoKeys.get(id)! });
    payloadHashValue = payloadHash(payload);
  } else throw new ResolveError('unknown_head');
  const revision: Revision = {
    revision_id: '', origin_id: row.origin_id, kind: row.kind, author: replica, parents: heads, control,
    natural: row.natural, payload_hash: payloadHashValue, payload,
  };
  revision.revision_id = revisionId(revision);
  storeRevision(db, revision, null, now);
  const resolve = resolverFor(db, replica);
  const localId = WRITERS[row.kind](db, readOrigin(db, row.origin_id)!, payload, control, resolve, now);
  const after = readOrigin(db, row.origin_id)!;
  const state = localId === null ? stateHash(payloadHashValue, control) : materializedState(db, after, localId, resolve) ?? stateHash(payloadHashValue, control);
  setSelectedHead(db, row.origin_id, revision.revision_id, revision.revision_id, state);
  prepared(db, "UPDATE sync_conflicts SET status = 'resolved' WHERE id = ? AND status = 'open'").run(`sync:${row.origin_id}`);
  if (row.kind === 'work' && row.local_id !== null) {
    prepared(db, `UPDATE sync_conflicts SET status = 'resolved' WHERE status = 'open' AND id NOT LIKE 'sync:%'
      AND json_extract(local_state_json, '$.work_id') = ?`).run(row.local_id);
  }
  return { revision_id: revision.revision_id };
}
