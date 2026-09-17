import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { isBusyError, openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import {
  assertLease,
  claimLease,
  heartbeat,
  isLeaseFree,
  releaseLease,
  rotateLease,
  transactionImmediate,
} from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';

function spawnLockHolder(dbPath: string, holdMs?: number): ChildProcess {
  const hold =
    holdMs === undefined
      ? 'setInterval(() => {}, 1 << 30);'
      : `setTimeout(() => { try { db.exec('ROLLBACK'); } finally { db.close(); } }, ${holdMs});`;
  return spawn(
    process.execPath,
    [
      '-e',
      `const { DatabaseSync } = require('node:sqlite');
const db = new DatabaseSync(${JSON.stringify(dbPath)}, { timeout: 2000 });
db.exec('BEGIN IMMEDIATE');
process.stdout.write('held\\n');
${hold}`,
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  );
}

function waitForHeld(child: ChildProcess): Promise<void> {
  return new Promise((resolve, reject) => {
    let buf = '';
    let err = '';
    const onData = (chunk: Buffer | string): void => {
      buf += String(chunk);
      if (buf.includes('held')) {
        cleanup();
        resolve();
      }
    };
    const onErr = (chunk: Buffer | string): void => {
      err += String(chunk);
    };
    const onExit = (code: number | null, signal: NodeJS.Signals | null): void => {
      cleanup();
      reject(new Error(`lock holder exited before reporting held (${code ?? signal}): ${err}`));
    };
    const onError = (error: Error): void => {
      cleanup();
      reject(error);
    };
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error(`lock holder did not report held: ${err}`));
    }, 5_000);
    const cleanup = (): void => {
      clearTimeout(timer);
      child.stdout?.off('data', onData);
      child.stderr?.off('data', onErr);
      child.off('exit', onExit);
      child.off('error', onError);
    };
    child.stdout?.on('data', onData);
    child.stderr?.on('data', onErr);
    child.on('exit', onExit);
    child.on('error', onError);
  });
}

async function reap(child: ChildProcess | undefined): Promise<void> {
  if (child === undefined) return;
  await new Promise<void>((resolve) => {
    child.once('exit', () => resolve());
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve();
      return;
    }
    child.kill('SIGKILL');
  });
}

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

test('rotateLease replaces the token and keeps pid and started_at', async () => {
  await withOpened((db) => {
    const now = 1_757_000_000_000;
    const first = claimLease(db, { pid: 7, now });
    if (first === null) assert.fail('expected a lease token');
    const rotated = rotateLease(db, first, now + 50);
    if (rotated === null) assert.fail('expected rotation to keep the lease');
    assert.notEqual(rotated, first);
    const row = leaseColumns(db);
    assert.equal(row?.owner_token, rotated);
    assert.equal(row?.pid, 7);
    assert.equal(row?.started_at, now);
    assert.equal(row?.heartbeat_at, now + 50);
    assert.equal(rotateLease(db, first, now + 80), null);
    assert.equal(leaseColumns(db)?.owner_token, rotated);
  });
});

test('rotateLease propagates SQLITE_BUSY so its caller can retry', async () => {
  await withTempHome((home) => {
    const dbPath = oboetePaths(home).db;
    const first = openDatabase({ path: dbPath, timeoutMs: 1000 });
    const token = claimLease(first.db, { pid: 7, now: 1_757_000_000_000 });
    if (token === null) assert.fail('expected a lease token');
    first.db.exec('BEGIN IMMEDIATE');
    const second = openDatabase({ path: dbPath, timeoutMs: 50 });
    try {
      assert.throws(() => rotateLease(second.db, token, 1_757_000_000_050), isBusyError,
        'rotateLease must propagate SQLITE_BUSY so the caller can retry');
      assert.equal(leaseColumns(first.db)?.owner_token, token);
      first.db.exec('ROLLBACK');
      const rotated = rotateLease(second.db, token, 1_757_000_000_100);
      assert.notEqual(rotated, null, 'rotation must succeed once the writer releases its transaction');
      assert.notEqual(rotated, token);
      assert.equal(leaseColumns(second.db)?.owner_token, rotated);
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

test('transactionImmediate on a lockWaitMs connection throws busy after the wall-clock bound', async () => {
  await withTempHome(async (home) => {
    const dbPath = oboetePaths(home).db;
    const migrated = openDatabase({ path: dbPath, timeoutMs: 2000 });
    migrated.db.close();
    const opened = openDatabase({ path: dbPath, timeoutMs: 150, hook: true, lockWaitMs: 100 });
    let child: ChildProcess | undefined;
    try {
      child = spawnLockHolder(dbPath);
      await waitForHeld(child);
      const started = performance.now();
      assert.throws(() => transactionImmediate(opened.db, () => 1), isBusyError);
      const elapsed = performance.now() - started;
      // Upper bound is the wait plus one scheduler wakeup and a generous runner-load
      // margin; Linux is tight, macOS short sleeps are several times longer without this loop.
      assert.ok(elapsed >= 95, `waited only ${elapsed.toFixed(1)} ms`);
      assert.ok(elapsed < 100 + 150, `waited ${elapsed.toFixed(1)} ms`);
    } finally {
      await reap(child);
      if (opened.db.isOpen) opened.db.close();
    }
  });
});

test('a lockWaitMs hook connection reports busy_timeout 0 and acquires after a child releases', async () => {
  await withTempHome(async (home) => {
    const dbPath = oboetePaths(home).db;
    const migrated = openDatabase({ path: dbPath, timeoutMs: 2000 });
    migrated.db.close();
    const opened = openDatabase({ path: dbPath, timeoutMs: 150, hook: true, lockWaitMs: 300 });
    let child: ChildProcess | undefined;
    try {
      assert.equal(opened.db.prepare('PRAGMA busy_timeout').get()?.timeout, 0);
      child = spawnLockHolder(dbPath, 60);
      await waitForHeld(child);
      assert.equal(transactionImmediate(opened.db, () => 1), 1);
    } finally {
      await reap(child);
      if (opened.db.isOpen) opened.db.close();
    }
  });
});

test('a connection without lockWaitMs keeps the SQLite busy timeout it was opened with', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1234 });
    try {
      assert.equal(opened.db.prepare('PRAGMA busy_timeout').get()?.timeout, 1234);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
});
