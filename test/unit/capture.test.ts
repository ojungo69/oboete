import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  CAPTURE_DEADLINE_MS,
  INJECTION_DEADLINE_MS,
  RAW_EVENT_TTL_MS,
  SPOOL_RESERVE_MS,
  STDIN_READ_BOUND,
  applyCompaction,
  captureEvent,
  readStdinBounded,
  runCapture,
  runHook,
  type CaptureDeps,
  type CaptureOutcome,
} from '../../src/capture.js';
import { openDatabase } from '../../src/db/open.js';
import { eventIdKey, type AgentName, type NormalizedEvent } from '../../src/events.js';
import { ensureDirectories, oboetePaths, type OboetePaths } from '../../src/paths.js';
import { detectSync, type DetectorInput } from '../../src/privacy/detect.js';
import type { GitSpawn } from '../../src/repo-identity.js';
import { listSpool, readSpoolEntry, spoolEntrySchema } from '../../src/spool.js';
import { recoverSpool } from '../../src/worker/batches.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';

type Json = Record<string, unknown>;

const NOW = 1_757_000_000_000;

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

function fixture(agent: string, name: string): Json {
  return JSON.parse(readFileSync(join(ROOT, 'test', 'contracts', agent, name), 'utf8')) as Json;
}

type CorpusLine = { id: string; secret: string; text: string };

const CORPUS = readFileSync(join(ROOT, 'test', 'corpus', 'secrets.jsonl'), 'utf8')
  .split('\n')
  .filter((line) => line.trim() !== '')
  .map((line) => JSON.parse(line) as CorpusLine);

/** Secret-shaped literals live in the corpus, never in this file. */
function corpusLine(id: string): CorpusLine {
  const line = CORPUS.find((entry) => entry.id === id);
  if (line === undefined) assert.fail(`the corpus is missing the line ${id}`);
  return line;
}

type Context = {
  home: string;
  repo: string;
  paths: OboetePaths;
  deps: CaptureDeps;
  spawned: number;
  capture(
    agent: AgentName,
    eventName: string,
    payload: unknown,
    over?: {
      deps?: Partial<CaptureDeps>;
      text?: string;
      truncated?: boolean;
      priorFailures?: string[];
    },
  ): Promise<CaptureOutcome>;
  all(sql: string, ...params: (string | number)[]): Json[];
};

/**
 * A temporary data directory with a migrated database and a working directory that stands in for a
 * repository. The detector is the real one, called in this process: the worker is the hook's wall
 * time bound (contracts/agents.md SLAs), which the end-to-end test exercises through the bundle.
 */
async function withCapture(
  fn: (context: Context) => Promise<void>,
  options: { database?: boolean } = {},
): Promise<void> {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const repo = join(home, 'workspace');
    mkdirSync(repo, { recursive: true });
    if (options.database !== false) openDatabase({ path: paths.db, timeoutMs: 2_000 }).db.close();

    const context: Context = {
      home,
      repo,
      paths,
      spawned: 0,
      deps: {
        // The cutoff is the worker's business; in process the real detector simply runs.
        detect: (input) => detectSync(input),
        now: () => NOW,
        elapsedMs: () => 0,
        spawnWorker: () => {
          context.spawned += 1;
        },
      },
      capture: (agent, eventName, payload, over = {}) => {
        const text = over.text ?? JSON.stringify(payload);
        return captureEvent(
          { ...context.deps, ...over.deps },
          {
            agent,
            eventName,
            paths,
            readStdin: () => ({ text, truncated: over.truncated ?? false }),
            priorFailures: over.priorFailures,
          },
        );
      },
      all: (sql, ...params) => {
        const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
        try {
          return opened.db.prepare(sql).all(...params) as Json[];
        } finally {
          opened.db.close();
        }
      },
    };
    await fn(context);
  });
}

function claudePostToolUse(repo: string, content: string): Json {
  const payload = { ...((fixture('claude', 'read.json').events as Json).PostToolUse as Json) };
  payload.cwd = repo;
  payload.tool_response = {
    type: 'text',
    file: { filePath: `${repo}/README.md`, content, numLines: 1, startLine: 1, totalLines: 1 },
  };
  (payload.tool_input as Json).file_path = `${repo}/README.md`;
  return payload;
}

function expectedToolResultId(payload: Json, repo: string, output: string): string {
  // Recomputed from events.ts, not from capture: the key is the array eventIdKey builds.
  const event: NormalizedEvent = {
    agent: 'claude',
    native_session_id: payload.session_id as string,
    cwd: repo,
    captured_at: NOW,
    kind: 'tool_result',
    prompt_id: payload.prompt_id as string,
    tool_call_id: payload.tool_use_id as string,
    output,
    is_error: false,
  };
  return createHash('sha256').update(JSON.stringify(eventIdKey(event)), 'utf8').digest('hex');
}

function everythingWritten(paths: OboetePaths): string {
  const parts: string[] = [];
  for (const name of listSpool(paths)) parts.push(readFileSync(join(paths.spool, name), 'utf8'));
  if (existsSync(paths.hookLog)) parts.push(readFileSync(paths.hookLog, 'utf8'));
  if (existsSync(paths.db)) parts.push(readFileSync(paths.db, 'latin1'));
  return parts.join('\n');
}

