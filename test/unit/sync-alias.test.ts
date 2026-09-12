// US6 device sync, the aliasing bullets of contracts/sync.md "Verification": natural-key alias,
// alias after mapping, mapped alias and repository keys. All of them turn on one rule from "Merge
// rules": an origin whose `natural` names a row this device already holds is mapped onto that row
// (canonical origin = the smallest origin id), so one local row carries the union of its origins'
// heads, one conflict report and one materialized state.
import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { sha256Hex, sha256Json } from '../../src/hash.js';
import { oboetePaths } from '../../src/paths.js';
import type { OboetePaths } from '../../src/paths.js';
import { resolveRow } from '../../src/sync/apply.js';
import type { Sensitivity } from '../../src/sync/identity.js';
import { initSpace, joinSpace, mapRepo, pullSpace, pushSpace } from '../../src/sync/space.js';
import {
  canonicalOf, effectiveControl, headsOf, localRepoOf, originsOfRow, readOrigin, readRevision, replicaOriginId,
} from '../../src/sync/store.js';
import {
  insertMemory, openHome, publish, pull, REMOTE, REPO, revisionCount, withHomes, withReplicas, type Replica,
} from '../helpers/sync.js';

const CLASSES: Sensitivity[] = ['eligible', 'local_only', 'private'];

/** The shared fixture pins one repository; these cases need memories in others. */
function insertIn(db: DatabaseSync, repo: string, id: string, title: string, body: string,
  extra: { sensitivity?: string; deleted_at?: number | null; content?: string } = {}): { material: string; content: string } {
  const material = sha256Json([title.toLowerCase(), body.toLowerCase()]);
  const content = extra.content ?? sha256Json([repo, material]);
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity,
    review_state, created_at, deleted_at) VALUES (?, ?, 'discovery', ?, ?, '', ?, ?, ?, 'reviewed', 1, ?)`)
    .run(id, repo, title, body, material, content, extra.sensitivity ?? 'eligible', extra.deleted_at ?? null);
  return { material, content };
}

function addRepo(db: DatabaseSync, id: string, kind: 'remote' | 'common_dir', identity: string): string {
  db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
    VALUES (?, ?, ?, '/work/other', 1, 1)`).run(id, kind, identity);
  return id;
}

/** What `memoryRecords` reads to call a memory a personal projection (0007 migration receipts). */
function markProjection(db: DatabaseSync, memoryId: string): void {
  db.prepare(`INSERT OR IGNORE INTO migration_imports (id, source_format, source_revision, source_sha256, mapping_json,
    mapping_hash, counts_json, imported_at) VALUES ('imp', 'native/2', 'r1', 'ff', '{}', 'ff', '{}', 1)`).run();
  db.prepare(`INSERT INTO migration_records (id, origin_key, origin_json, target_key, first_import_id, record_kind,
    payload_hash, destination_memory_id, identity_domain, effect, classification_state, detail_code)
    VALUES (?, ?, '{}', ?, 'imp', 'memory', 'ff', ?, 'personal_projection', 'inserted', 'clean', 'projected')`)
    .run(`rec_${memoryId}`, `key_${memoryId}`, memoryId, memoryId);
}

type Device = { db: DatabaseSync; paths: OboetePaths; id: string };

/** One space over N homes: the first device creates it, the rest join with its key line. */
function joinAll(homes: string[], shared: string): Device[] {
  const devices = homes.map((home) => {
    const db = openHome(home);
    return { db, paths: oboetePaths(home), id: replicaOriginId(db) };
  });
  const first = devices[0]!;
  const { keyLine } = initSpace(first.db, first.paths, { directory: shared, classes: CLASSES, now: 1 });
  for (const device of devices.slice(1)) joinSpace(device.db, device.paths, { directory: shared, keyLine, classes: CLASSES, now: 1 });
  return devices;
}

function memoryRow(db: DatabaseSync, originId: string): Record<string, unknown> {
  const local = readOrigin(db, originId)?.local_id;
  assert.ok(local, `${originId} has a local row`);
  return db.prepare('SELECT * FROM memories WHERE id = ?').get(local)!;
}

function count(db: DatabaseSync, sql: string, ...args: string[]): number {
  return Number(db.prepare(sql).get(...args)?.n);
}

