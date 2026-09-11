// US6 device sync, the flow half of contracts/sync.md "Verification": relay, partial descent,
// foreign edit, the change pass, the late child, the change-free round trip and delivery
// identity. Replica-level cases move a plaintext bundle between homes with `publish`/`pull`;
// the push-window cases need real spaces (initSpace/joinSpace/pushSpace with its probe).
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { captureLocalChanges } from '../../src/sync/capture.js';
import type { Sensitivity } from '../../src/sync/identity.js';
import { buildSnapshot } from '../../src/sync/publish.js';
import { initSpace, joinSpace, loadSyncConfig, mapRepo, pullSpace, pushSpace, spaceDirectory } from '../../src/sync/space.js';
import { canonicalOf, effectiveControl, headsOf, readOrigin, readRevision, replicaOriginId, repoKeyFor } from '../../src/sync/store.js';
import {
  insertMemory, memoryOf, openHome, publish, pull, REPO, revisionCount, SPACE, withHomes, withReplicas, type Replica,
} from '../helpers/sync.js';

const sorted = (list: readonly string[]): string[] => [...list].sort();
const selectedOf = (replica: Replica, origin: string): string | null => readOrigin(replica.db, origin)!.selected_head;

function conflictOf(replica: Replica, origin: string): string | undefined {
  const row = replica.db.prepare('SELECT status FROM sync_conflicts WHERE id = ?').get(`sync:${origin}`);
  return row === undefined ? undefined : String(row.status);
}

/** The head set the conflict report names, by the author of each head (contracts "Heads"). */
function conflictAuthors(replica: Replica, origin: string): string[] {
  const row = replica.db.prepare('SELECT remote_state_json FROM sync_conflicts WHERE id = ?').get(`sync:${origin}`)!;
  return sorted((JSON.parse(String(row.remote_state_json)) as { heads: { author: string }[] }).heads.map((head) => head.author));
}

/** Every stored revision as the wire carries it, plus whether this device holds its payload. */
function graphOf(replica: Replica): Record<string, unknown>[] {
  return replica.db.prepare(`SELECT revision_id, origin_id, kind, author, parents_json, control_json, natural_json,
    payload_hash, payload_json IS NULL AS identity_only FROM sync_revisions ORDER BY revision_id`).all().map((row) => ({ ...row }));
}

function holdsPayload(db: DatabaseSync, revision: string): boolean {
  return db.prepare('SELECT payload_json FROM sync_revisions WHERE revision_id = ?').get(revision)?.payload_json != null;
}

/** A context row whose memory is the lineage parent: what the 0005 trigger and `raiseToParents` walk. */
function linkSource(db: DatabaseSync, memoryId: string, parentMemoryId: string): void {
  db.prepare(`INSERT INTO memory_sources (memory_id, citation_kind, citation_value, source_agent, context_only, source_memory_id)
    VALUES (?, 'file_read', 'lineage', 'claude', 1, ?)`).run(memoryId, parentMemoryId);
}

function insertWork(db: DatabaseSync, contextId: string, workId: string): void {
  db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at)
    VALUES (?, ?, 'k1', '/work/sync', 1, 1)`).run(contextId, REPO);
  db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, purpose_sensitivity, state, created_at, updated_at)
    VALUES (?, ?, ?, 'A purpose', 'eligible', 'active', 1, 1)`).run(workId, REPO, contextId);
}

/** Like `publish`, but the caller needs the snapshot identity and the withheld counts too. */
function emit(replica: Replica, dir: string, name: string, classes: readonly Sensitivity[]): { path: string; snapshotId: string; withheld: number } {
  replica.db.exec('BEGIN IMMEDIATE');
  captureLocalChanges(replica.db, 100);
  replica.db.exec('COMMIT');
  const path = join(dir, name);
  const built = buildSnapshot(replica.db, { spaceId: SPACE, classes, now: 100, outputPath: path });
  return { path, snapshotId: built.snapshotId, withheld: built.withheld.memories };
}

// --- relay (contracts/sync.md "Verification": relay) ---

