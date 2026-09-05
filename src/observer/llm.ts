import type { spawn } from 'node:child_process';

import type { AgentCli, Credentials, PresetName } from '../config.js';
import {
  CONCEPTS,
  OBSERVATION_TYPES,
  observerOutputJsonSchema,
  observerOutputSchema,
  validateObserverOutput,
  type ObserverInput,
  type ObserverOutput,
} from './contract.js';
import {
  createLanguageModel,
  providerRequestOptions,
  runAgentCli,
} from './providers.js';
import { faultFetch, testFault } from '../testing/faults.js';

const REQUEST_TIMEOUT_MS = 60_000;
const MAX_RESPONSE_BYTES = 1024 * 1024;

// contracts/observer.md call policy item 7: Workers AI neurons per million tokens.
const INPUT_NEURONS_PER_MILLION_TOKENS = 5_500;
const OUTPUT_NEURONS_PER_MILLION_TOKENS = 36_400;

export type CallOutcome =
  | {
      ok: true;
      output: ObserverOutput;
      resolvedModel: string | null;
      neurons: number | null;
      attempts: number;
    }
  | {
      ok: false;
      reason:
        | 'daily_cap'
        | 'provider_exhausted'
        | 'provider_paid'
        | 'auth_failed'
        | 'unreachable'
        | 'timeout'
        | 'unusable_output'
        | 'model_alias'
        | 'consent_changed'
        | 'no_provider';
      attempts: number;
      detail: string;
    };

type FailureReason = Extract<CallOutcome, { ok: false }>['reason'];
type ApiError = { statusCode?: number; responseBody?: string; cause?: unknown };
type JsonValue = null | string | number | boolean | JsonObject | JsonValue[];
type JsonObject = { [key: string]: JsonValue | undefined };
type ProviderOptions = Record<string, JsonObject>;

const ERROR_OUTCOME_ROWS: ReadonlyArray<{
  status: number | null;
  bodyCode: number | string | null;
  outcome: FailureReason;
}> = [
  { status: 429, bodyCode: 3036, outcome: 'provider_exhausted' },
  { status: 429, bodyCode: 3040, outcome: 'provider_exhausted' },
  { status: 403, bodyCode: 5035, outcome: 'provider_paid' },
  { status: null, bodyCode: 3007, outcome: 'unreachable' },
  { status: 401, bodyCode: null, outcome: 'auth_failed' },
  { status: 403, bodyCode: null, outcome: 'auth_failed' },
  { status: 408, bodyCode: null, outcome: 'unreachable' },
  { status: 429, bodyCode: null, outcome: 'provider_exhausted' },
];

const NUMERIC_AUTH_CODES = new Set([9103, 9109, 10000, 10001]);

function sameCode(left: number | string | undefined, right: number | string): boolean {
  return String(left) === String(right);
}

function responseBodyCode(body: string | undefined): number | string | undefined {
  if (body === undefined || Buffer.byteLength(body, 'utf8') > MAX_RESPONSE_BYTES) return undefined;
  try {
    const parsed: unknown = JSON.parse(body);
    if (typeof parsed !== 'object' || parsed === null) return undefined;
    const record = parsed as Record<string, unknown>;
    if (Array.isArray(record.errors)) {
      const first = record.errors[0];
      if (typeof first === 'object' && first !== null && 'code' in first) {
        const code = (first as Record<string, unknown>).code;
        if (typeof code === 'number' || typeof code === 'string') return code;
      }
    }
    if (typeof record.error === 'object' && record.error !== null && 'code' in record.error) {
      const code = (record.error as Record<string, unknown>).code;
      if (typeof code === 'number' || typeof code === 'string') return code;
    }
  } catch {
    return undefined;
  }
  return undefined;
}

function isAuthCode(code: number | string | undefined): boolean {
  if (typeof code === 'number') return NUMERIC_AUTH_CODES.has(code);
  if (typeof code !== 'string') return false;
  return /auth|permission|unauthori[sz]ed|forbidden|invalid[-_ ]?(?:api[-_ ]?)?key|access[-_ ]?denied/i.test(
    code,
  );
}

