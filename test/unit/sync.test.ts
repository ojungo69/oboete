// US6 device sync (contracts/sync.md). Identity first: the ids that every replica must compute
// identically are pinned before any envelope or merge code exists.
import assert from 'node:assert/strict';
import { createDecipheriv, hkdfSync, randomBytes } from 'node:crypto';
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { grantVisibility } from '../../src/db/queries.js';
import { sha256Hex } from '../../src/hash.js';
import { oboetePaths } from '../../src/paths.js';
import { updateConfigFile } from '../../src/setup/consent.js';
import { resolveRow } from '../../src/sync/apply.js';
import { captureLocalChanges } from '../../src/sync/capture.js';
import { BundleError, CHUNK_BYTES, decryptBundle, encryptBundle, keyId, PREFIX_BYTES, TAG_BYTES } from '../../src/sync/envelope.js';
import { canonicalJson, payloadHash, revisionId, snapshotId } from '../../src/sync/identity.js';
import { syncItem } from '../../src/doctor/storage.js';
import { runSync } from '../../src/sync-cli.js';
import {
  initSpace, joinSpace, leaveSpace, pullSpace, pushSpace, showKey, SyncError, syncStatus, withSpaceLock,
} from '../../src/sync/space.js';
import { effectiveControl, headsOf, readOrigin, readRevision, replicaOriginId } from '../../src/sync/store.js';
import {
  insertMemory, insertSource, memoryOf, openHome, publish, pull, REMOTE, REPO, revisionCount, revisions, withHomes, withReplicas, withStore,
  type Replica,
} from '../helpers/sync.js';

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
    assert.match(String(source.local_id), /^source:[0-9a-f]{64}$/u);
    assert.equal(String(source.origin_id), `${replica}:${String(source.local_id)}`);
    const sourceRevision = revisions(db, String(source.origin_id))[0]!;
    assert.deepEqual(sourceRevision.natural, { key: String(source.local_id).slice('source:'.length) }, 'a source is named by its key alone');
    assert.equal(sourceRevision.payload?.memory_id, memoryOrigin, 'the memory it sits under is revision data');
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

// --- Publish and apply between replicas (contracts/sync.md "Merge rules", "Verification") ---

test('a memory with its source and grant round-trips A→B and a change-free B→A adds no revision', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertSource(a.db, 'm_one', 'src/a.ts');
    grantVisibility(a.db, 'm_one', { audience: 'project', repoId: REPO }, 'observer', 1);
    const bundle = publish(a, dir);
    const result = pull(b, a, bundle);
    assert.equal(result.stored, 3);
    assert.equal(result.withheldOnApply, 0);
    const copy = memoryOf(b, a, 'm_one');
    assert.equal(copy.title, 'Title');
    assert.equal(copy.body, 'Body text');
    assert.equal(copy.repo_id, REPO);
    assert.equal(copy.content_hash, memoryOf(a, a, 'm_one').content_hash);
    assert.equal(b.db.prepare('SELECT citation_value FROM memory_sources WHERE memory_id = ?').get(copy.id as string)?.citation_value, 'src/a.ts');
    assert.equal(b.db.prepare('SELECT audience FROM memory_visibility WHERE memory_id = ?').get(copy.id as string)?.audience, 'project');
    // B republishes what it received; A learns nothing new.
    const before = revisionCount(a.db);
    const back = pull(a, b, publish(b, dir));
    assert.equal(back.stored, 0);
    assert.equal(revisionCount(a.db), before);
    assert.equal(revisionCount(b.db), 3);
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'").get()?.n, 0);
  });
});

