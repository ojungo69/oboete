import assert from 'node:assert/strict';
import { test } from 'node:test';

import { PRESET_CATALOG, type Credentials } from '../../src/config.js';
import type { ObserverInput, ObserverOutput } from '../../src/observer/contract.js';
import { buildSummarizerPrompt, summarizeWithProvider } from '../../src/observer/llm.js';
import { cliSpawn } from '../helpers/agent-cli.js';

const MODEL = PRESET_CATALOG.openrouter.defaultModel;
const MAX_RESPONSE_CHARS = 1024 * 1024;

const INPUT: ObserverInput = {
  repo_ref: 'repo_1234',
  checkpoint_context: { state: 'none' },
  session: {
    started_at: 1_757_000_000_000,
    turns: [{ ordinal: 1, started_at: 1_757_000_000_000, ended_at: null }],
  },
  events: [{ id: 'e1', kind: 'prompt', text: 'この不具合を直してください。' }],
  free_summaries: {},
  nearby: [],
  language_hint: 'ja',
};

function output(sourceEventId = 'e1'): ObserverOutput {
  return {
    checkpoint: { decision: 'unchanged', source_event_ids: [sourceEventId], reason: '作業の進捗に変更はありません。' },
    observations: [
      {
        type: 'bugfix',
        visibility: 'project',
        title: '不具合を修正した',
        body: '共有経路の条件を修正した。',
        concepts: ['problem-solution'],
        citations: { files_read: [], files_modified: [], commits: [] },
        source_event_ids: [sourceEventId],
        classification: { decision: 'add', target: null, reason: '新しい修正' },
      },
    ],
  };
}

function apiCredentials(): Credentials {
  return {
    kind: 'api-key',
    present: true,
    source: 'test',
    values: { apiKey: 'test-key' },
  };
}

function openAiResponse(
  text: string,
  options: {
    model?: string;
    finishReason?: string;
    headers?: ConstructorParameters<typeof Headers>[0];
    inputTokens?: number;
    outputTokens?: number;
  } = {},
): Response {
  const inputTokens = options.inputTokens ?? 10;
  const outputTokens = options.outputTokens ?? 5;
  const headers = new Headers(options.headers);
  headers.set('content-type', 'application/json');
  return new Response(
    JSON.stringify({
      id: 'response-1',
      model: options.model ?? MODEL,
      choices: [
        {
          index: 0,
          message: { role: 'assistant', content: text },
          finish_reason: options.finishReason ?? 'stop',
        },
      ],
      usage: {
        prompt_tokens: inputTokens,
        completion_tokens: outputTokens,
        total_tokens: inputTokens + outputTokens,
      },
    }),
    {
      status: 200,
      headers,
    },
  );
}

function workersResponse(
  text: unknown,
  extraHeaders: ConstructorParameters<typeof Headers>[0] = {},
): Response {
  const headers = new Headers(extraHeaders);
  headers.set('content-type', 'application/json');
  return new Response(
    JSON.stringify({
      success: true,
      result: {
        response: text,
        usage: { prompt_tokens: 8, completion_tokens: 2 },
      },
      errors: [],
      messages: [],
    }),
    { status: 200, headers },
  );
}

