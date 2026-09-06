import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { memoryScope } from '../../src/db/queries.js';
import { CHANNEL_CAPS } from '../../src/injection/budget.js';
import { whyReport } from '../../src/injection/ledger.js';
import {
  DEGRADED_SENTENCES,
  buildPromptPack,
  buildSessionStartPack,
  type PromptPackInput,
  type SessionStartInput,
} from '../../src/injection/pack.js';
import { PACK_HEADER } from '../../src/injection/recognize.js';
import { sessionSummary } from '../../src/observer/classify.js';
import {
  DAILY_CAP,
  nextUtcMidnight,
  recordExhausted,
  usageEstimate,
  utcDay,
} from '../../src/observer/reservation.js';
import { oboetePaths } from '../../src/paths.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';
import {
  NOW as OBSERVE_NOW,
  captureEndedSession,
  cleanEnv,
  runObserveForFixture,
  withFixture,
  writeConfig,
} from '../helpers/observe.js';

const NOW = 1_700_000_000_000;
const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const REPO = 'r1';
const scope = (db: DatabaseSync) => memoryScope(db, { repoId: REPO, destination: 'injection' });
const IDENTITY = 'example.test/one';
const SUMMARY_REPO = 'a1b2c3d4e5f60718';

type PackInput = SessionStartInput & PromptPackInput;

function packInput(overrides: Partial<PackInput> = {}): PackInput {
  return {
    agent: 'claude',
    repoId: REPO,
    repoIdentityDisplay: IDENTITY,
    sessionId: 's_now',
    conversationId: 'c1',
    turnId: null,
    epoch: 0,
    model: 'claude-opus-5[1m]',
    channelCap: CHANNEL_CAPS.claude,
    contextFraction: 0.05,
    channel: 'claude:SessionStart',
    now: NOW,
    detect: () => false,
    directives: [],
    repoRoot: '/nonexistent-repository-root',
    waitForSummary: () => 'none' as const,
    prompt: '',
    ...overrides,
  };
}

function insertRepo(db: DatabaseSync, id = REPO, identity = IDENTITY): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, 'remote', ?, '/tmp/one', 1, 1)`,
  ).run(id, identity);
}

function insertMemory(
  db: DatabaseSync,
  seed: {
    id: string;
    title: string;
    body: string;
    type?: string;
    createdAt?: number;
    pinOrder?: number;
    degradedReason?: string | null;
  },
): void {
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, degraded_reason, pinned_at, pin_order, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'eligible', 'unreviewed', ?, ?, ?, ?)`,
  ).run(
    seed.id,
    REPO,
    seed.type ?? 'discovery',
    seed.title,
    seed.body,
    cjkBigrams(`${seed.title} ${seed.body}`),
    `material_${seed.id}`,
    `content_${seed.id}`,
    seed.degradedReason ?? null,
    seed.pinOrder === undefined ? null : NOW - DAY,
    seed.pinOrder ?? null,
    seed.createdAt ?? NOW - DAY,
  );
}

