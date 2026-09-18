import { grantVisibility } from '../../src/db/queries.js';
import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { memoryScope } from '../../src/db/queries.js';
import { CHANNEL_CAPS } from '../../src/injection/budget.js';
import { whyReport } from '../../src/injection/ledger.js';
import {
  buildPromptPack,
  buildSessionStartPack,
  type PromptPackInput,
  type SessionStartInput,
} from '../../src/injection/pack.js';
import { DEGRADED_SENTENCES } from '../../src/injection/pack-format.js';
import { PACK_HEADER } from '../../src/injection/recognize.js';
import { sessionSummary } from '../../src/observer/classify.js';
import {
  DAILY_CAP,
  nextUtcMidnight,
  presetExhaustedAt,
  recordExhausted,
  usageEstimate,
  utcDay,
} from '../../src/observer/reservation.js';
import { oboetePaths } from '../../src/paths.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';
import { seedWorkBinding } from '../helpers/work.js';
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
const scope = (db: DatabaseSync) => memoryScope(db, { repoId: REPO, destination: 'injection', workId: `fixture-work:${REPO}` });
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
  grantVisibility(db, seed.id, { audience: 'project', repoId: REPO }, 'migration', NOW);
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
  seedWorkBinding(db, session.id);
  db.prepare('UPDATE work_contexts SET root = ?').run('/nonexistent-repository-root');
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
  db.prepare('UPDATE raw_events SET work_binding_id = ? WHERE id = ?').run(`fixture-binding:${sessionId}`, `e_${sessionId}`);
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
  db.prepare('UPDATE memories SET work_id = ? WHERE id = ?').run(`fixture-work:${REPO}`, 'm_summary');
  db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ?').run('m_summary');
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
  for (const [index, batch] of batches.entries()) {
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
    const sourceId = `${sessionId}-p${index + 1}`;
    if (index > 0) {
      db.prepare(`INSERT INTO raw_events
        (id, repo_id, session_id, turn_id, agent, kind, content, sensitivity, classification_state, captured_at, expires_at)
        SELECT ?, repo_id, session_id, turn_id, agent, kind, content, sensitivity, classification_state, captured_at, expires_at
        FROM raw_events WHERE id = ?`).run(sourceId, `${sessionId}-p1`);
    }
    db.prepare('UPDATE raw_events SET batch_id = ?, processing_state = ?, processed_at = ? WHERE id = ?')
      .run(batch.id, batch.degraded === null ? 'processed' : 'waiting', batch.degraded === null ? NOW : null, sourceId);
    db.prepare(`INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, reason, recorded_at)
      VALUES (?, ?, ?, ?, ?)`).run(batch.id, sourceId, batch.degraded === null ? 'processed' : 'deferred', batch.degraded, NOW);
  }
}

function summaryDegraded(db: DatabaseSync, memoryId: string): string | null {
  const row = db.prepare('SELECT degraded_reason FROM memories WHERE id = ?').get(memoryId);
  return (row?.degraded_reason as string | null | undefined) ?? null;
}

test('a row the summary never counted as a source cannot label the summary', async () => {
  // `generationPending` counts sources through SUMMARY_SOURCE_SQL, which excludes a partial row that
  // is not a tool call carrying paths. `revalidateSources` still re-reads and defers such a row by
  // name, so without the same predicate its receipt would blame the summarizer for text it never saw.
  await withOpened((db, token) => {
    seedSummaryFixture(db, 'sess-excluded', 'Record the excluded source.', [{ id: 'b-applied', degraded: null }]);
    db.prepare(`INSERT INTO raw_events
      (id, repo_id, session_id, turn_id, agent, kind, content, sensitivity, classification_state, captured_at, expires_at)
      SELECT ?, repo_id, session_id, turn_id, agent, 'prompt', content, sensitivity, 'partial', captured_at, expires_at
      FROM raw_events WHERE id = 'sess-excluded-p1'`).run('sess-excluded-p2');
    db.prepare("UPDATE raw_events SET batch_id = 'b-applied', processing_state = 'waiting' WHERE id = 'sess-excluded-p2'")
      .run();
    db.prepare(`INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, reason, recorded_at)
      VALUES ('b-applied', 'sess-excluded-p2', 'deferred', 'detector_failed', ?)`).run(NOW);
    // An ordinary pending source, so the summary is still pending and a reason would be rendered.
    db.prepare("UPDATE raw_events SET processing_state = 'pending' WHERE id = 'sess-excluded-p1'").run();

    const result = sessionSummary(db, token, 'sess-excluded', NOW);
    assert.equal(result.state, 'waiting');
    if (result.memoryId === null) assert.fail('expected a summary memory');
    assert.equal(summaryDegraded(db, result.memoryId), 'rule_based');
  });
});