test('a deletion and a secret marking travel as control and erase the copy without resurrection', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertMemory(a.db, 'm_two', 'Other', 'Second body');
    pull(b, a, publish(a, dir));
    assert.equal(memoryOf(b, a, 'm_two').body, 'Second body');
    // B's bundle from before it learns of the deletion: a stale delivery with live payloads.
    const stale = publish(b, dir);
    a.db.prepare('UPDATE memories SET deleted_at = 50 WHERE id = ?').run('m_one');
    a.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = ?").run('m_two');
    pull(b, a, publish(a, dir));
    assert.notEqual(memoryOf(b, a, 'm_one').deleted_at, null);
    assert.equal(memoryOf(b, a, 'm_two').sensitivity, 'secret');
    assert.equal(memoryOf(b, a, 'm_two').body, '');
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM sync_revisions WHERE payload_json IS NOT NULL AND origin_id = ?").get(`${a.id}:m_two`)?.n, 0);
    // The stale bundle never undeletes or lowers on A, and B's fresh bundle does not either.
    pull(a, b, stale);
    assert.notEqual(memoryOf(a, a, 'm_one').deleted_at, null);
    assert.equal(memoryOf(a, a, 'm_two').sensitivity, 'secret');
    assert.equal(memoryOf(a, a, 'm_two').body, '');
    pull(a, b, publish(b, dir));
    assert.notEqual(memoryOf(a, a, 'm_one').deleted_at, null);
    assert.equal(a.db.prepare("SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'").get()?.n, 0);
  });
});

test('independent edits on both devices are siblings reported on both, and one resolve converges them', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    pull(b, a, publish(a, dir));
    a.db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run('m_one');
    b.db.prepare('UPDATE memories SET valid_to = 9 WHERE id = ?').run(memoryOf(b, a, 'm_one').id as string);
    const fromA = publish(a, dir);
    const fromB = publish(b, dir);
    pull(b, a, fromA);
    pull(a, b, fromB);
    const origin = `${a.id}:m_one`;
    for (const replica of [a, b]) {
      assert.equal(headsOf(replica.db, origin).length, 2, `${replica === a ? 'A' : 'B'} sees two heads`);
      assert.equal(replica.db.prepare("SELECT status FROM sync_conflicts WHERE id = ?").get(`sync:${origin}`)?.status, 'open');
    }
    // Each device keeps its own line as the effective row until a human resolves.
    assert.equal(memoryOf(a, a, 'm_one').pinned_at, 5);
    assert.equal(memoryOf(b, a, 'm_one').valid_to, 9);
    // C joins from B's bundle alone and sees the same two heads.
    pull(c, b, publish(b, dir));
    assert.equal(headsOf(c.db, origin).length, 2);
    assert.equal(c.db.prepare("SELECT status FROM sync_conflicts WHERE id = ?").get(`sync:${origin}`)?.status, 'open');
    // One resolve on A (keep B's line) converges every device through any relay.
    const keep = headsOf(a.db, origin).find((head) => readRevision(a.db, head)!.author === b.id)!;
    a.db.exec('BEGIN IMMEDIATE');
    const { revision_id } = resolveRow(a.db, origin, keep, 300);
    a.db.exec('COMMIT');
    assert.deepEqual(headsOf(a.db, origin), [revision_id]);
    assert.equal(readRevision(a.db, revision_id)!.parents.length, 2);
    assert.equal(memoryOf(a, a, 'm_one').valid_to, 9);
    assert.equal(memoryOf(a, a, 'm_one').pinned_at, null);
    assert.equal(a.db.prepare("SELECT status FROM sync_conflicts WHERE id = ?").get(`sync:${origin}`)?.status, 'resolved');
    const resolved = publish(a, dir);
    pull(c, a, resolved);
    pull(b, c, publish(c, dir));
    for (const replica of [b, c]) {
      assert.deepEqual(headsOf(replica.db, origin), [revision_id]);
      assert.equal(memoryOf(replica, a, 'm_one').valid_to, 9);
      assert.equal(memoryOf(replica, a, 'm_one').pinned_at, null);
      assert.equal(replica.db.prepare("SELECT status FROM sync_conflicts WHERE id = ?").get(`sync:${origin}`)?.status, 'resolved');
    }
    // A local edit after the resolution descends from it: no new conflict anywhere.
    b.db.prepare('UPDATE memories SET pinned_at = 7, pin_order = 1 WHERE id = ?').run(memoryOf(b, a, 'm_one').id as string);
    pull(a, b, publish(b, dir));
    assert.equal(headsOf(a.db, origin).length, 1);
    assert.equal(memoryOf(a, a, 'm_one').pinned_at, 7);
  });
});

// --- Space, keys, consent, lock, push and pull over a shared directory (T035) ---

