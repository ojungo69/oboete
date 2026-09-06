// The Pi extension runs inside Pi's own process (FR-007): every assertion here is about what it
// hands to a child process and about what it must never do itself. Sources: contracts/agents.md
// (Pi row, "Capture and injection per agent"), research R12 "Pi diagnostics", amendment A8,
// docs/research/oboete-contracts-2026-09-02.md (extension API, verified injection return shape).
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { piExtension, type PiApi, type PiTool, type SpawnLike } from '../../src/pi-extension.js';

const NODE = '/opt/node/bin/node';
const BUNDLE = '/opt/oboete/dist/oboete.mjs';
const SESSION = 'pi-session-1';

type Call = {
  command: string;
  args: string[];
  options: { detached?: boolean; stdio?: unknown; signal?: AbortSignal };
  stdin: string[];
  unrefs: number;
  /** What a real child does when it prints `text` without exiting. */
  write(text: string): void;
  /** What a real child does when it prints `text` and exits with `code`. */
  deliver(text?: string, code?: number): void;
  /** What a real child does when the spawn itself fails or the signal aborts it. */
  fail(): void;
};

function spawnRecorder(failAttempt?: number) {
  const calls: Call[] = [];
  let attempts = 0;
  const spawn: SpawnLike = (command, args, options) => {
    attempts += 1;
    if (attempts === failAttempt) throw new Error('spawn refused');
    const child = new EventEmitter();
    const stdout = new EventEmitter();
    const stdin = new EventEmitter();
    const call: Call = {
      command,
      args,
      options: options as Call['options'],
      stdin: [],
      unrefs: 0,
      write(text) {
        stdout.emit('data', text);
      },
      deliver(text, code = 0) {
        if (text !== undefined && text !== '') stdout.emit('data', text);
        child.emit('close', code);
      },
      fail() {
        child.emit('error', new Error('child failed'));
        child.emit('close', 1);
      },
    };
    options.signal?.addEventListener('abort', () => {
      call.fail();
    });
    calls.push(call);
    return {
      stdin: {
        end: (chunk?: string) => {
          if (chunk !== undefined) call.stdin.push(chunk);
        },
        on: (event: string, listener: () => void) => stdin.on(event, listener),
      },
      stdout: {
        setEncoding: () => undefined,
        on: (event: string, listener: (chunk: string) => void) => stdout.on(event, listener),
      },
      on: (event: string, listener: (value?: unknown) => void) => child.on(event, listener),
      unref: () => {
        call.unrefs += 1;
      },
    };
  };
  return { calls, spawn };
}

type Handler = (event: unknown, ctx: unknown) => unknown;

function stubPi() {
  const handlers = new Map<string, Handler>();
  const tools = new Map<string, PiTool>();
  const pi: PiApi = {
    on(event, handler) {
      handlers.set(event, handler as Handler);
    },
    registerTool(tool) {
      tools.set(tool.name, tool);
    },
  };
  const ctx = { sessionManager: { getSessionId: () => SESSION } };
  const fire = (event: string, payload: unknown = { type: event }): unknown => {
    const handler = handlers.get(event);
    assert.ok(handler, `the extension must subscribe to ${event}`);
    return handler(payload, ctx);
  };
  return { pi, handlers, tools, fire };
}

function envelope(call: Call): Record<string, unknown> {
  assert.equal(call.stdin.length, 1, 'the child receives exactly one write on stdin');
  return JSON.parse(call.stdin[0]) as Record<string, unknown>;
}

function captureArgs(event: string, invocation: string, priorFailures?: string): string[] {
  return [
    BUNDLE,
    'capture',
    '--agent',
    'pi',
    '--event',
    event,
    '--invocation',
    invocation,
    ...(priorFailures === undefined ? [] : ['--prior-failures', priorFailures]),
  ];
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

test('the extension subscribes to the capture events, the injection hook and the three tools', () => {
  const { pi, handlers, tools } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn: spawnRecorder().spawn });

  assert.deepEqual(
    [...handlers.keys()].sort(),
    [
      'agent_settled',
      'before_agent_start',
      'input',
      'session_compact',
      'session_shutdown',
      'session_start',
      'tool_result',
    ],
    'contracts/agents.md Pi row: six capture events plus before_agent_start',
  );
  assert.deepEqual([...tools.keys()].sort(), ['oboete_get', 'oboete_search', 'oboete_timeline']);
});

test('a capture event spawns a detached child carrying the normalized envelope on stdin', () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  assert.equal(fire('session_start', { type: 'session_start', reason: 'startup' }), undefined);

  assert.equal(calls.length, 1);
  const call = calls[0];
  assert.equal(call.command, NODE);
  const invocation = call.args[7];
  assert.match(invocation, /^[0-9a-f-]{36}$/, 'each spawn carries its own invocation id (A8)');
  assert.deepEqual(call.args, captureArgs('session_start', invocation));
  assert.equal(call.options.detached, true);
  assert.deepEqual(call.options.stdio, ['pipe', 'ignore', 'ignore'], 'Pi owns stdout in json mode');
  assert.equal(call.options.signal, undefined, 'the capture child is never awaited or bounded');
  assert.equal(call.unrefs, 1);
  assert.deepEqual(envelope(call), {
    event: 'session_start',
    session_id: SESSION,
    cwd: process.cwd(),
    payload: { type: 'session_start', reason: 'startup' },
  });
});

