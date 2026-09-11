// US6 cases from the correctness review of the sync module (contracts/sync.md "Merge rules",
// "Repository identity", "Push", "Pull"): pre-sync rows under other ids, control-only heads as
// the base for local changes, in-place source changes, checkpoint identity, ownership, relative
// directories, the revision bound over a re-sent log, and a publish failure that leaves nothing.
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { checkpointHash, materialHash } from '../../src/db/identity.js';
import { oboetePaths } from '../../src/paths.js';
import { captureLocalChanges } from '../../src/sync/capture.js';
import { initSpace, joinSpace, mapRepo, pullSpace, pushSpace, SyncError, syncPaths } from '../../src/sync/space.js';
import { headsOf, readOrigin, readRevision, replicaOriginId, repoKeyFor, revisionsOfOrigin } from '../../src/sync/store.js';
import {
  insertMemory, insertSource, memoryOf, openHome, publish, pull, REPO, revisionCount, withHomes, withReplicas, type Replica,
} from '../helpers/sync.js';

function seedWork(db: DatabaseSync, work = 'w_one', context = 'ctx_one', repo = REPO): void {
  db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at) VALUES (?, ?, ?, '/work/sync', 1, 1)`)
    .run(context, repo, context);
  db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, purpose_sensitivity, state, created_at, updated_at)
    VALUES (?, ?, ?, 'Ship US6', 'eligible', 'active', 1, 1)`).run(work, repo, context);
}

function conflicts(db: DatabaseSync): number {
  return Number(db.prepare("SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'").get()?.n);
}

test('a pulled grant lands on the row a 0006 migration created under another id, and no tombstone follows', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    a.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v_a', 'm_one', 'project', ?, NULL, NULL, 'observer', 1)`).run(REPO);
    // B already holds the same memory and the same scope under the migration's id.
    insertMemory(b.db, 'm_b', 'Title', 'Body text');
    b.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v_migration:m_b', 'm_b', 'project', ?, NULL, NULL, 'migration', 1)`).run(REPO);
    pull(b, a, publish(a, dir));
    assert.equal(readOrigin(b.db, `${a.id}:v_a`)!.local_id, 'v_migration:m_b', 'the origin binds to the row the scope resolves to');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_visibility').get()?.n, 1);
    publish(b, dir);
    assert.equal(revisionsOfOrigin(b.db, `${a.id}:v_a`).some((revision) => revision.control.tombstone), false, 'nothing is tombstoned');
    pull(a, b, `${dir}/${b.id}.plain`);
    assert.equal(a.db.prepare('SELECT COUNT(*) AS n FROM memory_visibility').get()?.n, 1, "A's grant survives B's push");
  });
});

test('a control-only head is the base of the next local change: a pin after a pulled floor is one successor, not a sibling', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    pull(b, a, publish(a, dir));
    a.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = 'm_one'").run();
    pull(b, a, publish(a, dir));
    const origin = `${a.id}:m_one`;
    const floor = headsOf(b.db, origin);
    assert.equal(readRevision(b.db, floor[0]!)!.payload, null, 'the head carries no payload under the secret floor');
    assert.equal(readOrigin(b.db, origin)!.materialized_revision, floor[0], 'the applied control is the materialized base');
    b.db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run(memoryOf(b, a, 'm_one').id as string);
    b.db.exec('BEGIN IMMEDIATE'); captureLocalChanges(b.db, 30); b.db.exec('COMMIT');
    const heads = headsOf(b.db, origin);
    assert.equal(heads.length, 1);
    assert.deepEqual(readRevision(b.db, heads[0]!)!.parents, floor);
    pull(a, b, publish(b, dir));
    assert.equal(conflicts(a.db), 0);
    assert.equal(headsOf(a.db, origin).length, 1, 'the successor is one line on A as well');
    // Payloads under a secret floor are erased on every replica, so the pin itself stays on B.
    assert.equal(a.db.prepare("SELECT pinned_at FROM memories WHERE id = 'm_one'").get()?.pinned_at, null);
  });
});

