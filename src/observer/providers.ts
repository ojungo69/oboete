import { spawn as nodeSpawn, type spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  PRESET_CATALOG,
  type Credentials,
  type OboeteConfig,
  type PresetName,
} from '../config.js';
import { observerOutputJsonSchema } from './contract.js';

export class ProviderConfigError extends Error {
  readonly code: 'model_required' | 'credentials_required' | 'unsupported_preset';

  constructor(message: string, code: ProviderConfigError['code']) {
    super(message);
    this.name = 'ProviderConfigError';
    this.code = code;
  }
}

export function resolveModel(
  config: OboeteConfig,
): { preset: PresetName | 'none'; model: string } {
  const preset = config.observer.preset;
  if (preset === 'none') return { preset, model: '' };
  const model = (config.observer.model ?? PRESET_CATALOG[preset].defaultModel).trim();
  if (model === '') {
    throw new ProviderConfigError(
      `The ${preset} preset requires an observer model in the configuration.`,
      'model_required',
    );
  }
  return { preset, model };
}

function credentialValue(credentials: Credentials, name: string): string {
  const value = credentials.values[name];
  if (!credentials.present || value === undefined || value === '') {
    throw new ProviderConfigError(
      'The selected observer preset does not have its required credentials.',
      'credentials_required',
    );
  }
  return value;
}

export async function createLanguageModel(
  preset: PresetName,
  model: string,
  credentials: Credentials,
  options: { fetch?: typeof globalThis.fetch } = {},
) {
  if (model.trim() === '') {
    throw new ProviderConfigError(
      `The ${preset} preset requires an observer model in the configuration.`,
      'model_required',
    );
  }
  if (preset === 'agent-cli') {
    throw new ProviderConfigError(
      'The agent-cli preset uses a child process instead of an HTTP language model.',
      'unsupported_preset',
    );
  }

  if (preset === 'workers-ai') {
    const accountId = credentialValue(credentials, 'accountId');
    const apiKey = credentialValue(credentials, 'token');
    const { createWorkersAI } = await import('workers-ai-provider');
    return createWorkersAI({
      accountId,
      apiKey,
      ...(options.fetch === undefined ? {} : { fetch: options.fetch }),
    })(model);
  }

  const apiKey =
    preset === 'ollama' ? undefined : credentialValue(credentials, 'apiKey');
  const { createOpenAICompatible } = await import('@ai-sdk/openai-compatible');
  const provider = createOpenAICompatible({
    name: preset,
    baseURL: PRESET_CATALOG[preset].baseUrl,
    ...(apiKey === undefined ? {} : { apiKey }),
    ...(options.fetch === undefined ? {} : { fetch: options.fetch }),
  });
  return provider.chatModel(model);
}

type RequestOptions = {
  structured: 'json_schema' | 'response_format' | 'text-json';
  providerOptions?: object;
};

export function providerRequestOptions(preset: PresetName): RequestOptions {
  const structured = PRESET_CATALOG[preset].structuredOutput;
  if (structured === 'json_schema') {
    return {
      structured,
      providerOptions: {
        'workers-ai': {
          response_format: {
            type: 'json_schema',
            json_schema: { name: 'observer_output', schema: observerOutputJsonSchema },
          },
        },
      },
    };
  }
  if (structured === 'response_format') {
    return {
      structured,
      providerOptions: { [preset]: { response_format: { type: 'json_object' } } },
    };
  }
  return { structured };
}

type ProcessResult = { ok: true; stdout: string } | { ok: false; error: string };

function isAbort(error: unknown): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'name' in error &&
    (error.name === 'AbortError' || error.name === 'TimeoutError')
  );
}

async function runChild(
  command: string,
  args: string[],
  prompt: string,
  timeoutMs: number,
  spawnFn: typeof spawn,
): Promise<ProcessResult> {
  const signal = AbortSignal.timeout(Math.max(1, timeoutMs));
  const env = { ...process.env };
  delete env.OBOETE_CF_API_TOKEN;
  delete env.OBOETE_CF_ACCOUNT_ID;
  for (const preset of Object.values(PRESET_CATALOG)) {
    if (preset.credential.kind === 'api-key') delete env[preset.credential.envName];
  }
  return await new Promise<ProcessResult>((resolve) => {
    let child: ReturnType<typeof nodeSpawn>;
    try {
      child = spawnFn(command, args, { stdio: ['pipe', 'pipe', 'pipe'], signal, env });
    } catch (error) {
      resolve({ ok: false, error: isAbort(error) ? 'timeout' : 'process_failed' });
      return;
    }

    const stdout: Buffer[] = [];
    child.stdout?.on('data', (chunk: Buffer | string) => {
      stdout.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
    });
    child.stderr?.resume();
    child.stdin?.on('error', () => {
      // The close/error event below owns the process result.
    });
    child.once('error', (error) => {
      resolve({ ok: false, error: isAbort(error) || signal.aborted ? 'timeout' : 'process_failed' });
    });
    child.once('close', (code) => {
      if (code !== 0) {
        resolve({ ok: false, error: signal.aborted ? 'timeout' : 'process_failed' });
        return;
      }
      resolve({ ok: true, stdout: Buffer.concat(stdout).toString('utf8') });
    });
    child.stdin?.end(prompt);
  });
}

function jsonText(stdout: string, field: 'result' | 'text'): string | undefined {
  try {
    const value: unknown = JSON.parse(stdout);
    if (typeof value !== 'object' || value === null || !(field in value)) return undefined;
    const text = (value as Record<string, unknown>)[field];
    return typeof text === 'string' ? text : undefined;
  } catch {
    return undefined;
  }
}

export async function runAgentCli(
  cli: 'claude' | 'codex' | 'grok',
  prompt: string,
  options: { timeoutMs: number; spawn?: typeof spawn },
): Promise<{ text: string } | { error: string }> {
  const spawnFn = options.spawn ?? nodeSpawn;
  if (cli === 'codex') {
    const directory = await mkdtemp(join(tmpdir(), 'oboete-codex-'));
    const outputPath = join(directory, 'last-message.txt');
    try {
      const result = await runChild(
        'codex',
        ['exec', '--json', '--output-last-message', outputPath, '-'],
        prompt,
        options.timeoutMs,
        spawnFn,
      );
      if (!result.ok) return { error: result.error };
      try {
        return { text: await readFile(outputPath, 'utf8') };
      } catch {
        return { error: 'invalid_output' };
      }
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  }

  const result = await runChild(
    cli,
    ['-p', '--output-format', 'json'],
    prompt,
    options.timeoutMs,
    spawnFn,
  );
  if (!result.ok) return { error: result.error };
  const text = jsonText(result.stdout, cli === 'claude' ? 'result' : 'text');
  return text === undefined ? { error: 'invalid_output' } : { text };
}
