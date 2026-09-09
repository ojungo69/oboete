// Hook routing (T046). The builders own pack text; this module only chooses
// the native channel, confirms delivery, and drives Grok's deferred state machine.
import type { DatabaseSync } from 'node:sqlite';

import type { OboeteConfig } from '../config.js';
import { latestSessionState } from '../db/queries.js';
import type { AgentName, NormalizedEvent } from '../events.js';
import { appendLogQuietly, errorCode } from '../log.js';
import type { OboetePaths } from '../paths.js';
import { detectSync } from '../privacy/detect.js';
import { DIRECTIVE_PHRASES } from '../observer/classify.js';
import { transactionImmediate } from '../worker/lease.js';
import { CHANNEL_CAPS } from './budget.js';
import {
  attachOnPreToolUse,
  closeOnStop,
  confirmOnPostToolUse,
  markFailure,
  storePending,
  type PackValidation,
} from './deferred.js';
import { confirmDelivery, sessionStartAttempted } from './ledger.js';
import {
  buildPromptPack,
  buildSessionStartPack,
  markInjectedMemories,
  type BuiltPack,
  type PackChannelInput,
} from './pack.js';

const SUMMARY_POLL_MS = 50;

export type HookContext = {
  agent: AgentName;
  eventName: string;
  event: NormalizedEvent;
  sessionId: string;
  conversationId: string;
  turnId?: string | null;
  epoch: number;
  repoId: string;
  repoIdentityDisplay: string;
  repoRoot: string;
  model: string | undefined;
  cwd: string;
  config: OboeteConfig;
  paths: OboetePaths;
  db?: DatabaseSync;
  /** True only when capture inserted this native session during the current hook (A18). */
  sessionCreated?: boolean;
  /** The combined global and repository path rules already read by capture. */
  secretPaths?: readonly string[];
  /** Remaining milliseconds in this hook's absolute budget. */
  remainingBudget(): number;
  /** Test clock seam; production uses a blocking sleep because the callback in pack.ts is sync. */
  sleep?: (milliseconds: number) => void;
};

