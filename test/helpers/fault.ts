// Shared spawn harness for the User Story 2 failure matrix (T058–T062).
// `scenario` is what makes the quickstart loop iterate one fault name per invocation
// while `npm test` runs every file. Sources: quickstart.md "Failure injection",
// contracts/agents.md (deadline), FR-002, FR-003, R1, R6.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { DatabaseSync, type SQLInputValue } from 'node:sqlite';
import { test, type TestContext } from 'node:test';
import { fileURLToPath } from 'node:url';

import { CAPTURE_DEADLINE_MS } from '../../src/capture.js';
import { PRESET_CATALOG } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';

export type Place = {
  home: string;
  repo: string;
  db: string;
  spool: string;
  cleanup(): void;
};

export type SpawnResult = {
  status: number | null;
  signal: NodeJS.Signals | null;
  stdout: string;
  stderr: string;
  elapsedMs: number;
};

export type SpawnEngineOptions = {
  home: string;
  cwd: string;
  input?: string;
  fault?: string;
  faultUrl?: string;
  extraEnv?: NodeJS.ProcessEnv;
  timeoutMs?: number;
};

type ScenarioFn = (t: TestContext) => void | Promise<void>;

function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    assert.notEqual(parent, directory, 'the repository root must contain package.json');
    directory = parent;
  }
}

export const ROOT = repositoryRoot();
export const BUNDLE = join(ROOT, 'dist', 'oboete.mjs');
export const SELECTOR = 'claude-or-grok';
const CLAUDE_FIXTURE = join(ROOT, 'test', 'contracts', 'claude', 'read.json');

function restoreModes(root: string): void {
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop() as string;
    try {
      chmodSync(current, 0o700);
    } catch {
      continue;
    }
    let entries;
    try {
      entries = readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const path = join(current, entry.name);
      try {
        chmodSync(path, entry.isDirectory() ? 0o700 : 0o600);
      } catch {
        continue;
      }
      if (entry.isDirectory()) stack.push(path);
    }
  }
}

export function childEnv(opts: SpawnEngineOptions): NodeJS.ProcessEnv {
  const env = { ...process.env };
  // A recovery or control run must not inherit the matrix name (or Grok's selector) from the
  // test runner. Callers that want a fault or a Grok child pass it explicitly.
  delete env.OBOETE_TEST_FAULT;
  delete env.OBOETE_TEST_FAULT_URL;
  delete env.GROK_HOOK_EVENT;
  delete env.GROK_SESSION_ID;
  // faultFetch stays off when Node's env proxy is on (envProxyEnabled).
  delete env.NODE_USE_ENV_PROXY;
  delete env.NODE_OPTIONS;
  env.NODE_ENV = 'test';
  env.OBOETE_HOME = opts.home;
  if (opts.extraEnv !== undefined) Object.assign(env, opts.extraEnv);
  if (opts.fault !== undefined) env.OBOETE_TEST_FAULT = opts.fault;
  else delete env.OBOETE_TEST_FAULT;
  if (opts.faultUrl !== undefined) env.OBOETE_TEST_FAULT_URL = opts.faultUrl;
  else delete env.OBOETE_TEST_FAULT_URL;
  return env;
}

/** Wraps `test(name, fn)` and skips unless `OBOETE_TEST_FAULT` is unset or equals `name`. */
export function scenario(name: string, fn: ScenarioFn): void;
export function scenario(name: string, title: string, fn: ScenarioFn): void;
export function scenario(name: string, titleOrFn: string | ScenarioFn, fn?: ScenarioFn): void {
  const title = typeof titleOrFn === 'string' ? titleOrFn : name;
  const body = typeof titleOrFn === 'function' ? titleOrFn : (fn as ScenarioFn);
  const selected = process.env.OBOETE_TEST_FAULT;
  const skip = selected !== undefined && selected !== name;
  test(title, { skip: skip ? `OBOETE_TEST_FAULT=${selected}` : false }, body);
}

/** Fresh `OBOETE_HOME`, a `git init`ed repo, and a worker-migrated `memory.db`. */
export function fixture(): Place {
  const home = mkdtempSync(join(tmpdir(), 'oboete-fault-home-'));
  const repo = mkdtempSync(join(tmpdir(), 'oboete-fault-repo-'));
  spawnSync('git', ['-C', repo, 'init', '--quiet'], { encoding: 'utf8' });
  const db = join(home, 'memory.db');
  // The worker role migrates; the hook itself never does (data-model.md).
  openDatabase({ path: db, timeoutMs: 5_000 }).db.close();
  return {
    home,
    repo,
    db,
    spool: join(home, 'spool'),
    cleanup: () => {
      restoreModes(home);
      restoreModes(repo);
      rmSync(home, { recursive: true, force: true });
      rmSync(repo, { recursive: true, force: true });
    },
  };
}