test('a Claude tool result becomes one raw_events row with the derived id and the stored content', async () => {
  await withCapture(async (context) => {
    const content = 'oboete probe repository\nsecond line\n';
    const payload = claudePostToolUse(context.repo, content);

    const outcome = await context.capture('claude', 'PostToolUse', payload);
    assert.equal(outcome.outcome, 'stored');

    const rows = context.all('SELECT * FROM raw_events');
    assert.equal(rows.length, 1);
    const row = rows[0] as Json;
    assert.equal(row.id, expectedToolResultId(payload, context.repo, content));
    assert.equal(row.kind, 'tool_result');
    assert.equal(row.agent, 'claude');
    // Fail open: a payload with nothing to redact is stored exactly as the agent reported it.
    assert.equal(row.content, content);
    assert.equal(row.sensitivity, 'local_only');
    assert.equal(row.classification_state, 'done');
    assert.equal(row.truncated, 0);
    assert.equal(row.via_spool, 0);
    assert.equal(row.captured_at, NOW);
    assert.equal(row.expires_at, NOW + RAW_EVENT_TTL_MS);
    const stored = JSON.parse(row.payload_json as string) as Json;
    assert.equal(stored.output, undefined, 'payload_json keeps metadata, not content');
    assert.equal(stored.tool_call_id, payload.tool_use_id);

    const sessions = context.all('SELECT * FROM sessions');
    assert.equal(sessions.length, 1);
    assert.equal(sessions[0]?.native_session_id, payload.session_id);
    assert.equal(sessions[0]?.conversation_id, sessions[0]?.id, 'a fresh session is its own root');
    assert.equal(sessions[0]?.agent, 'claude');
    assert.equal(context.all('SELECT * FROM repos').length, 1);
    assert.deepEqual(listSpool(context.paths), []);
  });
});