test('init, join by key line, push and pull move a memory through the shared directory', async () => {
  await withHomes(2, (homes, shared) => {
    const [homeA, homeB] = homes as [string, string];
    const a = openHome(homeA);
    const b = openHome(homeB);
    try {
      const pathsA = oboetePaths(homeA);
      const pathsB = oboetePaths(homeB);
      const { spaceId, keyLine: line } = initSpace(a, pathsA, { directory: shared, classes: ['eligible', 'local_only'], now: 1 });
      assert.match(line, /^oboete-sync-key\/1:[0-9a-f]{32}:[A-Za-z0-9_-]{43}$/u);
      assert.equal(showKey(pathsA), line);
      assert.equal((statSync(join(homeA, 'sync', `${spaceId}.key`)).mode & 0o777), 0o600);
      assert.equal(existsSync(join(shared, 'oboete-sync')), false, 'init writes nothing to the directory');
      assert.equal(joinSpace(b, pathsB, { directory: shared, keyLine: line, classes: ['eligible', 'local_only'], now: 1 }).spaceId, spaceId);
      insertMemory(a, 'm_one', 'Title', 'Body text');
      insertMemory(a, 'm_priv', 'Private', 'Not selected', { sensitivity: 'private' });
      const push = pushSpace(a, pathsA, { now: 10 });
      assert.equal(push.outcome, 'published');
      assert.equal(push.withheld.memories, 1);
      const bundle = join(shared, 'oboete-sync', 'v1', spaceId, `${replicaOriginId(a)}.osb`);
      assert.ok(existsSync(bundle));
      assert.equal(readdirSync(join(shared, 'oboete-sync', 'v1', spaceId)).length, 1, 'no temporary file left behind');
      assert.equal(pushSpace(a, pathsA, { now: 11 }).outcome, 'unchanged');
      assert.equal(readdirSync(join(homeA, 'sync', 'staging')).length, 0, 'staging is cleaned');
      const pull = pullSpace(b, pathsB, { now: 20 });
      assert.deepEqual(pull.bundles.map((entry) => entry.outcome), ['applied']);
      const copy = b.prepare('SELECT title, body, sensitivity FROM memories ORDER BY id').all().map((row) => ({ ...row }));
      assert.deepEqual(copy, [{ title: 'Title', body: 'Body text', sensitivity: 'eligible' }]);
      assert.equal(readOrigin(b, `${replicaOriginId(a)}:m_priv`)?.local_id, null, 'the unselected class travels identity-only');
      assert.deepEqual(pullSpace(b, pathsB, { now: 21 }).bundles.map((entry) => entry.outcome), ['skipped']);
      // B pushes; A pulls; nothing new anywhere (change-free round trip) and no conflict.
      assert.equal(pushSpace(b, pathsB, { now: 30 }).outcome, 'published');
      assert.deepEqual(pullSpace(a, pathsA, { now: 40 }).bundles.map((entry) => entry.outcome), ['applied']);
      assert.equal(a.prepare("SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'").get()?.n, 0);
      assert.equal(revisionCount(a), 2);
      assert.equal(syncStatus(a, pathsA).replicas.length, 1);
      // A tampered bundle is rejected by name and the other bundle's outcome is unaffected.
      const bytes = readFileSync(bundle);
      bytes[bytes.length - 3]! ^= 0x01;
      writeFileSync(bundle, bytes);
      assert.deepEqual(pullSpace(b, pathsB, { now: 50 }).bundles, [{ replica: replicaOriginId(a), outcome: 'rejected', reason: 'authentication_failed' }]);
      // Consent drift performs no I/O.
      updateConfigFile(pathsB, (root) => { (root.sync as Record<string, unknown>).classes = ['eligible']; });
      assert.throws(() => pullSpace(b, pathsB, { now: 60 }), (error: unknown) => error instanceof SyncError && error.code === 'consent_mismatch'
        && JSON.stringify(error.detail) === JSON.stringify({ changed: ['classes'] }));
      updateConfigFile(pathsB, (root) => { (root.sync as Record<string, unknown>).classes = ['eligible', 'local_only']; });
      // Leave removes the key, the cursors and this replica's own bundle only.
      leaveSpace(b, pathsB);
      assert.equal(existsSync(join(homeB, 'sync', `${spaceId}.key`)), false);
      assert.equal(existsSync(join(shared, 'oboete-sync', 'v1', spaceId, `${replicaOriginId(b)}.osb`)), false);
      assert.ok(existsSync(bundle));
      assert.equal(syncStatus(b, pathsB).configured, false);
    } finally { a.close(); b.close(); }
  });
});