test('an in-place source field change ships as a new origin plus a tombstone, and the receiver keeps one row', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    const rowid = insertSource(a.db, 'm_one', 'src/a.ts');
    a.db.prepare('UPDATE memory_sources SET source_hash = ?, portion_start = 0, portion_end = 10, source_total = 10 WHERE id = ?').run('1'.repeat(64), rowid);
    pull(b, a, publish(a, dir));
    // worker/imported.ts clears source_agent in place: the local id of the source changes.
    a.db.prepare('UPDATE memory_sources SET source_agent = NULL WHERE id = ?').run(rowid);
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0);
    const rows = b.db.prepare('SELECT source_agent FROM memory_sources').all();
    assert.deepEqual(rows.map((row) => row.source_agent), [null], 'one row, with the new value');
    const origins = b.db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'source' ORDER BY origin_id").all();
    assert.equal(origins.length, 2);
    // The old origin carries a tombstone on B too, so nothing resurrects it; the new one is live.
    const tombstoned = origins.filter((origin) => headsOf(b.db, String(origin.origin_id))
      .some((head) => readRevision(b.db, head)!.control.tombstone));
    assert.equal(tombstoned.length, 1);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1);
  });
});

test('a pulled checkpoint carries the checkpoint identity, not the ordinary content hash', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    seedWork(a.db);
    const material = materialHash('Base', 'Base body');
    const content = checkpointHash(REPO, 'w_one', null, material);
    a.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash, sensitivity,
      review_state, created_at, work_id, checkpoint_parent_id) VALUES ('m_c0', ?, 'session_summary', 'Base', 'Base body', '[]', '', ?, ?, 'eligible', 'reviewed', 1, 'w_one', NULL)`)
      .run(REPO, material, content);
    a.db.prepare("UPDATE work_items SET current_checkpoint_memory_id = 'm_c0' WHERE id = 'w_one'").run();
    pull(b, a, publish(a, dir));
    const row = memoryOf(b, a, 'm_c0');
    const work = readOrigin(b.db, `${a.id}:w_one`)!.local_id!;
    assert.equal(row.content_hash, checkpointHash(REPO, work, null, material));
    assert.notEqual(row.content_hash, materialHash('Base', 'Base body'));
  });
});

test('a proposal that arrived withheld aliases onto the equivalent local proposal once its repository is mapped', async () => {
  await withHomes(2, (homes, shared) => {
    const [homeA, homeB] = homes as [string, string];
    const a = openHome(homeA);
    const b = openHome(homeB);
    try {
      const pathsA = oboetePaths(homeA);
      const pathsB = oboetePaths(homeB);
      a.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/a/.git' WHERE id = ?").run(REPO);
      b.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/b/.git' WHERE id = ?").run(REPO);
      const { keyLine } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      for (const db of [a, b]) {
        seedWork(db);
        insertMemory(db, 'm_origin', 'Origin', 'Origin body');
        db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title, candidate_body,
          candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, created_at)
          VALUES (?, 'm_origin', ?, 'w_one', 'Candidate', 'Candidate body', ?, 'eligible', '[]', 'inferred', 'pending', 1)`)
          .run(db === a ? 'sp_a' : 'sp_b', REPO, materialHash('Candidate', 'Candidate body'));
      }
      pushSpace(a, pathsA, { now: 10 });
      pullSpace(b, pathsB, { now: 11 });
      assert.notEqual(readOrigin(b, `${replicaOriginId(a)}:sp_a`)!.withheld_reason, null, 'withheld until the repository is mapped');
      mapRepo(b, pathsB, { repoKey: repoKeyFor(replicaOriginId(a), 'common_dir', '/work/a/.git'), localRepoId: REPO, now: 12 });
      assert.equal(readOrigin(b, `${replicaOriginId(a)}:sp_a`)!.local_id, 'sp_b', 'the origin aliases onto the local proposal');
      assert.equal(b.prepare('SELECT COUNT(*) AS n FROM sharing_proposals').get()?.n, 1);
    } finally { a.close(); b.close(); }
  });
});

