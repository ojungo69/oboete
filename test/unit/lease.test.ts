import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import {
  assertLease,
  claimLease,
  heartbeat,
  isLeaseFree,
  releaseLease,
} from '../../src/worker/lease.js';
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

function leaseColumns(db: DatabaseSync): Record<string, unknown> | undefined {
  return db.prepare('SELECT owner_token, pid, started_at, heartbeat_at FROM worker_lease WHERE id = 1').get();
}

test('fresh database: claimLease returns a token and the row has pid/started_at/heartbeat_at', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    assert.equal(isLeaseFree(db, now), true);
    const token = claimLease(db, { pid: 4242, now });
    if (token === null) assert.fail('expected a lease token');
    const row = leaseColumns(db);
    assert.equal(row?.owner_token, token);
    assert.equal(row?.pid, 4242);
    assert.equal(row?.started_at, now);
    assert.equal(row?.heartbeat_at, now);
    assert.equal(isLeaseFree(db, now), false);
  });
});

test('a second claimLease with a fresh heartbeat returns null', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');
    assert.equal(claimLease(db, { pid: 2, now: now + 1_000 }), null);
    assert.equal(leaseColumns(db)?.owner_token, token);
  });
});

test('after 6001 ms without heartbeat the second claim steals and the first token is fenced out', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const first = claimLease(db, { pid: 1, now });
    if (first === null) assert.fail('expected a lease token');
    const stealAt = now + 6_001;
    const stolen = claimLease(db, { pid: 99, now: stealAt });
    if (stolen === null) assert.fail('expected the stale lease to be stolen');
    assert.notEqual(stolen, first);
    assert.equal(leaseColumns(db)?.owner_token, stolen);
    assert.equal(leaseColumns(db)?.pid, 99);
    assert.equal(leaseColumns(db)?.started_at, stealAt);
    assert.equal(leaseColumns(db)?.heartbeat_at, stealAt);
    assert.equal(heartbeat(db, first, stealAt), false);
    assert.equal(assertLease(db, first, stealAt), false);
    assert.equal(heartbeat(db, stolen, stealAt), true);
  });
});

test('a heartbeat 61 s in the future is stale and is stolen', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const first = claimLease(db, { pid: 1, now });
    if (first === null) assert.fail('expected a lease token');
    const jumped = now - 61_000;
    assert.equal(isLeaseFree(db, jumped), true);
    const stolen = claimLease(db, { pid: 2, now: jumped });
    if (stolen === null) assert.fail('expected the clock-jumped lease to be stolen');
    assert.notEqual(stolen, first);
    assert.equal(leaseColumns(db)?.owner_token, stolen);
  });
});

test('a heartbeat 59 s in the future is not stale', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const first = claimLease(db, { pid: 1, now });
    if (first === null) assert.fail('expected a lease token');
    const almost = now - 59_000;
    assert.equal(isLeaseFree(db, almost), false);
    assert.equal(claimLease(db, { pid: 2, now: almost }), null);
    assert.equal(leaseColumns(db)?.owner_token, first);
  });
});

test("releaseLease returns 'kept', 'released', or 'lost' and updates owner_token", async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const token = claimLease(db, { pid: 1, now });
    if (token === null) assert.fail('expected a lease token');

    assert.equal(releaseLease(db, token, () => false), 'kept');
    assert.equal(leaseColumns(db)?.owner_token, token);
    assert.equal(leaseColumns(db)?.pid, 1);

    assert.equal(releaseLease(db, 'foreign-token', () => true), 'lost');
    assert.equal(leaseColumns(db)?.owner_token, token);

    assert.equal(releaseLease(db, token, () => true), 'released');
    assert.equal(leaseColumns(db)?.owner_token, null);
    assert.equal(leaseColumns(db)?.pid, null);
    assert.equal(isLeaseFree(db, now), true);

    assert.equal(releaseLease(db, token, () => true), 'lost');
  });
});

test('claimLease returns null when another connection holds BEGIN IMMEDIATE', async () => {
  await withTempHome((home) => {
    const dbPath = oboetePaths(home).db;
    const first = openDatabase({ path: dbPath, timeoutMs: 1000 });
    first.db.exec('BEGIN IMMEDIATE');
    const second = openDatabase({ path: dbPath, timeoutMs: 50 });
    try {
      assert.equal(claimLease(second.db, { pid: 1, now: 1_757_000_000_000 }), null);
    } finally {
      try {
        if (first.db.isTransaction) first.db.exec('ROLLBACK');
      } catch {
        // Closing still runs.
      }
      if (second.db.isOpen) second.db.close();
      if (first.db.isOpen) first.db.close();
    }
  });
});
