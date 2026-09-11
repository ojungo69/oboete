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
import { ResolveError, resolveRow } from '../../src/sync/apply.js';
import { captureLocalChanges } from '../../src/sync/capture.js';
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

function seedRawEvent(db: DatabaseSync, id = 'raw_one'): void {
  db.prepare(`INSERT OR IGNORE INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
    VALUES ('s_one', ?, 'claude', 'n_one', 'c_one', 'active')`).run(REPO);
  db.prepare(`INSERT INTO raw_events (id, repo_id, session_id, kind, classification_state, processing_state, captured_at)
    VALUES (?, ?, 's_one', 'prompt', 'done', 'processed', 1)`).run(id, REPO);
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

test('an in-place source field change is a revision of the same origin, and the receiver updates its row in place', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    const rowid = insertSource(a.db, 'm_one', 'src/a.ts');
    a.db.prepare('UPDATE memory_sources SET source_hash = ?, portion_start = 0, portion_end = 10, source_total = 10 WHERE id = ?').run('1'.repeat(64), rowid);
    pull(b, a, publish(a, dir));
    const key = String(a.db.prepare('SELECT sync_key FROM memory_sources WHERE id = ?').get(rowid)!.sync_key);
    // worker/imported.ts clears source_agent in place: the key, and so the origin, stays.
    a.db.prepare('UPDATE memory_sources SET source_agent = NULL WHERE id = ?').run(rowid);
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0);
    assert.deepEqual(b.db.prepare('SELECT source_agent, sync_key FROM memory_sources').all().map((row) => ({ ...row })),
      [{ source_agent: null, sync_key: key }], 'one row, with the new value, under the same key');
    const origins = b.db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'source'").all();
    assert.equal(origins.length, 1);
    assert.equal(headsOf(b.db, String(origins[0]!.origin_id)).length, 1, 'the change is a successor, not a sibling');
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

test('an independent capture of a context-only source keyed by its raw event aliases onto the local row instead of colliding', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    for (const device of [a, b]) {
      insertMemory(device.db, 'm_one', 'Title', 'Body text');
      device.db.prepare(`INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
        VALUES ('s_one', ?, 'claude', 'n_one', 'c_one', 'active')`).run(REPO);
      device.db.prepare(`INSERT INTO raw_events (id, repo_id, session_id, kind, classification_state, processing_state, captured_at)
        VALUES ('raw_one', ?, 's_one', 'prompt', 'done', 'processed', 1)`).run(REPO);
    }
    // The same raw event cited on both devices with different agents: distinct keys, one UNIQUE tuple.
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'claude')").run();
    b.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'codex')").run();
    publish(b, dir);
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'one row: the UNIQUE tuple names the same source');
    const origins = b.db.prepare("SELECT origin_id, local_id, canonical_origin_id FROM sync_origins WHERE kind = 'source'").all();
    assert.equal(origins.length, 2);
    assert.equal(new Set(origins.map((origin) => origin.local_id)).size, 1, 'both origins name the row');
    assert.equal(new Set(origins.map((origin) => origin.canonical_origin_id)).size, 1, 'one canonical group');
    assert.equal(b.db.prepare("SELECT status FROM sync_conflicts WHERE id = ?").get(`sync:${String(origins[0]!.canonical_origin_id)}`)?.status, 'open',
      'two independent captures are two heads, reported');
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
      seedRawEvent(device.db);
      const rowid = insertSource(device.db, 'm_one', 'src/a.ts');
      device.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_one', source_hash = ?, portion_start = 0, portion_end = 10, source_total = 10 WHERE id = ?").run('4'.repeat(64), rowid);
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

test('a source is named alike on every device: the key it arrived with is the key its row keeps', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [lo, hi] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    insertMemory(lo.db, 'm_parent', 'Parent', 'Parent body');
    insertMemory(lo.db, 'm_child', 'Child', 'Child body');
    lo.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_child', 'file_read', 'm_parent', 1)").run();
    // hi already holds the parent under its own id, so on hi the parent memory has two origins and hi's sorts first.
    insertMemory(hi.db, 'a_parent', 'Parent', 'Parent body');
    hi.db.exec('BEGIN IMMEDIATE'); captureLocalChanges(hi.db, 5); hi.db.exec('COMMIT');
    pull(hi, lo, publish(lo, dir));
    const key = String(lo.db.prepare('SELECT sync_key FROM memory_sources').get()!.sync_key);
    const landed = hi.db.prepare('SELECT sync_key FROM memory_sources WHERE memory_id = (SELECT local_id FROM sync_origins WHERE origin_id = ?)').get(`${lo.id}:m_child`)!;
    assert.equal(landed.sync_key, key, 'the receiver keeps the key');
    assert.equal(readOrigin(hi.db, `${lo.id}:source:${key}`)!.local_id, `source:${key}`);
    pull(hi, lo, publish(lo, dir));
    assert.equal(hi.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'a second pull finds the row instead of inserting again');
  });
});

// Round four: aliases discovered at write time re-enter the pass, the base is realigned before the
// pass (a cascade during it is a local change), the materialized base survives a canonical change,
// and a source's identity is a stored key.

