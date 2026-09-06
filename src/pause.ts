import { existsSync, unlinkSync, writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';

import { ensureDirectories, oboetePaths, resolveHome } from './paths.js';

const PAUSED_TEXT =
  'Capture and injection are paused. Run `oboete resume` to continue; existing memories are untouched.';
const RESUMED_TEXT = 'Capture and injection are resumed.';
const NOT_PAUSED_TEXT = 'oboete was not paused; nothing changed.';

type PauseIo = {
  writeOut(text: string): void;
  writeError(text: string): void;
};

function ioWith(overrides: Partial<PauseIo> = {}): PauseIo {
  return {
    writeOut: (text) => {
      process.stdout.write(text);
    },
    writeError: (text) => {
      process.stderr.write(text);
    },
    ...overrides,
  };
}

function parse(argv: string[], io: PauseIo): { json: boolean } | null {
  try {
    const parsed = parseArgs({
      args: argv,
      strict: true,
      options: { json: { type: 'boolean' } },
    });
    return { json: parsed.values.json === true };
  } catch (error) {
    io.writeError(`${error instanceof Error ? error.message : String(error)}\n`);
    return null;
  }
}

function report(io: PauseIo, json: boolean, paused: boolean, text: string): void {
  io.writeOut(json ? `${JSON.stringify({ paused })}\n` : `${text}\n`);
}

/** `oboete pause [--json]`: create `~/.oboete/paused` without opening the database (FR-034, R12). */
export async function runPause(argv: string[], overrides: Partial<PauseIo> = {}): Promise<number> {
  const io = ioWith(overrides);
  const parsed = parse(argv, io);
  if (parsed === null) return 2;
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  writeFileSync(paths.paused, '', { mode: 0o600 });
  report(io, parsed.json, true, PAUSED_TEXT);
  return 0;
}

/** `oboete resume [--json]`: remove the pause marker if it is present (FR-034). */
export async function runResume(argv: string[], overrides: Partial<PauseIo> = {}): Promise<number> {
  const io = ioWith(overrides);
  const parsed = parse(argv, io);
  if (parsed === null) return 2;
  const paths = oboetePaths(resolveHome());
  const wasPaused = existsSync(paths.paused);
  if (wasPaused) unlinkSync(paths.paused);
  report(io, parsed.json, false, wasPaused ? RESUMED_TEXT : NOT_PAUSED_TEXT);
  return 0;
}
