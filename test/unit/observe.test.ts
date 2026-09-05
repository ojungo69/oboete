import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { PRESET_CATALOG } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { claimLease } from '../../src/worker/lease.js';
import { isStorageError } from '../../src/worker/observe.js';
import {
  DAY,
  NOW,
  captureEndedSession,
  catalogResponse,
  cleanEnv,
  eventBase,
  eventId,
  openAiResponse,
  providerOutput,
  runObserveForFixture,
  withFixture,
  workersResponse,
  writeConfig,
  type Fixture,
} from '../helpers/observe.js';

test('a constraint violation is a worker error, not unavailable storage', () => {
  const unique = Object.assign(new Error('UNIQUE constraint failed'), {
    code: 'ERR_SQLITE_ERROR',
    errcode: 2067,
  });
  const foreignKey = Object.assign(new Error('FOREIGN KEY constraint failed'), {
    code: 'ERR_SQLITE_ERROR',
    errcode: 787,
  });
  const readOnly = Object.assign(new Error('attempt to write a readonly database'), {
    code: 'ERR_SQLITE_ERROR',
    errcode: 8,
  });

  // contracts/cli.md: exit 3 means the data directory could not be used, which a rejected write is
  // not; treating it as one made every later run exit 3 as well.
  assert.equal(isStorageError(unique), false);
  assert.equal(isStorageError(foreignKey), false);
  assert.equal(isStorageError(readOnly), true);
  assert.equal(isStorageError(Object.assign(new Error('no space'), { code: 'ENOSPC' })), true);
});

test('no preset applies one fallback batch, writes a degraded session summary, releases, and checkpoints', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'fallback-session',
      prompts: ['Add retry handling.', 'Inspect the upload path.', 'Finish the change.'],
      tools: [
        { id: 'edit-one', path: 'src/upload.ts' },
        { id: 'edit-two', path: 'test/upload.test.ts' },
      ],
      assistant: 'The upload path now retries safely.',
    });

    assert.equal(await runObserveForFixture(fixture), 1);
    fixture.withDb((db) => {
      const batches = db
        .prepare('SELECT destination, state, degraded_reason FROM observation_batches')
        .all()
        .map((row) => ({ ...row }));
      assert.deepEqual(batches, [{ destination: 'fallback', state: 'fallback', degraded_reason: 'no_provider' }]);
      assert.ok(Number(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n) > 1);
      const summary = db.prepare("SELECT degraded_reason FROM memories WHERE type = 'session_summary'").get();
      assert.equal(summary?.degraded_reason, 'no_provider');
      assert.equal(db.prepare('SELECT summary_state FROM sessions').get()?.summary_state, 'done');
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
      assert.equal(db.prepare("SELECT citations_ok FROM memories WHERE type = 'change'").get()?.citations_ok, 0);
      const wal = db.prepare('PRAGMA wal_checkpoint(PASSIVE)').get();
      assert.equal(wal?.log, 0);
      assert.equal(wal?.checkpointed, 0);
    });
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /run start/);
    assert.match(log, /batch .*state=fallback reason=no_provider/);
    assert.match(log, /run end .*recovered=0 .*batches=1 .*fallback=1/);
    assert.equal(log.includes('The upload path now retries safely.'), false);
  });
});

