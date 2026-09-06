import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import {
  closeSync,
  existsSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { test, type TestContext } from 'node:test';
import { fileURLToPath } from 'node:url';

import { openDatabase } from '../src/db/open.js';
import { oboetePaths, type OboetePaths } from '../src/paths.js';
import { cjkBigrams } from '../src/retrieval/fts.js';
import { resolveRepoIdentity } from '../src/repo-identity.js';

type Json = Record<string, unknown>;

function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    assert.notEqual(parent, directory);
    directory = parent;
  }
}

const ROOT = repositoryRoot();
const BUNDLE = join(ROOT, 'dist', 'oboete.mjs');
const NOW = 1_800_000_000_000;
const HARD_LIMIT_MS = 1_300;

type Fixture = { home: string; repo: string; paths: OboetePaths; cleanup(): void };

function fixture(): Fixture {
  const home = mkdtempSync(join(tmpdir(), 'oboete-e2e-inject-home-'));
  const paths = oboetePaths(home);
  const repo = mkdtempSync(join(tmpdir(), 'oboete-e2e-inject-repo-'));
  spawnSync('git', ['-C', repo, 'init', '--quiet']);
  const identity = resolveRepoIdentity(repo);
  const db = openDatabase({ path: paths.db, timeoutMs: 5_000 }).db;
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, ?, ?, ?, ?, ?)`,
  ).run(identity.id, identity.identityKind, identity.normalizedIdentity, identity.root, NOW, NOW);
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, created_at)
     VALUES ('m-summary', ?, 'session_summary', 'Previous session', ?, '', 'ms', 'cs',
       'eligible', 'unreviewed', ?)`,
  ).run(identity.id, 'The previous migration completed.', NOW - 2_000);
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
       started_at, ended_at, status, turn_count, latest_summary_memory_id, summary_state)
     VALUES ('s-previous', ?, 'claude', 'previous-native', 's-previous', 'claude-opus-5[1m]',
       ?, ?, 'ended', 1, 'm-summary', 'done')`,
  ).run(identity.id, NOW - 10_000, NOW - 1_000);
  const title = '日本語の検索メモリ';
  const body = '予約データベースの接続設定はローカル構成にあります。';
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, created_at)
     VALUES ('m-ja', ?, 'discovery', ?, ?, ?, 'mj', 'cj', 'eligible', 'unreviewed', ?)`,
  ).run(identity.id, title, body, cjkBigrams(`${title} ${body}`), NOW - 1_000);
  db.close();
  return {
    home,
    repo,
    paths,
    cleanup: () => {
      rmSync(home, { recursive: true, force: true });
      rmSync(repo, { recursive: true, force: true });
    },
  };
}

async function run(
  context: TestContext,
  place: Fixture,
  command: 'hook' | 'inject',
  args: string[],
  input: Json,
  environment: Record<string, string> = {},
): Promise<{ stdout: string; elapsed: number }> {
  const id = `${command}-${args.at(-1) ?? 'command'}-${performance.now()}`.replaceAll(/[^\w.-]/g, '_');
  const stdinFile = join(place.home, `${id}.stdin`);
  const stdoutFile = join(place.home, `${id}.stdout`);
  const stderrFile = join(place.home, `${id}.stderr`);
  writeFileSync(stdinFile, JSON.stringify(input));
  const stdin = openSync(stdinFile, 'r');
  const stdout = openSync(stdoutFile, 'w');
  const stderr = openSync(stderrFile, 'w');
  const started = performance.now();
  const child = spawn(process.execPath, [BUNDLE, command, ...args], {
    cwd: place.repo,
    env: { ...process.env, OBOETE_HOME: place.home, ...environment },
    stdio: [stdin, stdout, stderr],
  });
  closeSync(stdin);
  closeSync(stdout);
  closeSync(stderr);
  const status = await new Promise<number | null>((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });
  const elapsed = performance.now() - started;
  const capturedStdout = readFileSync(stdoutFile, 'utf8');
  const capturedStderr = readFileSync(stderrFile, 'utf8');
  context.diagnostic(`${command} ${args.join(' ')} took ${elapsed.toFixed(1)} ms`);
  assert.equal(status, 0, capturedStderr);
  assert.ok(elapsed < HARD_LIMIT_MS, `the injection hook took ${elapsed.toFixed(1)} ms`);
  return { stdout: capturedStdout, elapsed };
}

function parseEnvelope(stdout: string, eventName: string): string {
  const parsed = JSON.parse(stdout) as {
    hookSpecificOutput: { hookEventName: string; additionalContext: string };
  };
  assert.equal(parsed.hookSpecificOutput.hookEventName, eventName);
  assert.ok(parsed.hookSpecificOutput.additionalContext.startsWith('oboete memory context'));
  assert.equal(parsed.hookSpecificOutput.additionalContext.startsWith('{'), false);
  return parsed.hookSpecificOutput.additionalContext;
}

test('the real bundle injects through the Claude, Grok, Codex, and Pi channels', async (context) => {
  const place = fixture();
  try {
    const claude = (await run(
      context,
      place,
      'hook',
      ['--agent', 'claude-or-grok', '--event', 'SessionStart'],
      {
        session_id: 'claude-start',
        cwd: place.repo,
        source: 'startup',
        model: 'claude-opus-5[1m]',
      },
    )).stdout;
    assert.ok(
      claude.startsWith('oboete memory context'),
      readFileSync(place.paths.hookLog, 'utf8'),
    );
    assert.ok(claude.endsWith('end of oboete memory context'));

    const grokStart = (await run(
      context,
      place,
      'hook',
      ['--agent', 'claude-or-grok', '--event', 'SessionStart'],
      { sessionId: 'grok-start', cwd: place.repo, source: 'new' },
      { GROK_SESSION_ID: 'grok-start' },
    )).stdout;
    assert.equal(grokStart, '');

    const grokPre = (await run(
      context,
      place,
      'hook',
      ['--agent', 'claude-or-grok', '--event', 'PreToolUse'],
      {
        hookEventName: 'PreToolUse',
        sessionId: 'grok-start',
        cwd: place.repo,
        toolName: 'read_file',
        toolUseId: 'call-1',
        toolInput: { target_file: join(place.repo, 'README.md') },
      },
      { GROK_SESSION_ID: 'grok-start' },
    )).stdout;
    parseEnvelope(grokPre, 'PreToolUse');

    const codex = (await run(
      context,
      place,
      'hook',
      ['--agent', 'codex', '--event', 'UserPromptSubmit'],
      {
        session_id: 'codex-new',
        turn_id: 'turn-1',
        cwd: place.repo,
        model: 'gpt-5.6-sol',
        prompt: '日本語の検索メモリと予約データベース',
      },
    )).stdout;
    const codexPack = parseEnvelope(codex, 'UserPromptSubmit');
    assert.ok(codexPack.includes('日本語の検索メモリ'), codexPack);
    assert.ok(codexPack.includes('previous migration'), codexPack);

    const pi = (await run(
      context,
      place,
      'inject',
      ['--agent', 'pi', '--kind', 'prompt'],
      {
        cwd: place.repo,
        session_id: 'pi-new',
        prompt: '日本語の検索メモリと予約データベース',
        model: 'gpt-5.6-luna',
      },
    )).stdout;
    assert.ok(pi.startsWith('oboete memory context'));
    assert.ok(pi.includes('日本語の検索メモリ'));
    assert.ok(pi.endsWith('end of oboete memory context'));
  } finally {
    place.cleanup();
  }
});
