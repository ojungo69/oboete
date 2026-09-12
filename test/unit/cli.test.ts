import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';

const root = fileURLToPath(new URL('../../..', import.meta.url));
const bin = join(root, 'dist/oboete.mjs');
// dist/oboete.mjs is the launcher that enables the compile cache (scripts/build.mjs, #210);
// the dispatch and its uncaught-error handling live in the engine file it imports, and only the
// engine can be re-evaluated per command with a query string.
const engine = join(root, 'dist/engine.mjs');
const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as {
  version: string;
};

function run(args: string[]) {
  const env = { ...process.env };
  delete env.FORCE_COLOR;
  delete env.NO_COLOR;
  return spawnSync(process.execPath, [bin, ...args], { encoding: 'utf8', env });
}

test('--version prints the package version', () => {
  const result = run(['--version']);
  assert.equal(result.status, 0);
  assert.equal(result.stdout.trim(), pkg.version);
});

test('unknown command exits 2 and prints usage to stderr', () => {
  for (const name of ['not-a-command', 'constructor', '__proto__', 'toString']) {
    const result = run([name]);
    assert.equal(result.status, 2, name);
    assert.match(result.stderr, /Usage: oboete/, name);
  }
});

test('oboete doctor prints a report', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    mkdirSync(userHome, { recursive: true });
    const result = spawnSync(process.execPath, [bin, 'doctor', '--json', '--no-probe-agents'], {
      encoding: 'utf8',
      env: {
        ...process.env,
        HOME: userHome,
        OBOETE_HOME: home,
        PATH: join(home, 'empty-bin'),
      },
    });
    assert.notEqual(result.status, 2, result.stderr);
    const report = JSON.parse(result.stdout) as { items: unknown; notes: unknown; view: unknown };
    assert.ok(Array.isArray(report.items));
    assert.match(JSON.stringify(report.notes), /lexical/);
    assert.match(String(report.view), /oboete view --open/);
  });
});

/**
 * Node 22.16's test runner reports through process.stdout.write itself: v8 frames (Buffers) under
 * `node --test`, TAP lines when the file runs directly. Swallowing those breaks the run, so the
 * stub lets them through and records only the command's own string writes.
 */
function recordCommandWrites(t: TestContext): string[] {
  const written: string[] = [];
  const original = process.stdout.write.bind(process.stdout);
  t.mock.method(process.stdout, 'write', ((chunk: unknown, ...rest: unknown[]) => {
    if (typeof chunk === 'string' && !/^(TAP version|# |ok |not ok |\s|1\.\.)/.test(chunk)) {
      written.push(chunk);
      return true;
    }
    return (original as (...args: unknown[]) => boolean)(chunk, ...rest);
  }) as typeof process.stdout.write);
  return written;
}

for (const command of ['hook', 'capture', 'inject', 'observe']) {
  test(`${command} contains an uncaught dispatch error with its contracted exit code`, async (t) => {
    await withTempHome(async (home) => {
      const previous = { argv: process.argv, env: process.env, exitCode: process.exitCode };
      const warnings = process.listeners('warning');
      const stdout = recordCommandWrites(t);
      const stderr = t.mock.method(process.stderr, 'write', () => true);
      process.argv = [process.execPath, bin, command, '--agent', 'pi'];
      if (command !== 'capture') {
        process.argv.slice = () => {
          throw new Error('dispatch failed\npayload must not be logged');
        };
      }
      process.env = { ...process.env, NODE_ENV: 'test', OBOETE_TEST_FAULT: 'pi-throw' };
      try {
        // Import the engine in-process; the query string gives each command a fresh evaluation,
        // which importing the launcher would not (its own import of the engine carries no suffix).
        await import(`${pathToFileURL(engine).href}?uncaught=${command}`);
        assert.equal(process.exitCode, command === 'observe' ? 3 : 0);
        assert.deepEqual(stdout, []);
        const log = oboetePaths(home).hookLog;
        if (command === 'observe') {
          assert.equal(stderr.mock.callCount(), 1);
          assert.equal(stderr.mock.calls[0]?.arguments[0], 'dispatch failed\n');
          assert.equal(existsSync(log), false);
        } else {
          assert.equal(stderr.mock.callCount(), 0);
          const lines = readFileSync(log, 'utf8').trimEnd().split('\n');
          assert.equal(lines.length, 1);
          assert.ok(
            lines[0]?.includes(`command=${command} reason=Error`),
            lines[0],
          );
          // The message is never logged: an error raised while reading stdin can quote the payload.
          assert.equal(lines[0]?.includes('dispatch failed'), false);
          assert.equal(lines[0]?.includes('pi-throw'), false);
        }
      } finally {
        process.argv = previous.argv;
        process.env = previous.env;
        process.exitCode = previous.exitCode;
        process.removeAllListeners('warning');
        for (const listener of warnings) process.on('warning', listener);
        t.mock.restoreAll();
      }
    });
  });
}
