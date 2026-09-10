import assert from 'node:assert/strict';
import test from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { grantVisibility } from '../../src/db/queries.js';
import { sha256Hex, sha256Json } from '../../src/hash.js';
import { importMemories, runExport } from '../../src/transfer.js';
import { NATIVE_FORMAT, NATIVE_REVISION } from '../../src/transfer-format.js';
import { insertMemory, insertSession, withFixture, type Fixture } from '../helpers/inject-fixture.js';
import { output } from '../helpers/output.js';
import { seedWorkBinding } from '../helpers/work.js';

// Codex security review 2026-09-11 (arch + g2, high): a redacted personal_projection record carries
// its content_hash as an unverifiable wire value. It must never select an ordinary memory.

const memoryDefaults = {
  kind: 'memory', concepts: '[]', sensitivity: 'local_only', review_state: 'reviewed', degraded_reason: null,
  source_session_id: null, source_batch_id: null, source_agent: null, valid_from: null, valid_to: null,
  superseded_by: null, pinned_at: null, pin_order: null, created_at: 1, work_id: null,
  checkpoint_parent_id: null, provenance_complete: 1, source_captured_at: 1,
};

const materialOf = (text: string) => materialHash(text, text);
const contentOf = (fixture: Fixture, text: string) => contentHash(fixture.identity.id, materialOf(text));
const ordinaryRecord = (fixture: Fixture, id: string, text: string, sensitivity: string) => ({ ...memoryDefaults, id,
  repo_id: fixture.identity.id, type: 'discovery', title: text, body: text, material_hash: materialOf(text),
  content_hash: contentOf(fixture, text), identity_domain: 'ordinary', deleted_at: null, sensitivity });

function file(fixture: Fixture, records: Record<string, unknown>[]): string {
  const lines = [
    { format: NATIVE_FORMAT, revision: NATIVE_REVISION, origin_id: 'b'.repeat(32), exported_at: 1 },
    { kind: 'repo', id: fixture.identity.id, identity_kind: fixture.identity.identityKind,
      normalized_identity: fixture.identity.normalizedIdentity },
    ...records,
  ];
  return lines.map((record) => JSON.stringify(record)).join('\n') + '\n';
}

function redactedPersonalTombstone(fixture: Fixture, contentHash: string) {
  return { ...memoryDefaults, id: 'foreign-personal-tombstone', repo_id: fixture.identity.id, type: 'decision',
    title: '', body: '', material_hash: 'c'.repeat(64), content_hash: contentHash,
    identity_domain: 'personal_projection', deleted_at: 5 };
}

test('a redacted personal tombstone cannot select an ordinary memory by its wire content hash', async () => {
  await withFixture(async (fixture) => {
    insertMemory(fixture, { id: 'victim', title: 'Victim', body: 'Ordinary local knowledge.' });
    const victimHash = contentOf(fixture, 'Victim');
    fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ? WHERE id = 'victim'").run(materialOf('Victim'), victimHash);
    const otherHash = '1'.repeat(64);
    fixture.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('other-repo', 'common_dir', '/other', '/other', 1, 1)`).run();
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
      sensitivity, review_state, created_at) VALUES ('other-victim', 'other-repo', 'discovery', 'Other', 'Elsewhere.', '',
      ?, ?, 'eligible', 'unreviewed', 1)`).run('2'.repeat(64), otherHash);
    const before = fixture.db.prepare('SELECT id, deleted_at, sensitivity FROM memories ORDER BY id').all();

    for (const target of [victimHash, otherHash]) {
      const result = await importMemories(fixture.db, file(fixture, [redactedPersonalTombstone(fixture, target)]), { now: 10 });
      assert.deepEqual(result.rejected, []);
      assert.equal(result.tombstones, 0, `${target}: no tombstone effect`);
    }
    assert.deepEqual(fixture.db.prepare('SELECT id, deleted_at, sensitivity FROM memories ORDER BY id').all(), before);
    assert.equal(fixture.db.prepare('SELECT COUNT(*) AS n FROM memories WHERE content_hash NOT IN (?, ?)').get(victimHash, otherHash)?.n, 0,
      'no blocking tombstone row is inserted under the attacker-chosen hash');
    assert.deepEqual(fixture.db.prepare("SELECT effect FROM migration_records WHERE record_kind = 'memory' ORDER BY id").all().map((r) => r.effect),
      ['historical_held', 'historical_held']);
  });
});

