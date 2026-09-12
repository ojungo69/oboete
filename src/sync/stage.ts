// Bundle plaintext validation (contracts/sync.md "Pull" steps 3–5): the header, every line's
// schema, recomputed ids and hashes, parents, entity references and bounds, staged in a private
// scratch SQLite exactly as the native reader stages input. Nothing here writes to the store; a
// bundle with any invalid line is rejected whole. Security-owned.
import { createHash } from 'node:crypto';
import { closeSync, openSync, readSync, rmSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';

import { materialHash } from '../db/identity.js';
import { loadSqlite } from '../db/open.js';
import { prepared } from '../db/statements.js';
import { sha256Json } from '../hash.js';
import {
  BOUNDS, headerSchema, naturalSchemas, payloadSchemas, repoLineSchema, revisionLineSchema, ENTITY_REFERENCES,
  type Header, type RevisionLine, type RepoLine,
} from './format.js';
import { naturalOf } from './capture.js';
import { canonicalJson, payloadHash, revisionId, snapshotId, type SyncKind } from './identity.js';
import { readOrigin, readRevision, type Row } from './store.js';

export class BundleRejected extends Error {
  constructor(readonly code: string, readonly detail: string | null = null) {
    super(detail === null ? code : `${code}: ${detail}`);
    this.name = 'BundleRejected';
  }
}

export type Staged = {
  header: Header;
  scratch: DatabaseSync;
  scratchPath: string;
  close(): void;
};

function* lines(path: string): Generator<Buffer> {
  const fd = openSync(path, 'r');
  try {
    const chunk = Buffer.alloc(1 << 20);
    let pending: Buffer[] = [];
    let pendingBytes = 0;
    for (;;) {
      const read = readSync(fd, chunk, 0, chunk.length, null);
      if (read === 0) break;
      let start = 0;
      for (let i = 0; i < read; i += 1) {
        if (chunk[i] !== 0x0a) continue;
        const piece = chunk.subarray(start, i);
        if (pendingBytes + piece.length > BOUNDS.lineBytes) throw new BundleRejected('line_too_long');
        yield pending.length === 0 ? Buffer.from(piece) : Buffer.concat([...pending, piece]);
        pending = []; pendingBytes = 0; start = i + 1;
      }
      if (start < read) {
        const rest = Buffer.from(chunk.subarray(start, read));
        pendingBytes += rest.length;
        if (pendingBytes > BOUNDS.lineBytes) throw new BundleRejected('line_too_long');
        pending.push(rest);
      }
    }
    if (pending.length > 0) throw new BundleRejected('unterminated_line');
  } finally { closeSync(fd); }
}

function parseJson(bytes: Buffer, code: string): unknown {
  try { return JSON.parse(bytes.toString('utf8')); } catch { throw new BundleRejected(code); }
}

/**
 * Reads and validates one decrypted bundle. `db` is only read (stored revisions and origins
 * satisfy parent and reference lookups); the caller applies the staged lines later.
 */
export function stageBundle(
  db: DatabaseSync,
  input: { plaintextPath: string; scratchPath: string; spaceId: string; senderOriginId: string },
): Staged {
  const reader = lines(input.plaintextPath);
  const first = reader.next();
  if (first.done) throw new BundleRejected('empty_bundle');
  if (first.value.length > BOUNDS.headerBytes) throw new BundleRejected('header_too_long');
  const headerParsed = headerSchema.safeParse(parseJson(first.value, 'header_not_json'));
  if (!headerParsed.success) throw new BundleRejected('invalid_header', headerParsed.error.issues[0]?.message ?? null);
  const header = headerParsed.data;
  if (header.replica_origin_id !== input.senderOriginId) throw new BundleRejected('replica_mismatch');
  if (header.space_id !== input.spaceId) throw new BundleRejected('space_mismatch');
  if (header.snapshot_id !== snapshotId(header.space_id, header.replica_origin_id, header.revisions_sha256)) throw new BundleRejected('snapshot_id_mismatch');

  rmSync(input.scratchPath, { force: true });
  const scratch = new (loadSqlite().DatabaseSync)(input.scratchPath);
  scratch.exec(`PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA temp_store = FILE;
    CREATE TABLE lines (revision_id TEXT PRIMARY KEY, origin_id TEXT NOT NULL, kind TEXT NOT NULL, head INTEGER NOT NULL,
      line_json TEXT NOT NULL) STRICT;
    CREATE INDEX lines_origin ON lines (origin_id);
    CREATE TABLE parents (child TEXT NOT NULL, parent TEXT NOT NULL, PRIMARY KEY (child, parent)) STRICT;
    CREATE TABLE origins (origin_id TEXT PRIMARY KEY, kind TEXT NOT NULL, natural_json TEXT NOT NULL) STRICT;
    CREATE TABLE repos (repo_key TEXT PRIMARY KEY, identity_kind TEXT NOT NULL, normalized_identity TEXT NOT NULL) STRICT;
    CREATE TABLE refs (revision_id TEXT NOT NULL, field TEXT NOT NULL, target_kind TEXT NOT NULL, target TEXT NOT NULL) STRICT;`);
  const staged: Staged = {
    header, scratch, scratchPath: input.scratchPath,
    close() { if (scratch.isOpen) scratch.close(); rmSync(input.scratchPath, { force: true }); },
  };
  try {
    validateBody(db, staged, reader);
    return staged;
  } catch (error) {
    staged.close();
    throw error;
  }
}

function validateBody(db: DatabaseSync, staged: Staged, reader: Generator<Buffer>): void {
  const { header, scratch } = staged;
  const digest = createHash('sha256');
  let repoLines = 0;
  let revisionLines = 0;
  let heads = 0;
  const insertLine = scratch.prepare('INSERT INTO lines (revision_id, origin_id, kind, head, line_json) VALUES (?, ?, ?, ?, ?)');
  const insertParent = scratch.prepare('INSERT INTO parents (child, parent) VALUES (?, ?)');
  const insertOrigin = scratch.prepare('INSERT OR IGNORE INTO origins (origin_id, kind, natural_json) VALUES (?, ?, ?)');
  const insertRepo = scratch.prepare('INSERT INTO repos (repo_key, identity_kind, normalized_identity) VALUES (?, ?, ?)');
  const insertRef = scratch.prepare('INSERT INTO refs (revision_id, field, target_kind, target) VALUES (?, ?, ?, ?)');
  for (const bytes of reader) {
    digest.update(bytes);
    digest.update('\n');
    const value = parseJson(bytes, 'line_not_json');
    if (value !== null && typeof value === 'object' && (value as Row).kind === 'repo') {
      // Repo lines carry their own cap: the header only declares `revision_lines`, so without one a
      // bundle could fill the plaintext with repo records that no bound counts.
      repoLines += 1;
      if (repoLines > BOUNDS.repoLines) throw new BundleRejected('repo_lines_exceeded');
      const repo = repoLineSchema.safeParse(value);
      if (!repo.success) throw new BundleRejected('invalid_repo_line', repo.error.issues[0]?.message ?? null);
      try { insertRepo.run(repo.data.origin_id, repo.data.identity_kind, repo.data.normalized_identity); }
      catch { throw new BundleRejected('duplicate_repo_line', repo.data.origin_id); }
      continue;
    }
    revisionLines += 1;
    if (revisionLines > header.revision_lines) throw new BundleRejected('revision_lines_exceeded');
    const parsed = revisionLineSchema.safeParse(value);
    if (!parsed.success) throw new BundleRejected('invalid_revision_line', parsed.error.issues[0]?.message ?? null);
    const line: RevisionLine = parsed.data;
    const natural = naturalSchemas[line.kind].safeParse(line.natural);
    if (!natural.success) throw new BundleRejected('invalid_natural', line.revision_id);
    if (line.revision_id !== revisionId(line)) throw new BundleRejected('revision_id_mismatch', line.revision_id);
    if (line.payload !== null) {
      if (payloadHash(line.payload) !== line.payload_hash) throw new BundleRejected('payload_hash_mismatch', line.revision_id);
      const payload = payloadSchemas[line.kind].safeParse(line.payload);
      if (!payload.success) throw new BundleRejected('invalid_payload', `${line.revision_id}: ${payload.error.issues[0]?.message ?? ''}`);
      if (line.payload.id !== line.origin_id) throw new BundleRejected('payload_id_mismatch', line.revision_id);
      checkPayloadIntegrity(line);
      for (const { field, kind } of ENTITY_REFERENCES[line.kind]) {
        const target = line.payload[field];
        if (target !== null && target !== undefined) insertRef.run(line.revision_id, field, kind, String(target));
      }
    }
    try { insertLine.run(line.revision_id, line.origin_id, line.kind, line.head ? 1 : 0, bytes.toString('utf8')); }
    catch { throw new BundleRejected('duplicate_revision', line.revision_id); }
    insertOrigin.run(line.origin_id, line.kind, JSON.stringify(line.natural));
    // A source's natural key names its memory: that reference must resolve even for a control-only line.
    if (line.kind === 'source') insertRef.run(line.revision_id, 'memory', 'memory', String(line.natural.memory));
    for (const parent of line.parents) insertParent.run(line.revision_id, parent);
    if (line.head) heads += 1;
  }
  if (revisionLines !== header.revision_lines) throw new BundleRejected('revision_lines_mismatch');
  if (heads !== header.heads) throw new BundleRejected('heads_mismatch');
  if (digest.digest('hex') !== header.revisions_sha256) throw new BundleRejected('revisions_sha256_mismatch');

  // One origin, one kind, one natural key; an origin of another kind elsewhere is a shape error.
  for (const row of scratch.prepare('SELECT origin_id, kind, natural_json FROM origins').iterate()) {
    const conflicting = scratch.prepare('SELECT 1 FROM lines WHERE origin_id = ? AND (kind <> ? OR json(json_extract(line_json, \'$.natural\')) <> json(?)) LIMIT 1')
      .get(row.origin_id, row.kind, row.natural_json);
    if (conflicting !== undefined) throw new BundleRejected('origin_identity_conflict', String(row.origin_id));
    const stored = readOrigin(db, String(row.origin_id));
    if (stored !== undefined && stored.kind !== row.kind) throw new BundleRejected('origin_kind_conflict', String(row.origin_id));
    // An origin's natural key is its immutable identity (never rewritten once stored). A bundle
    // that reuses a stored origin id with a different natural would bind a new payload to the old
    // local row without updating its material/content identity, so reject the change.
    if (stored !== undefined && canonicalJson(stored.natural) !== canonicalJson(JSON.parse(String(row.natural_json)))) {
      throw new BundleRejected('origin_natural_conflict', String(row.origin_id));
    }
    // Every snapshot re-sends the whole log, so a line already stored is not a new revision.
    let count = Number(scratch.prepare('SELECT COUNT(*) AS n FROM lines WHERE origin_id = ?').get(row.origin_id)?.n ?? 0)
      + Number(prepared(db, 'SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ?').get(row.origin_id)?.n ?? 0);
    if (count > BOUNDS.revisionsPerOrigin) {
      for (const stored of scratch.prepare('SELECT revision_id FROM lines WHERE origin_id = ?').iterate(row.origin_id)) {
        if (readRevision(db, String(stored.revision_id)) !== undefined) count -= 1;
      }
    }
    if (count > BOUNDS.revisionsPerOrigin) throw new BundleRejected('revisions_per_origin', String(row.origin_id));
  }
  // Every parent names a line here or a stored revision of the same kind.
  for (const row of scratch.prepare(`SELECT p.child, p.parent, l.kind FROM parents p JOIN lines l ON l.revision_id = p.child
    WHERE NOT EXISTS (SELECT 1 FROM lines x WHERE x.revision_id = p.parent AND x.kind = l.kind)`).iterate()) {
    const stored = readRevision(db, String(row.parent));
    if (stored === undefined || stored.kind !== row.kind) throw new BundleRejected('unresolvable_parent', String(row.child));
  }
  // Every entity reference names an origin in the bundle or one known locally; every repo key a
  // repo line or a known mapping.
  for (const row of scratch.prepare('SELECT DISTINCT target_kind, target FROM refs').iterate()) {
    const target = String(row.target);
    if (row.target_kind === 'repo') {
      if (scratch.prepare('SELECT 1 FROM repos WHERE repo_key = ?').get(target) === undefined
        && prepared(db, 'SELECT 1 FROM sync_repo_mappings WHERE repo_key = ?').get(target) === undefined) {
        throw new BundleRejected('unknown_repo_key', target);
      }
      continue;
    }
    const inBundle = scratch.prepare('SELECT kind FROM origins WHERE origin_id = ?').get(target);
    const kind = inBundle === undefined ? readOrigin(db, target)?.kind : (inBundle.kind as SyncKind);
    if (kind === undefined) throw new BundleRejected('unknown_reference', target);
    if (kind !== row.target_kind) throw new BundleRejected('reference_kind_mismatch', target);
  }
}

/**
 * The payload must be the record its natural key and hashes describe, exactly as the native
 * reader checks: redacted memories carry no text, a text-bearing memory or candidate hashes to
 * its material hash, and the natural key is the one the payload derives (so a line cannot alias
 * onto another row's identity while carrying other content).
 */
function checkPayloadIntegrity(line: RevisionLine): void {
  const payload = line.payload!;
  const reject = (code: string): never => { throw new BundleRejected(code, line.revision_id); };
  if (line.kind === 'memory') {
    const absentText = payload.deleted_at !== null || payload.sensitivity === 'secret' || line.control.tombstone || line.control.sensitivity_floor === 'secret';
    if (absentText) {
      if (payload.title !== '' || payload.body !== '' || (payload.concepts ?? '[]') !== '[]') reject('redacted_memory_text');
    } else if (materialHash(String(payload.title ?? ''), String(payload.body ?? '')) !== payload.material_hash) reject('material_hash_mismatch');
    // A projection's text is its identity, whether the payload or the origin's natural key says so.
    const projection = payload.identity_domain === 'personal_projection' ? payload.content_hash
      : line.natural.domain === 'personal_projection' ? line.natural.projection_hash : null;
    if (projection !== null) {
      if (payload.work_id !== null || payload.checkpoint_parent_id !== null) reject('personal_source_lineage');
      if (!absentText && sha256Json(['personal-projection-v1', payload.title, payload.body]) !== projection) reject('personal_identity_mismatch');
    }
  }
  // A dependency edge is a context-only edge to another memory (the 0005 CHECK).
  if (line.kind === 'source' && payload.source_memory_id !== null && (payload.context_only !== 1 || payload.source_memory_id === payload.memory_id)) {
    reject('invalid_source_edge');
  }
  if (line.kind === 'sharing_proposal' && payload.redacted !== true && payload.candidate_sensitivity !== 'secret'
    && line.control.sensitivity_floor !== 'secret'
    && materialHash(String(payload.candidate_title), String(payload.candidate_body)) !== payload.candidate_material_hash) reject('candidate_hash_mismatch');
  // The payload is already in origin form, so the sender's derivation applies unchanged.
  const derived = naturalOf(line.kind, payload, line.origin_id, { originOf: (_kind, id) => id, repoKey: (id) => id });
  // A projection released to an ordinary row keeps its origin: its identity is still the content hash.
  const released = line.kind === 'memory' && line.natural.domain === 'personal_projection' && derived.domain !== 'personal_projection';
  if (released ? payload.content_hash !== line.natural.projection_hash : canonicalJson(derived) !== canonicalJson(line.natural)) reject('natural_mismatch');
}

export function stagedLines(staged: Staged, originId: string): RevisionLine[] {
  return staged.scratch.prepare('SELECT line_json FROM lines WHERE origin_id = ? ORDER BY revision_id').all(originId)
    .map((row) => revisionLineSchema.parse(JSON.parse(String(row.line_json))));
}

/** SQL string literal for one of our own kind names: the ORDER BY below is built, not bound. */
function quoteLiteral(value: string): string {
  if (!/^[a-z_]+$/u.test(value)) throw new Error(`unexpected kind ${value}`);
  return `'${value}'`;
}

/**
 * The staged origins, streamed in `kinds` order and then by origin id. A near-limit bundle holds
 * hundreds of thousands of them, and the scratch table is the disk-backed copy that exists so the
 * apply pass never has to hold them all: reading them with `.all()` and sorting in memory would
 * defeat that. A kind `kinds` does not name sorts first, as `Array#indexOf` returning -1 did.
 */
export function* stagedOrigins(staged: Staged, kinds: readonly SyncKind[]): Generator<{ origin_id: string; kind: SyncKind; natural: Row }> {
  const rank = kinds.map((kind, index) => `WHEN ${quoteLiteral(kind)} THEN ${String(index)}`).join(' ');
  for (const row of staged.scratch.prepare(`SELECT origin_id, kind, natural_json FROM origins
    ORDER BY CASE kind ${rank} ELSE -1 END, origin_id`).iterate()) {
    yield { origin_id: String(row.origin_id), kind: row.kind as SyncKind, natural: JSON.parse(String(row.natural_json)) as Row };
  }
}

export function stagedRepos(staged: Staged): RepoLine[] {
  return staged.scratch.prepare('SELECT repo_key AS origin_id, identity_kind, normalized_identity FROM repos ORDER BY repo_key').all()
    .map((row) => ({ kind: 'repo' as const, origin_id: String(row.origin_id), identity_kind: row.identity_kind as 'remote' | 'common_dir',
      normalized_identity: String(row.normalized_identity) }));
}