function classifyApiError(error: ApiError): {
  reason: FailureReason;
  retry: boolean;
  exhaustedSignal: boolean;
  detail: string;
} {
  const status = error.statusCode;
  const bodyCode = responseBodyCode(error.responseBody);
  if (isAuthCode(bodyCode)) {
    return {
      reason: 'auth_failed',
      retry: false,
      exhaustedSignal: false,
      detail: 'provider authentication failed',
    };
  }

  const row = ERROR_OUTCOME_ROWS.find(
    (candidate) =>
      (candidate.status === null || candidate.status === status) &&
      (candidate.bodyCode === null || sameCode(bodyCode, candidate.bodyCode)),
  );
  const reason = row?.outcome ?? 'unreachable';
  const retry =
    status === 408 || sameCode(bodyCode, 3007) || (status === 429 && sameCode(bodyCode, 3040));
  return {
    reason,
    retry,
    exhaustedSignal: status === 429 && sameCode(bodyCode, 3036),
    detail: `provider request failed${status === undefined ? '' : ` with HTTP ${status}`}`,
  };
}

function findCause(error: unknown, predicate: (value: unknown) => boolean): unknown {
  let current: unknown = error;
  for (let depth = 0; depth < 5; depth += 1) {
    if (predicate(current)) return current;
    if (typeof current !== 'object' || current === null) return undefined;
    current = 'cause' in current ? current.cause : undefined;
  }
  return undefined;
}

function hasErrorName(error: unknown, names: readonly string[]): boolean {
  return (
    findCause(
      error,
      (value) =>
        typeof value === 'object' &&
        value !== null &&
        'name' in value &&
        typeof value.name === 'string' &&
        names.includes(value.name),
    ) !== undefined
  );
}

function isAbort(error: unknown): boolean {
  return hasErrorName(error, ['AbortError', 'TimeoutError', 'ResponseAborted']);
}

function findApiError(
  error: unknown,
  isInstance: (value: unknown) => boolean,
): ApiError | undefined {
  return findCause(error, isInstance) as ApiError | undefined;
}

async function responseWithinLimit(response: Response): Promise<Response> {
  const declaredLength = Number(response.headers.get('content-length'));
  if (Number.isFinite(declaredLength) && declaredLength > MAX_RESPONSE_BYTES) {
    const error = new Error('provider response exceeded 1 MB');
    error.name = 'ResponseTooLargeError';
    throw error;
  }
  if (response.body === null) return response;

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  while (true) {
    const chunk = await reader.read();
    if (chunk.done) break;
    size += chunk.value.byteLength;
    if (size > MAX_RESPONSE_BYTES) {
      try {
        await reader.cancel();
      } catch {
        // The size failure is authoritative.
      }
      const error = new Error('provider response exceeded 1 MB');
      error.name = 'ResponseTooLargeError';
      throw error;
    }
    chunks.push(chunk.value);
  }

  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new Response(size === 0 ? null : bytes, {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  });
}

function normalizeRuntimeModelId(model: string): string {
  return model.replace(/\[1m\]$/, '').replace(/-build$/, '');
}

function responseHeader(
  headers: Record<string, string> | undefined,
  names: readonly string[],
): string | undefined {
  if (headers === undefined) return undefined;
  for (const [name, value] of Object.entries(headers)) {
    if (names.includes(name.toLowerCase())) return value;
  }
  return undefined;
}

function neuronsFrom(
  headers: Record<string, string> | undefined,
  usage: { inputTokens?: number; outputTokens?: number },
): number | null {
  const header = responseHeader(headers, ['cf-aig-neurons', 'cf-neurons']);
  if (header !== undefined) {
    const parsed = Number(header);
    if (Number.isFinite(parsed) && parsed >= 0) return parsed;
  }
  const input = usage.inputTokens;
  const output = usage.outputTokens;
  if (input === undefined && output === undefined) return null;
  return (
    ((input ?? 0) * INPUT_NEURONS_PER_MILLION_TOKENS +
      (output ?? 0) * OUTPUT_NEURONS_PER_MILLION_TOKENS) /
    1_000_000
  );
}

function parseOutput(
  text: string,
  input: ObserverInput,
): { ok: true; output: ObserverOutput } | { ok: false; detail: string } {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ok: false, detail: 'provider response was not valid JSON' };
  }
  const validated = validateObserverOutput(parsed, input);
  return validated.ok
    ? { ok: true, output: validated.output }
    : { ok: false, detail: validated.detail };
}

