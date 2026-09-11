// US6 work/checkpoint merge rules (contracts/sync.md "Verification": checkpoint resolve,
// checkpoint order, checkpoint deletion, observer candidate, identity-only head). Works and
// checkpoints are seeded with raw INSERTs because the observer's own path needs a whole
// batch/lease pipeline; every assertion reads product state (rows, heads, conflict rows).
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { checkpointHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { oboetePaths } from '../../src/paths.js';
import { resolveRow } from '../../src/sync/apply.js';
import { snapshotId } from '../../src/sync/identity.js';
import { initSpace, syncStatus } from '../../src/sync/space.js';
import { headsOf, readOrigin, readRevision, replicaOriginId } from '../../src/sync/store.js';
import { openHome, publish, pull, REPO, withHomes, withReplicas, type Replica } from '../helpers/sync.js';

const WORK = 'w_sync_checkpoint';
const CONTEXT = 'ctx_sync_checkpoint';
/** `private` unselected, as the identity-only bullet requires. */
const VISIBLE = ['eligible', 'local_only'] as const;
const MATERIAL_X = materialHash('Material X', 'Material X body');

function seedWork(db: DatabaseSync): string {
  db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at)
    VALUES (?, ?, 'k1', '/work/sync', 1, 1)`).run(CONTEXT, REPO);
  db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, purpose_sensitivity, state, created_at, updated_at)
    VALUES (?, ?, ?, 'Ship sync', 'eligible', 'active', 1, 1)`).run(WORK, REPO, CONTEXT);
  return WORK;
}

