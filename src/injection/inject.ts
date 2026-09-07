// Injection command and hook routing (T046). The builders own pack text; this module only chooses
// the native channel, confirms delivery, and drives Grok's deferred state machine.
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';

import { z } from 'zod';

import { isPaused, loadConfig, loadRepoRules, type OboeteConfig } from '../config.js';
import { latestSessionState } from '../db/queries.js';
import { openDatabase } from '../db/open.js';
import type { AgentName, NormalizedEvent } from '../events.js';
import { appendLogQuietly, errorCode } from '../log.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from '../paths.js';
import { detectSync } from '../privacy/detect.js';
import { resolveRepoIdentity, type RepoIdentity } from '../repo-identity.js';
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

/** Pi's bounded child gets 300 ms, plus A2's one-second wait at session start. */
const PI_INJECTION_DEADLINE_MS = 300;
const PI_SESSION_START_DEADLINE_MS = 1_300;
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

export type InjectRuntime = {
  readStdin(): string;
  now(): number;
  elapsedMs(): number;
  sleep(milliseconds: number): void;
};

export const piInjectInputSchema = z.strictObject({
  cwd: z.string().min(1),
  session_id: z.string().min(1),
  prompt: z.string().optional(),
  model: z.string().min(1).optional(),
});

function sleep(milliseconds: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

function indexUnavailable(context: Pick<HookContext, 'agent' | 'eventName' | 'paths'>): string {
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

async function injectPi(context: HookContext, kind: 'start' | 'prompt', prompt: string): Promise<string> {
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

function defaultRuntime(): InjectRuntime {
  return {
    readStdin: () => readFileSync(0, 'utf8'),
    now: () => Date.now(),
    elapsedMs: () => performance.now(),
    sleep,
  };
}

function sessionForPi(
  db: DatabaseSync,
  input: { nativeSessionId: string; identity: RepoIdentity; model: string | undefined; now: number },
): { sessionId: string; conversationId: string; epoch: number; model: string | undefined; turnId: string | null } {
  return transactionImmediate(db, () => {
    let row = db
      .prepare(
        `SELECT id, conversation_id, model FROM sessions
         WHERE agent = 'pi' AND native_session_id = ?`,
      )
      .get(input.nativeSessionId);
    if (row === undefined) {
      db.prepare(
        `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET display_root = excluded.display_root,
           last_seen_at = excluded.last_seen_at`,
      ).run(
        input.identity.id,
        input.identity.identityKind,
        input.identity.normalizedIdentity,
        input.identity.root,
        input.now,
        input.now,
      );
      const id = randomUUID();
      db.prepare(
        `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
           started_at, status, turn_count, context_epoch)
         VALUES (?, ?, 'pi', ?, ?, ?, ?, 'active', 0, 0)`,
      ).run(id, input.identity.id, input.nativeSessionId, id, input.model ?? null, input.now);
      row = { id, conversation_id: id, model: input.model ?? null };
    } else if (input.model !== undefined && row.model === null) {
      db.prepare('UPDATE sessions SET model = ? WHERE id = ?').run(input.model, row.id);
      row = { ...row, model: input.model };
    }

    const sessionId = String(row.id);
    const conversationId = String(row.conversation_id);
    const root = db.prepare('SELECT context_epoch FROM sessions WHERE id = ?').get(conversationId);
    const turn = db
      .prepare('SELECT id FROM turns WHERE session_id = ? ORDER BY ordinal DESC LIMIT 1')
      .get(sessionId);
    return {
      sessionId,
      conversationId,
      epoch: Number(root?.context_epoch ?? 0),
      model: row.model === null ? undefined : String(row.model),
      turnId: turn === undefined ? null : String(turn.id),
    };
  });
}

type PiInjectInput = z.infer<typeof piInjectInputSchema>;

/** The `--agent pi --kind start|prompt` arguments, refused as one error for the hook log. */
function piInjectKind(argv: string[]): 'start' | 'prompt' {
  const { values } = parseArgs({
    args: argv,
    strict: false,
    allowPositionals: true,
    options: { agent: { type: 'string' }, kind: { type: 'string' } },
  });
  const kind = values.kind;
  if (values.agent !== 'pi' || (kind !== 'start' && kind !== 'prompt')) {
    throw new Error('inject_arguments_invalid');
  }
  return kind;
}

/** The event Pi's injection hook stands for: the session's start, or the prompt it carries. */
function piInjectEvent(
  kind: 'start' | 'prompt',
  input: PiInjectInput,
  model: string | undefined,
  now: number,
): NormalizedEvent {
  const common = {
    agent: 'pi' as const,
    native_session_id: input.session_id,
    cwd: input.cwd,
    captured_at: now,
    model: input.model ?? model,
  };
  if (kind === 'start') return { ...common, kind: 'session_start', source: 'startup' };
  return { ...common, kind: 'prompt', text: input.prompt ?? '', input_source: 'user' };
}

/** Opens the database for the hook, or answers null once it says the index is unavailable. */
function openForInject(
  paths: OboetePaths,
  kind: 'start' | 'prompt',
  remainingBudget: () => number,
): ReturnType<typeof openDatabase> | null {
  let opened: ReturnType<typeof openDatabase>;
  try {
    const timeoutMs = Math.max(1, Math.min(150, Math.floor(remainingBudget())));
    opened = openDatabase({ path: paths.db, timeoutMs, hook: true });
  } catch {
    indexUnavailable({ agent: 'pi', eventName: kind, paths });
    return null;
  }
  if (opened.schemaBehind) {
    opened.db.close();
    indexUnavailable({ agent: 'pi', eventName: kind, paths });
    return null;
  }
  return opened;
}

/** The hook context Pi's in-process injection runs against; one place assembles it. */
function piHookContext(input: {
  kind: ReturnType<typeof piInjectKind>;
  input: PiInjectInput;
  session: ReturnType<typeof sessionForPi>;
  identity: ReturnType<typeof resolveRepoIdentity>;
  config: HookContext['config'];
  paths: HookContext['paths'];
  db: DatabaseSync;
  secretPaths: HookContext['secretPaths'];
  remainingBudget: HookContext['remainingBudget'];
  sleep: HookContext['sleep'];
  now: number;
}): HookContext {
  const { session, identity } = input;
  return {
    agent: 'pi',
    eventName: input.kind,
    event: piInjectEvent(input.kind, input.input, session.model, input.now),
    sessionId: session.sessionId,
    conversationId: session.conversationId,
    turnId: session.turnId,
    epoch: session.epoch,
    repoId: identity.id,
    repoIdentityDisplay: identity.normalizedIdentity,
    repoRoot: identity.root,
    model: input.input.model ?? session.model,
    cwd: input.input.cwd,
    config: input.config,
    paths: input.paths,
    db: input.db,
    sessionCreated: false,
    secretPaths: input.secretPaths,
    remainingBudget: input.remainingBudget,
    sleep: input.sleep,
  };
}

/** Opens the Pi session on the already-open database and writes the pack it produces to stdout. */
async function writePiInjection(input: {
  kind: ReturnType<typeof piInjectKind>;
  input: PiInjectInput;
  identity: ReturnType<typeof resolveRepoIdentity>;
  config: HookContext['config'];
  paths: HookContext['paths'];
  db: DatabaseSync;
  secretPaths: HookContext['secretPaths'];
  remainingBudget: HookContext['remainingBudget'];
  sleep: HookContext['sleep'];
  now: number;
}): Promise<void> {
  const session = sessionForPi(input.db, {
    nativeSessionId: input.input.session_id,
    identity: input.identity,
    model: input.input.model,
    now: input.now,
  });
  const text = await injectPi(
    piHookContext({ ...input, session }),
    input.kind,
    input.input.prompt ?? '',
  );
  if (text !== '') process.stdout.write(text);
}

/** `oboete inject --agent pi --kind start|prompt`; agent-facing failures always return zero. */
export async function runInject(
  argv: string[],
  runtime: Partial<InjectRuntime> = {},
): Promise<number> {
  const paths = oboetePaths(resolveHome());
  const live = { ...defaultRuntime(), ...runtime };

  try {
    const kind = piInjectKind(argv);
    if (isPaused(paths)) return 0;
    const parsed = piInjectInputSchema.safeParse(JSON.parse(live.readStdin()));
    if (!parsed.success) throw new Error('inject_input_invalid');
    ensureDirectories(paths);
    const deadline = kind === 'start' ? PI_SESSION_START_DEADLINE_MS : PI_INJECTION_DEADLINE_MS;
    const remainingBudget = (): number => deadline - live.elapsedMs();

    const identity = resolveRepoIdentity(parsed.data.cwd);
    const config = loadConfig(paths);
    const secretPaths = [
      ...config.privacy.secret_paths,
      ...loadRepoRules(identity.root).secretPaths,
    ];
    if (remainingBudget() <= 0) throw new Error('inject_deadline');
    const opened = openForInject(paths, kind, remainingBudget);
    if (opened === null) return 0;

    try {
      await writePiInjection({
        kind,
        input: parsed.data,
        identity,
        config,
        paths,
        db: opened.db,
        secretPaths,
        remainingBudget,
        sleep: live.sleep,
        now: live.now(),
      });
    } finally {
      opened.db.close();
    }
  } catch (error) {
    appendLogQuietly(paths.hookLog, 'error', 'inject failed', {
      agent: 'pi',
      reason: errorCode(error),
    });
  }
  return 0;
}