test('the per-space lock makes a second push or pull exit busy and leaves nothing behind', async () => {
  await withHomes(1, (homes, shared) => {
    const [home] = homes as [string];
    const db = openHome(home);
    try {
      const paths = oboetePaths(home);
      const { spaceId } = initSpace(db, paths, { directory: shared, classes: ['eligible'], now: 1 });
      withSpaceLock(paths, spaceId, () => {
        assert.throws(() => pushSpace(db, paths, { now: 2 }), (error: unknown) => error instanceof SyncError && error.code === 'busy');
        assert.throws(() => pullSpace(db, paths, { now: 2 }), (error: unknown) => error instanceof SyncError && error.code === 'busy');
      });
      assert.equal(pushSpace(db, paths, { now: 3 }).outcome, 'published');
    } finally { db.close(); }
  });
});

test('a commit by another connection at any push window restarts the push and ships the control', async () => {
  await withHomes(2, (homes, shared) => {
    const [homeA, homeB] = homes as [string, string];
    const a = openHome(homeA);
    const b = openHome(homeB);
    try {
      const pathsA = oboetePaths(homeA);
      const pathsB = oboetePaths(homeB);
      const { keyLine: line } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine: line, classes: ['eligible'], now: 1 });
      insertMemory(a, 'm_one', 'Title', 'Body text');
      assert.equal(pushSpace(a, pathsA, { now: 10 }).outcome, 'published');
      pullSpace(b, pathsB, { now: 11 });
      assert.equal(memoryOf({ db: b, home: homeB, id: replicaOriginId(b) }, { db: a, home: homeA, id: replicaOriginId(a) }, 'm_one').body, 'Body text');
      const other = openDatabase({ path: pathsA.db, timeoutMs: 1000 }).db;
      try {
        for (const [index, step] of (['after_capture', 'after_staging', 'after_encrypt'] as const).entries()) {
          insertMemory(a, `m_${step}`, `Title ${step}`, `Body ${step}`);
          let fired = false;
          const push = pushSpace(a, pathsA, { now: 20, probe: (at) => {
            if (at !== step || fired) return;
            fired = true;
            other.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = ?").run(`m_${step}`);
          } });
          assert.equal(push.outcome, 'published', step);
          assert.equal(push.restarts, 1, `${step} restarted once`);
          assert.equal(push.withheld.memories, index + 1, `${step}: the secret row's payload is withheld and its control ships`);
          pullSpace(b, pathsB, { now: 30 });
          const copy = readOrigin(b, `${replicaOriginId(a)}:m_${step}`);
          assert.equal(copy?.local_id, null, `${step}: no text of the secret row reached B`);
          assert.equal(effectiveControl(b, `${replicaOriginId(a)}:m_${step}`).sensitivity_floor, 'secret', step);
        }
        // The "unchanged" path re-checks too: a secret marking during staging is never hidden.
        let fired = false;
        const push = pushSpace(a, pathsA, { now: 40, probe: (at) => {
          if (at !== 'after_staging' || fired) return;
          fired = true;
          other.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = 'm_one'").run();
        } });
        assert.equal(push.outcome, 'published');
        assert.equal(push.restarts, 1);
        pullSpace(b, pathsB, { now: 50 });
        assert.equal(memoryOf({ db: b, home: homeB, id: replicaOriginId(b) }, { db: a, home: homeA, id: replicaOriginId(a) }, 'm_one').sensitivity, 'secret');
      } finally { other.close(); }
    } finally { a.close(); b.close(); }
  });
});

// --- CLI surface (T036) ---

function fakeIo(tty: boolean, secret = ''): { io: Parameters<typeof runSync>[1]; out: string[]; err: string[] } {
  const out: string[] = [];
  const err: string[] = [];
  return { out, err, io: { out: (text) => { out.push(text); }, err: (text) => { err.push(text); }, isTty: () => tty, readSecret: async () => secret } };
}

