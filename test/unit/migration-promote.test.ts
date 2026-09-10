import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import test from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { grantVisibility } from '../../src/db/queries.js';
import { sha256Json } from '../../src/hash.js';
import { runShare } from '../../src/memories-cli.js';
import { oboetePaths } from '../../src/paths.js';
import { runExport, runImport } from '../../src/transfer.js';
import { runObserve } from '../../src/worker/observe.js';
import { insertSession, withFixture, type Fixture } from '../helpers/inject-fixture.js';
import { cleanEnv } from '../helpers/observe.js';
import { withTempHome } from '../helpers/home.js';

function output() {
  const text = { out: '', error: '' };
  return { text, io: { writeOut: (value: string) => { text.out += value; },
    writeError: (value: string) => { text.error += value; } } };
}

const WORK_TABLES = ['work_items', 'work_contexts', 'work_bindings', 'sessions', 'raw_events', 'observation_batches', 'injections'];
function snapshot(db: DatabaseSync, tables = WORK_TABLES) {
  return tables.map((table) => db.prepare(`SELECT * FROM "${table}"`).all().map((row) => JSON.stringify(row)).sort());
}

function allTables(db: DatabaseSync) {
  return db.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name").all().map((row) => String(row.name));
}

async function nativeFile(source: Fixture, approval?: 'cli' | 'automatic_direct') {
  const { db, identity } = source;
  insertSession(source, { id: 'source-session', agent: 'claude' });
  const workId = `fixture-work:${identity.id}`;
  const material = materialHash('Project preference', 'The team replies in Japanese.');
  const content = contentHash(identity.id, material);
  const id = memoryIdFor(content);
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, material_hash, content_hash,
    sensitivity, review_state, source_session_id, provenance_complete, created_at)
    VALUES (?, ?, 'discovery', 'Project preference', 'The team replies in Japanese.', ?, ?,
      'local_only', 'reviewed', 'source-session', 1, 1)`).run(id, identity.id, material, content);
  grantVisibility(db, id, { audience: 'work', repoId: identity.id, workId }, 'observer', 1);
  db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value,
    evidence, captured_at, capture_root, source_paths_json)
    VALUES (?, 'foreign-event', 'file_read', 'README.md', 'The source evidence.', 1, ?, '["README.md"]')`)
    .run(id, source.repo);
  const candidateBody = 'Reply <private>source-only</private>in Japanese.';
  db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
    candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity,
    source_event_ids_json, basis, state, created_at)
    VALUES ('foreign-proposal', ?, ?, ?, 'Personal preference', ?, ?, 'local_only',
      '["foreign-event"]', 'inferred', 'pending', 2)`)
    .run(id, identity.id, workId, candidateBody, materialHash('Personal preference', candidateBody));
  if (approval !== undefined) {
    const hash = sha256Json(['personal-projection-v1', 'Personal preference', candidateBody]);
    const projectionId = memoryIdFor(hash);
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, material_hash, content_hash,
      sensitivity, review_state, created_at) VALUES (?, ?, 'decision', 'Personal preference', ?, ?, ?,
        'local_only', 'reviewed', 3)`).run(projectionId, identity.id, candidateBody,
      materialHash('Personal preference', candidateBody), hash);
    db.prepare(`UPDATE sharing_proposals SET state = 'approved', decision_channel = ?,
      projected_memory_id = ?, decided_at = 3, basis = 'direct_declaration' WHERE id = 'foreign-proposal'`)
      .run(approval, projectionId);
    grantVisibility(db, projectionId, { audience: 'personal', proposalId: 'foreign-proposal' }, 'proposal_approval', 3);
  }
  const result = output();
  assert.equal(await runExport(['-'], result.io), 0, result.text.error);
  const path = join(source.paths.home, 'native.jsonl');
  writeFileSync(path, result.text.out);
  return { path, workId };
}

