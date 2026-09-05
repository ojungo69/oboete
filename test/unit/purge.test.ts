import assert from 'node:assert/strict';
import { existsSync, mkdirSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { claimLease } from '../../src/worker/lease.js';
import {
  checkpoint,
  cleanupPiAck,
  purgeExpiredEvents,
  runtimeStateGet,
  runtimeStateSet,
} from '../../src/worker/purge.js';
import { withTempHome } from '../helpers/home.js';

async function withOpened(fn: (db: DatabaseSync, home: string) => void | Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      await fn(opened.db, home);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

function seedGraph(db: DatabaseSync): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, created_at, last_seen_at)
     VALUES ('repo1', 'common_dir', '/tmp/oboete-purge', 1, 1)`,
  ).run();
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status, turn_count)
     VALUES ('sess1', 'repo1', 'claude', 'native-1', 'sess1', 'active', 0)`,
  ).run();
}

function insertBatch(db: DatabaseSync, id: string, state: string): void {
  db.prepare(
    `INSERT INTO observation_batches (id, session_id, through_event_id, destination, state)
     VALUES (?, 'sess1', ?, 'fallback', ?)`,
  ).run(id, id, state);
}

function insertEvent(
  db: DatabaseSync,
  row: {
    id: string;
    expiresAt: number;
    batchId: string | null;
    sensitivity?: string;
    classification?: string;
    kind?: string;
    content?: string | null;
    payload?: unknown;
  },
): void {
  db.prepare(
    `INSERT INTO raw_events
       (id, repo_id, session_id, kind, content, payload_json, sensitivity, classification_state,
        captured_at, expires_at, batch_id)
     VALUES (?, 'repo1', 'sess1', ?, ?, ?, ?, ?, 1, ?, ?)`,
  ).run(
    row.id,
    row.kind ?? 'prompt',
    row.content === undefined ? 'a captured prompt' : row.content,
    row.payload === undefined ? null : JSON.stringify(row.payload),
    row.sensitivity ?? 'local_only',
    row.classification ?? 'done',
    row.expiresAt,
    row.batchId,
  );
}

function eventIds(db: DatabaseSync): string[] {
  return db
    .prepare('SELECT id FROM raw_events ORDER BY id')
    .all()
    .map((row) => String(row.id));
}

test('expired row in an applied batch is deleted; pending and non-expired applied survive', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    seedGraph(db);
    insertBatch(db, 'b-applied', 'applied');
    insertBatch(db, 'b-pending', 'pending');
    insertEvent(db, { id: 'e-applied-expired', expiresAt: now, batchId: 'b-applied' });
    insertEvent(db, { id: 'e-pending-expired', expiresAt: now, batchId: 'b-pending' });
    insertEvent(db, { id: 'e-applied-fresh', expiresAt: now + 1, batchId: 'b-applied' });

    const result = purgeExpiredEvents(db, token, now);
    assert.equal(result.leaseLost, false);
    assert.equal(result.deleted, 1);
    assert.deepEqual(eventIds(db), ['e-applied-fresh', 'e-pending-expired']);
  });
});

test('expired unbatched failed and secret rows are deleted; expired unbatched local_only survives', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    seedGraph(db);
    insertEvent(db, {
      id: 'e-failed',
      expiresAt: now,
      batchId: null,
      classification: 'failed',
    });
    insertEvent(db, { id: 'e-secret', expiresAt: now, batchId: null, sensitivity: 'secret' });
    insertEvent(db, { id: 'e-local', expiresAt: now, batchId: null, sensitivity: 'local_only' });

    const result = purgeExpiredEvents(db, token, now);
    assert.equal(result.leaseLost, false);
    assert.equal(result.deleted, 2);
    assert.deepEqual(eventIds(db), ['e-local']);
  });
});

test('expired unbatched rows no summarizer can use are purged and the ones it can use survive', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    seedGraph(db);
    // FR-008: batches.ts never picks these up, so purge is the only thing that bounds them.
    insertEvent(db, { id: 'e-session-start', expiresAt: now, batchId: null, kind: 'session_start' });
    insertEvent(db, { id: 'e-turn-end', expiresAt: now, batchId: null, kind: 'turn_end' });
    insertEvent(db, {
      id: 'e-empty-summary',
      expiresAt: now,
      batchId: null,
      kind: 'compaction_summary',
      content: null,
    });
    // A tool call keeps its input in payload_json, so an empty content is not an empty row.
    insertEvent(db, {
      id: 'e-tool-call',
      expiresAt: now,
      batchId: null,
      kind: 'tool_call',
      content: '',
      payload: { tool_name: 'read', input: { paths: ['src/a.ts'] } },
    });
    insertEvent(db, { id: 'e-blank', expiresAt: now, batchId: null, content: ' \n\t ' });
    // `trim()` calls these blank too, so batches.ts never takes the row and only purge bounds it.
    insertEvent(db, { id: 'e-blank-wide', expiresAt: now, batchId: null, content: '\u3000\u00a0' });
    insertEvent(db, { id: 'e-prompt', expiresAt: now, batchId: null });

    const result = purgeExpiredEvents(db, token, now);
    assert.equal(result.leaseLost, false);
    assert.equal(result.deleted, 5);
    assert.deepEqual(eventIds(db), ['e-prompt', 'e-tool-call']);
  });
});

test('with limit 2 and 5 deletable rows the function reports deleted 5', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    seedGraph(db);
    insertBatch(db, 'b-applied', 'applied');
    for (const id of ['e1', 'e2', 'e3', 'e4', 'e5']) {
      insertEvent(db, { id, expiresAt: now, batchId: 'b-applied' });
    }

    const result = purgeExpiredEvents(db, token, now, { limit: 2 });
    assert.equal(result.leaseLost, false);
    assert.equal(result.deleted, 5);
    assert.deepEqual(eventIds(db), []);
  });
});