function errorResponse(status: number, bodyCode?: number | string, cloudflare = false): Response {
  const body =
    bodyCode === undefined
      ? { error: { message: 'failure' } }
      : cloudflare
        ? { errors: [{ code: bodyCode, message: 'failure' }] }
        : { error: { message: 'failure', code: bodyCode } };
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

type FetchStep = (...args: Parameters<typeof globalThis.fetch>) => Response | Promise<Response>;

function scriptedFetch(...steps: FetchStep[]): { fetch: typeof fetch; calls: () => number } {
  let count = 0;
  return {
    fetch: async (input, init) => {
      const step = steps[count];
      count += 1;
      if (step === undefined) throw new Error(`unexpected fetch ${count}`);
      return step(input, init);
    },
    calls: () => count,
  };
}

function httpHarness(
  fetch: typeof globalThis.fetch,
  overrides: Partial<Parameters<typeof summarizeWithProvider>[1]> = {},
): {
  ctx: Parameters<typeof summarizeWithProvider>[1];
  reservations: () => number;
  exhausted: string[];
} {
  let reservationCount = 0;
  const exhausted: string[] = [];
  return {
    ctx: {
      preset: 'openrouter',
      model: MODEL,
      credentials: apiCredentials(),
      consentOk: () => true,
      reserve: () => {
        reservationCount += 1;
        return { ok: true, reservationId: `reservation-${reservationCount}` };
      },
      onExhausted: (reservationId) => exhausted.push(reservationId),
      fetch,
      timeoutMs: 1000,
      ...overrides,
    },
    reservations: () => reservationCount,
    exhausted,
  };
}

test('buildSummarizerPrompt states the contract without an agent name or repository path', () => {
  const schemaPrompt = buildSummarizerPrompt(INPUT, 'schema');
  assert.equal(schemaPrompt.user, JSON.stringify(INPUT));
  assert.match(schemaPrompt.system, /bugfix/);
  assert.match(schemaPrompt.system, /security_note/);
  assert.match(schemaPrompt.system, /how-it-works/);
  assert.match(schemaPrompt.system, /source_event_ids/);
  assert.match(schemaPrompt.system, /events list only/);
  assert.match(schemaPrompt.system, /dominant language of the input/);
  assert.doesNotMatch(schemaPrompt.system, /claude|codex|grok|\bpi\b/i);
  assert.doesNotMatch(schemaPrompt.system, /\/home\/|repository path/i);

  const textJsonPrompt = buildSummarizerPrompt(INPUT, 'text-json');
  assert.match(textJsonPrompt.system, /Reply with exactly one JSON object matching this schema/);
  assert.match(textJsonPrompt.system, /"observations"/);
});

test('buildSummarizerPrompt preserves exact strings in both modes', () => {
  for (const mode of ['schema', 'text-json'] as const) {
    const prompt = buildSummarizerPrompt(INPUT, mode);
    assert.match(prompt.system, /verbatim/);
    assert.match(prompt.system, /never translate/);
  }
});

test('schema success returns validated output, model id, attempts, and header neurons', async () => {
  let requestBody: Record<string, unknown> | undefined;
  const scripted = scriptedFetch(async (_input, init) => {
    requestBody = JSON.parse(String(init?.body)) as Record<string, unknown>;
    return workersResponse(output(), { 'cf-aig-neurons': '45.25' });
  });
  const harness = httpHarness(scripted.fetch, {
    preset: 'workers-ai',
    model: PRESET_CATALOG['workers-ai'].defaultModel,
    credentials: {
      kind: 'cloudflare',
      present: true,
      source: 'test',
      values: { accountId: 'account-123', token: 'test-token' },
    },
  });
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.deepEqual(result, {
    ok: true,
    output: output(),
    resolvedModel: PRESET_CATALOG['workers-ai'].defaultModel,
    neurons: 45.25,
    attempts: 1,
  });
  assert.equal(scripted.calls(), 1);
  assert.equal(harness.reservations(), 1);
  const responseFormat = requestBody?.response_format as Record<string, unknown> | undefined;
  assert.equal(responseFormat?.type, 'json_schema');
  assert.equal(typeof responseFormat?.json_schema, 'object');
  assert.deepEqual(requestBody?.chat_template_kwargs, { enable_thinking: false }, 'thinking is off for the observer call');
});

test('neurons fall back to separate input and output token rates', async () => {
  let requestBody: Record<string, unknown> | undefined;
  const scripted = scriptedFetch(async (_input, init) => {
    requestBody = JSON.parse(String(init?.body)) as Record<string, unknown>;
    return openAiResponse(JSON.stringify(output()), { inputTokens: 100, outputTokens: 20 });
  });
  const result = await summarizeWithProvider(INPUT, httpHarness(scripted.fetch).ctx);
  assert.equal(result.ok, true);
  if (result.ok) {
    assert.equal(result.neurons, (100 * 5500 + 20 * 36_400) / 1_000_000);
  }
  assert.deepEqual(requestBody?.response_format, { type: 'json_object' });
});

test('HTTP status and body code classification is table-driven and code-sensitive', async (t) => {
  const cases: Array<{
    name: string;
    status: number;
    code?: number | string;
    cloudflare?: boolean;
    reason: Exclude<Awaited<ReturnType<typeof summarizeWithProvider>>, { ok: true }>['reason'];
    attempts: number;
  }> = [
    { name: '429 code 3036', status: 429, code: 3036, cloudflare: true, reason: 'provider_exhausted', attempts: 1 },
    { name: '429 code 3040', status: 429, code: 3040, reason: 'provider_exhausted', attempts: 2 },
    { name: '429 without code', status: 429, reason: 'provider_exhausted', attempts: 1 },
    { name: '403 code 5035', status: 403, code: 5035, reason: 'provider_paid', attempts: 1 },
    { name: '403 without code', status: 403, reason: 'auth_failed', attempts: 1 },
    { name: '401', status: 401, reason: 'auth_failed', attempts: 1 },
    { name: '408', status: 408, reason: 'unreachable', attempts: 2 },
    { name: 'body code 3007', status: 500, code: 3007, reason: 'unreachable', attempts: 2 },
    { name: 'permission body code', status: 400, code: 'permission_denied', reason: 'auth_failed', attempts: 1 },
  ];

  for (const row of cases) {
    await t.test(row.name, async () => {
      const steps = Array.from({ length: row.attempts }, () => async () =>
        errorResponse(row.status, row.code, row.cloudflare),
      );
      const scripted = scriptedFetch(...steps);
      const harness = httpHarness(scripted.fetch);
      const result = await summarizeWithProvider(INPUT, harness.ctx);
      assert.equal(result.ok, false);
      if (!result.ok) assert.equal(result.reason, row.reason);
      assert.equal(result.attempts, row.attempts);
      assert.equal(scripted.calls(), row.attempts);
      assert.equal(harness.reservations(), row.attempts);
      assert.deepEqual(
        harness.exhausted,
        row.status === 429 && row.code === 3036 ? ['reservation-1'] : [],
      );
    });
  }
});

test('a network failure without a status is unreachable', async () => {
  const scripted = scriptedFetch(async () => {
    throw new TypeError('offline');
  });
  const result = await summarizeWithProvider(INPUT, httpHarness(scripted.fetch).ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'unreachable');
  assert.equal(result.attempts, 1);
});

test('an aborted provider request is timeout', async () => {
  const scripted = scriptedFetch(
    async (_input, init) =>
      await new Promise<Response>((_resolve, reject) => {
        const signal = init?.signal;
        if (signal === null || signal === undefined) assert.fail('missing abort signal');
        const keepAlive = setTimeout(() => {}, 1000);
        signal.addEventListener(
          'abort',
          () => {
            clearTimeout(keepAlive);
            reject(signal.reason);
          },
          { once: true },
        );
      }),
  );
  const harness = httpHarness(scripted.fetch, { timeoutMs: 10 });
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'timeout');
  assert.equal(result.attempts, 1);
});

