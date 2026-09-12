import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import type { DetectorResult } from '../../src/privacy/detect.js';
import {
  classifyPending,
  createBatches,
  loadBatchInput,
  reclaimStale,
  toolInputOf,
  BLANK_CODE_POINTS,
} from '../../src/worker/batches.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';
import { seedWorkBinding } from '../helpers/work.js';

const NOW = 1_757_000_000_000;
const DAY = 24 * 60 * 60 * 1000;

/** The fake detector of the task: unchanged text unless it carries the marker, which it redacts. */
async function fakeDetect(text: string): Promise<DetectorResult> {
  if (text.includes('DETECTOR-FAILS')) return { ok: false, reason: 'detector_error' };
  if (!text.includes('SECRET-MARKER')) {
    return { ok: true, text, texts: [], redactions: [], privateRemoved: 0, sensitivity: 'local_only', pathRule: null };
  }
  return {
    ok: true,
    text: text.replaceAll('SECRET-MARKER', '[REDACTED:test]'),
    texts: [],
    redactions: [{ rule: 'test', count: 1 }],
    privateRemoved: 0,
    sensitivity: 'secret',
    pathRule: null,
  };
}

async function withOpened(
  fn: (db: DatabaseSync, home: string, token: string) => void | Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      const token = claimLease(opened.db, { pid: 1, now: NOW });
      if (token === null) assert.fail('expected a lease token');
      await fn(opened.db, home, token);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

function seedRepo(db: DatabaseSync): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES ('repo1', 'common_dir', '/tmp/oboete-batches', '/tmp/oboete-batches', 1, 1)`,
  ).run();
}

function seedSession(
  db: DatabaseSync,
  id: string,
  options: { status?: 'active' | 'ended'; turnCount?: number; turns?: number } = {},
): void {
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, started_at, status, turn_count)
     VALUES (?, 'repo1', 'claude', ?, ?, ?, ?, ?)`,
  ).run(id, `native-${id}`, id, NOW - DAY, options.status ?? 'active', options.turnCount ?? 0);
  for (let ordinal = 1; ordinal <= (options.turns ?? 0); ordinal += 1) {
    db.prepare(
      `INSERT INTO turns (id, session_id, ordinal, started_at, ended_at) VALUES (?, ?, ?, ?, ?)`,
    ).run(`${id}-t${ordinal}`, id, ordinal, NOW - DAY + ordinal, NOW - DAY + ordinal + 1);
  }
}

type EventSeed = {
  id: string;
  sessionId?: string;
  turn?: number;
  kind?: string;
  content?: string | null;
  payload?: unknown;
  sensitivity?: string;
  state?: string;
  capturedAt?: number;
  expiresAt?: number;
  batchId?: string | null;
};

let capturedCounter = 0;

