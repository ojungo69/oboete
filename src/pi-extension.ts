// The extension Pi loads (`~/.pi/agent/extensions/oboete.js`, written by src/setup/write-pi.ts).
// Pi has no handler timeout and awaits every handler in sequence, so anything this file does is
// time the developer waits for (docs/research/oboete-contracts-2026-09-02.md, Pi section). FR-007:
// it therefore performs no storage and no network work itself. It generates an invocation id per
// spawn, hands the work to child processes, counts its own failures in memory by message code and
// gives them to the next capture child as `--prior-failures`, which is the only durable record of
// them (Pi keeps none: research R12 "Pi diagnostics", amendment A8).
import { spawn } from 'node:child_process';
import type { SpawnOptions } from 'node:child_process';

/** The part of Pi's `ExtensionAPI` oboete uses (verified against Pi 0.84.4). */
export type PiApi = {
  on(event: string, handler: (event: unknown, ctx: unknown) => unknown): unknown;
  registerTool?(tool: PiTool): unknown;
};

export type PiTool = {
  name: string;
  label: string;
  description: string;
  parameters: Record<string, unknown>;
  /**
   * Pi calls a tool with the call id first and the arguments second (`ToolDefinition.execute` of
   * pi-coding-agent, verified against 0.85.0). A handler written to take the arguments first
   * receives the call id string instead and every argument reads as undefined.
   */
  execute(
    toolCallId: string,
    params: Record<string, unknown>,
  ): Promise<{ content: { type: 'text'; text: string }[] }>;
};

/** Everything the extension touches on a child process; `node:child_process` `spawn` satisfies it. */
export type SpawnedChild = {
  stdin: { end(chunk: string): void; on(event: 'error', listener: () => void): unknown } | null;
  stdout: {
    setEncoding(encoding: BufferEncoding): unknown;
    on(event: 'data', listener: (chunk: string) => void): unknown;
  } | null;
  on(event: string, listener: (value?: unknown) => void): unknown;
  unref(): unknown;
};

export type SpawnLike = (command: string, args: string[], options: SpawnOptions) => SpawnedChild;

export type PiExtensionOptions = {
  /** Absolute paths written into the loader by setup, so neither is resolved at run time. */
  node: string;
  bundle: string;
  /** Replaced by tests; production always spawns through `node:child_process`. */
  spawn?: SpawnLike;
};

/** contracts/agents.md, Pi row. `before_agent_start` is the injection hook and is not captured. */
const CAPTURE_EVENTS = [
  'session_start',
  'input',
  'tool_result',
  'agent_settled',
  'session_shutdown',
  'session_compact',
] as const;

// contracts/agents.md "Hook process rules and SLAs": the session start waits for a summary that may
// still be pending, every later prompt does not.
const START_DEADLINE_MS = 1_300;
const PROMPT_DEADLINE_MS = 300;
// A tool call is a developer waiting on purpose, not a hook budget; the bound only stops a hang.
const TOOL_DEADLINE_MS = 10_000;
const PROMPT_CAP = 10_000;
// ponytail: bounded counter list, failures beyond it in one drain interval are dropped.
const MAX_FAILURE_CODES = 32;

