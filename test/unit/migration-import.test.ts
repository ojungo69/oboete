import assert from 'node:assert/strict';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { openDatabase } from '../../src/db/open.js';
import { grantVisibility } from '../../src/db/queries.js';
import { oboetePaths } from '../../src/paths.js';
import { exportMemories, runExport, runImport } from '../../src/transfer.js';
import { withTempHome } from '../helpers/home.js';
import { insertSession, withFixture } from '../helpers/inject-fixture.js';
import { output } from '../helpers/output.js';

test('default native export retains exact pending candidate, source evidence and work visibility', async () => {
  await withFixture(async (fixture) => {
    const { db, identity } = fixture;
    insertSession(fixture, { id: 'native-origin-session', agent: 'claude' });
    const workId = `fixture-work:${identity.id}`;
    const material = materialHash('Preference', 'Reply in Japanese.');
    const content = contentHash(identity.id, material);
    const memoryId = memoryIdFor(content);
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, material_hash, content_hash,
      sensitivity, review_state, source_session_id, provenance_complete, created_at)
      VALUES (?, ?, 'discovery', 'Preference', 'Reply in Japanese.', ?, ?, 'local_only',
      'reviewed', 'native-origin-session', 1, 1)`).run(memoryId, identity.id, material, content);
    grantVisibility(db, memoryId, { audience: 'work', repoId: identity.id, workId }, 'observer', 1);
    db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, evidence, source_hash,
      portion_start, portion_end, source_total, captured_at, capture_root, source_paths_json)
      VALUES (?, 'foreign-raw-origin', 'The exact original evidence.', ?, 0, 28, 28, 1, ?, '[]')`)
      .run(memoryId, 'a'.repeat(64), fixture.repo);
    db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
      candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity,
      source_event_ids_json, basis, state, created_at)
      VALUES ('native-proposal', ?, ?, ?, 'PREFERENCE', 'Reply in Japanese.', ?, 'local_only',
      '["foreign-raw-origin"]', 'inferred', 'pending', 2)`).run(memoryId, identity.id, workId, material);

    const { text, io } = output();
    assert.equal(await runExport(['-'], io), 0, text.error);
    const lines = text.out.trim().split('\n').map((line) => JSON.parse(line) as Record<string, unknown>);
    assert.equal(lines[0]?.format, 'oboete-export/2');
    const candidate = lines.find((line) => line.kind === 'sharing_proposal');
    assert.equal(candidate?.candidate_title, 'PREFERENCE');
    assert.equal(candidate?.candidate_body, 'Reply in Japanese.');
    assert.equal(candidate?.state, 'pending');
    assert.equal(candidate?.decision_channel, null);
    assert.ok(lines.some((line) => line.kind === 'source' && line.evidence === 'The exact original evidence.'));
    assert.ok(lines.some((line) => line.kind === 'visibility' && line.audience === 'work' && line.work_id === workId));
    const source = join(fixture.paths.home, 'native.jsonl');
    writeFileSync(source, text.out);
    const sourceBytes = readFileSync(source);
    await withFixture(async (target) => {
      insertSession(target, { id: 'current-target-session', agent: 'codex' });
      const states = () => ['sessions', 'work_items', 'work_contexts', 'work_bindings', 'raw_events', 'injections']
        .map((table) => JSON.stringify(target.db.prepare(`SELECT * FROM ${table} ORDER BY id`).all()));
      const before = states();
      const mapping = `${identity.id}=${target.identity.id}`;
      const preview = output();
      assert.equal(await runImport([source, '--map-repo', mapping, '--json'], preview.io), 0, preview.text.error);
      assert.equal(JSON.parse(preview.text.out).applied, false);
      assert.equal(target.db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
      const applied = output();
      assert.equal(await runImport([source, '--map-repo', mapping, '--apply', '--json'], applied.io), 0, applied.text.error);
      assert.equal(JSON.parse(applied.text.out).inserted, 1);
      assert.deepEqual(states(), before, 'foreign history changes no active local state');
      assert.equal(target.db.prepare('SELECT COUNT(*) AS n FROM sharing_proposals').get()?.n, 0);
      assert.equal(target.db.prepare('SELECT COUNT(*) AS n FROM memory_visibility').get()?.n, 0, 'foreign work stays held without a work map');
      const stored = target.db.prepare('SELECT * FROM memories').get()!;
      assert.equal(stored.review_state, 'imported');
      assert.equal(stored.source_session_id, null);
      assert.equal(stored.work_id, null);
      assert.equal(stored.checkpoint_parent_id, null);
      assert.equal(target.db.prepare('SELECT COUNT(*) AS n FROM memory_sources').get()?.n, 0);
      const provenance = target.db.prepare("SELECT payload_json FROM migration_records WHERE record_kind = 'source'").get()!;
      assert.equal(JSON.parse(String(provenance.payload_json)).raw_event_id, 'foreign-raw-origin');
      assert.equal(JSON.parse(String(provenance.payload_json)).evidence, 'The exact original evidence.');
      const receipt = target.db.prepare("SELECT payload_json FROM migration_records WHERE record_kind = 'sharing_proposal'").get()!;
      assert.equal(JSON.parse(String(receipt.payload_json)).candidate_title, 'PREFERENCE');
      const count = target.db.prepare('SELECT COUNT(*) AS n FROM migration_records').get()?.n;
      const repeated = output();
      assert.equal(await runImport([source, '--map-repo', mapping, '--apply', '--json'], repeated.io), 0, repeated.text.error);
      assert.equal(JSON.parse(repeated.text.out).duplicate, true);
      assert.equal(target.db.prepare('SELECT COUNT(*) AS n FROM migration_records').get()?.n, count);
      assert.deepEqual(readFileSync(source), sourceBytes);
      const again = output();
      assert.equal(await runExport(['-'], again.io), 0, again.text.error);
      const restored = again.text.out.trim().split('\n').map((line) => JSON.parse(line) as Record<string, unknown>);
      assert.ok(restored.some((line) => line.kind === 'migration_origin'
        && (line.payload as Record<string, unknown> | null)?.candidate_title === 'PREFERENCE'), 're-export keeps the exact held proposal');
    });
  });
});

test('a v1 dry run reads the source without creating a missing destination store', async () => {
  await withTempHome(async (sourceHome) => {
    const db = openDatabase({ path: oboetePaths(sourceHome).db, timeoutMs: 2_000 }).db;
    const lines: string[] = [];
    exportMemories(db, (line) => lines.push(line), 1);
    db.close();
    const source = join(sourceHome, 'v1.jsonl');
    writeFileSync(source, `${lines.join('\n')}\n`);
    const before = readFileSync(source);
    await withTempHome(async (targetHome) => {
      const { text, io } = output();
      assert.equal(await runImport([source, '--dry-run'], io), 0, text.error);
      assert.equal(existsSync(oboetePaths(targetHome).db), false);
      assert.deepEqual(readFileSync(source), before);
    });
  });
});

test('native input rejects invalid UTF-8 before opening the destination', async () => {
  await withTempHome(async (home) => {
    const source = join(home, 'invalid.jsonl');
    writeFileSync(source, Buffer.concat([
      Buffer.from('{"format":"oboete-export/1","repos":[],"unknown":"'),
      Buffer.from([255]), Buffer.from('"}\n'),
    ]));
    const { text, io } = output();
    assert.equal(await runImport([source], io), 2, text.out);
    assert.equal(existsSync(oboetePaths(home).db), false);
    assert.ok(!text.error.includes('\ufffd'));
  });
});
