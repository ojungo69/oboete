// The trust entry Codex needs before it runs a hook (FR-031). Without a `[hooks.state]` row whose
// hash matches, Codex skips the hook and exits 0 without a word, so the rule here is what decides
// whether a wired Codex session captures anything at all. Codex hashes a normalized,
// config-derived identity rather than the source text, which is why the identity below is rebuilt
// from the handler instead of taken from the file bytes (docs/research/oboete-contracts-2026-09-02.md
// "Codex CLI 0.152.1 hooks", evidence 6 and 7; corrected rule and its verification in
// docs/research/oboete-contracts-probes.md and scripts/e2e/probe-lib/trusthash.mjs).
import { createHash } from 'node:crypto';

export type CodexHandler = {
  type: 'command';
  command: string;
  /** Seconds. Absent means Codex's own default, which is what the identity then carries. */
  timeout?: number;
  additionalContextLimit?: number;
};

/** Codex's normalized timeouts: ten minutes, and one second capped at three for the two late events. */
const DEFAULT_TIMEOUT_SECONDS = 600;
const LATE_TIMEOUT_SECONDS = 1;
const LATE_TIMEOUT_MAX_SECONDS = 3;
/** The default spill limit; the identity carries the limit only when it differs from it. */
const DEFAULT_CONTEXT_LIMIT = 2500;

/** `<absolute hooks.json path>:<snake_case event>:<group index>:<handler index>`. */
export function trustKey(
  hooksPath: string,
  event: string,
  groupIndex: number,
  handlerIndex: number,
): string {
  return `${hooksPath}:${snakeEvent(event)}:${groupIndex}:${handlerIndex}`;
}

/** The value of `trusted_hash`: `sha256:` and the hex digest of the identity. */
export function trustedHash(
  event: string,
  matcher: string | undefined,
  handler: CodexHandler,
): string {
  const digest = createHash('sha256').update(trustIdentity(event, matcher, handler)).digest('hex');
  return `sha256:${digest}`;
}

/**
 * The preimage: the matcher group holding this one handler, with the keys of every object sorted
 * and the handler normalized the way Codex normalizes it before hashing.
 */
export function trustIdentity(
  event: string,
  matcher: string | undefined,
  handler: CodexHandler,
): string {
  const name = snakeEvent(event);
  const normalized: Record<string, unknown> = {
    // oboete never asks for an asynchronous hook, and Codex demotes an asynchronous SessionEnd to a
    // synchronous one regardless; the identity carries the value either way.
    async: false,
    command: handler.command,
    timeout: normalizeTimeout(name, handler.timeout),
    type: handler.type,
  };
  if (
    handler.additionalContextLimit !== undefined &&
    handler.additionalContextLimit !== DEFAULT_CONTEXT_LIMIT
  ) {
    normalized.additionalContextLimit = handler.additionalContextLimit;
  }
  const group: Record<string, unknown> = { event_name: name, hooks: [normalized] };
  if (matcher !== undefined) group.matcher = matcher;
  return canonical(group);
}

function normalizeTimeout(name: string, timeout: number | undefined): number {
  const late = name === 'session_end' || name === 'interrupt';
  if (timeout === undefined) return late ? LATE_TIMEOUT_SECONDS : DEFAULT_TIMEOUT_SECONDS;
  return late ? Math.min(timeout, LATE_TIMEOUT_MAX_SECONDS) : timeout;
}

function snakeEvent(event: string): string {
  return event.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();
}

/** JSON with the keys of every object sorted and no whitespace. */
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    const source = value as Record<string, unknown>;
    const pairs = Object.keys(source)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonical(source[key])}`);
    return `{${pairs.join(',')}}`;
  }
  return JSON.stringify(value);
}
