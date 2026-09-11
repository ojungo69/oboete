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
import { sha256Json } from '../../src/hash.js';
import { oboetePaths } from '../../src/paths.js';
import { resolveRow } from '../../src/sync/apply.js';
import { captureLocalChanges, sourceLocalId } from '../../src/sync/capture.js';
import { initSpace, joinSpace, mapRepo, pullSpace, pushSpace, SyncError, syncPaths } from '../../src/sync/space.js';
import { BundleRejected } from '../../src/sync/stage.js';
import { effectiveControl, headsOf, readOrigin, readRevision, replicaOriginId, repoKeyFor, revisionsOfOrigin } from '../../src/sync/store.js';
import {
  insertMemory, insertSource, memoryOf, openHome, publish, pull, REPO, revisionCount, withHomes, withReplicas, type Replica,
} from '../helpers/sync.js';

function seedWork(db: DatabaseSync, work = 'w_one', context = 'ctx_one', repo = REPO): void {
  db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at) VALUES (?, ?, ?, '/work/sync', 1, 1)`)
    .run(context, repo, context);
  db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, purpose_sensitivity, state, created_at, updated_at)
    VALUES (?, ?, ?, 'Ship US6', 'eligible', 'active', 1, 1)`).run(work, repo, context);
}

function seedProposal(db: DatabaseSync, id: string, memory: string): void {
  db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title, candidate_body,
    candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, created_at)
    VALUES (?, ?, ?, 'w_one', 'Candidate', 'Candidate body', ?, 'eligible', '[]', 'inferred', 'pending', 1)`)
    .run(id, memory, REPO, materialHash('Candidate', 'Candidate body'));
}

function seedCheckpoint(db: DatabaseSync, id: string, work: string, title: string): void {
  const material = materialHash(title, `${title} body`);
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash, sensitivity,
    review_state, created_at, work_id, checkpoint_parent_id) VALUES (?, ?, 'session_summary', ?, ?, '[]', '', ?, ?, 'eligible', 'reviewed', 1, ?, NULL)`)
    .run(id, REPO, title, `${title} body`, material, checkpointHash(REPO, work, null, material), work);
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

// --- round two ---

test('context-only and citation sources with NULL keys are distinct rows on every device', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    seedWork(a.db);
    for (const root of ['/work/one', '/work/two']) {
      a.db.prepare(`INSERT INTO memory_sources (memory_id, citation_kind, citation_value, source_agent, capture_root, source_paths_json, source_context_id, context_only)
        VALUES ('m_one', 'file_read', NULL, 'claude', ?, '["a.ts"]', 'ctx_one', 1)`).run(root);
    }
    insertSource(a.db, 'm_one', 'src/a.ts');
    insertSource(a.db, 'm_one', 'src/b.ts');
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 4);
    pull(a, b, publish(b, dir));
    assert.equal(a.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 4, 'nothing was tombstoned');
    assert.equal(a.db.prepare("SELECT COUNT(*) AS n FROM sync_revisions WHERE kind = 'source' AND author = ?").get(b.id)?.n, 0);
  });
});

test('references follow the natural key: an alias flip and a mapped repository do not make a device reject its peer', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [x, y] = replicas as [Replica, Replica];
    const [lo, hi] = [x, y].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    // Both devices hold the same memory from before sync; only hi has a source on it.
    insertMemory(lo.db, 'm_one', 'Title', 'Body text');
    insertMemory(hi.db, 'm_one', 'Title', 'Body text');
    insertSource(hi.db, 'm_one', 'src/a.ts');
    hi.db.exec('BEGIN IMMEDIATE'); captureLocalChanges(hi.db, 5); hi.db.exec('COMMIT');
    pull(hi, lo, publish(lo, dir));
    assert.equal(readOrigin(hi.db, `${hi.id}:m_one`)!.canonical_origin_id, `${lo.id}:m_one`, 'the canonical origin flipped');
    // hi's next push names the memory as its source's natural key does (hi:m_one), not as the
    // flipped canonical origin: lo accepts the bundle and applies the source onto its own row.
    const result = pull(lo, hi, publish(hi, dir));
    assert.equal(result.withheldOnApply, 0);
    assert.equal(lo.db.prepare("SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = 'm_one'").get()?.n, 1, 'the source applied on lo');
    // The two independent captures of one material stay a two-head conflict until a resolve (alias rule).
    assert.equal(headsOf(lo.db, `${lo.id}:m_one`).length, 2);
  });
});