function commonDirKey(replica: string, path: string): string {
  return `${replica}:common_dir:${sha256Hex(path)}`;
}

// --- natural-key alias ---

test('two devices that captured the same material map onto one row, and a tombstone under either origin deletes it on both', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_a1', 'Shared', 'Same material');
    insertMemory(b.db, 'm_b1', 'Shared', 'Same material');
    insertMemory(a.db, 'm_a2', 'Second', 'Also captured twice');
    insertMemory(b.db, 'm_b2', 'Second', 'Also captured twice');
    const fromA = publish(a, dir);
    const fromB = publish(b, dir);
    pull(a, b, fromB);
    pull(b, a, fromA);
    const pairs: [string, string][] = [[`${a.id}:m_a1`, `${b.id}:m_b1`], [`${a.id}:m_a2`, `${b.id}:m_b2`]];
    for (const device of [a, b]) {
      assert.equal(count(device.db, 'SELECT COUNT(*) AS n FROM memories'), 2, 'the second origin mapped onto the existing row');
      for (const [first, second] of pairs) {
        const one = readOrigin(device.db, first)!;
        const two = readOrigin(device.db, second)!;
        assert.ok(one.local_id !== null && one.local_id === two.local_id, 'both origins name one local row');
        const smallest = [first, second].sort()[0]!;
        assert.equal(canonicalOf(device.db, first).origin_id, smallest, 'the canonical origin is the smallest origin id');
        assert.equal(canonicalOf(device.db, second).origin_id, smallest);
        assert.equal(originsOfRow(device.db, 'memory', one.local_id).length, 2);
        assert.equal(headsOf(device.db, smallest).length, 2, 'the head set is the union across the origins');
      }
    }
    // A deletes the first row and B the second: a tombstone under either origin reaches both.
    a.db.prepare('UPDATE memories SET deleted_at = 7 WHERE id = ?').run('m_a1');
    b.db.prepare('UPDATE memories SET deleted_at = 8 WHERE id = ?').run('m_b2');
    pull(b, a, publish(a, dir));
    pull(a, b, publish(b, dir));
    for (const device of [a, b]) {
      for (const [first] of pairs) {
        assert.notEqual(memoryRow(device.db, first).deleted_at, null, `${first} is deleted on ${device.id}`);
        assert.equal(effectiveControl(device.db, canonicalOf(device.db, first).origin_id).tombstone, true);
      }
    }
  });
});

test('control for material the other device never saw maps through natural and applies, canonical origin or not', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    // Neither device has published, so each one's first sight of the other's origin is a
    // control-only revision: the deleted and the secret row ship without a payload.
    insertMemory(a.db, 'm_gone', 'Gone', 'Deleted on A', { deleted_at: 5 });
    insertMemory(a.db, 'm_sec', 'Secret', 'Secret on A', { sensitivity: 'secret' });
    insertMemory(a.db, 'm_kept', 'Kept', 'Deleted on B');
    insertMemory(b.db, 'b_gone', 'Gone', 'Deleted on A');
    insertMemory(b.db, 'b_sec', 'Secret', 'Secret on A');
    insertMemory(b.db, 'b_kept', 'Kept', 'Deleted on B', { deleted_at: 6 });
    pull(b, a, publish(a, dir));
    pull(a, b, publish(b, dir));
    for (const origin of [`${a.id}:m_gone`, `${a.id}:m_sec`]) {
      assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ? AND payload_json IS NOT NULL', origin),
        0, 'nothing but the identity fields carried the alias');
    }
    assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM memories'), 3, 'B kept its own three rows');
    assert.equal(readOrigin(b.db, `${a.id}:m_gone`)!.local_id, 'b_gone');
    assert.notEqual(memoryRow(b.db, `${a.id}:m_gone`).deleted_at, null, "A's tombstone deleted B's copy");
    assert.equal(memoryRow(b.db, `${a.id}:m_sec`).sensitivity, 'secret');
    assert.equal(memoryRow(b.db, `${a.id}:m_sec`).body, '');
    assert.equal(readOrigin(a.db, `${b.id}:b_kept`)!.local_id, 'm_kept');
    assert.notEqual(memoryRow(a.db, `${b.id}:b_kept`).deleted_at, null, "B's tombstone deleted A's copy");
    // Whichever replica id sorts first, exactly one of the two applied tombstones sits on an
    // origin that is not its row's canonical one.
    const carrier = a.id < b.id ? { db: a.db, origin: `${b.id}:b_kept` } : { db: b.db, origin: `${a.id}:m_gone` };
    assert.notEqual(canonicalOf(carrier.db, carrier.origin).origin_id, carrier.origin);
    assert.equal(effectiveControl(carrier.db, canonicalOf(carrier.db, carrier.origin).origin_id).tombstone, true);
  });
});