/** One checkpoint memory of `workId`; `select: false` leaves it stored but unselected (a candidate). */
function addCheckpoint(db: DatabaseSync, workId: string, parent: string | null, title: string, body: string,
  extra: { sensitivity?: string; select?: boolean } = {}): string {
  const material = materialHash(title, body);
  const content = checkpointHash(REPO, workId, parent, material);
  const id = memoryIdFor(content);
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash,
    sensitivity, review_state, created_at, work_id, checkpoint_parent_id)
    VALUES (?, ?, 'session_summary', ?, ?, '[]', '', ?, ?, ?, 'reviewed', 1, ?, ?)`)
    .run(id, REPO, title, body, material, content, extra.sensitivity ?? 'eligible', workId, parent);
  if (extra.select !== false) {
    db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ?, updated_at = updated_at + 1 WHERE id = ?').run(id, workId);
  }
  return id;
}

const localOf = (db: DatabaseSync, originId: string): string | null => readOrigin(db, originId)?.local_id ?? null;

const pointerOfWork = (db: DatabaseSync, workLocalId: string | null): string | null =>
  (db.prepare('SELECT current_checkpoint_memory_id AS p FROM work_items WHERE id = ?').get(workLocalId)?.p as string | null) ?? null;

/** `work_items.current_checkpoint_memory_id` of the row a work origin names on this device. */
const pointer = (db: DatabaseSync, workOrigin: string): string | null => pointerOfWork(db, localOf(db, workOrigin));

const conflictOf = (db: DatabaseSync, originId: string): string | undefined =>
  db.prepare('SELECT status FROM sync_conflicts WHERE id = ?').get(`sync:${originId}`)?.status as string | undefined;

const openConflicts = (db: DatabaseSync): number =>
  Number(db.prepare("SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'").get()?.n);

const deletedAt = (db: DatabaseSync, originId: string): number | null =>
  (db.prepare('SELECT deleted_at AS d FROM memories WHERE id = ?').get(localOf(db, originId))?.d as number | null) ?? null;

/** Live checkpoints of a work carrying one material: the unit `control.tombstone` deletes. */
const liveWithMaterial = (db: DatabaseSync, workOrigin: string, material: string): number =>
  Number(db.prepare('SELECT COUNT(*) AS n FROM memories WHERE work_id = ? AND material_hash = ? AND deleted_at IS NULL')
    .get(localOf(db, workOrigin), material)?.n);

/** Rewrites a plaintext bundle with the named origins' lines first, in that order. */
function reorder(path: string, originOrder: readonly string[], out: string): string {
  const [header, ...body] = readFileSync(path, 'utf8').split('\n').filter((line) => line.length > 0);
  const rank = (line: string): number => {
    const index = originOrder.indexOf(String((JSON.parse(line) as { origin_id?: string }).origin_id));
    return index === -1 ? originOrder.length : index;
  };
  const text = [...body].sort((x, y) => rank(x) - rank(y)).map((line) => `${line}\n`).join('');
  const head = JSON.parse(header!) as Record<string, unknown>;
  head.revisions_sha256 = createHash('sha256').update(text).digest('hex');
  head.snapshot_id = snapshotId(String(head.space_id), String(head.replica_origin_id), String(head.revisions_sha256));
  writeFileSync(out, `${JSON.stringify(head)}\n${text}`);
  return out;
}

/** C0 ← C1 ← C2 on `a`, published with the three checkpoint lines listed C2, C1, C0. */
function chainBundle(a: Replica, dir: string): { c0: string; c1: string; c2: string; path: string } {
  const c0 = addCheckpoint(a.db, WORK, null, 'Base', 'Base body');
  const c1 = addCheckpoint(a.db, WORK, c0, 'Middle', 'Middle body');
  const c2 = addCheckpoint(a.db, WORK, c1, 'Latest', 'Latest body');
  const path = reorder(publish(a, dir), [`${a.id}:${c2}`, `${a.id}:${c1}`, `${a.id}:${c0}`], join(dir, 'reordered.plain'));
  return { c0, c1, c2, path };
}

/** A and B fork C1/C2 from a shared C0; returns B's C2 and the work origin. */
function fork(a: Replica, b: Replica, dir: string): { workOrigin: string; c1: string; c2: string } {
  const workOrigin = `${a.id}:${WORK}`;
  seedWork(a.db);
  const c0 = addCheckpoint(a.db, WORK, null, 'Base', 'Base body');
  pull(b, a, publish(a, dir));
  const c1 = addCheckpoint(a.db, WORK, c0, 'Left', 'Left body');
  const c2 = addCheckpoint(b.db, localOf(b.db, workOrigin)!, localOf(b.db, `${a.id}:${c0}`), 'Right', 'Right body');
  return { workOrigin, c1, c2 };
}

// --- checkpoint resolve ---

test('resolving a work fork by keeping the other branch moves head, pointer and conflict together', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    const { workOrigin, c1, c2 } = fork(a, b, dir);
    const fromB = publish(b, dir);
    publish(a, dir);
    pull(a, b, fromB);
    assert.equal(headsOf(a.db, workOrigin).length, 2, 'the two work revisions are siblings');
    assert.equal(conflictOf(a.db, workOrigin), 'open');
    assert.equal(pointer(a.db, workOrigin), c1);
    // `resolve --keep C2` names a stored checkpoint memory of the work, not a revision.
    a.db.exec('BEGIN IMMEDIATE');
    const { revision_id } = resolveRow(a.db, workOrigin, `${b.id}:${c2}`, 300);
    a.db.exec('COMMIT');
    assert.deepEqual(headsOf(a.db, workOrigin), [revision_id]);
    assert.equal(readRevision(a.db, revision_id)!.parents.length, 2);
    assert.equal(pointer(a.db, workOrigin), localOf(a.db, `${b.id}:${c2}`));
    assert.equal(conflictOf(a.db, workOrigin), 'resolved');
    assert.equal(openConflicts(a.db), 0, 'the resolve opens no new conflict');
  });
});

test('a resolution keeping C2 moves head, pointer and conflict together on the device that is on the C1 branch',
  async () => {
    await withReplicas(2, (replicas, dir) => {
      const [a, b] = replicas as [Replica, Replica];
      const { workOrigin, c1, c2 } = fork(a, b, dir);
      const fromA = publish(a, dir);
      pull(a, b, publish(b, dir));
      pull(b, a, fromA);
      for (const replica of [a, b]) assert.equal(conflictOf(replica.db, workOrigin), 'open');
      assert.equal(pointer(a.db, workOrigin), c1, 'A is still on its own branch before the resolution arrives');
      b.db.exec('BEGIN IMMEDIATE');
      const resolution = resolveRow(b.db, workOrigin, `${b.id}:${c2}`, 300).revision_id;
      b.db.exec('COMMIT');
      pull(a, b, publish(b, dir));
      assert.deepEqual(headsOf(a.db, workOrigin), [resolution]);
      assert.equal(pointer(a.db, workOrigin), localOf(a.db, `${b.id}:${c2}`));
      assert.equal(conflictOf(a.db, workOrigin), 'resolved');
      assert.equal(openConflicts(a.db), 0);
    });
  });

test('a resolution reaches a device through its single-parent successor carrying the payload',
  async () => {
    await withReplicas(3, (replicas, dir) => {
      const [a, b, c] = replicas as [Replica, Replica, Replica];
      const { workOrigin, c2 } = fork(a, b, dir);
      const fromA = publish(a, dir);
      pull(c, a, fromA);
      pull(b, a, fromA);
      b.db.exec('BEGIN IMMEDIATE');
      const resolution = resolveRow(b.db, workOrigin, `${b.id}:${c2}`, 300).revision_id;
      b.db.exec('COMMIT');
      // A further local change supersedes R, so R ships identity-only and S carries the payload.
      b.db.prepare("UPDATE work_items SET state = 'completed', completed_at = 310, updated_at = 310 WHERE id = ?")
        .run(localOf(b.db, workOrigin));
      const fromB = publish(b, dir);
      const successor = headsOf(b.db, workOrigin);
      assert.deepEqual(readRevision(b.db, successor[0]!)!.parents, [resolution]);
      pull(a, b, fromB);
      pull(c, b, fromB);
      for (const replica of [a, c]) {
        assert.equal(readRevision(replica.db, resolution)!.payload, null, 'R travels identity-only once S is the head');
        assert.deepEqual(headsOf(replica.db, workOrigin), successor);
        assert.equal(pointer(replica.db, workOrigin), localOf(replica.db, `${b.id}:${c2}`));
        // Neither device held both siblings before R arrived, so no conflict row was ever opened.
        assert.notEqual(conflictOf(replica.db, workOrigin), 'open');
      }
    });
  });

test('a resolution that misses a head the receiver holds is one more sibling', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    const workOrigin = `${a.id}:${WORK}`;
    seedWork(a.db);
    const c0 = addCheckpoint(a.db, WORK, null, 'Base', 'Base body');
    const fromA0 = publish(a, dir);
    pull(b, a, fromA0);
    pull(c, a, fromA0);
    addCheckpoint(a.db, WORK, c0, 'Left', 'Left body');
    const c2 = addCheckpoint(b.db, localOf(b.db, workOrigin)!, localOf(b.db, `${a.id}:${c0}`), 'Right', 'Right body');
    // C forks a third line that A never sees before resolving.
    const c3 = addCheckpoint(c.db, localOf(c.db, workOrigin)!, localOf(c.db, `${a.id}:${c0}`), 'Third', 'Third body');
    pull(a, b, publish(b, dir));
    const own = headsOf(c.db, workOrigin);
    a.db.exec('BEGIN IMMEDIATE');
    const resolution = resolveRow(a.db, workOrigin, `${b.id}:${c2}`, 300).revision_id;
    a.db.exec('COMMIT');
    pull(c, a, publish(a, dir));
    const heads = headsOf(c.db, workOrigin);
    assert.equal(heads.length, 2, 'the resolution is a sibling of the head it never cited');
    assert.ok(heads.includes(resolution));
    assert.equal(readRevision(c.db, resolution)!.parents.includes(own[0]!), false);
    assert.equal(conflictOf(c.db, workOrigin), 'open');
    assert.equal(pointer(c.db, workOrigin), c3, "C's own line still drives the pointer");
  });
});

// --- checkpoint order ---

test('a joining device rebuilds the chain from heads alone, whatever order the lines arrive in', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, d] = replicas as [Replica, Replica];
    const workOrigin = `${a.id}:${WORK}`;
    seedWork(a.db);
    const { c0, c1, c2, path } = chainBundle(a, dir);
    pull(d, a, path);
    const parentOf = (id: string | null): string | null =>
      (d.db.prepare('SELECT checkpoint_parent_id AS p FROM memories WHERE id = ?').get(id)?.p as string | null) ?? null;
    assert.equal(pointer(d.db, workOrigin), localOf(d.db, `${a.id}:${c2}`));
    assert.equal(parentOf(localOf(d.db, `${a.id}:${c2}`)), localOf(d.db, `${a.id}:${c1}`));
    assert.equal(parentOf(localOf(d.db, `${a.id}:${c1}`)), localOf(d.db, `${a.id}:${c0}`));
    assert.equal(parentOf(localOf(d.db, `${a.id}:${c0}`)), null);
    assert.equal(openConflicts(d.db), 0);
  });
});

test('a bundle listing C2, C1, C0 in that order fast-forwards a work sitting at C0',
  async () => {
    await withReplicas(2, (replicas, dir) => {
      const [a, b] = replicas as [Replica, Replica];
      const workOrigin = `${a.id}:${WORK}`;
      seedWork(a.db);
      const c0 = addCheckpoint(a.db, WORK, null, 'Base', 'Base body');
      pull(b, a, publish(a, dir));
      assert.equal(pointer(b.db, workOrigin), localOf(b.db, `${a.id}:${c0}`));
      const c1 = addCheckpoint(a.db, WORK, c0, 'Middle', 'Middle body');
      const c2 = addCheckpoint(a.db, WORK, c1, 'Latest', 'Latest body');
      pull(b, a, reorder(publish(a, dir), [`${a.id}:${c2}`, `${a.id}:${c1}`, `${a.id}:${c0}`], join(dir, 'reordered.plain')));
      assert.equal(headsOf(b.db, workOrigin).length, 1, 'the work fast-forwards instead of forking');
      assert.equal(pointer(b.db, workOrigin), localOf(b.db, `${a.id}:${c2}`));
      assert.equal(openConflicts(b.db), 0);
    });
  });

// --- checkpoint deletion ---

test('a tombstone for C0 applies to a checkpoint of the same material arriving later under another parent',
  async () => {
    await withReplicas(2, (replicas, dir) => {
      // The apply visits memory rows in ascending origin id, so giving the deleting device the
      // smaller id fixes the arrival order the contract says the outcome must not depend on.
      const [a, b] = [...replicas].sort((x, y) => (x.id < y.id ? -1 : 1)) as [Replica, Replica];
      const workOrigin = `${a.id}:${WORK}`;
      seedWork(a.db);
      const c0 = addCheckpoint(a.db, WORK, null, 'Material X', 'Material X body');
      pull(b, a, publish(a, dir));
      const workOnB = localOf(b.db, workOrigin)!;
      const c1 = addCheckpoint(b.db, workOnB, localOf(b.db, `${a.id}:${c0}`), 'Material Y', 'Material Y body');
      const c2 = addCheckpoint(b.db, workOnB, c1, 'Material X', 'Material X body');
      const fromB = publish(b, dir);
      a.db.prepare('UPDATE memories SET deleted_at = 250 WHERE id = ?').run(c0);
      pull(a, b, fromB);
      assert.equal(deletedAt(a.db, `${a.id}:${c0}`), 250);
      assert.notEqual(deletedAt(a.db, `${b.id}:${c2}`), null, 'C2 is applied deleted');
      assert.equal(liveWithMaterial(a.db, workOrigin, MATERIAL_X), 0);
    });
  });

test('a tombstone reaching a device that already stores C2 sweeps it, ships it, and a relayed device converges', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    const workOrigin = `${a.id}:${WORK}`;
    seedWork(a.db);
    const c0 = addCheckpoint(a.db, WORK, null, 'Material X', 'Material X body');
    const fromA0 = publish(a, dir);
    pull(b, a, fromA0);
    pull(c, a, fromA0);
    const workOnB = localOf(b.db, workOrigin)!;
    const c1 = addCheckpoint(b.db, workOnB, localOf(b.db, `${a.id}:${c0}`), 'Material Y', 'Material Y body');
    const c2 = addCheckpoint(b.db, workOnB, c1, 'Material X', 'Material X body');
    // C takes B's live C2 before A's tombstone exists.
    pull(c, b, publish(b, dir));
    assert.equal(deletedAt(c.db, `${b.id}:${c2}`), null);
    a.db.prepare('UPDATE memories SET deleted_at = 250 WHERE id = ?').run(c0);
    const fromA1 = publish(a, dir);
    pull(b, a, fromA1);
    assert.notEqual(deletedAt(b.db, `${b.id}:${c2}`), null, "B sweeps its own C2 on A's tombstone");
    // B's next push carries C2's own tombstone, not only C0's.
    const fromB2 = publish(b, dir);
    const head = headsOf(b.db, `${b.id}:${c2}`);
    assert.equal(head.length, 1);
    assert.equal(readRevision(b.db, head[0]!)!.control.tombstone, true);
    pull(c, a, fromA1);
    assert.notEqual(deletedAt(c.db, `${b.id}:${c2}`), null, 'a device that pulled B before A ends deleted too');
    pull(a, b, fromB2);
    for (const replica of [a, b, c]) assert.equal(liveWithMaterial(replica.db, workOrigin, MATERIAL_X), 0);
  });
});

// --- observer candidate ---

test('observer checkpoint conflicts predating sync are listed by status and closed by the resolve of their work', async () => {
  await withHomes(1, (homes, shared) => {
    const [home] = homes as [string];
    const db = openHome(home);
    try {
      const paths = oboetePaths(home);
      initSpace(db, paths, { directory: shared, classes: ['eligible'], now: 1 });
      seedWork(db);
      const c0 = addCheckpoint(db, WORK, null, 'Base', 'Base body');
      const c1 = addCheckpoint(db, WORK, c0, 'Current', 'Current body');
      const candidates = [addCheckpoint(db, WORK, c0, 'Candidate', 'Candidate body', { select: false }),
        addCheckpoint(db, WORK, c0, 'Other candidate', 'Other candidate body', { select: false })];
      const rows = candidates.map((candidate) => {
        const id = randomUUID();
        db.prepare(`INSERT INTO sync_conflicts (id, repo_id, content_hash, local_state_json, remote_state_json, status, created_at)
          VALUES (?, ?, (SELECT content_hash FROM memories WHERE id = ?), ?, ?, 'open', 5)`)
          .run(id, REPO, candidate, JSON.stringify({ work_id: WORK, checkpoint_memory_id: c1 }),
            JSON.stringify({ work_id: WORK, checkpoint_memory_id: candidate, parent_memory_id: c0 }));
        return id;
      });
      assert.deepEqual(syncStatus(db, paths).conflicts.map((row) => row.id).sort(), [...rows].sort(),
        'rows the observer wrote before the first sync are listed');
      assert.equal(pointerOfWork(db, WORK), c1);
      const workOrigin = `${replicaOriginId(db)}:${WORK}`;
      db.exec('BEGIN IMMEDIATE');
      const { revision_id } = resolveRow(db, workOrigin, `${replicaOriginId(db)}:${candidates[0]!}`, 300);
      db.exec('COMMIT');
      assert.equal(pointerOfWork(db, WORK), candidates[0]);
      assert.deepEqual(headsOf(db, workOrigin), [revision_id]);
      assert.deepEqual(db.prepare('SELECT status FROM sync_conflicts ORDER BY id').all().map((row) => row.status),
        ['resolved', 'resolved']);
      assert.deepEqual(syncStatus(db, paths).conflicts, []);
    } finally { db.close(); }
  });
});

// --- identity-only head ---

test('a completion after an identity-only work revision is a sibling of the last materialized revision, and the relay converges', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    const workOrigin = `${a.id}:${WORK}`;
    seedWork(a.db);
    const fromA0 = publish(a, dir, VISIBLE);
    pull(b, a, fromA0);
    pull(c, a, fromA0);
    const w0 = headsOf(a.db, workOrigin)[0]!;
    // A's first checkpoint is private, so the work revision pointing at it ships identity-only.
    const c1 = addCheckpoint(a.db, WORK, null, 'Private step', 'Private body', { sensitivity: 'private' });
    pull(b, a, publish(a, dir, VISIBLE));
    const w1 = headsOf(a.db, workOrigin)[0]!;
    assert.equal(readRevision(b.db, w1)!.payload, null, 'B receives W1 identity-only');
    assert.equal(localOf(b.db, `${a.id}:${c1}`), null, 'and C1 itself has no row on B');
    b.db.prepare("UPDATE work_items SET state = 'completed', completed_at = 310, updated_at = 310 WHERE id = ?")
      .run(localOf(b.db, workOrigin));
    const fromB1 = publish(b, dir, VISIBLE);
    const sibling = headsOf(b.db, workOrigin).find((head) => head !== w1)!;
    assert.deepEqual(readRevision(b.db, sibling)!.parents, [w0], 'the parent is the last materialized revision, not W1');
    pull(a, b, fromB1);
    pull(b, a, publish(a, dir, VISIBLE));
    for (const replica of [a, b]) {
      assert.equal(headsOf(replica.db, workOrigin).length, 2);
      assert.equal(conflictOf(replica.db, workOrigin), 'open');
    }
    assert.equal(pointer(a.db, workOrigin), c1, 'C1 is still selected on A');
    assert.equal(readOrigin(a.db, workOrigin)!.selected_head, w1);
    // R keeps A's line, so it still points at the private C1 and ships identity-only; A then drops
    // the pointer, and that successor S is the first revision of the line to carry a payload.
    a.db.exec('BEGIN IMMEDIATE');
    const resolution = resolveRow(a.db, workOrigin, w1, 320).revision_id;
    a.db.exec('COMMIT');
    a.db.prepare('UPDATE work_items SET current_checkpoint_memory_id = NULL, updated_at = 330 WHERE id = ?').run(WORK);
    const fromA3 = publish(a, dir, VISIBLE);
    pull(b, a, fromA3);
    pull(c, a, fromA3);
    const successor = headsOf(a.db, workOrigin);
    assert.deepEqual(readRevision(a.db, successor[0]!)!.parents, [resolution]);
    for (const replica of [b, c]) {
      assert.equal(readRevision(replica.db, resolution)!.payload, null, 'the resolution itself is identity-only');
      assert.deepEqual(headsOf(replica.db, workOrigin), successor);
      assert.equal(pointer(replica.db, workOrigin), null);
      assert.equal(openConflicts(replica.db), 0);
    }
  });
});