test('relay: a joining device stores the ancestor identity-only and the head with its payload, and A->B->C equals A->C', async () => {
  await withReplicas(4, (replicas, dir) => {
    const [a, b, c, d] = replicas as [Replica, Replica, Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'First body');
    publish(a, dir);
    a.db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run('m_one');
    const file = publish(a, dir);
    const origin = `${a.id}:m_one`;
    const r1 = headsOf(a.db, origin)[0]!;
    const r0 = readRevision(a.db, r1)!.parents[0]!;
    // Only r1's payload is in the file: C stores r0 as identity-only and r1 as the head.
    pull(c, a, file);
    assert.equal(holdsPayload(c.db, r0), false, 'r0 arrives identity-only');
    assert.equal(holdsPayload(c.db, r1), true);
    assert.deepEqual(headsOf(c.db, origin), [r1]);
    assert.equal(memoryOf(c, a, 'm_one').pinned_at, 5);
    assert.equal(memoryOf(c, a, 'm_one').body, 'First body');
    // The same knowledge relayed A->B->D leaves D with the same graph and the same row.
    pull(b, a, file);
    pull(d, b, publish(b, dir));
    assert.deepEqual(graphOf(d), graphOf(c));
    assert.deepEqual(headsOf(d.db, origin), [r1]);
    assert.deepEqual({ ...memoryOf(d, a, 'm_one') }, { ...memoryOf(c, a, 'm_one') });
  });
});

test('relay: siblings reach a third device through one bundle with both heads and the same conflict', async () => {
  await withReplicas(4, (replicas, dir) => {
    const [a, b, c, d] = replicas as [Replica, Replica, Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    pull(b, a, publish(a, dir));
    a.db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run('m_one');
    b.db.prepare('UPDATE memories SET valid_to = 9 WHERE id = ?').run(memoryOf(b, a, 'm_one').id as string);
    const fromA = publish(a, dir);
    const fromB = publish(b, dir);
    const origin = `${a.id}:m_one`;
    pull(b, a, fromA);
    const both = sorted(headsOf(b.db, origin));
    assert.equal(both.length, 2);
    // C pulls only B's bundle after B pulled A: the same two heads and the same conflict.
    const relayed = publish(b, dir);
    pull(c, b, relayed);
    assert.deepEqual(sorted(headsOf(c.db, origin)), both);
    assert.equal(conflictOf(c, origin), 'open');
    assert.deepEqual(conflictAuthors(c, origin), sorted([a.id, b.id]));
    assert.deepEqual(conflictAuthors(b, origin), sorted([a.id, b.id]));
    // A->B->C equals A->C: D takes both bundles straight from their authors.
    pull(d, a, fromA);
    pull(d, b, fromB);
    assert.deepEqual(graphOf(d), graphOf(c));
    assert.deepEqual(sorted(headsOf(d.db, origin)), both);
    assert.equal(conflictOf(d, origin), 'open');
  });
});

// --- partial descent (contracts/sync.md "Verification": partial descent) ---

test('partial descent: a1 replaces a in the head set, the conflict stays open, and the selected head moves only if a was selected', async () => {
  await withReplicas(4, (replicas, dir) => {
    const [a, b, c, d] = replicas as [Replica, Replica, Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    pull(b, a, publish(a, dir));
    a.db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run('m_one');
    b.db.prepare('UPDATE memories SET valid_to = 9 WHERE id = ?').run(memoryOf(b, a, 'm_one').id as string);
    const fromA = publish(a, dir);
    const fromB = publish(b, dir);
    const origin = `${a.id}:m_one`;
    const headA = headsOf(a.db, origin)[0]!;
    const headB = headsOf(b.db, origin)[0]!;
    // C selects A's line first, D selects B's line first; both hold {a, b}.
    pull(c, a, fromA); pull(c, b, fromB);
    pull(d, b, fromB); pull(d, a, fromA);
    for (const replica of [c, d]) assert.deepEqual(sorted(headsOf(replica.db, origin)), sorted([headA, headB]));
    assert.equal(selectedOf(c, origin), headA);
    assert.equal(selectedOf(d, origin), headB);
    // a1 descends from a alone.
    a.db.prepare('UPDATE memories SET pin_order = 2 WHERE id = ?').run('m_one');
    const withA1 = publish(a, dir);
    const headA1 = headsOf(a.db, origin)[0]!;
    assert.deepEqual(readRevision(a.db, headA1)!.parents, [headA]);
    pull(c, a, withA1); pull(d, a, withA1);
    for (const replica of [c, d]) {
      assert.deepEqual(sorted(headsOf(replica.db, origin)), sorted([headA1, headB]), 'a1 removed a and b stayed a head');
      assert.equal(conflictOf(replica, origin), 'open', 'the conflict stays open');
    }
    assert.equal(selectedOf(c, origin), headA1, 'a was selected, so the selection descends to a1');
    assert.equal(memoryOf(c, a, 'm_one').pin_order, 2);
    assert.equal(selectedOf(d, origin), headB, 'b was selected, so the selection does not move');
    assert.equal(memoryOf(d, a, 'm_one').valid_to, 9);
    assert.equal(memoryOf(d, a, 'm_one').pinned_at, null);
  });
});

// --- foreign edit (contracts/sync.md "Verification": foreign edit) ---

test('foreign edit: B edits, deletes and marks secret a memory A created, and the erased text is never published again', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Original body');
    const seed = publish(a, dir);
    pull(b, a, seed); pull(c, a, seed);
    const origin = `${a.id}:m_one`;
    const localOnB = memoryOf(b, a, 'm_one').id as string;
    // B edits A's memory (text is immutable identity, so the edit is a pin); the successor reaches
    // A and C along A's own line.
    b.db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run(localOnB);
    const edited = publish(b, dir);
    pull(a, b, edited); pull(c, b, edited);
    for (const replica of [a, c]) {
      assert.equal(memoryOf(replica, a, 'm_one').pinned_at, 5);
      assert.equal(headsOf(replica.db, origin).length, 1, 'a descendant of the local line is no conflict');
    }
    // B then deletes it and marks it secret; both successors reach A and C.
    b.db.prepare('UPDATE memories SET deleted_at = 7 WHERE id = ?').run(localOnB);
    b.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = ?").run(localOnB);
    const terminal = publish(b, dir);
    pull(a, b, terminal); pull(c, b, terminal);
    for (const replica of [a, c]) {
      const row = memoryOf(replica, a, 'm_one');
      assert.equal(row.sensitivity, 'secret');
      assert.equal(row.body, '');
      assert.equal(row.title, '');
      assert.notEqual(row.deleted_at, null);
      assert.equal(effectiveControl(replica.db, origin).tombstone, true);
      assert.equal(replica.db.prepare('SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ? AND payload_json IS NOT NULL')
        .get(origin)?.n, 0, 'every stored payload of the origin is erased');
    }
    // And nothing re-publishes it: no bundle any replica writes carries either text.
    for (const replica of [a, b, c]) {
      const text = readFileSync(publish(replica, dir), 'utf8');
      assert.equal(text.includes('Original body'), false, 'the erased text is never re-published');
      assert.equal(text.includes('Original body'), false);
    }
  });
});

