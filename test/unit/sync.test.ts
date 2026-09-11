// US6 device sync (contracts/sync.md). Identity first: the ids that every replica must compute
// identically are pinned before any envelope or merge code exists.
import assert from 'node:assert/strict';
import { createDecipheriv, hkdfSync, randomBytes } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import type { DatabaseSync } from 'node:sqlite';

import { openDatabase } from '../../src/db/open.js';
import { grantVisibility } from '../../src/db/queries.js';
import { sha256Hex } from '../../src/hash.js';
import { oboetePaths } from '../../src/paths.js';
import { captureLocalChanges } from '../../src/sync/capture.js';
import { BundleError, CHUNK_BYTES, decryptBundle, encryptBundle, keyId, PREFIX_BYTES, TAG_BYTES } from '../../src/sync/envelope.js';
import { canonicalJson, payloadHash, revisionId, snapshotId } from '../../src/sync/identity.js';
import {
  effectiveControl, headsOf, readOrigin, readRevision, replicaOriginId, revisionsOfOrigin, type Revision,
} from '../../src/sync/store.js';
import { withTempHome } from '../helpers/home.js';

const REPLICA_A = 'a'.repeat(32);
const base = {
  origin_id: `${REPLICA_A}:m_0123456789abcdef01234567`,
  kind: 'memory' as const,
  author: REPLICA_A,
  parents: ['1'.repeat(64), '2'.repeat(64)],
  control: { tombstone: false, sensitivity_floor: 'local_only' as const },
  natural: { domain: 'ordinary', repo: 'remote:' + '3'.repeat(64), material_hash: '4'.repeat(64) },
  payload_hash: '5'.repeat(64),
};

test('canonical JSON sorts object members at every depth and keeps array order', () => {
  assert.equal(canonicalJson({ b: [{ z: 1, a: null }], a: 'x' }), '{"a":"x","b":[{"a":null,"z":1}]}');
});

test('revision_id ignores parent order and member order but not control or payload', () => {
  const id = revisionId(base);
  assert.match(id, /^[0-9a-f]{64}$/u);
  assert.equal(revisionId({ ...base, parents: [base.parents[1]!, base.parents[0]!] }), id);
  assert.equal(revisionId({ ...base, control: { sensitivity_floor: 'local_only', tombstone: false } }), id);
  assert.notEqual(revisionId({ ...base, control: { tombstone: true, sensitivity_floor: 'local_only' } }), id);
  assert.notEqual(revisionId({ ...base, control: { tombstone: false, sensitivity_floor: 'private' } }), id);
  assert.notEqual(revisionId({ ...base, payload_hash: null }), id);
  assert.notEqual(revisionId({ ...base, author: 'b'.repeat(32) }), id);
  assert.notEqual(revisionId({ ...base, parents: [base.parents[0]!] }), id);
});

test('payload_hash covers the canonical payload, so member order cannot change it', () => {
  assert.equal(payloadHash({ id: 'x', kind: 'context', repo_id: 'r' }), payloadHash({ repo_id: 'r', kind: 'context', id: 'x' }));
  assert.notEqual(payloadHash({ id: 'x' }), payloadHash({ id: 'y' }));
});

test('snapshot_id identifies the exact delivery of one replica in one space', () => {
  const space = 'c'.repeat(32);
  const sha = '6'.repeat(64);
  const id = snapshotId(space, REPLICA_A, sha);
  assert.match(id, /^[0-9a-f]{64}$/u);
  assert.equal(snapshotId(space, REPLICA_A, sha), id);
  assert.notEqual(snapshotId(space, 'b'.repeat(32), sha), id);
  assert.notEqual(snapshotId(space, REPLICA_A, '7'.repeat(64)), id);
});

// --- Envelope: oboete-sync-bundle/1 (age STREAM framing over AES-256-GCM) ---

function withDir(fn: (dir: string) => void): void {
  const dir = mkdtempSync(join(tmpdir(), 'oboete-sync-'));
  try { fn(dir); } finally { rmSync(dir, { recursive: true, force: true }); }
}

function roundTrip(dir: string, key: Buffer, plaintext: Buffer, name = 'a'): { cipher: string; plain: string } {
  const plain = join(dir, `${name}.plain`);
  const cipher = join(dir, `${name}.osb`);
  writeFileSync(plain, plaintext);
  encryptBundle(key, plain, cipher);
  return { cipher, plain };
}

function rejects(key: Buffer, cipher: string, out: string, code: string): void {
  assert.throws(() => decryptBundle(key, cipher, out), (error: unknown) => error instanceof BundleError && error.code === code,
    `expected ${code}`);
}