test('with a foreign token nothing is deleted and leaseLost is true', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    seedGraph(db);
    insertBatch(db, 'b-applied', 'applied');
    insertEvent(db, { id: 'e-applied-expired', expiresAt: now, batchId: 'b-applied' });

    const result = purgeExpiredEvents(db, 'foreign-token', now);
    assert.equal(result.deleted, 0);
    assert.equal(result.leaseLost, true);
    assert.deepEqual(eventIds(db), ['e-applied-expired']);
    assert.equal(
      db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token,
      token,
    );
  });
});

test('cleanupPiAck deletes a .done file (folded)', async () => {
  await withOpened((db, home) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    const piAck = oboetePaths(home).piAck;
    mkdirSync(piAck, { recursive: true });
    const done = join(piAck, 'inv-done.done');
    writeFileSync(done, 'must-not-be-read');

    const result = cleanupPiAck(db, token, piAck, now);
    assert.equal(result.folded, 1);
    assert.equal(result.hangs, 0);
    assert.equal(result.removed, 0);
    assert.equal(existsSync(done), false);
  });
});

test('a .started file 31 s old yields one diagnostics row with count 1 and survives; a second run increments count to 2', async () => {
  await withOpened((db, home) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    const piAck = oboetePaths(home).piAck;
    mkdirSync(piAck, { recursive: true });
    const started = join(piAck, 'inv-hang.started');
    writeFileSync(started, 'must-not-be-read');
    const aged = new Date(now - 31_000);
    utimesSync(started, aged, aged);

    const first = cleanupPiAck(db, token, piAck, now);
    assert.equal(first.hangs, 1);
    assert.equal(first.removed, 0);
    assert.equal(existsSync(started), true);

    const row = db
      .prepare(
        'SELECT kind, severity, agent, message_code, count, details_json, first_seen_at, last_seen_at FROM diagnostics',
      )
      .get();
    assert.equal(row?.kind, 'pi_child_hang');
    assert.equal(row?.severity, 'warn');
    assert.equal(row?.agent, 'pi');
    assert.equal(row?.message_code, 'pi_child_hang');
    assert.equal(row?.count, 1);
    assert.equal(row?.first_seen_at, now);
    assert.equal(row?.last_seen_at, now);
    assert.deepEqual(JSON.parse(String(row?.details_json)), { invocations: ['inv-hang'] });

    const second = cleanupPiAck(db, token, piAck, now + 1);
    assert.equal(second.hangs, 1);
    assert.equal(existsSync(started), true);
    const again = db.prepare('SELECT count, details_json, first_seen_at, last_seen_at FROM diagnostics').all();
    assert.equal(again.length, 1);
    assert.equal(again[0]?.count, 2);
    assert.equal(again[0]?.first_seen_at, now);
    assert.equal(again[0]?.last_seen_at, now + 1);
    assert.deepEqual(JSON.parse(String(again[0]?.details_json)), { invocations: ['inv-hang'] });
  });
});

test('a .started file 25 h old is deleted', async () => {
  await withOpened((db, home) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    const piAck = oboetePaths(home).piAck;
    mkdirSync(piAck, { recursive: true });
    const started = join(piAck, 'inv-old.started');
    writeFileSync(started, 'must-not-be-read');
    const aged = new Date(now - 25 * 60 * 60 * 1000);
    utimesSync(started, aged, aged);

    const result = cleanupPiAck(db, token, piAck, now);
    assert.equal(result.hangs, 1);
    assert.equal(result.removed, 1);
    assert.equal(existsSync(started), false);
  });
});

test('cleanupPiAck returns zeros when the directory is missing', async () => {
  await withOpened((db, home) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    const missing = join(home, 'no-pi-ack');
    assert.deepEqual(cleanupPiAck(db, token, missing, now), { folded: 0, hangs: 0, removed: 0 });
  });
});

test('checkpoint runs without error in both modes and PASSIVE after TRUNCATE reports 0 pages', async () => {
  await withOpened((db) => {
    db.prepare('INSERT INTO runtime_state (key, value_json, updated_at) VALUES (?, ?, ?)').run(
      'wal-seed',
      '1',
      1,
    );
    checkpoint(db, 'PASSIVE');
    checkpoint(db, 'TRUNCATE');
    const row = db.prepare('PRAGMA wal_checkpoint(PASSIVE)').get();
    assert.equal(row?.log, 0);
    assert.equal(row?.checkpointed, 0);
  });
});

test('runtimeState get/set round trip and overwrite', async () => {
  await withOpened((db) => {
    assert.equal(runtimeStateGet(db, 'last_purge'), undefined);
    runtimeStateSet(db, 'last_purge', '{"at":1}', 10);
    assert.equal(runtimeStateGet(db, 'last_purge'), '{"at":1}');
    runtimeStateSet(db, 'last_purge', '{"at":2}', 20);
    assert.equal(runtimeStateGet(db, 'last_purge'), '{"at":2}');
    assert.equal(
      db.prepare('SELECT updated_at FROM runtime_state WHERE key = ?').get('last_purge')?.updated_at,
      20,
    );
    runtimeStateSet(db, 'last_checkpoint', '"ok"', 30);
    assert.equal(runtimeStateGet(db, 'last_checkpoint'), '"ok"');
    assert.equal(runtimeStateGet(db, 'last_purge'), '{"at":2}');
  });
});
