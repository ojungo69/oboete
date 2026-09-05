import assert from 'node:assert/strict';
import { mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import type { DetectorResult } from '../../src/privacy/detect.js';
import {
  classifyPending,
  createBatches,
  loadBatchInput,
  recoverSpool,
  reclaimStale,
  toolInputOf,
  BLANK_CODE_POINTS,
} from '../../src/worker/batches.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';

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
        classification_state, captured_at, expires_at, batch_id)
     VALUES (?, 'repo1', ?, ?, 'claude', ?, ?, ?, ?, ?, ?, ?, ?)`,
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

    const created = createBatches(db, token, NOW, { preset: 'none' }).created;
    const bySession = new Map(created.map((batch) => [batch.session_id, batch.trigger]));
    assert.equal(bySession.get('ended1'), 'session_end');
    assert.equal(bySession.get('live1'), 'retention');
    // Nine turns short of a ten-turn batch, still running, nothing expiring: no batch at all.
    assert.equal(bySession.has('quiet1'), false);
    assert.deepEqual(unbatched(db), ['z1']);
  });
});

test('an expired row in a pending provider batch is forced into a fallback batch', async () => {
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
    assert.deepEqual(batches.map((batch) => batch.destination), ['fallback']);
    assert.equal(batches[0].trigger, 'retention');
    assert.deepEqual(rowsOfBatch(db, batches[0].id), ['p1']);
  });
});

test('a second round over the same range does not collide with a finished batch', async () => {
  await withOpened((db, _home, token) => {
    seedRepo(db);
    seedSession(db, 'sess1', { turns: 1, turnCount: 1 });
    // The fallback batch of the first round is finished, and the provider batch of the same range
    // is still pending, so its expired row is detached and batched again (R6 retention).
    db.prepare(
      `INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, provider_attempts, claimed_at)
       VALUES ('b-done', 'repo1', 'sess1', 'p1', 'fallback', 'retention', 'fallback', ?, 0, ?)`,
    ).run(token, NOW - DAY);
    db.prepare(
      `INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, provider_attempts, claimed_at)
       VALUES ('b-stuck', 'repo1', 'sess1', 'p1', 'remote_observer', 'ten_turns', 'pending', ?, 0, ?)`,
    ).run(token, NOW - DAY);
    seedEvent(db, { id: 'p1', turn: 1, batchId: 'b-stuck', expiresAt: NOW - 1 });

    const result = createBatches(db, token, NOW, { preset: 'remote' });

    assert.equal(result.leaseLost, false);
    assert.equal(result.created.length, 1);
    const created = result.created[0];
    assert.equal(created.destination, 'fallback');
    assert.deepEqual(rowsOfBatch(db, created.id), ['p1']);
    assert.notEqual(created.through_event_id, 'p1', 'the second round carries its own key');
    assert.deepEqual(
      batchRows(db).map((batch) => batch.id).sort(),
      ['b-done', created.id].sort(),
      'the empty provider batch is gone and the finished one is untouched',
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

test('spool recovery is idempotent and quarantines a file it cannot read', async () => {
  await withOpened(async (db, home, token) => {
    const paths = oboetePaths(home);
    mkdirSync(paths.spool, { recursive: true });
    const entry = {
      repo: {
        id: 'repo1',
        identity_kind: 'common_dir',
        normalized_identity: '/tmp/oboete-spool',
        display_root: '/tmp/oboete-spool',
      },
      session: {
        id: 'sess-spool',
        repo_id: 'repo1',
        agent: 'claude',
        native_session_id: 'native-spool',
        conversation_id: 'sess-spool',
        started_at: NOW - DAY,
        status: 'active',
      },
      row: {
        id: 'spooled-1',
        repo_id: 'repo1',
        session_id: 'sess-spool',
        // The hook could not read the database, so a spool entry carries no turn (R7).
        turn_id: null,
        agent: 'claude',
        kind: 'prompt',
        content: 'a prompt that was spooled',
        truncated: 0,
        payload_json: null,
        content_hash: 'hash-1',
        sensitivity: 'local_only',
        classification_state: 'done',
        captured_at: NOW - DAY,
        expires_at: NOW + 7 * DAY,
      },
    };
    writeFileSync(join(paths.spool, `${NOW - DAY}-spooled-1.json`), JSON.stringify(entry));
    writeFileSync(join(paths.spool, `${NOW - DAY}-broken.json`), '{ not json');
    // A file that parses but is not an entry is not trusted into the database either (R4).
    writeFileSync(
      join(paths.spool, `${NOW - DAY}-shaped.json`),
      JSON.stringify({ id: 'x', repo_id: 'repo1', event: { kind: 'prompt' } }),
    );

    const first = recoverSpool(db, paths, token, NOW);
    assert.equal(first.inserted, 1);
    assert.equal(first.failed, 2);
    const stored = db.prepare('SELECT via_spool, turn_id FROM raw_events WHERE id = ?').get('spooled-1');
    assert.equal(Number(stored?.via_spool), 1);
    // FR-010: the recovered prompt opens the turn it would have opened on the direct path.
    const turn = db.prepare('SELECT id, ordinal FROM turns WHERE session_id = ?').get('sess-spool');
    assert.equal(Number(turn?.ordinal), 1);
    assert.equal(stored?.turn_id, turn?.id);
    assert.equal(Number(db.prepare('SELECT turn_count FROM sessions WHERE id = ?').get('sess-spool')?.turn_count), 1);
    assert.deepEqual(readdirSync(paths.spoolFailed).sort(), [
      `${NOW - DAY}-broken.json`,
      `${NOW - DAY}-shaped.json`,
    ]);

    // FR-003: the deterministic id makes a second recovery of the same file a no-op.
    writeFileSync(join(paths.spool, `${NOW - DAY}-spooled-1.json`), JSON.stringify(entry));
    const second = recoverSpool(db, paths, token, NOW);
    assert.equal(second.inserted, 0);
    assert.equal(second.skipped, 1);
    assert.equal(Number(db.prepare('SELECT COUNT(*) AS n FROM raw_events').get()?.n), 1);
    assert.equal(
      Number(db.prepare('SELECT COUNT(*) AS n FROM turns').get()?.n),
      1,
      'a repeated recovery opens no second turn',
    );
    assert.deepEqual(readdirSync(paths.spool).filter((name) => name.endsWith('.json')), []);
  });
});

test('a spool entry the database refuses is quarantined and the run continues', async () => {
  await withOpened(async (db, home, token) => {
    const paths = oboetePaths(home);
    mkdirSync(paths.spool, { recursive: true });
    const entry = (id: string, repoId: string): unknown => ({
      repo: {
        id: 'repo-spool',
        identity_kind: 'common_dir',
        normalized_identity: '/tmp/oboete-refused',
        display_root: '/tmp/oboete-refused',
      },
      session: {
        id: `sess-${id}`,
        repo_id: 'repo-spool',
        agent: 'claude',
        native_session_id: `native-${id}`,
        conversation_id: `sess-${id}`,
        started_at: NOW - DAY,
        status: 'active',
      },
      row: {
        // FR-003: the row's repository is the one the hook derived; an unknown one fails the
        // foreign key of raw_events, which must not stop the recovery of the other entries.
        id,
        repo_id: repoId,
        session_id: `sess-${id}`,
        turn_id: null,
        agent: 'claude',
        kind: 'prompt',
        content: `text of ${id}`,
        truncated: 0,
        payload_json: null,
        content_hash: `hash-${id}`,
        sensitivity: 'local_only',
        classification_state: 'done',
        captured_at: NOW - DAY,
        expires_at: NOW + 7 * DAY,
      },
    });
    writeFileSync(join(paths.spool, `${NOW - DAY}-refused.json`), JSON.stringify(entry('refused', 'ghost-repo')));
    writeFileSync(join(paths.spool, `${NOW - DAY}-sound.json`), JSON.stringify(entry('sound', 'repo-spool')));

    const result = recoverSpool(db, paths, token, NOW);

    assert.equal(result.failed, 1);
    assert.equal(result.inserted, 1, 'the entry after the refused one is still recovered');
    assert.deepEqual(readdirSync(paths.spoolFailed), [`${NOW - DAY}-refused.json`]);
    assert.deepEqual(
      db.prepare('SELECT id FROM raw_events').all().map((row) => String(row.id)),
      ['sound'],
    );
    assert.deepEqual(readdirSync(paths.spool).filter((name) => name.endsWith('.json')), []);
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
    assert.equal(input.turns.length, 12);
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
