import assert from 'node:assert/strict';
import { chmodSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

import { nextUtcMidnight, recordExhausted, utcDay } from '../../src/observer/reservation.js';
import { DAILY_CAP } from '../../src/observer/reservation.js';
import { detectSync } from '../../src/privacy/detect.js';
import { cliSpawn } from '../helpers/agent-cli.js';
import {
  NOW,
  captureEndedSession,
  catalogResponse,
  cleanEnv,
  eventId,
  openAiResponse,
  providerOutput,
  runObserveForFixture,
  withFixture,
  writeChainConfig,
  type Fixture,
} from '../helpers/observe.js';

const OLLAMA_MODEL = 'qwen2.5:7b';

/** Workers AI credentials plus an NVIDIA key, so a listed `nim` target fails for policy, not secrets. */
function chainEnv(fixture: Fixture): NodeJS.ProcessEnv {
  return cleanEnv(fixture.home, {
    OBOETE_CF_API_TOKEN: 'chain-test-token',
    OBOETE_CF_ACCOUNT_ID: 'chain-test-account',
    OBOETE_NIM_API_KEY: 'nvapi-chain-test',
  });
}

type Hosts = { cloudflare: number; ollama: number; nim: number; catalog: number };

/**
 * Counts requests per host so a target that must not be attempted can be asserted at zero, and
 * answers each host from `plan` in the order the chain reaches it.
 */
function chainFetch(
  hosts: Hosts,
  plan: { cloudflare?: () => Promise<Response>; ollama?: () => Promise<Response>; nim?: () => Promise<Response> },
): typeof fetch {
  return async (input) => {
    // Matched on the parsed origin, never on a substring of the whole URL: a host is only this
    // target's host when it is the authority, not when it appears in a path or a query.
    const url = new URL(String(input));
    if (url.pathname.endsWith('/models/search')) {
      hosts.catalog += 1;
      return catalogResponse(Number(url.searchParams.get('page') ?? '1'));
    }
    if (url.host === 'api.cloudflare.com') {
      hosts.cloudflare += 1;
      return await (plan.cloudflare ?? (() => assert.fail('the Workers AI target was not expected')))();
    }
    if (url.host === '127.0.0.1:11434') {
      hosts.ollama += 1;
      return await (plan.ollama ?? (() => assert.fail('the ollama target was not expected')))();
    }
    if (url.host === 'integrate.api.nvidia.com') {
      hosts.nim += 1;
      return await (plan.nim ?? (() => assert.fail('the nim target was not expected')))();
    }
    return assert.fail(`an unexpected host was called: ${url.href}`);
  };
}

function counters(): Hosts {
  return { cloudflare: 0, ollama: 0, nim: 0, catalog: 0 };
}

function batchRows(fixture: Fixture): Record<string, unknown>[] {
  let rows: Record<string, unknown>[] = [];
  fixture.withDb((db) => {
    rows = db.prepare(`SELECT destination, state, degraded_reason, provider_attempts
      FROM observation_batches ORDER BY destination`).all().map((row) => ({ ...row }));
  });
  return rows;
}

test('an exhausted primary hands the same batch to the next admitted target', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record how the retry reaches the second target.';
    await captureEndedSession(fixture, { sessionId: 'chain-exhausted', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    fixture.withDb((db) => {
      db.prepare(`INSERT INTO provider_usage (utc_day, preset, calls, neurons_estimate, reset_at)
        VALUES (?, 'workers-ai', 1, 0, ?)`).run(utcDay(NOW), nextUtcMidnight(NOW));
      recordExhausted(db, { preset: 'workers-ai', reservationId: 'chain-exhausted', now: NOW });
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, { ollama: async () => openAiResponse(providerOutput(sourceId), OLLAMA_MODEL) });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 0);

    // The exhausted preset is refused at its own reservation, so its host is never reached.
    assert.equal(hosts.cloudflare, 0);
    assert.equal(hosts.ollama, 1);
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'applied', degraded_reason: null, provider_attempts: 1 },
    ]);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=provider_exhausted/);
    assert.match(log, /batch .*state=applied/);
  });
});

