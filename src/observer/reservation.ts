import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { PRESET_CATALOG, type PresetName } from '../config.js';
import { prepared } from '../db/statements.js';
import { assertLease, transactionImmediate } from '../worker/lease.js';

export const DAILY_CAP = 150;
export const SESSION_END_RESERVE = 10;

const CAPPED_PRESETS = (Object.entries(PRESET_CATALOG) as Array<
  [PresetName, (typeof PRESET_CATALOG)[PresetName]]
>)
  .filter(([, preset]) => preset.capped)
  .map(([name]) => name);
const CAPPED_PLACEHOLDERS = CAPPED_PRESETS.map(() => '?').join(', ');

export function utcDay(now: number): string {
  return new Date(now).toISOString().slice(0, 10);
}

export function nextUtcMidnight(now: number): number {
  const date = new Date(now);
  return Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate() + 1);
}

function numberValue(value: unknown): number {
  if (typeof value === 'number') return value;
  if (typeof value === 'bigint') return Number(value);
  return 0;
}

/**
 * Today's exhaustion stamp for one preset, or null when it may still be reserved. `exhausted_at` is
 * per-preset: one preset's exhaustion says nothing about another's, and nothing about the shared
 * daily call count, which `usageEstimate` reports.
 */
export function presetExhaustedAt(db: DatabaseSync, preset: PresetName, now: number): number | null {
  // Cached: a chain asks this once per target per batch, and every fallback item asks it again
  // (src/db/statements.ts: a statement prepared per call is native memory until a collection).
  const row = prepared(db, 'SELECT exhausted_at, reset_at FROM provider_usage WHERE utc_day = ? AND preset = ?')
    .get(utcDay(now), preset);
  const exhaustedAt = row?.exhausted_at;
  // Only a number is a stamp: `numberValue` would read anything else as the epoch and report a row
  // that was never stamped as exhausted since 1970. Which way an unusable value should fall is moot
  // rather than chosen — `provider_usage` is a `STRICT` table whose `exhausted_at` is `INTEGER`
  // (`src/db/migrations/0003_operations.sql`), and `recordExhausted` is its only writer.
  if (typeof exhaustedAt !== 'number' && typeof exhaustedAt !== 'bigint') return null;
  return numberValue(row?.reset_at) > now ? numberValue(exhaustedAt) : null;
}

function cappedCalls(db: DatabaseSync, day: string): number {
  const row = db
    .prepare(
      `SELECT COALESCE(SUM(COALESCE(calls, 0)), 0) AS calls
       FROM provider_usage
       WHERE utc_day = ? AND preset IN (${CAPPED_PLACEHOLDERS})`,
    )
    .get(day, ...CAPPED_PRESETS);
  return numberValue(row?.calls);
}

/**
 * One target's attempt on one batch. `claimed_at` is restamped here, not only at creation, because
 * `reclaimStale` measures its 120 s grace from that column: a batch created minutes before the
 * attempt would otherwise be reclaimable — and its provider call repeated — the instant it starts
 * running (`src/worker/batches.ts` RECLAIM_AFTER_MS).
 */
export function reserveAttempt(
  db: DatabaseSync,
  options: {
    preset: PresetName;
    capped: boolean;
    trigger: 'ten_turns' | 'session_end' | 'retention';
    batchId: string;
    token: string;
    now: number;
  },
):
  | { ok: true; reservationId: string }
  | { ok: false; reason: 'daily_cap' | 'provider_exhausted' | 'lease_lost' } {
  return transactionImmediate(db, () => {
    if (!assertLease(db, options.token, options.now)) {
      db.exec('ROLLBACK');
      return { ok: false, reason: 'lease_lost' };
    }

    const day = utcDay(options.now);
    if (presetExhaustedAt(db, options.preset, options.now) !== null) {
      return { ok: false, reason: 'provider_exhausted' };
    }

    if (options.capped) {
      const calls = cappedCalls(db, day);
      if (
        calls >= DAILY_CAP ||
        (options.trigger !== 'session_end' && calls >= DAILY_CAP - SESSION_END_RESERVE)
      ) {
        return { ok: false, reason: 'daily_cap' };
      }
    }

    const reservationId = randomUUID();
    recordProviderAttempt(db, { preset: options.preset, now: options.now });

    const batch = db
      .prepare(
        `UPDATE observation_batches
         SET provider_attempts = COALESCE(provider_attempts, 0) + 1,
             last_reservation_id = ?, state = 'running', claimed_at = ?
         WHERE id = ? AND owner_token = ?`,
      )
      .run(reservationId, options.now, options.batchId, options.token);
    if (Number(batch.changes) === 0) {
      db.exec('ROLLBACK');
      return { ok: false, reason: 'lease_lost' };
    }
    return { ok: true, reservationId };
  });
}

export function recordProviderAttempt(
  db: DatabaseSync,
  options: { preset: PresetName; now: number },
): void {
  const day = utcDay(options.now);
  const resetAt = nextUtcMidnight(options.now);
  db.prepare(
    `INSERT INTO provider_usage
       (utc_day, preset, calls, neurons_estimate, reset_at)
     VALUES (?, ?, 1, 0, ?)
     ON CONFLICT(utc_day, preset) DO UPDATE SET
       calls = COALESCE(provider_usage.calls, 0) + 1,
       reset_at = excluded.reset_at,
       exhausted_at = CASE
         WHEN COALESCE(provider_usage.reset_at, 0) <= ? THEN NULL
         ELSE provider_usage.exhausted_at
       END,
       exhausted_reservation_id = CASE
         WHEN COALESCE(provider_usage.reset_at, 0) <= ? THEN NULL
         ELSE provider_usage.exhausted_reservation_id
       END`,
  ).run(day, options.preset, resetAt, options.now, options.now);
}

export function recordExhausted(
  db: DatabaseSync,
  options: { preset: PresetName; reservationId: string; now: number },
): void {
  transactionImmediate(db, () => {
    db.prepare(
      `UPDATE provider_usage
       SET exhausted_at = COALESCE(exhausted_at, ?),
           exhausted_reservation_id = COALESCE(exhausted_reservation_id, ?)
       WHERE utc_day = ? AND preset = ?`,
    ).run(options.now, options.reservationId, utcDay(options.now), options.preset);
  });
}

/**
 * The allowance every capped preset shares. Per-preset exhaustion is `presetExhaustedAt`, not a
 * field here: summing it over the capped presets would answer for a preset that reported nothing.
 */
export function usageEstimate(
  db: DatabaseSync,
  now: number,
): { day: string; calls: number; remaining: number; resetAt: number } {
  const day = utcDay(now);
  const calls = cappedCalls(db, day);
  return {
    day,
    calls,
    remaining: Math.max(0, DAILY_CAP - calls),
    resetAt: nextUtcMidnight(now),
  };
}