test('a local change on a mapped repository row publishes under the natural key and applies on the owner', async () => {
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
      insertMemory(a, 'm_one', 'Title', 'Body text');
      pushSpace(a, pathsA, { now: 10 });
      pullSpace(b, pathsB, { now: 11 });
      mapRepo(b, pathsB, { repoKey: repoKeyFor(replicaOriginId(a), 'common_dir', '/work/a/.git'), localRepoId: REPO, now: 12 });
      const local = readOrigin(b, `${replicaOriginId(a)}:m_one`)!.local_id!;
      b.prepare('UPDATE memories SET pinned_at = 7, pin_order = 1 WHERE id = ?').run(local);
      assert.equal(pushSpace(b, pathsB, { now: 13 }).outcome, 'published');
      const pulled = pullSpace(a, pathsA, { now: 14 }).bundles[0]!;
      assert.equal(pulled.outcome, 'applied', String(pulled.reason));
      assert.equal(a.prepare("SELECT pinned_at FROM memories WHERE id = 'm_one'").get()?.pinned_at, 7);
    } finally { a.close(); b.close(); }
  });
});

test('a dependency edge to a personal projection of another repository applies and raises the child', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    a.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('repo_two', 'remote', 'github.com/example/two', '/work/two', 1, 1)`).run();
    b.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('repo_two', 'remote', 'github.com/example/two', '/work/two', 1, 1)`).run();
    insertMemory(a.db, 'm_parent', 'Parent', 'Parent body', { sensitivity: 'private' });
    a.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity, review_state, created_at)
      VALUES ('m_child', 'repo_two', 'discovery', 'Child', 'Child body', '', ?, ?, 'eligible', 'reviewed', 1)`)
      .run(materialHash('Child', 'Child body'), sha256Json(['repo_two', materialHash('Child', 'Child body')]));
    a.db.prepare(`INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_child', 'file_read', 'm_parent', 1)`).run();
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0, 'the edge is not an ownership violation');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE source_memory_id IS NOT NULL').get()?.n, 1);
    assert.equal(memoryOf(b, a, 'm_child').sensitivity, 'private', 'lineage inheritance raised the child');
  });
});

test('a personal projection whose text does not hash to its identity is rejected before apply', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    seedWork(a.db);
    insertMemory(a.db, 'm_origin', 'Origin', 'Origin body');
    a.db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title, candidate_body,
      candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, created_at)
      VALUES ('p_one', 'm_origin', ?, 'w_one', 'Candidate', 'Candidate body', ?, 'eligible', '[]', 'inferred', 'pending', 1)`)
      .run(REPO, materialHash('Candidate', 'Candidate body'));
    const identity = sha256Json(['personal-projection-v1', 'Candidate', 'Candidate body']);
    insertMemory(a.db, 'm_proj', 'Evil', 'Evil body', { content: identity });
    a.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v_proj', 'm_proj', 'personal', NULL, NULL, 'p_one', 'proposal_approval', 2)`).run();
    assert.throws(() => pull(b, a, publish(a, dir)), (error: unknown) => error instanceof BundleRejected && error.code === 'personal_identity_mismatch');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
  });
});