test('a batch that failed is reported even when every row it left behind is excluded', async () => {
  // The other side of the predicate above. A receipt is that row's own verdict, so an excluded row
  // must not label the summary — but `degraded_reason` describes the attempt, not the row. Gating
  // both on `SUMMARY_SOURCE_SQL` turns a real provider or consent failure into `rule_based`, which
  // tells the user the notes were written by the built-in rules and hides why.
  for (const batchReason of ['provider_exhausted', 'consent_changed'] as const) {
    await withOpened((db, token) => {
      const session = `sess-failed-${batchReason}`;
      seedSummaryFixture(db, session, 'Record the failed batch.', [{ id: 'b-failed', degraded: batchReason }]);
      // The only row the failed batch still holds is a partial prompt, which SUMMARY_SOURCE_SQL
      // excludes: the summary never counted it, but the batch it belonged to did fail.
      db.prepare(`INSERT INTO raw_events
        (id, repo_id, session_id, turn_id, agent, kind, content, sensitivity, classification_state, captured_at, expires_at)
        SELECT ?, repo_id, session_id, turn_id, agent, 'prompt', content, sensitivity, 'partial', captured_at, expires_at
        FROM raw_events WHERE id = ?`).run(`${session}-p2`, `${session}-p1`);
      db.prepare("UPDATE raw_events SET batch_id = 'b-failed', processing_state = 'waiting' WHERE id = ?")
        .run(`${session}-p2`);
      db.prepare(`INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, reason, recorded_at)
        VALUES ('b-failed', ?, 'deferred', ?, ?)`).run(`${session}-p2`, batchReason, NOW);
      // Detach the fixture's own source so the excluded partial is the only row the failed batch
      // still holds; it stays pending and unbatched, which is what keeps the summary generating.
      db.prepare('DELETE FROM observation_batch_sources WHERE raw_event_id = ?').run(`${session}-p1`);
      db.prepare("UPDATE raw_events SET batch_id = NULL, processing_state = 'pending' WHERE id = ?")
        .run(`${session}-p1`);

      const result = sessionSummary(db, token, session, NOW);
      assert.equal(result.state, 'waiting');
      if (result.memoryId === null) assert.fail('expected a summary memory');
      assert.equal(summaryDegraded(db, result.memoryId), batchReason);
    });
  }
});