test('a redacted personal tombstone still deletes a real local personal projection', async () => {
  await withFixture(async (fixture) => {
    insertMemory(fixture, { id: 'origin', title: 'Personal preference', body: 'Reply in Japanese.' });
    insertSession(fixture, { id: 'session', agent: 'claude' });
    seedWorkBinding(fixture.db, 'session');
    const hash = sha256Json(['personal-projection-v1', 'Personal preference', 'Reply in Japanese.']);
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
      sensitivity, review_state, created_at) VALUES ('projection', ?, 'decision', 'Personal preference', 'Reply in Japanese.', '',
      ?, ?, 'local_only', 'reviewed', 1)`).run(fixture.identity.id, 'd'.repeat(64), hash);
    fixture.db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title,
      candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, decision_channel,
      projected_memory_id, created_at, decided_at) VALUES ('sp_local', 'origin', ?, ?, 'Personal preference', 'Reply in Japanese.',
      ?, 'local_only', '[]', 'inferred', 'approved', 'cli', 'projection', 1, 1)`)
      .run(fixture.identity.id, `fixture-work:${fixture.identity.id}`, 'd'.repeat(64));
    grantVisibility(fixture.db, 'projection', { audience: 'personal', proposalId: 'sp_local' }, 'proposal_approval', 1);
    const result = await importMemories(fixture.db, file(fixture, [redactedPersonalTombstone(fixture, hash)]), { now: 10 });
    assert.deepEqual(result.rejected, []);
    assert.equal(result.tombstones, 1);
    assert.equal(fixture.db.prepare("SELECT deleted_at FROM memories WHERE id = 'projection'").get()?.deleted_at, 5);
  });
});

// Codex security review 2026-09-11 (g2, medium): the sensitivity decision must read the live row,
// not the plan cache, because a dependency trigger can raise it between two records of one file.
test('a later record cannot lower a sensitivity that a dependency trigger raised during the same import', async () => {
  await withFixture(async (fixture) => {
    insertMemory(fixture, { id: 'dependent', title: 'Dependent', body: 'Derived from the base.' });
    insertMemory(fixture, { id: 'base', title: 'Base', body: 'Base material.' });
    fixture.db.prepare(`INSERT INTO memory_sources (memory_id, source_memory_id, context_only) VALUES ('dependent', 'base', 1)`).run();
    // Ordinary identity is repo + material, so both wire records collide with the local rows through their material.
    fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ? WHERE id = 'dependent'")
      .run(materialOf('Dependent-A'), contentOf(fixture, 'Dependent-A'));
    fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ? WHERE id = 'base'")
      .run(materialOf('Base-B'), contentOf(fixture, 'Base-B'));
    const records = [
      ordinaryRecord(fixture, 'a-first', 'Dependent-A', 'eligible'),
      { ...ordinaryRecord(fixture, 'b-secret', 'Base-B', 'secret'), title: '', body: '' },
      ordinaryRecord(fixture, 'a-again', 'Dependent-A', 'private'),
    ];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, []);
    assert.equal(fixture.db.prepare("SELECT sensitivity FROM memories WHERE id = 'base'").get()?.sensitivity, 'secret');
    assert.equal(fixture.db.prepare("SELECT sensitivity FROM memories WHERE id = 'dependent'").get()?.sensitivity, 'secret',
      'the trigger-raised secret classification survives the later private record');
    // The receipts of the trigger-raised row follow the live row, not the cache the file left behind.
    assert.deepEqual(fixture.db.prepare("SELECT payload_json, classification_state FROM migration_records WHERE destination_memory_id = 'dependent' ORDER BY id").all()
      .map((r) => [r.payload_json, r.classification_state]), [[null, 'secret'], [null, 'secret']]);
  });
});