function seedEvent(db: DatabaseSync, seed: EventSeed): void {
  capturedCounter += 1;
  const sessionId = seed.sessionId ?? 'sess1';
  db.prepare(
    `INSERT INTO raw_events
       (id, repo_id, session_id, turn_id, agent, kind, content, payload_json, sensitivity,
        classification_state, captured_at, expires_at, batch_id, work_binding_id)
     VALUES (?, 'repo1', ?, ?, 'claude', ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    seed.id,
    sessionId,
    seed.turn === undefined ? null : `${sessionId}-t${seed.turn}`,
    seed.kind ?? 'prompt',
    seed.content === undefined ? `text of ${seed.id}` : seed.content,
    seed.payload === undefined ? null : JSON.stringify(seed.payload),
    seed.sensitivity ?? 'eligible',
    seed.state ?? 'done',
    seed.capturedAt ?? NOW - DAY + capturedCounter,
    seed.expiresAt ?? NOW + 7 * DAY,
    seed.batchId ?? null,
    seedWorkBinding(db, sessionId),
  );
}

function batchRows(db: DatabaseSync): { id: string; destination: string; trigger: string; through: string; state: string }[] {
  return db
    .prepare('SELECT id, destination, trigger, through_event_id, state FROM observation_batches ORDER BY destination')
    .all()
    .map((row) => ({
      id: String(row.id),
      destination: String(row.destination),
      trigger: String(row.trigger),
      through: String(row.through_event_id),
      state: String(row.state),
    }));
}

function rowsOfBatch(db: DatabaseSync, batchId: string): string[] {
  return db
    .prepare('SELECT id FROM raw_events WHERE batch_id = ? ORDER BY id')
    .all(batchId)
    .map((row) => String(row.id));
}

function unbatched(db: DatabaseSync): string[] {
  return db
    .prepare('SELECT id FROM raw_events WHERE batch_id IS NULL ORDER BY id')
    .all()
    .map((row) => String(row.id));
}

/** Twelve turns, one row per turn, every sensitivity and both unsummarizable states. */
function seedMixedSession(db: DatabaseSync): void {
  seedRepo(db);
  seedSession(db, 'sess1', { turns: 12, turnCount: 12 });
  seedEvent(db, { id: 'e01-eligible', turn: 1 });
  seedEvent(db, { id: 'e02-eligible-tool', turn: 2, kind: 'tool_result', content: 'output body' });
  seedEvent(db, { id: 'e03-local', turn: 3, sensitivity: 'local_only' });
  seedEvent(db, { id: 'e04-private', turn: 4, sensitivity: 'private' });
  seedEvent(db, { id: 'e05-secret', turn: 5, sensitivity: 'secret' });
  seedEvent(db, { id: 'e06-failed', turn: 6, sensitivity: 'local_only', state: 'failed', content: null });
  seedEvent(db, {
    id: 'e07-partial',
    turn: 7,
    sensitivity: 'local_only',
    state: 'partial',
    kind: 'tool_call',
    content: 'read part of the payload',
    payload: { tool_name: 'read', input: { paths: ['src/a.ts'] } },
  });
  seedEvent(db, { id: 'e08-lifecycle', turn: 8, kind: 'session_start', content: null });
  for (const turn of [8, 9, 10, 11, 12]) {
    seedEvent(db, { id: `e1${turn}-eligible`, turn });
  }
}

test('a remote preset splits a mixed session into disjoint remote and fallback batches', async () => {
  await withOpened((db, _home, token) => {
    seedMixedSession(db);

    const result = createBatches(db, token, NOW, { preset: 'remote' });
    assert.equal(result.leaseLost, false);

    const batches = batchRows(db);
    assert.deepEqual(
      batches.map((batch) => batch.destination),
      ['fallback', 'remote_observer'],
    );
    const [fallback, remote] = batches;

    assert.equal(remote.trigger, 'ten_turns');
    assert.equal(fallback.trigger, 'ten_turns');
    assert.equal(remote.state, 'pending');
    // contracts/observer.md: a remote batch and a fallback batch cover the same range.
    assert.equal(remote.through, fallback.through);
    assert.equal(remote.through, 'e112-eligible');

    const remoteRows = rowsOfBatch(db, remote.id);
    const fallbackRows = rowsOfBatch(db, fallback.id);
    assert.deepEqual(remoteRows, [
      'e01-eligible',
      'e02-eligible-tool',
      'e18-eligible',
      'e19-eligible',
      'e110-eligible',
      'e111-eligible',
      'e112-eligible',
    ].sort());
    assert.deepEqual(fallbackRows, ['e03-local', 'e04-private', 'e07-partial'].sort());
    // "no observation is generated twice": the two batches share no row at all.
    assert.equal(remoteRows.some((id) => fallbackRows.includes(id)), false);
    // A7: a partial row reaches the fallback only, never a provider batch.
    assert.equal(remoteRows.includes('e07-partial'), false);
    // The secret row, the failed row and the lifecycle row are in no batch.
    assert.deepEqual(unbatched(db), ['e05-secret', 'e06-failed', 'e08-lifecycle']);
  });
});

test('batch allocation and classification bound payload bytes independently of source count', async () => {
  await withOpened(async (db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { status: 'ended' });
    const content = 'A'.repeat(800_000);
    for (let index = 0; index < 8; index += 1) seedEvent(db, { id: `large-${index}`, content, sensitivity: 'local_only' });
    let scanned = 0;
    await classifyPending(db, token, NOW, async (text) => { scanned += Buffer.byteLength(text); return await fakeDetect(text); });
    assert.ok(scanned > 0 && scanned <= 2 * 1024 * 1024, `classified ${scanned} bytes in one page`);
    const created = createBatches(db, token, NOW, { preset: 'remote' });
    const bytes = Number(db.prepare('SELECT SUM(length(CAST(content AS BLOB))) AS n FROM raw_events WHERE batch_id IS NOT NULL').get()?.n);
    assert.ok(created.created.length > 0);
    assert.ok(bytes <= 2 * 1024 * 1024, `claimed ${bytes} bytes in one page`);
  });
});

test('a local preset batches the non-secret rows and leaves the partial row to the fallback', async () => {
  await withOpened((db, _home, token) => {
    seedMixedSession(db);

    createBatches(db, token, NOW, { preset: 'local' });

    const batches = batchRows(db);
    assert.deepEqual(
      batches.map((batch) => batch.destination),
      ['fallback', 'local_observer'],
    );
    const local = batches[1];
    const localRows = rowsOfBatch(db, local.id);
    assert.equal(localRows.includes('e03-local'), true);
    assert.equal(localRows.includes('e04-private'), true);
    assert.equal(localRows.includes('e05-secret'), false);
    assert.equal(localRows.includes('e07-partial'), false);
    assert.deepEqual(rowsOfBatch(db, batches[0].id), ['e07-partial']);
  });
});

test('no preset sends every non-secret row to one fallback batch', async () => {
  await withOpened((db, _home, token) => {
    seedMixedSession(db);

    createBatches(db, token, NOW, { preset: 'none' });

    const batches = batchRows(db);
    assert.deepEqual(batches.map((batch) => batch.destination), ['fallback']);
    assert.deepEqual(
      rowsOfBatch(db, batches[0].id),
      [
        'e01-eligible',
        'e02-eligible-tool',
        'e03-local',
        'e04-private',
        'e07-partial',
        'e18-eligible',
        'e19-eligible',
        'e110-eligible',
        'e111-eligible',
        'e112-eligible',
      ].sort(),
    );
  });
});

test('an ended session is batched at session end and an expiring row forces a retention batch', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'ended1', { status: 'ended', turns: 2, turnCount: 2 });
    seedEvent(db, { id: 'x1', sessionId: 'ended1', turn: 1 });
    seedSession(db, 'live1', { turns: 2, turnCount: 2 });
    seedEvent(db, { id: 'y1', sessionId: 'live1', turn: 1, expiresAt: NOW + 12 * 60 * 60 * 1000 });
    seedSession(db, 'quiet1', { turns: 2, turnCount: 2 });
    seedEvent(db, { id: 'z1', sessionId: 'quiet1', turn: 1 });

    const created = [
      ...createBatches(db, token, NOW, { preset: 'none' }).created,
      ...createBatches(db, token, NOW, { preset: 'none' }).created,
    ];
    const bySession = new Map(created.map((batch) => [batch.session_id, batch.trigger]));
    assert.equal(bySession.get('ended1'), 'session_end');
    assert.equal(bySession.get('live1'), 'retention');
    // Nine turns short of a ten-turn batch, still running, nothing expiring: no batch at all.
    assert.equal(bySession.has('quiet1'), false);
    assert.deepEqual(unbatched(db), ['z1']);
  });
});

test('capture-time expiry does not replace pending generation with fallback', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { turns: 1, turnCount: 1 });
    db.prepare(
      `INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, provider_attempts, claimed_at)
       VALUES ('b-stuck', 'repo1', 'sess1', 'p1', 'remote_observer', 'ten_turns', 'pending', ?, 0, ?)`,
    ).run(token, NOW - DAY);
    seedEvent(db, { id: 'p1', turn: 1, batchId: 'b-stuck', expiresAt: NOW - 1 });

    createBatches(db, token, NOW, { preset: 'remote' });

    const batches = batchRows(db);
    assert.deepEqual(batches.map((batch) => batch.destination), ['remote_observer']);
    assert.equal(batches[0].id, 'b-stuck');
    assert.equal(batches[0].trigger, 'ten_turns');
    assert.deepEqual(rowsOfBatch(db, batches[0].id), ['p1']);
  });
});

test('a second round over the same range does not collide with a finished batch', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { turns: 1, turnCount: 1 });
    // A due source receives a new attempt without overwriting the previous attempt's identity.
    db.prepare(
      `INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, provider_attempts, claimed_at)
       VALUES ('b-done', 'repo1', 'sess1', 'p1', 'remote_observer', 'retention', 'fallback', ?, 0, ?)`,
    ).run(token, NOW - DAY);
    seedEvent(db, { id: 'p1', turn: 1, expiresAt: NOW - 1 });
    db.prepare("UPDATE raw_events SET processing_state = 'waiting', retry_after = ? WHERE id = 'p1'").run(NOW - 1);

    const result = createBatches(db, token, NOW, { preset: 'remote' });

    assert.equal(result.leaseLost, false);
    assert.equal(result.created.length, 1);
    const created = result.created[0];
    assert.equal(created.destination, 'remote_observer');
    assert.deepEqual(rowsOfBatch(db, created.id), ['p1']);
    assert.notEqual(created.through_event_id, 'p1', 'the second round carries its own key');
    assert.deepEqual(
      batchRows(db).map((batch) => batch.id).sort(),
      ['b-done', created.id].sort(),
      'the finished attempt is untouched',
    );
  });
});

test('a foreign token writes nothing and reports the lost lease', async () => {
  await withOpened((db, _home, token) => {
    seedMixedSession(db);

    const result = createBatches(db, 'not-the-owner', NOW, { preset: 'remote' });
    assert.equal(result.leaseLost, true);
    assert.deepEqual(batchRows(db), []);
    assert.equal(unbatched(db).length, 13);
    assert.notEqual(token, 'not-the-owner');
  });
});

test('a stale running batch of a dead worker is reclaimed after 120 seconds', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { turns: 1 });
    for (const [id, age] of [
      ['b-stale', 130_000],
      ['b-fresh', 30_000],
    ] as const) {
      db.prepare(
        `INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, provider_attempts, claimed_at)
         VALUES (?, 'repo1', 'sess1', ?, 'fallback', 'ten_turns', 'running', 'dead-worker', 1, ?)`,
      ).run(id, id, NOW - age);
    }

    const result = reclaimStale(db, token, NOW);
    assert.equal(result.reclaimed, 1);
    const rows = db
      .prepare('SELECT id, state, owner_token FROM observation_batches ORDER BY id')
      .all()
      .map((row) => [String(row.id), String(row.state), String(row.owner_token)]);
    assert.deepEqual(rows, [
      ['b-fresh', 'running', 'dead-worker'],
      ['b-stale', 'pending', token],
    ]);
  });
});

test('classification promotes a clean local-only row and leaves partial, private and secret rows', async () => {
  await withOpened(async (db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { turns: 1 });
    seedEvent(db, { id: 'clean', turn: 1, sensitivity: 'local_only', state: 'pending' });
    seedEvent(db, { id: 'marked', turn: 1, sensitivity: 'local_only', state: 'pending', content: 'token SECRET-MARKER here' });
    seedEvent(db, { id: 'partial', turn: 1, sensitivity: 'local_only', state: 'partial' });
    seedEvent(db, { id: 'private', turn: 1, sensitivity: 'private', state: 'pending' });
    seedEvent(db, { id: 'secret', turn: 1, sensitivity: 'secret', state: 'done' });
    seedEvent(db, { id: 'unreadable', turn: 1, sensitivity: 'local_only', state: 'pending', content: 'DETECTOR-FAILS here' });

    const result = await classifyPending(db, token, NOW, fakeDetect);
    assert.equal(result.leaseLost, false);
    assert.equal(result.promoted, 1);
    assert.equal(result.failed, 1);

    const rows = db
      .prepare('SELECT id, sensitivity, classification_state, content FROM raw_events ORDER BY id')
      .all()
      .map((row) => [String(row.id), String(row.sensitivity), String(row.classification_state)]);
    assert.deepEqual(rows, [
      ['clean', 'eligible', 'done'],
      ['marked', 'secret', 'done'],
      ['partial', 'local_only', 'partial'],
      ['private', 'private', 'pending'],
      ['secret', 'secret', 'done'],
      // A detector failure never promotes and never discards what capture already redacted.
      ['unreadable', 'local_only', 'pending'],
    ]);
    // FR-018: what the second detector run found is redacted in the stored row too.
    const marked = db.prepare('SELECT content FROM raw_events WHERE id = ?').get('marked');
    assert.equal(String(marked?.content).includes('SECRET-MARKER'), false);
  });
});

test('classification never promotes a row whose secret is in the tool input that travels', async () => {
  await withOpened(async (db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { turns: 1 });
    // The stored content is clean, but the normalized tool input is part of the outbound request,
    // so the promotion gate has to read it too (FR-017, contracts/observer.md "Input").
    seedEvent(db, {
      id: 'cmd',
      turn: 1,
      kind: 'tool_call',
      content: 'ran the deploy command',
      payload: { tool_name: 'bash', input: { paths: [], command: 'deploy --token SECRET-MARKER' } },
      sensitivity: 'local_only',
      state: 'pending',
    });

    const result = await classifyPending(db, token, NOW, fakeDetect);
    assert.equal(result.promoted, 0);
    assert.equal(result.secret, 1);
    assert.equal(
      String(db.prepare('SELECT sensitivity FROM raw_events WHERE id = ?').get('cmd')?.sensitivity),
      'secret',
    );
  });
});

test('a tool call whose only content is its command still enters a batch', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { status: 'ended', turns: 1 });
    seedEvent(db, {
      id: 'bash1',
      turn: 1,
      kind: 'tool_call',
      content: '',
      payload: { tool_name: 'bash', input: { paths: [], command: 'npm test' } },
    });

    createBatches(db, token, NOW, { preset: 'none' });
    assert.notEqual(db.prepare('SELECT batch_id FROM raw_events WHERE id = ?').get('bash1')?.batch_id, null);
  });
});

test('a stored tool call gives its command back to a shell tool and its text to any other', () => {
  // The shape capture writes: the free text of the call is in `content`, not in `payload_json`.
  assert.deepEqual(
    toolInputOf({
      content: 'npm test',
      payload_json: JSON.stringify({ tool_name: 'bash', input: { paths: [] } }),
    }),
    { paths: [], command: 'npm test' },
  );
  assert.deepEqual(
    toolInputOf({
      content: 'before\n→\nafter',
      payload_json: JSON.stringify({
        tool_name: 'edit',
        input: { paths: ['src/a.ts'], lines_added: 1, lines_removed: 1 },
      }),
    }),
    { paths: ['src/a.ts'], lines_added: 1, lines_removed: 1, text: 'before\n→\nafter' },
  );
  // A7: a partial row hands over its paths and never the text it holds.
  assert.deepEqual(
    toolInputOf({
      content: 'the payload was cut',
      payload_json: JSON.stringify({ tool_name: 'bash', input: { paths: [] } }),
      classification_state: 'partial',
    }),
    { paths: [] },
  );
});

test('the batch input keeps the row order and strips a partial row to its metadata', async () => {
  await withOpened((db, _home, token) => {
    seedMixedSession(db);
    createBatches(db, token, NOW, { preset: 'none' });
    const batch = batchRows(db)[0];

    const input = loadBatchInput(db, batch.id);
    if (input === null) assert.fail('expected the batch to load');
    assert.equal(input.session.id, 'sess1');
    assert.equal(input.turns.length, 10);
    assert.deepEqual(
      input.rows.map((row) => row.id),
      [
        'e01-eligible',
        'e02-eligible-tool',
        'e03-local',
        'e04-private',
        'e07-partial',
        'e18-eligible',
        'e19-eligible',
        'e110-eligible',
        'e111-eligible',
        'e112-eligible',
      ],
    );
    const partial = input.rows.find((row) => row.id === 'e07-partial');
    assert.equal(partial?.content, null);
    assert.equal(partial?.payload_json, JSON.stringify({ tool_name: 'read', input: { paths: ['src/a.ts'] } }));
  });
});

test('one creation transaction claims at most fifty sources and skips metadata before testing ten turns', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { turns: 10 });
    for (let index = 0; index < 120; index += 1) {
      seedEvent(db, { id: `metadata-${index}`, kind: 'turn_end', content: null });
    }
    for (let index = 0; index < 70; index += 1) {
      seedEvent(db, { id: `source-${index}`, turn: index < 61 ? 1 : index - 59 });
    }
    createBatches(db, token, NOW, { preset: 'local' });
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM raw_events WHERE batch_id IS NOT NULL').get()?.n, 50);
    // The remaining nine turns are the tail of an already-triggered cohort, even though fewer
    // than ten distinct turns remain after the first page completes.
    db.exec("UPDATE raw_events SET processing_state = 'processed' WHERE batch_id IS NOT NULL");
    db.exec("UPDATE observation_batches SET state = 'applied'");
    createBatches(db, token, NOW + 1, { preset: 'local' });
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM raw_events WHERE batch_id IS NOT NULL').get()?.n, 70);
  });
});

test('a large partially processed source cannot hide a fresh source before the page cap', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { status: 'ended' });
    seedEvent(db, { id: 'old-partial', content: 'x'.repeat(2 * 1024 * 1024 + 1), capturedAt: NOW - 100 });
    const first = createBatches(db, token, NOW, { preset: 'local' }).created[0];
    assert.ok(first);
    db.prepare("UPDATE observation_batches SET state = 'applied' WHERE id = ?").run(first.id);
    db.prepare("UPDATE observation_batch_sources SET outcome = 'processed' WHERE batch_id = ?").run(first.id);
    db.exec("UPDATE raw_events SET batch_id = NULL, processing_offset = 100 WHERE id = 'old-partial'");
    seedEvent(db, { id: 'fresh-source', content: 'New independent upload evidence.', capturedAt: NOW - 1 });
    const next = createBatches(db, token, NOW + 1, { preset: 'local' }).created[0];
    assert.ok(next);
    assert.deepEqual(db.prepare('SELECT id FROM raw_events WHERE batch_id = ?').all(next.id).map((row) => row.id), ['fresh-source']);
    assert.equal(db.prepare("SELECT batch_id FROM raw_events WHERE id = 'old-partial'").get()?.batch_id, null);
  });
});

test('the blank characters the SQL predicates trim are the ones trim() removes', () => {
  // purge's delete predicate and the worker's queue check ask "has content" in SQL, while
  // `isSummarizableRow` asks it with `trim()`. A character on one list only is a row that is never
  // batched, never purged, and keeps the worker awake for its whole run.
  const stripped: number[] = [];
  for (let code = 0; code <= 0xffff; code += 1) {
    const character = String.fromCharCode(code);
    if (character.trim() === '') stripped.push(code);
  }
  assert.deepEqual([...BLANK_CODE_POINTS], stripped);
});