test('a record whose repository differs from its parent is withheld as repo_mismatch and never written', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    a.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('repo_two', 'remote', 'github.com/example/two', '/work/two', 1, 1)`).run();
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    seedWork(a.db, 'w_two', 'ctx_two', 'repo_two');
    // A grant on a repo_one memory scoped to repo_two, and a work in repo_two whose context is in repo_one.
    a.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v_cross', 'm_one', 'project', 'repo_two', NULL, NULL, 'observer', 1)`).run();
    seedWork(a.db, 'w_cross_context_owner', 'ctx_one');
    a.db.prepare("UPDATE work_items SET origin_context_id = 'ctx_one' WHERE id = 'w_two'").run();
    pull(b, a, publish(a, dir));
    assert.equal(readOrigin(b.db, `${a.id}:v_cross`)!.withheld_reason, 'repo_mismatch');
    assert.equal(readOrigin(b.db, `${a.id}:w_two`)!.withheld_reason, 'repo_mismatch');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_visibility').get()?.n, 0);
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM work_items WHERE repo_id = 'repo_two'").get()?.n, 0);
    assert.equal(memoryOf(b, a, 'm_one').body, 'Body text', 'the well-formed rows still apply');
  });
});

test('a sync directory given as a relative path is recorded absolute, so a push from another directory finds it', async () => {
  await withHomes(1, async (homes, shared) => {
    const [home] = homes as [string];
    const db = openHome(home);
    const cwd = process.cwd();
    try {
      const paths = oboetePaths(home);
      process.chdir(home);
      initSpace(db, paths, { directory: relative(home, shared), classes: ['eligible'], now: 1 });
      process.chdir(cwd);
      insertMemory(db, 'm_one', 'Title', 'Body text');
      assert.equal(pushSpace(db, paths, { now: 10 }).outcome, 'published');
    } finally { process.chdir(cwd); db.close(); }
  });
});

test('the per-origin revision bound counts a re-sent log once', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    // 2,100 pin toggles: more than half the bound, so a doubled count would reject the second pull.
    for (let i = 0; i < 2_100; i += 1) {
      a.db.prepare('UPDATE memories SET pinned_at = ?, pin_order = ? WHERE id = ?').run(i % 2 === 0 ? i + 1 : null, i % 2 === 0 ? 1 : null, 'm_one');
      a.db.exec('BEGIN IMMEDIATE'); captureLocalChanges(a.db, i); a.db.exec('COMMIT');
    }
    assert.equal(revisionCount(a.db), 2_100);
    pull(b, a, publish(a, dir));
    assert.equal(revisionCount(b.db), 2_100);
    a.db.prepare('UPDATE memories SET pinned_at = 9_999 WHERE id = ?').run('m_one');
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.stored, 1, 'only the new revision is stored');
    assert.equal(revisionCount(b.db), 2_101);
  });
});

test('a snapshot that cannot be built fails with a coded error, leaves the connection and the staging directory clean', async () => {
  await withHomes(1, (homes, shared) => {
    const [home] = homes as [string];
    const db = openHome(home);
    try {
      const paths = oboetePaths(home);
      initSpace(db, paths, { directory: shared, classes: ['eligible'], now: 1 });
      insertMemory(db, 'm_big', 'Title', 'x'.repeat(5 * 1024 * 1024));
      assert.throws(() => pushSpace(db, paths, { now: 10 }), (error: unknown) => error instanceof SyncError && error.code === 'publish_failed'
        && error.detail.code === 'line_too_long');
      db.exec('BEGIN IMMEDIATE'); db.exec('COMMIT');
      assert.deepEqual(readdirSync(syncPaths(paths).staging), [], 'no plaintext is left behind');
    } finally { db.close(); }
  });
});

test('the security-owned sync sources are text: no NUL byte, so every diff reviewer sees them', () => {
  for (const name of ['apply', 'capture', 'envelope', 'format', 'identity', 'publish', 'space', 'stage', 'status', 'store']) {
    const source = readFileSync(join(process.cwd(), 'src', 'sync', `${name}.ts`), 'utf8');
    assert.equal(source.includes('\u0000'), false, name);
  }
});