test('a payload released by a mapping meets the terminal control of the row it aliases onto', async () => {
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
      insertMemory(a, 'm_one', 'Title', 'Body text');
      // B already holds the same material and has marked it secret.
      insertMemory(b, 'm_b', 'Title', 'Body text', { sensitivity: 'secret' });
      b.prepare("UPDATE memories SET title = '', body = '' WHERE id = 'm_b'").run();
      pushSpace(b, pathsB, { now: 5 });
      pushSpace(a, pathsA, { now: 10 });
      pullSpace(b, pathsB, { now: 11 });
      assert.equal(readOrigin(b, `${replicaOriginId(a)}:m_one`)!.local_id, null, 'withheld until mapped');
      mapRepo(b, pathsB, { repoKey: repoKeyFor(replicaOriginId(a), 'common_dir', '/work/a/.git'), localRepoId: REPO, now: 12 });
      const row = b.prepare("SELECT sensitivity, body FROM memories WHERE id = 'm_b'").get()!;
      assert.deepEqual({ ...row }, { sensitivity: 'secret', body: '' }, 'the aliased payload never lowers the row');
      assert.equal(readOrigin(b, `${replicaOriginId(a)}:m_one`)!.local_id, 'm_b');
    } finally { a.close(); b.close(); }
  });
});

test('a secret floor reaches a work and a proposal that ship identity-only', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    seedWork(a.db);
    insertMemory(a.db, 'm_origin', 'Origin', 'Origin body');
    a.db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title, candidate_body,
      candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, created_at)
      VALUES ('p_one', 'm_origin', ?, 'w_one', 'Candidate', 'Candidate body', ?, 'eligible', '[]', 'inferred', 'pending', 1)`)
      .run(REPO, materialHash('Candidate', 'Candidate body'));
    pull(b, a, publish(a, dir));
    const work = readOrigin(b.db, `${a.id}:w_one`)!.local_id!;
    const proposal = readOrigin(b.db, `${a.id}:p_one`)!.local_id!;
    assert.equal(b.db.prepare('SELECT purpose FROM work_items WHERE id = ?').get(work)?.purpose, 'Ship US6');
    a.db.prepare("UPDATE work_items SET purpose_sensitivity = 'secret', purpose = NULL WHERE id = 'w_one'").run();
    a.db.prepare("UPDATE sharing_proposals SET candidate_sensitivity = 'secret', candidate_title = '', candidate_body = '' WHERE id = 'p_one'").run();
    pull(b, a, publish(a, dir));
    assert.deepEqual({ ...b.db.prepare('SELECT purpose, purpose_sensitivity FROM work_items WHERE id = ?').get(work) }, { purpose: null, purpose_sensitivity: 'secret' });
    assert.deepEqual({ ...b.db.prepare('SELECT candidate_title, candidate_body, candidate_sensitivity FROM sharing_proposals WHERE id = ?').get(proposal) },
      { candidate_title: '', candidate_body: '', candidate_sensitivity: 'secret' });
  });
});

test('a work whose pointer names another work\'s checkpoint is withheld whole, not applied in part', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    seedWork(a.db, 'w_one', 'ctx_one');
    seedWork(a.db, 'w_two', 'ctx_two');
    const material = materialHash('Base', 'Base body');
    a.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash, sensitivity,
      review_state, created_at, work_id, checkpoint_parent_id) VALUES ('m_c1', ?, 'session_summary', 'Base', 'Base body', '[]', '', ?, ?, 'eligible', 'reviewed', 1, 'w_one', NULL)`)
      .run(REPO, material, checkpointHash(REPO, 'w_one', null, material));
    a.db.prepare("UPDATE work_items SET current_checkpoint_memory_id = 'm_c1' WHERE id = 'w_one'").run();
    pull(b, a, publish(a, dir));
    const before = { ...b.db.prepare('SELECT state, completed_at, current_checkpoint_memory_id FROM work_items WHERE id = ?').get(readOrigin(b.db, `${a.id}:w_two`)!.local_id!) };
    // A's next revision of w_two completes it and points at w_one's checkpoint.
    a.db.prepare("UPDATE work_items SET state = 'completed', completed_at = 20, current_checkpoint_memory_id = 'm_c1' WHERE id = 'w_two'").run();
    const result = pull(b, a, publish(a, dir));
    const origin = readOrigin(b.db, `${a.id}:w_two`)!;
    assert.equal(origin.withheld_reason, 'repo_mismatch');
    assert.ok(result.withheldOnApply >= 1);
    assert.deepEqual({ ...b.db.prepare('SELECT state, completed_at, current_checkpoint_memory_id FROM work_items WHERE id = ?').get(origin.local_id!) }, before,
      'the row is exactly what it was');
    publish(b, dir);
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ? AND author = ?").get(origin.origin_id, b.id)?.n, 0, 'no partial application is authored');
  });
});