test('oboete sync commands: init, key show on a terminal only, join by typed key, push, pull, status, resolve, leave', async () => {
  await withHomes(2, async (homes, shared) => {
    const [homeA, homeB] = homes as [string, string];
    const pathsA = oboetePaths(homeA);
    const pathsB = oboetePaths(homeB);
    openHome(homeA).close();
    openHome(homeB).close();
    const bad = fakeIo(false);
    assert.equal(await runSync(['init', shared, '--classes', 'eligible,foo', '--json'], bad.io, pathsA, 1), 1);
    assert.equal((JSON.parse(bad.err[0]!) as { error: string }).error, 'invalid_classes');
    assert.equal(existsSync(join(homeA, 'config.toml')) && readFileSync(join(homeA, 'config.toml'), 'utf8').includes('[sync]'), false, 'nothing was configured');
    const init = fakeIo(false);
    assert.equal(await runSync(['init', shared, '--json'], init.io, pathsA, 1), 0);
    const spaceId = (JSON.parse(init.out[0]!) as { space_id: string }).space_id;
    const hidden = fakeIo(false);
    assert.equal(await runSync(['key', 'show'], hidden.io, pathsA, 1), 2);
    assert.equal(hidden.out.length, 0);
    const shown = fakeIo(true);
    assert.equal(await runSync(['key', 'show'], shown.io, pathsA, 1), 0);
    const line = shown.out[0]!.trim();
    assert.equal(await runSync(['join', shared, '--json'], fakeIo(false, line).io, pathsB, 1), 2, 'join needs a terminal');
    const joined = fakeIo(true, line);
    assert.equal(await runSync(['join', shared, '--json'], joined.io, pathsB, 1), 0);
    assert.equal((JSON.parse(joined.out[0]!) as { space_id: string }).space_id, spaceId);
    const a = openHome(homeA);
    insertMemory(a, 'm_one', 'Title', 'Body text');
    a.close();
    const push = fakeIo(false);
    assert.equal(await runSync(['push', '--json'], push.io, pathsA, 10), 0);
    assert.equal((JSON.parse(push.out[0]!) as { outcome: string }).outcome, 'published');
    const pull = fakeIo(false);
    assert.equal(await runSync(['pull', '--json'], pull.io, pathsB, 20), 0);
    assert.equal((JSON.parse(pull.out[0]!) as { bundles: { outcome: string }[] }).bundles[0]!.outcome, 'applied');
    const status = fakeIo(false);
    assert.equal(await runSync(['status', '--json'], status.io, pathsB, 21), 0);
    assert.equal((JSON.parse(status.out[0]!) as { replicas: unknown[] }).replicas.length, 1);
    assert.equal(await runSync(['resolve', 'x'], fakeIo(false).io, pathsB, 22), 2, '--keep is required');
    const unknown = fakeIo(false);
    assert.equal(await runSync(['resolve', 'x', '--keep', 'y', '--json'], unknown.io, pathsB, 22), 1);
    assert.equal((JSON.parse(unknown.err[0]!) as { error: string }).error, 'unknown_origin');
    const leave = fakeIo(false);
    assert.equal(await runSync(['leave', '--json'], leave.io, pathsB, 30), 0);
    const gone = fakeIo(false);
    assert.equal(await runSync(['status', '--json'], gone.io, pathsB, 31), 0);
    assert.equal((JSON.parse(gone.out[0]!) as { configured: boolean }).configured, false);
    const notConfigured = fakeIo(false);
    assert.equal(await runSync(['push'], notConfigured.io, pathsB, 32), 1);
  });
});

test('oboete doctor reports sync from local data only: unconfigured, configured, consent drift', async () => {
  await withHomes(1, async (homes, shared) => {
    const [home] = homes as [string];
    const paths = oboetePaths(home);
    const db = openHome(home);
    try {
      assert.deepEqual(syncItem(paths, db, false), { item: 'sync', status: 'healthy', reason: 'Sync is not configured.', consequence: '', recovery: '' });
      const { spaceId } = initSpace(db, paths, { directory: shared, classes: ['eligible', 'local_only', 'private'], now: 1 });
      assert.equal(syncItem(paths, db, false).reason, `Sync space ${spaceId} is configured and its consent matches.`);
      rmSync(shared, { recursive: true, force: true });
      assert.equal(syncItem(paths, db, false).status, 'healthy', 'the space directory is never opened');
      updateConfigFile(paths, (root) => { (root.sync as Record<string, unknown>).classes = ['eligible']; });
      const drifted = syncItem(paths, db, false);
      assert.equal(drifted.status, 'warning');
      assert.equal(drifted.reason, 'The sync consent no longer matches (classes).');
      assert.equal(syncItem(paths, null, true).status, 'unverified');
    } finally { db.close(); }
  });
});
