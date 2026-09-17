import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { DatabaseSync } from 'node:sqlite';
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
import { WALL_CLOCK_IS_MEASURED, withTempHome } from '../helpers/home.js';

function spawnLock(dbPath: string, setupSql: string, releaseDelayMs: number): ChildProcess {
  const child = spawn(
    process.execPath,
    [
      '-e',
      `const { DatabaseSync } = require('node:sqlite');
const db = new DatabaseSync(${JSON.stringify(dbPath)}, { timeout: 2000 });
${setupSql}
process.stdout.write('held\\n');
process.stdin.once('data', () => {
  setTimeout(() => { try { if (db.isTransaction) db.exec('ROLLBACK'); } finally { db.close(); process.exit(0); } }, ${releaseDelayMs});
});`,
    ],
    { stdio: ['pipe', 'pipe', 'pipe'] },
  );
  child.stdin?.on('error', () => {});
  return child;
}

async function waitForHeld(child: ChildProcess): Promise<void> {
  let err = '';
  child.stderr?.on('data', (chunk: Buffer | string) => {
    err += String(chunk);
  });
  const exited = once(child, 'exit').then(([code, signal]) => {
    throw new Error(`lock holder exited before reporting held (${code ?? signal}): ${err}`);
  });
  exited.catch(() => {});
  await Promise.race([once(child.stdout!, 'data', { signal: AbortSignal.timeout(5_000) }), exited]);
}