test('the event and the agent are the handler’s own, never the payload’s', () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  // contracts/agents.md "Normalized events" and "Agent identity (fixed selectors)": the selector and
  // the event name come from the fixed arguments this handler was registered with, cross-checked
  // against the payload by the child, never read out of the payload here.
  fire('tool_result', { type: 'input', agent: 'claude', text: 'a payload that disagrees' });

  const call = calls[0];
  assert.deepEqual(call.args, captureArgs('tool_result', call.args[7]));
  assert.equal(envelope(call).event, 'tool_result');
});

test('the prompt id is generated on input and repeats for the rest of the turn', () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  fire('input', { type: 'input', text: 'fix the parser', source: 'interactive' });
  fire('tool_result', { type: 'tool_result', toolName: 'read', toolCallId: 'c1' });
  fire('agent_settled', { type: 'agent_settled', text: 'DONE' });
  fire('input', { type: 'input', text: 'now run it', source: 'interactive' });

  const ids = calls.map((call) => envelope(call).prompt_id);
  assert.match(String(ids[0]), /^[0-9a-f-]{36}$/);
  assert.equal(ids[1], ids[0], 'the tool result belongs to the turn the prompt opened (R7)');
  assert.equal(ids[2], ids[0]);
  assert.notEqual(ids[3], ids[0], 'the next prompt opens the next turn');
});

test('a failure is counted in memory and handed to the next capture child exactly once', () => {
  const { calls, spawn } = spawnRecorder(1);
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  assert.equal(fire('session_start'), undefined, 'a thrown spawn never reaches Pi (FR-007)');
  fire('input', { type: 'input', text: 'hello' });
  fire('agent_settled', { type: 'agent_settled', text: 'DONE' });

  assert.equal(calls[0].args.at(-2), '--prior-failures');
  assert.equal(calls[0].args.at(-1), 'capture_failed');
  assert.equal(calls[1].args.includes('--prior-failures'), false, 'the counters are drained');
});

test('the codes a capture child never carried go back to the next one', () => {
  const { calls, spawn } = spawnRecorder(1);
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  fire('session_start'); // The spawn throws, so the code is counted in memory.
  fire('input', { type: 'input', text: 'hello' }); // This child is handed the code...
  calls[0].fail(); // ...and then turns out never to have started.
  fire('tool_result', { type: 'tool_result', toolName: 'read', toolCallId: 'c1' });

  // The child process is the only durable record of a failure (Pi keeps none: R12, amendment A8),
  // so a spawn that failed must not take the codes it was given with it.
  assert.equal(calls[1].args.at(-2), '--prior-failures');
  assert.equal(calls[1].args.at(-1), 'capture_failed,capture_spawn_failed');
});

test('a capture child that fails to start is counted for the next child', () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  fire('session_start');
  calls[0].fail();
  fire('input', { type: 'input', text: 'hello' });

  assert.equal(calls[1].args.at(-1), 'capture_spawn_failed');
});

test('the first before_agent_start injects at session start, later ones per prompt', async () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  fire('session_start');
  fire('input', { type: 'input', text: 'fix the parser' });
  const first = fire('before_agent_start', { type: 'before_agent_start' }) as Promise<unknown>;

  const start = calls[2];
  assert.deepEqual(start.args, [BUNDLE, 'inject', '--agent', 'pi', '--kind', 'start']);
  assert.deepEqual(start.options.stdio, ['pipe', 'pipe', 'ignore']);
  assert.ok(start.options.signal instanceof AbortSignal, 'the injection child is bounded');
  assert.equal(start.options.detached, undefined);
  assert.deepEqual(JSON.parse(start.stdin[0]), {
    cwd: process.cwd(),
    session_id: SESSION,
    prompt: 'fix the parser',
  });

  start.deliver('oboete memory context\n- one\nend of oboete memory context');
  assert.deepEqual(await first, {
    message: {
      customType: 'oboete',
      content: 'oboete memory context\n- one\nend of oboete memory context',
      display: true,
    },
  });

  fire('input', { type: 'input', text: 'now run it' });
  const second = fire('before_agent_start', { type: 'before_agent_start' }) as Promise<unknown>;
  const prompt = calls.at(-1) as Call;
  assert.deepEqual(prompt.args, [BUNDLE, 'inject', '--agent', 'pi', '--kind', 'prompt']);
  assert.equal(JSON.parse(prompt.stdin[0]).prompt, 'now run it');
  prompt.deliver('');
  assert.equal(await second, undefined, 'an empty pack adds no message to the turn');
});