function insertSession(
  db: DatabaseSync,
  session: {
    id: string;
    conversationId: string;
    status: 'active' | 'ended';
    endedAt?: number;
    summaryState?: 'pending' | 'done' | 'no_content';
    summaryId?: string;
  },
): void {
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
       started_at, ended_at, status, turn_count, latest_summary_memory_id, context_epoch, summary_state)
     VALUES (?, ?, 'claude', ?, ?, 'claude-opus-5', ?, ?, ?, 1, ?, 0, ?)`,
  ).run(
    session.id,
    REPO,
    `native_${session.id}`,
    session.conversationId,
    NOW - 2 * HOUR,
    session.endedAt ?? null,
    session.status,
    session.summaryId ?? null,
    session.summaryState ?? null,
  );
}

function insertPromptEvent(db: DatabaseSync, sessionId: string, content: string): void {
  db.prepare(
    `INSERT INTO raw_events (id, repo_id, session_id, agent, kind, content, payload_json,
       sensitivity, classification_state, captured_at, expires_at)
     VALUES (?, ?, ?, 'claude', 'prompt', ?, ?, 'local_only', 'done', ?, ?)`,
  ).run(
    `e_${sessionId}`,
    REPO,
    sessionId,
    content,
    JSON.stringify({ kind: 'prompt', text: content }),
    NOW - HOUR,
    NOW + DAY,
  );
}

function seedReadySession(
  db: DatabaseSync,
  options: { summaryDegraded?: string | null } = {},
): void {
  insertMemory(db, {
    id: 'm_summary',
    type: 'session_summary',
    title: 'Previous session',
    body: 'The database work landed.',
    createdAt: NOW - HOUR,
    degradedReason: options.summaryDegraded ?? null,
  });
  insertSession(db, {
    id: 's_prev',
    conversationId: 'c_prev',
    status: 'ended',
    endedAt: NOW - HOUR,
    summaryState: 'done',
    summaryId: 'm_summary',
  });
  insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
}

async function withDb(fn: (db: DatabaseSync) => Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 });
    insertRepo(db);
    try {
      await fn(db);
    } finally {
      db.close();
    }
  });
}

async function withOpened(
  fn: (db: DatabaseSync, token: string) => void | Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1_000 });
    try {
      const token = claimLease(opened.db, { pid: 1, now: NOW });
      if (token === null) assert.fail('expected a lease token');
      await fn(opened.db, token);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

function seedSummaryFixture(
  db: DatabaseSync,
  sessionId: string,
  prompt: string,
  batches: { id: string; degraded: string | null }[],
): void {
  insertRepo(db, SUMMARY_REPO, 'github.com/example/uploader');
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, started_at, ended_at,
       status, turn_count, summary_state)
     VALUES (?, ?, 'claude', ?, ?, ?, ?, 'ended', 1, 'pending')`,
  ).run(sessionId, SUMMARY_REPO, `native-${sessionId}`, sessionId, NOW - DAY, NOW - 1_000);
  db.prepare(
    'INSERT INTO turns (id, session_id, ordinal, started_at, ended_at) VALUES (?, ?, 1, ?, NULL)',
  ).run(`${sessionId}-t1`, sessionId, NOW - DAY);
  db.prepare(
    `INSERT INTO raw_events
       (id, repo_id, session_id, turn_id, agent, kind, content, payload_json, sensitivity,
        classification_state, captured_at, expires_at)
     VALUES (?, ?, ?, ?, 'claude', 'prompt', ?, NULL, 'eligible', 'done', ?, ?)`,
  ).run(`${sessionId}-p1`, SUMMARY_REPO, sessionId, `${sessionId}-t1`, prompt, NOW - DAY, NOW + 7 * DAY);
  for (const batch of batches) {
    db.prepare(
      `INSERT INTO observation_batches
         (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token,
          provider_attempts, degraded_reason, claimed_at)
       VALUES (?, ?, ?, ?, ?, 'session_end', ?, 'worker', 1, ?, ?)`,
    ).run(
      batch.id,
      SUMMARY_REPO,
      sessionId,
      `through-${batch.id}`,
      batch.degraded === null ? 'remote_observer' : 'fallback',
      batch.degraded === null ? 'applied' : 'fallback',
      batch.degraded,
      NOW - 1_000,
    );
  }
}

function summaryDegraded(db: DatabaseSync, memoryId: string): string | null {
  const row = db.prepare('SELECT degraded_reason FROM memories WHERE id = ?').get(memoryId);
  return (row?.degraded_reason as string | null | undefined) ?? null;
}

test('session summary degraded_reason is the most severe batch reason; NULL only when every batch was applied from a provider', async () => {
  await withOpened((db, token) => {
    seedSummaryFixture(db, 'sess-severe', 'Record the mixed fallback reasons.', [
      { id: 'b-rules', degraded: 'rule_based' },
      { id: 'b-unreach', degraded: 'unreachable' },
      { id: 'b-exhaust', degraded: 'provider_exhausted' },
    ]);
    const result = sessionSummary(db, token, 'sess-severe', NOW);
    assert.equal(result.state, 'done');
    if (result.memoryId === null) assert.fail('expected a summary memory');
    assert.equal(summaryDegraded(db, result.memoryId), 'provider_exhausted');
  });

  await withOpened((db, token) => {
    seedSummaryFixture(db, 'sess-clean', 'Record the provider-applied session.', [
      { id: 'b-one', degraded: null },
      { id: 'b-two', degraded: null },
    ]);
    const result = sessionSummary(db, token, 'sess-clean', NOW);
    assert.equal(result.state, 'done');
    if (result.memoryId === null) assert.fail('expected a summary memory');
    assert.equal(summaryDegraded(db, result.memoryId), null);
  });

  await withOpened((db, token) => {
    seedSummaryFixture(db, 'sess-mixed', 'Record one provider batch and one gap.', [
      { id: 'b-ok', degraded: null },
      { id: 'b-none', degraded: 'no_provider' },
    ]);
    const result = sessionSummary(db, token, 'sess-mixed', NOW);
    assert.equal(result.state, 'done');
    if (result.memoryId === null) assert.fail('expected a summary memory');
    assert.equal(summaryDegraded(db, result.memoryId), 'no_provider');
  });
});