test('the daily cap advances past every capped target and stops at none of the local ones', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      costPolicy: ['free-tier', 'local', 'remote'],
      fallback: [{ preset: 'nim' }, { preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record the behaviour once the shared allowance is gone.';
    await captureEndedSession(fixture, { sessionId: 'chain-capped', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    fixture.withDb((db) => {
      db.prepare(`INSERT INTO provider_usage (utc_day, preset, calls, neurons_estimate, reset_at)
        VALUES (?, 'workers-ai', ?, 0, ?)`).run(utcDay(NOW), DAILY_CAP, nextUtcMidnight(NOW));
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, { ollama: async () => openAiResponse(providerOutput(sourceId), OLLAMA_MODEL) });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 0);

    // The cap is one allowance summed over capped presets, so nim cannot pass a reservation either.
    assert.equal(hosts.cloudflare, 0);
    assert.equal(hosts.nim, 0);
    assert.equal(hosts.ollama, 1);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=daily_cap/);
    assert.match(log, /provider attempt .*position=1 preset=nim model=[^ ]+ reason=daily_cap/);
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'applied', degraded_reason: null, provider_attempts: 1 },
    ]);
  });
});

test('a target with no credentials is attempted, answers without a request and the chain moves on', async () => {
  await withFixture(async (fixture) => {
    // No OBOETE_NIM_API_KEY: credentials are not an admission test, so nim is attempted and fails.
    fixture.env = cleanEnv(fixture.home, {
      OBOETE_CF_API_TOKEN: 'chain-test-token',
      OBOETE_CF_ACCOUNT_ID: 'chain-test-account',
    });
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      costPolicy: ['free-tier', 'local', 'remote'],
      fallback: [{ preset: 'nim' }, { preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record what an unset credential variable does to the chain.';
    await captureEndedSession(fixture, { sessionId: 'chain-uncredentialed', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      cloudflare: async () => new Response('upstream is unavailable', { status: 503 }),
      ollama: async () => openAiResponse(providerOutput(sourceId), OLLAMA_MODEL),
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 0);

    assert.equal(hosts.nim, 0);
    assert.equal(hosts.ollama, 1);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=1 preset=nim model=[^ ]+ reason=no_provider/);
  });
});

test('every target failing settles once, keeps the worst reason and leaves the source retryable', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record what happens when no target answers at all.';
    await captureEndedSession(fixture, { sessionId: 'chain-all-failed', prompts: [prompt] });
    fixture.withDb((db) => {
      db.prepare(`INSERT INTO provider_usage (utc_day, preset, calls, neurons_estimate, reset_at)
        VALUES (?, 'workers-ai', 1, 0, ?)`).run(utcDay(NOW), nextUtcMidnight(NOW));
      recordExhausted(db, { preset: 'workers-ai', reservationId: 'chain-all-failed', now: NOW });
    });

    const hosts = counters();
    // provider_exhausted outranks unreachable in DEGRADED_PRECEDENCE, so the batch keeps the former.
    const fetchImpl = chainFetch(hosts, { ollama: async () => { throw new Error('connection refused'); } });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);

    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'fallback', degraded_reason: 'provider_exhausted', provider_attempts: 1 },
    ]);
    fixture.withDb((db) => {
      const source = db.prepare(`SELECT processing_state, processing_attempts, retry_after
        FROM raw_events WHERE kind = 'prompt'`).get();
      assert.equal(source?.processing_state, 'waiting');
      // N targets are one attempt at the batch, so the source is not punished per target.
      assert.equal(source?.processing_attempts, 1);
      assert.notEqual(source?.retry_after, null);
    });
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=provider_exhausted/);
    assert.match(log, /provider attempt .*position=1 preset=ollama model=[^ ]+ reason=unreachable/);
  });
});

test('an unusable answer stops the chain instead of spending a second allowance on it', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    await captureEndedSession(fixture, {
      sessionId: 'chain-unusable',
      prompts: ['Record what an unusable answer does to the chain.'],
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      cloudflare: async () => new Response(JSON.stringify({ success: true, result: { response: 'not an observation' },
        errors: [], messages: [] }), { status: 200, headers: { 'content-type': 'application/json' } }),
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);

    // The request reached a provider and was answered; the chain is for failures to get an answer.
    assert.equal(hosts.ollama, 0);
    // Two reservations for one target: llm.ts already retries an unusable answer once, which is
    // exactly the retry the chain must not duplicate.
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'fallback', degraded_reason: 'unusable_output', provider_attempts: 2 },
    ]);
  });
});

