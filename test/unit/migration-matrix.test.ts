import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { MIGRATIONS, openDatabase } from '../../src/db/open.js';
import { grantVisibility } from '../../src/db/queries.js';
import { sha256Hex, sha256Json } from '../../src/hash.js';
import { oboetePaths } from '../../src/paths.js';
import { detectSync } from '../../src/privacy/detect.js';
import { verifiedRepoContext } from '../../src/privacy/source-context.js';
import { resolveRepoIdentity } from '../../src/repo-identity.js';
import { decideSharing } from '../../src/sharing.js';
import { NATIVE_FORMAT, NATIVE_REVISION, type NativeMemory, type NativeRecord } from '../../src/transfer-format.js';
import { runExport, runImport } from '../../src/transfer.js';
import { runObserve } from '../../src/worker/observe.js';
import { withTempHome } from '../helpers/home.js';
import { insertSession, withFixture, type Fixture } from '../helpers/inject-fixture.js';
import { cleanEnv } from '../helpers/observe.js';
import { output } from '../helpers/output.js';

function nativeFile(home: string, records: NativeRecord[], name = 'native.jsonl', time = 1) {
  const path = join(home, name);
  writeFileSync(path, [{ format: NATIVE_FORMAT, revision: NATIVE_REVISION,
    origin_id: 'a'.repeat(32), exported_at: time }, ...records].map((row) => JSON.stringify(row)).join('\n') + '\n');
  return path;
}

function repo(id = 'source-repo'): NativeRecord {
  return { kind: 'repo', id, identity_kind: 'common_dir', normalized_identity: `/foreign/${id}/.git` };
}

function memory(id = 'source-memory', repoId = 'source-repo'): NativeMemory {
  const title = `Fact ${id}`, body = 'The migration matrix preserves this fact.';
  const material = materialHash(title, body);
  return { kind: 'memory', id, repo_id: repoId, type: 'discovery', title, body, concepts: '[]',
    material_hash: material, content_hash: contentHash(repoId, material), sensitivity: 'local_only',
    review_state: 'reviewed', degraded_reason: null, source_session_id: null, source_batch_id: null,
    source_agent: null, valid_from: null, valid_to: null, superseded_by: null, pinned_at: null,
    pin_order: null, deleted_at: null, created_at: 1, identity_domain: 'ordinary', work_id: null,
    checkpoint_parent_id: null, provenance_complete: 1, source_captured_at: 1 };
}

function snapshot(db: DatabaseSync) {
  return db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").all().map(({ name }) => ({
    name, rows: db.prepare(`SELECT * FROM "${String(name).replaceAll('"', '""')}"`).all()
      .map((row) => JSON.stringify(row)).sort(),
  }));
}

async function exportedFile(fixture: Fixture, name = 'export.jsonl') {
  const result = output();
  assert.equal(await runExport(['-'], result.io), 0, result.text.error || result.text.out);
  const path = join(fixture.paths.home, name);
  writeFileSync(path, result.text.out);
  const records = result.text.out.trim().split('\n').slice(1).map((line) => JSON.parse(line) as NativeRecord);
  return { path, records };
}

async function classify(fixture: Fixture) {
  assert.equal(await runObserve([], { env: cleanEnv(fixture.paths.home), now: () => 2_000,
    heartbeatMs: 60_000, fetch: async () => assert.fail('migration classification must use no provider'),
    detect: detectSync, writeError: () => undefined }), 0);
}