test('a floor raised through a parent during the pass is recorded as a local change even when the child\'s own writer withholds', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_parent', 'Parent', 'Parent body');
    insertMemory(a.db, 'm_child', 'Child', 'Child body');
    pull(b, a, publish(a, dir));
    // Only B holds the dependency edge; A then raises the parent and points the child at a memory B cannot resolve.
    b.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES (?, 'file_read', ?, 1)")
      .run(memoryOf(b, a, 'm_child').id as string, memoryOf(b, a, 'm_parent').id as string);
    a.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('repo_two', 'common_dir', '/work/two/.git', '/work/two', 1, 1)`).run();
    a.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity, review_state, created_at)
      VALUES ('m_far', 'repo_two', 'discovery', 'Far', 'Far body', '', ?, ?, 'eligible', 'reviewed', 1)`)
      .run(materialHash('Far', 'Far body'), sha256Json(['repo_two', materialHash('Far', 'Far body')]));
    a.db.prepare("UPDATE memories SET sensitivity = 'private' WHERE id = 'm_parent'").run();
    a.db.prepare("UPDATE memories SET superseded_by = 'm_far' WHERE id = 'm_child'").run();
    pull(b, a, publish(a, dir));
    assert.equal(readOrigin(b.db, `${a.id}:m_child`)!.withheld_reason, 'unresolved_reference', 'the child payload is withheld');
    assert.equal(memoryOf(b, a, 'm_child').sensitivity, 'private', 'the trigger raised the child through the edge');
    assert.equal(effectiveControl(b.db, `${a.id}:m_child`).sensitivity_floor, 'private', 'the raise is a recorded control, not absorbed into the base');
    pull(a, b, publish(b, dir));
    assert.equal(a.db.prepare("SELECT sensitivity FROM memories WHERE id = 'm_child'").get()?.sensitivity, 'private', 'and it reaches A');
  });
});

test('an origin whose writer finds the row another origin created earlier in the pass re-enters with the combined heads and floor', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [lo, hi, c] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica, Replica];
    insertMemory(lo.db, 'm_one', 'Title', 'Body text', { sensitivity: 'private' });
    insertMemory(hi.db, 'm_one', 'Title', 'Body text');
    publish(hi, dir);
    pull(hi, lo, publish(lo, dir));
    // On an empty c neither origin binds before the pass: lo's writer creates the row, hi's finds it.
    pull(c, hi, publish(hi, dir));
    const local = readOrigin(c.db, `${lo.id}:m_one`)!.local_id!;
    assert.equal(c.db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(local)?.sensitivity, 'private', 'the combined floor holds');
    assert.equal(readOrigin(c.db, `${hi.id}:m_one`)!.local_id, local, 'one row');
    const before = revisionCount(c.db);
    publish(c, dir);
    assert.equal(revisionCount(c.db), before, 'c invents nothing');
    assert.equal(c.db.prepare("SELECT status FROM sync_conflicts WHERE id = ?").get(`sync:${lo.id}:m_one`)?.status, 'open', 'two independent captures are reported');
  });
});

