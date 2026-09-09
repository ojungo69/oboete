// Pi's bounded injection command (T046); failures keep the agent-facing exit contract.
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';

import { z } from 'zod';

import { isPaused, loadConfig, loadRepoRules } from '../config.js';
import { openDatabase } from '../db/open.js';
import type { NormalizedEvent } from '../events.js';
import { appendLogQuietly, errorCode } from '../log.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from '../paths.js';
import { resolveRepoIdentity, type RepoIdentity } from '../repo-identity.js';
import { transactionImmediate } from '../worker/lease.js';
import { indexUnavailable, injectPi, sleep, type HookContext } from './inject.js';

/** Pi's bounded child gets 300 ms, plus A2's one-second wait at session start. */
const PI_INJECTION_DEADLINE_MS = 300;

const PI_SESSION_START_DEADLINE_MS = 1_300;

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