test('Workers AI applies eligible rows and keeps local-only text out of the outbound body', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, {
      OBOETE_CF_API_TOKEN: 'worker-test-token',
      OBOETE_CF_ACCOUNT_ID: 'worker-test-account',
    });
    writeConfig(fixture, 'workers-ai', fixture.env);
    const remoteText = 'Describe the upload retry behavior.';
    const localText = 'LOCAL-ONLY-CONTENT';
    await captureEndedSession(fixture, {
      sessionId: 'remote-session',
      prompts: [remoteText],
      tools: [{ id: 'local-edit', path: 'src/local.ts', text: localText }],
      assistant: 'The upload path retries once.',
    });
    const sourceId = eventId(fixture, remoteText);
    fixture.withDb((db) => {
      db.prepare(
        "UPDATE raw_events SET sensitivity = 'local_only', classification_state = 'partial' WHERE content LIKE ?",
      ).run(`%${localText}%`);
    });

    let catalogCalls = 0;
    let providerCalls = 0;
    let providerBody = '';
    const fetchImpl: typeof fetch = async (input, init) => {
      if (String(input).includes('/models/search')) {
        catalogCalls += 1;
        return catalogResponse(Number(new URL(String(input)).searchParams.get('page') ?? '1'));
      }
      providerCalls += 1;
      providerBody = String(init?.body ?? '');
      return workersResponse(providerOutput(sourceId));
    };

    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 0);
    assert.equal(catalogCalls, 2);
    assert.equal(providerCalls, 1);
    assert.equal(providerBody.includes(localText), false);
    fixture.withDb((db) => {
      const batches = db
        .prepare('SELECT destination, state, degraded_reason FROM observation_batches ORDER BY destination')
        .all()
        .map((row) => ({ ...row }));
      assert.deepEqual(batches, [
        { destination: 'fallback', state: 'fallback', degraded_reason: 'rule_based' },
        { destination: 'remote_observer', state: 'applied', degraded_reason: null },
      ]);
      assert.equal(
        db.prepare("SELECT degraded_reason FROM memories WHERE source_batch_id = (SELECT id FROM observation_batches WHERE destination = 'remote_observer')").get()
          ?.degraded_reason,
        null,
      );
      assert.equal(db.prepare('SELECT calls FROM provider_usage').get()?.calls, 1);
    });
  });
});

test('a heartbeat failure stays contained when its log file is unavailable', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'worker-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'Keep heartbeat failures away from the worker loop.';
    await captureEndedSession(fixture, { sessionId: 'heartbeat-session', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    let failHeartbeat = false;
    let failedHeartbeatCalls = 0;

    const status = await runObserveForFixture(fixture, {
      heartbeatMs: 1,
      now: () => {
        if (failHeartbeat) {
          failedHeartbeatCalls += 1;
          throw new Error('heartbeat clock failed');
        }
        return NOW;
      },
      fetch: async () => openAiResponse(providerOutput(sourceId)),
      applyHook: async () => {
        rmSync(fixture.paths.observeLog);
        mkdirSync(fixture.paths.observeLog);
        failHeartbeat = true;
        await new Promise((resolve) => setTimeout(resolve, 20));
        failHeartbeat = false;
        rmSync(fixture.paths.observeLog, { recursive: true });
      },
    });

    assert.ok(failedHeartbeatCalls > 0, 'the heartbeat failure path did not run');
    assert.equal(status, 0);
  });
});

test('provider 429/3036 persists exhaustion and falls back without retrying', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    await captureEndedSession(fixture, {
      sessionId: 'exhausted-session',
      prompts: ['Record the retry behavior.'],
      assistant: 'The upload path retries once.',
    });
    let calls = 0;
    const fetchImpl: typeof fetch = async () => {
      calls += 1;
      return new Response(JSON.stringify({ error: { code: 3036, message: 'limit' } }), {
        status: 429,
        headers: { 'content-type': 'application/json' },
      });
    };

    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);
    assert.equal(calls, 1);
    fixture.withDb((db) => {
      const batch = db.prepare('SELECT state, degraded_reason, provider_attempts FROM observation_batches').get();
      assert.deepEqual({ ...batch }, { state: 'fallback', degraded_reason: 'provider_exhausted', provider_attempts: 1 });
      const usage = db.prepare('SELECT calls, exhausted_at FROM provider_usage').get();
      assert.equal(usage?.calls, 1);
      assert.equal(usage?.exhausted_at, NOW);
    });
  });
});

const privateResponseText = 'private-provider-body openrouter-test-key\nsecond line';
for (const [name, content, detail] of [
  ['malformed', `not JSON: ${privateResponseText}`, 'provider response was not valid JSON'],
  ['schema-invalid', JSON.stringify({ observations: [], [privateResponseText]: true }), 'provider response failed observation validation'],
  ['unknown-source', JSON.stringify(providerOutput(privateResponseText)), 'provider response failed observation validation'],
] as const) {
  test(`a ${name} provider response logs the fixed failure detail without its body or credentials`, async () => {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
      writeConfig(fixture, 'openrouter', fixture.env);
      await captureEndedSession(fixture, {
        sessionId: 'malformed-session',
        prompts: ['Record the retry behavior.'],
        assistant: 'The upload path retries once.',
      });
      let calls = 0;
      const fetchImpl: typeof fetch = async () => {
        calls += 1;
        return new Response(JSON.stringify({
          id: 'malformed-response',
          model: PRESET_CATALOG.openrouter.defaultModel,
          choices: [{
            index: 0,
            message: { role: 'assistant', content },
            finish_reason: 'stop',
          }],
        }), { status: 200, headers: { 'content-type': 'application/json' } });
      };

      assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);
      assert.equal(calls, 2);
      const log = readFileSync(fixture.paths.observeLog, 'utf8');
      assert.match(log, /batch .*state=fallback reason=unusable_output detail=/);
      assert.ok(log.includes(`detail=${JSON.stringify(detail)}\n`));
      assert.equal(log.includes('private-provider-body'), false);
      assert.equal(log.includes('openrouter-test-key'), false);
      assert.equal(log.includes('second line'), false);
    });
  });
}