export function piExtension(pi: PiApi, options: PiExtensionOptions): void {
  const spawnChild = options.spawn ?? spawn;
  const failures: string[] = [];
  let sessionId = '';
  let promptId: string | undefined;
  let prompt = '';
  let injectedThisSession = false;

  const count = (code: string): void => {
    if (failures.length < MAX_FAILURE_CODES) failures.push(code);
  };

  const rememberSession = (ctx: unknown): void => {
    try {
      const manager = (ctx as { sessionManager?: { getSessionId?: () => unknown } } | undefined)
        ?.sessionManager;
      const id = manager?.getSessionId?.();
      if (typeof id === 'string' && id !== '') sessionId = id;
    } catch {
      // The child records the missing session id as an invalid payload; guessing one would be worse.
    }
  };

  /**
   * Resolves with what the child printed and whether it exited cleanly. A child that failed, was
   * aborted or exited non-zero may have printed half a pack; half a pack is never emitted, and the
   * caller counts the failure (a non-zero exit is the ordinary one: it needs no `error` event).
   */
  const readChild = (
    args: string[],
    stdio: SpawnOptions,
    input: string | null,
  ): Promise<{ text: string; ok: boolean }> =>
    new Promise((resolve) => {
      const child = spawnChild(options.node, [options.bundle, ...args], stdio);
      let text = '';
      let settled = false;
      const finish = (ok: boolean): void => {
        if (!settled) {
          settled = true;
          resolve({ text: ok ? text : '', ok });
        }
      };
      child.on('error', () => finish(false));
      child.on('close', (code) => finish(code === 0));
      child.stdout?.setEncoding('utf8');
      child.stdout?.on('data', (chunk) => {
        text += chunk;
      });
      if (input !== null) {
        child.stdin?.on('error', () => undefined); // A dead pipe must not throw inside Pi.
        child.stdin?.end(input);
      }
    });

  const capture = (event: string, payload: unknown, ctx: unknown): undefined => {
    try {
      rememberSession(ctx);
      if (event === 'session_start') {
        injectedThisSession = false;
        promptId = undefined;
        prompt = '';
      }
      if (event === 'input') {
        // Pi supplies no per-turn key, so the extension is the one that gives the turn its identity
        // (src/agents/pi.ts, R7): a new id on every prompt, repeated by the rest of the turn.
        promptId = crypto.randomUUID();
        const text = (payload as { text?: unknown } | undefined)?.text;
        prompt = typeof text === 'string' ? text.slice(0, PROMPT_CAP) : '';
      }
      const envelope = JSON.stringify({
        event,
        session_id: sessionId,
        cwd: process.cwd(),
        ...(promptId === undefined ? {} : { prompt_id: promptId }),
        payload,
      });
      const codes = failures.join(',');
      const child = spawnChild(
        options.node,
        [
          options.bundle,
          'capture',
          '--agent',
          'pi',
          '--event',
          event,
          '--invocation',
          crypto.randomUUID(),
          ...(codes === '' ? [] : ['--prior-failures', codes]),
        ],
        // Pi owns stdout in `--mode json`, so the child must never inherit it.
        { detached: true, stdio: ['pipe', 'ignore', 'ignore'] },
      );
      // The child is the only durable record of these codes (Pi keeps none: R12, amendment A8), so
      // they leave the buffer once it exists and go back in when it turns out never to have started.
      const carried = failures.splice(0, failures.length);
      child.on('error', () => {
        failures.unshift(...carried.slice(0, Math.max(0, MAX_FAILURE_CODES - failures.length)));
        count('capture_spawn_failed');
      });
      child.stdin?.on('error', () => undefined);
      child.unref();
      child.stdin?.end(envelope);
    } catch {
      count('capture_failed');
    }
    return undefined;
  };

  const inject = async (ctx: unknown): Promise<unknown> => {
    try {
      rememberSession(ctx);
      const kind = injectedThisSession ? 'prompt' : 'start';
      injectedThisSession = true;
      const signal = AbortSignal.timeout(kind === 'start' ? START_DEADLINE_MS : PROMPT_DEADLINE_MS);
      const { text, ok } = await readChild(
        ['inject', '--agent', 'pi', '--kind', kind],
        { stdio: ['pipe', 'pipe', 'ignore'], signal },
        JSON.stringify({ cwd: process.cwd(), session_id: sessionId, prompt }),
      );
      if (signal.aborted) {
        count('inject_timeout');
        return undefined;
      }
      if (!ok) {
        count('inject_failed');
        return undefined;
      }
      if (text.trim() === '') return undefined;
      // The one delivery path that reaches the model (verified end to end, R13 2026-09-03).
      return { message: { customType: 'oboete', content: text, display: true } };
    } catch {
      count('inject_failed');
      return undefined;
    }
  };

  const tool = (
    name: string,
    label: string,
    description: string,
    parameters: Record<string, unknown>,
    commandFor: (input: Record<string, unknown>) => string[],
  ): PiTool => ({
    name,
    label,
    description,
    parameters,
    execute: async (_toolCallId, params) => {
      let text = '';
      let ok = false;
      try {
        ({ text, ok } = await readChild(
          commandFor(params ?? {}),
          { stdio: ['ignore', 'pipe', 'ignore'], signal: AbortSignal.timeout(TOOL_DEADLINE_MS) },
          null,
        ));
      } catch {
        // A refused spawn or an unusable input leaves `ok` false, which the same sentence covers.
      }
      if (!ok) {
        count('tool_failed');
        text = 'oboete could not run that command. Run oboete doctor to see why.';
      }
      return { content: [{ type: 'text', text }] };
    },
  });

  const optional = (values: unknown, flag: string): string[] =>
    typeof values === 'string' || typeof values === 'number' ? [flag, String(values)] : [];

  for (const event of CAPTURE_EVENTS) {
    pi.on(event, (payload, ctx) => capture(event, payload, ctx));
  }
  pi.on('before_agent_start', (_payload, ctx) => inject(ctx));

  // FR-030 for Pi: the tool surface is the command line, run as a child process (research R9).
  const tools = [
    tool(
      'oboete_search',
      'oboete search',
      'Search the memories of this repository by keyword.',
      {
        type: 'object',
        properties: { query: { type: 'string' }, limit: { type: 'number' } },
        required: ['query'],
      },
      (input) => ['search', String(input.query ?? ''), '--json', ...optional(input.limit, '--limit')],
    ),
    tool(
      'oboete_timeline',
      'oboete timeline',
      'List the recorded sessions, turns and memories of this repository.',
      { type: 'object', properties: { session: { type: 'string' } } },
      (input) => ['timeline', '--json', ...optional(input.session, '--session')],
    ),
    tool(
      'oboete_get',
      'oboete get',
      'Read one memory of this repository by its identifier.',
      { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] },
      (input) => ['get', String(input.id ?? ''), '--json'],
    ),
  ];
  for (const entry of tools) {
    try {
      pi.registerTool?.(entry);
    } catch {
      count('tool_failed');
    }
  }
}
