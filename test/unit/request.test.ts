import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { configSchema, consentHash, consentMatches, consentTuple } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { nearbyCandidates } from '../../src/db/queries.js';
import { buildObserverRequest } from '../../src/observer/request.js';
import { oboetePaths } from '../../src/paths.js';
import { loadDestinationRules } from '../../src/privacy/egress.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import {
  createBatches,
  loadBatchInput,
  type BatchDestination,
  type RawEventRow,
  type SessionRow,
  type TurnRow,
} from '../../src/worker/batches.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_757_000_000_000;
const DAY = 24 * 60 * 60 * 1000;
const REPO_ID = 'a1b2c3d4e5f60718';
const NORMALIZED_IDENTITY = 'github.com/example/uploader-service';
const CWD = '/home/somebody/work/uploader-service';

async function withOpened(
  fn: (db: DatabaseSync, token: string) => void | Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      const token = claimLease(opened.db, { pid: 1, now: NOW });
      if (token === null) assert.fail('expected a lease token');
      await fn(opened.db, token);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

function seedRepoAndSession(db: DatabaseSync, turns: number): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, 'remote', ?, ?, 1, 1)`,
  ).run(REPO_ID, NORMALIZED_IDENTITY, CWD);
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, started_at, status, turn_count)
     VALUES ('sess1', ?, 'claude', 'native-1', 'sess1', ?, 'active', ?)`,
  ).run(REPO_ID, NOW - DAY, turns);
  for (let ordinal = 1; ordinal <= turns; ordinal += 1) {
    db.prepare(
      'INSERT INTO turns (id, session_id, ordinal, started_at, ended_at) VALUES (?, ?, ?, ?, ?)',
    ).run(`t${ordinal}`, 'sess1', ordinal, NOW - DAY + ordinal, ordinal === turns ? null : NOW - DAY + ordinal + 1);
  }
}

let capturedCounter = 0;

