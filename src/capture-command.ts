import { spawn } from 'node:child_process';
import { existsSync, readSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { parseArgs } from 'node:util';

import { resolveAgent } from './agents/index.js';
import {
  captureEvent,
  type CaptureDeps,
  type CaptureInput,
  type CaptureOutcome,
  type StdinRead,
} from './capture.js';
import { appendLogQuietly, errorCode } from './log.js';
import { ensureDirectories, oboetePaths, resolveHome } from './paths.js';
import { detectInWorker } from './privacy/detect.js';
import { testFault } from './testing/faults.js';

/**
 * How much of stdin the hook reads before it stops (A7 with the A14 default, 2026-09-04). The
 * secret-dense worst case of the full detector measures 406-665 ms per 1 MB on Node 22 and 24, so
 * the 1 MB bound of the spec cannot hold the 240 ms detector cutoff; 256 KiB keeps that worst case
 * near 100-170 ms and stays above A14's 200 KB escalation floor. The read part is treated exactly
 * as A7 prescribes: a redacted `partial` row marked truncated.
 */
export const STDIN_READ_BOUND = 262_144;

const STDIN_WAIT_LIMIT = 100;

export type CaptureRuntime = { deps: CaptureDeps; readStdin: () => StdinRead };

function sleep(milliseconds: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

/** One chunk of standard input; the seam a test replaces to drive the read bound exactly. */
export type StdinReader = (target: Buffer, length: number) => number;

const fromStandardInput: StdinReader = (target, length) => readSync(0, target, 0, length, null);

/**
 * Reads at most `STDIN_READ_BOUND` bytes and stops; the rest is never drained, so capture time does
 * not grow with the payload (A7, A14). A pipe with nothing ready yet answers EAGAIN, which is
 * waited out in short steps rather than spun on.
 */
export function readStdinBounded(read: StdinReader = fromStandardInput): StdinRead {
  const chunks: Buffer[] = [];
  const buffer = Buffer.allocUnsafe(64 * 1_024);
  let total = 0;
  let waits = 0;

  // One byte past the bound is read but never stored: a payload of exactly the bound was not cut,
  // so only a byte beyond it makes the row partial (A7).
  while (total <= STDIN_READ_BOUND) {
    let taken: number;
    try {
      taken = read(buffer, Math.min(buffer.length, STDIN_READ_BOUND + 1 - total));
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === 'EAGAIN' && waits < STDIN_WAIT_LIMIT) {
        waits += 1;
        sleep(1);
        continue;
      }
      break;
    }
    if (taken === 0) break;
    chunks.push(Buffer.from(buffer.subarray(0, taken)));
    total += taken;
  }
  return {
    text: Buffer.concat(chunks).subarray(0, STDIN_READ_BOUND).toString('utf8'),
    truncated: total > STDIN_READ_BOUND,
  };
}

function defaultRuntime(): CaptureRuntime {
  const bundlePath = process.argv[1] ?? '';
  return {
    deps: {
      detect: (input, cutoffMs) => detectInWorker(input, { cutoffMs, workerScript: bundlePath }),
      now: () => Date.now(),
      // performance.now() counts from process start, which is where the budget is measured from.
      elapsedMs: () => performance.now(),
      spawnWorker: () => {
        const child = spawn(process.execPath, [bundlePath, 'observe'], {
          detached: true,
          stdio: 'ignore',
        });
        // A spawn failure arrives as an asynchronous 'error' event after the hook has returned, so
        // without a listener it would throw past the exit-0 contract (FR-002); the spawn is
        // best-effort, and the next hook retries it.
        child.on('error', () => {});
        child.unref();
      },
    },
    readStdin: () => readStdinBounded(),
  };
}

function option(values: Record<string, unknown>, name: string): string | undefined {
  const value = values[name];
  return typeof value === 'string' && value !== '' ? value : undefined;
}

async function runCaptureCommand(
  input: Omit<CaptureInput, 'readStdin'>,
  runtime: CaptureRuntime,
): Promise<number> {
  let outcome: CaptureOutcome;
  try {
    outcome = await captureEvent(runtime.deps, { ...input, readStdin: runtime.readStdin });
  } catch (error) {
    // FR-002: the agent is never blocked, so every failure ends as one log line and exit 0.
    appendLogQuietly(input.paths.hookLog, 'error', 'capture failed', {
      agent: input.agent,
      event: input.eventName,
      reason: errorCode(error),
    });
    return 0;
  }

  if (outcome.outcome !== 'paused') {
    appendLogQuietly(input.paths.hookLog, 'info', 'capture', {
      agent: input.agent,
      event: input.eventName,
      outcome: outcome.outcome,
      rows: outcome.rows,
    });
  }
  if (outcome.stdout !== undefined && outcome.stdout !== '') process.stdout.write(outcome.stdout);
  return 0;
}

/** `oboete hook --agent codex|claude-or-grok --event <name>` (contracts/cli.md); always exits 0. */
export async function runHook(argv: string[], runtime: Partial<CaptureRuntime> = {}): Promise<number> {
  const { values } = parseArgs({
    args: argv,
    strict: false,
    allowPositionals: true,
    options: { agent: { type: 'string' }, event: { type: 'string' } },
  });
  const paths = oboetePaths(resolveHome());
  return runCaptureCommand(
    {
      agent: resolveAgent(option(values, 'agent'), process.env),
      eventName: option(values, 'event') ?? '',
      paths,
    },
    { ...defaultRuntime(), ...runtime },
  );
}

/**
 * `oboete capture --agent pi --event <name> --invocation <id> [--prior-failures <codes>]`: Pi's
 * detached capture child. The acknowledgement is written before stdin is read and renamed on the
 * way out, which is how doctor tells a hung child from a failed spawn (data-model "Pi", A8).
 */
export async function runCapture(
  argv: string[],
  runtime: Partial<CaptureRuntime> = {},
): Promise<number> {
  // FR-002: an unanticipated bug in the Pi capture child must still leave the agent unblocked.
  if (testFault('pi-throw')) throw new Error('OBOETE_TEST_FAULT: pi-throw');
  const { values } = parseArgs({
    args: argv,
    strict: false,
    allowPositionals: true,
    options: {
      agent: { type: 'string' },
      event: { type: 'string' },
      invocation: { type: 'string' },
      'prior-failures': { type: 'string' },
    },
  });
  const paths = oboetePaths(resolveHome());
  const invocation = option(values, 'invocation');
  const started = invocation === undefined ? null : join(paths.piAck, `${invocation}.started`);

  if (started !== null) {
    try {
      ensureDirectories(paths);
      writeFileSync(started, '', { mode: 0o600 });
    } catch {
      // FR-007: the acknowledgement is diagnostics; capture continues without it.
    }
  }

  const code = await runCaptureCommand(
    {
      agent: resolveAgent(option(values, 'agent'), process.env),
      eventName: option(values, 'event') ?? '',
      paths,
      priorFailures: (option(values, 'prior-failures') ?? '')
        .split(',')
        .map((entry) => entry.trim())
        .filter((entry) => entry !== ''),
    },
    { ...defaultRuntime(), ...runtime },
  );

  if (started !== null) {
    try {
      if (existsSync(started)) renameSync(started, started.replace(/\.started$/, '.done'));
    } catch {
      // The worker records a `.started` file older than 30 s as `pi_child_hang` (data-model "Pi").
    }
  }
  return code;
}