export function buildSummarizerPrompt(
  input: ObserverInput,
  mode: 'schema' | 'text-json',
): { system: string; user: string } {
  const lines = [
    'Produce observations from the supplied input.',
    `Allowed observation types: ${OBSERVATION_TYPES.join(', ')}.`,
    `Allowed concepts: ${CONCEPTS.join(', ')}.`,
    'Citations must name only files and commits supported by the supplied events; use empty arrays when there are none.',
    'Every observation must have non-empty source_event_ids chosen from the events list only.',
    'Classify each observation as add, update, delete, or noop against the supplied nearby records.',
    'Keep identifiers, tokens, codes, file names, error text and any string the developer marks as exact verbatim in titles and bodies; never translate or paraphrase them.',
    'Answer in the dominant language of the input.',
  ];
  if (mode === 'text-json') {
    lines.push(
      'Reply with exactly one JSON object matching this schema:',
      JSON.stringify(observerOutputJsonSchema),
    );
  }
  return { system: lines.join('\n'), user: JSON.stringify(input) };
}

type SummarizeContext = {
  preset: PresetName | 'none';
  model: string;
  agentCli?: AgentCli;
  credentials: Credentials;
  consentOk: () => boolean;
  reserve: () =>
    | { ok: true; reservationId: string }
    | { ok: false; reason: 'daily_cap' | 'provider_exhausted' };
  onExhausted: (reservationId: string) => void;
  fetch?: typeof globalThis.fetch;
  spawn?: typeof spawn;
  now?: () => number;
  timeoutMs?: number;
};

function failure(reason: FailureReason, attempts: number, detail: string): CallOutcome {
  return { ok: false, reason, attempts, detail };
}

async function summarizeWithAgentCli(
  input: ObserverInput,
  ctx: SummarizeContext,
): Promise<CallOutcome> {
  if (!ctx.credentials.present || ctx.model.trim() === '') {
    return failure('no_provider', 0, 'the agent-cli preset is not configured');
  }
  const prompt = buildSummarizerPrompt(input, 'text-json');
  const childPrompt = `${prompt.system}\n\nInput JSON:\n${prompt.user}`;
  let attempts = 0;
  while (attempts < 2) {
    if (!ctx.consentOk()) {
      return failure('consent_changed', attempts, 'observer consent changed before the child process');
    }
    attempts += 1;
    const result = await runAgentCli(ctx.agentCli ?? 'claude', childPrompt, {
      timeoutMs: ctx.timeoutMs ?? (testFault('provider-hang') ? 500 : REQUEST_TIMEOUT_MS),
      ...(ctx.spawn === undefined ? {} : { spawn: ctx.spawn }),
    });
    if ('error' in result) {
      if (result.error === 'timeout') {
        return failure('timeout', attempts, 'the agent CLI timed out');
      }
      if (result.error === 'invalid_output' && attempts < 2) continue;
      return failure(
        result.error === 'invalid_output' ? 'unusable_output' : 'unreachable',
        attempts,
        result.error === 'invalid_output'
          ? 'the agent CLI did not return its documented JSON output'
          : 'the agent CLI process failed',
      );
    }
    if (Buffer.byteLength(result.text, 'utf8') > MAX_RESPONSE_BYTES) {
      return failure('unusable_output', attempts, 'provider response exceeded 1 MB');
    }
    const parsed = parseOutput(result.text, input);
    if (parsed.ok) {
      return {
        ok: true,
        output: parsed.output,
        resolvedModel: null,
        neurons: null,
        attempts,
      };
    }
    if (attempts >= 2) return failure('unusable_output', attempts, parsed.detail);
  }
  return failure('unusable_output', attempts, 'the agent CLI response was unusable');
}

