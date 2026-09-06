// Pi half of the User Story 2 failure matrix (spec User Story 2 scenario 4, FR-007, A8, R12):
// the detached capture child stays contained, a hung child is recorded for doctor, a Pi that cannot
// spawn is named by setup's probe, and the extension's own failure counters land in diagnostics.
// Seam: only pi-throw (src/capture.ts runCapture); everything else is the real child or CLI.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import type { TestContext } from 'node:test';

import { oboetePaths } from '../src/paths.js';
import { claimLease } from '../src/worker/lease.js';
import { cleanupPiAck } from '../src/worker/purge.js';
import { BUNDLE, fixture, ROOT, rows, scenario, spawnEngine, type Place } from './helpers/fault.js';

type Json = Record<string, unknown>;

const NOW = Date.now();

/** The envelope the extension pipes to `oboete capture --agent pi` (src/agents/pi.ts). */
function envelope(place: Place, turn = 1): Json {
  const file = JSON.parse(readFileSync(join(ROOT, 'test', 'contracts', 'pi', 'read.json'), 'utf8')) as {
    events: { tool_result: Json };
  };
  // A distinct turn per call: the same turn end twice is one row (R7), not a second capture.
  return {
    event: 'tool_result',
    session_id: 'pi-fault-session',
    cwd: place.repo,
    prompt_id: `prompt-${turn}`,
    payload: { ...file.events.tool_result, toolCallId: `call-${turn}` },
  };
}

function captureArgs(invocation: string, priorFailures?: string): string[] {
  return [
    'capture',
    '--agent',
    'pi',
    '--event',
    'tool_result',
    '--invocation',
    invocation,
    ...(priorFailures === undefined ? [] : ['--prior-failures', priorFailures]),
  ];
}

function piAck(place: Place): string {
  return oboetePaths(place.home).piAck;
}

function diagnostics(place: Place, kind: string): Json[] {
  return rows(
    place,
    `SELECT message_code, count, details_json FROM diagnostics WHERE kind = '${kind}' AND cleared_at IS NULL ORDER BY message_code`,
  );
}

scenario('pi-throw', (t: TestContext) => {
  const place = fixture();
  try {
    const result = spawnEngine(captureArgs('inv-throw'), {
      home: place.home,
      cwd: place.repo,
      input: JSON.stringify(envelope(place)),
      fault: 'pi-throw',
      timeoutMs: 5_000,
    });
    t.diagnostic(`pi-throw: status=${result.status} stderr=${result.stderr.trim()}`);
    // contracts/cli.md: capture always exits 0 and prints nothing; the throw sits above the
    // acknowledgement, so no `.started` may be left behind either. RED until T063 fixes the
    // uncaught-throw exit in src/cli.ts.
    assert.equal(result.status, 0, `capture exited ${result.status}: ${result.stderr}`);
    assert.equal(result.stdout, '');
    assert.equal(existsSync(join(piAck(place), 'inv-throw.started')), false);
    assert.equal(rows(place, 'SELECT id FROM raw_events').length, 0);
  } finally {
    place.cleanup();
  }
});

scenario('pi-child-hang', async () => {
  const place = fixture();
  const dir = piAck(place);
  const started = join(dir, 'inv-hang.started');
  const child = spawn(process.execPath, [BUNDLE, ...captureArgs('inv-hang')], {
    cwd: place.repo,
    env: { ...process.env, NODE_ENV: 'test', OBOETE_HOME: place.home, OBOETE_TEST_FAULT: '' },
    stdio: ['pipe', 'ignore', 'pipe'],
  });
  try {
    // The child acknowledges before it reads stdin (FR-007); stdin is never written or closed, so
    // it hangs exactly where a stuck extension host would leave it.
    const deadline = Date.now() + 5_000;
    while (!existsSync(started) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 20));
    assert.ok(existsSync(started), 'the child never wrote its .started acknowledgement');
    child.kill('SIGKILL');
    await new Promise((resolve) => child.once('exit', resolve));
    assert.equal(existsSync(started), true, 'a killed child leaves its .started behind');

    // The worker's sweep is a plain function of `now`, so the 30 s threshold needs no waiting.
    const mtime = Math.ceil(statSync(started).mtimeMs);
    const db = new DatabaseSync(place.db, { timeout: 5_000 });
    try {
      const token = claimLease(db, { pid: process.pid, now: NOW });
      assert.ok(token !== null);
      const first = cleanupPiAck(db, token, dir, mtime + 31_000);
      assert.equal(first.hangs, 1);
      assert.equal(first.removed, 0, 'a hung ack is kept for a day so doctor can show it');
      let found = diagnostics(place, 'pi_child_hang');
      assert.equal(found.length, 1);
      assert.equal(Number(found[0]?.count), 1);
      assert.deepEqual(JSON.parse(String(found[0]?.details_json)), { invocations: ['inv-hang'] });
      assert.equal(existsSync(started), true);

      // `count` is sightings, one per worker sweep; the invocation list stays de-duplicated.
      const second = cleanupPiAck(db, token, dir, mtime + 62_000);
      assert.equal(second.hangs, 1);
      found = diagnostics(place, 'pi_child_hang');
      assert.equal(found.length, 1);
      assert.equal(Number(found[0]?.count), 2);
      assert.deepEqual(JSON.parse(String(found[0]?.details_json)), { invocations: ['inv-hang'] });
    } finally {
      db.close();
    }
  } finally {
    if (child.exitCode === null) child.kill('SIGKILL');
    place.cleanup();
  }
});