test('a target whose answer is refused during apply has its own attempt line', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record what a refused checkpoint does to the attempt lines.';
    await captureEndedSession(fixture, { sessionId: 'chain-apply-refused', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    fixture.withDb((db) => {
      db.prepare(`INSERT INTO provider_usage (utc_day, preset, calls, neurons_estimate, reset_at)
        VALUES (?, 'workers-ai', 1, 0, ?)`).run(utcDay(NOW), nextUtcMidnight(NOW));
      recordExhausted(db, { preset: 'workers-ai', reservationId: 'chain-apply-refused', now: NOW });
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      ollama: async () => openAiResponse({
        ...providerOutput(sourceId),
        checkpoint: { decision: 'replace', purpose: 'Verify the refused checkpoint', constraints: [],
          decisions: [], outstanding: ['Check the attempt line.'], source_event_ids: [sourceId],
          reason: 'Retained progress.' },
      }, OLLAMA_MODEL),
    });
    // The detector refuses the checkpoint text only, so the request itself passes its final check and
    // the second target answers; `applyObservations` then rejects the required progress decision and
    // mints `unusable_output` after the call, where no reservation ties it to a target.
    assert.equal(await runObserveForFixture(fixture, {
      fetch: fetchImpl,
      detect: async (input) => input.text.startsWith('Purpose\n')
        ? { ok: false, reason: 'detector_error' }
        : await detectSync(input),
    }), 1);

    assert.equal(hosts.ollama, 1);
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'fallback', degraded_reason: 'unusable_output', provider_attempts: 1 },
    ]);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=provider_exhausted/);
    assert.match(log, /provider attempt .*position=1 preset=ollama model=[^ ]+ reason=unusable_output/);
  });
});

test('a local target is never given a batch a remote target could not have been given', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    // A local primary with a remote fallback is refused at the resolve, so the run has no provider.
    writeChainConfig(fixture, {
      preset: 'ollama',
      model: OLLAMA_MODEL,
      costPolicy: ['free-tier', 'local', 'remote'],
      fallback: [{ preset: 'nim' }],
      env: fixture.env,
    });
    await captureEndedSession(fixture, {
      sessionId: 'chain-widened',
      prompts: ['Record that a local selection cannot reach the network.'],
    });

    const hosts = counters();
    assert.equal(await runObserveForFixture(fixture, { fetch: chainFetch(hosts, {}) }), 1);
    assert.equal(hosts.nim, 0);
    assert.equal(hosts.ollama, 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT degraded_reason FROM observation_batches').get()?.degraded_reason, 'no_provider');
    });
  });
});

test('a consent change between targets stops the chain before the next host', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    await captureEndedSession(fixture, {
      sessionId: 'chain-consent',
      prompts: ['Record that a withdrawn consent stops the chain.'],
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      // The stored record stops describing the live configuration while the first target is in flight.
      cloudflare: async () => {
        writeChainConfig(fixture, {
          preset: 'workers-ai',
          fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
          env: fixture.env,
          consent: 'invalid',
        });
        throw new Error('connection refused');
      },
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);

    assert.equal(hosts.ollama, 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT degraded_reason FROM observation_batches').get()?.degraded_reason,
        'consent_changed');
    });
  });
});

test('lease loss at the next reservation keeps the earlier provider attempt in the log', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    await captureEndedSession(fixture, {
      sessionId: 'chain-lease-lost',
      prompts: ['Record the provider attempt spent before the lease was lost.'],
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      cloudflare: async () => {
        // Target 1 reaches reserveAttempt after this failure; its lease_lost result makes
        // providerCall throw after target 0 has already spent its reservation.
        fixture.withDb((db) => {
          db.prepare("UPDATE worker_lease SET owner_token = 'chain-lease-thief' WHERE id = 1").run();
        });
        return new Response('unauthorized', { status: 401 });
      },
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 0);

    assert.equal(hosts.cloudflare, 1);
    assert.equal(hosts.ollama, 0);
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'running', degraded_reason: null, provider_attempts: 1 },
    ]);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /end .*reason=lease_lost/);
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=auth_failed/,
      'lease loss at the next reservation must retain the earlier provider attempt');
    assert.equal(log.match(/provider attempt /g)?.length, 1);
  });
});

test('a target that answers after two failures applies its output like any other', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      costPolicy: ['free-tier', 'local', 'remote'],
      fallback: [{ preset: 'nim' }, { preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record that the third target still applies normally.';
    await captureEndedSession(fixture, { sessionId: 'chain-third', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      cloudflare: async () => { throw new Error('connection refused'); },
      nim: async () => new Response('service unavailable', { status: 503 }),
      ollama: async () => openAiResponse(providerOutput(sourceId), OLLAMA_MODEL),
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 0);

    // One reservation per target: an unreachable host is not retried inside the target either.
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'applied', degraded_reason: null, provider_attempts: 3 },
    ]);
    fixture.withDb((db) => {
      assert.equal(db.prepare("SELECT processing_state FROM raw_events WHERE kind = 'prompt'").get()?.processing_state,
        'processed');
    });
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=unreachable/);
    assert.match(log, /provider attempt .*position=1 preset=nim model=[^ ]+ reason=unreachable/);
    // The target that answered is not an attempt line: the batch state already says it applied.
    assert.equal(/position=2/.test(log), false);
  });
});