test('an injection child that dies half way through the pack contributes nothing', async () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  fire('session_start');
  const injection = fire('before_agent_start') as Promise<unknown>;
  calls[1].write('oboete memory context\n- one');
  calls[1].fail();

  assert.equal(await injection, undefined, 'half a pack has lost its framing');
  fire('input', { type: 'input', text: 'hello' });
  assert.equal(calls[2].args.at(-1), 'inject_failed', 'and the failure is recorded (A8)');

  // A child that prints and then exits non-zero is the same loss without an `error` event.
  const second = fire('before_agent_start') as Promise<unknown>;
  calls[3].write('oboete memory context\n- one');
  calls[3].deliver('', 2);

  assert.equal(await second, undefined);
  fire('input', { type: 'input', text: 'again' });
  assert.equal(calls[4].args.at(-1), 'inject_failed');
});

test('the injection deadline is 1.3 s at session start and 300 ms per prompt', async () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  fire('session_start');
  const start = fire('before_agent_start') as Promise<unknown>;
  fire('input', { type: 'input', text: 'hello' });
  const prompt = fire('before_agent_start') as Promise<unknown>;
  calls[3].write('half a pack, cut off by the deadline');

  // 300 ms < 600 ms < 1,300 ms (contracts/agents.md Pi row).
  await sleep(600);
  assert.equal(calls[1].options.signal?.aborted, false, 'session start waits for the summary');
  assert.equal(calls[3].options.signal?.aborted, true, 'a later prompt does not');
  assert.equal(await prompt, undefined, 'a timed-out child never contributes a partial pack');

  calls[1].deliver('pack');
  assert.notEqual(await start, undefined);

  fire('input', { type: 'input', text: 'again' });
  assert.equal(calls.at(-1)?.args.at(-1), 'inject_timeout');
});

test('the tools run the command line as child processes', async () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, tools } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  const search = tools.get('oboete_search')?.execute('call-search', { query: 'parser', limit: 3 });
  calls[0].deliver('{"results":[]}');
  assert.deepEqual(await search, { content: [{ type: 'text', text: '{"results":[]}' }] });
  assert.deepEqual(calls[0].args, [BUNDLE, 'search', 'parser', '--json', '--limit', '3']);
  assert.deepEqual(calls[0].options.stdio, ['ignore', 'pipe', 'ignore']);

  const get = tools.get('oboete_get')?.execute('call-get', { id: 'm_abc' });
  calls[1].deliver('{"id":"m_abc"}');
  assert.deepEqual(await get, { content: [{ type: 'text', text: '{"id":"m_abc"}' }] });
  assert.deepEqual(calls[1].args, [BUNDLE, 'get', 'm_abc', '--json']);

  const timeline = tools.get('oboete_timeline')?.execute('call-timeline', {});
  calls[2].deliver('[]');
  assert.deepEqual(await timeline, { content: [{ type: 'text', text: '[]' }] });
  assert.deepEqual(calls[2].args, [BUNDLE, 'timeline', '--json']);
});

test('a tool child that exits non-zero is a counted failure, not an empty answer', async () => {
  const { calls, spawn } = spawnRecorder();
  const { pi, tools, fire } = stubPi();
  piExtension(pi, { node: NODE, bundle: BUNDLE, spawn });

  // `oboete search --json` prints its complaint on stderr and exits 2 while this task is unbuilt;
  // an empty stdout must never reach the model as a successful answer.
  const search = tools.get('oboete_search')?.execute('call-search-fail', { query: 'parser' });
  calls[0].deliver('', 2);

  assert.deepEqual(await search, {
    content: [
      { type: 'text', text: 'oboete could not run that command. Run oboete doctor to see why.' },
    ],
  });

  fire('input', { type: 'input', text: 'hello' });
  assert.equal(calls[1].args.at(-1), 'tool_failed', 'the only durable record of the failure (A8)');

  // A spawn the operating system refuses ends in the same sentence rather than in a rejected tool
  // call, because a rejection would leave Pi's own turn broken (FR-007).
  const refused = stubPi();
  piExtension(refused.pi, { node: NODE, bundle: BUNDLE, spawn: spawnRecorder(1).spawn });
  assert.deepEqual(await refused.tools.get('oboete_get')?.execute('call-get-refused', { id: 'm_abc' }), {
    content: [
      { type: 'text', text: 'oboete could not run that command. Run oboete doctor to see why.' },
    ],
  });
});

test('the built extension can reach nothing but a child process', () => {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  while (!existsSync(join(directory, 'package.json'))) {
    const parent = dirname(directory);
    assert.notEqual(parent, directory, 'the repository root must contain package.json');
    directory = parent;
  }
  const built = join(directory, 'dist', 'pi-extension.mjs');
  assert.ok(existsSync(built), 'scripts/build.mjs builds the extension the Pi loader imports');
  const text = readFileSync(built, 'utf8');

  assert.deepEqual(
    [...text.matchAll(/(?:^|\s)(?:from|import)\s*["']([^"']+)["']/g)].map((match) => match[1]),
    ['node:child_process'],
    'FR-007: the extension does no file or network work in Pi’s process',
  );
  assert.match(text, /export\s*\{[^}]*\bpiExtension\b/);
  for (const forbidden of ['node:fs', 'node:http', 'node:net', 'fetch(', 'import(', 'require(']) {
    assert.equal(text.includes(forbidden), false, `the extension must not contain ${forbidden}`);
  }
});

