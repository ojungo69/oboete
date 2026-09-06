import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { captureEvent, type CaptureDeps } from '../../src/capture.js';
import { isPaused } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { runPause, runResume } from '../../src/pause.js';
import { oboetePaths } from '../../src/paths.js';
import { detectSync } from '../../src/privacy/detect.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_757_000_000_000;
const PAUSED_TEXT =
  'Capture and injection are paused. Run `oboete resume` to continue; existing memories are untouched.\n';
const RESUMED_TEXT = 'Capture and injection are resumed.\n';
const NOT_PAUSED_TEXT = 'oboete was not paused; nothing changed.\n';

type Command = typeof runPause;

async function run(
  command: Command,
  argv: string[],
): Promise<{ status: number; stdout: string; stderr: string }> {
  let stdout = '';
  let stderr = '';
  const status = await command(argv, {
    writeOut: (text) => {
      stdout += text;
    },
    writeError: (text) => {
      stderr += text;
    },
  });
  return { status, stdout, stderr };
}

function hookDeps(): CaptureDeps {
  return {
    detect: (input) => detectSync(input),
    now: () => NOW,
    elapsedMs: () => 0,
    spawnWorker: () => {},
  };
}

function insertMemory(db: DatabaseSync): void {
  const now = 1_000_000;
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, 'remote', ?, ?, ?, ?)`,
  ).run('repo-pause', 'https://example.test/pause', '/tmp/pause', now, now);
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, valid_from, created_at)
     VALUES (?, ?, 'discovery', ?, ?, '[]', '', ?, ?, 'eligible', 'unreviewed', ?, ?)`,
  ).run(
    'm_pause',
    'repo-pause',
    'Pause leaves memories',
    'Existing memories stay untouched.',
    'material-pause',
    'content-pause',
    now,
    now,
  );
}

function count(home: string, sql: string): number {
  const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 });
  try {
    const row = opened.db.prepare(sql).get() as { n: number };
    return Number(row.n);
  } finally {
    opened.db.close();
  }
}

test('pause creates the marker and is idempotent', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    assert.equal(isPaused(paths), false);

    const first = await run(runPause, []);
    assert.equal(first.status, 0);
    assert.equal(first.stdout, PAUSED_TEXT);
    assert.equal(existsSync(paths.paused), true);
    assert.equal(readFileSync(paths.paused, 'utf8'), '');
    assert.equal(statSync(paths.paused).mode & 0o777, 0o600);
    assert.equal(isPaused(paths), true);

    const second = await run(runPause, []);
    assert.equal(second.status, 0);
    assert.equal(second.stdout, PAUSED_TEXT);
    assert.equal(isPaused(paths), true);
  });
});

test('resume removes the marker and a second resume changes nothing', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    const paused = await run(runPause, []);
    assert.equal(paused.status, 0);
    assert.equal(isPaused(paths), true);

    const first = await run(runResume, []);
    assert.equal(first.status, 0);
    assert.equal(first.stdout, RESUMED_TEXT);
    assert.equal(existsSync(paths.paused), false);
    assert.equal(isPaused(paths), false);

    const second = await run(runResume, []);
    assert.equal(second.status, 0);
    assert.equal(second.stdout, NOT_PAUSED_TEXT);
    assert.equal(isPaused(paths), false);
  });
});

test('pause leaves existing memories untouched and skips capture until resume', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    const repo = join(home, 'workspace');
    mkdirSync(repo, { recursive: true });
    const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
    try {
      insertMemory(opened.db);
    } finally {
      opened.db.close();
    }
    const memoriesBefore = count(home, 'SELECT COUNT(*) AS n FROM memories');
    assert.equal(memoriesBefore, 1);

    const payload = {
      session_id: 'pause-session',
      cwd: repo,
      prompt_id: 'prompt-1',
      prompt: 'what is paused?',
    };
    const input = {
      agent: 'claude' as const,
      eventName: 'UserPromptSubmit',
      paths,
      readStdin: () => ({ text: JSON.stringify(payload), truncated: false }),
    };

    assert.equal((await run(runPause, [])).status, 0);
    const paused = await captureEvent(hookDeps(), input);
    assert.equal(paused.outcome, 'paused');
    assert.equal(count(home, 'SELECT COUNT(*) AS n FROM raw_events'), 0);
    assert.equal(count(home, 'SELECT COUNT(*) AS n FROM memories'), memoriesBefore);

    assert.equal((await run(runResume, [])).status, 0);
    const captured = await captureEvent(hookDeps(), input);
    assert.equal(captured.outcome, 'stored');
    assert.equal(count(home, 'SELECT COUNT(*) AS n FROM raw_events'), 1);
    assert.equal(count(home, 'SELECT COUNT(*) AS n FROM memories'), memoriesBefore);
  });
});

test('--json prints a paused flag', async () => {
  await withTempHome(async () => {
    const paused = await run(runPause, ['--json']);
    assert.equal(paused.status, 0);
    assert.deepEqual(JSON.parse(paused.stdout) as unknown, { paused: true });

    const resumed = await run(runResume, ['--json']);
    assert.equal(resumed.status, 0);
    assert.deepEqual(JSON.parse(resumed.stdout) as unknown, { paused: false });
  });
});

test('an unknown option exits 2', async () => {
  await withTempHome(async () => {
    const paused = await run(runPause, ['--bogus']);
    assert.equal(paused.status, 2);
    assert.match(paused.stderr, /Unknown option '--bogus'/);
    assert.equal(paused.stdout, '');

    const resumed = await run(runResume, ['--bogus']);
    assert.equal(resumed.status, 2);
    assert.match(resumed.stderr, /Unknown option '--bogus'/);
    assert.equal(resumed.stdout, '');
  });
});