test('key id is an 8-byte HKDF output that depends only on the space key', () => {
  const key = randomBytes(32);
  assert.match(keyId(key), /^[0-9a-f]{16}$/u);
  assert.equal(keyId(key), keyId(Buffer.from(key)));
  assert.notEqual(keyId(key), keyId(randomBytes(32)));
});

test('bundles round-trip at every framing boundary and the file size follows the framing', () => {
  withDir((dir) => {
    const key = randomBytes(32);
    for (const length of [0, 1, CHUNK_BYTES - 1, CHUNK_BYTES, CHUNK_BYTES + 1, 2 * CHUNK_BYTES, 2 * CHUNK_BYTES + 1]) {
      const plaintext = randomBytes(length);
      const { cipher } = roundTrip(dir, key, plaintext, `len${length}`);
      const chunks = Math.max(1, Math.ceil(length / CHUNK_BYTES));
      assert.equal(statSync(cipher).size, PREFIX_BYTES + length + TAG_BYTES * chunks, `size for ${length}`);
      const out = join(dir, `len${length}.out`);
      decryptBundle(key, cipher, out);
      assert.ok(readFileSync(out).equals(plaintext), `plaintext for ${length}`);
    }
  });
});

test('a rewritten bundle has a new salt and different ciphertext for the same plaintext', () => {
  withDir((dir) => {
    const key = randomBytes(32);
    const plaintext = randomBytes(100);
    const first = readFileSync(roundTrip(dir, key, plaintext, 'one').cipher);
    const second = readFileSync(roundTrip(dir, key, plaintext, 'two').cipher);
    assert.ok(!first.subarray(28, 44).equals(second.subarray(28, 44)));
    assert.ok(!first.subarray(44).equals(second.subarray(44)));
    assert.ok(first.subarray(0, 28).equals(second.subarray(0, 28)));
  });
});

test('every truncation, tamper, reorder, splice and wrong-key case rejects the whole bundle', () => {
  withDir((dir) => {
    const key = randomBytes(32);
    const plaintext = randomBytes(2 * CHUNK_BYTES + 7);
    const { cipher } = roundTrip(dir, key, plaintext);
    const bytes = readFileSync(cipher);
    const out = join(dir, 'out');
    const variant = (name: string, mutate: (copy: Buffer) => Buffer): string => {
      const path = join(dir, `${name}.osb`);
      writeFileSync(path, mutate(Buffer.from(bytes)));
      return path;
    };
    const unit = CHUNK_BYTES + TAG_BYTES;
    // Every chunk position: a flipped byte in the prefix, in each chunk body and in each tag.
    for (const offset of [0, 21, 30, PREFIX_BYTES, PREFIX_BYTES + CHUNK_BYTES, PREFIX_BYTES + unit + 5,
      PREFIX_BYTES + unit + CHUNK_BYTES + 3, PREFIX_BYTES + 2 * unit + 2, bytes.length - 1]) {
      const path = variant(`flip${offset}`, (copy) => { copy[offset]! ^= 0x01; return copy; });
      rejects(key, path, out, offset === 21 ? 'key_mismatch' : offset === 0 ? 'bad_magic' : 'authentication_failed');
    }
    rejects(key, variant('short-tail', (copy) => copy.subarray(0, copy.length - 1)), out, 'authentication_failed');
    rejects(key, variant('no-final', (copy) => copy.subarray(0, PREFIX_BYTES + 2 * unit)), out, 'authentication_failed');
    rejects(key, variant('one-chunk', (copy) => copy.subarray(0, PREFIX_BYTES + unit)), out, 'authentication_failed');
    rejects(key, variant('remainder', (copy) => copy.subarray(0, PREFIX_BYTES + 5)), out, 'truncated');
    rejects(key, variant('prefix-only', (copy) => copy.subarray(0, PREFIX_BYTES)), out, 'truncated');
    rejects(key, variant('swap', (copy) => Buffer.concat([copy.subarray(0, PREFIX_BYTES),
      copy.subarray(PREFIX_BYTES + unit, PREFIX_BYTES + 2 * unit), copy.subarray(PREFIX_BYTES, PREFIX_BYTES + unit),
      copy.subarray(PREFIX_BYTES + 2 * unit)])), out, 'authentication_failed');
    // A full-size final chunk from one bundle spliced as the last chunk of another (same key).
    const other = readFileSync(roundTrip(dir, key, randomBytes(CHUNK_BYTES), 'full-final').cipher);
    rejects(key, variant('splice', (copy) => Buffer.concat([copy.subarray(0, PREFIX_BYTES + 2 * unit), other.subarray(PREFIX_BYTES)])),
      out, 'authentication_failed');
    rejects(key, variant('drop-to-full-final', (copy) => Buffer.concat([copy.subarray(0, PREFIX_BYTES + unit)])), out, 'authentication_failed');
    rejects(randomBytes(32), cipher, out, 'key_mismatch');
    rejects(key, variant('oversize', (copy) => Buffer.concat([copy, Buffer.alloc(1)])), join(dir, 'x'), 'authentication_failed');
    assert.throws(() => decryptBundle(key, cipher, out, { maxCiphertextBytes: bytes.length - 1 }),
      (error: unknown) => error instanceof BundleError && error.code === 'oversize');
  });
});

