import { appendFileSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';

/** The oboete credential variables of contracts/cli.md: the Cloudflare pair and one key per preset. */
export function isCredentialVariable(name: string): boolean {
  if (name === 'OBOETE_CF_ACCOUNT_ID') return true;
  return name.startsWith('OBOETE_') && (name.endsWith('_API_KEY') || name.endsWith('_API_TOKEN'));
}

/**
 * The environment a process oboete starts on someone else's behalf gets: FR-016 keeps oboete's
 * provider credentials on oboete's own request path, never on an agent CLI's.
 */
export function childEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  return Object.fromEntries(Object.entries(env).filter(([name]) => !isCredentialVariable(name)));
}

/**
 * Replaces the value of every oboete credential variable with `[credential]`.
 * FR-016 and contracts/cli.md: logs and diagnostics never contain a credential value.
 */
export function scrubCredentials(text: string, env: NodeJS.ProcessEnv = process.env): string {
  let scrubbed = text;
  for (const value of credentialValues(env)) scrubbed = scrubbed.split(value).join('[credential]');
  return scrubbed;
}

/**
 * No real credential is shorter than this; a placeholder such as `test` in a credential variable
 * would otherwise redact every row and every log line that contains those letters.
 */
const MIN_CREDENTIAL_LENGTH = 8;

/** The values of the credential variables, longest first so a value that contains another one is replaced whole. */
export function credentialValues(env: NodeJS.ProcessEnv): string[] {
  return Object.entries(env)
    .filter(([name]) => isCredentialVariable(name))
    .map(([, value]) => value?.trim() ?? '')
    .filter((value) => value.length >= MIN_CREDENTIAL_LENGTH)
    .sort((a, b) => b.length - a.length);
}

function formatValue(value: string | number | boolean): string {
  const text = String(value);
  return /[\s"]/.test(text) ? JSON.stringify(text) : text;
}

/** A log-safe name for an error: its `code` when it has one, else its class name (never its message, which can quote captured content). */
export function errorCode(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'code' in error && typeof error.code === 'string') {
    return error.code;
  }
  return error instanceof Error ? error.name : 'unknown';
}

/** Appends one line `<ISO time> <level> <message> key=value ...` (conventions, "CLI and processes"). */
export function appendLog(
  file: string,
  level: 'info' | 'warn' | 'error',
  message: string,
  fields: Record<string, string | number | boolean> = {},
): void {
  const parts = [new Date().toISOString(), level, /[\r\n]/.test(message) ? JSON.stringify(message) : message];
  for (const [key, value] of Object.entries(fields)) parts.push(`${key}=${formatValue(value)}`);
  mkdirSync(dirname(file), { recursive: true, mode: 0o700 });
  appendFileSync(file, `${scrubCredentials(parts.join(' '))}\n`, { mode: 0o600 });
}

/** Appends a line, or nothing: an agent-facing hook exits 0 even with an unwritable data directory. */
export function appendLogQuietly(
  file: string,
  level: 'info' | 'warn' | 'error',
  message: string,
  fields: Record<string, string | number | boolean> = {},
): void {
  try {
    appendLog(file, level, message, fields);
  } catch {
    // FR-002: the diagnostic surface being unavailable must not change the exit code.
  }
}
