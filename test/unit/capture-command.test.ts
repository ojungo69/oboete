import assert from 'node:assert/strict';
import { chmodSync, existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  STDIN_READ_BOUND,
  readStdinBounded,
  runCapture,
  runHook,
} from '../../src/capture-command.js';
import { detectSync } from '../../src/privacy/detect.js';
import { NOW, claudePostToolUse, withCapture } from '../helpers/capture.js';
import { withTempHome } from '../helpers/home.js';

test('runHook without a selector records unknown provenance and a diagnostics counter', async () => {
  await withCapture(async (context) => {
    const payload = claudePostToolUse(context.repo, 'notes');

    const code = await runHook(['--event', 'PostToolUse'], {
      deps: context.deps,
      readStdin: () => ({ text: JSON.stringify(payload), truncated: false }),
    });

    assert.equal(code, 0);
    const rows = context.all('SELECT agent, classification_state FROM raw_events');
    assert.equal(rows.length, 1);
    assert.equal(rows[0]?.agent, 'unknown');
    assert.equal(rows[0]?.classification_state, 'failed');
    const diagnostics = context.all('SELECT kind, agent, count FROM diagnostics');
    assert.equal(diagnostics.length, 1);
    assert.equal(diagnostics[0]?.kind, 'unknown_agent');
    assert.equal(diagnostics[0]?.agent, 'unknown');
  });
});

test('runHook writes one line per invocation and nothing to stdout', async () => {
  await withCapture(async (context) => {
    const payload = claudePostToolUse(context.repo, 'notes');
    const written: string[] = [];
    const original = process.stdout.write.bind(process.stdout);
    process.stdout.write = ((chunk: string) => {
      written.push(String(chunk));
      return true;
    }) as typeof process.stdout.write;
    try {
      await runHook(['--agent', 'claude-or-grok', '--event', 'PostToolUse'], {
        deps: context.deps,
        readStdin: () => ({ text: JSON.stringify(payload), truncated: false }),
      });
    } finally {
      process.stdout.write = original;
    }

    assert.deepEqual(written, []);
    const log = readFileSync(context.paths.hookLog, 'utf8').trimEnd().split('\n');
    assert.equal(log.length, 1);
    assert.match(log[0] as string, /agent=claude event=PostToolUse/);
  });
});

test('the Pi capture child acknowledges before it reads stdin and records prior failures', async () => {
  await withCapture(async (context) => {
    const started = join(context.paths.piAck, 'inv-1.started');
    let acknowledgedBeforeRead = false;
    const envelope = {
      event: 'input',
      session_id: 'pi-session',
      cwd: context.repo,
      payload: { text: 'what changed in the parser?', source: 'interactive' },
    };

    const code = await runCapture(
      [
        '--agent',
        'pi',
        '--event',
        'input',
        '--invocation',
        'inv-1',
        '--prior-failures',
        'spawn_failed,timeout',
      ],
      {
        deps: context.deps,
        readStdin: () => {
          acknowledgedBeforeRead = existsSync(started);
          return { text: JSON.stringify(envelope), truncated: false };
        },
      },
    );

    assert.equal(code, 0);
    assert.equal(acknowledgedBeforeRead, true, 'the acknowledgement must precede the stdin read');
    assert.equal(existsSync(started), false);
    assert.equal(existsSync(join(context.paths.piAck, 'inv-1.done')), true);
    assert.equal(context.all('SELECT id FROM raw_events').length, 1);
    const codes = context
      .all(`SELECT message_code FROM diagnostics WHERE kind = 'pi_child_failed' ORDER BY message_code`)
      .map((row) => row.message_code);
    assert.deepEqual(codes, ['spawn_failed', 'timeout']);
  });
});

test('a data directory that cannot be created still exits 0 and reports the count (FR-002)', async (t) => {
  if (process.getuid?.() === 0) {
    t.skip('the root user writes into a directory without write permission');
    return;
  }
  await withTempHome(async (home) => {
    // Nothing exists yet, so creating the data directory is the first thing that fails.
    chmodSync(home, 0o500);
    const written: string[] = [];
    const original = process.stderr.write.bind(process.stderr);
    process.stderr.write = ((chunk: string) => {
      written.push(String(chunk));
      return true;
    }) as typeof process.stderr.write;
    let code: number;
    try {
      code = await runHook(['--agent', 'claude-or-grok', '--event', 'PostToolUse'], {
        deps: {
          detect: (input) => detectSync(input),
          now: () => NOW,
          elapsedMs: () => 0,
          spawnWorker: () => undefined,
        },
        readStdin: () => ({
          text: JSON.stringify(claudePostToolUse(home, 'notes')),
          truncated: false,
        }),
      });
    } finally {
      process.stderr.write = original;
      chmodSync(home, 0o700);
    }
    assert.equal(code, 0, 'contracts/cli.md: the hook always exits 0');
    assert.match(written.join(''), /1 event/, 'the loss is reported to stderr');
  });
});

test('files written by capture stay owner-only', async () => {
  await withCapture(async (context) => {
    await context.capture('claude', 'PostToolUse', claudePostToolUse(context.repo, 'notes'));
    await runHook(['--agent', 'claude-or-grok', '--event', 'PostToolUse'], {
      deps: context.deps,
      readStdin: () => ({ text: '{}', truncated: false }),
    });

    for (const file of [context.paths.hookLog, context.paths.spool, context.paths.spoolFailed]) {
      assert.equal(statSync(file).mode & 0o077, 0, `${file} is readable by other users`);
    }
    assert.deepEqual(
      readdirSync(context.paths.spool).sort(),
      ['failed', 'pi-ack'],
      'only the two directories live in an empty spool',
    );
  });
});

test('stdin is read one byte past the bound, and only that byte makes the payload partial', () => {
  // A source that hands out `available` bytes in chunks, counting what the reader asked for.
  function source(available: number): { read: (target: Buffer, length: number) => number; asked: () => number } {
    let sent = 0;
    let asked = 0;
    return {
      read: (target, length) => {
        asked += length;
        const count = Math.min(length, available - sent);
        if (count <= 0) return 0;
        target.fill(0x61, 0, count);
        sent += count;
        return count;
      },
      asked: () => asked,
    };
  }

  const short = source(10);
  assert.deepEqual(readStdinBounded(short.read), { text: 'a'.repeat(10), truncated: false });

  // A payload of exactly the bound was not cut: nothing is missing, so the row is complete (A7).
  const exact = source(STDIN_READ_BOUND);
  const atBound = readStdinBounded(exact.read);
  assert.equal(atBound.truncated, false);
  assert.equal(atBound.text.length, STDIN_READ_BOUND);

  const over = source(STDIN_READ_BOUND + 1);
  const past = readStdinBounded(over.read);
  assert.equal(past.truncated, true);
  assert.equal(past.text.length, STDIN_READ_BOUND, 'the byte past the bound is never stored');

  // A14: capture time does not grow with the payload, so the rest is never drained.
  const huge = source(64 * 1_024 * 1_024);
  assert.equal(readStdinBounded(huge.read).truncated, true);
  assert.ok(
    huge.asked() <= STDIN_READ_BOUND + 1,
    `the hook asked for ${huge.asked()} bytes of a 64 MiB payload`,
  );
});