test('a consent hash mismatch never calls the provider and records consent_changed', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env, 'invalid');
    await captureEndedSession(fixture, {
      sessionId: 'consent-session',
      prompts: ['Record the retry behavior.'],
      assistant: 'The upload path retries once.',
    });
    let calls = 0;
    assert.equal(
      await runObserveForFixture(fixture, {
        fetch: async () => {
          calls += 1;
          return openAiResponse(providerOutput('unused'));
        },
      }),
      1,
    );
    assert.equal(calls, 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT degraded_reason FROM observation_batches').get()?.degraded_reason, 'consent_changed');
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM provider_usage').get()?.n, 0);
    });
  });
});

// contracts/observer.md call policy 6 and R8: the tuple is recomputed from the configuration as it
// is on disk, not from the snapshot the run started with. A run lasts up to twenty minutes, so a
// developer who changes the destination or makes the file unreadable stops the next batch.
for (const variant of [
  { change: 'chooses no preset', write: (fixture: Fixture) => writeConfig(fixture, 'none') },
  {
    change: 'leaves the file unparsable',
    write: (fixture: Fixture) => writeFileSync(fixture.paths.config, '[observer\npreset = '),
  },
  {
    // The destination stays remote and only changes which one, so the stored hash is valid again:
    // the run must still stop, because it would keep sending to the preset it started with.
    change: 'accepts another remote preset',
    write: (fixture: Fixture) => writeConfig(fixture, 'workers-ai', fixture.env),
  },
]) {
  test(`a developer who ${variant.change} mid-run degrades the next batch with consent_changed`, async () => {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
      writeConfig(fixture, 'openrouter', fixture.env);
      const first = 'Record the retry behavior.';
      const second = 'Record the upload timeout.';
      for (const [index, prompt] of [first, second].entries()) {
        await captureEndedSession(fixture, {
          sessionId: `backlog-session-${index + 1}`,
          prompts: [prompt],
          assistant: 'The upload path retries once.',
        });
      }
      const sourceIds = { [first]: eventId(fixture, first), [second]: eventId(fixture, second) };

      let calls = 0;
      const fetchImpl: typeof fetch = async (_input, init) => {
        calls += 1;
        const body = String(init?.body ?? '');
        const answered = body.includes(first) ? first : second;
        variant.write(fixture);
        return openAiResponse(providerOutput(sourceIds[answered]));
      };

      const exit = await runObserveForFixture(fixture, { fetch: fetchImpl });
      assert.equal(calls, 1);
      assert.equal(exit, 1);
      fixture.withDb((db) => {
        assert.deepEqual(
          db
            .prepare('SELECT state, degraded_reason FROM observation_batches ORDER BY state')
            .all()
            .map((row) => ({ ...row })),
          [
            { state: 'applied', degraded_reason: null },
            { state: 'fallback', degraded_reason: 'consent_changed' },
          ],
        );
        // The check runs before the reservation as well, so the refused batch never counts an attempt.
        assert.equal(db.prepare('SELECT calls FROM provider_usage').get()?.calls, 1);
      });
    });
  });
}

test('two English answers for Japanese input fall back with language_mismatch', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'アップロード処理の再試行を記録してください。';
    await captureEndedSession(fixture, {
      sessionId: 'language-session',
      prompts: [prompt],
      assistant: 'アップロード処理は一回再試行します。',
    });
    const sourceId = eventId(fixture, prompt);
    let calls = 0;
    const fetchImpl: typeof fetch = async () => {
      calls += 1;
      return openAiResponse(providerOutput(sourceId));
    };

    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);
    assert.equal(calls, 2);
    fixture.withDb((db) => {
      const batch = db.prepare('SELECT state, degraded_reason, provider_attempts FROM observation_batches').get();
      assert.deepEqual({ ...batch }, { state: 'fallback', degraded_reason: 'language_mismatch', provider_attempts: 2 });
      assert.equal(db.prepare('SELECT calls FROM provider_usage').get()?.calls, 2);
    });
  });
});