async function reap(child: ChildProcess | undefined): Promise<void> {
  if (child === undefined) return;
  if (child.exitCode !== null || child.signalCode !== null) return;
  try {
    const exited = once(child, 'exit', { signal: AbortSignal.timeout(2_000) });
    child.kill('SIGKILL');
    await exited;
  } catch {
    // AbortSignal.timeout: child did not exit.
  }
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

function withHeldLock(dbPath: string, fn: () => void): void {
  const holder = new DatabaseSync(dbPath, { timeout: 2000 });
  try {
    holder.exec('PRAGMA journal_mode = WAL');
    holder.exec('BEGIN IMMEDIATE');
    fn();
  } finally {
    try {
      if (holder.isTransaction) holder.exec('ROLLBACK');
    } catch {
      // Closing still runs.
    }
    if (holder.isOpen) holder.close();
  }
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

test('transactionImmediate on a lockBudgetMs connection throws busy after the wall-clock bound', async () => {
  await withTempHome((home) => {
    const dbPath = oboetePaths(home).db;
    const migrated = openDatabase({ path: dbPath, timeoutMs: 2000 });
    migrated.db.close();
    withHeldLock(dbPath, () => {
      const started = performance.now();
      const opened = openDatabase({ path: dbPath, timeoutMs: 0, hook: true, lockBudgetMs: 100 });
      try {
        assert.throws(() => transactionImmediate(opened.db, () => 1), isBusyError);
        const elapsed = performance.now() - started;
        // Upper bound is the wait plus one scheduler wakeup and a generous runner-load
        // margin; Linux is tight, macOS short sleeps are several times longer without this loop.
        assert.ok(elapsed >= 95, `waited only ${elapsed.toFixed(1)} ms`);
        if (WALL_CLOCK_IS_MEASURED) {
          assert.ok(elapsed < 250, `waited ${elapsed.toFixed(1)} ms`);
        }
      } finally {
        if (opened.db.isOpen) opened.db.close();
      }
    });
  });
});

test('idle time after the open spends the lock budget', async () => {
  await withTempHome((home) => {
    const dbPath = oboetePaths(home).db;
    const migrated = openDatabase({ path: dbPath, timeoutMs: 2000 });
    migrated.db.close();
    const sleep = new Int32Array(new SharedArrayBuffer(4));
    const opened = performance.now();
    const { db } = openDatabase({ path: dbPath, timeoutMs: 0, hook: true, lockBudgetMs: 200 });
    try {
      Atomics.wait(sleep, 0, 0, 120);
      withHeldLock(dbPath, () => {
        assert.throws(() => transactionImmediate(db, () => 1), isBusyError);
        const elapsed = performance.now() - opened;
        assert.ok(elapsed >= 190, `waited only ${elapsed.toFixed(1)} ms`);
        if (WALL_CLOCK_IS_MEASURED) {
          assert.ok(elapsed < 250, `waited ${elapsed.toFixed(1)} ms`);
        }
      });
    } finally {
      if (db.isOpen) db.close();
    }
  });
});

test('a lockBudgetMs hook connection reports busy_timeout 0 and acquires after a child releases', async () => {
  await withTempHome(async (home) => {
    const dbPath = oboetePaths(home).db;
    const migrated = openDatabase({ path: dbPath, timeoutMs: 2000 });
    migrated.db.close();
    let child: ChildProcess | undefined;
    let opened: ReturnType<typeof openDatabase> | undefined;
    try {
      child = spawnLock(dbPath, "db.exec('BEGIN IMMEDIATE');", 10);
      await waitForHeld(child);
      opened = openDatabase({ path: dbPath, timeoutMs: 0, hook: true, lockBudgetMs: 1000 });
      assert.equal(opened.db.prepare('PRAGMA busy_timeout').get()?.timeout, 0);
      const probe = new DatabaseSync(dbPath, { timeout: 0 });
      try {
        assert.throws(() => probe.exec('BEGIN IMMEDIATE'), isBusyError);
      } finally {
        if (probe.isOpen) probe.close();
      }
      child.stdin?.write('go\n');
      assert.equal(transactionImmediate(opened.db, () => 1), 1);
    } finally {
      await reap(child);
      if (opened?.db.isOpen) opened.db.close();
    }
  });
});

test('a lockBudgetMs hook open waits out an exclusive lock that blocks the open step', async () => {
  await withTempHome(async (home) => {
    const dbPath = oboetePaths(home).db;
    const migrated = openDatabase({ path: dbPath, timeoutMs: 2000 });
    migrated.db.close();
    let child: ChildProcess | undefined;
    let opened: ReturnType<typeof openDatabase> | undefined;
    try {
      child = spawnLock(
        dbPath,
        `db.exec('PRAGMA journal_mode = WAL');
db.exec('PRAGMA locking_mode = EXCLUSIVE');
db.exec('BEGIN IMMEDIATE');
db.exec('CREATE TABLE IF NOT EXISTS lock_probe(x INTEGER)');
db.exec('INSERT INTO lock_probe VALUES (1)');
db.exec('COMMIT');`,
        10,
      );
      await waitForHeld(child);
      const probe = new DatabaseSync(dbPath, { timeout: 0 });
      try {
        assert.throws(() => probe.exec('PRAGMA journal_mode = WAL'), isBusyError);
      } finally {
        if (probe.isOpen) probe.close();
      }
      child.stdin?.write('go\n');
      opened = openDatabase({ path: dbPath, timeoutMs: 0, hook: true, lockBudgetMs: 1000 });
      assert.equal(opened.db.isOpen, true);
    } finally {
      await reap(child);
      if (opened?.db.isOpen) opened.db.close();
    }
  });
});

test('transactionImmediate on a lockBudgetMs connection caps each wait at 150 ms', async () => {
  await withTempHome((home) => {
    const dbPath = oboetePaths(home).db;
    const migrated = openDatabase({ path: dbPath, timeoutMs: 2000 });
    migrated.db.close();
    withHeldLock(dbPath, () => {
      const opened = openDatabase({ path: dbPath, timeoutMs: 0, hook: true, lockBudgetMs: 1000 });
      try {
        const started = performance.now();
        assert.throws(() => transactionImmediate(opened.db, () => 1), isBusyError);
        const elapsed = performance.now() - started;
        assert.ok(elapsed >= 140, `waited only ${elapsed.toFixed(1)} ms`);
        if (WALL_CLOCK_IS_MEASURED) {
          assert.ok(elapsed < 150 + 100, `waited ${elapsed.toFixed(1)} ms`);
        }
      } finally {
        if (opened.db.isOpen) opened.db.close();
      }
    });
  });
});

test('a connection without lockBudgetMs keeps the SQLite busy timeout it was opened with', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1234 });
    try {
      assert.equal(opened.db.prepare('PRAGMA busy_timeout').get()?.timeout, 1234);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
});

test('openDatabase throws when lockBudgetMs is given without hook: true', () => {
  assert.throws(
    () => openDatabase({ path: 'unused.db', timeoutMs: 0, lockBudgetMs: 100 }),
    { message: /lockBudgetMs requires hook: true/ },
  );
});