test('a checkpoint chain whose origins sort child-first binds every link before the pass, so nothing lands on a non-canonical origin', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    seedWork(a.db);
    pull(b, a, publish(a, dir));
    const work = String(b.db.prepare('SELECT id FROM work_items').get()!.id);
    // Both devices capture the same chain; the ids put the deepest link first on the wire.
    const chain = (db: DatabaseSync, ids: [string, string, string], workId: string): void => {
      let parent: string | null = null;
      for (const [index, id] of ids.entries()) {
        const title = `Step ${index}`;
        const material = materialHash(title, `${title} body`);
        db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash, sensitivity,
          review_state, created_at, work_id, checkpoint_parent_id) VALUES (?, ?, 'session_summary', ?, ?, '[]', '', ?, ?, 'eligible', 'reviewed', 1, ?, ?)`)
          .run(id, REPO, title, `${title} body`, material, checkpointHash(REPO, workId, parent, material), workId, parent);
        parent = id;
      }
    };
    chain(a.db, ['z_c1', 'y_c2', 'x_c3'], 'w_one');
    chain(b.db, ['b_c1', 'b_c2', 'b_c3'], work);
    publish(b, dir);
    pull(b, a, publish(a, dir));
    for (const [id, local] of [['z_c1', 'b_c1'], ['y_c2', 'b_c2'], ['x_c3', 'b_c3']] as const) {
      const origin = readOrigin(b.db, `${a.id}:${id}`)!;
      assert.equal(origin.local_id, local, `${id} aliases onto the local link`);
      const canonical = readOrigin(b.db, origin.canonical_origin_id)!;
      assert.ok(canonical.selected_head, 'the state sits on the canonical origin');
      if (origin.origin_id !== canonical.origin_id) assert.equal(origin.selected_head, null, 'never on an aliased origin');
    }
    const before = revisionCount(b.db);
    publish(b, dir);
    assert.equal(revisionCount(b.db), before, 'no phantom revision');
  });
});

test('a canonical that changes to an origin that only arrived keeps the materialized base of the local row', async () => {
  await withHomes(2, (homes, shared) => {
    const [homeX, homeY] = homes as [string, string];
    const x = openHome(homeX);
    const y = openHome(homeY);
    try {
      const [lo, hi] = replicaOriginId(x) < replicaOriginId(y) ? [{ db: x, home: homeX }, { db: y, home: homeY }] : [{ db: y, home: homeY }, { db: x, home: homeX }];
      const pathsLo = oboetePaths(lo.home);
      const pathsHi = oboetePaths(hi.home);
      lo.db.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/lo/.git' WHERE id = ?").run(REPO);
      hi.db.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/hi/.git' WHERE id = ?").run(REPO);
      const { keyLine } = initSpace(lo.db, pathsLo, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(hi.db, pathsHi, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      // lo's memory points at a memory in a second repository hi never maps, so its writer withholds.
      lo.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
        VALUES ('repo_two', 'common_dir', '/work/two/.git', '/work/two', 1, 1)`).run();
      lo.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity, review_state, created_at)
        VALUES ('m_far', 'repo_two', 'discovery', 'Far', 'Far body', '', ?, ?, 'eligible', 'reviewed', 1)`)
        .run(materialHash('Far', 'Far body'), sha256Json(['repo_two', materialHash('Far', 'Far body')]));
      insertMemory(lo.db, 'm_one', 'Title', 'Body text');
      lo.db.prepare("UPDATE memories SET superseded_by = 'm_far' WHERE id = 'm_one'").run();
      insertMemory(hi.db, 'm_hi', 'Title', 'Body text');
      pushSpace(hi.db, pathsHi, { now: 5 });
      const base = String(readOrigin(hi.db, `${replicaOriginId(hi.db)}:m_hi`)!.materialized_revision);
      pushSpace(lo.db, pathsLo, { now: 10 });
      pullSpace(hi.db, pathsHi, { now: 11 });
      mapRepo(hi.db, pathsHi, { repoKey: repoKeyFor(replicaOriginId(lo.db), 'common_dir', '/work/lo/.git'), localRepoId: REPO, now: 12 });
      const canonical = readOrigin(hi.db, `${replicaOriginId(lo.db)}:m_one`)!;
      assert.equal(canonical.local_id, 'm_hi');
      assert.equal(canonical.canonical_origin_id, canonical.origin_id, 'the arriving origin sorts first and is canonical');
      assert.equal(canonical.withheld_reason, 'unresolved_reference');
      assert.equal(canonical.materialized_revision, base, 'the local row keeps its base');
      hi.db.prepare("UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = 'm_hi'").run();
      pushSpace(hi.db, pathsHi, { now: 20 });
      const heads = headsOf(hi.db, canonical.origin_id);
      assert.equal(heads.length, 2, "lo's head and hi's successor");
      assert.ok(heads.some((head) => readRevision(hi.db, head)!.parents.includes(base)), 'the pin is a successor of the base');
    } finally { x.close(); y.close(); }
  });
});

test('a source erased by its memory\'s secret marking stays one origin, bound, and B records nothing for it', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertSource(a.db, 'm_one', 'src/a.ts');
    pull(b, a, publish(a, dir));
    a.db.prepare("UPDATE memories SET sensitivity = 'secret', title = '', body = '' WHERE id = 'm_one'").run();
    a.db.prepare("UPDATE memory_sources SET citation_value = NULL, evidence = NULL, capture_root = NULL, source_paths_json = NULL WHERE memory_id = 'm_one'").run();
    pull(b, a, publish(a, dir));
    const origins = b.db.prepare("SELECT local_id, withheld_reason FROM sync_origins WHERE kind = 'source'").all();
    assert.equal(origins.length, 1, 'one origin for the source through the marking');
    assert.ok(origins[0]!.local_id, 'bound');
    assert.equal(origins[0]!.withheld_reason, null);
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM sync_revisions WHERE kind = 'source' AND author = ?").get(b.id)?.n, 0, 'B authors nothing');
    pull(a, b, publish(b, dir));
    assert.equal(conflicts(a.db), 0);
  });
});

test('a source under a deleted memory ships redacted and stays one row on both devices', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertSource(a.db, 'm_one', 'src/a.ts');
    pull(b, a, publish(a, dir));
    // The 0004 trigger clears the citation locally; the wire also drops source_agent.
    a.db.prepare("UPDATE memories SET deleted_at = 5 WHERE id = 'm_one'").run();
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1);
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM sync_revisions WHERE kind = 'source' AND author = ?").get(b.id)?.n, 0, 'B authors nothing');
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'a second pull inserts nothing');
    pull(a, b, publish(b, dir));
    assert.equal(a.db.prepare("SELECT COUNT(*) AS n FROM memory_sources").get()?.n, 1, 'A keeps its row');
    assert.equal(a.db.prepare("SELECT source_agent FROM memory_sources").get()?.source_agent, 'claude', 'with its own metadata');
  });
});

test('two context edges to same-material checkpoints under different parents are two sources with two keys', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    seedWork(a.db);
    seedCheckpoint(a.db, 'm_c1', 'w_one', 'Same');
    const material = materialHash('Same', 'Same body');
    a.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash, sensitivity,
      review_state, created_at, work_id, checkpoint_parent_id) VALUES ('m_c2', ?, 'session_summary', 'Same', 'Same body', '[]', '', ?, ?, 'eligible', 'reviewed', 1, 'w_one', 'm_c1')`)
      .run(REPO, material, checkpointHash(REPO, 'w_one', 'm_c1', material));
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    for (const parent of ['m_c1', 'm_c2']) {
      a.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_one', 'file_read', ?, 1)").run(parent);
    }
    pull(b, a, publish(a, dir));
    assert.equal(a.db.prepare('SELECT COUNT(DISTINCT sync_key) AS n FROM memory_sources').get()?.n, 2, 'distinct keys');
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ?").get(memoryOf(b, a, 'm_one').id as string)?.n, 2, 'both edges arrive');
  });
});

test('a tombstone for a row this device never held is applied, not withheld', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    const rowid = insertSource(a.db, 'm_one', 'src/a.ts');
    insertMemory(a.db, 'm_gone', 'Gone', 'Gone body');
    publish(a, dir);
    a.db.prepare('DELETE FROM memory_sources WHERE id = ?').run(rowid);
    a.db.prepare("UPDATE memories SET deleted_at = 5 WHERE id = 'm_gone'").run();
    // B joins after the deletions: the log carries the tombstone heads and no payload.
    pull(b, a, publish(a, dir));
    const origin = b.db.prepare("SELECT local_id, withheld_reason, materialized_revision FROM sync_origins WHERE kind = 'source'").get()!;
    assert.equal(origin.local_id, null, 'no row, no binding');
    assert.equal(origin.materialized_revision, null, 'and no base');
    assert.equal(origin.withheld_reason, null);
    assert.equal(readOrigin(b.db, `${a.id}:m_gone`)!.withheld_reason, null, 'a memory deleted before it shipped is applied too');
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM sync_origins WHERE withheld_reason IS NOT NULL").get()?.n, 0);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 0);
    const before = revisionCount(b.db);
    publish(b, dir);
    assert.equal(revisionCount(b.db), before, 'and B records nothing for it');
  });
});

// Round five: the source key through the bound row, the memory a source sits under as revision
// data, UNIQUE-tuple aliases with the index's NULL semantics, merges before any write, unbound
// applied heads on map-repo, and keys that agree across devices.