test('length finish reason retries once then returns unusable_output', async () => {
  const scripted = scriptedFetch(
    async () => openAiResponse(JSON.stringify(output()), { finishReason: 'length' }),
    async () => openAiResponse(JSON.stringify(output()), { finishReason: 'length' }),
  );
  const harness = httpHarness(scripted.fetch);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'unusable_output');
  assert.equal(result.attempts, 2);
  assert.equal(harness.reservations(), 2);
});

test('invalid JSON retries once then returns unusable_output', async () => {
  const scripted = scriptedFetch(
    async () => openAiResponse('not json'),
    async () => openAiResponse('still not json'),
  );
  const harness = httpHarness(scripted.fetch);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'unusable_output');
  assert.equal(result.attempts, 2);
  assert.equal(harness.reservations(), 2);
});

test('foreign source_event_ids retry once then return unusable_output', async () => {
  const scripted = scriptedFetch(
    async () => openAiResponse(JSON.stringify(output('foreign'))),
    async () => openAiResponse(JSON.stringify(output('foreign'))),
  );
  const result = await summarizeWithProvider(INPUT, httpHarness(scripted.fetch).ctx);
  assert.equal(result.ok, false);
  if (!result.ok) {
    assert.equal(result.reason, 'unusable_output');
    assert.match(result.detail, /source_event_ids/);
  }
  assert.equal(result.attempts, 2);
});