scenario('pi-spawn-failure', (t: TestContext) => {
  const place = fixture();
  try {
    // A `pi` that detection finds on PATH (executable, first on PATH) but the kernel refuses to run:
    // the interpreter does not exist, so every exec fails and setup must name it instead of waiting
    // out the 90 s probe deadline. A non-executable file would not do: detection skips it (X_OK).
    const bin = join(place.home, 'bin');
    mkdirSync(bin, { recursive: true });
    writeFileSync(join(bin, 'pi'), '#!/nonexistent/interpreter\nexit 0\n', { mode: 0o755 });
    const result = spawnEngine(
      ['setup', '--agents', 'pi', '--provider', 'none', '--accept-egress', '--json'],
      {
        home: place.home,
        cwd: place.repo,
        extraEnv: { HOME: place.home, PATH: `${bin}:${process.env.PATH ?? ''}` },
        timeoutMs: 20_000,
      },
    );
    t.diagnostic(`setup: status=${result.status} elapsed=${result.elapsedMs.toFixed(0)} ms`);
    assert.equal(result.status, 1, `setup exited ${result.status}: ${result.stderr}\n${result.stdout}`);
    const report = JSON.parse(result.stdout) as { agents: { agent: string; wired: string; probe: string }[] };
    const pi = report.agents.find((row) => row.agent === 'pi');
    assert.ok(pi !== undefined, result.stdout);
    assert.equal(pi.wired, 'yes');
    assert.equal(pi.probe, 'fail');
    // A refused spawn fails at once; the 90 s probe deadline is the other way a probe can fail.
    // setup's JSON carries the status only; the reason belongs to doctor (T069).
    assert.ok(result.elapsedMs < 10_000, `setup took ${result.elapsedMs.toFixed(0)} ms`);
    assert.equal(existsSync(join(place.home, '.pi', 'agent', 'extensions', 'oboete.js')), true);
  } finally {
    place.cleanup();
  }
});

scenario('prior-failure counters recorded', (t: TestContext) => {
  const place = fixture();
  try {
    // Not a fault: the extension hands its in-memory failure codes to the next child, and the
    // child is the only durable record of them (R12, A8). Distinct codes never collide; a repeat
    // of the same code counts up.
    for (const pass of [1, 2]) {
      const result = spawnEngine(captureArgs(`inv-${pass}`, 'capture_failed,capture_spawn_failed'), {
        home: place.home,
        cwd: place.repo,
        input: JSON.stringify(envelope(place, pass)),
        timeoutMs: 5_000,
      });
      assert.equal(result.status, 0, `capture exited ${result.status}: ${result.stderr}`);
      assert.equal(result.stdout, '');
      const found = diagnostics(place, 'pi_child_failed');
      assert.deepEqual(
        found.map((row) => [row.message_code, Number(row.count)]),
        [
          ['capture_failed', pass],
          ['capture_spawn_failed', pass],
        ],
      );
      assert.equal(existsSync(join(piAck(place), `inv-${pass}.done`)), true, 'a finished child renames its ack');
    }
    assert.equal(rows(place, "SELECT id FROM raw_events WHERE kind = 'tool_result'").length, 2);
    t.diagnostic(`pi-ack: ${readdirSync(piAck(place)).sort().join(', ')}`);
  } finally {
    place.cleanup();
  }
});