test('a pending previous summary puts summary_pending on the session-start pack; a summary that becomes ready does not', async () => {
  await withDb(async (db) => {
    insertSession(db, {
      id: 's_prev',
      conversationId: 'c_prev',
      status: 'ended',
      endedAt: NOW - HOUR,
      summaryState: 'pending',
    });
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    insertPromptEvent(db, 's_prev', 'Please finish the retrieval module.');

    const pack = await buildSessionStartPack(
      db,
      packInput({ waitForSummary: () => 'pending' }),
    );
    assert.notEqual(pack, null);
    assert.ok(pack!.text.includes(`> degraded: ${DEGRADED_SENTENCES.summary_pending}`), pack!.text);
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, 'summary_pending');
  });

  await withDb(async (db) => {
    insertMemory(db, {
      id: 'm_summary',
      type: 'session_summary',
      title: 'Previous session',
      body: 'The database work landed.',
      createdAt: NOW - HOUR,
    });
    insertSession(db, {
      id: 's_prev',
      conversationId: 'c_prev',
      status: 'ended',
      endedAt: NOW - HOUR,
      summaryState: 'pending',
    });
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });

    const pack = await buildSessionStartPack(
      db,
      packInput({
        waitForSummary: () => {
          db.prepare(
            "UPDATE sessions SET summary_state = 'done', latest_summary_memory_id = 'm_summary' WHERE id = 's_prev'",
          ).run();
          return 'ready';
        },
      }),
    );
    assert.notEqual(pack, null);
    assert.equal(pack!.text.includes('> degraded:'), false, pack!.text);
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, null);
  });
});

test('session-start and prompt packs carry the most severe batch reason; summary_pending wins over a batch reason', async () => {
  await withDb(async (db) => {
    seedReadySession(db, { summaryDegraded: 'daily_cap' });
    const pack = await buildSessionStartPack(db, packInput());
    assert.notEqual(pack, null);
    assert.ok(pack!.text.includes(`> degraded: ${DEGRADED_SENTENCES.daily_cap}`), pack!.text);
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, 'daily_cap');
  });

  await withDb(async (db) => {
    insertSession(db, {
      id: 's_prev',
      conversationId: 'c_prev',
      status: 'ended',
      endedAt: NOW - HOUR,
      summaryState: 'pending',
    });
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    insertPromptEvent(db, 's_prev', 'Please finish the retrieval module.');
    insertMemory(db, {
      id: 'm_pin',
      title: 'Build command',
      body: 'Run npm run build before the tests.',
      pinOrder: 1,
      degradedReason: 'daily_cap',
    });

    const pack = await buildSessionStartPack(
      db,
      packInput({ waitForSummary: () => 'pending' }),
    );
    assert.notEqual(pack, null);
    assert.ok(pack!.text.includes(`> degraded: ${DEGRADED_SENTENCES.summary_pending}`), pack!.text);
    assert.equal(pack!.text.includes(DEGRADED_SENTENCES.daily_cap), false, pack!.text);
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, 'summary_pending');
  });

  await withDb(async (db) => {
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    insertMemory(db, {
      id: 'm_rules',
      title: 'Retrieval note',
      body: 'The ranking is lexical.',
      degradedReason: 'rule_based',
    });
    insertMemory(db, {
      id: 'm_cap',
      title: 'Retrieval order',
      body: 'Lexical ranking, then MMR.',
      degradedReason: 'provider_exhausted',
    });

    const pack = await buildPromptPack(
      db,
      packInput({ channel: 'claude:UserPromptSubmit', prompt: 'retrieval lexical ranking' }),
    );
    assert.notEqual(pack, null);
    assert.ok(pack!.text.includes(`> degraded: ${DEGRADED_SENTENCES.provider_exhausted}`), pack!.text);
    assert.equal(pack!.text.includes(DEGRADED_SENTENCES.rule_based), false, pack!.text);
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, 'provider_exhausted');
  });

  await withDb(async (db) => {
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    insertMemory(db, { id: 'm_ok', title: 'Retrieval note', body: 'The ranking is lexical.' });

    const pack = await buildPromptPack(
      db,
      packInput({ channel: 'claude:UserPromptSubmit', prompt: 'retrieval note' }),
    );
    assert.notEqual(pack, null);
    assert.equal(pack!.text.includes('> degraded:'), false, pack!.text);
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, null);
  });
});