test('a response above 1 MB is refused without parsing or retrying', async () => {
  const scripted = scriptedFetch(async () => openAiResponse('x'.repeat(MAX_RESPONSE_CHARS + 1)));
  const harness = httpHarness(scripted.fetch);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'unusable_output');
  assert.equal(result.attempts, 1);
  assert.equal(harness.reservations(), 1);
});

test('an HTTP envelope above 1 MB is stopped before the SDK parses it', async () => {
  const scripted = scriptedFetch(
    async () =>
      new Response('x'.repeat(MAX_RESPONSE_CHARS + 1), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
  );
  const harness = httpHarness(scripted.fetch);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'unusable_output');
  assert.equal(result.attempts, 1);
  assert.equal(harness.reservations(), 1);
});

test('a declared content-length above 1 MB cancels the live body', async () => {
  let cancelled = false;
  const scripted = scriptedFetch(
    async () =>
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(new Uint8Array([120]));
          },
          cancel() {
            cancelled = true;
          },
        }),
        {
          status: 200,
          headers: {
            'content-type': 'application/json',
            'content-length': '2097152',
          },
        },
      ),
  );
  const harness = httpHarness(scripted.fetch);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) {
    assert.equal(result.reason, 'unusable_output');
    assert.equal(result.detail, 'provider response exceeded 1 MB');
  }
  assert.equal(result.attempts, 1);
  assert.equal(harness.reservations(), 1);
  assert.equal(cancelled, true);
});

test('a returned model id alias is rejected', async () => {
  const scripted = scriptedFetch(async () =>
    openAiResponse(JSON.stringify(output()), { model: `${MODEL}-2026-09-04` }),
  );
  const result = await summarizeWithProvider(INPUT, httpHarness(scripted.fetch).ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'model_alias');
  assert.equal(result.attempts, 1);
});

test('known runtime-id decorations are normalized before the model comparison', async () => {
  const scripted = scriptedFetch(async () =>
    openAiResponse(JSON.stringify(output()), { model: 'claude-opus-5[1m]' }),
  );
  const harness = httpHarness(scripted.fetch, { model: 'claude-opus-5' });
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, true);
  if (result.ok) assert.equal(result.resolvedModel, 'claude-opus-5[1m]');
});

test('changed consent prevents both reservation and fetch', async () => {
  const scripted = scriptedFetch(async () => assert.fail('fetch must not run'));
  const harness = httpHarness(scripted.fetch, { consentOk: () => false });
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'consent_changed');
  assert.equal(result.attempts, 0);
  assert.equal(harness.reservations(), 0);
  assert.equal(scripted.calls(), 0);
});

test('a refused reservation returns its reason without a fetch', async (t) => {
  for (const reason of ['daily_cap', 'provider_exhausted'] as const) {
    await t.test(reason, async () => {
      const scripted = scriptedFetch(async () => assert.fail('fetch must not run'));
      const harness = httpHarness(scripted.fetch, { reserve: () => ({ ok: false, reason }) });
      const result = await summarizeWithProvider(INPUT, harness.ctx);
      assert.equal(result.ok, false);
      if (!result.ok) assert.equal(result.reason, reason);
      assert.equal(result.attempts, 0);
      assert.equal(scripted.calls(), 0);
    });
  }
});