test('matrix A1: preview requires an explicit verified context when zero or two candidates exist', async (t) => {
  await withTempHome(async (home) => {
    // Conventions "Tests": real Git in temporary homes; FDs also work on hosts denying subprocess pipes.
    const spawn = childProcess.spawnSync;
    t.mock.method(childProcess, 'spawnSync', ((file: string, args: string[], options: childProcess.SpawnSyncOptionsWithStringEncoding) => {
      const out = join(home, 'git.stdout'), error = join(home, 'git.stderr');
      const outFd = openSync(out, 'w'), errorFd = openSync(error, 'w');
      try {
        const result = spawn(file, args, { ...options, stdio: ['ignore', outFd, errorFd] });
        const stdout = readFileSync(out, 'utf8'), stderr = readFileSync(error, 'utf8');
        return { ...result, stdout, stderr, output: [null, stdout, stderr] };
      } finally { closeSync(outFd); closeSync(errorFd); }
    }) as typeof spawn);
    syncBuiltinESMExports();
    const db = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 }).db;
    try {
      const roots = ['first', 'newest'].map((name) => join(home, name));
      for (const root of roots) {
        mkdirSync(root);
        assert.equal(spawn('git', ['-C', root, 'init', '--quiet'], { stdio: 'ignore' }).status, 0);
        assert.equal(spawn('git', ['-C', root, 'remote', 'add', 'origin', 'https://example.test/matrix.git'], { stdio: 'ignore' }).status, 0);
      }
      const identities = roots.map((root) => resolveRepoIdentity(root));
      assert.equal(identities[0].identityKind, 'remote');
      assert.equal(identities[1].id, identities[0].id);
      assert.notEqual(identities[0].worktreeKey, identities[1].worktreeKey);
      const repoId = identities[0].id;
      db.prepare('INSERT INTO repos (id, identity_kind, normalized_identity) VALUES (?, ?, ?)')
        .run(repoId, 'remote', identities[0].normalizedIdentity);
      const file = nativeFile(home, [repo(), memory()]);
      const args = [file, '--dry-run', '--map-repo', `source-repo=${repoId}`];
      for (const count of [0, 2]) {
        if (count === 2) for (const [index, identity] of identities.entries()) {
          db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at)
            VALUES (?, ?, ?, ?, ?, ?)`).run(index === 0 ? 'context-first' : 'context-newest', repoId,
              identity.worktreeKey, identity.root, index + 1, index + 1);
        }
        const before = snapshot(db);
        const json = output();
        assert.equal(await runImport([...args, '--json'], json.io), 0, json.text.error || json.text.out);
        const contextCandidates = count === 0 ? [] : ['context-first', 'context-newest'];
        assert.deepEqual(JSON.parse(json.text.out).mapping.projects,
          [{ sourceHash: '9d40a5cff7158826c7f86e75c709f78e4bfa1a40a69f4932f25b9bc6a598377d', destinationRepo: repoId,
            context: null, contextCandidates, contextCandidatesOmitted: 0 }]);
        const human = output();
        assert.equal(await runImport(args, human.io), 0, human.text.error || human.text.out);
        assert.deepEqual(human.text.out.split('\n').filter((line) => line.startsWith('Context candidate ')),
          contextCandidates.map((id) => `Context candidate ${repoId}: ${id}.`));
        assert.doesNotMatch(human.text.out, /context candidates omitted/u);
        assert.equal((json.text.out + human.text.out).includes(home), false);
        assert.deepEqual(snapshot(db), before);
      }
      for (const id of ['context-first', 'context-newest']) {
        assert.equal(verifiedRepoContext(db, repoId, id)?.id, id);
        const explicit = output();
        assert.equal(await runImport([...args, '--map-context', `${repoId}=${id}`, '--json'], explicit.io), 0, explicit.text.error || explicit.text.out);
        assert.deepEqual(JSON.parse(explicit.text.out).mapping.projects,
          [{ sourceHash: '9d40a5cff7158826c7f86e75c709f78e4bfa1a40a69f4932f25b9bc6a598377d', destinationRepo: repoId, context: id }]);
        const human = output();
        assert.equal(await runImport([...args, '--map-context', `${repoId}=${id}`], human.io), 0, human.text.error || human.text.out);
        assert.doesNotMatch(human.text.out, /Context candidate|context candidates omitted/u);
      }
      const extraIds = Array.from({ length: 10 }, (_, index) => `context-${String(index).padStart(2, '0')}`);
      for (const id of extraIds.toReversed()) db.prepare(`INSERT INTO work_contexts
        (id, repo_id, local_key, root, created_at, last_seen_at) VALUES (?, ?, ?, ?, 3, 3)`)
        .run(id, repoId, `unverified-${id}`, roots[0]);
      const before = snapshot(db);
      const bounded = output();
      assert.equal(await runImport([...args, '--json'], bounded.io), 0, bounded.text.error || bounded.text.out);
      const project = JSON.parse(bounded.text.out).mapping.projects[0];
      assert.equal(project.context, null);
      assert.deepEqual(project.contextCandidates, extraIds);
      assert.equal(project.contextCandidatesOmitted, 2);
      const human = output();
      assert.equal(await runImport(args, human.io), 0, human.text.error || human.text.out);
      assert.deepEqual(human.text.out.split('\n').filter((line) => line.startsWith('Context candidate ')),
        extraIds.map((id) => `Context candidate ${repoId}: ${id}.`));
      assert.ok(human.text.out.split('\n').includes(`2 context candidates omitted for ${repoId}.`));
      assert.equal((bounded.text.out + human.text.out).includes(home), false);
      assert.deepEqual(snapshot(db), before);
      db.prepare("UPDATE work_contexts SET local_key = 'replaced-generation' WHERE id = 'context-newest'").run();
      for (const id of ['unknown-context', 'context-newest']) {
        const invalid = output();
        assert.equal(await runImport([...args, '--map-context', `${repoId}=${id}`, '--json'], invalid.io), 2);
        assert.deepEqual(JSON.parse(invalid.text.out).rejected, [{ line: 0, reason: 'invalid_context_mapping' }]);
      }
    } finally {
      db.close();
      t.mock.restoreAll();
      syncBuiltinESMExports();
    }
  });
});

test('matrix A2: native preview bounds project and unresolved details with matching human counts', async () => {
  await withFixture(async (fixture) => {
    const records = Array.from({ length: 107 }, (_, index) => repo(`project-${String(index).padStart(3, '0')}`));
    const file = nativeFile(fixture.paths.home, records);
    const before = snapshot(fixture.db);
    const json = output();
    assert.equal(await runImport([file, '--dry-run', '--json'], json.io), 2);
    const result = JSON.parse(json.text.out);
    assert.equal(result.mapping.projects.length, 100);
    assert.equal(result.mapping.omitted, 7);
    const hashes = records.slice(0, 100).map((record) => createHash('sha256').update(record.id).digest('hex'));
    assert.deepEqual(result.mapping.projects.map((project: { sourceHash: string }) => project.sourceHash), hashes);
    assert.deepEqual(result.mapping.unresolved, hashes);
    assert.equal(result.mapping.unresolvedOmitted, 7);
    assert.equal(result.rejected.length, 100);
    const human = output();
    assert.equal(await runImport([file, '--dry-run'], human.io), 2);
    assert.equal(human.text.out.split('\n').filter((line) => line.startsWith('Project SHA-256 ')).length, 100);
    assert.match(human.text.out, /7 project details omitted\./u);
    assert.deepEqual(human.text.out.split('\n').filter((line) => line.includes('unresolved projects')),
      [`${result.mapping.unresolved.length + result.mapping.unresolvedOmitted} unresolved projects.`]);
    assert.equal(human.text.error.trim().split('\n').length, 100);
    assert.deepEqual(snapshot(fixture.db), before);
  });
});

test('matrix A3: preview preserves missing, behind, ahead and writer-held WAL destinations', async () => {
  await withTempHome(async (home) => {
    const identity = 'example.test/schema';
    const repoId = sha256Hex(identity).slice(0, 16);
    const file = nativeFile(home, [{ kind: 'repo', id: repoId, identity_kind: 'remote', normalized_identity: identity }, memory('schema', repoId)]);
    const bytes = readFileSync(file);
    for (const schema of ['missing', 'behind', 'ahead', 'ready']) {
      const target = join(home, schema);
      const previous = process.env.OBOETE_HOME;
      process.env.OBOETE_HOME = target;
      let db: DatabaseSync | undefined;
      let writer: DatabaseSync | undefined;
      try {
        if (schema !== 'missing') {
          mkdirSync(target);
          if (schema === 'behind') {
            db = new DatabaseSync(oboetePaths(target).db);
            for (const migration of MIGRATIONS.filter((item) => item.version <= 6)) {
              db.exec('BEGIN IMMEDIATE');
              db.exec(migration.sql);
              db.prepare('INSERT INTO schema_migrations VALUES (?, ?, ?, ?)')
                .run(migration.version, migration.name, sha256Hex(migration.sql), 1);
              db.exec(`PRAGMA user_version = ${migration.version}; COMMIT`);
            }
            assert.equal(db.prepare('PRAGMA user_version').get()?.user_version, 6);
            assert.deepEqual(db.prepare('SELECT version FROM schema_migrations ORDER BY version').all().map((row) => row.version), [1, 2, 3, 4, 5, 6]);
            assert.equal(db.prepare("SELECT name FROM sqlite_master WHERE name = 'migration_records'").get(), undefined);
          } else db = openDatabase({ path: oboetePaths(target).db, timeoutMs: 2_000 }).db;
          if (schema === 'ahead') db.exec('PRAGMA user_version = 8');
          if (schema === 'ready') {
            assert.equal(db.prepare('PRAGMA user_version').get()?.user_version, 7);
            assert.equal(db.prepare('PRAGMA journal_mode').get()?.journal_mode, 'wal');
            writer = openDatabase({ path: oboetePaths(target).db, timeoutMs: 2_000 }).db;
            writer.exec("BEGIN IMMEDIATE; INSERT INTO runtime_state (key, value_json, updated_at) VALUES ('uncommitted-writer', '1', 1)");
          }
        }
        const state = () => db === undefined ? null : {
          version: db.prepare('PRAGMA user_version').get(),
          schema: db.prepare('SELECT * FROM sqlite_master ORDER BY name').all(), tables: snapshot(db),
        };
        const before = state();
        const preview = output();
        assert.equal(await runImport([file, '--dry-run', '--json'], preview.io), 0, preview.text.error || preview.text.out);
        assert.equal(JSON.parse(preview.text.out).destinationSchema, schema);
        assert.equal(JSON.parse(preview.text.out).applied, false);
        assert.equal(JSON.parse(preview.text.out).inserted, 1);
        assert.deepEqual(state(), before);
        if (schema === 'missing' || schema === 'behind' || schema === 'ahead') {
          const applied = output();
          assert.equal(await runImport([file, '--apply', '--json'], applied.io), 2);
          assert.equal(JSON.parse(applied.text.out).rejected[0].reason, 'destination_schema_not_ready');
          assert.equal(JSON.parse(applied.text.out).applied, false);
          assert.deepEqual(state(), before);
        }
        if (schema === 'missing') {
          assert.equal(existsSync(oboetePaths(target).db), false);
          assert.equal(existsSync(target), false);
        }
        if (writer !== undefined) assert.equal(writer.isTransaction, true);
        assert.deepEqual(readFileSync(file), bytes);
      } finally {
        if (writer?.isTransaction) writer.exec('ROLLBACK');
        writer?.close();
        db?.close();
        if (previous === undefined) delete process.env.OBOETE_HOME;
        else process.env.OBOETE_HOME = previous;
      }
    }
  });
});

test('matrix B4: changing mappings for the same file rejects without any table changes', async () => {
  await withFixture(async (fixture) => {
    fixture.db.exec("INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('other', 'common_dir', '/other')");
    const file = nativeFile(fixture.paths.home, [repo(), memory()]);
    const first = output();
    assert.equal(await runImport([file, '--apply', '--map-repo', `source-repo=${fixture.identity.id}`], first.io), 0, first.text.error || first.text.out);
    const before = snapshot(fixture.db);
    const changed = output();
    assert.equal(await runImport([file, '--apply', '--map-repo', 'source-repo=other'], changed.io), 2);
    assert.equal(changed.text.error, 'line 0: import_mapping_changed\n');
    assert.deepEqual(snapshot(fixture.db), before);
  });
});

test('matrix B5: an overlapping origin mapped elsewhere rolls back even after earlier records', async () => {
  for (const conflictFirst of [true, false]) await withFixture(async (fixture) => {
    fixture.db.exec("INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('other', 'common_dir', '/other')");
    const shared = memory('shared');
    const first = nativeFile(fixture.paths.home, [repo(), shared], 'first.jsonl');
    const initial = output();
    assert.equal(await runImport([first, '--apply', '--map-repo', `source-repo=${fixture.identity.id}`], initial.io), 0, initial.text.error || initial.text.out);
    const fresh = memory('fresh');
    const second = nativeFile(fixture.paths.home, [repo(), ...(conflictFirst ? [shared, fresh] : [fresh, shared])], 'second.jsonl', 2);
    assert.notDeepEqual(readFileSync(first), readFileSync(second));
    const before = snapshot(fixture.db);
    const rejected = output();
    assert.equal(await runImport([second, '--apply', '--map-repo', 'source-repo=other'], rejected.io), 2);
    assert.equal(rejected.text.error, `line ${conflictFirst ? 3 : 4}: origin_mapping_changed\n`);
    assert.deepEqual(snapshot(fixture.db), before);
  });
});

test('matrix B6: two source projects can share a destination while retaining distinct origins', async () => {
  await withFixture(async (fixture) => {
    const file = nativeFile(fixture.paths.home, [repo('one'), repo('two'), memory('one-memory', 'one'), memory('two-memory', 'two')]);
    const args = [file, '--json', '--map-repo', `one=${fixture.identity.id}`, '--map-repo', `two=${fixture.identity.id}`];
    const preview = output();
    assert.equal(await runImport([...args, '--dry-run'], preview.io), 0, preview.text.error || preview.text.out);
    assert.equal(JSON.parse(preview.text.out).mapping.collisions, 1);
    assert.equal(fixture.db.prepare('SELECT COUNT(*) AS n FROM migration_records').get()?.n, 0);
    const applied = output();
    assert.equal(await runImport([...args, '--apply'], applied.io), 0, applied.text.error || applied.text.out);
    assert.equal(JSON.parse(applied.text.out).inserted, 2);
    const origins = fixture.db.prepare('SELECT origin_key, target_key FROM migration_records ORDER BY id').all();
    assert.equal(origins.length, 2);
    assert.notEqual(origins[0].origin_key, origins[1].origin_key);
    assert.deepEqual(origins.map((row) => row.target_key), [`repo:${fixture.identity.id}`, `repo:${fixture.identity.id}`]);
    const before = snapshot(fixture.db);
    const repeated = output();
    assert.equal(await runImport([...args, '--dry-run'], repeated.io), 0, repeated.text.error || repeated.text.out);
    const duplicate = JSON.parse(repeated.text.out);
    assert.equal(duplicate.duplicate, true);
    assert.equal(duplicate.mapping.collisions, 1);
    assert.deepEqual(duplicate.mapping.projects.map((project: { destinationRepo: string }) => project.destinationRepo),
      [fixture.identity.id, fixture.identity.id]);
    assert.deepEqual(duplicate.mapping.unresolved, []);
    assert.equal(duplicate.mapping.unresolvedOmitted, 0);
    const human = output();
    assert.equal(await runImport(args.filter((arg) => arg !== '--json'), human.io), 0, human.text.error || human.text.out);
    assert.match(human.text.out, /1 project collisions\./u);
    assert.match(human.text.out, /^0 unresolved projects\.$/mu);
    assert.doesNotMatch(human.text.out, /mapping required/u);
    assert.deepEqual(snapshot(fixture.db), before);
  });
});

async function proposalFile(source: Fixture) {
  const { db, identity } = source;
  insertSession(source, { id: 'source-session', agent: 'claude' });
  const workId = `fixture-work:${identity.id}`;
  const statements = [
    ['pending', 'Pending preference', 'Use short paragraphs.'],
    ['approved', 'Approved preference', 'Reply in Japanese.'],
    ['rejected', 'Rejected preference', 'Use long paragraphs.'],
  ] as const;
  for (const [state, title, body] of statements) {
    const material = materialHash(title, body);
    const content = contentHash(identity.id, material);
    const id = memoryIdFor(content);
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, material_hash,
      content_hash, sensitivity, review_state, provenance_complete, created_at)
      VALUES (?, ?, 'discovery', ?, ?, '[]', ?, ?, 'local_only', 'reviewed', 1, 1)`)
      .run(id, identity.id, title, body, material, content);
    grantVisibility(db, id, { audience: 'work', repoId: identity.id, workId }, 'observer', 1);
    const personalHash = sha256Json(['personal-projection-v1', title, body]);
    const projectionId = state === 'approved' ? memoryIdFor(personalHash) : null;
    if (projectionId !== null) db.prepare(`INSERT INTO memories (id, repo_id, type, title, body,
      concepts, material_hash, content_hash, sensitivity, review_state, created_at)
      VALUES (?, ?, 'decision', ?, ?, '[]', ?, ?, 'local_only', 'reviewed', 2)`)
      .run(projectionId, identity.id, title, body, material, personalHash);
    db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
      candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity,
      source_event_ids_json, basis, state, decision_channel, projected_memory_id, created_at, decided_at)
      VALUES (?, ?, ?, ?, ?, ?, ?, 'local_only', '[]', 'inferred', ?, ?, ?, 1, ?)`)
      .run(`proposal-${state}`, id, identity.id, workId, title, body, material, state,
        state === 'pending' ? null : 'cli', projectionId, state === 'pending' ? null : 2);
    if (projectionId !== null) grantVisibility(db, projectionId,
      { audience: 'personal', proposalId: `proposal-${state}` }, 'proposal_approval', 2);
  }
  return exportedFile(source);
}

test('matrix C7: all proposal decisions round trip as private history without nested origins or grants', async () => {
  await withFixture(async (source) => {
    const file = await proposalFile(source);
    const bytes = readFileSync(file.path);
    const proposals = file.records.filter((row) => row.kind === 'sharing_proposal');
    assert.deepEqual(proposals.map((row) => row.state).sort(), ['approved', 'pending', 'rejected']);
    await withFixture(async (target) => {
      insertSession(target, { id: 'target-session', agent: 'codex' });
      const result = output();
      assert.equal(await runImport([file.path, '--apply', '--map-repo', `${source.identity.id}=${target.identity.id}`], result.io), 0, result.text.error || result.text.out);
      const { db } = target;
      const assertHistory = (state: 'pending' | 'clean') => {
        const held = db.prepare("SELECT payload_json, classification_state FROM migration_records WHERE record_kind = 'sharing_proposal'").all();
        assert.equal(held.length, 3);
        for (const proposal of proposals) {
          const record = held.find((row) => JSON.parse(String(row.payload_json)).id === proposal.id);
          assert.ok(record, proposal.id);
          assert.deepEqual(JSON.parse(String(record.payload_json)), proposal);
          assert.equal(record.classification_state, state);
        }
        assert.equal(db.prepare('SELECT COUNT(*) AS n FROM sharing_proposals').get()?.n, 0);
        assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memory_visibility WHERE audience = 'personal'").get()?.n, 0);
        const personal = db.prepare(`SELECT m.* FROM memories m JOIN migration_records r ON r.destination_memory_id = m.id
          WHERE r.record_kind = 'memory' AND r.identity_domain = 'personal_projection'`).all();
        assert.equal(personal.length, 1);
        assert.equal(personal[0].title, 'Approved preference');
        assert.equal(personal[0].body, 'Reply in Japanese.');
        for (const field of ['work_id', 'source_session_id', 'source_batch_id', 'checkpoint_parent_id']) assert.equal(personal[0][field], null);
        assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ?').get(personal[0].id)?.n, 0);
        assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_visibility WHERE memory_id = ?').get(personal[0].id)?.n, 0);
      };
      assertHistory('pending');
      const origins = db.prepare('SELECT origin_key, payload_hash FROM migration_records ORDER BY origin_key').all();
      assert.equal(origins.length, 13);
      await classify(target);
      assertHistory('clean');
      const exported = await exportedFile(target);
      const inherited = exported.records.filter((row) => row.kind === 'migration_origin');
      assert.deepEqual(inherited.map((row) => ({ origin_key: row.id, payload_hash: row.payload_hash }))
        .sort((a, b) => a.origin_key.localeCompare(b.origin_key)), origins.map((row) => ({ ...row })));
      const transferred = new Set(inherited.map((row) => row.id));
      assert.equal(transferred.size, 13);
      await withFixture(async (next) => {
        insertSession(next, { id: 'next-session', agent: 'codex' });
        const imported = output();
        assert.equal(await runImport([exported.path, '--apply', '--map-repo', `${target.identity.id}=${next.identity.id}`], imported.io), 0, imported.text.error || imported.text.out);
        await classify(next);
        const reexported = await exportedFile(next);
        const repeated = reexported.records.filter((row) => row.kind === 'migration_origin');
        assert.equal(new Set(repeated.map((row) => row.id)).size, repeated.length);
        for (const row of inherited) {
          const copies = repeated.filter((record) => record.id === row.id);
          assert.equal(copies.length, 1);
          assert.equal(copies[0].payload_hash, row.payload_hash);
          assert.equal(copies[0].origin_json, row.origin_json);
        }
        assert.ok(repeated.every((row) => row.payload === null || row.payload.kind !== 'migration_origin'));
      });
    });
    assert.deepEqual(readFileSync(file.path), bytes);
  });
});

test('matrix C8: redacted source dependencies stay terminal after a forged plaintext replay', async () => {
  for (const terminal of ['secret', 'deleted']) await withFixture(async (source) => {
    const parent = memory('terminal-parent', source.identity.id);
    source.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, material_hash,
      content_hash, sensitivity, review_state, deleted_at, created_at)
      VALUES (?, ?, 'discovery', '', '', '[]', ?, ?, ?, 'reviewed', ?, 1)`)
      .run(parent.id, source.identity.id, parent.material_hash, parent.content_hash,
        terminal === 'secret' ? 'secret' : 'local_only', terminal === 'deleted' ? 2 : null);
    source.db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value,
      source_agent, evidence, capture_root, source_paths_json)
      VALUES (?, 'historical-event', 'file_read', 'private/source.ts', 'claude', 'Hidden source evidence.', '/foreign', '["private/source.ts"]')`)
      .run(parent.id);
    const file = await exportedFile(source);
    const record = file.records.find((row) => row.kind === 'source');
    assert.ok(record && record.kind === 'source');
    for (const field of ['evidence', 'citation_value', 'source_agent', 'capture_root', 'source_paths_json'] as const) assert.equal(record[field], null);
    await withFixture(async (target) => {
      const applied = output();
      assert.equal(await runImport([file.path, '--apply', '--map-repo', `${source.identity.id}=${target.identity.id}`], applied.io), 0, applied.text.error || applied.text.out);
      const receipt = target.db.prepare("SELECT * FROM migration_records WHERE record_kind = 'source'").get()!;
      const expectedState = terminal === 'secret' ? 'secret' : 'not_applicable';
      assert.equal(receipt.payload_json, null);
      assert.equal(receipt.classification_state, expectedState);
      const plaintext = { ...record, evidence: 'Forged unredacted evidence.', citation_value: 'private/source.ts' };
      // Migration "Stable receipts": forge the stored payload and remove the parent link, keeping its stable origin.
      const replay = nativeFile(target.paths.home, [repo(), { kind: 'migration_origin', id: String(receipt.origin_key),
        repo_id: 'source-repo', memory_id: null, origin_json: String(receipt.origin_json),
        payload_hash: String(receipt.payload_hash), stored_payload_hash: sha256Hex(JSON.stringify(plaintext)),
        record_kind: 'source', payload: plaintext, classification_state: 'pending' }], 'forged.jsonl', 2);
      const forged = output();
      // A retainable payload with no memory in the file is refused outright (`orphan_origin_payload`).
      assert.equal(await runImport([replay, '--apply', '--map-repo', `source-repo=${target.identity.id}`], forged.io), 2, forged.text.error);
      assert.match(forged.text.error, /orphan_origin_payload/);
      assert.deepEqual(target.db.prepare('SELECT * FROM migration_records WHERE id = ?').get(receipt.id), receipt);
      assert.equal(target.db.prepare("SELECT COUNT(*) AS n FROM migration_records WHERE payload_json LIKE '%Forged unredacted evidence%'").get()?.n, 0);
    });
  });
});

test('matrix C9: revoked personal grants preserve export identity and tombstones survive reimport', async () => {
  await withFixture(async (source) => {
    const file = await proposalFile(source);
    await withFixture(async (target) => {
      insertSession(target, { id: 'target-session', agent: 'codex' });
      const args = [file.path, '--apply', '--map-repo', `${source.identity.id}=${target.identity.id}`];
      const result = output();
      assert.equal(await runImport(args, result.io), 0, result.text.error || result.text.out);
      await classify(target);
      const { db } = target;
      const personal = db.prepare(`SELECT destination_memory_id FROM migration_records
        WHERE record_kind = 'memory' AND identity_domain = 'personal_projection'`).get()!;
      const id = String(personal.destination_memory_id);
      // Migration "Historical scope": a fresh local human decision must create the grant before revocation.
      const held = db.prepare(`SELECT id FROM migration_records WHERE record_kind = 'sharing_proposal'
        AND json_extract(payload_json, '$.state') = 'approved'`).get()!;
      const cwd = process.cwd();
      process.chdir(target.repo);
      try {
        const promoted = output();
        assert.equal(await runImport(['promote', String(held.id), '--work', `fixture-work:${target.identity.id}`, '--json'], promoted.io), 0, promoted.text.error || promoted.text.out);
        const decision = await decideSharing(db, { repoId: target.identity.id, bindingId: 'fixture-binding:target-session', home: target.paths.home },
          { id: JSON.parse(promoted.text.out).id, decision: 'approve', channel: 'cli', now: 3_000 });
        assert.equal(decision?.projectedMemoryId, id);
      } finally { process.chdir(cwd); }
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memory_visibility WHERE memory_id = ? AND audience = 'personal'").get(id)?.n, 1);
      assert.equal(db.prepare("DELETE FROM memory_visibility WHERE memory_id = ? AND audience = 'personal'").run(id).changes, 1);
      const exported = await exportedFile(target);
      const projection = exported.records.find((row) => row.kind === 'memory' && row.id === id);
      assert.ok(projection && projection.kind === 'memory');
      assert.equal(projection.identity_domain, 'personal_projection');
      db.prepare('UPDATE memories SET deleted_at = 4000 WHERE id = ?').run(id);
      const before = snapshot(db);
      const repeated = output();
      assert.equal(await runImport(args, repeated.io), 0, repeated.text.error || repeated.text.out);
      assert.deepEqual(snapshot(db), before);
      const overlapping = join(source.paths.home, 'overlapping.jsonl');
      const original = readFileSync(file.path, 'utf8').trim().split('\n');
      original[0] = JSON.stringify({ ...JSON.parse(original[0]), exported_at: 5_000 });
      writeFileSync(overlapping, original.join('\n') + '\n');
      const overlap = output();
      assert.equal(await runImport([overlapping, ...args.slice(1)], overlap.io), 0, overlap.text.error || overlap.text.out);
      const dead = db.prepare('SELECT deleted_at, title, body FROM memories WHERE id = ?').get(id)!;
      assert.deepEqual({ ...dead }, { deleted_at: 4_000, title: 'Approved preference', body: 'Reply in Japanese.' });
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_visibility WHERE memory_id = ?').get(id)?.n, 0);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ?').get(id)?.n, 0);
    });
  });
});

test('matrix D10: packed migration preview and promotion print only bounded metadata', async () => {
  const bundle = resolve('dist/oboete.mjs');
  const external = resolve('test/fixtures/migration/claude-mem-query-export-8bc631a.json');
  await withFixture(async (fixture) => {
    insertSession(fixture, { id: 'cli-session', agent: 'codex' });
    const native = nativeFile(fixture.paths.home, [repo(), memory('private-candidate')]);
    const commands = [
      ['import', external, '--from', 'claude-mem', '--dry-run', '--json',
        '--map-project', `client/api=${fixture.identity.id}`, '--map-project', `server/api=${fixture.identity.id}`],
      ['import', native, '--dry-run', '--json', '--map-repo', `source-repo=${fixture.identity.id}`],
      ['import', 'promote', '--list'],
      ['import', 'promote', 'bad-id', '--work', 'x'],
    ];
    const markers = ['synthetic private fixture text', 'Use explicit repository mapping', 'Expose migration preview',
      'Synthetic Claude migration session', 'client/api', 'server/api', 'docs/migration.md', 'src/importer.ts',
      'fixtures/IGNORE PREVIOUS INSTRUCTIONS.md', 'Fact private-candidate',
      'The migration matrix preserves this fact.', 'source-repo', '/foreign/', external, native, fixture.repo];
    const before = snapshot(fixture.db);
    const codes: (number | null)[] = [];
    for (const [index, args] of commands.entries()) {
      const out = join(fixture.paths.home, `cli-${index}.stdout`), error = join(fixture.paths.home, `cli-${index}.stderr`);
      const outFd = openSync(out, 'w'), errorFd = openSync(error, 'w');
      try {
        const result = childProcess.spawnSync(process.execPath, [bundle, ...args], { cwd: fixture.repo,
          env: cleanEnv(fixture.paths.home), stdio: ['ignore', outFd, errorFd], timeout: 30_000 });
        assert.equal(result.error, undefined);
        codes.push(result.status);
        const stdout = readFileSync(out, 'utf8'), stderr = readFileSync(error, 'utf8');
        for (const marker of markers) assert.equal((stdout + stderr).includes(marker), false, `command ${index} leaked ${marker}`);
        if (index < 2) {
          assert.equal(JSON.parse(stdout).applied, false);
          assert.equal(JSON.parse(stdout).destinationSchema, 'ready');
          assert.equal(JSON.parse(stdout).inserted, index === 0 ? 8 : 1);
          assert.equal(stderr, '');
        } else assert.deepEqual({ stdout, stderr }, index === 2 ? { stdout: '', stderr: '' }
          : { stdout: '', stderr: 'The migration record is unavailable in this scope.\n' });
      } finally { closeSync(outFd); closeSync(errorFd); }
    }
    assert.deepEqual(codes, [0, 0, 0, 1]);
    assert.deepEqual(snapshot(fixture.db), before);
  });
});