export function sleep(milliseconds: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

export function indexUnavailable(context: Pick<HookContext, 'agent' | 'eventName' | 'paths'>): string {
  appendLogQuietly(context.paths.hookLog, 'warn', 'injection unavailable', {
    agent: context.agent,
    event: context.eventName,
    degraded: 'index_unavailable',
  });
  return '';
}

function detectorFor(context: HookContext): PackValidation['detect'] {
  return async (text) => {
    if (context.remainingBudget() <= 0) return true;
    const result = await detectSync({
      text,
      paths: [],
      repoRoot: context.repoRoot,
      secretPaths: [...(context.secretPaths ?? context.config.privacy.secret_paths)],
    });
    return (
      !result.ok ||
      context.remainingBudget() <= 0 ||
      result.sensitivity === 'secret' ||
      result.privateRemoved > 0 ||
      result.text !== text
    );
  };
}

function validationFor(context: HookContext): PackValidation {
  return { detect: detectorFor(context), directives: DIRECTIVE_PHRASES };
}

function waitForSummary(context: HookContext, waitMs: number): 'ready' | 'pending' | 'none' {
  const db = context.db;
  if (db === undefined) return 'none';
  const pause = context.sleep ?? sleep;
  let waited = 0;

  for (;;) {
    const state = latestSessionState(db, context.repoId)?.summaryState;
    if (state === 'done') return 'ready';
    if (state !== 'pending') return 'none';

    const remaining = Math.min(waitMs - waited, Math.floor(context.remainingBudget()));
    if (remaining <= 0) return 'pending';
    const interval = Math.min(SUMMARY_POLL_MS, remaining);
    pause(interval);
    waited += interval;
  }
}

function packInput(
  context: HookContext,
  channel: string,
  validation: PackValidation,
): PackChannelInput {
  return {
    agent: context.agent,
    repoId: context.repoId,
    repoIdentityDisplay: context.repoIdentityDisplay,
    sessionId: context.sessionId,
    conversationId: context.conversationId,
    turnId: context.turnId ?? null,
    epoch: context.epoch,
    model: context.model,
    channelCap: CHANNEL_CAPS[context.agent],
    contextFraction: context.config.injection.context_fraction,
    channel,
    now: context.event.captured_at,
    detect: validation.detect,
    directives: validation.directives,
    repoRoot: context.repoRoot,
    remainingBudget: context.remainingBudget,
  };
}

async function startPack(
  context: HookContext,
  channel: string,
  validation: PackValidation,
  pending = false,
): Promise<BuiltPack | null> {
  const db = context.db;
  if (db === undefined) return null;
  return buildSessionStartPack(db, {
    ...packInput(context, channel, validation),
    state: pending ? 'pending' : 'built',
    waitForSummary: (waitMs) => waitForSummary(context, waitMs),
  });
}

async function promptPack(
  context: HookContext,
  channel: string,
  prompt: string,
  validation: PackValidation,
  pending = false,
): Promise<BuiltPack | null> {
  const db = context.db;
  if (db === undefined) return null;
  return buildPromptPack(db, {
    ...packInput(context, channel, validation),
    state: pending ? 'pending' : 'built',
    prompt,
    threshold: context.config.injection.threshold,
  });
}

function includedMemoryIds(db: DatabaseSync, injectionId: string): string[] {
  return db
    .prepare(
      `SELECT memory_id FROM injection_items
       WHERE injection_id = ? AND decision = 'included' AND memory_id IS NOT NULL`,
    )
    .all(injectionId)
    .map((row) => String(row.memory_id));
}

function confirm(db: DatabaseSync, pack: BuiltPack, now: number): void {
  confirmDelivery(db, pack.injectionId, now);
  markInjectedMemories(
    db,
    pack.items
      .filter((item) => item.decision === 'planned' && item.memoryId !== null)
      .map((item) => item.memoryId as string),
    now,
  );
}

function markLatestDeferred(context: HookContext): void {
  const db = context.db;
  if (db === undefined) return;
  transactionImmediate(db, () => {
    const row = db
      .prepare(
        `SELECT id FROM injections
         WHERE conversation_id = ? AND state = 'emitted'
         ORDER BY emitted_at DESC, id DESC LIMIT 1`,
      )
      .get(context.conversationId);
    if (row === undefined) return;
    markInjectedMemories(
      db,
      includedMemoryIds(db, String(row.id)),
      context.event.captured_at,
    );
  });
}

function envelope(eventName: string, pack: string): string {
  return JSON.stringify({
    hookSpecificOutput: { hookEventName: eventName, additionalContext: pack },
  });
}

function sawToolHook(context: HookContext): boolean {
  const db = context.db;
  if (db === undefined) return false;
  const turnId = context.turnId ?? null;
  if (turnId !== null) {
    return (
      db
        .prepare(
          `SELECT 1 AS found FROM raw_events
           WHERE session_id = ? AND turn_id = ?
             AND kind IN ('tool_call', 'tool_result', 'tool_failure') LIMIT 1`,
        )
        .get(context.sessionId, turnId) !== undefined
    );
  }
  return (
    db
      .prepare(
        `SELECT 1 AS found FROM raw_events
         WHERE session_id = ? AND kind IN ('tool_call', 'tool_result', 'tool_failure')
           AND captured_at >= COALESCE(
             (SELECT MAX(captured_at) FROM raw_events WHERE session_id = ? AND kind = 'prompt'),
             0
           )
         LIMIT 1`,
      )
      .get(context.sessionId, context.sessionId) !== undefined
  );
}

function immediate(
  context: HookContext,
  packs: readonly (BuiltPack | null)[],
  eventName?: string,
): string {
  const db = context.db;
  if (db === undefined) return '';
  const built = packs.filter((pack): pack is BuiltPack => pack !== null);
  for (const pack of built) confirm(db, pack, context.event.captured_at);
  const text = built.map((pack) => pack.text).join('\n');
  if (text === '') return '';
  return eventName === undefined ? text : envelope(eventName, text);
}

async function injectClaude(context: HookContext, validation: PackValidation): Promise<string> {
  if (context.eventName === 'SessionStart' && context.event.kind === 'session_start') {
    if (!['startup', 'clear', 'compact'].includes(context.event.source)) return '';
    return immediate(
      context,
      [await startPack(context, 'claude:SessionStart', validation)],
    );
  }
  if (context.eventName === 'UserPromptSubmit' && context.event.kind === 'prompt') {
    return immediate(
      context,
      [await promptPack(context, 'claude:UserPromptSubmit', context.event.text, validation)],
    );
  }
  return '';
}

async function injectCodex(context: HookContext, validation: PackValidation): Promise<string> {
  if (context.eventName === 'SessionStart' && context.event.kind === 'session_start') {
    if (!['startup', 'clear', 'compact'].includes(context.event.source)) return '';
    return immediate(
      context,
      [await startPack(context, 'codex:SessionStart', validation)],
      'SessionStart',
    );
  }
  if (context.eventName !== 'UserPromptSubmit' || context.event.kind !== 'prompt') return '';

  const db = context.db;
  const packs: BuiltPack[] = [];
  // A18 (`/new` fires no SessionStart) and A21 (a SessionStart that spooled or was cut off after
  // PostCompact): the first prompt of an epoch that has no session-start pack in the ledger carries it.
  if (db !== undefined && !sessionStartAttempted(db, context.conversationId, context.epoch)) {
    const start = await startPack(context, 'codex:UserPromptSubmit', validation);
    // Delivery is immediate; confirming before prompt retrieval keeps a matching pinned or summary
    // memory from appearing twice in the one additionalContext value (FR-026).
    if (start !== null) {
      confirm(db, start, context.event.captured_at);
      packs.push(start);
    }
  }
  const prompt = await promptPack(
    context,
    'codex:UserPromptSubmit',
    context.event.text,
    validation,
  );
  if (prompt !== null && db !== undefined) {
    confirm(db, prompt, context.event.captured_at);
    packs.push(prompt);
  }
  const text = packs.map((pack) => pack.text).join('\n');
  return text === '' ? '' : envelope('UserPromptSubmit', text);
}

async function deferPack(
  context: HookContext,
  pack: BuiltPack | null,
  validation: PackValidation,
): Promise<void> {
  if (pack === null || context.db === undefined) return;
  context.db
    .prepare("UPDATE injections SET kind = 'grok_deferred' WHERE id = ?")
    .run(pack.injectionId);
  await storePending(context.db, {
    conversationId: context.conversationId,
    epoch: context.epoch,
    pack,
    now: context.event.captured_at,
    validation,
  });
}

/** The Grok lane builds a pack on a session or prompt event and defers it to the next tool call. */
async function deferGrokPack(context: HookContext, validation: PackValidation): Promise<void> {
  if (context.eventName === 'SessionStart' && context.event.kind === 'session_start') {
    // Grok reports both resume and --fork-session as `load`: only a new native id opens a root.
    if (context.event.source === 'resume' && !context.sessionCreated) return;
    await deferPack(context, await startPack(context, 'grok:PreToolUse', validation, true), validation);
    return;
  }
  if (context.eventName === 'UserPromptSubmit' && context.event.kind === 'prompt') {
    await deferPack(
      context,
      await promptPack(context, 'grok:PreToolUse', context.event.text, validation, true),
      validation,
    );
  }
}

/** The Grok lane attaches the deferred pack to the tool call itself. */
function grokOnToolCall(context: HookContext, db: DatabaseSync, toolCallId: string): string {
  const text = attachOnPreToolUse(db, {
    conversationId: context.conversationId,
    toolCallId,
    now: context.event.captured_at,
  });
  return text === null ? '' : envelope('PreToolUse', text);
}

/** The tool's result confirms delivery, or carries the pack when the call could not. */
function grokOnToolResult(
  context: HookContext,
  db: DatabaseSync,
  toolCallId: string,
  isError: boolean,
): string {
  const delivered = confirmOnPostToolUse(db, {
    conversationId: context.conversationId,
    toolCallId,
    exitCode: isError ? 1 : 0,
    now: context.event.captured_at,
  });
  if (delivered.status === 'emitted') markLatestDeferred(context);
  return delivered.text === null ? '' : envelope('PostToolUse', delivered.text);
}

/** A failed or denied tool call still resolves the deferred row. */
function grokOnToolFailure(
  context: HookContext,
  db: DatabaseSync,
  toolCallId: string,
  kind: 'PostToolUseFailure' | 'PermissionDenied',
): void {
  const state = markFailure(db, {
    conversationId: context.conversationId,
    toolCallId,
    kind,
    now: context.event.captured_at,
  });
  if (state === 'emitted') markLatestDeferred(context);
}

/** The Grok lane's answer on a tool hook: attach on the call, confirm or mark on its outcome. */
function grokToolHook(context: HookContext, db: DatabaseSync): string {
  const event = context.event;
  if (context.eventName === 'PreToolUse' && event.kind === 'tool_call') {
    return grokOnToolCall(context, db, event.tool_call_id);
  }
  if (context.eventName === 'PostToolUse' && event.kind === 'tool_result') {
    return grokOnToolResult(context, db, event.tool_call_id, event.is_error);
  }
  if (
    (context.eventName === 'PostToolUseFailure' || context.eventName === 'PermissionDenied') &&
    event.kind === 'tool_failure'
  ) {
    grokOnToolFailure(context, db, event.tool_call_id, context.eventName);
    return '';
  }
  if (context.eventName === 'Stop') {
    closeOnStop(db, {
      conversationId: context.conversationId,
      sawAnyToolHook: sawToolHook(context),
      now: event.captured_at,
    });
  }
  return '';
}

async function injectGrok(context: HookContext, validation: PackValidation): Promise<string> {
  const db = context.db;
  if (db === undefined) return '';
  const isPackEvent =
    (context.eventName === 'SessionStart' && context.event.kind === 'session_start') ||
    (context.eventName === 'UserPromptSubmit' && context.event.kind === 'prompt');
  if (isPackEvent) {
    await deferGrokPack(context, validation);
    return '';
  }
  return grokToolHook(context, db);
}

/** Called by capture after the normalized event was stored, or with no database after spooling. */
export async function injectForHook(context: HookContext): Promise<string> {
  if (context.agent === 'pi' || context.agent === 'unknown') return '';
  if (context.db === undefined) return indexUnavailable(context);
  if (context.remainingBudget() <= 0) return '';

  try {
    const validation = validationFor(context);
    if (context.agent === 'claude') return await injectClaude(context, validation);
    if (context.agent === 'codex') return await injectCodex(context, validation);
    if (context.agent === 'grok') return await injectGrok(context, validation);
    return '';
  } catch (error) {
    appendLogQuietly(context.paths.hookLog, 'error', 'injection failed', {
      agent: context.agent,
      event: context.eventName,
      reason: errorCode(error),
    });
    return '';
  }
}

export async function injectPi(context: HookContext, kind: 'start' | 'prompt', prompt: string): Promise<string> {
  const validation = validationFor(context);
  if (kind === 'start') {
    return immediate(
      context,
      [await startPack(context, 'pi:before_agent_start', validation)],
    );
  }
  return immediate(
    context,
    [await promptPack(context, 'pi:before_agent_start', prompt, validation)],
  );
}