test('missing credentials return no_provider without reserving or fetching', async () => {
  const scripted = scriptedFetch(async () => assert.fail('fetch must not run'));
  const harness = httpHarness(scripted.fetch, {
    credentials: { kind: 'api-key', present: false, source: 'test', values: {} },
  });
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'no_provider');
  assert.equal(result.attempts, 0);
  assert.equal(harness.reservations(), 0);
});

test('3036 persists exactly the reservation that observed exhaustion', async () => {
  const scripted = scriptedFetch(async () => errorResponse(429, 3036, true));
  const harness = httpHarness(scripted.fetch);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  assert.deepEqual(harness.exhausted, ['reservation-1']);
});

/**
 * The agent-cli half of `httpHarness`: an uncapped target still takes a reservation, because the
 * reservation is what puts the batch in `running` and fences a dead worker's in-flight child
 * process (contracts/provider-fallback.md "For each target").
 */
function cliHarness(
  cli: ReturnType<typeof cliSpawn>,
  overrides: Partial<Parameters<typeof summarizeWithProvider>[1]> = {},
): { ctx: Parameters<typeof summarizeWithProvider>[1]; reservations: () => number } {
  let reservationCount = 0;
  return {
    ctx: {
      preset: 'agent-cli',
      model: 'agent-model',
      agentCli: 'claude',
      credentials: { kind: 'agent-login', present: true, source: 'test', values: {} },
      consentOk: () => true,
      reserve: () => {
        reservationCount += 1;
        return { ok: true, reservationId: `reservation-${reservationCount}` };
      },
      onExhausted: () => assert.fail('agent-cli cannot persist provider exhaustion'),
      spawn: cli.spawn,
      timeoutMs: 1000,
      ...overrides,
    },
    reservations: () => reservationCount,
  };
}

test('agent-cli is uncapped, consented, reserves its attempt and validates the CLI text as observer JSON', async () => {
  const cli = cliSpawn([JSON.stringify(output())]);
  const harness = cliHarness(cli);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.deepEqual(result, {
    ok: true,
    output: output(),
    resolvedModel: null,
    neurons: null,
    attempts: 1,
  });
  assert.equal(cli.calls(), 1);
  assert.equal(harness.reservations(), 1);
});

test('agent-cli retries one non-JSON model reply then returns unusable_output', async () => {
  const cli = cliSpawn(['not json', 'still not json']);
  const harness = cliHarness(cli);
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'unusable_output');
  assert.equal(result.attempts, 2);
  assert.equal(cli.calls(), 2);
  assert.equal(harness.reservations(), 2);
});

test('a refused reservation stops agent-cli before the paid child process runs', async () => {
  const cli = cliSpawn([JSON.stringify(output())]);
  const harness = cliHarness(cli, {
    reserve: () => ({ ok: false, reason: 'provider_exhausted' }),
  });
  const result = await summarizeWithProvider(INPUT, harness.ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'provider_exhausted');
  assert.equal(result.attempts, 0);
  assert.equal(cli.calls(), 0);
});

test('a consent change after the agent-cli reservation stops the chain before the child process', async () => {
  const cli = cliSpawn([JSON.stringify(output())]);
  let reservations = 0;
  const result = await summarizeWithProvider(INPUT, cliHarness(cli, {
    reserve: () => {
      reservations += 1;
      return { ok: true, reservationId: 'reservation-1' };
    },
    consentOk: () => reservations === 0,
  }).ctx);
  assert.equal(result.ok, false);
  if (!result.ok) assert.equal(result.reason, 'consent_changed');
  assert.equal(cli.calls(), 0);
});

/**
 * The stub is handed to the product through slots typed `typeof spawn` (`deps.spawn` in
 * `src/worker/observe.ts`, `src/doctor.ts` and `src/setup/probe.ts`), and that type permits
 * `spawn(command, args)`. A stub that read `options.signal` unconditionally would fail such a call
 * with a `TypeError` in the tests while the same call worked in production.
 */
test('the agent CLI stub answers a spawn call that omits options, as `typeof spawn` allows', () => {
  const cli = cliSpawn([]);
  assert.doesNotThrow(() => cli.spawn('claude', ['--version']));
});