test('fail-closed: a secret in a tool result is redacted before the first write', async () => {
  await withCapture(async (context) => {
    const line = corpusLine('aws-secret-access-key');
    const payload = claudePostToolUse(context.repo, `deploy notes\n${line.text}\n`);

    await context.capture('claude', 'PostToolUse', payload);

    const row = context.all('SELECT * FROM raw_events')[0] as Json;
    assert.match(row.content as string, /\[REDACTED:/);
    assert.equal(row.sensitivity, 'secret', 'a rule hit classifies the row as secret (FR-017)');
    assert.ok(
      !everythingWritten(context.paths).includes(line.secret),
      'the secret reached the database, the spool or the log',
    );
  });
});

test('a re-delivered payload leaves one row and one turn', async () => {
  await withCapture(async (context) => {
    const payload = { ...((fixture('claude', 'read.json').events as Json).PostToolUse as Json) };
    payload.cwd = context.repo;

    await context.capture('claude', 'PostToolUse', payload);
    const second = await context.capture('claude', 'PostToolUse', payload);

    assert.equal(second.rows, 0, 're-delivery stores nothing new');
    assert.equal(context.all('SELECT id FROM raw_events').length, 1);
  });
});

test('a prompt opens a turn and Stop closes it', async () => {
  await withCapture(async (context) => {
    const session = 'session-turns';
    const base = { session_id: session, cwd: context.repo, prompt_id: 'prompt-1' };

    await context.capture('claude', 'UserPromptSubmit', { ...base, prompt: 'first question' });
    await context.capture('claude', 'Stop', { ...base, last_assistant_message: 'first answer' });

    const turns = context.all('SELECT * FROM turns');
    assert.equal(turns.length, 1);
    assert.equal(turns[0]?.ordinal, 1);
    assert.equal(turns[0]?.ended_at, NOW);
    assert.equal((context.all('SELECT turn_count FROM sessions')[0] as Json).turn_count, 1);
    const kinds = context.all('SELECT kind FROM raw_events ORDER BY kind').map((row) => row.kind);
    assert.deepEqual(kinds, ['last_assistant_message', 'prompt', 'turn_end']);
  });
});

test('resume keeps the conversation root and fork starts a new one', async () => {
  await withCapture(async (context) => {
    const session = 'session-root';
    await context.capture('claude', 'SessionStart', {
      session_id: session,
      cwd: context.repo,
      source: 'startup',
    });
    const root = context.all('SELECT id, conversation_id FROM sessions')[0] as Json;

    await context.capture('claude', 'SessionStart', {
      session_id: session,
      cwd: context.repo,
      source: 'resume',
    });
    await context.capture('claude', 'SessionStart', {
      session_id: 'session-fork',
      cwd: context.repo,
      source: 'fork',
    });

    const sessions = context.all('SELECT id, native_session_id, conversation_id FROM sessions');
    assert.equal(sessions.length, 2);
    const resumed = sessions.find((row) => row.native_session_id === session) as Json;
    const forked = sessions.find((row) => row.native_session_id === 'session-fork') as Json;
    assert.equal(resumed.conversation_id, root.conversation_id);
    assert.equal(forked.conversation_id, forked.id, 'a fork is its own root');
  });
});

test('Grok load with the same session id keeps the root, a new id starts one', async () => {
  await withCapture(async (context) => {
    const session = '01a06800-30c4-7d11-b7d4-184941e9c38a';
    await context.capture('grok', 'SessionStart', {
      sessionId: session,
      cwd: context.repo,
      source: 'new',
    });
    const root = context.all('SELECT conversation_id FROM sessions')[0] as Json;

    await context.capture('grok', 'SessionStart', {
      sessionId: session,
      cwd: context.repo,
      source: 'load',
    });
    await context.capture('grok', 'SessionStart', {
      sessionId: 'forked-session',
      cwd: context.repo,
      source: 'load',
    });

    const sessions = context.all('SELECT id, native_session_id, conversation_id FROM sessions');
    assert.equal(sessions.length, 2);
    assert.equal(
      (sessions.find((row) => row.native_session_id === session) as Json).conversation_id,
      root.conversation_id,
    );
    const forked = sessions.find((row) => row.native_session_id === 'forked-session') as Json;
    assert.equal(forked.conversation_id, forked.id, 'Grok --fork-session reports load with a new id');
  });
});

test('a resumed session is reopened, so its later work is summarized again (FR-024)', async () => {
  await withCapture(async (context) => {
    const base = { session_id: 'session-reopen', cwd: context.repo };
    await context.capture('claude', 'SessionStart', { ...base, source: 'startup' });
    await context.capture('claude', 'SessionEnd', { ...base, reason: 'clear' });

    // The worker summarized that run: summary_state is done and the memory is recorded.
    const opened = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    opened.db
      .prepare("UPDATE sessions SET summary_state = 'done', latest_summary_memory_id = ?")
      .run('m_first');
    opened.db.close();

    await context.capture('claude', 'SessionStart', { ...base, source: 'resume' });
    const resumed = context.all(
      'SELECT status, ended_at, summary_state, latest_summary_memory_id FROM sessions',
    )[0] as Json;
    assert.equal(resumed.status, 'active', 'a resumed session is active again');
    assert.equal(resumed.ended_at, null);
    assert.equal(resumed.summary_state, null, 'the finished summary does not cover the new work');
    assert.equal(resumed.latest_summary_memory_id, 'm_first', 'the old summary still stands');

    await context.capture('claude', 'SessionEnd', { ...base, reason: 'exit' });
    const ended = context.all('SELECT status, summary_state FROM sessions')[0] as Json;
    assert.equal(ended.status, 'ended');
    assert.equal(ended.summary_state, 'pending', 'the resumed run is summarized again');
  });
});

test('a re-delivered SessionEnd does not reopen the session', async () => {
  await withCapture(async (context) => {
    const payload = { session_id: 'session-redelivered-end', cwd: context.repo, reason: 'clear' };
    await context.capture('claude', 'SessionEnd', payload);
    await context.capture('claude', 'SessionEnd', payload);

    const rows = context.all('SELECT status, summary_state FROM sessions');
    assert.equal(rows.length, 1);
    assert.equal(rows[0]?.status, 'ended');
    assert.equal(rows[0]?.summary_state, 'pending');
  });
});

test('Pi keeps the root of a session id it already knows', async () => {
  await withCapture(async (context) => {
    const envelope = {
      event: 'session_start',
      session_id: 'pi-session',
      cwd: context.repo,
      payload: { reason: 'startup' },
    };

    await context.capture('pi', 'session_start', envelope);
    const root = context.all('SELECT conversation_id FROM sessions')[0] as Json;
    await context.capture('pi', 'session_start', { ...envelope, payload: { reason: 'startup' } });

    const sessions = context.all('SELECT conversation_id FROM sessions');
    assert.equal(sessions.length, 1);
    assert.equal(sessions[0]?.conversation_id, root.conversation_id);
  });
});

test('a Codex hook with a new session id and no SessionStart starts a new root (A18)', async () => {
  await withCapture(async (context) => {
    const first = {
      session_id: 'codex-one',
      turn_id: 'turn-1',
      cwd: context.repo,
      prompt: 'the first question',
    };
    await context.capture('codex', 'UserPromptSubmit', first);
    await context.capture('codex', 'UserPromptSubmit', { ...first, session_id: 'codex-two' });

    const sessions = context.all('SELECT id, conversation_id FROM sessions');
    assert.equal(sessions.length, 2);
    for (const session of sessions) assert.equal(session.conversation_id, session.id);
  });
});

function compactionEvent(agent: AgentName, key: string, text: string): NormalizedEvent {
  return {
    agent,
    native_session_id: 'session-1',
    cwd: '/repo',
    captured_at: NOW,
    kind: 'compaction_summary',
    text,
    compaction_key: key,
  };
}

test('applyCompaction advances the epoch once per compaction on Grok', async () => {
  const first = compactionEvent('grok', '2026-09-03T16:01:11.622755654+00:00', '');
  const again = compactionEvent('grok', '2026-09-03T16:04:02.101110000+00:00', '');

  const opened = applyCompaction('grok', first, { contextEpoch: 0, lastCompactionKey: null }, 'id-1');
  assert.equal(opened?.contextEpoch, 1);
  assert.equal(
    applyCompaction('grok', first, opened as never, 'id-1'),
    null,
    'a re-delivery adds no epoch',
  );
  const second = applyCompaction('grok', again, opened as never, 'id-2');
  assert.equal(second?.contextEpoch, 2);
});

test('applyCompaction keys Claude Code and Codex compactions by the event id (A16)', async () => {
  const claudeStart: NormalizedEvent = {
    agent: 'claude',
    native_session_id: 'session-1',
    cwd: '/repo',
    captured_at: NOW,
    kind: 'session_start',
    source: 'compact',
  };
  // On Claude Code the SessionStart(compact) hook runs ~24 ms before PostCompact, so it opens the
  // epoch and PostCompact only confirms it (R13 "Compaction identity and order", A16).
  const opened = applyCompaction(
    'claude',
    claudeStart,
    { contextEpoch: 0, lastCompactionKey: null },
    'start-1',
  );
  assert.equal(opened?.contextEpoch, 1);
  // A16: without a native per-compaction value the key is the stored id of the PostCompact row.
  const confirmed = applyCompaction(
    'claude',
    compactionEvent('claude', '', 'summary text'),
    opened as never,
    'postcompact-1',
  );
  assert.equal(confirmed?.contextEpoch, 1, 'PostCompact must not advance the epoch a second time');
  const next = applyCompaction('claude', claudeStart, confirmed as never, 'start-2');
  assert.equal(next?.contextEpoch, 2, 'the next compaction opens the next epoch');

  const codex = compactionEvent('codex', '', '');
  const codexOpened = applyCompaction(
    'codex',
    codex,
    { contextEpoch: 0, lastCompactionKey: null },
    'postcompact-2',
  );
  assert.equal(codexOpened?.contextEpoch, 1);
  assert.equal(applyCompaction('codex', codex, codexOpened as never, 'postcompact-2'), null);
  // Codex fires SessionStart(compact) after PostCompact, so it only reads the epoch.
  assert.equal(
    applyCompaction(
      'codex',
      { ...codex, kind: 'session_start', source: 'compact' } as never,
      codexOpened as never,
      'start-3',
    ),
    null,
  );
});

test('applyCompaction keys Pi compactions by compactionEntry.id', async () => {
  const first = compactionEvent('pi', '480afbf2', 'summary');
  const second = compactionEvent('pi', '4283239e', 'summary');
  const opened = applyCompaction('pi', first, { contextEpoch: 0, lastCompactionKey: null }, 'id-1');
  assert.equal(opened?.contextEpoch, 1);
  assert.equal(applyCompaction('pi', first, opened as never, 'id-1'), null);
  assert.equal(applyCompaction('pi', second, opened as never, 'id-2')?.contextEpoch, 2);
});

test('a Claude compaction advances the epoch of the root session exactly once', async () => {
  await withCapture(async (context) => {
    const session = 'session-compact';
    await context.capture('claude', 'SessionStart', {
      session_id: session,
      cwd: context.repo,
      source: 'startup',
    });
    assert.equal((context.all('SELECT context_epoch FROM sessions')[0] as Json).context_epoch, 0);

    await context.capture('claude', 'SessionStart', {
      session_id: session,
      cwd: context.repo,
      source: 'compact',
    });
    assert.equal((context.all('SELECT context_epoch FROM sessions')[0] as Json).context_epoch, 1);

    await context.capture('claude', 'PostCompact', {
      session_id: session,
      cwd: context.repo,
      compact_summary: 'the conversation so far',
    });
    assert.equal((context.all('SELECT context_epoch FROM sessions')[0] as Json).context_epoch, 1);
  });
});

for (const reason of ['deadline', 'detector_error'] as const) {
  test(`fail-closed: a detector ${reason} stores metadata only`, async () => {
    await withCapture(async (context) => {
      const line = corpusLine('github-classic-pat');
      const payload = claudePostToolUse(context.repo, line.text);

      await context.capture('claude', 'PostToolUse', payload, {
        deps: { detect: async () => ({ ok: false, reason }) },
      });

      const row = context.all('SELECT * FROM raw_events')[0] as Json;
      assert.equal(row.content, null);
      assert.equal(row.classification_state, 'failed');
      assert.equal((JSON.parse(row.payload_json as string) as Json).failure_reason, reason);
      assert.ok(!everythingWritten(context.paths).includes(line.secret));
    });
  });
}

test('a Claude compaction whose PostCompact commits first advances the epoch once', async () => {
  await withCapture(async (context) => {
    const session = 'session-compact-reordered';
    const base = { session_id: session, cwd: context.repo };
    await context.capture('claude', 'SessionStart', { ...base, source: 'startup' });

    // The two hooks fire about 24 ms apart and are serialized only by BEGIN IMMEDIATE, so
    // PostCompact can commit first (A16); one compaction still opens exactly one epoch.
    await context.capture('claude', 'PostCompact', {
      ...base,
      compact_summary: 'the conversation so far',
    });
    await context.capture('claude', 'SessionStart', { ...base, source: 'compact' });
    assert.equal((context.all('SELECT context_epoch FROM sessions')[0] as Json).context_epoch, 1);

    // The next compaction still opens the next epoch, in either order.
    await context.capture('claude', 'PostCompact', { ...base, compact_summary: 'and later' });
    await context.capture('claude', 'SessionStart', { ...base, source: 'compact' });
    assert.equal((context.all('SELECT context_epoch FROM sessions')[0] as Json).context_epoch, 2);
  });
});

test('fail-closed: a malformed .oboete.toml makes the event a failed row', async () => {
  await withCapture(async (context) => {
    writeFileSync(join(context.repo, '.oboete.toml'), '[privacy\nsecret_paths = ["secrets/**"]\n');
    const payload = claudePostToolUse(context.repo, 'the deployment notes');

    await context.capture('claude', 'PostToolUse', payload);

    const row = context.all('SELECT * FROM raw_events')[0] as Json;
    assert.equal(row.content, null);
    assert.equal(row.classification_state, 'failed');
    assert.equal((JSON.parse(row.payload_json as string) as Json).failure_reason, 'config_malformed');
  });
});

test('fail-closed: a repository path rule keeps the tool name and drops the content', async () => {
  await withCapture(async (context) => {
    writeFileSync(join(context.repo, '.oboete.toml'), '[privacy]\nsecret_paths = ["secrets/**"]\n');
    const line = corpusLine('rsa-private-key');
    const payload = { ...((fixture('claude', 'read.json').events as Json).PostToolUse as Json) };
    payload.cwd = context.repo;
    payload.tool_input = { file_path: `${context.repo}/secrets/x.pem` };
    payload.tool_response = {
      type: 'text',
      file: { filePath: `${context.repo}/secrets/x.pem`, content: line.text },
    };

    await context.capture('claude', 'PostToolUse', payload);

    const row = context.all('SELECT * FROM raw_events')[0] as Json;
    assert.equal(row.content, null);
    assert.equal(row.sensitivity, 'secret');
    const stored = JSON.parse(row.payload_json as string) as Json;
    assert.equal(stored.path_rule, 'secrets/**');
    assert.ok(!JSON.stringify(stored).includes(line.secret));
    assert.ok(!everythingWritten(context.paths).includes(line.secret));
  });
});

test('fail-closed: a tool without a fixture is stored as metadata with unmapped_payload', async () => {
  await withCapture(async (context) => {
    const line = corpusLine('slack-bot-token');
    const payload = {
      session_id: 'session-unknown-tool',
      cwd: context.repo,
      tool_name: 'NotebookEdit',
      tool_use_id: 'call-1',
      tool_input: { notebook_path: `${context.repo}/notes.ipynb`, source: line.text },
    };

    await context.capture('claude', 'PreToolUse', payload);

    const row = context.all('SELECT * FROM raw_events')[0] as Json;
    assert.equal(row.content, null);
    assert.equal(row.classification_state, 'failed');
    const stored = JSON.parse(row.payload_json as string) as Json;
    assert.equal(stored.failure_reason, 'unmapped_payload');
    assert.equal(stored.tool_name, 'NotebookEdit');
    assert.ok(!everythingWritten(context.paths).includes(line.secret));
  });
});

test('an event name this build does not capture stores nothing', async () => {
  await withCapture(async (context) => {
    const outcome = await context.capture('claude', 'PreCompact', {
      session_id: 'session-1',
      cwd: context.repo,
    });
    assert.equal(outcome.outcome, 'not_captured');
    assert.equal(context.all('SELECT id FROM raw_events').length, 0);
  });
});

test('a payload above the read bound becomes one partial row (A7, A14)', async () => {
  await withCapture(async (context) => {
    const line = corpusLine('openai-api-key');
    const tail = 'TAILMARKER';
    const payload = {
      session_id: 'session-partial',
      cwd: context.repo,
      tool_name: 'Write',
      tool_use_id: 'call-huge',
      tool_input: {
        file_path: `${context.repo}/notes.md`,
        content: `${line.text}\n${'a'.repeat(1_500_000)}${tail}`,
      },
    };
    const whole = JSON.stringify(payload);
    assert.ok(whole.length > STDIN_READ_BOUND);

    const outcome = await context.capture('claude', 'PreToolUse', payload, {
      text: whole.slice(0, STDIN_READ_BOUND),
      truncated: true,
    });
    assert.equal(outcome.outcome, 'stored');

    const rows = context.all('SELECT * FROM raw_events');
    assert.equal(rows.length, 1);
    const row = rows[0] as Json;
    assert.equal(row.truncated, 1);
    assert.equal(row.classification_state, 'partial');
    assert.equal(row.kind, 'tool_call', 'the kind comes from the --event argument');
    assert.ok((row.content as string).length <= STDIN_READ_BOUND);
    assert.match(row.content as string, /\[REDACTED:/);
    const written = everythingWritten(context.paths);
    assert.ok(!written.includes(line.secret), 'the planted secret survived in the prefix');
    assert.ok(!written.includes(tail), 'bytes past the read bound were stored');
    assert.equal(
      (context.all('SELECT native_session_id FROM sessions')[0] as Json).native_session_id,
      'session-partial',
      'the session id is recovered from the bounded prefix scan',
    );
  });
});

test('an unparsed injection event uses the injection deadline', async () => {
  await withCapture(async (context) => {
    let cutoff = 0;
    const elapsed = CAPTURE_DEADLINE_MS + 10;
    const outcome = await context.capture('claude', 'UserPromptSubmit', {}, {
      text: `{"session_id":"session-unparsed-deadline","prompt":"${'a'.repeat(200)}`,
      truncated: true,
      deps: {
        elapsedMs: () => elapsed,
        detect: (input, cutoffMs) => {
          cutoff = cutoffMs;
          return detectSync(input);
        },
      },
    });

    assert.ok(cutoff > CAPTURE_DEADLINE_MS, `the detector received only ${cutoff} ms`);
    assert.equal(outcome.outcome, 'stored');
    assert.equal(listSpool(context.paths).length, 0);
    const rows = context.all('SELECT classification_state, truncated FROM raw_events');
    assert.equal(rows.length, 1);
    assert.equal(rows[0]?.classification_state, 'partial');
    assert.equal(rows[0]?.truncated, 1);
  });
});

test('a cut payload without a session id increments a diagnostics counter only', async () => {
  await withCapture(async (context) => {
    const outcome = await context.capture('claude', 'PreToolUse', {}, {
      text: '{"tool_name":"Write","tool_input":{"content":"aaaa',
      truncated: true,
    });

    assert.equal(outcome.outcome, 'dropped');
    assert.equal(context.all('SELECT id FROM raw_events').length, 0);
    const diagnostics = context.all('SELECT * FROM diagnostics');
    assert.equal(diagnostics.length, 1);
    assert.equal(diagnostics[0]?.kind, 'partial_without_session');
    assert.equal(diagnostics[0]?.count, 1);
  });
});

test('two Grok session starts of one session get one id per turn ordinal (R7)', async () => {
  await withCapture(async (context) => {
    const session = 'grok-session-ordinals';
    const base = { sessionId: session, cwd: context.repo };
    const sessionStart = { ...base, hookEventName: 'session_start', source: 'load' };
    // The event both hooks produce: identical in every field the id is built from except the
    // ordinal of the turn it attaches to.
    const event: NormalizedEvent = {
      agent: 'grok',
      native_session_id: session,
      cwd: context.repo,
      captured_at: NOW,
      kind: 'session_start',
      source: 'resume',
    };
    const idAt = (ordinal: number): string =>
      createHash('sha256').update(JSON.stringify(eventIdKey(event, ordinal)), 'utf8').digest('hex');

    for (const ordinal of [1, 2, 3, 4, 5]) {
      await context.capture('grok', 'UserPromptSubmit', { ...base, prompt: `question ${ordinal}` });
      if (ordinal === 1 || ordinal === 5) {
        await context.capture('grok', 'SessionStart', sessionStart);
      }
    }

    const rows = context.all(
      `SELECT id FROM raw_events WHERE kind = 'session_start' ORDER BY id`,
    ).map((row) => String(row.id));
    assert.equal(rows.length, 2, 'the two session starts belong to two different turns');
    assert.deepEqual(rows, [idAt(1), idAt(5)].sort());
    assert.notEqual(idAt(1), idAt(5));
    assert.equal(
      context.all(`SELECT turn_count FROM sessions`)[0]?.turn_count,
      5,
      'five prompts opened five turns',
    );
  });
});

test('a slow git leaves the detector its slice of the deadline (FR-002)', async () => {
  await withCapture(async (context) => {
    // A cold WSL /mnt/c, an NFS checkout or a cold disk: every git call burns its whole timeout.
    const gitSpawn: GitSpawn = (_file, _args, options) => {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, Number(options.timeout ?? 0));
      return {
        pid: 0,
        output: [],
        stdout: '',
        stderr: '',
        status: null,
        signal: 'SIGTERM',
        error: new Error('git timed out'),
      };
    };

    let cutoff = 0;
    const started = performance.now();
    const outcome = await context.capture(
      'claude',
      'PostToolUse',
      claudePostToolUse(context.repo, 'notes'),
      {
        deps: {
          gitSpawn,
          elapsedMs: () => performance.now() - started,
          detect: (input, cutoffMs) => {
            cutoff = cutoffMs;
            return detectSync(input);
          },
        },
      },
    );

    assert.ok(cutoff > 0, `the detector was left ${cutoff} ms to run in`);
    assert.equal(outcome.outcome, 'stored');
    const row = context.all('SELECT content, classification_state FROM raw_events')[0] as Json;
    assert.equal(row.classification_state, 'done', 'a slow git is not a classification failure');
    assert.match(row.content as string, /notes/);
    // FR-004: without git the identity is still derived here, from the working directory.
    assert.equal(context.all('SELECT identity_kind FROM repos')[0]?.identity_kind, 'common_dir');
  });
});

test('the event goes to the spool when the database file is missing', async () => {
  await withCapture(
    async (context) => {
      const content = 'oboete probe repository\nsecond line\n';
      const payload = claudePostToolUse(context.repo, content);

      const outcome = await context.capture('claude', 'PostToolUse', payload);

      assert.equal(outcome.outcome, 'spooled');
      const names = listSpool(context.paths);
      assert.equal(names.length, 1);
      assert.equal(names[0], `${NOW}-${expectedToolResultId(payload, context.repo, content)}.json`);
      const entry = readSpoolEntry(context.paths, names[0] as string);
      assert.equal(entry?.row.classification_state, 'done');
      assert.equal(entry?.row.kind, 'tool_result');
      assert.equal(entry?.row.content, content);
      assert.equal(entry?.session.native_session_id, payload.session_id);
    },
    { database: false },
  );
});

test('the spool file never holds an unredacted secret', async () => {
  await withCapture(
    async (context) => {
      const line = corpusLine('aws-secret-access-key');

      const outcome = await context.capture(
        'claude',
        'PostToolUse',
        claudePostToolUse(context.repo, `deploy notes\n${line.text}\n`),
      );

      assert.equal(outcome.outcome, 'spooled');
      const names = listSpool(context.paths);
      assert.equal(names.length, 1);
      const written = readFileSync(join(context.paths.spool, names[0] as string), 'utf8');
      assert.match(written, /\[REDACTED:/);
      assert.ok(!written.includes(line.secret), 'the secret reached the spool file');
      assert.ok(!everythingWritten(context.paths).includes(line.secret));
    },
    { database: false },
  );
});

/** Recovers every spool file of `paths` into a fresh database and returns the raw_events rows. */
function recoverInto(paths: OboetePaths): { inserted: number; failed: number; rows: Json[] } {
  const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
  try {
    const token = claimLease(opened.db, { pid: 1, now: NOW });
    assert.notEqual(token, null, 'the recovery needs the worker lease');
    const outcome = recoverSpool(opened.db, paths, token as string, NOW);
    return {
      inserted: outcome.inserted,
      failed: outcome.failed,
      rows: opened.db.prepare('SELECT * FROM raw_events ORDER BY id').all() as Json[],
    };
  } finally {
    opened.db.close();
  }
}

test('a partial row travels through the spool and recovers with its redacted prefix', async () => {
  await withCapture(
    async (context) => {
      const line = corpusLine('openai-api-key');
      const payload = {
        session_id: 'session-partial-spool',
        cwd: context.repo,
        tool_name: 'Write',
        tool_use_id: 'call-huge',
        tool_input: {
          file_path: `${context.repo}/notes.md`,
          content: `${line.text}\n${'a'.repeat(400_000)}`,
        },
      };
      const prefix = JSON.stringify(payload).slice(0, 300 * 1_024);

      const outcome = await context.capture('claude', 'PreToolUse', payload, {
        text: prefix,
        truncated: true,
      });
      assert.equal(outcome.outcome, 'spooled', 'a metadata-only row has a spool file too');

      const names = listSpool(context.paths);
      assert.equal(names.length, 1);
      const parsed = spoolEntrySchema.safeParse(
        JSON.parse(readFileSync(join(context.paths.spool, names[0] as string), 'utf8')),
      );
      assert.equal(parsed.success, true, `the worker cannot read the spool file: ${parsed.error}`);

      const recovered = recoverInto(context.paths);
      assert.equal(recovered.failed, 0, 'the worker quarantined the file the hook wrote');
      assert.equal(recovered.inserted, 1);
      const row = recovered.rows[0] as Json;
      // R7: recovery replays the id the hook wrote and never recomputes it.
      assert.equal(row.id, parsed.data?.row.id);
      assert.equal(row.classification_state, 'partial');
      assert.equal(row.truncated, 1);
      assert.equal(row.via_spool, 1);
      assert.equal(row.kind, 'tool_call', 'the kind comes from the --event argument');
      assert.match(row.content as string, /\[REDACTED:/);
      assert.ok(!(row.content as string).includes(line.secret));
      assert.ok(!everythingWritten(context.paths).includes(line.secret));
    },
    { database: false },
  );
});

test('the spool path recovers the row the direct path would have written', async () => {
  await withCapture(async (direct) => {
    const content = 'oboete probe repository\nsecond line\n';
    // The same payload on both paths, so the repository, the session and the content are the same
    // and only the path into the database differs.
    const payload = claudePostToolUse(direct.repo, content);

    await direct.capture('claude', 'PostToolUse', payload);
    const directRow = direct.all('SELECT * FROM raw_events')[0] as Json;

    await withCapture(
      async (spooled) => {
        const outcome = await spooled.capture('claude', 'PostToolUse', payload);
        assert.equal(outcome.outcome, 'spooled');

        const recovered = recoverInto(spooled.paths);
        assert.equal(recovered.inserted, 1);
        const spooledRow = recovered.rows[0] as Json;

        // The two rows differ in exactly two columns, both by design: `session_id` is a fresh uuid
        // per installation (conventions "Identifiers"), and `via_spool` records the path the row
        // took (data-model.md raw_events).
        assert.equal(spooledRow.via_spool, 1);
        assert.equal(directRow.via_spool, 0);
        assert.notEqual(spooledRow.session_id, directRow.session_id);
        const columns = Object.keys(directRow).filter(
          (name) => name !== 'session_id' && name !== 'via_spool',
        );
        assert.ok(columns.includes('id') && columns.includes('payload_json'));
        for (const column of columns) {
          assert.deepEqual(spooledRow[column], directRow[column], `column ${column}`);
        }

        const sessions = spooled.all('SELECT native_session_id, agent FROM sessions');
        assert.equal(sessions.length, 1);
        assert.equal(sessions[0]?.native_session_id, payload.session_id);
        assert.equal(spooled.all('SELECT id FROM repos').length, 1);
      },
      { database: false },
    );
  });
});

test('a recovered prompt opens the next turn, and later events attach to it (FR-003)', async () => {
  await withCapture(async (context) => {
    const base = { session_id: 'session-spooled-turn', cwd: context.repo };
    await context.capture('claude', 'UserPromptSubmit', { ...base, prompt_id: 'p1', prompt: 'one' });
    await context.capture('claude', 'UserPromptSubmit', { ...base, prompt_id: 'p2', prompt: 'two' });
    assert.equal(context.all('SELECT turn_count FROM sessions')[0]?.turn_count, 2);

    // The third prompt has nothing left after the detector and goes to the spool instead.
    let elapsed = 0;
    const spooled = await context.capture(
      'claude',
      'UserPromptSubmit',
      { ...base, prompt_id: 'p3', prompt: 'three' },
      {
        deps: {
          detect: async (input) => {
            elapsed = INJECTION_DEADLINE_MS - SPOOL_RESERVE_MS + 10;
            return detectSync(input);
          },
          elapsedMs: () => elapsed,
        },
      },
    );
    assert.equal(spooled.outcome, 'spooled');
    assert.equal(recoverInto(context.paths).inserted, 1);

    assert.equal(
      context.all('SELECT turn_count FROM sessions')[0]?.turn_count,
      3,
      'the recovered prompt opened the third turn',
    );
    const third = context.all('SELECT id FROM turns WHERE ordinal = 3')[0] as Json;
    assert.equal(
      context.all(`SELECT turn_id FROM raw_events WHERE kind = 'prompt' ORDER BY captured_at, id`)
        .map((row) => row.turn_id)
        .filter((id) => id === third.id).length,
      1,
      'the recovered prompt itself belongs to the third turn',
    );

    // A later direct-path event of the same session attaches to that turn, not to the second one.
    await context.capture('claude', 'PostToolUse', {
      ...claudePostToolUse(context.repo, 'notes'),
      session_id: base.session_id,
    });
    const tool = context.all(`SELECT turn_id FROM raw_events WHERE kind = 'tool_result'`)[0] as Json;
    assert.equal(tool.turn_id, third.id);
  });
});

test('a metadata-only row is spooled like any other row', async () => {
  await withCapture(
    async (context) => {
      const payload: Json = {
        session_id: 'sess-unmapped',
        cwd: context.repo,
        tool_name: 'NotebookEdit',
        tool_use_id: 'call-1',
        tool_input: { notebook_path: `${context.repo}/notes.ipynb`, source: 'cells' },
      };
      const written: string[] = [];
      const original = process.stderr.write.bind(process.stderr);
      process.stderr.write = ((chunk: string) => {
        written.push(String(chunk));
        return true;
      }) as typeof process.stderr.write;
      try {
        const outcome = await context.capture('claude', 'PreToolUse', payload);
        assert.equal(outcome.outcome, 'spooled');
      } finally {
        process.stderr.write = original;
      }

      // Nothing was lost, so the hook reports nothing to the agent's stderr.
      assert.equal(written.join(''), '');
      const names = listSpool(context.paths);
      assert.equal(names.length, 1);
      const entry = readSpoolEntry(context.paths, names[0] as string);
      assert.equal(entry?.row.classification_state, 'failed');
      assert.equal(entry?.row.content, null, 'a metadata-only row carries no content');
      assert.equal(entry?.session.native_session_id, 'sess-unmapped');
    },
    { database: false },
  );
});

test('the event goes to the spool when the schema is behind the bundle', async () => {
  await withCapture(
    async (context) => {
      copyFileSync(join(ROOT, 'test', 'fixtures', 'previous-version.db'), context.paths.db);

      const outcome = await context.capture(
        'claude',
        'PostToolUse',
        claudePostToolUse(context.repo, 'notes'),
      );

      assert.equal(outcome.outcome, 'spooled');
      assert.equal(listSpool(context.paths).length, 1);
    },
    { database: false },
  );
});

test('below the spool reserve the database is not opened at all', async () => {
  await withCapture(async (context) => {
    const before = statSync(context.paths.db).mtimeMs;
    // A detector that used almost the whole budget: what is left is under the spool reserve.
    let elapsed = 0;
    const detect = async (input: DetectorInput) => {
      elapsed = CAPTURE_DEADLINE_MS - SPOOL_RESERVE_MS + 10;
      return detectSync(input);
    };

    const outcome = await context.capture(
      'claude',
      'PostToolUse',
      claudePostToolUse(context.repo, 'notes'),
      { deps: { detect, elapsedMs: () => elapsed } },
    );

    assert.equal(outcome.outcome, 'spooled');
    assert.equal(listSpool(context.paths).length, 1);
    assert.equal(statSync(context.paths.db).mtimeMs, before);
    assert.equal(existsSync(`${context.paths.db}-wal`), false, 'the database was opened');
  });
});

test('a busy database spools inside the capture budget', async () => {
  await withCapture(async (context) => {
    const holder = new DatabaseSync(context.paths.db, { timeout: 2_000 });
    holder.exec('PRAGMA journal_mode = WAL');
    holder.exec('BEGIN IMMEDIATE');
    try {
      const started = performance.now();
      const outcome = await context.capture(
        'claude',
        'PostToolUse',
        claudePostToolUse(context.repo, 'notes'),
        { deps: { elapsedMs: () => performance.now() - started } },
      );
      const elapsed = performance.now() - started;

      assert.equal(outcome.outcome, 'spooled');
      assert.equal(listSpool(context.paths).length, 1);
      assert.ok(elapsed < CAPTURE_DEADLINE_MS, `the hook took ${elapsed.toFixed(1)} ms`);
    } finally {
      holder.exec('ROLLBACK');
      holder.close();
    }
  });
});

test('an unwritable spool reports a count to stderr and still succeeds', async (t) => {
  if (process.getuid?.() === 0) {
    t.skip('the root user writes into a directory without write permission');
    return;
  }
  await withCapture(
    async (context) => {
      chmodSync(context.paths.spool, 0o500);
      const written: string[] = [];
      const original = process.stderr.write.bind(process.stderr);
      process.stderr.write = ((chunk: string) => {
        written.push(String(chunk));
        return true;
      }) as typeof process.stderr.write;
      try {
        const outcome = await context.capture(
          'claude',
          'PostToolUse',
          claudePostToolUse(context.repo, 'notes'),
        );
        assert.equal(outcome.outcome, 'dropped');
      } finally {
        process.stderr.write = original;
        chmodSync(context.paths.spool, 0o700);
      }
      assert.match(written.join(''), /1 event/);
    },
    { database: false },
  );
});

test('the paused marker stops capture before anything is written', async () => {
  await withCapture(async (context) => {
    writeFileSync(context.paths.paused, '');

    const outcome = await context.capture(
      'claude',
      'PostToolUse',
      claudePostToolUse(context.repo, 'notes'),
    );

    assert.equal(outcome.outcome, 'paused');
    assert.equal(context.all('SELECT id FROM raw_events').length, 0);
    assert.deepEqual(listSpool(context.paths), []);
    assert.equal(existsSync(context.paths.hookLog), false);
  });
});

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

test('the worker is spawned at the end of a turn only while the lease is free', async () => {
  await withCapture(async (context) => {
    const base = { session_id: 'session-spawn', cwd: context.repo, prompt_id: 'prompt-1' };

    await context.capture('claude', 'PostToolUse', claudePostToolUse(context.repo, 'notes'));
    assert.equal(context.spawned, 0, 'a tool result is not a batch trigger');

    await context.capture('claude', 'Stop', { ...base, last_assistant_message: 'done' });
    assert.equal(context.spawned, 1);

    const opened = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    opened.db
      .prepare('UPDATE worker_lease SET owner_token = ?, heartbeat_at = ? WHERE id = 1')
      .run('another-worker', Date.now());
    opened.db.close();

    await context.capture('claude', 'Stop', {
      ...base,
      prompt_id: 'prompt-2',
      last_assistant_message: 'done again',
    });
    assert.equal(context.spawned, 1, 'a held lease means no second worker');
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

test('a worker spawn that throws leaves the stored rows alone', async () => {
  await withCapture(async (context) => {
    const base = { session_id: 'session-spawn-throws', cwd: context.repo, prompt_id: 'prompt-1' };
    const outcome = await context.capture(
      'claude',
      'Stop',
      { ...base, last_assistant_message: 'done' },
      {
        deps: {
          spawnWorker: () => {
            throw new Error('spawn failed');
          },
        },
      },
    );

    assert.equal(outcome.outcome, 'stored', 'the rows are stored, whatever the spawn did');
    assert.deepEqual(listSpool(context.paths), [], 'stored rows are never spooled again');
    const kinds = context.all('SELECT kind FROM raw_events ORDER BY kind').map((row) => row.kind);
    assert.deepEqual(kinds, ['last_assistant_message', 'turn_end']);
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