test('an in-place change of a context-only source keyed by its raw event replaces the row on the receiver', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    a.db.prepare(`INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
      VALUES ('s_one', ?, 'claude', 'n_one', 'c_one', 'active')`).run(REPO);
    a.db.prepare(`INSERT INTO raw_events (id, repo_id, session_id, kind, classification_state, processing_state, captured_at)
      VALUES ('raw_one', ?, 's_one', 'prompt', 'done', 'processed', 1)`).run(REPO);
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, NULL)").run();
    const rowid = String(a.db.prepare('SELECT id FROM memory_sources').get()?.id);
    const localId = (): string => sourceLocalId(a.db, rowid);
    // The adverse order: the origin the change creates sorts before the origin it retires, so the
    // receiver inserts the new row before the old origin's tombstone deletes the old one.
    const retired = localId();
    for (let i = 0; localId() <= retired; i += 1) a.db.prepare('UPDATE memory_sources SET source_agent = ? WHERE id = ?').run(`agent-${i}`, rowid);
    pull(b, a, publish(a, dir));
    a.db.prepare("UPDATE memory_sources SET source_agent = NULL WHERE memory_id = 'm_one'").run();
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0);
    assert.deepEqual(b.db.prepare('SELECT raw_event_id, source_agent FROM memory_sources').all().map((row) => ({ ...row })),
      [{ raw_event_id: 'raw_one', source_agent: null }]);
  });
});

// Round three of the correctness review: the released-projection text check, the late alias as a
// pre-pass, capture before map-repo binds, the kept head aligned on resolve, a referenced foreign
// work, and a device-independent source identity.