test('a fully private session becomes no_content and is not revisited', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const common = eventBase('private-session');
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', {
      ...common,
      prompt_id: 'private-prompt',
      prompt: '<private>この内容は保存しません。</private>',
    });
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });

    assert.equal(await runObserveForFixture(fixture), 0);
    const first = fixture.withDb((db) => ({
      batches: Number(db.prepare('SELECT COUNT(*) AS n FROM observation_batches').get()?.n),
      memories: Number(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n),
      runtime: Number(db.prepare('SELECT COUNT(*) AS n FROM runtime_state').get()?.n),
      state: db.prepare('SELECT summary_state FROM sessions').get()?.summary_state,
    }));
    assert.deepEqual(first, { batches: 0, memories: 0, runtime: 1, state: 'no_content' });
    assert.equal(await runObserveForFixture(fixture), 0);
    const second = fixture.withDb((db) => ({
      batches: Number(db.prepare('SELECT COUNT(*) AS n FROM observation_batches').get()?.n),
      memories: Number(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n),
      runtime: Number(db.prepare('SELECT COUNT(*) AS n FROM runtime_state').get()?.n),
      state: db.prepare('SELECT summary_state FROM sessions').get()?.summary_state,
    }));
    assert.deepEqual(second, first);
  });
});

test('a live foreign lease makes observe exit without changing queued work', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'held-session',
      prompts: ['Queued work remains untouched.'],
    });
    const held = openDatabase({ path: fixture.paths.db, timeoutMs: 1_000 }).db;
    try {
      const token = claimLease(held, { pid: 999, now: NOW });
      if (token === null) assert.fail('expected the foreign lease');
      assert.equal(await runObserveForFixture(fixture), 0);
      const state = held.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get();
      assert.equal(state?.owner_token, token);
      assert.equal(held.prepare('SELECT COUNT(*) AS n FROM observation_batches').get()?.n, 0);
      assert.equal(held.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
      assert.equal(held.prepare('SELECT summary_state FROM sessions').get()?.summary_state, 'pending');
    } finally {
      held.close();
    }
  });
});

test('a lease stolen after the provider response discards the apply and exits zero', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'Record the fenced apply behavior.';
    await captureEndedSession(fixture, {
      sessionId: 'stolen-session',
      prompts: [prompt],
    });
    const sourceId = eventId(fixture, prompt);
    let now = NOW;
    let thiefToken: string | null = null;

    const exit = await runObserveForFixture(fixture, {
      now: () => now,
      fetch: async () => openAiResponse(providerOutput(sourceId)),
      applyHook: () => {
        now += 7_000;
        const thief = openDatabase({ path: fixture.paths.db, timeoutMs: 1_000 }).db;
        try {
          thiefToken = claimLease(thief, { pid: 998, now });
          if (thiefToken === null) assert.fail('expected the stale lease to be stolen');
        } finally {
          thief.close();
        }
      },
    });
    assert.equal(exit, 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
      assert.equal(db.prepare('SELECT state FROM observation_batches').get()?.state, 'running');
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, thiefToken);
    });
  });
});

test('a crash after response leaves running work that is reclaimed once with two calls and one apply', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'openrouter-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'Record the crash recovery behavior.';
    await captureEndedSession(fixture, {
      sessionId: 'crash-session',
      prompts: [prompt],
    });
    const sourceId = eventId(fixture, prompt);
    let now = NOW;
    let fetchCalls = 0;
    let throwOnce = true;
    const fetchImpl: typeof fetch = async () => {
      fetchCalls += 1;
      return openAiResponse(providerOutput(sourceId));
    };
    const applyHook = () => {
      if (!throwOnce) return;
      throwOnce = false;
      throw new Error('simulated worker kill');
    };

    assert.equal(await runObserveForFixture(fixture, { now: () => now, fetch: fetchImpl, applyHook }), 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT state FROM observation_batches').get()?.state, 'running');
      assert.equal(db.prepare('SELECT provider_attempts FROM observation_batches').get()?.provider_attempts, 1);
      assert.equal(typeof db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, 'string');
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
    });

    now += 120_001;
    assert.equal(await runObserveForFixture(fixture, { now: () => now, fetch: fetchImpl, applyHook }), 0);
    assert.equal(fetchCalls, 2);
    fixture.withDb((db) => {
      const batch = db.prepare('SELECT state, provider_attempts FROM observation_batches').get();
      assert.deepEqual({ ...batch }, { state: 'applied', provider_attempts: 2 });
      assert.equal(
        db.prepare('SELECT COUNT(*) AS n FROM memories WHERE source_batch_id IS NOT NULL').get()?.n,
        1,
      );
      assert.equal(db.prepare('SELECT calls FROM provider_usage').get()?.calls, 2);
    });
  });
});