test('a repository identity of 16,384 characters round-trips through its hashed key', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    const identity = `example.com/${'a'.repeat(16_384 - 'example.com/'.length)}`;
    assert.equal(identity.length, 16_384);
    const local = addRepo(a.db, sha256Hex(identity).slice(0, 16), 'remote', identity);
    insertIn(a.db, local, 'm_far', 'Far away', 'In a long repository');
    pull(b, a, publish(a, dir));
    const key = `remote:${sha256Hex(identity)}`;
    assert.equal(key.length, 71);
    assert.equal(readOrigin(b.db, `${a.id}:m_far`)!.natural.repo, key, 'the reference carries the fixed-size key');
    const mapping = b.db.prepare('SELECT normalized_identity, local_repo_id FROM sync_repo_mappings WHERE repo_key = ?').get(key)!;
    assert.equal(mapping.normalized_identity, identity, 'the repo line carries the identity in full');
    assert.equal(String(mapping.normalized_identity).length, 16_384);
    assert.equal(b.db.prepare('SELECT normalized_identity FROM repos WHERE id = ?').get(mapping.local_repo_id)?.normalized_identity, identity);
    assert.equal(memoryRow(b.db, `${a.id}:m_far`).repo_id, mapping.local_repo_id, 'the memory landed in the repository the key resolved to');
    assert.equal(memoryRow(b.db, `${a.id}:m_far`).body, 'In a long repository');
    assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM repos WHERE normalized_identity = ?', REMOTE), 1, 'the fixture repository is untouched');
  });
});

/** A and B capture the same material and each edits its own copy before it sees the other origin,
 * so the row they end up sharing has one head under each origin. */
function divergeAcrossAliases(a: Replica, b: Replica, dir: string): { originA: string; originB: string; canonical: string } {
  insertMemory(a.db, 'm_a', 'Shared', 'Same material');
  insertMemory(b.db, 'm_b', 'Shared', 'Same material');
  publish(a, dir);
  publish(b, dir);
  a.db.prepare('UPDATE memories SET pinned_at = 5, pin_order = 1 WHERE id = ?').run('m_a');
  b.db.prepare('UPDATE memories SET valid_to = 9 WHERE id = ?').run('m_b');
  const fromA = publish(a, dir);
  const fromB = publish(b, dir);
  pull(a, b, fromB);
  pull(b, a, fromA);
  const originA = `${a.id}:m_a`;
  const originB = `${b.id}:m_b`;
  return { originA, originB, canonical: [originA, originB].sort()[0]! };
}

function headOfOrigin(db: DatabaseSync, canonical: string, originId: string): string {
  const head = headsOf(db, canonical).find((candidate) => readRevision(db, candidate)!.origin_id === originId);
  assert.ok(head, `a head under ${originId}`);
  return head;
}

