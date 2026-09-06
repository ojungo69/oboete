import assert from 'node:assert/strict';
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { ensureDirectories, oboetePaths } from '../../src/paths.js';
import {
  listSpool,
  quarantineSpoolEntry,
  readSpoolEntry,
  removeSpoolEntry,
  writeSpoolEntry,
  type SpoolEntry,
} from '../../src/spool.js';
import { withTempHome } from '../helpers/home.js';

const CAPTURED_AT = 1_757_000_000_000;

function entry(capturedAt: number, id: string, content = 'the deployment notes'): SpoolEntry {
  return {
    repo: {
      id: 'a1b2c3d4e5f60718',
      identity_kind: 'common_dir',
      normalized_identity: '/tmp/oboete-spool',
      display_root: '/tmp/oboete-spool',
    },
    session: {
      id: 'session-1',
      repo_id: 'a1b2c3d4e5f60718',
      agent: 'claude',
      native_session_id: 'native-1',
      conversation_id: 'session-1',
      model: null,
      started_at: capturedAt,
      status: 'active',
    },
    row: {
      id,
      repo_id: 'a1b2c3d4e5f60718',
      session_id: 'session-1',
      turn_id: null,
      agent: 'claude',
      kind: 'prompt',
      content,
      truncated: 0,
      payload_json: '{"kind":"prompt"}',
      content_hash: 'f'.repeat(64),
      sensitivity: 'local_only',
      classification_state: 'done',
      captured_at: capturedAt,
      expires_at: capturedAt + 1_000,
    },
  };
}

test('writeSpoolEntry renames into place and leaves no temporary file behind', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const written = entry(CAPTURED_AT, 'event-1');

    writeSpoolEntry(paths, written);

    const names = readdirSync(paths.spool).filter((name) => name.endsWith('.json'));
    assert.deepEqual(names, [`${CAPTURED_AT}-event-1.json`]);
    assert.deepEqual(
      JSON.parse(readFileSync(join(paths.spool, names[0] as string), 'utf8')),
      written,
      'the file holds exactly the entry the hook derived',
    );
  });
});

test('listSpool returns the entries in name order', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    for (const [capturedAt, id] of [
      [CAPTURED_AT + 2, 'c'],
      [CAPTURED_AT, 'a'],
      [CAPTURED_AT + 1, 'b'],
    ] as const) {
      writeSpoolEntry(paths, entry(capturedAt, id));
    }

    assert.deepEqual(listSpool(paths), [
      `${CAPTURED_AT}-a.json`,
      `${CAPTURED_AT + 1}-b.json`,
      `${CAPTURED_AT + 2}-c.json`,
    ]);
  });
});

test('readSpoolEntry and removeSpoolEntry round trip one entry', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const written = entry(CAPTURED_AT, 'event-1', 'a note about the migration');
    writeSpoolEntry(paths, written);
    const [name] = listSpool(paths);
    assert.ok(name !== undefined);

    assert.deepEqual(readSpoolEntry(paths, name), written);

    removeSpoolEntry(paths, name);
    assert.deepEqual(listSpool(paths), []);
    // Recovery is idempotent, so removing an entry that is already gone is not an error (R6).
    removeSpoolEntry(paths, name);
  });
});

test('readSpoolEntry refuses an entry that does not match the schema', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    writeFileSync(join(paths.spool, '1-broken.json'), '{"row":{"kind":"nonsense"}}');

    assert.equal(readSpoolEntry(paths, '1-broken.json'), null);
    assert.equal(readSpoolEntry(paths, '1-missing.json'), null);
  });
});

test('quarantineSpoolEntry moves the file out of the spool', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    writeFileSync(join(paths.spool, '1-broken.json'), '{ not json');

    quarantineSpoolEntry(paths, '1-broken.json');

    assert.deepEqual(listSpool(paths), []);
    assert.deepEqual(readdirSync(paths.spoolFailed), ['1-broken.json']);
  });
});
