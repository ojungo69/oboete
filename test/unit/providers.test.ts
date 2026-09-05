import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { writeFileSync } from 'node:fs';
import type { spawn } from 'node:child_process';
import { test } from 'node:test';

import {
  PRESET_CATALOG,
  configSchema,
  type Credentials,
  type PresetName,
} from '../../src/config.js';
import { observerOutputJsonSchema } from '../../src/observer/contract.js';
import {
  ProviderConfigError,
  createLanguageModel,
  providerRequestOptions,
  resolveModel,
  runAgentCli,
} from '../../src/observer/providers.js';

type SpawnChild = EventEmitter & {
  stdin: PassThrough;
  stdout: PassThrough;
  stderr: PassThrough;
  kill: () => boolean;
};

type SpawnCall = {
  command: string;
  args: readonly string[];
  input: string;
  env: NodeJS.ProcessEnv | undefined;
  child: SpawnChild;
};

function fakeSpawn(respond: (call: SpawnCall) => void): typeof spawn {
  return ((
    command: string,
    args: readonly string[],
    options: { signal?: AbortSignal; env?: NodeJS.ProcessEnv },
  ) => {
    const child = Object.assign(new EventEmitter(), {
      stdin: new PassThrough(),
      stdout: new PassThrough(),
      stderr: new PassThrough(),
      kill: () => true,
    }) as SpawnChild;
    const chunks: Buffer[] = [];
    child.stdin.on('data', (chunk: Buffer) => chunks.push(chunk));
    child.stdin.on('finish', () => {
      respond({ command, args, input: Buffer.concat(chunks).toString('utf8'), env: options.env, child });
    });
    options.signal?.addEventListener(
      'abort',
      () => {
        const error = new Error('aborted');
        error.name = 'AbortError';
        child.emit('error', error);
      },
      { once: true },
    );
    return child;
  }) as unknown as typeof spawn;
}

function finish(child: SpawnChild, stdout = '', code = 0): void {
  if (stdout !== '') child.stdout.write(stdout);
  child.stdout.end();
  child.stderr.end();
  queueMicrotask(() => child.emit('close', code, null));
}

function credentialsFor(preset: PresetName): Credentials {
  if (preset === 'workers-ai') {
    return {
      kind: 'cloudflare',
      present: true,
      source: 'test',
      values: { accountId: 'account-123', token: 'credential-value' },
    };
  }
  if (preset === 'ollama') {
    return { kind: 'none', present: true, source: 'none', values: {} };
  }
  return {
    kind: 'api-key',
    present: true,
    source: 'test',
    values: { apiKey: 'credential-value' },
  };
}

test('resolveModel uses the configured override and catalog default', () => {
  assert.deepEqual(
    resolveModel(configSchema.parse({ observer: { preset: 'workers-ai', model: 'custom/model' } })),
    { preset: 'workers-ai', model: 'custom/model' },
  );
  assert.deepEqual(resolveModel(configSchema.parse({ observer: { preset: 'nim' } })), {
    preset: 'nim',
    model: PRESET_CATALOG.nim.defaultModel,
  });
  assert.deepEqual(resolveModel(configSchema.parse({ observer: { preset: 'none' } })), {
    preset: 'none',
    model: '',
  });
});

test('resolveModel rejects presets without a model default', () => {
  for (const preset of ['ollama', 'agent-cli'] as const) {
    assert.throws(
      () => resolveModel(configSchema.parse({ observer: { preset } })),
      (error: unknown) =>
        error instanceof ProviderConfigError && error.code === 'model_required',
      preset,
    );
  }
  assert.deepEqual(
    resolveModel(configSchema.parse({ observer: { preset: 'ollama', model: 'qwen3:8b' } })),
    { preset: 'ollama', model: 'qwen3:8b' },
  );
});

test('providerRequestOptions follows the preset structured-output policy', () => {
  assert.deepEqual(providerRequestOptions('workers-ai'), {
    structured: 'json_schema',
    providerOptions: {
      'workers-ai': {
        response_format: {
          type: 'json_schema',
          json_schema: { name: 'observer_output', schema: observerOutputJsonSchema },
        },
      },
    },
  });
  for (const preset of ['ollama', 'openrouter'] as const) {
    assert.deepEqual(providerRequestOptions(preset), {
      structured: 'response_format',
      providerOptions: { [preset]: { response_format: { type: 'json_object' } } },
    });
  }
  for (const preset of ['nim', 'gemini', 'agent-cli'] as const) {
    assert.deepEqual(providerRequestOptions(preset), { structured: 'text-json' });
  }
});