// --- change pass (contracts/sync.md "Verification": change pass) ---

test('change pass: a completed work is a head before the pull, so a descendant of the older head is a sibling', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertWork(a.db, 'ctx_one', 'w_one');
    pull(b, a, publish(a, dir));
    const origin = `${a.id}:w_one`;
    const w0 = headsOf(a.db, origin)[0]!;
    // B descends from the older head while A completes the work locally.
    b.db.prepare("UPDATE work_items SET purpose = 'B purpose', updated_at = 20 WHERE id = ?")
      .run(readOrigin(b.db, origin)!.local_id as string);
    const fromB = publish(b, dir);
    a.db.prepare("UPDATE work_items SET state = 'completed', completed_at = 30, updated_at = 30 WHERE id = 'w_one'").run();
    // A pulls before pushing: the change pass runs first, so the completion is already a head.
    pull(a, b, fromB);
    const heads = headsOf(a.db, origin);
    assert.equal(heads.length, 2, 'B is a sibling of the completion, not its replacement');
    const completion = heads.find((head) => readRevision(a.db, head)!.author === a.id)!;
    assert.deepEqual(readRevision(a.db, completion)!.parents, [w0]);
    assert.equal(conflictOf(a, origin), 'open');
    assert.equal(selectedOf(a, origin), completion);
    const row = a.db.prepare('SELECT state, purpose FROM work_items WHERE id = ?').get('w_one')!;
    assert.equal(row.state, 'completed');
    assert.equal(row.purpose, 'A purpose', "B's purpose did not replace the completion");
  });
});