type Imported = Fixture & { recordId: string; memoryId: string; sourceWork: string; localWork: string };
async function withImported(run: (fixture: Imported) => Promise<void>, approval?: 'cli' | 'automatic_direct') {
  await withFixture(async (source) => {
    const file = await nativeFile(source, approval);
    const sourceBytes = readFileSync(file.path);
    await withFixture(async (target) => {
      insertSession(target, { id: 'target-session', agent: 'codex' });
      const before = snapshot(target.db);
      const imported = output();
      assert.equal(await runImport([file.path, '--map-repo', `${source.identity.id}=${target.identity.id}`,
        '--apply', '--json'], imported.io), 0, imported.text.error);
      assert.deepEqual(snapshot(target.db), before);
      const row = target.db.prepare("SELECT * FROM migration_records WHERE record_kind = 'sharing_proposal'").get()!;
      const cwd = process.cwd();
      process.chdir(target.repo);
      try {
        await run({ ...target, recordId: String(row.id), memoryId: String(row.destination_memory_id),
          sourceWork: file.workId, localWork: `fixture-work:${target.identity.id}` });
      } finally { process.chdir(cwd); }
    });
    assert.deepEqual(readFileSync(file.path), sourceBytes);
  });
}

async function classify(fixture: Fixture) {
  assert.equal(await runObserve([], { env: cleanEnv(fixture.paths.home), now: () => 2_000,
    heartbeatMs: 60_000, fetch: async () => assert.fail('classification must use no provider'),
    writeError: () => undefined }), 0);
}

test('native candidate promotion requires fresh human approval and preserves current work', async () => {
  await withImported(async (fixture) => {
    const { db, recordId, memoryId, sourceWork, localWork } = fixture;
    await classify(fixture);
    assert.equal(db.prepare('SELECT classification_state FROM migration_records WHERE id = ?').get(recordId)?.classification_state, 'clean');
    const before = snapshot(db);
    const promoted = output();
    assert.equal(await runImport(['promote', recordId, '--map-work', `${sourceWork}=${localWork}`, '--json'], promoted.io), 0, promoted.text.error);
    const result = JSON.parse(promoted.text.out) as { id: string; state: string };
    assert.match(result.id, /^sp_[0-9a-f]{64}$/u);
    assert.deepEqual(result, { id: result.id, state: 'pending' });
    assert.equal(promoted.text.error, '');
    const row = db.prepare('SELECT * FROM sharing_proposals').get()!;
    assert.deepEqual({ ...row }, { id: result.id, origin_memory_id: memoryId, origin_repo_id: fixture.identity.id,
      origin_work_id: localWork, candidate_title: 'Personal preference', candidate_body: 'Reply in Japanese.',
      candidate_material_hash: 'a0acb9bf014f044481de1b66cada63fedc775b364b95277d181b1c6953ed275c',
      candidate_sensitivity: 'local_only', source_event_ids_json: '[]', basis: 'inferred', state: 'pending',
      decision_channel: null, projected_memory_id: null, created_at: row.created_at, decided_at: null });
    assert.equal(typeof row.created_at, 'number');
    const grants = db.prepare('SELECT audience, repo_id, work_id, grant_kind FROM memory_visibility WHERE memory_id = ?').all(memoryId);
    assert.deepEqual(grants.map((grant) => ({ ...grant })), [{ audience: 'work', repo_id: fixture.identity.id, work_id: localWork, grant_kind: 'migration' }]);
    assert.equal(db.prepare('SELECT promoted_proposal_id FROM migration_records WHERE id = ?').get(recordId)?.promoted_proposal_id, result.id);
    const status = output();
    assert.equal(await runShare(['status', '--json'], status.io), 0, status.text.error);
    const candidates = JSON.parse(status.text.out).proposals;
    assert.equal(candidates.length, 1);
    assert.equal(candidates[0].candidate_body, 'Reply in Japanese.');
    const approved = output();
    assert.equal(await runShare(['approve', result.id, '--json'], approved.io), 0, approved.text.error);
    const decision = JSON.parse(approved.text.out);
    assert.equal(decision.state, 'approved');
    const projection = db.prepare('SELECT title, body, review_state FROM memories WHERE id = ?').get(decision.projectedMemoryId)!;
    assert.deepEqual({ ...projection }, { title: 'Personal preference', body: 'Reply in Japanese.', review_state: 'reviewed' });
    assert.equal(db.prepare('SELECT audience FROM memory_visibility WHERE memory_id = ?').get(decision.projectedMemoryId)?.audience, 'personal');
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ?').get(decision.projectedMemoryId)?.n, 0);
    assert.deepEqual(snapshot(db), before);
  });
});