test('divergent edits across aliases report one conflict naming every head with its origin, and one resolve converges both devices', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    const { originA, originB, canonical } = divergeAcrossAliases(a, b, dir);
    for (const device of [a, b]) {
      const heads = headsOf(device.db, canonical);
      assert.equal(heads.length, 2);
      assert.equal(count(device.db, "SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'"), 1,
        'one report for the row, not one per origin');
      const report = device.db.prepare('SELECT status, remote_state_json FROM sync_conflicts WHERE id = ?').get(`sync:${canonical}`)!;
      assert.equal(report.status, 'open');
      const detail = JSON.parse(String(report.remote_state_json)) as { heads: { revision: string; origin: string }[] };
      assert.deepEqual(detail.heads.map((head) => head.revision).sort(), [...heads].sort());
      assert.deepEqual(detail.heads.map((head) => head.origin).sort(), [originA, originB].sort(), 'each head is named with its origin');
    }
    // One resolve, addressed to the origin that is not the canonical one when A sorts second.
    a.db.exec('BEGIN IMMEDIATE');
    const { revision_id } = resolveRow(a.db, originA, headOfOrigin(a.db, canonical, canonical), 300);
    a.db.exec('COMMIT');
    pull(b, a, publish(a, dir));
    const kept = canonical === originA ? { pinned_at: 5, valid_to: null } : { pinned_at: null, valid_to: 9 };
    for (const device of [a, b]) {
      assert.deepEqual(headsOf(device.db, canonical), [revision_id], 'the resolution is the single head');
      assert.equal(memoryRow(device.db, originA).pinned_at, kept.pinned_at, 'both devices show the kept line');
      assert.equal(memoryRow(device.db, originA).valid_to, kept.valid_to);
      assert.equal(memoryRow(device.db, originB).id, memoryRow(device.db, originA).id, 'still one row');
      assert.equal(count(device.db, "SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'"), 0);
      // A stale materialized_hash would make the next change pass invent a revision.
      const before = revisionCount(device.db);
      publish(device, dir);
      assert.equal(revisionCount(device.db), before, 'the row matches the state the resolution materialized');
    }
  });
});

test('a resolve that keeps the head of the other aliased origin converges too', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    const { originA, originB, canonical } = divergeAcrossAliases(a, b, dir);
    const other = canonical === originA ? originB : originA;
    a.db.exec('BEGIN IMMEDIATE');
    const { revision_id } = resolveRow(a.db, originA, headOfOrigin(a.db, canonical, other), 300);
    a.db.exec('COMMIT');
    pull(b, a, publish(a, dir));
    const kept = other === originA ? { pinned_at: 5, valid_to: null } : { pinned_at: null, valid_to: 9 };
    for (const device of [a, b]) {
      assert.deepEqual(headsOf(device.db, canonical), [revision_id]);
      assert.equal(memoryRow(device.db, originA).pinned_at, kept.pinned_at);
      assert.equal(memoryRow(device.db, originA).valid_to, kept.valid_to);
      assert.equal(count(device.db, "SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'"), 0);
    }
  });
});

test('a personal projection maps onto the local row across repositories', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    // A projection's identity is the hash of its text, so both devices hold the same wording.
    const projection = sha256Json(['personal-projection-v1', 'Projected', 'Shared wording']);
    insertIn(a.db, REPO, 'm_proj', 'Projected', 'Shared wording', { content: projection });
    markProjection(a.db, 'm_proj');
    const other = addRepo(b.db, 'r_other', 'remote', 'example.com/other');
    insertIn(b.db, other, 'b_proj', 'Projected', 'Shared wording', { content: projection });
    pull(b, a, publish(a, dir));
    const origin = readOrigin(b.db, `${a.id}:m_proj`)!;
    assert.equal(origin.natural.domain, 'personal_projection');
    assert.equal(origin.natural.projection_hash, projection, 'the natural key is the projection hash alone');
    assert.equal(origin.local_id, 'b_proj', 'the projection mapped onto the row B already held');
    assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM memories'), 1, 'no second row was inserted');
    assert.equal(originsOfRow(b.db, 'memory', 'b_proj').length, 2);
    const heads = headsOf(b.db, canonicalOf(b.db, `${a.id}:m_proj`).origin_id);
    assert.equal(heads.length, 2, 'one row carrying the union of both origins heads');
    // The row it mapped onto lives in another repository than the arriving payload names.
    const fromA = heads.map((head) => readRevision(b.db, head)!).find((revision) => revision.origin_id === `${a.id}:m_proj`)!;
    assert.equal(localRepoOf(b.db, String(fromA.payload!.repo_id)), REPO, "the payload names A's repository");
    assert.equal(memoryRow(b.db, `${a.id}:m_proj`).repo_id, other, "the aliased row stays in B's repository");
  });
});

// --- alias after mapping, mapped alias and repository keys (space level) ---