test('a primary with absent credentials is a failed target, not a run without a provider', async () => {
  await withFixture(async (fixture) => {
    // No Workers AI credentials: the destination label still comes from the primary's egress, so
    // the loop is reached and the primary is one more target that answers `no_provider`.
    fixture.env = cleanEnv(fixture.home, {});
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record what happens when the primary has no secret at all.';
    await captureEndedSession(fixture, { sessionId: 'chain-uncredentialed-primary', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, { ollama: async () => openAiResponse(providerOutput(sourceId), OLLAMA_MODEL) });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 0);

    assert.equal(hosts.cloudflare, 0);
    assert.equal(hosts.ollama, 1);
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'applied', degraded_reason: null, provider_attempts: 1 },
    ]);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=no_provider/);
    assert.match(log, /batch .*state=applied/);
  });
});

test('an agent-cli target reserves its attempt before the paid child process runs', async () => {
  await withFixture(async (fixture) => {
    // The primary has no credentials, so it answers `no_provider` and takes no reservation: the
    // agent-cli target behind it is the first thing in the pass that can fence the batch.
    fixture.env = cleanEnv(fixture.home, {});
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      costPolicy: ['free-tier', 'own-subscription'],
      fallback: [{ preset: 'agent-cli', model: 'claude-sonnet-4-5' }],
      env: fixture.env,
    });
    const prompt = 'Record what the subscription target spends.';
    await captureEndedSession(fixture, { sessionId: 'chain-agent-cli-reserves', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);

    const hosts = counters();
    const cli = cliSpawn([JSON.stringify(providerOutput(sourceId))]);
    assert.equal(await runObserveForFixture(fixture, { fetch: chainFetch(hosts, {}), spawn: cli.spawn }), 0);

    assert.equal(hosts.cloudflare, 0);
    assert.equal(cli.calls(), 1);
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'applied', degraded_reason: null, provider_attempts: 1 },
    ]);
    let usage: Record<string, unknown>[] = [];
    fixture.withDb((db) => {
      usage = db.prepare('SELECT preset, calls FROM provider_usage ORDER BY preset')
        .all().map((row) => ({ ...row }));
    });
    assert.deepEqual(usage, [{ preset: 'agent-cli', calls: 1 }]);
  });
});

test('the reason a stop ended the chain on outranks a more severe reason behind it', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      costPolicy: ['free-tier', 'local', 'remote'],
      fallback: [{ preset: 'nim' }],
      env: fixture.env,
    });
    await captureEndedSession(fixture, {
      sessionId: 'chain-stop-reason',
      prompts: ['Record which reason a stopped chain keeps.'],
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      // The primary fails with a reason that outranks `consent_changed` in `DEGRADED_PRECEDENCE`,
      // and withdraws consent on its way out.
      cloudflare: async () => {
        writeChainConfig(fixture, {
          preset: 'workers-ai',
          costPolicy: ['free-tier', 'local', 'remote'],
          fallback: [{ preset: 'nim' }],
          env: fixture.env,
          consent: 'invalid',
        });
        return new Response('unauthorized', { status: 401 });
      },
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);

    assert.equal(hosts.nim, 0);
    // `auth_failed` is the more severe reason, but consent is what the user has to act on, and
    // sending them to fix a credential instead would be the wrong instruction.
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'fallback', degraded_reason: 'consent_changed', provider_attempts: 1 },
    ]);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=auth_failed/);
    assert.match(log, /provider attempt .*position=1 preset=nim model=[^ ]+ reason=consent_changed/);
  });
});