test('change pass: a raise the 0005 trigger applies during a pull is recorded before commit and shipped by the next push', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    insertMemory(a.db, 'm_p', 'Parent', 'Parent body');
    const seed = publish(a, dir);
    pull(b, a, seed); pull(c, a, seed);
    // B holds D under P; C captured the same D independently, so both origins name one row on B.
    for (const replica of [b, c]) {
      insertMemory(replica.db, 'm_d', 'Child', 'Child body');
      linkSource(replica.db, 'm_d', memoryOf(replica, a, 'm_p').id as string);
    }
    pull(b, c, publish(c, dir));
    assert.equal(canonicalOf(b.db, `${c.id}:m_d`).local_id, 'm_d', 'the same material aliases onto one row');
    const child = canonicalOf(b.db, `${b.id}:m_d`).origin_id;
    const beforeHead = headsOf(b.db, child);
    // A marks P secret. B's pull raises D through the trigger and records the raise before commit.
    a.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = 'm_p'").run();
    pull(b, a, publish(a, dir));
    assert.equal(memoryOf(b, b, 'm_d').sensitivity, 'secret');
    const raise = headsOf(b.db, child).find((head) => !beforeHead.includes(head))!;
    assert.ok(raise !== undefined, 'the raise is a new revision of the raised origin');
    assert.equal(readRevision(b.db, raise)!.author, b.id);
    assert.equal(readRevision(b.db, raise)!.control.sensitivity_floor, 'secret');
    // The next push ships it: C follows without ever reading A's bundle.
    pull(c, b, publish(b, dir));
    assert.equal(effectiveControl(c.db, canonicalOf(c.db, `${c.id}:m_d`).origin_id).sensitivity_floor, 'secret');
    assert.equal(memoryOf(c, c, 'm_d').sensitivity, 'secret');
    assert.equal(memoryOf(c, c, 'm_d').body, '');
    assert.equal(readRevision(c.db, raise)!.author, b.id, 'the raise travels as a revision, not as a local edit');
  });
});

// --- late child (contracts/sync.md "Verification": late child) ---

test('late child: a descendant arriving after its parent secret floor is stored secret without text, and the raise reaches a third device', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    insertMemory(a.db, 'm_p', 'Parent', 'Parent body');
    const seed = publish(a, dir);
    pull(b, a, seed); pull(c, a, seed);
    // C captures D under P. B applies P's secret floor before D ever exists there.
    insertMemory(c.db, 'm_d', 'Child', 'Child body');
    linkSource(c.db, 'm_d', memoryOf(c, a, 'm_p').id as string);
    a.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = 'm_p'").run();
    pull(b, a, publish(a, dir));
    assert.equal(effectiveControl(b.db, `${a.id}:m_p`).sensitivity_floor, 'secret');
    // The trigger cannot fire for a child that arrives later, so the apply walks the lineage.
    pull(b, c, publish(c, dir));
    const stored = memoryOf(b, c, 'm_d');
    assert.equal(stored.sensitivity, 'secret');
    assert.equal(stored.body, '');
    assert.equal(stored.title, '');
    // The raise is a control revision, so a device that holds neither A's nor C's bundle follows.
    const child = canonicalOf(b.db, `${c.id}:m_d`).origin_id;
    const raise = headsOf(b.db, child)[0]!;
    assert.equal(readRevision(b.db, raise)!.author, b.id);
    assert.equal(readRevision(b.db, raise)!.control.sensitivity_floor, 'secret');
    pull(a, b, publish(b, dir));
    assert.equal(effectiveControl(a.db, child).sensitivity_floor, 'secret');
    assert.equal(a.db.prepare("SELECT COUNT(*) AS n FROM memories WHERE title = 'Child'").get()?.n, 0,
      "D's text never reached A");
  });
});

// --- change-free round trip and the push windows (contracts/sync.md "Push" steps 1-4) ---