test('an empty plaintext is one empty final chunk and nothing shorter decrypts', () => {
  withDir((dir) => {
    const key = randomBytes(32);
    const { cipher } = roundTrip(dir, key, Buffer.alloc(0), 'empty');
    assert.equal(statSync(cipher).size, PREFIX_BYTES + TAG_BYTES);
    const out = join(dir, 'out');
    decryptBundle(key, cipher, out);
    assert.equal(readFileSync(out).length, 0);
  });
});

test('every chunk binds the whole 44-byte prefix as AAD and derives its key from the salt', () => {
  withDir((dir) => {
    const key = randomBytes(32);
    const bytes = readFileSync(roundTrip(dir, key, Buffer.from('hello'), 'aad').cipher);
    const prefix = bytes.subarray(0, PREFIX_BYTES);
    const chunkKey = Buffer.from(hkdfSync('sha256', key, prefix.subarray(28, 44), 'oboete-sync-bundle/1', 32));
    const open = (aad: Buffer): Buffer => {
      const decipher = createDecipheriv('aes-256-gcm', chunkKey, Buffer.from('000000000000000000000001', 'hex'), { authTagLength: 16 });
      decipher.setAAD(aad);
      decipher.setAuthTag(bytes.subarray(bytes.length - TAG_BYTES));
      return Buffer.concat([decipher.update(bytes.subarray(PREFIX_BYTES, bytes.length - TAG_BYTES)), decipher.final()]);
    };
    assert.equal(open(prefix).toString(), 'hello');
    assert.throws(() => open(prefix.subarray(0, 20)));
    assert.throws(() => open(prefix.subarray(0, 28)));
  });
});

// --- Change capture: origins, revisions, heads (contracts/sync.md "Local change capture") ---

const REPO = 'r1b2c3d4e5f60718';
const REMOTE = 'github.com/example/sync';

async function withStore(fn: (db: DatabaseSync, replica: string) => void | Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      opened.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
        VALUES (?, 'remote', ?, '/work/sync', 1, 1)`).run(REPO, REMOTE);
      await fn(opened.db, replicaOriginId(opened.db));
    } finally { if (opened.db.isOpen) opened.db.close(); }
  });
}

function insertMemory(db: DatabaseSync, id: string, title: string, body: string,
  extra: { sensitivity?: string; deleted_at?: number | null; pinned_at?: number | null } = {}): { material: string; content: string } {
  const material = sha256Hex(JSON.stringify([title.toLowerCase(), body.toLowerCase()]));
  const content = sha256Hex(JSON.stringify([REPO, material]));
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity,
    review_state, created_at, deleted_at, pinned_at) VALUES (?, ?, 'discovery', ?, ?, '', ?, ?, ?, 'reviewed', 1, ?, ?)`)
    .run(id, REPO, title, body, material, content, extra.sensitivity ?? 'eligible', extra.deleted_at ?? null, extra.pinned_at ?? null);
  return { material, content };
}

function insertSource(db: DatabaseSync, memoryId: string, citation: string): number {
  db.prepare(`INSERT INTO memory_sources (memory_id, citation_kind, citation_value, source_agent, context_only)
    VALUES (?, 'file_read', ?, 'claude', 0)`).run(memoryId, citation);
  return Number(db.prepare('SELECT last_insert_rowid() AS id').get()?.id);
}

function revisions(db: DatabaseSync, originId: string): Revision[] {
  return revisionsOfOrigin(db, originId);
}