export function spawnEngine(args: string[], opts: SpawnEngineOptions): SpawnResult {
  const started = performance.now();
  const result = spawnSync(process.execPath, [BUNDLE, ...args], {
    input: opts.input ?? '',
    cwd: opts.cwd,
    encoding: 'utf8',
    env: childEnv(opts),
    timeout: opts.timeoutMs,
    killSignal: 'SIGTERM',
  });
  return {
    status: result.status,
    signal: result.signal,
    stdout: result.stdout ?? '',
    stderr: result.stderr ?? '',
    elapsedMs: performance.now() - started,
  };
}

/**
 * `oboete hook --agent <selector> --event <event>` the way an agent spawns it. Asserts exit 0,
 * empty stdout, and wall time under `2 * CAPTURE_DEADLINE_MS` (same slack as test/e2e-hook.test.ts).
 */
export function runHook(
  t: TestContext,
  place: Place,
  agent: string,
  event: string,
  payload: unknown,
  opts: Omit<SpawnEngineOptions, 'home' | 'cwd' | 'input'> = {},
): number {
  const input = typeof payload === 'string' ? payload : JSON.stringify(payload);
  const result = spawnEngine(['hook', '--agent', agent, '--event', event], {
    ...opts,
    home: place.home,
    cwd: place.repo,
    input,
    timeoutMs: opts.timeoutMs ?? 5_000,
  });
  t.diagnostic(
    `hook --agent ${agent} --event ${event} with ${input.length} bytes of stdin took ${result.elapsedMs.toFixed(1)} ms (budget ${CAPTURE_DEADLINE_MS} ms)`,
  );
  assert.equal(result.status, 0, `the hook exited ${result.status} signal=${result.signal}: ${result.stderr}`);
  assert.equal(result.stdout, '', 'a capture hook prints nothing to stdout');
  assert.ok(
    result.elapsedMs < 2 * CAPTURE_DEADLINE_MS,
    `the hook took ${result.elapsedMs.toFixed(1)} ms`,
  );
  return result.elapsedMs;
}

export function withDb<T>(place: Place, fn: (db: DatabaseSync) => T): T {
  const db = new DatabaseSync(place.db, { timeout: 5_000 });
  try {
    return fn(db);
  } finally {
    db.close();
  }
}

export function rows(place: Place, sql: string, params: SQLInputValue[] = []): Record<string, unknown>[] {
  return withDb(place, (db) => db.prepare(sql).all(...params) as Record<string, unknown>[]);
}

export function run(place: Place, sql: string, params: SQLInputValue[] = []): void {
  withDb(place, (db) => {
    db.prepare(sql).run(...params);
  });
}

export function observeLog(place: Place): string {
  const file = oboetePaths(place.home).observeLog;
  return existsSync(file) ? readFileSync(file, 'utf8') : '';
}

/** Seed the workers-ai catalog cache so observe does not hit api.cloudflare.com (raw fetch, not faultFetch). */
export function seedWorkersAiCatalog(place: Place, accountId: string): void {
  run(place, 'INSERT INTO runtime_state (key, value_json, updated_at) VALUES (?, ?, ?)', [
    'workers_ai_catalog',
    JSON.stringify({
      accountId,
      models: [PRESET_CATALOG['workers-ai'].defaultModel],
      defaultModelPresent: true,
      hasPaidOnlyModels: false,
      fetchedAt: Date.now() - 1_000,
    }),
    Date.now(),
  ]);
}

export function spoolFiles(place: Place): string[] {
  if (!existsSync(place.spool)) return [];
  return readdirSync(place.spool, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith('.json'))
    .map((entry) => entry.name)
    .sort();
}

/**
 * Claude Code payload from test/contracts/claude/read.json. Fixture keys: `PreToolUse`,
 * `PostToolUse`. Other capture kinds are the PostToolUse envelope plus the fields that adapter
 * reads (`error`, `last_assistant_message`, `compact_summary`, `reason`).
 */
export function claudePayload(repo: string, event = 'PostToolUse'): Record<string, unknown> {
  const file = JSON.parse(readFileSync(CLAUDE_FIXTURE, 'utf8')) as {
    events: Record<string, Record<string, unknown>>;
  };
  const fromFixture = file.events[event];
  const payload: Record<string, unknown> = { ...(fromFixture ?? file.events.PostToolUse) };
  payload.cwd = repo;
  payload.hook_event_name = event;
  const toolInput = payload.tool_input;
  if (toolInput !== null && typeof toolInput === 'object' && 'file_path' in toolInput) {
    (toolInput as Record<string, unknown>).file_path = `${repo}/README.md`;
  }
  const response = payload.tool_response;
  if (response !== null && typeof response === 'object' && 'file' in response) {
    const stored = (response as Record<string, unknown>).file;
    if (stored !== null && typeof stored === 'object' && 'filePath' in stored) {
      (stored as Record<string, unknown>).filePath = `${repo}/README.md`;
    }
  }
  if (fromFixture === undefined) {
    if (event === 'PostToolUseFailure') {
      delete payload.tool_response;
      payload.error = 'the file could not be read';
    } else if (event === 'Stop') {
      payload.last_assistant_message = 'the task is done';
    } else if (event === 'PostCompact') {
      payload.compact_summary = 'the conversation so far';
    } else if (event === 'SessionEnd') {
      payload.reason = 'clear';
    }
  }
  return payload;
}