test('a successor released from a projection still has to hash to the projection text: other text is rejected', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    const identity = sha256Json(['personal-projection-v1', 'Candidate', 'Candidate body']);
    seedWork(b.db);
    insertMemory(b.db, 'm_origin', 'Origin', 'Origin body');
    seedProposal(b.db, 'p_one', 'm_origin');
    insertMemory(b.db, 'm_proj', 'Candidate', 'Candidate body', { content: identity });
    b.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v', 'm_proj', 'personal', NULL, NULL, 'p_one', 'proposal_approval', 2)`).run();
    b.db.prepare(`INSERT INTO sync_approvals (proposal_id, candidate_hash, projection_hash, scope_json, approved_at) VALUES ('p_one', ?, ?, '{"audience":"personal"}', 2)`)
      .run(materialHash('Candidate', 'Candidate body'), identity);
    // A holds B's projection origin withheld; a peer then authors a successor under an ordinary identity with other text.
    pull(a, b, publish(b, dir));
    a.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity, review_state, created_at)
      VALUES ('m_evil', ?, 'discovery', 'Evil', 'Evil body', '', ?, ?, 'eligible', 'reviewed', 1)`).run(REPO, materialHash('Evil', 'Evil body'), identity);
    a.db.prepare("UPDATE sync_origins SET local_id = 'm_evil', withheld_reason = NULL, materialized_revision = selected_head, materialized_hash = 'x' WHERE origin_id = ?")
      .run(`${b.id}:m_proj`);
    assert.throws(() => pull(b, a, publish(a, dir)), (error: unknown) => error instanceof BundleRejected && error.code === 'personal_identity_mismatch');
    assert.equal(b.db.prepare("SELECT title FROM memories WHERE id = 'm_proj'").get()?.title, 'Candidate', 'the approved projection text is unchanged');
  });
});

test('a source whose writer withholds after the late alias is not bound, so no local tombstone deletes the peer row', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    // A path-identified repository B cannot map: A's parent memory is identity-only on B.
    a.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('repo_two', 'common_dir', '/work/two/.git', '/work/two', 1, 1)`).run();
    insertMemory(a.db, 'm_child', 'Child', 'Child body');
    a.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity, review_state, created_at)
      VALUES ('m_parent', 'repo_two', 'discovery', 'Parent', 'Parent body', '', ?, ?, 'eligible', 'reviewed', 1)`)
      .run(materialHash('Parent', 'Parent body'), sha256Json(['repo_two', materialHash('Parent', 'Parent body')]));
    a.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_child', 'file_read', 'm_parent', 1)").run();
    pull(b, a, publish(a, dir));
    const source = b.db.prepare("SELECT local_id, withheld_reason FROM sync_origins WHERE kind = 'source'").get()!;
    assert.equal(source.local_id, null, 'a withheld source stays unbound');
    assert.equal(source.withheld_reason, 'unresolved_reference');
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM sync_revisions WHERE kind = 'source' AND author = ?").get(b.id)?.n, 0, 'B authors nothing for it');
    pull(a, b, publish(b, dir));
    assert.equal(a.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'A keeps its own source row');
  });
});

test('map-repo captures local changes before the mapping lets a withheld origin alias onto an edited row', async () => {
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
      insertMemory(a, 'm_one', 'Title', 'Body text');
      pushSpace(a, pathsA, { now: 10 });
      pullSpace(b, pathsB, { now: 11 });
      // B marks the same material secret after its last sync command: nothing has captured it yet.
      insertMemory(b, 'm_b', 'Title', 'Body text', { sensitivity: 'secret' });
      b.prepare("UPDATE memories SET title = '', body = '' WHERE id = 'm_b'").run();
      mapRepo(b, pathsB, { repoKey: repoKeyFor(replicaOriginId(a), 'common_dir', '/work/a/.git'), localRepoId: REPO, now: 12 });
      assert.equal(readOrigin(b, `${replicaOriginId(a)}:m_one`)!.local_id, 'm_b', 'the origin aliases onto the local row');
      assert.deepEqual({ ...b.prepare("SELECT sensitivity, body FROM memories WHERE id = 'm_b'").get()! }, { sensitivity: 'secret', body: '' },
        'the row already terminal on this device keeps that state');
    } finally { a.close(); b.close(); }
  });
});

test('an origin that aliases late onto a row already written in the same pass still applies its control', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [lo, hi, c] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica, Replica];
    insertMemory(lo.db, 'm_one', 'Title', 'Body text');
    insertMemory(hi.db, 'm_one', 'Title', 'Body text', { sensitivity: 'private' });
    publish(hi, dir);
    pull(hi, lo, publish(lo, dir));
    assert.equal(hi.db.prepare("SELECT sensitivity FROM memories WHERE id = 'm_one'").get()?.sensitivity, 'private');
    // c receives both origins in one bundle: the smaller (lo) is written first, hi aliases onto it afterwards.
    pull(c, hi, publish(hi, dir));
    const local = readOrigin(c.db, `${lo.id}:m_one`)!.local_id!;
    assert.equal(effectiveControl(c.db, `${lo.id}:m_one`).sensitivity_floor, 'private');
    assert.equal(c.db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(local)?.sensitivity, 'private', 'the floor of the aliased origin reaches the row');
  });
});

test('a resolve that keeps the head of an aliased origin ships references aligned to the canonical natural key', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [lo, hi] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    for (const device of [lo, hi]) {
      insertMemory(device.db, 'm_one', 'Title', 'Body text');
      insertSource(device.db, 'm_one', 'src/a.ts');
      device.db.exec('BEGIN IMMEDIATE'); captureLocalChanges(device.db, 5); device.db.exec('COMMIT');
    }
    pull(hi, lo, publish(lo, dir));
    pull(lo, hi, publish(hi, dir));
    const canonical = String(lo.db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'source' AND origin_id = canonical_origin_id").get()!.origin_id);
    const heads = headsOf(lo.db, canonical);
    assert.equal(heads.length, 2, 'one head under each origin');
    const other = heads.find((head) => readRevision(lo.db, head)!.origin_id !== canonical)!;
    lo.db.exec('BEGIN IMMEDIATE');
    const { revision_id } = resolveRow(lo.db, canonical, other, 300);
    lo.db.exec('COMMIT');
    const successor = readRevision(lo.db, revision_id)!;
    assert.equal(successor.payload!.memory_id, successor.natural.memory, 'the successor names the memory the way its natural key does');
    pull(hi, lo, publish(lo, dir));
    assert.deepEqual(headsOf(hi.db, canonical), [revision_id], 'the peer accepts the resolution');
  });
});

test('a new work whose pointer is foreign but whose checkpoints arrived with it is withheld without a local revision', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    seedWork(a.db, 'w_one', 'ctx_one');
    seedWork(a.db, 'w_two', 'ctx_two');
    seedCheckpoint(a.db, 'm_c1', 'w_one', 'One');
    seedCheckpoint(a.db, 'm_c2', 'w_two', 'Two');
    a.db.prepare("UPDATE work_items SET current_checkpoint_memory_id = 'm_c1' WHERE id = 'w_one'").run();
    a.db.prepare("UPDATE work_items SET state = 'completed', completed_at = 20, current_checkpoint_memory_id = 'm_c1' WHERE id = 'w_two'").run();
    pull(b, a, publish(a, dir));
    const origin = readOrigin(b.db, `${a.id}:w_two`)!;
    assert.equal(origin.withheld_reason, 'repo_mismatch');
    assert.ok(origin.local_id, 'the row stays because its checkpoint references it');
    publish(b, dir);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ? AND author = ?').get(origin.origin_id, b.id)?.n, 0,
      'the closing pass records nothing for it');
  });
});

test('a source is named alike on every device: its identity hashes the parent material and the context key, not origin ids', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [lo, hi] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    insertMemory(lo.db, 'm_parent', 'Parent', 'Parent body');
    insertMemory(lo.db, 'm_child', 'Child', 'Child body');
    lo.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_child', 'file_read', 'm_parent', 1)").run();
    // hi already holds the parent under its own id, so on hi the parent memory has two origins and hi's sorts first.
    insertMemory(hi.db, 'a_parent', 'Parent', 'Parent body');
    hi.db.exec('BEGIN IMMEDIATE'); captureLocalChanges(hi.db, 5); hi.db.exec('COMMIT');
    pull(hi, lo, publish(lo, dir));
    const rowid = String(hi.db.prepare("SELECT id FROM memory_sources WHERE memory_id = (SELECT local_id FROM sync_origins WHERE origin_id = ?)").get(`${lo.id}:m_child`)!.id);
    const expected = readOrigin(hi.db, `${lo.id}:${sourceLocalId(lo.db, String(lo.db.prepare('SELECT id FROM memory_sources').get()!.id))}`)!;
    assert.equal(sourceLocalId(hi.db, rowid), expected.local_id, 'the receiver recomputes the same local id');
    pull(hi, lo, publish(lo, dir));
    assert.equal(hi.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'a second pull finds the row instead of inserting again');
  });
});