test('after map-repo a common_dir repository maps material the other device captured independently, and a later secret floor applies', async () => {
  await withHomes(2, (homes, shared) => {
    const devices = joinAll(homes, shared);
    const [a, b] = devices as [Device, Device];
    try {
      addRepo(a.db, 'cd_a', 'common_dir', '/work/a/project');
      addRepo(b.db, 'cd_b', 'common_dir', '/work/b/project');
      insertIn(a.db, 'cd_a', 'm_a', 'Local repo', 'Captured on both');
      insertIn(b.db, 'cd_b', 'm_b', 'Local repo', 'Captured on both');
      const key = commonDirKey(a.id, '/work/a/project');
      assert.equal(pushSpace(a.db, a.paths, { now: 10 }).outcome, 'published');
      const first = pullSpace(b.db, b.paths, { now: 20 }).bundles[0]!;
      assert.equal(first.outcome, 'applied');
      assert.equal(first.result!.withheldOnApply, 1, 'a foreign common_dir key never resolves by itself');
      assert.equal(localRepoOf(b.db, key), null);
      assert.equal(readOrigin(b.db, `${a.id}:m_a`)!.local_id, null);
      assert.equal(readOrigin(b.db, `${a.id}:m_a`)!.withheld_reason, 'unmapped_repo');
      assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM memories'), 1);
      // The mapping makes material hash plus mapped repository name B's own row.
      assert.equal(mapRepo(b.db, b.paths, { repoKey: key, localRepoId: 'cd_b', now: 30 }).reapplied, 1);
      assert.equal(readOrigin(b.db, `${a.id}:m_a`)!.local_id, 'm_b');
      assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM memories'), 1, 'mapped onto the row, not inserted beside it');
      assert.equal(originsOfRow(b.db, 'memory', 'm_b').length, 2);
      // A's later secret floor travels as control and reaches the row it now shares.
      a.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = 'm_a'").run();
      assert.equal(pushSpace(a.db, a.paths, { now: 40 }).outcome, 'published');
      pullSpace(b.db, b.paths, { now: 50 });
      assert.equal(memoryRow(b.db, `${a.id}:m_a`).sensitivity, 'secret');
      assert.equal(memoryRow(b.db, `${a.id}:m_a`).body, '');
      assert.equal(effectiveControl(b.db, canonicalOf(b.db, `${a.id}:m_a`).origin_id).sensitivity_floor, 'secret');
      // A tombstone on the same material travels the same way.
      a.db.prepare("UPDATE memories SET deleted_at = 60 WHERE id = 'm_a'").run();
      assert.equal(pushSpace(a.db, a.paths, { now: 61 }).outcome, 'published');
      pullSpace(b.db, b.paths, { now: 62 });
      assert.notEqual(memoryRow(b.db, `${a.id}:m_a`).deleted_at, null);
      assert.equal(effectiveControl(b.db, canonicalOf(b.db, `${a.id}:m_a`).origin_id).tombstone, true);
    } finally { for (const device of devices) device.db.close(); }
  });
});

test('two devices with the same common_dir path do not merge, and the mapped key applies the payloads that were withheld', async () => {
  await withHomes(2, (homes, shared) => {
    const devices = joinAll(homes, shared);
    const [a, b] = devices as [Device, Device];
    try {
      addRepo(a.db, 'cd_a', 'common_dir', '/work/same/project');
      addRepo(b.db, 'cd_b', 'common_dir', '/work/same/project');
      insertIn(a.db, 'cd_a', 'm_one', 'First', 'Only on A');
      insertIn(a.db, 'cd_a', 'm_two', 'Second', 'Only on A');
      const key = commonDirKey(a.id, '/work/same/project');
      assert.notEqual(key, commonDirKey(b.id, '/work/same/project'), 'one path is a different key on each device');
      pushSpace(a.db, a.paths, { now: 10 });
      const pulled = pullSpace(b.db, b.paths, { now: 20 }).bundles[0]!;
      assert.equal(pulled.result!.withheldOnApply, 2);
      assert.equal(localRepoOf(b.db, key), null, 'an identical path does not merge the repositories');
      assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM memories'), 0);
      assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM repos'), 2, 'no repository was created for the foreign key');
      assert.equal(mapRepo(b.db, b.paths, { repoKey: key, localRepoId: 'cd_b', now: 30 }).reapplied, 2, 'the mapped key applies both withheld payloads');
      assert.equal(count(b.db, 'SELECT COUNT(*) AS n FROM memories WHERE repo_id = ?', 'cd_b'), 2);
      assert.equal(memoryRow(b.db, `${a.id}:m_one`).body, 'Only on A');
      // The mapping holds for later pulls: the next payload of that repository applies at once.
      insertIn(a.db, 'cd_a', 'm_three', 'Third', 'Later on A');
      pushSpace(a.db, a.paths, { now: 40 });
      const later = pullSpace(b.db, b.paths, { now: 50 }).bundles[0]!;
      assert.equal(later.result!.withheldOnApply, 0);
      assert.equal(memoryRow(b.db, `${a.id}:m_three`).body, 'Later on A');
    } finally { for (const device of devices) device.db.close(); }
  });
});

