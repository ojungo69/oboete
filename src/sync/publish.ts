// Snapshot plaintext (contracts/sync.md "What a replica publishes", "Plaintext"): every stored
// revision as an identity line, heads flagged, and a head's payload only when the content filter
// and the reference closure allow it. Security-owned: this is where secret, quarantined and
// unselected text is kept off the wire; control revisions always travel.
import { createHash } from 'node:crypto';
import { closeSync, openSync, readSync, rmSync, writeSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';

import { prepared } from '../db/statements.js';
import { compareCodeUnits } from '../hash.js';
import { BOUNDS, ENTITY_REFERENCES, type Control } from './format.js';
import { canonicalJson, snapshotId, SNAPSHOT_FORMAT, type Sensitivity, type SyncKind } from './identity.js';
import { effectiveControl, headsOf, readOrigin, replicaOriginId, revisionFromRow, type Origin, type Revision, type Row } from './store.js';

const RANK: Record<Sensitivity, number> = { eligible: 0, local_only: 1, private: 2, secret: 3 };

export type Withheld = { works: number; memories: number; sources: number; contexts: number; proposals: number };
export type PublishOptions = { spaceId: string; classes: readonly Sensitivity[]; now: number; outputPath: string };
export type PublishResult = { snapshotId: string; revisionLines: number; heads: number; withheld: Withheld; bytes: number };

export class PublishError extends Error {
  constructor(readonly code: string) { super(code); this.name = 'PublishError'; }
}

type Head = { revision: Revision; row: Origin; kind: SyncKind };

function raised(value: unknown, control: Control): Sensitivity {
  const own = value as Sensitivity;
  return RANK[control.sensitivity_floor] > RANK[own] ? control.sensitivity_floor : own;
}

function unfinishedSources(db: DatabaseSync, memoryLocalId: string): boolean {
  return prepared(db, `SELECT 1 FROM memory_sources ms JOIN raw_events e ON e.id = ms.raw_event_id
    WHERE ms.memory_id = ? AND (e.classification_state <> 'done' OR e.processing_state NOT IN ('processed', 'excluded')) LIMIT 1`)
    .get(memoryLocalId) !== undefined;
}

/** The content filter's own class rule per kind; sources, grants and contexts follow the closure. */
function passesClassRule(db: DatabaseSync, head: Head, classes: readonly Sensitivity[], control: Control): boolean {
  const payload = head.revision.payload;
  if (payload === null || control.tombstone) return false;
  switch (head.kind) {
    case 'memory':
      return payload.deleted_at === null && payload.review_state !== 'imported'
        && classes.includes(raised(payload.sensitivity, control))
        && (head.row.local_id === null || !unfinishedSources(db, head.row.local_id));
    case 'work': return classes.includes(raised(payload.purpose_sensitivity, control));
    case 'sharing_proposal': return classes.includes(raised(payload.candidate_sensitivity, control));
    default: return true;
  }
}

/** Writes the plaintext for this replica's snapshot and returns what the header records. */
export function buildSnapshot(db: DatabaseSync, options: PublishOptions): PublishResult {
  const replica = replicaOriginId(db);
  if (options.classes.includes('secret')) throw new PublishError('secret_never_selectable');
  const rows = prepared(db, 'SELECT * FROM sync_origins WHERE origin_id = canonical_origin_id ORDER BY origin_id').all()
    .map((row) => readOrigin(db, String(row.origin_id))!);
  const controls = new Map<string, Control>();
  const heads: Head[] = [];
  const headIds = new Set<string>();
  for (const row of rows) {
    const control = effectiveControl(db, row.origin_id);
    controls.set(row.origin_id, control);
    const ids = headsOf(db, row.origin_id);
    if (ids.length > BOUNDS.headsPerOrigin) throw new PublishError('heads_per_origin');
    for (const id of ids) {
      const revision = revisionFromRow(prepared(db, 'SELECT * FROM sync_revisions WHERE revision_id = ?').get(id)!);
      heads.push({ revision, row, kind: row.kind });
      headIds.add(id);
    }
  }
  const canonicalOf = (originId: string): string | undefined => readOrigin(db, originId)?.canonical_origin_id;

  // (1) candidates by their own class rule, (2) contexts by reference or repository, (3) closure.
  const candidates = new Set<string>();
  const byRow = new Map<string, Head[]>();
  for (const head of heads) {
    const list = byRow.get(head.row.origin_id) ?? [];
    list.push(head);
    byRow.set(head.row.origin_id, list);
    if (head.kind !== 'context' && passesClassRule(db, head, options.classes, controls.get(head.row.origin_id)!)) candidates.add(head.revision.revision_id);
  }
  const candidateRepos = new Set<string>();
  const referencedContexts = new Set<string>();
  for (const head of heads) {
    if (!candidates.has(head.revision.revision_id)) continue;
    const payload = head.revision.payload!;
    for (const { field, kind } of ENTITY_REFERENCES[head.kind]) {
      const value = payload[field];
      if (value === null || value === undefined) continue;
      if (kind === 'repo') candidateRepos.add(String(value));
      if (kind === 'context') referencedContexts.add(canonicalOf(String(value)) ?? String(value));
    }
  }
  for (const head of heads) {
    if (head.kind !== 'context' || head.revision.payload === null) continue;
    if (referencedContexts.has(head.row.origin_id) || candidateRepos.has(String(head.revision.payload.repo_id))) {
      candidates.add(head.revision.revision_id);
    }
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const head of heads) {
      if (!candidates.has(head.revision.revision_id)) continue;
      const payload = head.revision.payload!;
      for (const { field, kind } of ENTITY_REFERENCES[head.kind]) {
        const value = payload[field];
        if (value === null || value === undefined || kind === 'repo') continue;
        const target = canonicalOf(String(value));
        const shipped = target !== undefined && (byRow.get(target) ?? []).some((other) => candidates.has(other.revision.revision_id));
        if (!shipped) { candidates.delete(head.revision.revision_id); changed = true; break; }
      }
    }
  }

  // Repo lines for every repository a shipped payload references.
  const repoKeys = new Set<string>();
  for (const head of heads) {
    if (!candidates.has(head.revision.revision_id)) continue;
    for (const { field, kind } of ENTITY_REFERENCES[head.kind]) {
      const value = head.revision.payload![field];
      if (kind === 'repo' && value !== null && value !== undefined) repoKeys.add(String(value));
    }
  }
  const withheld: Withheld = { works: 0, memories: 0, sources: 0, contexts: 0, proposals: 0 };
  const withheldKey: Partial<Record<SyncKind, keyof Withheld>> = {
    work: 'works', memory: 'memories', source: 'sources', context: 'contexts', sharing_proposal: 'proposals',
  };
  for (const head of heads) {
    if (head.revision.payload !== null && !candidates.has(head.revision.revision_id)) {
      const key = withheldKey[head.kind];
      if (key !== undefined) withheld[key] += 1;
    }
  }

  const body = openSync(`${options.outputPath}.body`, 'w', 0o600);
  const digest = createHash('sha256');
  let bytes = 0;
  let revisionLines = 0;
  const emit = (line: Row): void => {
    const text = `${canonicalJson(line)}\n`;
    const buffer = Buffer.from(text, 'utf8');
    if (buffer.length > BOUNDS.lineBytes) throw new PublishError('line_too_long');
    writeSync(body, buffer);
    digest.update(buffer);
    bytes += buffer.length;
  };
  try {
    for (const key of [...repoKeys].sort(compareCodeUnits)) {
      const repo = prepared(db, 'SELECT identity_kind, normalized_identity FROM sync_repo_mappings WHERE repo_key = ?').get(key);
      if (repo === undefined) throw new PublishError('unknown_repo_key');
      emit({ kind: 'repo', origin_id: key, identity_kind: repo.identity_kind, normalized_identity: repo.normalized_identity });
    }
    for (const row of prepared(db, 'SELECT * FROM sync_revisions ORDER BY origin_id, revision_id').iterate()) {
      const revision = revisionFromRow(row);
      const head = headIds.has(revision.revision_id);
      revisionLines += 1;
      if (revisionLines > BOUNDS.revisionLines) throw new PublishError('too_many_lines');
      emit({
        origin_id: revision.origin_id, kind: revision.kind, revision_id: revision.revision_id, author: revision.author,
        parents: revision.parents, control: revision.control, natural: revision.natural, payload_hash: revision.payload_hash,
        head, payload: head && candidates.has(revision.revision_id) ? revision.payload : null,
      });
    }
  } finally { closeSync(body); }
  const revisionsSha256 = digest.digest('hex');
  const snapshot = snapshotId(options.spaceId, replica, revisionsSha256);
  const header = `${canonicalJson({
    format: SNAPSHOT_FORMAT, space_id: options.spaceId, replica_origin_id: replica, snapshot_id: snapshot,
    revision_lines: revisionLines, heads: headIds.size, revisions_sha256: revisionsSha256, withheld, produced_at: options.now,
  })}\n`;
  if (Buffer.byteLength(header) > BOUNDS.headerBytes) throw new PublishError('header_too_long');
  concatenate(options.outputPath, header, `${options.outputPath}.body`);
  return { snapshotId: snapshot, revisionLines, heads: headIds.size, withheld, bytes: bytes + Buffer.byteLength(header) };
}

function concatenate(target: string, header: string, bodyPath: string): void {
  const output = openSync(target, 'w', 0o600);
  try {
    writeSync(output, header);
    // The body was just written by this process; a streamed copy keeps RSS flat for large stores.
    const input = openSync(bodyPath, 'r');
    try {
      const chunk = Buffer.alloc(1 << 20);
      for (;;) {
        const read = readSync(input, chunk, 0, chunk.length, null);
        if (read === 0) break;
        writeSync(output, chunk, 0, read);
      }
    } finally { closeSync(input); }
  } finally { closeSync(output); }
  rmSync(bodyPath, { force: true });
}