// Codex security review 2026-09-11 (g3, medium): a dependency edge to a secret memory must not leave the
// dependent (or the edge's own record) holding plaintext; the local trigger keeps that invariant, so the
// validator rejects files that break it instead of importing a state the database could never reach.
const dependencyDefaults = {
  kind: 'source', raw_event_id: null, source_context_id: null, citation_kind: null, citation_value: null,
  source_agent: null, portion_start: null, portion_end: null, source_total: null, source_hash: null,
  evidence: null, captured_at: 1, source_processed_at: 1, capture_root: null, source_paths_json: null, context_only: 1,
};

test('a dependency source cannot carry evidence text of its own', async () => {
  await withFixture(async (fixture) => {
    const records = [ordinaryRecord(fixture, 'a', 'A', 'eligible'), ordinaryRecord(fixture, 'b', 'B', 'eligible'),
      { ...dependencyDefaults, id: 's', memory_id: 'a', source_memory_id: 'b', evidence: 'derived text' }];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, [{ line: 5, reason: 'dependency_source_has_text' }]);
    // The local dependency row is identifiers only, so metadata has no shape there either.
    const metadata = await importMemories(fixture.db, file(fixture, [...records.slice(0, 2),
      { ...dependencyDefaults, id: 's', memory_id: 'a', source_memory_id: 'b', source_hash: 'a'.repeat(64) }]), { now: 10 });
    assert.deepEqual(metadata.rejected, [{ line: 5, reason: 'dependency_source_has_text' }]);
  });
});

test('a memory cannot carry a lower sensitivity than the memory it depends on', async () => {
  await withFixture(async (fixture) => {
    for (const [parent, child] of [['secret', 'eligible'], ['private', 'local_only'], ['local_only', 'eligible']]) {
      const records = [ordinaryRecord(fixture, 'a', 'A', child),
        { ...ordinaryRecord(fixture, 'b', 'B', parent), ...(parent === 'secret' ? { title: '', body: '' } : {}) },
        { ...dependencyDefaults, id: 's', memory_id: 'a', source_memory_id: 'b' }];
      const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
      assert.deepEqual(result.rejected, [{ line: 3, reason: 'dependency_sensitivity_below_parent' }], `${parent} > ${child}`);
    }
    assert.equal(fixture.db.prepare("SELECT COUNT(*) AS n FROM memories WHERE title = 'A'").get()?.n, 0);
  });
});

// The file can be internally consistent while the parent's local row is already stricter: the local
// provenance rule (descendant is at least as strict as its ancestor) must hold after the merge too.
test('a dependency on a locally secret memory raises the imported child in preview and apply alike', async () => {
  await withFixture(async (fixture) => {
    insertMemory(fixture, { id: 'base', title: 'Base', body: 'Base material.' });
    fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ?, sensitivity = 'secret' WHERE id = 'base'")
      .run(materialOf('Base-B'), contentOf(fixture, 'Base-B'));
    const records = [ordinaryRecord(fixture, 'a', 'Dependent-A', 'eligible'), ordinaryRecord(fixture, 'b', 'Base-B', 'eligible'),
      { ...dependencyDefaults, id: 's', memory_id: 'a', source_memory_id: 'b' }];
    const preview = await importMemories(fixture.db, file(fixture, records), { now: 10, dryRun: true });
    const applied = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(preview.rejected, []);
    assert.deepEqual({ ...applied, applied: false }, preview, 'preview and apply agree');
    assert.equal(fixture.db.prepare("SELECT sensitivity FROM memories WHERE title = 'Dependent-A'").get()?.sensitivity, 'secret');
  });
});