test('a prompt pack for an undocumented model says window_unknown; a batch reason wins over window_unknown', async () => {
  await withDb(async (db) => {
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    insertMemory(db, { id: 'm_en', title: 'Retrieval note', body: 'The ranking is lexical.' });

    const unknown = await buildPromptPack(
      db,
      packInput({
        channel: 'claude:UserPromptSubmit',
        prompt: 'retrieval note',
        model: 'claude-not-in-the-table',
      }),
    );
    assert.notEqual(unknown, null);
    assert.ok(
      unknown!.text.includes(`> degraded: ${DEGRADED_SENTENCES.window_unknown}`),
      unknown!.text,
    );
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, 'window_unknown');
  });

  await withDb(async (db) => {
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    insertMemory(db, {
      id: 'm_en',
      title: 'Retrieval note',
      body: 'The ranking is lexical.',
      degradedReason: 'daily_cap',
    });

    const pack = await buildPromptPack(
      db,
      packInput({
        channel: 'claude:UserPromptSubmit',
        prompt: 'retrieval note',
        model: 'claude-not-in-the-table',
      }),
    );
    assert.notEqual(pack, null);
    assert.ok(pack!.text.includes(`> degraded: ${DEGRADED_SENTENCES.daily_cap}`), pack!.text);
    assert.equal(pack!.text.includes(DEGRADED_SENTENCES.window_unknown), false, pack!.text);
    assert.equal(whyReport(db, 's_now', scope(db))[0].degradedReason, 'daily_cap');
  });
});

test('a prompt that matches nothing returns null and records an omitted empty injection', async () => {
  await withDb(async (db) => {
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    const pack = await buildPromptPack(
      db,
      packInput({
        channel: 'claude:UserPromptSubmit',
        prompt: 'zzzz-no-such-memory-token',
      }),
    );
    assert.equal(pack, null);
    const report = whyReport(db, 's_now', scope(db));
    assert.equal(report.length, 1);
    assert.equal(report[0].state, 'omitted');
    assert.equal(report[0].degradedReason, 'empty');
  });
});

test('a pack whose rendered text trips the detector returns null and records omitted index_unavailable', async () => {
  await withDb(async (db) => {
    insertSession(db, { id: 's_now', conversationId: 'c1', status: 'active' });
    insertMemory(db, { id: 'm_en', title: 'Retrieval note', body: 'The ranking is lexical.' });

    const pack = await buildPromptPack(
      db,
      packInput({
        channel: 'claude:UserPromptSubmit',
        prompt: 'retrieval note',
        detect: (text) => text.includes(PACK_HEADER),
      }),
    );
    assert.equal(pack, null);
    const report = whyReport(db, 's_now', scope(db));
    assert.equal(report.length, 1);
    assert.equal(report[0].state, 'omitted');
    assert.equal(report[0].degradedReason, 'index_unavailable');
  });
});

test('usageEstimate reports exhausted while reset_at is in the future and not after it', async () => {
  await withTempHome(async (home) => {
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1_000 });
    try {
      const now = Date.UTC(2026, 8, 4, 12, 0, 0);
      const later = Date.UTC(2026, 8, 5, 0, 0, 1);
      const calls = 40;
      db.prepare(
        `INSERT INTO provider_usage
           (utc_day, preset, calls, neurons_estimate, reset_at, exhausted_at)
         VALUES (?, 'workers-ai', ?, 0, ?, ?)`,
      ).run(utcDay(now), calls, nextUtcMidnight(now), now);

      const today = usageEstimate(db, now);
      assert.equal(today.exhausted, true);
      assert.equal(today.remaining, DAILY_CAP - calls);
      assert.equal(today.calls, calls);

      assert.equal(usageEstimate(db, later).exhausted, false);
    } finally {
      db.close();
    }
  });
});

test('observe makes no provider fetch after recordExhausted and the batch falls back with provider_exhausted', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    await captureEndedSession(fixture, {
      sessionId: 'already-exhausted-session',
      prompts: ['Record the retry behavior.'],
      assistant: 'The upload path retries once.',
    });
    fixture.withDb((db) => {
      db.prepare(
        `INSERT INTO provider_usage
           (utc_day, preset, calls, neurons_estimate, reset_at)
         VALUES (?, 'openrouter', 1, 0, ?)`,
      ).run(utcDay(OBSERVE_NOW), nextUtcMidnight(OBSERVE_NOW));
      recordExhausted(db, {
        preset: 'openrouter',
        reservationId: 't073-exhausted',
        now: OBSERVE_NOW,
      });
    });

    let calls = 0;
    const fetchImpl: typeof fetch = async () => {
      calls += 1;
      throw new Error('provider fetch must not run after recordExhausted');
    };

    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);
    assert.equal(calls, 0);
    fixture.withDb((db) => {
      const batch = db
        .prepare('SELECT state, degraded_reason FROM observation_batches')
        .get();
      assert.deepEqual(
        { ...batch },
        { state: 'fallback', degraded_reason: 'provider_exhausted' },
      );
    });
  });
});