test('a resolve spanning both origins of a mapped repository applies on the mapper, and on a third device only after it maps the repository too', async () => {
  await withHomes(3, (homes, shared) => {
    const devices = joinAll(homes, shared);
    const [a, b, c] = devices as [Device, Device, Device];
    try {
      addRepo(a.db, 'cd_a', 'common_dir', '/work/a');
      addRepo(b.db, 'cd_b', 'common_dir', '/work/b');
      addRepo(c.db, 'cd_c', 'common_dir', '/work/c');
      insertIn(a.db, 'cd_a', 'm_a', 'Same', 'material on both');
      insertIn(b.db, 'cd_b', 'm_b', 'Same', 'material on both');
      const originA = `${a.id}:m_a`;
      const originB = `${b.id}:m_b`;
      const canonical = [originA, originB].sort()[0]!;
      pushSpace(a.db, a.paths, { now: 10 });
      pullSpace(b.db, b.paths, { now: 20 });
      mapRepo(b.db, b.paths, { repoKey: commonDirKey(a.id, '/work/a'), localRepoId: 'cd_b', now: 21 });
      assert.equal(headsOf(b.db, canonical).length, 2, 'the mapping unions the two origins into one head set');
      pushSpace(b.db, b.paths, { now: 22 });
      // C holds neither key: each origin keeps its own heads and has no row at all.
      assert.deepEqual(pullSpace(c.db, c.paths, { now: 30 }).bundles.map((entry) => entry.outcome), ['applied', 'applied']);
      assert.notEqual(canonicalOf(c.db, originA).origin_id, canonicalOf(c.db, originB).origin_id, 'until the repository is mapped each origin is its own row');
      assert.equal(headsOf(c.db, canonicalOf(c.db, originA).origin_id).length, 1);
      assert.equal(headsOf(c.db, canonicalOf(c.db, originB).origin_id).length, 1);
      assert.equal(count(c.db, 'SELECT COUNT(*) AS n FROM memories'), 0);
      // B resolves across the two origins.
      const heads = headsOf(b.db, canonical);
      b.db.exec('BEGIN IMMEDIATE');
      const { revision_id } = resolveRow(b.db, originB, headOfOrigin(b.db, canonical, canonical), 40);
      b.db.exec('COMMIT');
      const parents = readRevision(b.db, revision_id)!.parents;
      assert.deepEqual([...parents].sort(), [...heads].sort());
      assert.deepEqual([...new Set(parents.map((parent) => readRevision(b.db, parent)!.origin_id))].sort(),
        [originA, originB].sort(), 'the parents span both origins');
      assert.deepEqual(headsOf(b.db, canonical), [revision_id]);
      assert.equal(count(b.db, "SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'"), 0);
      pushSpace(b.db, b.paths, { now: 41 });
      // C maps both repositories: the origins become one row, and then the resolution applies.
      mapRepo(c.db, c.paths, { repoKey: commonDirKey(a.id, '/work/a'), localRepoId: 'cd_c', now: 50 });
      mapRepo(c.db, c.paths, { repoKey: commonDirKey(b.id, '/work/b'), localRepoId: 'cd_c', now: 51 });
      assert.equal(canonicalOf(c.db, originA).origin_id, canonicalOf(c.db, originB).origin_id);
      assert.equal(count(c.db, 'SELECT COUNT(*) AS n FROM memories'), 1);
      pullSpace(c.db, c.paths, { now: 60 });
      assert.deepEqual(headsOf(c.db, canonical), [revision_id], 'the resolution validates and applies on the third device');
      assert.equal(memoryRow(c.db, originB).id, memoryRow(c.db, originA).id);
      assert.equal(memoryRow(c.db, originA).body, 'material on both');
      assert.equal(count(c.db, "SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'"), 0);
    } finally { for (const device of devices) device.db.close(); }
  });
});