test('repeated promotion reuses the proposal and historical work grant without writes', async () => {
  await withImported(async (fixture) => {
    await classify(fixture);
    const args = ['promote', fixture.recordId, '--map-work', `${fixture.sourceWork}=${fixture.localWork}`, '--json'];
    const first = output();
    assert.equal(await runImport(args, first.io), 0, first.text.error);
    const before = snapshot(fixture.db, ['sharing_proposals', 'memory_visibility', 'migration_records']);
    const repeated = output();
    assert.equal(await runImport(args, repeated.io), 0, repeated.text.error);
    assert.deepEqual(JSON.parse(repeated.text.out), JSON.parse(first.text.out));
    assert.deepEqual(snapshot(fixture.db, ['sharing_proposals', 'memory_visibility', 'migration_records']), before);
    assert.equal(fixture.db.prepare('SELECT COUNT(*) AS n FROM sharing_proposals').get()?.n, 1);
    assert.equal(fixture.db.prepare('SELECT COUNT(*) AS n FROM memory_visibility').get()?.n, 1);
  });
});

test('promotion preserves a local rejection and never creates another pending proposal', async () => {
  await withImported(async (fixture) => {
    await classify(fixture);
    const args = ['promote', fixture.recordId, '--map-work', `${fixture.sourceWork}=${fixture.localWork}`, '--json'];
    const first = output();
    assert.equal(await runImport(args, first.io), 0, first.text.error);
    const id = JSON.parse(first.text.out).id;
    const rejected = output();
    assert.equal(await runShare(['reject', id, '--json'], rejected.io), 0, rejected.text.error);
    const before = snapshot(fixture.db, ['sharing_proposals', 'memory_visibility', 'migration_records']);
    const repeated = output();
    assert.equal(await runImport(args, repeated.io), 0, repeated.text.error);
    assert.deepEqual(JSON.parse(repeated.text.out), { id, state: 'rejected' });
    assert.deepEqual(snapshot(fixture.db, ['sharing_proposals', 'memory_visibility', 'migration_records']), before);
    assert.equal(fixture.db.prepare("SELECT COUNT(*) AS n FROM sharing_proposals WHERE state = 'pending'").get()?.n, 0);
  });
});

for (const approval of ['cli', 'automatic_direct'] as const) {
  test(`source ${approval} approval and its projection grant cannot approve a local promotion`, async () => {
    await withImported(async (fixture) => {
      const { db, recordId, sourceWork, localWork } = fixture;
      await classify(fixture);
      const held = JSON.parse(String(db.prepare('SELECT payload_json FROM migration_records WHERE id = ?').get(recordId)?.payload_json));
      assert.equal(held.state, 'approved');
      assert.equal(held.decision_channel, approval);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories WHERE deleted_at IS NULL').get()?.n, 2);
      const promoted = output();
      assert.equal(await runImport(['promote', recordId, '--map-work', `${sourceWork}=${localWork}`, '--json'], promoted.io), 0, promoted.text.error);
      assert.equal(JSON.parse(promoted.text.out).state, 'pending');
      const row = db.prepare('SELECT source_event_ids_json, basis, state, decision_channel, projected_memory_id, decided_at FROM sharing_proposals').get()!;
      assert.deepEqual({ ...row }, { source_event_ids_json: '[]', basis: 'inferred', state: 'pending',
        decision_channel: null, projected_memory_id: null, decided_at: null });
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memory_visibility WHERE audience = 'personal'").get()?.n, 0);
    }, approval);
  });
}

function changeRecord(fixture: Imported, assignment: string) {
  fixture.db.prepare(`UPDATE migration_records SET ${assignment} WHERE id = ?`).run(fixture.recordId);
}

function changePayload(fixture: Imported, field: string, value: string) {
  fixture.db.prepare('UPDATE migration_records SET payload_json = json_set(payload_json, ?, ?) WHERE id = ?')
    .run(`$.${field}`, value, fixture.recordId);
}

