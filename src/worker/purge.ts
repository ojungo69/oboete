import { randomUUID } from 'node:crypto';
import { existsSync, readdirSync, statSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';

import { BLANK_CHARACTERS_SQL, SUMMARIZABLE_KINDS_SQL } from './batches.js';
import { assertLease, transactionImmediate } from './lease.js';

export const PI_HANG_AFTER_MS = 30_000;
const PI_ACK_REMOVE_AFTER_MS = 24 * 60 * 60 * 1000;

// Accepted sources expire only after successful processing. Pending, waiting and legacy-unknown
// material survives any capture-time expiry. Referenced source material is retained as evidence.
// Failed/secret/lifecycle/empty metadata keeps its existing expiry; a tool call may carry input
// outside `content`, so an empty-content tool call is left alone.
// ponytail: a tool call with neither content nor input is the one shape that still accumulates;
// read payload_json here if one ever turns up.
const DELETE_EXPIRED = `DELETE FROM raw_events WHERE id IN (
  SELECT r.id FROM raw_events r
  WHERE r.expires_at <= ?
    AND ((r.processing_state = 'processed' AND r.processed_at IS NOT NULL
      AND NOT EXISTS (
        SELECT 1 FROM memory_sources ms JOIN memories m ON m.id = ms.memory_id
        WHERE ms.raw_event_id = r.id AND ms.context_only = 0 AND m.deleted_at IS NULL
          AND m.type <> 'session_summary' AND m.degraded_reason IS NULL
          AND NOT EXISTS (SELECT 1 FROM memory_sources evidence
            WHERE evidence.memory_id = m.id AND evidence.raw_event_id = r.id AND evidence.context_only = 0 AND evidence.evidence IS NOT NULL)
      )) OR r.classification_state = 'failed' OR r.sensitivity = 'secret'
      OR (r.batch_id IS NULL AND (
        r.kind NOT IN (${SUMMARIZABLE_KINDS_SQL})
        OR (r.kind <> 'tool_call' AND TRIM(COALESCE(r.content, ''), ${BLANK_CHARACTERS_SQL}) = '')
      )))
  LIMIT ?
)`;

export function purgeExpiredEvents(
  db: DatabaseSync,
  token: string,
  now: number,
  { limit = 500 }: { limit?: number } = {},
): { deleted: number; leaseLost: boolean } {
  if (limit < 1) return { deleted: 0, leaseLost: false };
  let deleted = 0;
  let n = limit;
  while (n >= limit) {
    const step = transactionImmediate(db, () => {
      if (!assertLease(db, token, now)) {
        db.exec('ROLLBACK');
        return { lost: true, n: 0 };
      }
      return { lost: false, n: Number(db.prepare(DELETE_EXPIRED).run(now, limit).changes) };
    });
    if (step.lost) return { deleted, leaseLost: true };
    n = step.n;
    deleted += n;
  }
  return { deleted, leaseLost: false };
}

function isEnoent(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT';
}

function readInvocations(detailsJson: unknown): string[] {
  if (typeof detailsJson !== 'string') return [];
  try {
    const parsed: unknown = JSON.parse(detailsJson);
    if (typeof parsed !== 'object' || parsed === null || !('invocations' in parsed)) return [];
    const { invocations } = parsed as { invocations: unknown };
    return Array.isArray(invocations)
      ? invocations.filter((item): item is string => typeof item === 'string')
      : [];
  } catch {
    return [];
  }
}

function recordHangs(db: DatabaseSync, token: string, invocations: string[], now: number): boolean {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return false;
    }
    const row = db
      .prepare(
        `SELECT id, count, details_json FROM diagnostics
         WHERE kind = 'pi_child_hang' AND message_code = 'pi_child_hang' AND agent = 'pi' AND cleared_at IS NULL`,
      )
      .get();
    const listed = row === undefined ? [] : readInvocations(row.details_json);
    for (const invocation of invocations) {
      if (!listed.includes(invocation)) listed.push(invocation);
    }
    const details = JSON.stringify({ invocations: listed });
    if (row === undefined) {
      db.prepare(
        `INSERT INTO diagnostics
           (id, kind, severity, agent, message_code, details_json, count, first_seen_at, last_seen_at)
         VALUES (?, 'pi_child_hang', 'warn', 'pi', 'pi_child_hang', ?, ?, ?, ?)`,
      ).run(randomUUID(), details, invocations.length, now, now);
      return true;
    }
    db.prepare('UPDATE diagnostics SET details_json = ?, count = ?, last_seen_at = ? WHERE id = ?').run(
      details,
      Number(row.count) + invocations.length,
      now,
      row.id,
    );
    return true;
  });
}

function collectPiAckFile(
  piAckDir: string,
  name: string,
  now: number,
  hangInvocations: string[],
  removePaths: string[],
): number {
  const path = join(piAckDir, name);
  try {
    if (name.endsWith('.done')) {
      unlinkSync(path);
      return 1;
    }
    if (!name.endsWith('.started')) return 0;
    const age = now - statSync(path).mtimeMs;
    if (age > PI_HANG_AFTER_MS) hangInvocations.push(name.slice(0, -'.started'.length));
    if (age > PI_ACK_REMOVE_AFTER_MS) removePaths.push(path);
  } catch (error) {
    if (isEnoent(error)) return 0;
    throw error;
  }
  return 0;
}

function removePiAckPath(path: string): number {
  try {
    unlinkSync(path);
    return 1;
  } catch (error) {
    if (!isEnoent(error)) throw error;
  }
  return 0;
}

export function cleanupPiAck(
  db: DatabaseSync,
  token: string,
  piAckDir: string,
  now: number,
): { folded: number; hangs: number; removed: number } {
  if (!existsSync(piAckDir)) return { folded: 0, hangs: 0, removed: 0 };

  let folded = 0;
  let hangs = 0;
  let removed = 0;
  const hangInvocations: string[] = [];
  const removePaths: string[] = [];

  for (const name of readdirSync(piAckDir)) {
    folded += collectPiAckFile(piAckDir, name, now, hangInvocations, removePaths);
  }

  if (hangInvocations.length > 0 && recordHangs(db, token, hangInvocations, now)) {
    hangs = hangInvocations.length;
    for (const path of removePaths) {
      removed += removePiAckPath(path);
    }
  }
  return { folded, hangs, removed };
}

export function checkpoint(db: DatabaseSync, mode: 'PASSIVE' | 'TRUNCATE'): void {
  db.exec(mode === 'TRUNCATE' ? 'PRAGMA wal_checkpoint(TRUNCATE)' : 'PRAGMA wal_checkpoint(PASSIVE)');
}

export function runtimeStateSet(db: DatabaseSync, key: string, valueJson: string, now: number): void {
  db.prepare(
    `INSERT INTO runtime_state (key, value_json, updated_at) VALUES (?, ?, ?)
     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`,
  ).run(key, valueJson, now);
}

export function runtimeStateGet(db: DatabaseSync, key: string): string | undefined {
  const row = db.prepare('SELECT value_json FROM runtime_state WHERE key = ?').get(key);
  return typeof row?.value_json === 'string' ? row.value_json : undefined;
}