// Codex security re-review 2026-09-11 (fix 1 follow-up, medium): a hash-verified personal record must still
// resolve an existing tombstone that lost its personal marker when sanitation moved the receipt.
test('a verified personal record resolves a marker-less local tombstone instead of colliding', async () => {
  await withFixture(async (fixture) => {
    const hash = sha256Json(['personal-projection-v1', 'Personal preference', 'Reply in Japanese.']);
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
      sensitivity, review_state, created_at, deleted_at) VALUES ('old', ?, 'decision', '', '', '', ?, ?, 'local_only', 'imported', 1, 3)`)
      .run(fixture.identity.id, materialHash('Personal preference', 'Reply in Japanese.'), hash);
    const record = { ...redactedPersonalTombstone(fixture, hash), title: 'Personal preference', body: 'Reply in Japanese.',
      material_hash: materialHash('Personal preference', 'Reply in Japanese.'), deleted_at: null };
    const result = await importMemories(fixture.db, file(fixture, [record]), { now: 10 });
    assert.deepEqual(result.rejected, []);
    assert.deepEqual([result.inserted, result.unchanged], [0, 1]);
    assert.equal(fixture.db.prepare("SELECT effect FROM migration_records WHERE record_kind = 'memory'").get()?.effect, 'held_by_tombstone');
  });
});

// /code-review 2026-09-11: the parent's cached sensitivity can be stale when the local trigger raised it
// through a local edge during the same import; the raise must read the live parent in apply.
test('a parent raised by the local trigger during the import still raises its imported child', async () => {
  await withFixture(async (fixture) => {
    insertMemory(fixture, { id: 'p', title: 'P', body: 'P text.' });
    insertMemory(fixture, { id: 'q', title: 'Q', body: 'Q text.' });
    fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ?, sensitivity = 'local_only' WHERE id = 'p'")
      .run(materialOf('P-text'), contentOf(fixture, 'P-text'));
    fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ?, sensitivity = 'local_only' WHERE id = 'q'")
      .run(materialOf('Q-text'), contentOf(fixture, 'Q-text'));
    fixture.db.prepare("INSERT INTO memory_sources (memory_id, source_memory_id, context_only) VALUES ('p', 'q', 1)").run();
    const records = [ordinaryRecord(fixture, 'p', 'P-text', 'eligible'),
      { ...ordinaryRecord(fixture, 'q', 'Q-text', 'secret'), title: '', body: '' },
      ordinaryRecord(fixture, 'c', 'C-text', 'eligible'),
      { ...dependencyDefaults, id: 's', memory_id: 'c', source_memory_id: 'p' }];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, []);
    assert.deepEqual(fixture.db.prepare("SELECT id, sensitivity FROM memories WHERE id IN ('p', 'q') ORDER BY id").all().map((r) => r.sensitivity),
      ['secret', 'secret']);
    assert.equal(fixture.db.prepare("SELECT sensitivity FROM memories WHERE title = 'C-text'").get()?.sensitivity, 'secret');
  });
});

// Codex security review round 2 (2026-09-11): propagation must run over destination identities, not
// source origins. Two origins with the same content are one local memory.
test('a raise reaches a child whose parent is the same memory under another origin id', async () => {
  await withFixture(async (fixture) => {
    insertMemory(fixture, { id: 'p', title: 'P', body: 'P text.' });
    fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ?, sensitivity = 'secret' WHERE id = 'p'")
      .run(materialOf('P-text'), contentOf(fixture, 'P-text'));
    const records = [
      ordinaryRecord(fixture, 'b', 'B-text', 'eligible'),
      ordinaryRecord(fixture, 'a2', 'A-text', 'eligible'),
      { ...dependencyDefaults, id: 's1', memory_id: 'b', source_memory_id: 'a2' },
      ordinaryRecord(fixture, 'a1', 'A-text', 'eligible'),
      ordinaryRecord(fixture, 'p', 'P-text', 'eligible'),
      { ...dependencyDefaults, id: 's2', memory_id: 'a1', source_memory_id: 'p' },
    ];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, []);
    assert.deepEqual(fixture.db.prepare("SELECT title, sensitivity FROM memories WHERE title IN ('A-text', 'B-text') ORDER BY title").all()
      .map((r) => r.sensitivity), ['secret', 'secret']);
  });
});

test('a parent held against a terminal personal row in another repository still redacts and raises', async () => {
  await withFixture(async (fixture) => {
    const hash = sha256Json(['personal-projection-v1', 'Personal preference', 'Reply in Japanese.']);
    fixture.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('other-repo', 'common_dir', '/other', '/other', 1, 1)`).run();
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
      sensitivity, review_state, created_at) VALUES ('foreign-projection', 'other-repo', 'decision', '', '', '', ?, ?,
      'secret', 'imported', 1)`).run(materialHash('Personal preference', 'Reply in Japanese.'), hash);
    const parent = { ...redactedPersonalTombstone(fixture, hash), id: 'pp', title: 'Personal preference', body: 'Reply in Japanese.',
      material_hash: materialHash('Personal preference', 'Reply in Japanese.'), deleted_at: null };
    const records = [parent, ordinaryRecord(fixture, 'c', 'C-text', 'local_only'),
      { ...dependencyDefaults, id: 's', memory_id: 'c', source_memory_id: 'pp' }];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, []);
    const held = fixture.db.prepare("SELECT effect, payload_json, classification_state FROM migration_records WHERE record_kind = 'memory' AND identity_domain = 'personal_projection'").get()!;
    assert.deepEqual([held.effect, held.payload_json, held.classification_state], ['historical_held', null, 'secret']);
    assert.equal(fixture.db.prepare("SELECT sensitivity FROM memories WHERE title = 'C-text'").get()?.sensitivity, 'secret');
  });
});

// Codex security review round 3 (2026-09-11): the worklist must see raises the local trigger performs
// while it runs, unverified records must not depend on file order, and receipts must reflect the
// final matched state of their identity.
test('a parent raised by the local trigger after it was visited still raises its imported child', async () => {
  await withFixture(async (fixture) => {
    // The worklist pops the smallest hash first: make p's identity sort before x's so p is visited
    // before x raises q (and, through the local edge, p).
    const xText = Array.from({ length: 64 }, (_, i) => `X-text-${i}`).find((text) => contentOf(fixture, text) > contentOf(fixture, 'P-text'))!;
    for (const [id, text] of [['p', 'P-text'], ['q', 'Q-text'], ['x', xText]]) {
      insertMemory(fixture, { id, title: text, body: text });
      fixture.db.prepare("UPDATE memories SET material_hash = ?, content_hash = ?, sensitivity = ? WHERE id = ?")
        .run(materialOf(text), contentOf(fixture, text), id === 'x' ? 'secret' : 'local_only', id);
    }
    fixture.db.prepare("INSERT INTO memory_sources (memory_id, source_memory_id, context_only) VALUES ('p', 'q', 1)").run();
    const records = [ordinaryRecord(fixture, 'c', 'C-text', 'local_only'), ordinaryRecord(fixture, 'p', 'P-text', 'local_only'),
      ordinaryRecord(fixture, 'q', 'Q-text', 'local_only'), ordinaryRecord(fixture, 'x', xText, 'local_only'),
      { ...dependencyDefaults, id: 's1', memory_id: 'c', source_memory_id: 'p' },
      { ...dependencyDefaults, id: 's2', memory_id: 'q', source_memory_id: 'x' }];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, []);
    assert.deepEqual(fixture.db.prepare("SELECT title, sensitivity FROM memories WHERE title IN ('C-text', 'P-text', 'Q-text') ORDER BY title").all()
      .map((r) => r.sensitivity), ['secret', 'secret', 'secret']);
  });
});

test('an unverified personal record is resolved after the verified alias that proves its identity, whatever the file order', async () => {
  await withFixture(async (fixture) => {
    const hash = sha256Json(['personal-projection-v1', 'Personal preference', 'Reply in Japanese.']);
    fixture.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('other-repo', 'common_dir', '/other', '/other', 1, 1)`).run();
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
      sensitivity, review_state, created_at) VALUES ('H', 'other-repo', 'decision', '', '', '', ?, ?, 'secret', 'imported', 1)`)
      .run(materialHash('Personal preference', 'Reply in Japanese.'), hash);
    const verified = { ...redactedPersonalTombstone(fixture, hash), id: 'p2', title: 'Personal preference', body: 'Reply in Japanese.',
      material_hash: materialHash('Personal preference', 'Reply in Japanese.'), deleted_at: null };
    const records = [{ ...redactedPersonalTombstone(fixture, hash), id: 'p1' }, verified,
      ordinaryRecord(fixture, 'c', 'C-text', 'local_only'), { ...dependencyDefaults, id: 's', memory_id: 'c', source_memory_id: 'p1' }];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, []);
    assert.equal(fixture.db.prepare("SELECT sensitivity FROM memories WHERE title = 'C-text'").get()?.sensitivity, 'secret');
    assert.equal(fixture.db.prepare("SELECT deleted_at FROM memories WHERE id = 'H'").get()?.deleted_at, 5, 'the proven tombstone applies');
  });
});

test('a receipt reflects the final state of its identity, including a later alias that made it terminal', async () => {
  await withFixture(async (fixture) => {
    const hash = sha256Json(['personal-projection-v1', 'Personal preference', 'Reply in Japanese.']);
    const material = materialHash('Personal preference', 'Reply in Japanese.');
    fixture.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('other-repo', 'common_dir', '/other', '/other', 1, 1)`).run();
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
      sensitivity, review_state, created_at) VALUES ('P', 'other-repo', 'decision', 'Personal preference', 'Reply in Japanese.', '',
      ?, ?, 'local_only', 'reviewed', 1)`).run(material, hash);
    const verified = { ...redactedPersonalTombstone(fixture, hash), id: 'p1', title: 'Personal preference', body: 'Reply in Japanese.',
      material_hash: material, deleted_at: null };
    const records = [verified, { ...redactedPersonalTombstone(fixture, hash), id: 'p2', deleted_at: null, sensitivity: 'secret' }];
    const result = await importMemories(fixture.db, file(fixture, records), { now: 10 });
    assert.deepEqual(result.rejected, []);
    const receipts = fixture.db.prepare("SELECT payload_json, classification_state FROM migration_records WHERE record_kind = 'memory' ORDER BY id").all();
    assert.deepEqual(receipts.map((r) => [r.payload_json, r.classification_state]), [[null, 'secret'], [null, 'secret']]);
    // A deleted and secret target classifies as secret, not merely not_applicable.
    fixture.db.prepare("UPDATE memories SET deleted_at = 3 WHERE id = 'P'").run();
    const again = await importMemories(fixture.db, file(fixture, [{ ...verified, id: 'p3' }].map((r) => ({ ...r, id: 'p3' }))), { now: 11 });
    assert.deepEqual(again.rejected, []);
    assert.equal(fixture.db.prepare("SELECT classification_state FROM migration_records WHERE origin_json LIKE '%p3%' OR id NOT IN (SELECT id FROM migration_records WHERE first_import_id = (SELECT id FROM migration_imports ORDER BY imported_at LIMIT 1))").get()?.classification_state, 'secret');
  });
});

// Codex security review round 4 (2026-09-11): a held receipt has no destination, so a later deletion or
// secret classification of the matched personal identity (any repository, any import or local command)
// must still clear it; the clearing trigger matches held receipts by identity.
test('a receipt whose identity lives in another repository keeps no payload, nor do receipts nested under it', async () => {
  await withFixture(async (fixture) => {
    const hash = sha256Json(['personal-projection-v1', 'Personal preference', 'Reply in Japanese.']);
    const material = materialHash('Personal preference', 'Reply in Japanese.');
    fixture.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('other-repo', 'common_dir', '/other', '/other', 1, 1)`).run();
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
      sensitivity, review_state, created_at) VALUES ('P', 'other-repo', 'decision', 'Personal preference', 'Reply in Japanese.', '',
      ?, ?, 'local_only', 'reviewed', 1)`).run(material, hash);
    const verified = { ...redactedPersonalTombstone(fixture, hash), id: 'p1', title: 'Personal preference', body: 'Reply in Japanese.',
      material_hash: material, deleted_at: null };
    // An inherited receipt under p1 whose embedded payload names another identity: with no destination
    // there is nothing to attach it to, so it is hash-only as well.
    const embedded = { ...verified, id: 'embedded', content_hash: 'e'.repeat(64) };
    const origin = ['migration-origin-v1', NATIVE_FORMAT, NATIVE_REVISION, 'c'.repeat(32), 'memory', sha256Hex('embedded'), sha256Hex('opaque')];
    const inherited = { kind: 'migration_origin', id: sha256Json(origin), repo_id: fixture.identity.id, memory_id: 'p1',
      origin_json: JSON.stringify(origin), payload_hash: sha256Hex('opaque'), stored_payload_hash: sha256Hex(JSON.stringify(embedded)),
      record_kind: 'memory', payload: embedded, classification_state: 'pending' };
    const result = await importMemories(fixture.db, file(fixture, [verified, inherited]), { now: 10 });
    assert.deepEqual(result.rejected, []);
    const receipts = fixture.db.prepare("SELECT effect, destination_memory_id, payload_json, classification_state, detail_code FROM migration_records ORDER BY record_kind, id").all();
    assert.equal(receipts.length, 2);
    assert.deepEqual(receipts.map((r) => [r.effect, r.destination_memory_id, r.payload_json, r.classification_state, r.detail_code]),
      [['historical_held', null, null, 'not_applicable', 'identity_elsewhere'], ['historical_held', null, null, 'not_applicable', 'identity_elsewhere']]);
    assert.equal(fixture.db.prepare("SELECT title FROM memories WHERE id = 'P'").get()?.title, 'Personal preference', 'the exact text stays with its row');
  });
});

// Codex security review round 7 (2026-09-11): a retained origin that carries a payload must belong to
// a memory in the file; an orphan payload would be stored with no identity to clear it by.
test('a memory-kind migration origin with a payload but no memory is refused', async () => {
  await withFixture(async (fixture) => {
    const embedded = { ...ordinaryRecord(fixture, 'embedded', 'Orphan text', 'local_only') };
    const origin = ['migration-origin-v1', NATIVE_FORMAT, NATIVE_REVISION, 'c'.repeat(32), 'memory', sha256Hex('embedded'), sha256Hex('opaque')];
    const orphan = { kind: 'migration_origin', id: sha256Json(origin), repo_id: fixture.identity.id, memory_id: null,
      origin_json: JSON.stringify(origin), payload_hash: sha256Hex('opaque'), stored_payload_hash: sha256Hex(JSON.stringify(embedded)),
      record_kind: 'memory', payload: embedded, classification_state: 'pending' };
    const result = await importMemories(fixture.db, file(fixture, [orphan]), { now: 10 });
    assert.deepEqual(result.rejected, [{ line: 3, reason: 'orphan_origin_payload' }]);
    assert.equal(fixture.db.prepare('SELECT COUNT(*) AS n FROM migration_records').get()?.n, 0);
    // The same receipt without a payload is an acceptable hash-only trace.
    const bare = await importMemories(fixture.db, file(fixture, [{ ...orphan, payload: null, stored_payload_hash: null }]), { now: 10 });
    assert.deepEqual(bare.rejected, []);
    // A terminal label only counts in the field its own kind uses: a foreign label on a memory payload
    // (`redacted`) or on a source payload changes nothing, so those orphans are refused as well.
    for (const payload of [{ ...embedded, redacted: true }, { ...embedded, candidate_sensitivity: 'secret' },
      { ...dependencyDefaults, id: 's', memory_id: 'x', source_memory_id: null, evidence: 'Orphan evidence', redacted: true }]) {
      const tuple = [...origin.slice(0, 4), payload.kind, sha256Hex(payload.id), sha256Hex('opaque')];
      const record = { ...orphan, id: sha256Json(tuple), origin_json: JSON.stringify(tuple), payload,
        stored_payload_hash: sha256Hex(JSON.stringify(payload)), record_kind: payload.kind };
      const refused = await importMemories(fixture.db, file(fixture, [record]), { now: 10 });
      assert.deepEqual(refused.rejected, [{ line: 3, reason: 'orphan_origin_payload' }], JSON.stringify(payload));
    }
    const secret = { ...embedded, sensitivity: 'secret', title: '', body: '' };
    const labelled = await importMemories(fixture.db, file(fixture, [{ ...orphan, payload: secret, stored_payload_hash: sha256Hex(JSON.stringify(secret)) }]), { now: 10 });
    assert.deepEqual(labelled.rejected, []);
  });
});

// Codex security review round 8 (2026-09-11, pre-existing): a proposal receipt inherits the terminal state
// of its projected memory, not only of its origin memory.
test('a proposal whose projection matches a local secret row keeps no candidate text', async () => {
  let exported = '';
  await withFixture(async (source) => {
    insertSession(source, { id: 'source-session', agent: 'claude' });
    seedWorkBinding(source.db, 'source-session');
    const [title, body] = ['Approved preference', 'Reply in Japanese.'];
    const material = materialHash(title, body);
    const originId = memoryIdFor(contentHash(source.identity.id, material));
    source.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, material_hash, content_hash, sensitivity,
      review_state, provenance_complete, created_at) VALUES (?, ?, 'discovery', ?, ?, '[]', ?, ?, 'local_only', 'reviewed', 1, 1)`)
      .run(originId, source.identity.id, title, body, material, contentHash(source.identity.id, material));
    grantVisibility(source.db, originId, { audience: 'work', repoId: source.identity.id, workId: `fixture-work:${source.identity.id}` }, 'observer', 1);
    const personalHash = sha256Json(['personal-projection-v1', title, body]);
    const projectionId = memoryIdFor(personalHash);
    source.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, material_hash, content_hash, sensitivity,
      review_state, created_at) VALUES (?, ?, 'decision', ?, ?, '[]', ?, ?, 'local_only', 'reviewed', 2)`)
      .run(projectionId, source.identity.id, title, body, material, personalHash);
    source.db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title,
      candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, decision_channel,
      projected_memory_id, created_at, decided_at) VALUES ('proposal-approved', ?, ?, ?, ?, ?, ?, 'local_only', '[]', 'inferred',
      'approved', 'cli', ?, 1, 2)`).run(originId, source.identity.id, `fixture-work:${source.identity.id}`, title, body, material, projectionId);
    grantVisibility(source.db, projectionId, { audience: 'personal', proposalId: 'proposal-approved' }, 'proposal_approval', 2);
    const result = output();
    assert.equal(await runExport(['-'], result.io), 0, result.text.error);
    exported = result.text.out;
  });
  await withFixture(async (target) => {
    const personalHash = sha256Json(['personal-projection-v1', 'Approved preference', 'Reply in Japanese.']);
    target.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity,
      review_state, created_at) VALUES ('local-projection', ?, 'decision', '', '', '', ?, ?, 'secret', 'imported', 1)`)
      .run(target.identity.id, materialHash('Approved preference', 'Reply in Japanese.'), personalHash);
    const sourceRepo = (JSON.parse(exported.split('\n')[1]) as { id: string }).id;
    const result = await importMemories(target.db, exported, { now: 10, mapRepo: { [sourceRepo]: target.identity.id } });
    assert.deepEqual(result.rejected, []);
    const proposal = target.db.prepare("SELECT payload_json, classification_state FROM migration_records WHERE record_kind = 'sharing_proposal'").get()!;
    assert.deepEqual([proposal.payload_json, proposal.classification_state], [null, 'secret']);
    // The origin memory's own text is its own content; only the projection and the proposal are terminal.
    assert.equal(target.db.prepare(`SELECT COUNT(*) AS n FROM migration_records WHERE payload_json LIKE '%Reply in Japanese%'
      AND (record_kind = 'sharing_proposal' OR identity_domain = 'personal_projection')`).get()?.n, 0);
  });
});