test('createLanguageModel sends each HTTP preset to its catalog endpoint with the right auth', async (t) => {
  const { generateText } = await import('ai');
  for (const preset of ['workers-ai', 'ollama', 'nim', 'openrouter', 'gemini'] as const) {
    await t.test(preset, async () => {
      let requestUrl = '';
      let authorization: string | null = null;
      const stubFetch: typeof fetch = async (input, init) => {
        requestUrl = String(input);
        authorization = new Headers(init?.headers).get('authorization');
        const body =
          preset === 'workers-ai'
            ? { success: true, result: { response: 'ok' }, errors: [], messages: [] }
            : {
                id: 'response-1',
                model: PRESET_CATALOG[preset].defaultModel || 'qwen3:8b',
                choices: [
                  { index: 0, message: { role: 'assistant', content: 'ok' }, finish_reason: 'stop' },
                ],
                usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
              };
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      };
      const modelId = PRESET_CATALOG[preset].defaultModel || 'qwen3:8b';
      const model = await createLanguageModel(preset, modelId, credentialsFor(preset), {
        fetch: stubFetch,
      });
      const result = await generateText({ model, prompt: 'test', maxRetries: 0 });
      assert.equal(result.text, 'ok');
      const baseUrl =
        preset === 'workers-ai'
          ? PRESET_CATALOG[preset].baseUrl.replace('<account>', 'account-123')
          : PRESET_CATALOG[preset].baseUrl;
      assert.ok(requestUrl.startsWith(baseUrl), requestUrl);
      assert.equal(
        authorization,
        preset === 'ollama' ? null : 'Bearer credential-value',
        `${preset} authorization`,
      );
    });
  }
});

test('createLanguageModel never exposes a credential in configuration errors', async () => {
  const secret = 'do-not-print-this-value';
  await assert.rejects(
    createLanguageModel(
      'workers-ai',
      PRESET_CATALOG['workers-ai'].defaultModel,
      { kind: 'cloudflare', present: false, source: 'test', values: { token: secret } },
      { fetch: async () => assert.fail('fetch must not run') },
    ),
    (error: unknown) =>
      error instanceof ProviderConfigError &&
      error.code === 'credentials_required' &&
      !error.message.includes(secret),
  );
});

test('runAgentCli uses the verified JSON field for claude and grok', async (t) => {
  for (const cli of ['claude', 'grok'] as const) {
    await t.test(cli, async () => {
      let seen: SpawnCall | undefined;
      const result = await runAgentCli(cli, 'observer prompt', {
        timeoutMs: 1000,
        spawn: fakeSpawn((call) => {
          seen = call;
          finish(call.child, JSON.stringify(cli === 'claude' ? { result: 'model text' } : { text: 'model text' }));
        }),
      });
      assert.deepEqual(result, { text: 'model text' });
      assert.equal(seen?.command, cli);
      assert.deepEqual(seen?.args, ['-p', '--output-format', 'json']);
      assert.equal(seen?.input, 'observer prompt');
    });
  }
});

test('runAgentCli reads the codex last-message file', async () => {
  let seen: SpawnCall | undefined;
  const result = await runAgentCli('codex', 'observer prompt', {
    timeoutMs: 1000,
    spawn: fakeSpawn((call) => {
      seen = call;
      const outputIndex = call.args.indexOf('--output-last-message');
      const outputPath = call.args[outputIndex + 1];
      if (outputPath === undefined) assert.fail('missing --output-last-message path');
      writeFileSync(outputPath, 'model text', 'utf8');
      finish(call.child, '{"type":"turn.completed"}\n');
    }),
  });
  assert.deepEqual(result, { text: 'model text' });
  assert.equal(seen?.command, 'codex');
  assert.equal(seen?.args[0], 'exec');
  assert.ok(seen?.args.includes('--json'));
  assert.equal(seen?.args.at(-1), '-');
  assert.equal(seen?.input, 'observer prompt');
});

test('runAgentCli aborts a child at its timeout', async () => {
  const keepAlive = setTimeout(() => {}, 1000);
  try {
    const result = await runAgentCli('claude', 'observer prompt', {
      timeoutMs: 10,
      spawn: fakeSpawn(() => {
        // The injected child stays open until the AbortSignal fires.
      }),
    });
    assert.deepEqual(result, { error: 'timeout' });
  } finally {
    clearTimeout(keepAlive);
  }
});

test('runAgentCli inherits the login environment without forwarding oboete credentials', async () => {
  const previous = process.env.OBOETE_NIM_API_KEY;
  process.env.OBOETE_NIM_API_KEY = 'do-not-forward';
  let seen: SpawnCall | undefined;
  try {
    const result = await runAgentCli('claude', 'observer prompt', {
      timeoutMs: 1000,
      spawn: fakeSpawn((call) => {
        seen = call;
        finish(call.child, JSON.stringify({ result: 'model text' }));
      }),
    });
    assert.deepEqual(result, { text: 'model text' });
    assert.equal(seen?.env?.OBOETE_NIM_API_KEY, undefined);
    assert.equal(seen?.env?.PATH, process.env.PATH);
  } finally {
    if (previous === undefined) delete process.env.OBOETE_NIM_API_KEY;
    else process.env.OBOETE_NIM_API_KEY = previous;
  }
});
