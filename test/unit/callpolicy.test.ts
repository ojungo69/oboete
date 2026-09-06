import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { PRESET_CATALOG, type PresetName } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import {
  DAILY_CAP,
  SESSION_END_RESERVE,
  nextUtcMidnight,
  recordExhausted,
  reserveAttempt,
  usageEstimate,
  utcDay,
} from '../../src/observer/reservation.js';
import { oboetePaths } from '../../src/paths.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';

const NOW = Date.UTC(2026, 8, 4, 12, 34, 56);

async function withDatabase(
  fn: (db: DatabaseSync, token: string) => void | Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      const token = claimLease(opened.db, { pid: 123, now: NOW });
      if (token === null) assert.fail('expected a claimed worker lease');
      await fn(opened.db, token);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

function seedBatch(
  db: DatabaseSync,
  id: string,
  token: string,
  trigger: 'ten_turns' | 'session_end' | 'retention' = 'session_end',
): void {
  db.prepare(
    `INSERT INTO observation_batches
       (id, session_id, through_event_id, destination, trigger, state, owner_token, provider_attempts)
     VALUES (?, ?, ?, 'remote_observer', ?, 'pending', ?, 0)`,
  ).run(id, `session-${id}`, `event-${id}`, trigger, token);
}

function seedUsage(
  db: DatabaseSync,
  preset: PresetName,
  calls: number,
  options: { exhaustedAt?: number | null; resetAt?: number } = {},
): void {
  db.prepare(
    `INSERT INTO provider_usage
       (utc_day, preset, calls, neurons_estimate, reset_at, exhausted_at)
     VALUES (?, ?, ?, 0, ?, ?)`,
  ).run(
    utcDay(NOW),
    preset,
    calls,
    options.resetAt ?? nextUtcMidnight(NOW),
    options.exhaustedAt ?? null,
  );
}

function reserve(
  db: DatabaseSync,
  token: string,
  preset: PresetName,
  batchId: string,
  trigger: 'ten_turns' | 'session_end' | 'retention' = 'session_end',
  now = NOW,
) {
  return reserveAttempt(db, {
    preset,
    capped: PRESET_CATALOG[preset].capped,
    trigger,
    batchId,
    token,
    now,
  });
}

test('utcDay and nextUtcMidnight use UTC boundaries', () => {
  const nearMidnight = Date.UTC(2026, 8, 4, 23, 59, 59, 999);
  assert.equal(utcDay(nearMidnight), '2026-09-04');
  assert.equal(nextUtcMidnight(nearMidnight), Date.UTC(2026, 8, 5));
});

test('attempt 150 is allowed and attempt 151 is refused with daily_cap', async () => {
  await withDatabase((db, token) => {
    seedUsage(db, 'workers-ai', DAILY_CAP - 1);
    seedBatch(db, 'batch-boundary', token);

    const allowed = reserve(db, token, 'workers-ai', 'batch-boundary');
    assert.equal(allowed.ok, true);
    assert.equal(
      db.prepare(`SELECT calls FROM provider_usage WHERE utc_day = ? AND preset = 'workers-ai'`).get(utcDay(NOW))
        ?.calls,
      DAILY_CAP,
    );

    assert.deepEqual(reserve(db, token, 'workers-ai', 'batch-boundary'), {
      ok: false,
      reason: 'daily_cap',
    });
  });
});

test('the daily cap sums calls across every capped preset', async () => {
  await withDatabase((db, token) => {
    seedUsage(db, 'workers-ai', 100);
    seedUsage(db, 'nim', 50);
    for (const preset of ['workers-ai', 'nim', 'openrouter', 'gemini'] as const) {
      const batchId = `batch-${preset}`;
      seedBatch(db, batchId, token);
      assert.deepEqual(reserve(db, token, preset, batchId), {
        ok: false,
        reason: 'daily_cap',
      });
    }
  });
});

test('an uncapped agent-cli attempt is recorded but excluded from the 150-call sum', async () => {
  await withDatabase((db, token) => {
    seedUsage(db, 'workers-ai', DAILY_CAP - 1);
    seedUsage(db, 'agent-cli', 999);
    seedBatch(db, 'batch-http', token);
    seedBatch(db, 'batch-cli', token);

    assert.equal(reserve(db, token, 'workers-ai', 'batch-http').ok, true);
    assert.equal(reserve(db, token, 'agent-cli', 'batch-cli').ok, true);
    assert.equal(
      db.prepare(`SELECT calls FROM provider_usage WHERE utc_day = ? AND preset = 'agent-cli'`).get(utcDay(NOW))
        ?.calls,
      1000,
    );
    assert.equal(usageEstimate(db, NOW).calls, DAILY_CAP);
  });
});

test('the final ten calls are reserved for session_end batches', async () => {
  await withDatabase((db, token) => {
    seedUsage(db, 'workers-ai', DAILY_CAP - SESSION_END_RESERVE);
    seedBatch(db, 'batch-ten-turns', token, 'ten_turns');
    seedBatch(db, 'batch-retention', token, 'retention');
    seedBatch(db, 'batch-session-end', token, 'session_end');

    assert.deepEqual(reserve(db, token, 'workers-ai', 'batch-ten-turns', 'ten_turns'), {
      ok: false,
      reason: 'daily_cap',
    });
    assert.deepEqual(reserve(db, token, 'workers-ai', 'batch-retention', 'retention'), {
      ok: false,
      reason: 'daily_cap',
    });
    assert.equal(reserve(db, token, 'workers-ai', 'batch-session-end', 'session_end').ok, true);
  });
});

test('exhausted_at refuses attempts until reset_at has passed', async () => {
  await withDatabase((db, token) => {
    seedUsage(db, 'workers-ai', 1, { exhaustedAt: NOW - 1 });
    seedBatch(db, 'batch-exhausted', token);
    assert.deepEqual(reserve(db, token, 'workers-ai', 'batch-exhausted'), {
      ok: false,
      reason: 'provider_exhausted',
    });

    db.prepare(`UPDATE provider_usage SET reset_at = ? WHERE utc_day = ? AND preset = 'workers-ai'`).run(
      NOW - 1,
      utcDay(NOW),
    );
    assert.equal(reserve(db, token, 'workers-ai', 'batch-exhausted').ok, true);
    const row = db
      .prepare(`SELECT exhausted_at, exhausted_reservation_id FROM provider_usage WHERE utc_day = ? AND preset = 'workers-ai'`)
      .get(utcDay(NOW));
    assert.equal(row?.exhausted_at, null);
    assert.equal(row?.exhausted_reservation_id, null);
  });
});

test('recordExhausted is monotonic, idempotent, and survives a stolen lease', async () => {
  await withDatabase((db, token) => {
    seedBatch(db, 'batch-exhaustion-signal', token);
    const reserved = reserve(db, token, 'workers-ai', 'batch-exhaustion-signal');
    if (!reserved.ok) assert.fail(`reservation failed: ${reserved.reason}`);

    const stolenAt = NOW + 6_001;
    const newToken = claimLease(db, { pid: 999, now: stolenAt });
    if (newToken === null) assert.fail('expected the stale lease to be stolen');
    assert.notEqual(newToken, token);

    recordExhausted(db, {
      preset: 'workers-ai',
      reservationId: reserved.reservationId,
      now: stolenAt,
    });
    recordExhausted(db, {
      preset: 'workers-ai',
      reservationId: 'another-reservation',
      now: stolenAt + 1,
    });
    const row = db
      .prepare(`SELECT exhausted_at, exhausted_reservation_id FROM provider_usage WHERE utc_day = ? AND preset = 'workers-ai'`)
      .get(utcDay(stolenAt));
    assert.equal(row?.exhausted_at, stolenAt);
    assert.equal(row?.exhausted_reservation_id, reserved.reservationId);

    assert.deepEqual(
      reserve(db, token, 'workers-ai', 'batch-exhaustion-signal', 'session_end', stolenAt + 1),
      { ok: false, reason: 'lease_lost' },
    );
  });
});

test('each accepted reservation updates provider_attempts and last_reservation_id', async () => {
  await withDatabase((db, token) => {
    seedBatch(db, 'batch-attempts', token);
    const first = reserve(db, token, 'workers-ai', 'batch-attempts');
    const second = reserve(db, token, 'workers-ai', 'batch-attempts');
    if (!first.ok || !second.ok) assert.fail('expected both reservations');
    assert.notEqual(first.reservationId, second.reservationId);
    const batch = db
      .prepare(`SELECT state, provider_attempts, last_reservation_id FROM observation_batches WHERE id = 'batch-attempts'`)
      .get();
    assert.equal(batch?.state, 'running');
    assert.equal(batch?.provider_attempts, 2);
    assert.equal(batch?.last_reservation_id, second.reservationId);
  });
});

test('usageEstimate reports capped calls, remaining calls, exhaustion, and reset', async () => {
  await withDatabase((db) => {
    seedUsage(db, 'workers-ai', 40, { exhaustedAt: NOW - 1 });
    seedUsage(db, 'nim', 2);
    seedUsage(db, 'agent-cli', 1000);
    assert.deepEqual(usageEstimate(db, NOW), {
      day: '2026-09-04',
      calls: 42,
      remaining: DAILY_CAP - 42,
      exhausted: true,
      resetAt: Date.UTC(2026, 8, 5),
    });
  });
});