test('maxRunMs releases the lease and leaves pending work for the next hook', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'bounded-session',
      prompts: ['Leave this work queued.'],
    });
    let calls = 0;
    const now = () => (calls++ === 0 ? NOW : NOW + 1);
    assert.equal(await runObserveForFixture(fixture, { now, maxRunMs: 1 }), 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
      assert.equal(db.prepare('SELECT batch_id FROM raw_events WHERE kind = ?').get('prompt')?.batch_id, null);
      assert.equal(db.prepare('SELECT summary_state FROM sessions').get()?.summary_state, 'pending');
    });
  });
});

test('a session of lifecycle rows only is no queued work and the run ends empty', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    // No summarizer can use a lifecycle row, so the queue check must not read it as work; an
    // advancing clock makes a wrong answer end the run as max_run instead of hanging the test.
    await fixture.capture('SessionStart', { ...eventBase('lifecycle-session'), source: 'startup' });
    let clock = NOW;
    assert.equal(await runObserveForFixture(fixture, { now: () => clock++, maxRunMs: 5_000 }), 0);

    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
      assert.equal(Number(db.prepare('SELECT COUNT(*) AS n FROM raw_events').get()?.n), 1);
    });
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=empty/);
  });
});

test('a prompt of non-ASCII blanks is no queued work and the run ends empty', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    // U+3000 and U+00A0 are whitespace to `String.prototype.trim`, so `isSummarizableRow` never
    // batches this row and purge never deletes it. The queue check has to call it blank too, or the
    // worker treats it as work on every pass for the whole run (FR-002: the worker runs beside the
    // hooks). The advancing clock turns a wrong answer into `max_run` instead of a hanging test.
    await fixture.capture('SessionStart', { ...eventBase('blank-session'), source: 'startup' });
    await fixture.capture('UserPromptSubmit', {
      ...eventBase('blank-session'),
      prompt: '\u3000\u00a0',
    });
    let clock = NOW;
    assert.equal(await runObserveForFixture(fixture, { now: () => clock++, maxRunMs: 5_000 }), 0);

    fixture.withDb((db) => {
      assert.equal(Number(db.prepare('SELECT COUNT(*) AS n FROM observation_batches').get()?.n), 0);
    });
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=empty/);
  });
});

test('the loop purges expired terminal rows and folds Pi done acknowledgements', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await fixture.capture('SessionStart', { ...eventBase('purge-session'), source: 'startup' });
    fixture.withDb((db) => {
      const session = db.prepare('SELECT id, repo_id FROM sessions').get();
      if (typeof session?.id !== 'string' || typeof session.repo_id !== 'string') {
        assert.fail('expected the captured session');
      }
      db.prepare(
        `INSERT INTO observation_batches
           (id, repo_id, session_id, through_event_id, destination, trigger, state, provider_attempts, completed_at)
         VALUES ('expired-batch', ?, ?, 'expired-event', 'fallback', 'retention', 'applied', 0, ?)`,
      ).run(session.repo_id, session.id, NOW - 1);
      db.prepare(
        `INSERT INTO raw_events
           (id, repo_id, session_id, kind, content, sensitivity, classification_state, captured_at, expires_at, batch_id)
         VALUES ('expired-event', ?, ?, 'prompt', 'expired body', 'local_only', 'done', ?, ?, 'expired-batch')`,
      ).run(session.repo_id, session.id, NOW - 2 * DAY, NOW - 1);
    });
    mkdirSync(fixture.paths.piAck, { recursive: true });
    const done = join(fixture.paths.piAck, 'finished.done');
    writeFileSync(done, 'must-not-be-read');

    assert.equal(await runObserveForFixture(fixture), 0);
    assert.equal(existsSync(done), false);
    fixture.withDb((db) => {
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE id = 'expired-event'").get()?.n, 0);
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
  });
});