test('a tombstone for an origin that aliased onto a row with another key deletes that row', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [lo, hi] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    for (const device of [lo, hi]) { insertMemory(device.db, 'm_one', 'Title', 'Body text'); seedRawEvent(device.db); }
    lo.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'claude')").run();
    hi.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'codex')").run();
    publish(hi, dir);
    pull(hi, lo, publish(lo, dir));
    assert.equal(hi.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'one row under two origins');
    lo.db.prepare('DELETE FROM memory_sources').run();
    const result = pull(hi, lo, publish(lo, dir));
    assert.equal(result.withheldOnApply, 0);
    assert.equal(hi.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 0, 'the row the canonical names is gone');
    const before = revisionCount(hi.db);
    publish(hi, dir);
    assert.equal(revisionCount(hi.db), before, 'hi records nothing for it');
  });
});

test('a source moved under another memory is a new source there, and the old one ships a tombstone', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_old', 'Old', 'Old body');
    insertSource(a.db, 'm_old', 'src/a.ts');
    pull(b, a, publish(a, dir));
    // worker/imported.ts moves the sources of a retired memory under its replacement.
    insertMemory(a.db, 'm_new', 'New', 'New body');
    a.db.prepare("UPDATE memory_sources SET memory_id = 'm_new'").run();
    a.db.prepare("UPDATE memories SET deleted_at = 9 WHERE id = 'm_old'").run();
    const header = JSON.parse(readFileSync(publish(a, dir), 'utf8').split('\n')[0]!) as { withheld: { sources: number } };
    assert.equal(header.withheld.sources, 0, 'the move ships');
    pull(b, a, `${dir}/${a.id}.plain`);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1);
    assert.equal(b.db.prepare('SELECT memory_id FROM memory_sources').get()?.memory_id, memoryOf(b, a, 'm_new').id, 'under the new memory');
    const origins = b.db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'source'").all().map((row) => String(row.origin_id));
    assert.equal(origins.length, 2, 'the old origin and the new one');
    assert.equal(origins.filter((origin) => headsOf(b.db, origin).some((head) => readRevision(b.db, head)!.control.tombstone)).length, 1, 'the old one is tombstoned');
  });
});

test('two citations that differ only where the UNIQUE index has NULL stay two rows on the receiver', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    for (const citation of ['src/x.ts', 'src/y.ts']) {
      const rowid = insertSource(a.db, 'm_one', citation);
      a.db.prepare('UPDATE memory_sources SET source_hash = ?, portion_start = 0, portion_end = 10, source_total = 10 WHERE id = ?').run('2'.repeat(64), rowid);
    }
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 2, 'raw_event_id NULL never matches, as in the index');
    assert.equal(conflicts(b.db), 0);
  });
});