test('change-free round trip records nothing, and a deletion at any push window restarts the push and ships its control', async () => {
  await withHomes(2, (homes, shared) => {
    const [homeA, homeB] = homes as [string, string];
    const a = openHome(homeA);
    const b = openHome(homeB);
    try {
      const pathsA = oboetePaths(homeA);
      const pathsB = oboetePaths(homeB);
      const { keyLine } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      const A: Replica = { db: a, home: homeA, id: replicaOriginId(a) };
      const B: Replica = { db: b, home: homeB, id: replicaOriginId(b) };
      for (const name of ['m_capture', 'm_staging', 'm_unchanged']) insertMemory(a, name, `T ${name}`, `Body ${name}`);
      assert.equal(pushSpace(a, pathsA, { now: 10 }).outcome, 'published');
      assert.deepEqual(pullSpace(b, pathsB, { now: 11 }).bundles.map((entry) => entry.outcome), ['applied']);
      // A pushes, B pulls and pushes, A pulls: no new revision anywhere and nothing left to push.
      const counts = [revisionCount(a), revisionCount(b)];
      assert.equal(pushSpace(b, pathsB, { now: 12 }).outcome, 'published');
      assert.deepEqual(pullSpace(a, pathsA, { now: 13 }).bundles.map((entry) => entry.outcome), ['applied']);
      assert.deepEqual([revisionCount(a), revisionCount(b)], counts, 'an applied revision leaves the row where its payload says');
      assert.equal(pushSpace(a, pathsA, { now: 14 }).outcome, 'unchanged');
      const other = openDatabase({ path: pathsA.db, timeoutMs: 1000 }).db;
      try {
        // A deletion committed between the change pass and step 2, and one committed during
        // staging, each make data_version differ: the push restarts and ships the tombstone.
        for (const [name, step] of [['m_capture', 'after_capture'], ['m_staging', 'after_staging']] as const) {
          let fired = false;
          const push = pushSpace(a, pathsA, { now: 20, probe: (at) => {
            if (at !== step || fired) return;
            fired = true;
            other.prepare('UPDATE memories SET deleted_at = 7 WHERE id = ?').run(name);
          } });
          assert.equal(push.outcome, 'published', step);
          assert.equal(push.restarts, 1, `${step} restarted once`);
          pullSpace(b, pathsB, { now: 21 });
          assert.equal(effectiveControl(b, `${A.id}:${name}`).tombstone, true, `${step}: the tombstone reached B`);
          assert.notEqual(memoryOf(B, A, name).deleted_at, null);
          assert.equal(memoryOf(B, A, name).body, '');
        }
        // The same during the staging of a push that would otherwise exit "unchanged".
        assert.equal(pushSpace(a, pathsA, { now: 29 }).outcome, 'unchanged');
        let fired = false;
        const push = pushSpace(a, pathsA, { now: 30, probe: (at) => {
          if (at !== 'after_staging' || fired) return;
          fired = true;
          other.prepare("UPDATE memories SET deleted_at = 9 WHERE id = 'm_unchanged'").run();
        } });
        assert.equal(push.outcome, 'published', 'the deletion is never hidden behind a matching snapshot');
        assert.equal(push.restarts, 1);
        pullSpace(b, pathsB, { now: 31 });
        assert.equal(effectiveControl(b, `${A.id}:m_unchanged`).tombstone, true);
        assert.notEqual(memoryOf(B, A, 'm_unchanged').deleted_at, null);
      } finally { other.close(); }
    } finally { a.close(); b.close(); }
  });
});

// --- delivery identity (contracts/sync.md "Verification": delivery identity) ---

