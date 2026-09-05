// The Phase 2 checkpoint: one event through the real engine bundle, spawned the way an agent
// spawns a hook. Sources: contracts/agents.md ("Hook process rules and SLAs", "Size cap"),
// contracts/cli.md (`hook`), plan.md "Delivery order" step 1 (replay skeleton).
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { test, type TestContext } from 'node:test';
import { fileURLToPath } from 'node:url';

import { openDatabase } from '../src/db/open.js';

type Json = Record<string, unknown>;

function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    assert.notEqual(parent, directory, 'the repository root must contain package.json');
    directory = parent;
  }
}

const ROOT = repositoryRoot();
const BUNDLE = join(ROOT, 'dist', 'oboete.mjs');
// A capture hook is budgeted at 300 ms (FR-002). The number is reported on every run; the hard
// assertion is looser because a shared machine adds process start time this test cannot control.
const BUDGET_MS = 300;
const HARD_LIMIT_MS = 600;
// The e2e files run in their own sequential `node --test` step (package.json "test"), after the
// parallel unit files, so the hook is not competing with 25 other test files for the CPU; the
// stored-row assertions below therefore always run.

type Fixture = { home: string; repo: string; cleanup(): void };

function fixture(): Fixture {
  const home = mkdtempSync(join(tmpdir(), 'oboete-e2e-home-'));
  const repo = mkdtempSync(join(tmpdir(), 'oboete-e2e-repo-'));
  spawnSync('git', ['-C', repo, 'init', '--quiet'], { encoding: 'utf8' });
  // The worker role migrates; the hook itself never does (data-model.md).
  openDatabase({ path: join(home, 'memory.db'), timeoutMs: 5_000 }).db.close();
  return {
    home,
    repo,
    cleanup: () => {
      rmSync(home, { recursive: true, force: true });
      rmSync(repo, { recursive: true, force: true });
    },
  };
}

function runHook(
  context: TestContext,
  place: Fixture,
  args: string[],
  input: string,
  environment: Record<string, string> = {},
): number {
  const started = performance.now();
  const result = spawnSync(process.execPath, [BUNDLE, 'hook', ...args], {
    input,
    cwd: place.repo,
    encoding: 'utf8',
    env: { ...process.env, OBOETE_HOME: place.home, ...environment },
  });
  const elapsed = performance.now() - started;

  context.diagnostic(
    `${args.join(' ')} with ${input.length} bytes of stdin took ${elapsed.toFixed(1)} ms (budget ${BUDGET_MS} ms)`,
  );
  assert.equal(result.status, 0, `the hook exited ${result.status}: ${result.stderr}`);
  assert.equal(result.stdout, '', 'a capture hook prints nothing to stdout');
  assert.ok(elapsed < HARD_LIMIT_MS, `the hook took ${elapsed.toFixed(1)} ms`);
  return elapsed;
}

function rows(place: Fixture, sql: string): Json[] {
  const db = new DatabaseSync(join(place.home, 'memory.db'), { timeout: 5_000 });
  try {
    return db.prepare(sql).all() as Json[];
  } finally {
    db.close();
  }
}

function hookLogLines(place: Fixture): string[] {
  return readFileSync(join(place.home, 'logs', 'hook.log'), 'utf8').trimEnd().split('\n');
}

function claudePayload(repo: string): Json {
  const file = JSON.parse(
    readFileSync(join(ROOT, 'test', 'contracts', 'claude', 'read.json'), 'utf8'),
  ) as Json;
  const payload = { ...((file.events as Json).PostToolUse as Json) };
  payload.cwd = repo;
  (payload.tool_input as Json).file_path = `${repo}/README.md`;
  return payload;
}

test('the real bundle captures one Claude tool result', (context) => {
  const place = fixture();
  try {
    runHook(
      context,
      place,
      ['--agent', 'claude-or-grok', '--event', 'PostToolUse'],
      JSON.stringify(claudePayload(place.repo)),
    );

    const stored = rows(place, 'SELECT agent, kind, content, truncated FROM raw_events');
    assert.equal(stored.length, 1);
    assert.equal(stored[0]?.agent, 'claude');
    assert.equal(stored[0]?.kind, 'tool_result');
    assert.equal(stored[0]?.content, 'oboete probe repository\nsecond line\n');
    assert.equal(stored[0]?.truncated, 0);
    assert.equal(hookLogLines(place).length, 1);
  } finally {
    place.cleanup();
  }
});

test('the real bundle keeps a payload above the read bound as a partial row', (context) => {
  const place = fixture();
  try {
    const payload = claudePayload(place.repo);
    payload.tool_name = 'Write';
    payload.tool_input = { file_path: `${place.repo}/notes.md`, content: 'a'.repeat(1_500_000) };
    delete payload.tool_response;

    runHook(
      context,
      place,
      ['--agent', 'claude-or-grok', '--event', 'PreToolUse'],
      JSON.stringify(payload),
    );

    const stored = rows(place, 'SELECT agent, kind, truncated, classification_state FROM raw_events');
    assert.equal(stored.length, 1);
    assert.equal(stored[0]?.truncated, 1);
    assert.equal(stored[0]?.classification_state, 'partial');
    assert.equal(stored[0]?.kind, 'tool_call');
    assert.equal(hookLogLines(place).length, 1);
  } finally {
    place.cleanup();
  }
});

test('the same handler captures Grok Build when its environment says so', (context) => {
  const place = fixture();
  try {
    const file = JSON.parse(
      readFileSync(join(ROOT, 'test', 'contracts', 'grok', 'read_file.json'), 'utf8'),
    ) as Json;
    const payload = { ...((file.events as Json).PostToolUse as Json) };
    payload.cwd = place.repo;

    runHook(
      context,
      place,
      ['--agent', 'claude-or-grok', '--event', 'PostToolUse'],
      JSON.stringify(payload),
      { GROK_SESSION_ID: String(payload.sessionId) },
    );

    const stored = rows(place, 'SELECT agent, kind FROM raw_events');
    assert.equal(stored.length, 1);
    assert.equal(stored[0]?.agent, 'grok', 'Grok Build must never be recorded as Claude Code');
    assert.equal(hookLogLines(place).length, 1);
  } finally {
    place.cleanup();
  }
});