test('a head whose tuple collides with a row under another index replaces that row instead of failing the bundle', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    for (const device of [a, b]) { insertMemory(device.db, 'm_one', 'Title', 'Body text'); seedRawEvent(device.db); }
    // B holds a context-only row for the raw event; A ships a hashed source for the same raw event that is also context-only.
    b.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only) VALUES ('m_one', 'raw_one', 'file_read', 1)").run();
    publish(b, dir);
    a.db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_hash, portion_start, portion_end, source_total)
      VALUES ('m_one', 'raw_one', 'file_read', 1, ?, 0, 10, 10)`).run('3'.repeat(64));
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0, 'no apply_failed');
    assert.deepEqual(b.db.prepare('SELECT raw_event_id FROM memory_sources').all().map((row) => row.raw_event_id), ['raw_one'], 'one row for the raw event');
    const origins = b.db.prepare("SELECT DISTINCT canonical_origin_id AS id FROM sync_origins WHERE kind = 'source'").all();
    assert.equal(origins.length, 1, 'the two captures alias');
    assert.equal(b.db.prepare('SELECT status FROM sync_conflicts WHERE id = ?').get(`sync:${String(origins[0]!.id)}`)?.status, 'open', 'and are reported as two heads');
  });
});

test('a merge is discovered before any write: a peer\'s plaintext never lands in a row cleared by a secret marking', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [lo, hi] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    for (const device of [lo, hi]) { insertMemory(device.db, 'm_one', 'Title', 'Body text'); seedRawEvent(device.db); }
    lo.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value, context_only) VALUES ('m_one', 'raw_one', 'file_read', 'src/lo.ts', 1)").run();
    hi.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value, context_only) VALUES ('m_one', 'raw_one', 'file_read', 'src/hi.ts', 1)").run();
    publish(lo, dir);
    lo.db.prepare("UPDATE memories SET sensitivity = 'secret', title = '', body = '' WHERE id = 'm_one'").run();
    lo.db.prepare('UPDATE memory_sources SET citation_value = NULL').run();
    pull(lo, hi, publish(hi, dir));
    assert.equal(lo.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1);
    assert.equal(lo.db.prepare('SELECT citation_value FROM memory_sources').get()?.citation_value, null, 'the row under the secret memory holds no evidence');
    const before = revisionCount(lo.db);
    publish(lo, dir);
    assert.equal(revisionCount(lo.db), before, 'and nothing was invented');
  });
});

test('map-repo re-evaluates an applied tombstone that had no row, so it reaches the row the mapping resolves', async () => {
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
      insertMemory(a, 'm_other', 'Other', 'Other body');
      insertMemory(b, 'm_b', 'Title', 'Body text');
      pushSpace(a, pathsA, { now: 5 });
      a.prepare("UPDATE memories SET deleted_at = 6 WHERE id = 'm_one'").run();
      pushSpace(a, pathsA, { now: 10 });
      pullSpace(b, pathsB, { now: 11 });
      const origin = readOrigin(b, `${replicaOriginId(a)}:m_one`)!;
      assert.equal(origin.local_id, null, 'nothing to bind before the mapping');
      mapRepo(b, pathsB, { repoKey: repoKeyFor(replicaOriginId(a), 'common_dir', '/work/a/.git'), localRepoId: REPO, now: 12 });
      assert.equal(readOrigin(b, `${replicaOriginId(a)}:m_one`)!.local_id, 'm_b', 'the tombstone binds to the equivalent row');
      assert.notEqual(b.prepare("SELECT deleted_at FROM memories WHERE id = 'm_b'").get()?.deleted_at, null, 'and applies');
    } finally { a.close(); b.close(); }
  });
});

test('the same citation under the same text in another repository has another key, so it never aliases across repositories', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertSource(a.db, 'm_one', 'src/a.ts');
    // B holds the same text and citation in an unrelated repository.
    b.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('repo_two', 'remote', 'github.com/example/two', '/work/two', 1, 1)`).run();
    b.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity, review_state, created_at)
      VALUES ('m_two', 'repo_two', 'discovery', 'Title', 'Body text', '', ?, ?, 'eligible', 'reviewed', 1)`)
      .run(materialHash('Title', 'Body text'), sha256Json(['repo_two', materialHash('Title', 'Body text')]));
    insertSource(b.db, 'm_two', 'src/a.ts');
    publish(b, dir);
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 2);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 2, "A's source lands under A's memory, not B's");
    assert.equal(b.db.prepare('SELECT COUNT(DISTINCT sync_key) AS n FROM memory_sources').get()?.n, 2);
    a.db.prepare('DELETE FROM memory_sources').run();
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = 'm_two'").get()?.n, 1, "A's deletion never reaches B's row");
  });
});

test('a row processed again after a merge is counted once', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [lo, hi, c] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica, Replica];
    insertMemory(lo.db, 'm_one', 'Title', 'Body text', { sensitivity: 'private' });
    insertMemory(hi.db, 'm_one', 'Title', 'Body text');
    publish(hi, dir);
    pull(hi, lo, publish(lo, dir));
    const result = pull(c, hi, publish(hi, dir));
    assert.equal(result.materialized, 1, 'one row');
    assert.equal(result.conflicts, 1, 'one report');
  });
});

test('an origin applied without a row carries no base, so an alias later keeps the local row\'s base', async () => {
  await withHomes(2, (homes, shared) => {
    const [homeX, homeY] = homes as [string, string];
    const x = openHome(homeX);
    const y = openHome(homeY);
    try {
      const [lo, hi] = replicaOriginId(x) < replicaOriginId(y) ? [{ db: x, home: homeX }, { db: y, home: homeY }] : [{ db: y, home: homeY }, { db: x, home: homeX }];
      const pathsLo = oboetePaths(lo.home);
      const pathsHi = oboetePaths(hi.home);
      lo.db.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/lo/.git' WHERE id = ?").run(REPO);
      hi.db.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/hi/.git' WHERE id = ?").run(REPO);
      const { keyLine } = initSpace(lo.db, pathsLo, { directory: shared, classes: ['eligible', 'private'], now: 1 });
      joinSpace(hi.db, pathsHi, { directory: shared, keyLine, classes: ['eligible', 'private'], now: 1 });
      insertMemory(lo.db, 'm_one', 'Title', 'Body text');
      insertMemory(hi.db, 'm_hi', 'Title', 'Body text');
      pushSpace(hi.db, pathsHi, { now: 5 });
      const base = String(readOrigin(hi.db, `${replicaOriginId(hi.db)}:m_hi`)!.materialized_revision);
      pushSpace(lo.db, pathsLo, { now: 10 });
      pullSpace(hi.db, pathsHi, { now: 11 });
      // lo deletes: a control-only head hi applies without a row.
      lo.db.prepare("UPDATE memories SET deleted_at = 12 WHERE id = 'm_one'").run();
      pushSpace(lo.db, pathsLo, { now: 12 });
      pullSpace(hi.db, pathsHi, { now: 13 });
      const unbound = readOrigin(hi.db, `${replicaOriginId(lo.db)}:m_one`)!;
      assert.equal(unbound.local_id, null);
      assert.equal(unbound.withheld_reason, null, 'applied');
      assert.equal(unbound.materialized_revision, null, 'no row, no base');
      mapRepo(hi.db, pathsHi, { repoKey: repoKeyFor(replicaOriginId(lo.db), 'common_dir', '/work/lo/.git'), localRepoId: REPO, now: 14 });
      const canonical = readOrigin(hi.db, `${replicaOriginId(lo.db)}:m_one`)!;
      assert.equal(canonical.local_id, 'm_hi');
      assert.equal(canonical.materialized_revision, canonical.selected_head, 'the tombstone head is materialized on the row');
      assert.notEqual(hi.db.prepare("SELECT deleted_at FROM memories WHERE id = 'm_hi'").get()?.deleted_at, null);
      assert.ok(headsOf(hi.db, canonical.origin_id).includes(base), "hi's own head stays a sibling");
      assert.equal(hi.db.prepare("SELECT status FROM sync_conflicts WHERE id = ?").get(`sync:${canonical.origin_id}`)?.status, 'open');
    } finally { x.close(); y.close(); }
  });
});

// Round six: a head whose tuple another origin's row holds waits for that origin, a bound row
// that vanished in the pass is a tombstone, a memory alias flip is not a source change, an origin
// that stopped being canonical is not processed, a malformed edge is rejected, a row created in
// the pass is an alias target, and lineage stays acyclic.

test('a re-created source whose tuple the old origin still holds lands after the old origin dies, in any order', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    seedRawEvent(a.db);
    // worker/imported.ts deletes and re-inserts the context row: a new origin holding the old tuple.
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'claude')").run();
    pull(b, a, publish(a, dir));
    const retired = String(a.db.prepare('SELECT sync_key FROM memory_sources').get()!.sync_key);
    a.db.prepare('DELETE FROM memory_sources').run();
    // The adverse order: the new origin sorts before the retired one, so its head is processed first.
    let agent = 'agent-0';
    for (let i = 0; ; i += 1, agent = `agent-${i}`) {
      a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, ?)").run(agent);
      a.db.exec('BEGIN IMMEDIATE'); captureLocalChanges(a.db, 10 + i); a.db.exec('COMMIT');
      if (String(a.db.prepare('SELECT sync_key FROM memory_sources').get()!.sync_key) < retired) break;
      a.db.prepare('DELETE FROM memory_sources').run();
    }
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0);
    assert.deepEqual(b.db.prepare('SELECT raw_event_id, source_agent FROM memory_sources').all().map((row) => ({ ...row })), [{ raw_event_id: 'raw_one', source_agent: agent }]);
    const live = b.db.prepare("SELECT DISTINCT canonical_origin_id AS id FROM sync_origins WHERE kind = 'source'").all()
      .filter((row) => headsOf(b.db, String(row.id)).some((head) => !readRevision(b.db, head)!.control.tombstone));
    assert.equal(live.length, 1, 'the new origin is the only live one, never merged with a retired one');
    assert.equal(conflicts(b.db), 0);
  });
});

test('a bound source whose row another head replaced in the pass is a tombstone with a parent, not an overwrite of the other row', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    seedRawEvent(a.db, 'raw_one');
    seedRawEvent(a.db, 'raw_two');
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'x')").run();
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_two', 'file_read', 1, 'y')").run();
    pull(b, a, publish(a, dir));
    // A retires the raw_one row and points the raw_two row at raw_one; B edited the raw_one row meanwhile.
    // The adverse order: the raw_two row's origin is processed first (origin id order), so its head
    // takes the tuple before the retired one dies. The keys are random, so the roles are swapped on A
    // (and pulled, as any edit) when the order comes out the other way.
    const two = a.db.prepare("SELECT sync_key FROM memory_sources WHERE raw_event_id = 'raw_two'").get()!;
    const first = b.db.prepare("SELECT local_id FROM sync_origins WHERE kind = 'source' ORDER BY origin_id LIMIT 1").get()!;
    if (first.local_id !== `source:${String(two.sync_key)}`) {
      a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_tmp' WHERE raw_event_id = 'raw_one'").run();
      a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_one' WHERE raw_event_id = 'raw_two'").run();
      a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_two' WHERE raw_event_id = 'raw_tmp'").run();
      assert.equal(pull(b, a, publish(a, dir)).conflicts, 0);
    }
    const survivor = String(a.db.prepare("SELECT source_agent FROM memory_sources WHERE raw_event_id = 'raw_two'").get()!.source_agent);
    b.db.prepare("UPDATE memory_sources SET source_agent = 'codex' WHERE raw_event_id = 'raw_one'").run();
    a.db.prepare("DELETE FROM memory_sources WHERE raw_event_id = 'raw_one'").run();
    a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_one' WHERE raw_event_id = 'raw_two'").run();
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0);
    assert.deepEqual(b.db.prepare('SELECT raw_event_id, source_agent FROM memory_sources').all().map((row) => ({ ...row })), [{ raw_event_id: 'raw_one', source_agent: survivor }],
      "the surviving row is A's raw_two row, never overwritten by B's edit of the retired one");
    for (const origin of b.db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'source'").all()) {
      for (const head of headsOf(b.db, String(origin.origin_id))) {
        const revision = readRevision(b.db, head)!;
        if (revision.control.tombstone) assert.ok(revision.parents.length > 0, 'a tombstone descends from the last materialized revision');
      }
    }
  });
});

test('a memory alias flip is not a change of the sources under it', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [lo, hi] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    insertMemory(hi.db, 'm_one', 'Title', 'Body text');
    insertSource(hi.db, 'm_one', 'src/a.ts');
    publish(hi, dir);
    insertMemory(lo.db, 'm_one', 'Title', 'Body text');
    // hi pulls lo's memory-only bundle: lo's origin sorts first and becomes canonical for the row.
    pull(hi, lo, publish(lo, dir));
    assert.equal(readOrigin(hi.db, `${hi.id}:m_one`)!.canonical_origin_id, `${lo.id}:m_one`);
    const before = revisionCount(hi.db);
    publish(hi, dir);
    assert.equal(revisionCount(hi.db), before, 'no phantom source revision');
    assert.equal(hi.db.prepare("SELECT COUNT(*) AS n FROM sync_revisions WHERE kind = 'source'").get()?.n, 1);
  });
});

test('an origin that stopped being canonical during the pass is neither written nor counted', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [lo, hi] = [...replicas].sort((p, q) => (p.id < q.id ? -1 : 1)) as [Replica, Replica];
    for (const device of [lo, hi]) { insertMemory(device.db, 'm_one', 'Title', 'Body text'); seedRawEvent(device.db); }
    lo.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'claude')").run();
    hi.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'codex')").run();
    pull(lo, hi, publish(hi, dir));
    // hi pulls lo: lo's origins claim hi's rows and become canonical while hi's own origins are still pending.
    const result = pull(hi, lo, publish(lo, dir));
    assert.equal(result.materialized, 2, 'one memory and one source');
    const stale = hi.db.prepare("SELECT selected_head, materialized_hash FROM sync_origins WHERE origin_id <> canonical_origin_id").all();
    assert.ok(stale.length > 0);
    assert.ok(stale.every((origin) => origin.selected_head === null && origin.materialized_hash === null), 'nothing recorded on a non-canonical origin');
  });
});

test('a dependency edge that is not context-only, or that names its own memory, is rejected before apply', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertMemory(a.db, 'm_two', 'Other', 'Other body');
    // A row the 0005 CHECK forbids, as a foreign writer could ship it.
    a.db.exec('PRAGMA ignore_check_constraints = ON');
    a.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_one', 'file_read', 'm_two', 0)").run();
    a.db.exec('PRAGMA ignore_check_constraints = OFF');
    assert.throws(() => pull(b, a, publish(a, dir)), (error: unknown) => error instanceof BundleRejected && error.code === 'invalid_source_edge');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 0);
  });
});

test('a terminal origin applied without a row aliases onto the row another origin creates in the same map-repo', async () => {
  await withHomes(3, (homes, shared) => {
    const [homeA, homeB, homeC] = homes as [string, string, string];
    const a = openHome(homeA);
    const b = openHome(homeB);
    const c = openHome(homeC);
    try {
      const [pathsA, pathsB, pathsC] = [oboetePaths(homeA), oboetePaths(homeB), oboetePaths(homeC)];
      a.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/a/.git' WHERE id = ?").run(REPO);
      b.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/b/.git' WHERE id = ?").run(REPO);
      const { keyLine } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      joinSpace(c, pathsC, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      insertMemory(a, 'm_one', 'Title', 'Body text');
      insertMemory(a, 'm_two', 'Other', 'Other body');
      insertMemory(b, 'm_one', 'Title', 'Body text');
      pushSpace(a, pathsA, { now: 5 });
      a.prepare("UPDATE memories SET deleted_at = 6 WHERE id = 'm_one'").run();
      pushSpace(a, pathsA, { now: 7 });
      pushSpace(b, pathsB, { now: 8 });
      pullSpace(c, pathsC, { now: 9 });
      mapRepo(c, pathsC, { repoKey: repoKeyFor(replicaOriginId(a), 'common_dir', '/work/a/.git'), localRepoId: REPO, now: 10 });
      mapRepo(c, pathsC, { repoKey: repoKeyFor(replicaOriginId(b), 'common_dir', '/work/b/.git'), localRepoId: REPO, now: 11 });
      const rows = c.prepare("SELECT deleted_at FROM memories WHERE material_hash = ?").all(materialHash('Title', 'Body text'));
      assert.equal(rows.length, 1, "B's row and A's tombstone are one row");
      assert.notEqual(rows[0]!.deleted_at, null, "A's tombstone reached the row B's payload created");
      assert.equal(readOrigin(c, `${replicaOriginId(a)}:m_one`)!.local_id, readOrigin(c, `${replicaOriginId(b)}:m_one`)!.local_id);
    } finally { a.close(); b.close(); c.close(); }
  });
});

test('a dependency edge that would close a cycle on this device is withheld', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    for (const device of [a, b]) {
      insertMemory(device.db, 'm_c', 'C', 'C body');
      insertMemory(device.db, 'm_d', 'D', 'D body');
    }
    // B: d depends on c. A: c depends on d. Each is acyclic alone.
    b.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_d', 'file_read', 'm_c', 1)").run();
    a.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_c', 'file_read', 'm_d', 1)").run();
    publish(b, dir);
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 1);
    const withheld = b.db.prepare("SELECT withheld_reason FROM sync_origins WHERE kind = 'source' AND withheld_reason IS NOT NULL").get();
    assert.equal(withheld?.withheld_reason, 'lineage_cycle');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, "A's edge is not written");
  });
});

// Round seven: one identity for a source (a random key; sameness only through a UNIQUE tuple),
// heads that exchange tuples in one pass, a bound head that meets a settled row, tombstones that
// reach rows later bundles create, resolve failures as coded errors, and a cycle that terminates.

test('two heads that exchange their UNIQUE tuples in one pass both land', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    seedRawEvent(a.db, 'raw_one');
    seedRawEvent(a.db, 'raw_two');
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'x')").run();
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_two', 'file_read', 1, 'y')").run();
    pull(b, a, publish(a, dir));
    a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_tmp' WHERE raw_event_id = 'raw_one'").run();
    a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_one' WHERE raw_event_id = 'raw_two'").run();
    a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_two' WHERE raw_event_id = 'raw_tmp'").run();
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0, 'neither head waits on the other');
    assert.deepEqual(b.db.prepare('SELECT raw_event_id, source_agent FROM memory_sources ORDER BY raw_event_id').all().map((row) => ({ ...row })),
      [{ raw_event_id: 'raw_one', source_agent: 'y' }, { raw_event_id: 'raw_two', source_agent: 'x' }]);
    assert.equal(conflicts(b.db), 0);
  });
});

test('a bound head whose tuple a settled row of another origin holds aliases onto that row, as the other device did', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    for (const device of [a, b]) { insertMemory(device.db, 'm_one', 'Title', 'Body text'); seedRawEvent(device.db, 'raw_one'); seedRawEvent(device.db, 'raw_two'); }
    a.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_two', 'file_read', 1, 'a')").run();
    pull(b, a, publish(a, dir));
    // B captures raw_one on its own; A then points its row at raw_one.
    b.db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, context_only, source_agent) VALUES ('m_one', 'raw_one', 'file_read', 1, 'b')").run();
    publish(b, dir);
    a.db.prepare("UPDATE memory_sources SET raw_event_id = 'raw_one'").run();
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 0, 'nothing is withheld for good');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'one row for raw_one');
    const groups = b.db.prepare("SELECT DISTINCT canonical_origin_id AS id FROM sync_origins WHERE kind = 'source' AND local_id IN (SELECT 'source:' || sync_key FROM memory_sources)").all();
    assert.equal(groups.length, 1, 'the two origins alias');
    pull(a, b, publish(b, dir));
    assert.equal(a.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'and A holds one row too');
  });
});

test('a source deleted and inserted again after its tombstone shipped is a new source that reaches the peer', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    const rowid = insertSource(a.db, 'm_one', 'src/a.ts');
    pull(b, a, publish(a, dir));
    a.db.prepare('DELETE FROM memory_sources WHERE id = ?').run(rowid);
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 0);
    insertSource(a.db, 'm_one', 'src/a.ts');
    pull(b, a, publish(a, dir));
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, 'the re-inserted row reaches B');
    pull(a, b, publish(b, dir));
    assert.equal(a.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 1, "and A's own row survives the echo of the log");
  });
});

test('a tombstone stored from an earlier bundle reaches the row a later bundle creates, whatever the bundle order', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertMemory(a.db, 'm_other', 'Other', 'Other body');
    publish(a, dir);
    a.db.prepare("UPDATE memories SET deleted_at = 5 WHERE id = 'm_one'").run();
    const fromA = publish(a, dir);
    insertMemory(b.db, 'm_b', 'Title', 'Body text');
    const fromB = publish(b, dir);
    // C pulls the tombstone first (no row to bind), then the live copy.
    pull(c, a, fromA);
    assert.equal(readOrigin(c.db, `${a.id}:m_one`)!.local_id, null);
    pull(c, b, fromB);
    assert.equal(readOrigin(c.db, `${a.id}:m_one`)!.local_id, readOrigin(c.db, `${b.id}:m_b`)!.local_id, 'the tombstone binds to the row B created');
    assert.notEqual(memoryOf(c, b, 'm_b').deleted_at, null, 'and applies');
  });
});

test('a withheld origin does not stop late binding for the others in the same pass', async () => {
  await withHomes(3, (homes, shared) => {
    const [homeA, homeB, homeC] = homes as [string, string, string];
    const a = openHome(homeA);
    const b = openHome(homeB);
    const c = openHome(homeC);
    try {
      const [pathsA, pathsB, pathsC] = [oboetePaths(homeA), oboetePaths(homeB), oboetePaths(homeC)];
      a.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/a/.git' WHERE id = ?").run(REPO);
      b.prepare("UPDATE repos SET identity_kind = 'common_dir', normalized_identity = '/work/b/.git' WHERE id = ?").run(REPO);
      const { keyLine } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      joinSpace(c, pathsC, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
      // A also holds a memory in a repository C never maps: that origin stays withheld throughout.
      a.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
        VALUES ('repo_u', 'common_dir', '/work/u/.git', '/work/u', 1, 1)`).run();
      a.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity, review_state, created_at)
        VALUES ('m_u', 'repo_u', 'discovery', 'U', 'U body', '', ?, ?, 'eligible', 'reviewed', 1)`).run(materialHash('U', 'U body'), sha256Json(['repo_u', materialHash('U', 'U body')]));
      insertMemory(a, 'm_one', 'Title', 'Body text');
      insertMemory(a, 'm_other', 'Other', 'Other body');
      insertMemory(b, 'm_one', 'Title', 'Body text');
      pushSpace(a, pathsA, { now: 5 });
      a.prepare("UPDATE memories SET deleted_at = 6 WHERE id = 'm_one'").run();
      pushSpace(a, pathsA, { now: 7 });
      pushSpace(b, pathsB, { now: 8 });
      pullSpace(c, pathsC, { now: 9 });
      mapRepo(c, pathsC, { repoKey: repoKeyFor(replicaOriginId(a), 'common_dir', '/work/a/.git'), localRepoId: REPO, now: 10 });
      mapRepo(c, pathsC, { repoKey: repoKeyFor(replicaOriginId(b), 'common_dir', '/work/b/.git'), localRepoId: REPO, now: 11 });
      assert.equal(readOrigin(c, `${replicaOriginId(a)}:m_u`)!.withheld_reason, 'unmapped_repo', 'the unrelated origin stays withheld');
      const rows = c.prepare('SELECT deleted_at FROM memories WHERE material_hash = ?').all(materialHash('Title', 'Body text'));
      assert.equal(rows.length, 1);
      assert.notEqual(rows[0]!.deleted_at, null, "A's tombstone reached the row B's payload created");
    } finally { a.close(); b.close(); c.close(); }
  });
});

test('a resolve whose kept head the writer cannot apply fails with a coded error', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    for (const device of [a, b]) { insertMemory(device.db, 'm_c', 'C', 'C body'); insertMemory(device.db, 'm_d', 'D', 'D body'); }
    b.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_d', 'file_read', 'm_c', 1)").run();
    a.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_c', 'file_read', 'm_d', 1)").run();
    publish(b, dir);
    pull(b, a, publish(a, dir));
    const withheld = b.db.prepare("SELECT origin_id, selected_head FROM sync_origins WHERE kind = 'source' AND withheld_reason = 'lineage_cycle'").get()!;
    b.db.exec('BEGIN IMMEDIATE');
    assert.throws(() => resolveRow(b.db, String(withheld.origin_id), String(withheld.selected_head), 300),
      (error: unknown) => error instanceof ResolveError && error.code === 'lineage_cycle');
    b.db.exec('ROLLBACK');
  });
});

test('a dependency edge that closes a cycle deeper than any walk bound is still withheld', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    // B holds the chain m_1 -> m_0, m_2 -> m_1, ... : m_0 reaches the last memory 4097 edges away.
    const depth = 4098;
    for (let i = 0; i < depth; i += 1) insertMemory(b.db, `m_${i}`, `M${i}`, `Body ${i}`);
    const edge = b.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES (?, 'file_read', ?, 1)");
    for (let i = 1; i < depth; i += 1) edge.run(`m_${i}`, `m_${i - 1}`);
    // A, which holds only the two ends, records that m_0 depends on the last one: acyclic on A, a cycle on B.
    insertMemory(a.db, 'm_0', 'M0', 'Body 0');
    insertMemory(a.db, `m_${depth - 1}`, `M${depth - 1}`, `Body ${depth - 1}`);
    a.db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, source_memory_id, context_only) VALUES ('m_0', 'file_read', ?, 1)").run(`m_${depth - 1}`);
    const result = pull(b, a, publish(a, dir));
    assert.equal(result.withheldOnApply, 1, 'the edge that closes the cycle is withheld, however long the way round');
    assert.deepEqual(b.db.prepare('SELECT withheld_reason FROM sync_origins WHERE withheld_reason IS NOT NULL').all().map((row) => row.withheld_reason), ['lineage_cycle']);
    assert.equal(b.db.prepare("SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = 'm_0'").get()?.n, 0);
  });
});