test('delivery identity: widening the classes and newly holding a payload change snapshot_id, and a late payload fills a null-payload revision unless the origin is terminal', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    for (const name of ['m_open', 'm_dead', 'm_secret']) insertMemory(a.db, name, `T ${name}`, `Body ${name}`, { sensitivity: 'local_only' });
    const narrow = emit(a, dir, 'a-narrow.plain', ['eligible']);
    const wide = emit(a, dir, 'a-wide.plain', ['eligible', 'local_only']);
    assert.equal(narrow.withheld, 3);
    assert.equal(wide.withheld, 0);
    assert.notEqual(wide.snapshotId, narrow.snapshotId, 'widening the consent classes changes snapshot_id');
    // C holds all three payloads; B gets the identity-only lines of the narrow bundle.
    pull(c, a, wide.path);
    const applied = pull(b, a, narrow.path);
    assert.equal(applied.filled, 0);
    const ids = ['m_open', 'm_dead', 'm_secret'].map((name) => headsOf(b.db, `${a.id}:${name}`)[0]!);
    for (const revision of ids) assert.equal(holdsPayload(b.db, revision), false);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
    // A deletes one and marks another secret; B stores both controls.
    a.db.prepare('UPDATE memories SET deleted_at = 50 WHERE id = ?').run('m_dead');
    a.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = ?").run('m_secret');
    pull(b, a, emit(a, dir, 'a-control.plain', ['eligible']).path);
    assert.equal(effectiveControl(b.db, `${a.id}:m_dead`).tombstone, true);
    assert.equal(effectiveControl(b.db, `${a.id}:m_secret`).sensitivity_floor, 'secret');
    const before = emit(b, dir, 'b-before.plain', ['eligible', 'local_only', 'private']).snapshotId;
    // C's bundle still carries all three payloads: only the live origin is filled in.
    const late = pull(b, c, publish(c, dir));
    assert.equal(late.filled, 1);
    assert.equal(holdsPayload(b.db, ids[0]!), true, 'a known null-payload revision is filled in');
    assert.equal(holdsPayload(b.db, ids[1]!), false, 'a tombstoned origin is never filled in');
    assert.equal(holdsPayload(b.db, ids[2]!), false, 'a secret floor is never filled in');
    assert.equal(memoryOf(b, a, 'm_open').body, 'Body m_open');
    assert.equal(readOrigin(b.db, `${a.id}:m_dead`)!.local_id, null);
    assert.equal(readOrigin(b.db, `${a.id}:m_secret`)!.local_id, null);
    const after = emit(b, dir, 'b-after.plain', ['eligible', 'local_only', 'private']).snapshotId;
    assert.notEqual(after, before, 'a payload newly held changes snapshot_id');
  });
});

test('delivery identity: map-repo applies the withheld payloads without a pull and --republish rewrites the same delivery', async () => {
  await withHomes(2, (homes, shared) => {
    const [homeA, homeB] = homes as [string, string];
    const a = openHome(homeA);
    const b = openHome(homeB);
    try {
      const pathsA = oboetePaths(homeA);
      const pathsB = oboetePaths(homeB);
      // The repository is a path on one device, so its key never resolves by itself.
      a.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/a/.git' WHERE id = ?").run(REPO);
      b.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/b/.git' WHERE id = ?").run(REPO);
      const { keyLine } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      const originA = replicaOriginId(a);
      insertMemory(a, 'm_one', 'Title', 'Body text');
      assert.equal(pushSpace(a, pathsA, { now: 10 }).outcome, 'published');
      pullSpace(b, pathsB, { now: 11 });
      assert.equal(readOrigin(b, `${originA}:m_one`)!.local_id, null);
      assert.equal(readOrigin(b, `${originA}:m_one`)!.withheld_reason, 'unmapped_repo');
      assert.equal(b.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
      const published = pushSpace(b, pathsB, { now: 12 });
      assert.equal(published.outcome, 'published');
      // The mapping re-evaluates the stored revisions without any new pull.
      const key = repoKeyFor(originA, 'common_dir', '/work/a/.git');
      assert.equal(mapRepo(b, pathsB, { repoKey: key, localRepoId: REPO, now: 13 }).reapplied, 1);
      assert.equal(readOrigin(b, `${originA}:m_one`)!.withheld_reason, null);
      assert.equal(b.prepare('SELECT body, repo_id FROM memories').get()?.body, 'Body text');
      // The mapping changes how this replica reads, not what it publishes: the same lines give the
      // same snapshot_id, and --republish still rewrites the file under a new salt.
      const bundle = join(spaceDirectory(shared, loadSyncConfig(pathsB)!.space_id), `${replicaOriginId(b)}.osb`);
      const bytesBefore = readFileSync(bundle);
      const republished = pushSpace(b, pathsB, { now: 14, republish: true });
      assert.equal(republished.outcome, 'published');
      assert.equal(republished.snapshotId, published.snapshotId);
      assert.notDeepEqual(readFileSync(bundle), bytesBefore, 'the bundle was rewritten');
    } finally { a.close(); b.close(); }
  });
});