function seedEvent(
  db: DatabaseSync,
  seed: {
    id: string;
    kind: string;
    content: string | null;
    sensitivity?: string;
    state?: string;
    payload?: unknown;
    turn?: number;
  },
): void {
  capturedCounter += 1;
  db.prepare(
    `INSERT INTO raw_events
       (id, repo_id, session_id, turn_id, agent, kind, content, payload_json, sensitivity,
        classification_state, captured_at, expires_at)
     VALUES (?, ?, 'sess1', ?, 'claude', ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    seed.id,
    REPO_ID,
    `t${seed.turn ?? 1}`,
    seed.kind,
    seed.content,
    seed.payload === undefined ? null : JSON.stringify(seed.payload),
    seed.sensitivity ?? 'eligible',
    seed.state ?? 'done',
    NOW - DAY + capturedCounter,
    NOW + 7 * DAY,
  );
}

function seedMemory(
  db: DatabaseSync,
  seed: { id: string; title: string; body: string; sensitivity: string; deleted?: boolean },
): void {
  db.prepare(
    `INSERT INTO memories
       (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash,
        sensitivity, review_state, valid_from, deleted_at, created_at)
     VALUES (?, ?, 'discovery', ?, ?, '[]', ?, ?, ?, ?, 'unreviewed', ?, ?, ?)`,
  ).run(
    seed.id,
    REPO_ID,
    seed.title,
    seed.body,
    cjkBigrams(`${seed.title} ${seed.body}`),
    `material-${seed.id}`,
    `content-${seed.id}`,
    seed.sensitivity,
    NOW - DAY,
    seed.deleted === true ? NOW - DAY : null,
    NOW - DAY,
  );
}

/** Ten turns of mixed sensitivity, with a marker word per class so the body can be searched. */
function seedMixedBatch(db: DatabaseSync): void {
  seedRepoAndSession(db, 12);
  seedEvent(db, { id: 'r1', kind: 'prompt', content: 'Add a retry to the uploader.', turn: 1 });
  // The shape capture writes: the free text of the call lives in `content`, and `payload_json`
  // keeps the normalized fields without it (src/capture.ts payloadJson).
  seedEvent(db, {
    id: 'r2',
    kind: 'tool_call',
    content: 'grep -rn uploader src',
    payload: { tool_name: 'grep', input: { paths: ['src/uploader.ts'] } },
    turn: 2,
  });
  seedEvent(db, {
    id: 'r13',
    kind: 'tool_call',
    content: 'git log --oneline -5',
    payload: { tool_name: 'bash', input: { paths: [] } },
    turn: 2,
  });
  seedEvent(db, {
    id: 'r14',
    kind: 'tool_call',
    content: 'BASHLOCALMARKER cat .env',
    payload: { tool_name: 'bash', input: { paths: [] } },
    sensitivity: 'local_only',
    turn: 5,
  });
  seedEvent(db, { id: 'r3', kind: 'tool_result', content: 'three matches in the uploader', turn: 3 });
  seedEvent(db, {
    id: 'r4',
    kind: 'last_assistant_message',
    content: 'The uploader now retries three times.',
    turn: 4,
  });
  seedEvent(db, {
    id: 'r5',
    kind: 'prompt',
    content: 'LOCALONLYMARKER a note that stays here',
    sensitivity: 'local_only',
    turn: 5,
  });
  seedEvent(db, {
    id: 'r6',
    kind: 'prompt',
    content: 'PRIVATEMARKER a private note',
    sensitivity: 'private',
    turn: 6,
  });
  seedEvent(db, {
    id: 'r7',
    kind: 'prompt',
    content: 'SECRETMARKER a secret note',
    sensitivity: 'secret',
    turn: 7,
  });
  seedEvent(db, {
    id: 'r8',
    kind: 'tool_call',
    content: 'PARTIALMARKER the payload was cut',
    sensitivity: 'local_only',
    state: 'partial',
    payload: { tool_name: 'read', input: { paths: ['src/PARTIALPATH.ts'] } },
    turn: 8,
  });
  seedEvent(db, { id: 'r9', kind: 'prompt', content: null, sensitivity: 'local_only', state: 'failed', turn: 9 });
  seedEvent(db, { id: 'r10', kind: 'prompt', content: 'And add a test for the uploader.', turn: 10 });
  seedEvent(db, { id: 'r11', kind: 'prompt', content: 'Run the uploader test suite.', turn: 11 });
  seedEvent(db, { id: 'r12', kind: 'prompt', content: 'Document the uploader retry.', turn: 12 });
  seedMemory(db, {
    id: 'm-eligible',
    title: 'The uploader retries',
    body: 'The uploader retries three times before it gives up.',
    sensitivity: 'eligible',
  });
  seedMemory(db, {
    id: 'm-local',
    title: 'LOCALMEMORYMARKER uploader note',
    body: 'A local uploader note about retries that a remote observer must never receive.',
    sensitivity: 'local_only',
  });
  seedMemory(db, {
    id: 'm-secret',
    title: 'SECRETMEMORYMARKER uploader credential',
    body: 'A secret uploader note about retries.',
    sensitivity: 'secret',
  });
}

/** Through the real path: classify, batch, load the batch, build its request. */
function buildFromBatch(db: DatabaseSync, token: string, destination: BatchDestination) {
  createBatches(db, token, NOW, { preset: destination === 'local_observer' ? 'local' : 'remote' });
  const batchId = db
    .prepare('SELECT id FROM observation_batches WHERE destination = ?')
    .get(destination)?.id;
  if (typeof batchId !== 'string') assert.fail(`expected a ${destination} batch`);
  const loaded = loadBatchInput(db, batchId);
  if (loaded === null) assert.fail('expected the batch to load');
  return build(db, destination, loaded.rows, loaded.session, loaded.turns);
}

/**
 * Every row of the session, batched or not. The builder is the outbound boundary, so it has to
 * refuse what it may not send even when a caller hands it everything (FR-023, SC-006).
 */
function buildFromEveryRow(db: DatabaseSync, destination: BatchDestination) {
  const rows = db
    .prepare('SELECT * FROM raw_events ORDER BY captured_at, id')
    .all() as unknown as RawEventRow[];
  const session = db.prepare('SELECT * FROM sessions WHERE id = ?').get('sess1') as unknown as SessionRow;
  const turns = db
    .prepare('SELECT id, ordinal, started_at, ended_at FROM turns ORDER BY ordinal')
    .all() as unknown as TurnRow[];
  return build(db, destination, rows, session, turns);
}

function build(
  db: DatabaseSync,
  destination: BatchDestination,
  rows: RawEventRow[],
  session: SessionRow,
  turns: TurnRow[],
) {
  if (destination === 'fallback') assert.fail('the fallback builds no request');
  return buildObserverRequest({
    rows,
    session,
    turns,
    destination,
    repoId: REPO_ID,
    nearby: nearbyCandidates(db, { repoId: REPO_ID, text: 'uploader retries' }),
    rules: loadDestinationRules(db),
  });
}

test('the outbound body of a mixed batch carries the eligible rows and nothing else', async () => {
  await withOpened((db) => {
    seedMixedBatch(db);
    const built = buildFromEveryRow(db, 'remote_observer');
    const body = JSON.stringify(built.input);

    // Fail open: every eligible row is delivered (FR-023).
    assert.equal(body.includes('Add a retry to the uploader.'), true);
    assert.equal(body.includes('three matches in the uploader'), true);
    assert.equal(body.includes('The uploader now retries three times.'), true);
    assert.equal(body.includes('src/uploader.ts'), true);
    // The command and the free text of a tool call live in `content`, so the request has to put
    // them back or the summarizer sees paths where the call had a command (FR-015).
    assert.equal(body.includes('grep -rn uploader src'), true);
    assert.equal(body.includes('git log --oneline -5'), true);

    // Fail closed: SC-006, nothing below `eligible` appears anywhere in the body.
    for (const marker of [
      'LOCALONLYMARKER',
      'PRIVATEMARKER',
      'SECRETMARKER',
      'PARTIALMARKER',
      'PARTIALPATH',
      'BASHLOCALMARKER',
    ]) {
      assert.equal(body.includes(marker), false, `${marker} must not reach a remote observer`);
    }
    assert.deepEqual(
      built.dropped.filter((item) => item.reason === 'partial').map((item) => item.rowId),
      ['r8'],
    );
    assert.deepEqual(
      built.dropped.filter((item) => item.reason === 'sensitivity').map((item) => item.rowId).sort(),
      ['m-local', 'm-secret', 'r14', 'r5', 'r6', 'r7'],
    );

    // R10: the repository travels as an opaque id, never as its identity or a path.
    assert.equal(built.input.repo_ref, REPO_ID);
    assert.equal(body.includes(NORMALIZED_IDENTITY), false);
    assert.equal(body.includes(CWD), false);

    // SC-006: the producing agent is provenance only and absent from the request.
    assert.equal(/\b(claude|codex|grok|pi)\b/i.test(body), false);

    // The nearby list carries only what the destination may receive.
    assert.deepEqual(built.input.nearby.map((item) => item.id), ['m-eligible']);
    assert.equal(built.excerpted, false);
    assert.equal(built.input.language_hint, 'en');
    assert.deepEqual(
      built.input.session.turns.map((turn) => turn.ordinal),
      [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
    );
    assert.equal(built.input.free_summaries.last_assistant_message, 'The uploader now retries three times.');
  });
});

test('a local observer receives the local-only and private rows and their nearby memories', async () => {
  await withOpened((db) => {
    seedMixedBatch(db);
    const built = buildFromEveryRow(db, 'local_observer');
    const body = JSON.stringify(built.input);

    assert.equal(body.includes('LOCALONLYMARKER'), true);
    assert.equal(body.includes('PRIVATEMARKER'), true);
    assert.equal(body.includes('BASHLOCALMARKER cat .env'), true);
    // FR-020: a secret row reaches no destination at all, local or remote.
    assert.equal(body.includes('SECRETMARKER'), false);
    assert.equal(body.includes('PARTIALMARKER'), false);
    assert.deepEqual(built.input.nearby.map((item) => item.id).sort(), ['m-eligible', 'm-local']);
  });
});

test('only the agent column changes and the outbound body stays byte-identical', async () => {
  await withOpened((db, token) => {
    seedMixedBatch(db);
    const first = JSON.stringify(buildFromBatch(db, token, 'remote_observer').input);

    db.exec("UPDATE raw_events SET agent = 'grok'");
    db.exec("UPDATE sessions SET agent = 'grok'");
    db.exec('UPDATE raw_events SET batch_id = NULL');
    db.exec('DELETE FROM observation_batches');
    const second = JSON.stringify(buildFromBatch(db, token, 'remote_observer').input);

    assert.equal(second, first);
  });
});

test('a Japanese batch is labelled ja and an oversized batch is excerpted', async () => {
  await withOpened((db, token) => {
    seedRepoAndSession(db, 10);
    seedEvent(db, { id: 'j1', kind: 'prompt', content: 'アップローダーに再試行を追加してください。', turn: 1 });
    seedEvent(db, {
      id: 'j2',
      kind: 'tool_result',
      content: `アップローダーの再試行を確認しました。${'確認しました。'.repeat(3_000)}`,
      turn: 2,
    });
    for (let turn = 3; turn <= 10; turn += 1) {
      seedEvent(db, { id: `j${turn}`, kind: 'prompt', content: `${turn} 番目の確認です。`, turn });
    }

    const built = buildFromBatch(db, token, 'remote_observer');
    assert.equal(built.input.language_hint, 'ja');
    // FR-015: the input is bounded to 12,000 characters and the excerpting is recorded.
    assert.equal(built.excerpted, true);
    assert.ok(JSON.stringify(built.input).length <= 12_000);
  });
});

test('the consent gate refuses a stored hash that no longer describes the configuration', async () => {
  const env = { OBOETE_CF_API_TOKEN: 'token-value', OBOETE_CF_ACCOUNT_ID: 'account-value' };
  const live = configSchema.parse({ observer: { preset: 'workers-ai' } });
  const accepted = configSchema.parse({
    observer: { preset: 'workers-ai' },
    consent: { hash: consentHash(consentTuple(live, env)), accepted_at: NOW },
  });
  assert.equal(consentMatches(accepted, env), true);

  const changed = configSchema.parse({
    observer: { preset: 'openrouter' },
    consent: { hash: consentHash(consentTuple(live, env)), accepted_at: NOW },
  });
  assert.equal(consentMatches(changed, env), false);
});
