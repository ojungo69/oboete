import type { FailureReason } from './llm.js';

export const MAX_RESPONSE_BYTES = 1024 * 1024;

type ApiError = { statusCode?: number; responseBody?: string; cause?: unknown };

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

type ProviderCode = number | string;

function sameCode(left: ProviderCode | undefined, right: ProviderCode): boolean {
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

export function classifyApiError(error: ApiError): {
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
  const httpStatus = status === undefined ? '' : ` with HTTP ${status}`;
  return {
    reason,
    retry,
    exhaustedSignal: status === 429 && sameCode(bodyCode, 3036),
    detail: `provider request failed${httpStatus}`,
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

export function hasErrorName(error: unknown, names: readonly string[]): boolean {
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

export function isAbort(error: unknown): boolean {
  return hasErrorName(error, ['AbortError', 'TimeoutError', 'ResponseAborted']);
}

export function findApiError(
  error: unknown,
  isInstance: (value: unknown) => boolean,
): ApiError | undefined {
  return findCause(error, isInstance) as ApiError | undefined;
}