test('a stop is noticed even when the next target could not have made a request', async () => {
  await withFixture(async (fixture) => {
    // No NVIDIA key: `summarizeWithProvider` answers `no_provider` for `nim` before it ever asks
    // whether consent still holds, so the stop has to be noticed by the loop itself
    // (contracts/provider-fallback.md "The attempt sequence", step 2).
    fixture.env = cleanEnv(fixture.home, {
      OBOETE_CF_API_TOKEN: 'chain-test-token',
      OBOETE_CF_ACCOUNT_ID: 'chain-test-account',
    });
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      costPolicy: ['free-tier', 'local', 'remote'],
      fallback: [{ preset: 'nim' }],
      env: fixture.env,
    });
    await captureEndedSession(fixture, {
      sessionId: 'chain-stop-uncredentialed',
      prompts: ['Record which reason a stopped chain keeps when the next target has no key.'],
    });

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      cloudflare: async () => {
        writeChainConfig(fixture, {
          preset: 'workers-ai',
          costPolicy: ['free-tier', 'local', 'remote'],
          fallback: [{ preset: 'nim' }],
          env: fixture.env,
          consent: 'invalid',
        });
        return new Response('unauthorized', { status: 401 });
      },
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);

    assert.equal(hosts.nim, 0);
    // Without the loop's own check the batch keeps `auth_failed` by precedence and sends the user
    // to fix a credential, when consent is the thing they have to act on.
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'fallback', degraded_reason: 'consent_changed', provider_attempts: 1 },
    ]);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=1 preset=nim model=[^ ]+ reason=consent_changed/);
  });
});

test('a log the worker cannot write ends the run as a storage failure', async (t) => {
  if (process.getuid?.() === 0) {
    t.skip('the root user writes into a file without write permission');
    return;
  }
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'Record that an unwritable log is a storage failure.';
    await captureEndedSession(fixture, { sessionId: 'chain-log-eacces', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      cloudflare: async () => { throw new Error('connection refused'); },
      // Read-only from the answer on, so the `batch` line's own append fails. That line is written
      // with the throwing writer on purpose: `EACCES` is `isStorageError`, so `recordRunFailure`
      // ends the run at exit 3 instead of reporting a clean pass over a log nobody can read.
      ollama: async () => {
        chmodSync(fixture.paths.observeLog, 0o400);
        return openAiResponse(providerOutput(sourceId), OLLAMA_MODEL);
      },
    });
    let exit: number;
    try {
      exit = await runObserveForFixture(fixture, { fetch: fetchImpl });
    } finally {
      chmodSync(fixture.paths.observeLog, 0o600);
    }

    assert.equal(hosts.ollama, 1);
    assert.equal(exit, 3, 'an unwritable observe log is a storage failure, not a clean run');
    // The exit alone does not say why: `logEnd` returns 3 when its own append fails, whatever the
    // run reached. What separates the two is where the run stopped. Measured: with a writable log
    // the same scenario exits 0 and records `last_run` reason `empty`; here the batch is `applied`
    // and its session is still `ended` with `summary_state = 'pending'`, so `queueIsEmpty` is false
    // and the run that threw out of the batch line records no end at all.
    let lastRun: unknown;
    fixture.withDb((db) => {
      lastRun = db.prepare('SELECT value_json FROM runtime_state WHERE key = ?').get('last_run');
    });
    assert.equal(lastRun, undefined, 'a storage failure leaves the queue undrained, unlike a clean pass');
    // The attempt lines use the quiet writer, so the unwritable log swallows them instead of
    // ending the run before the batch line decides its exit.
    assert.doesNotMatch(readFileSync(fixture.paths.observeLog, 'utf8'), /provider attempt/);
  });
});

test('a target whose answer is refused for its language is still named in the log', async () => {
  await withFixture(async (fixture) => {
    fixture.env = chainEnv(fixture);
    writeChainConfig(fixture, {
      preset: 'workers-ai',
      fallback: [{ preset: 'ollama', model: OLLAMA_MODEL }],
      env: fixture.env,
    });
    const prompt = 'アップロード処理の再試行を記録してください。';
    await captureEndedSession(fixture, {
      sessionId: 'chain-language',
      prompts: [prompt],
      assistant: 'アップロード処理は一回再試行します。',
    });
    const sourceId = eventId(fixture, prompt);

    const hosts = counters();
    const fetchImpl = chainFetch(hosts, {
      cloudflare: async () => { throw new Error('connection refused'); },
      // Two English answers for Japanese input: the target answered, so the chain stops here.
      ollama: async () => openAiResponse(providerOutput(sourceId), OLLAMA_MODEL),
    });
    assert.equal(await runObserveForFixture(fixture, { fetch: fetchImpl }), 1);

    assert.equal(hosts.ollama, 2, 'the language retry happens inside the target');
    assert.deepEqual(batchRows(fixture), [
      { destination: 'remote_observer', state: 'fallback', degraded_reason: 'language_mismatch', provider_attempts: 3 },
    ]);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /provider attempt .*position=0 preset=workers-ai model=[^ ]+ reason=unreachable/);
    // The target that spent an allowance is the one the log must not omit.
    assert.match(log, /provider attempt .*position=1 preset=ollama model=[^ ]+ reason=language_mismatch/);
  });
});