test('the change pass gives every row an origin and one revision, then records nothing on a second pass', async () => {
  await withStore((db, replica) => {
    insertMemory(db, 'm_one', 'Title', 'Body');
    insertSource(db, 'm_one', 'src/a.ts');
    db.exec('BEGIN IMMEDIATE');
    const first = captureLocalChanges(db, 10);
    db.exec('COMMIT');
    assert.deepEqual(first, { revisions: 2, tombstones: 0 });
    const memoryOrigin = `${replica}:m_one`;
    const stored = revisions(db, memoryOrigin);
    assert.equal(stored.length, 1);
    assert.deepEqual(stored[0]!.parents, []);
    assert.equal(stored[0]!.author, replica);
    assert.deepEqual(stored[0]!.control, { tombstone: false, sensitivity_floor: 'eligible' });
    assert.equal(stored[0]!.natural.domain, 'ordinary');
    assert.equal(stored[0]!.natural.repo, `remote:${sha256Hex(REMOTE)}`);
    assert.equal(stored[0]!.payload?.id, memoryOrigin);
    assert.equal(stored[0]!.payload?.repo_id, `remote:${sha256Hex(REMOTE)}`);
    assert.deepEqual(headsOf(db, memoryOrigin), [stored[0]!.revision_id]);
    assert.equal(readOrigin(db, memoryOrigin)?.selected_head, stored[0]!.revision_id);
    const source = db.prepare("SELECT origin_id, local_id FROM sync_origins WHERE kind = 'source'").get()!;
    assert.match(String(source.local_id), /^source:m_one:[0-9a-f]{64}$/u);
    assert.equal(String(source.origin_id), `${replica}:${String(source.local_id)}`);
    assert.equal(revisions(db, String(source.origin_id))[0]!.natural.memory, memoryOrigin);
    db.exec('BEGIN IMMEDIATE');
    assert.deepEqual(captureLocalChanges(db, 11), { revisions: 0, tombstones: 0 });
    db.exec('COMMIT');
  });
});

test('a pin is a successor of the materialized revision and a deletion is a tombstone control', async () => {
  await withStore((db, replica) => {
    insertMemory(db, 'm_one', 'Title', 'Body');
    db.exec('BEGIN IMMEDIATE'); captureLocalChanges(db, 10); db.exec('COMMIT');
    const origin = `${replica}:m_one`;
    const [initial] = revisions(db, origin);
    db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run('m_one');
    db.exec('BEGIN IMMEDIATE');
    assert.deepEqual(captureLocalChanges(db, 20), { revisions: 1, tombstones: 0 });
    db.exec('COMMIT');
    const pinned = revisions(db, origin).find((revision) => revision.parents.length === 1)!;
    assert.deepEqual(pinned.parents, [initial!.revision_id]);
    assert.equal(pinned.payload?.pinned_at, 5);
    assert.deepEqual(headsOf(db, origin), [pinned.revision_id]);
    db.prepare("UPDATE memories SET deleted_at = 30, sensitivity = 'secret' WHERE id = ?").run('m_one');
    db.exec('BEGIN IMMEDIATE'); captureLocalChanges(db, 30); db.exec('COMMIT');
    const [head] = headsOf(db, origin);
    const deleted = readRevision(db, head!)!;
    assert.deepEqual(deleted.control, { tombstone: true, sensitivity_floor: 'secret' });
    assert.deepEqual(deleted.parents, [pinned.revision_id]);
    assert.equal(deleted.payload?.title, '');
    assert.deepEqual(effectiveControl(db, origin), { tombstone: true, sensitivity_floor: 'secret' });
  });
});

test('a physically deleted source or grant becomes a tombstone revision once', async () => {
  await withStore((db, replica) => {
    insertMemory(db, 'm_one', 'Title', 'Body');
    const rowid = insertSource(db, 'm_one', 'src/a.ts');
    grantVisibility(db, 'm_one', { audience: 'project', repoId: REPO }, 'observer', 1);
    db.exec('BEGIN IMMEDIATE'); captureLocalChanges(db, 10); db.exec('COMMIT');
    const sourceOrigin = String(db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'source'").get()!.origin_id);
    const grantOrigin = String(db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'visibility'").get()!.origin_id);
    db.prepare('DELETE FROM memory_sources WHERE id = ?').run(rowid);
    db.prepare('DELETE FROM memory_visibility WHERE memory_id = ?').run('m_one');
    db.exec('BEGIN IMMEDIATE');
    assert.deepEqual(captureLocalChanges(db, 20), { revisions: 0, tombstones: 2 });
    db.exec('COMMIT');
    for (const origin of [sourceOrigin, grantOrigin]) {
      const [head] = headsOf(db, origin);
      const tombstone = readRevision(db, head!)!;
      assert.equal(tombstone.control.tombstone, true);
      assert.equal(tombstone.payload_hash, null);
      assert.equal(tombstone.parents.length, 1);
    }
    db.exec('BEGIN IMMEDIATE');
    assert.deepEqual(captureLocalChanges(db, 21), { revisions: 0, tombstones: 0 });
    db.exec('COMMIT');
    assert.ok(replica);
  });
});