type Selection = { id: string; source: string; work: string };
const unavailableCases: [string, (fixture: Imported, selection: Selection) => void][] = [
  ['unclassified', () => undefined],
  ['missing', (_fixture, selection) => { selection.id = 'a'.repeat(64); }],
  ['malformed-id', (_fixture, selection) => { selection.id = '../PRIVATE-ID\n'; }],
  ['wrong-repository', (fixture) => { process.chdir(fixture.paths.home); }],
  ['unknown-work', (_fixture, selection) => { selection.work = 'unknown-work'; }],
  ['wrong-work-repository', (fixture, selection) => {
    fixture.db.exec(`INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('foreign-repo', 'common_dir', '/foreign');
      INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at)
        VALUES ('foreign-context', 'foreign-repo', 'foreign', '/foreign', 1, 1);
      INSERT INTO work_items (id, repo_id, origin_context_id, created_at, updated_at)
        VALUES ('foreign-work', 'foreign-repo', 'foreign-context', 1, 1)`);
    selection.work = 'foreign-work';
  }],
  ['mismatched-source', (_fixture, selection) => { selection.source += '-other'; }],
  ['secret-record', (fixture) => changeRecord(fixture, "classification_state = 'secret'")],
  ['not-applicable', (fixture) => changeRecord(fixture, "classification_state = 'not_applicable'")],
  ['null-payload', (fixture) => changeRecord(fixture, 'payload_json = NULL')],
  ['malformed-payload', (fixture) => changeRecord(fixture, "payload_json = '{}'")],
  ['wrong-payload-kind', (fixture) => changePayload(fixture, 'kind', 'work')],
  ['wrong-record-kind', (fixture) => changeRecord(fixture, "record_kind = 'memory'")],
  ['null-memory', (fixture) => changeRecord(fixture, 'destination_memory_id = NULL')],
  ['imported-origin', (fixture) => { fixture.db.prepare("UPDATE memories SET review_state = 'imported' WHERE id = ?").run(fixture.memoryId); }],
  ['deleted-origin', (fixture) => { fixture.db.prepare('UPDATE memories SET deleted_at = 4 WHERE id = ?').run(fixture.memoryId); }],
  ['expired-origin', (fixture) => { fixture.db.prepare('UPDATE memories SET valid_to = 4 WHERE id = ?').run(fixture.memoryId); }],
  ['summary-origin', (fixture) => { fixture.db.prepare("UPDATE memories SET type = 'session_summary' WHERE id = ?").run(fixture.memoryId); }],
  ['secret-origin', (fixture) => { fixture.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = ?").run(fixture.memoryId); }],
  ['personal-domain', (fixture) => changeRecord(fixture, "identity_domain = 'personal_projection'")],
  ['stale-context', (fixture) => { fixture.db.exec("UPDATE work_contexts SET local_key = 'replaced-generation'"); }],
  ['overlong-title', (fixture) => changePayload(fixture, 'candidate_title', 'x'.repeat(121))],
  ['overlong-body', (fixture) => changePayload(fixture, 'candidate_body', 'x'.repeat(2001))],
  ['blank-body', (fixture) => changePayload(fixture, 'candidate_body', ' \n\t')],
];

for (const [unavailable, arrange] of unavailableCases) {
  test(`unavailable promotion: ${unavailable} returns the fixed result without any table writes`, async () => {
    await withImported(async (fixture) => {
      const { db, recordId } = fixture;
      if (unavailable !== 'unclassified') await classify(fixture);
      const selection = { id: recordId, source: fixture.sourceWork, work: fixture.localWork };
      arrange(fixture, selection);
      const tables = allTables(db);
      const before = snapshot(db, tables);
      const result = output();
      assert.equal(await runImport(['promote', selection.id, '--map-work', `${selection.source}=${selection.work}`, '--json'], result.io), 1);
      assert.deepEqual(result.text, { out: '', error: 'The migration record is unavailable in this scope.\n' });
      assert.deepEqual(snapshot(db, tables), before);
    });
  });
}

for (const invalid of ['no-id', 'no-map', 'duplicate-map', 'missing-equals', 'empty-source', 'empty-target',
  'overlong-source', 'unknown-option', 'extra-positional'] as const) {
  test(`invalid promotion arguments: ${invalid} exits 2 without writes`, async () => {
    await withImported(async (fixture) => {
      const mapping = `${fixture.sourceWork}=${fixture.localWork}`;
      const args = ['promote', ...(invalid === 'no-id' ? [] : [fixture.recordId])];
      const malformed: Partial<Record<typeof invalid, string>> = {
        'missing-equals': 'malformed', 'empty-source': '=work', 'empty-target': 'source=', 'overlong-source': `${'x'.repeat(513)}=work`,
      };
      if (invalid !== 'no-map') args.push('--map-work', malformed[invalid] ?? mapping);
      if (invalid === 'duplicate-map') args.push('--map-work', mapping);
      if (invalid === 'unknown-option') args.push('--unknown');
      if (invalid === 'extra-positional') args.push('extra');
      const tables = allTables(fixture.db);
      const before = snapshot(fixture.db, tables);
      const result = output();
      assert.equal(await runImport(args, result.io), 2);
      assert.equal(result.text.out, '');
      assert.match(result.text.error, /^Usage: oboete import promote /u);
      assert.deepEqual(snapshot(fixture.db, tables), before);
    });
  });
}

test('promotion maps an opaque source work containing equals exactly', async () => {
  await withImported(async (fixture) => {
    const source = `${fixture.sourceWork}=archived`;
    fixture.db.prepare("UPDATE migration_records SET payload_json = json_set(payload_json, '$.origin_work_id', ?) WHERE id = ?")
      .run(source, fixture.recordId);
    await classify(fixture);
    const result = output();
    assert.equal(await runImport(['promote', fixture.recordId, '--map-work', `${source}=${fixture.localWork}`, '--json'], result.io), 0, result.text.error);
    assert.equal(JSON.parse(result.text.out).state, 'pending');
  });
});

test('promotion identity uses the exact candidate strings and local origin and work', async () => {
  await withImported(async (fixture) => {
    const { db, recordId } = fixture;
    await classify(fixture);
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, material_hash, content_hash,
      sensitivity, review_state) VALUES ('vector-origin', ?, 'discovery', 'Known origin', 'Already local.',
        'vector-material', 'vector-content', 'local_only', 'reviewed')`).run(fixture.identity.id);
    db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, created_at, updated_at)
      SELECT 'vector-work', repo_id, origin_context_id, 1, 1 FROM work_items WHERE id = ?`).run(fixture.localWork);
    db.prepare("UPDATE migration_records SET destination_memory_id = 'vector-origin' WHERE id = ?").run(recordId);
    const args = ['promote', recordId, '--map-work', `${fixture.sourceWork}=vector-work`, '--json'];
    const first = output();
    assert.equal(await runImport(args, first.io), 0, first.text.error);
    assert.deepEqual(JSON.parse(first.text.out), {
      id: 'sp_c71b994fcae850e9d0e64cfd0cece305d4a247778602dcebfa2a372c902bed1f', state: 'pending',
    });
    db.prepare("UPDATE migration_records SET payload_json = json_set(payload_json, '$.candidate_body', 'reply in Japanese.') WHERE id = ?").run(recordId);
    const exact = output();
    assert.equal(await runImport(args, exact.io), 0, exact.text.error);
    assert.deepEqual(JSON.parse(exact.text.out), {
      id: 'sp_9a60cc0ec85e25e3d57baab8ae4c88ec6e017148055cb1c467c8e5a86681b0fe', state: 'pending',
    });
    assert.equal(db.prepare('SELECT COUNT(DISTINCT candidate_material_hash) AS n FROM sharing_proposals').get()?.n, 1);
  });
});