export async function summarizeWithProvider(
  input: ObserverInput,
  ctx: SummarizeContext,
): Promise<CallOutcome> {
  if (ctx.preset === 'none' || !ctx.credentials.present || ctx.model.trim() === '') {
    return failure('no_provider', 0, 'no usable observer provider is configured');
  }
  if (ctx.preset === 'agent-cli') return await summarizeWithAgentCli(input, ctx);

  const requestOptions = providerRequestOptions(ctx.preset);
  const prompt = buildSummarizerPrompt(
    input,
    requestOptions.structured === 'text-json' ? 'text-json' : 'schema',
  );
  const transportFetch = faultFetch(ctx.fetch ?? globalThis.fetch);
  let capturedHeaders: Record<string, string> | undefined;
  const captureFetch: typeof globalThis.fetch = async (request, init) => {
    const response = await transportFetch(request, init);
    capturedHeaders = Object.fromEntries(response.headers.entries());
    return await responseWithinLimit(response);
  };

  const model = await createLanguageModel(ctx.preset, ctx.model, ctx.credentials, {
    fetch: captureFetch,
  }).catch(() => null);
  if (model === null) {
    return failure('no_provider', 0, 'the selected observer provider is not configured');
  }

  const { APICallError, generateText, Output } = await import('ai');
  const baseOutput =
    requestOptions.structured === 'text-json'
      ? undefined
      : requestOptions.structured === 'json_schema'
        ? Output.object({ schema: observerOutputSchema, name: 'observer_output' })
        : Output.json();
  const output =
    baseOutput === undefined
      ? undefined
      : {
          ...baseOutput,
          async parseCompleteOutput({ text }: { text: string }) {
            // Parsing stays here so the 1 MB check happens first.
            return text;
          },
          async parsePartialOutput({ text }: { text: string }) {
            return { partial: text };
          },
        };
  let attempts = 0;
  while (attempts < 2) {
    if (!ctx.consentOk()) {
      return failure('consent_changed', attempts, 'observer consent changed before reservation');
    }
    const reservation = ctx.reserve();
    if (!reservation.ok) {
      return failure(reservation.reason, attempts, `provider reservation refused: ${reservation.reason}`);
    }
    if (!ctx.consentOk()) {
      return failure('consent_changed', attempts, 'observer consent changed before the provider call');
    }

    attempts += 1;
    capturedHeaders = undefined;
    try {
      const result = await generateText({
        model,
        system: prompt.system,
        prompt: prompt.user,
        maxRetries: 0,
        abortSignal: AbortSignal.timeout(
          ctx.timeoutMs ?? (testFault('provider-hang') ? 500 : REQUEST_TIMEOUT_MS),
        ),
        ...(requestOptions.providerOptions === undefined
          ? {}
          : {
              providerOptions: requestOptions.providerOptions as ProviderOptions,
            }),
        ...(output === undefined ? {} : { output }),
      });

      const resolvedModel = result.response.modelId || null;
      if (
        resolvedModel !== null &&
        normalizeRuntimeModelId(resolvedModel) !== normalizeRuntimeModelId(ctx.model)
      ) {
        return failure('model_alias', attempts, 'the provider returned a different model id');
      }
      if (result.finishReason === 'length') {
        if (attempts < 2) continue;
        return failure('unusable_output', attempts, 'provider output reached its length limit');
      }
      if (result.text.trim() === '') {
        if (attempts < 2) continue;
        return failure('unusable_output', attempts, 'provider response contained no text');
      }
      if (Buffer.byteLength(result.text, 'utf8') > MAX_RESPONSE_BYTES) {
        return failure('unusable_output', attempts, 'provider response exceeded 1 MB');
      }

      const parsed = parseOutput(result.text, input);
      if (!parsed.ok) {
        if (attempts < 2) continue;
        return failure('unusable_output', attempts, parsed.detail);
      }
      // A11: the crash window between a parsed response and its fenced apply, as a real kill -9
      // (a throw would reach releaseForExit and release the lease, which the fault must skip).
      if (testFault('worker-kill-after-response')) process.kill(process.pid, 'SIGKILL');
      return {
        ok: true,
        output: parsed.output,
        resolvedModel,
        neurons: neuronsFrom(result.response.headers ?? capturedHeaders, result.usage),
        attempts,
      };
    } catch (error) {
      if (isAbort(error)) return failure('timeout', attempts, 'the provider call timed out');
      if (hasErrorName(error, ['ResponseTooLargeError'])) {
        return failure('unusable_output', attempts, 'provider response exceeded 1 MB');
      }
      const apiError = findApiError(error, APICallError.isInstance);
      if (apiError === undefined) {
        return failure('unreachable', attempts, 'the provider call failed without an HTTP status');
      }
      const classified = classifyApiError(apiError);
      if (classified.exhaustedSignal) ctx.onExhausted(reservation.reservationId);
      if (classified.retry && attempts < 2) continue;
      return failure(classified.reason, attempts, classified.detail);
    }
  }
  return failure('unusable_output', attempts, 'provider output was unusable');
}
