import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { PRESET_CATALOG, type PresetName } from '../config.js';
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
    const resetAt = nextUtcMidnight(options.now);
    const presetUsage = db
      .prepare(
        'SELECT exhausted_at, reset_at FROM provider_usage WHERE utc_day = ? AND preset = ?',
      )
      .get(day, options.preset);
    if (
      presetUsage?.exhausted_at !== null &&
      presetUsage?.exhausted_at !== undefined &&
      numberValue(presetUsage.reset_at) > options.now
    ) {
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

    const batch = db
      .prepare(
        `UPDATE observation_batches
         SET provider_attempts = COALESCE(provider_attempts, 0) + 1,
             last_reservation_id = ?, state = 'running'
         WHERE id = ? AND owner_token = ?`,
      )
      .run(reservationId, options.batchId, options.token);
    if (Number(batch.changes) === 0) {
      db.exec('ROLLBACK');
      return { ok: false, reason: 'lease_lost' };
    }
    return { ok: true, reservationId };
  });
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

export function usageEstimate(
  db: DatabaseSync,
  now: number,
): { day: string; calls: number; remaining: number; exhausted: boolean; resetAt: number } {
  const day = utcDay(now);
  const row = db
    .prepare(
      `SELECT
         COALESCE(SUM(COALESCE(calls, 0)), 0) AS calls,
         COALESCE(MAX(CASE WHEN exhausted_at IS NOT NULL AND reset_at > ? THEN 1 ELSE 0 END), 0) AS exhausted
       FROM provider_usage
       WHERE utc_day = ? AND preset IN (${CAPPED_PLACEHOLDERS})`,
    )
    .get(now, day, ...CAPPED_PRESETS);
  const calls = numberValue(row?.calls);
  return {
    day,
    calls,
    remaining: Math.max(0, DAILY_CAP - calls),
    exhausted: numberValue(row?.exhausted) !== 0,
    resetAt: nextUtcMidnight(now),
  };
}