test('an approved promotion remains approved and human output reveals only its id and state', async () => {
  await withImported(async (fixture) => {
    await classify(fixture);
    const args = ['promote', fixture.recordId, '--map-work', `${fixture.sourceWork}=${fixture.localWork}`];
    const first = output();
    assert.equal(await runImport([...args, '--json'], first.io), 0, first.text.error);
    const id = JSON.parse(first.text.out).id;
    const approved = output();
    assert.equal(await runShare(['approve', id], approved.io), 0, approved.text.error);
    const before = snapshot(fixture.db, allTables(fixture.db));
    const repeated = output();
    assert.equal(await runImport(args, repeated.io), 0, repeated.text.error);
    assert.deepEqual(repeated.text, { error: '',
      out: `Sharing proposal ${id} is approved. Use oboete share status to review the candidate.\n` });
    assert.deepEqual(snapshot(fixture.db, allTables(fixture.db)), before);
  });
});

for (const stricter of ['origin', 'candidate'] as const) {
  test(`promotion retains the stricter ${stricter} sensitivity`, async () => {
    await withImported(async (fixture) => {
      await classify(fixture);
      if (stricter === 'origin') fixture.db.prepare("UPDATE memories SET sensitivity = 'private' WHERE id = ?").run(fixture.memoryId);
      else fixture.db.prepare("UPDATE migration_records SET payload_json = json_set(payload_json, '$.candidate_sensitivity', 'private') WHERE id = ?").run(fixture.recordId);
      const result = output();
      assert.equal(await runImport(['promote', fixture.recordId, '--map-work', `${fixture.sourceWork}=${fixture.localWork}`, '--json'], result.io), 0, result.text.error);
      assert.equal(fixture.db.prepare('SELECT candidate_sensitivity FROM sharing_proposals').get()?.candidate_sensitivity, 'private');
    });
  });
}

