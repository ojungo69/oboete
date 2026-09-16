import assert from 'node:assert/strict';
import { spawn as nodeSpawn, type spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';

import { AGENT_CLIS } from '../../src/config.js';

const FAKED: ReadonlySet<string> = new Set(AGENT_CLIS);

type FakeChild = EventEmitter & {
  stdin: PassThrough;
  stdout: PassThrough;
  stderr: PassThrough;
  kill: () => boolean;
};

/**
 * A stand-in for the agent command line tool: each spawn answers with the next text wrapped the
 * way the real CLI wraps it, and one more spawn than there are texts fails the test rather than
 * hanging. Shared by the observer unit tests and the chain's worker tests so both see the same
 * child process shape.
 *
 * Only the agent command line tool is faked. A worker pass also spawns `git` for citations, and
 * answering that with a scripted CLI reply (or failing it as unexpected) stalls the pass instead of
 * reporting anything, so every other command is handed to the real `spawn`.
 */
export function cliSpawn(texts: string[]): { spawn: typeof spawn; calls: () => number } {
  let count = 0;
  return {
    spawn: ((command: string, args: readonly string[], options: { signal?: AbortSignal }) => {
      if (!FAKED.has(command)) return nodeSpawn(command, [...args], options);
      const child = Object.assign(new EventEmitter(), {
        stdin: new PassThrough(),
        stdout: new PassThrough(),
        stderr: new PassThrough(),
        kill: () => true,
      }) as FakeChild;
      child.stdin.resume();
      child.stdin.on('finish', () => {
        const text = texts[count];
        count += 1;
        if (text === undefined) assert.fail('unexpected child process');
        child.stdout.end(JSON.stringify({ result: text }));
        child.stderr.end();
        queueMicrotask(() => child.emit('close', 0, null));
      });
      options.signal?.addEventListener(
        'abort',
        () => {
          const error = new Error('aborted');
          error.name = 'AbortError';
          child.emit('error', error);
        },
        { once: true },
      );
      return child;
    }) as unknown as typeof spawn,
    calls: () => count,
  };
}
