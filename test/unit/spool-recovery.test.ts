import assert from 'node:assert/strict';
import { mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { claimLease } from '../../src/worker/lease.js';
import { recoverSpool } from '../../src/worker/spool-recovery.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_757_000_000_000;
const DAY = 24 * 60 * 60 * 1000;

async function withOpened(
  fn: (db: DatabaseSync, home: string, token: string) => void | Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      const token = claimLease(opened.db, { pid: 1, now: NOW });
      if (token === null) assert.fail('expected a lease token');
      await fn(opened.db, home, token);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

test('spool recovery is idempotent and quarantines a file it cannot read', async () => {
  await withOpened(async (db, home, token) => {
    const paths = oboetePaths(home);
    mkdirSync(paths.spool, { recursive: true });
    const entry = {
      repo: {
        id: 'repo1',
        identity_kind: 'common_dir',
        normalized_identity: '/tmp/oboete-spool',
        display_root: '/tmp/oboete-spool',
      },
      session: {
        id: 'sess-spool',
        repo_id: 'repo1',
        agent: 'claude',
        native_session_id: 'native-spool',
        conversation_id: 'sess-spool',
        started_at: NOW - DAY,
        status: 'active',
      },
      row: {
        id: 'spooled-1',
        repo_id: 'repo1',
        session_id: 'sess-spool',
        // The hook could not read the database, so a spool entry carries no turn (R7).
        turn_id: null,
        agent: 'claude',
        kind: 'prompt',
        content: 'a prompt that was spooled',
        truncated: 0,
        payload_json: null,
        content_hash: 'hash-1',
        sensitivity: 'local_only',
        classification_state: 'done',
        captured_at: NOW - DAY,
        expires_at: NOW + 7 * DAY,
      },
    };
    writeFileSync(join(paths.spool, `${NOW - DAY}-spooled-1.json`), JSON.stringify(entry));
    writeFileSync(join(paths.spool, `${NOW - DAY}-broken.json`), '{ not json');
    // A file that parses but is not an entry is not trusted into the database either (R4).
    writeFileSync(
      join(paths.spool, `${NOW - DAY}-shaped.json`),
      JSON.stringify({ id: 'x', repo_id: 'repo1', event: { kind: 'prompt' } }),
    );

    const first = recoverSpool(db, paths, token, NOW);
    assert.equal(first.inserted, 1);
    assert.equal(first.failed, 2);
    const stored = db.prepare('SELECT via_spool, turn_id FROM raw_events WHERE id = ?').get('spooled-1');
    assert.equal(Number(stored?.via_spool), 1);
    // FR-010: the recovered prompt opens the turn it would have opened on the direct path.
    const turn = db.prepare('SELECT id, ordinal FROM turns WHERE session_id = ?').get('sess-spool');
    assert.equal(Number(turn?.ordinal), 1);
    assert.equal(stored?.turn_id, turn?.id);
    assert.equal(Number(db.prepare('SELECT turn_count FROM sessions WHERE id = ?').get('sess-spool')?.turn_count), 1);
    assert.deepEqual(readdirSync(paths.spoolFailed).sort(), [
      `${NOW - DAY}-broken.json`,
      `${NOW - DAY}-shaped.json`,
    ]);

    // FR-003: the deterministic id makes a second recovery of the same file a no-op.
    writeFileSync(join(paths.spool, `${NOW - DAY}-spooled-1.json`), JSON.stringify(entry));
    const second = recoverSpool(db, paths, token, NOW);
    assert.equal(second.inserted, 0);
    assert.equal(second.skipped, 1);
    assert.equal(Number(db.prepare('SELECT COUNT(*) AS n FROM raw_events').get()?.n), 1);
    assert.equal(
      Number(db.prepare('SELECT COUNT(*) AS n FROM turns').get()?.n),
      1,
      'a repeated recovery opens no second turn',
    );
    assert.deepEqual(readdirSync(paths.spool).filter((name) => name.endsWith('.json')), []);
  });
});

test('a spool entry the database refuses is quarantined and the run continues', async () => {
  await withOpened(async (db, home, token) => {
    const paths = oboetePaths(home);
    mkdirSync(paths.spool, { recursive: true });
    const entry = (id: string, repoId: string): unknown => ({
      repo: {
        id: 'repo-spool',
        identity_kind: 'common_dir',
        normalized_identity: '/tmp/oboete-refused',
        display_root: '/tmp/oboete-refused',
      },
      session: {
        id: `sess-${id}`,
        repo_id: 'repo-spool',
        agent: 'claude',
        native_session_id: `native-${id}`,
        conversation_id: `sess-${id}`,
        started_at: NOW - DAY,
        status: 'active',
      },
      row: {
        // FR-003: the row's repository is the one the hook derived; an unknown one fails the
        // foreign key of raw_events, which must not stop the recovery of the other entries.
        id,
        repo_id: repoId,
        session_id: `sess-${id}`,
        turn_id: null,
        agent: 'claude',
        kind: 'prompt',
        content: `text of ${id}`,
        truncated: 0,
        payload_json: null,
        content_hash: `hash-${id}`,
        sensitivity: 'local_only',
        classification_state: 'done',
        captured_at: NOW - DAY,
        expires_at: NOW + 7 * DAY,
      },
    });
    writeFileSync(join(paths.spool, `${NOW - DAY}-refused.json`), JSON.stringify(entry('refused', 'ghost-repo')));
    writeFileSync(join(paths.spool, `${NOW - DAY}-sound.json`), JSON.stringify(entry('sound', 'repo-spool')));

    const result = recoverSpool(db, paths, token, NOW);

    assert.equal(result.failed, 1);
    assert.equal(result.inserted, 1, 'the entry after the refused one is still recovered');
    assert.deepEqual(readdirSync(paths.spoolFailed), [`${NOW - DAY}-refused.json`]);
    assert.deepEqual(
      db.prepare('SELECT id FROM raw_events').all().map((row) => String(row.id)),
      ['sound'],
    );
    assert.deepEqual(readdirSync(paths.spool).filter((name) => name.endsWith('.json')), []);
  });
});
