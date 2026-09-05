import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { isBusyError } from '../db/open.js';

export const STALE_AFTER_MS = 6_000;
export const FUTURE_SKEW_MS = 60_000;

function stale(heartbeatAt: unknown, now: number): boolean {
  const ts =
    typeof heartbeatAt === 'number'
      ? heartbeatAt
      : typeof heartbeatAt === 'bigint'
        ? Number(heartbeatAt)
        : Number.NaN;
  return !Number.isFinite(ts) || now - ts > STALE_AFTER_MS || ts - now > FUTURE_SKEW_MS;
}

function leaseRow(db: DatabaseSync): Record<string, unknown> | undefined {
  return db.prepare('SELECT owner_token, heartbeat_at FROM worker_lease WHERE id = 1').get();
}

export function transactionImmediate<T>(db: DatabaseSync, fn: () => T): T {
  db.exec('BEGIN IMMEDIATE');
  try {
    const result = fn();
    if (db.isTransaction) db.exec('COMMIT');
    return result;
  } catch (error) {
    try {
      if (db.isTransaction) db.exec('ROLLBACK');
    } catch {
      // Prefer the original error.
    }
    throw error;
  }
}

/** Read-only; hooks spawn `oboete observe` when this is true (R6). */
export function isLeaseFree(db: DatabaseSync, now: number): boolean {
  const row = leaseRow(db);
  return row === undefined || row.owner_token === null || stale(row.heartbeat_at, now);
}

export function claimLease(db: DatabaseSync, { pid, now }: { pid: number; now: number }): string | null {
  try {
    return transactionImmediate(db, () => {
      const row = leaseRow(db);
      if (row !== undefined && row.owner_token !== null && !stale(row.heartbeat_at, now)) {
        return null;
      }
      const token = randomUUID();
      db.prepare(
        'UPDATE worker_lease SET owner_token = ?, pid = ?, started_at = ?, heartbeat_at = ? WHERE id = 1',
      ).run(token, pid, now, now);
      return token;
    });
  } catch (error) {
    // R6 reviewer change: SQLITE_BUSY on claim (another process holds the write lock).
    if (isBusyError(error)) return null;
    throw error;
  }
}

/** Every fenced transaction is `BEGIN IMMEDIATE; assertLease; <writes>; COMMIT`. */
export function assertLease(db: DatabaseSync, token: string, now: number): boolean {
  const result = db
    .prepare('UPDATE worker_lease SET heartbeat_at = ? WHERE id = 1 AND owner_token = ?')
    .run(now, token);
  return Number(result.changes) !== 0;
}

export function heartbeat(db: DatabaseSync, token: string, now: number): boolean {
  return assertLease(db, token, now);
}

export function releaseLease(
  db: DatabaseSync,
  token: string,
  queueIsEmpty: () => boolean,
): 'released' | 'kept' | 'lost' {
  return transactionImmediate(db, () => {
    const row = leaseRow(db);
    if (row?.owner_token !== token) return 'lost';
    if (!queueIsEmpty()) return 'kept';
    db.prepare('UPDATE worker_lease SET owner_token = NULL, pid = NULL WHERE id = 1 AND owner_token = ?').run(
      token,
    );
    return 'released';
  });
}