test('a failure saving the promotion receipt rolls back both the proposal and work grant', async () => {
  await withImported(async (fixture) => {
    await classify(fixture);
    fixture.db.exec(`CREATE TRIGGER reject_promotion BEFORE UPDATE OF promoted_proposal_id ON migration_records
      BEGIN SELECT RAISE(ABORT, 'fixture failure'); END`);
    const before = snapshot(fixture.db, allTables(fixture.db));
    const result = output();
    assert.equal(await runImport(['promote', fixture.recordId, '--map-work', `${fixture.sourceWork}=${fixture.localWork}`, '--json'], result.io), 1);
    assert.deepEqual(result.text, { out: '', error: 'The migration record is unavailable in this scope.\n' });
    assert.deepEqual(snapshot(fixture.db, allTables(fixture.db)), before);
  });
});

test('a personal projection remains ineligible as an origin after its visibility grant is removed', async () => {
  await withImported(async (fixture) => {
    await classify(fixture);
    const projection = fixture.db.prepare("SELECT destination_memory_id FROM migration_records WHERE identity_domain = 'personal_projection'").get()!;
    fixture.db.prepare('UPDATE migration_records SET destination_memory_id = ? WHERE id = ?').run(projection.destination_memory_id, fixture.recordId);
    fixture.db.prepare('DELETE FROM memory_visibility WHERE memory_id = ?').run(projection.destination_memory_id);
    const before = snapshot(fixture.db, allTables(fixture.db));
    const result = output();
    assert.equal(await runImport(['promote', fixture.recordId, '--map-work', `${fixture.sourceWork}=${fixture.localWork}`, '--json'], result.io), 1);
    assert.deepEqual(result.text, { out: '', error: 'The migration record is unavailable in this scope.\n' });
    assert.deepEqual(snapshot(fixture.db, allTables(fixture.db)), before);
  }, 'cli');
});

test('an unavailable promotion never creates a missing destination database', async () => {
  await withTempHome(async (home) => {
    const result = output();
    assert.equal(await runImport(['promote', 'a'.repeat(64), '--map-work', 'source=local', '--json'], result.io), 1);
    assert.deepEqual(result.text, { out: '', error: 'The migration record is unavailable in this scope.\n' });
    assert.equal(existsSync(oboetePaths(home).db), false);
  });
});

test('promotion leaves the database write lock free while verifying Git context', async () => {
  await withImported(async (fixture) => {
    await classify(fixture);
    fixture.db.exec('PRAGMA busy_timeout = 0');
    const writeAvailability: boolean[] = [];
    const original = childProcess.spawnSync;
    childProcess.spawnSync = ((...args: Parameters<typeof original>) => {
      if (args[0] === 'git') {
        try {
          fixture.db.exec('BEGIN IMMEDIATE');
          fixture.db.exec('ROLLBACK');
          writeAvailability.push(true);
        } catch { writeAvailability.push(false); }
      }
      return Reflect.apply(original, childProcess, args);
    }) as typeof original;
    syncBuiltinESMExports();
    try {
      const result = output();
      assert.equal(await runImport(['promote', fixture.recordId, '--map-work', `${fixture.sourceWork}=${fixture.localWork}`, '--json'], result.io), 0, result.text.error);
      assert.ok(writeAvailability.length > 0, 'the fixture must exercise Git verification');
      assert.ok(writeAvailability.every(Boolean), 'Git verification must not hold the database write lock');
    } finally {
      childProcess.spawnSync = original;
      syncBuiltinESMExports();
    }
  });
});