test('an unprocessed source reports what its own receipt says, whatever the batch did', async () => {
  // A batch can apply with `degraded_reason` NULL while one of its sources is still unprocessed and
  // its receipt is the only record. Reading the batch alone reports rule-based notes for all of these.
  const cases = [
    { outcome: 'deferred', reason: 'detector_failed', expect: 'unusable_output' },
    { outcome: 'deferred', reason: 'consent_changed', expect: 'consent_changed' },
    { outcome: 'deferred', reason: 'unreachable', expect: 'unreachable' },
    { outcome: 'uncovered', reason: 'unaccounted', expect: 'unusable_output' },
    { outcome: 'rejected', reason: 'directive', expect: 'unusable_output' },
    // Nothing happened to the source, or what happened says where it is in the queue.
    { outcome: 'deferred', reason: 'source_context_unknown', expect: 'rule_based' },
    { outcome: 'deferred', reason: 'partial_capture', expect: 'rule_based' },
    { outcome: 'deferred', reason: 'work_selection_required', expect: 'rule_based' },
    { outcome: 'uncovered', reason: 'not_sent', expect: 'rule_based' },
    { outcome: 'rejected', reason: 'secret', expect: 'rule_based' },
    // `reconcilePendingDestinations` writes these two beside a batch it marks `rule_based` and
    // `consent_changed` respectively. Left to the fail-closed default both would read as an unusable
    // answer, which contradicts the batch's own verdict for the first and only survives severity
    // order for the second.
    { outcome: 'deferred', reason: 'request_page_limit', expect: 'rule_based' },
    { outcome: 'deferred', reason: 'destination_changed', expect: 'consent_changed' },
    // The three outcomes that return before the reason is read. Each carries a reason the default
    // would turn into `unusable_output`, so dropping its arm from the early return fails here:
    // `assigned` beside a stale reason, `processed` the way `settleSources` leaves a partial row,
    // and `legacy_unknown` the way migration 0004 writes every pre-receipt source.
    { outcome: 'assigned', reason: null, expect: 'rule_based' },
    { outcome: 'assigned', reason: 'detector_failed', expect: 'rule_based' },
    { outcome: 'processed', reason: 'deduplicated', expect: 'rule_based' },
    { outcome: 'legacy_unknown', reason: 'legacy_processing_unknown', expect: 'rule_based' },
  ];
  for (const [index, { outcome, reason, expect }] of cases.entries()) {
    await withOpened((db, token) => {
      const session = `sess-receipt-${index}`;
      seedSummaryFixture(db, session, 'Record the mixed source outcome.', [{ id: 'b-applied', degraded: null }]);
      db.prepare(`INSERT INTO raw_events
        (id, repo_id, session_id, turn_id, agent, kind, content, sensitivity, classification_state, captured_at, expires_at)
        SELECT ?, repo_id, session_id, turn_id, agent, kind, content, sensitivity, classification_state, captured_at, expires_at
        FROM raw_events WHERE id = ?`).run(`${session}-p2`, `${session}-p1`);
      db.prepare("UPDATE raw_events SET batch_id = 'b-applied', processing_state = 'waiting' WHERE id = ?")
        .run(`${session}-p2`);
      db.prepare(`INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, reason, recorded_at)
        VALUES ('b-applied', ?, ?, ?, ?)`).run(`${session}-p2`, outcome, reason, NOW);

      const result = sessionSummary(db, token, session, NOW);
      assert.equal(result.state, 'waiting');
      if (result.memoryId === null) assert.fail('expected a summary memory');
      assert.equal(summaryDegraded(db, result.memoryId), expect, `${outcome}/${reason}`);
    });
  }
});

test('session summary reflects unresolved source outcomes and complete processing clears degradation', async () => {
  await withOpened((db, token) => {
    seedSummaryFixture(db, 'sess-severe', 'Record the mixed fallback reasons.', [
      { id: 'b-rules', degraded: 'rule_based' },
      { id: 'b-unreach', degraded: 'unreachable' },
      { id: 'b-exhaust', degraded: 'provider_exhausted' },
    ]);
    const result = sessionSummary(db, token, 'sess-severe', NOW);
    assert.equal(result.state, 'waiting');
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
    assert.equal(result.state, 'waiting');
    if (result.memoryId === null) assert.fail('expected a summary memory');
    assert.equal(summaryDegraded(db, result.memoryId), 'no_provider');
  });
});

test('pending work activity carries summary_pending; a current checkpoint without pending activity does not', async () => {
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
      packInput(),
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

    db.prepare('UPDATE memories SET work_id = ? WHERE id = ?').run(`fixture-work:${REPO}`, 'm_summary');
    db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ?').run('m_summary');
    const pack = await buildSessionStartPack(db, packInput());
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
      packInput(),
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

test('a preset reads exhausted while reset_at is in the future and not after it', async () => {
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
      assert.equal(presetExhaustedAt(db, 'workers-ai', now), now);
      assert.equal(today.remaining, DAILY_CAP - calls);
      assert.equal(today.calls, calls);

      assert.equal(presetExhaustedAt(db, 'workers-ai', later), null);
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
