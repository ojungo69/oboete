import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { configSchema } from '../../src/config.js';
import {
  CAPTURE_DEADLINE_MS,
  INJECTION_DEADLINE_MS,
  hookDeadlineMs,
  runHook,
} from '../../src/capture.js';
import { openDatabase } from '../../src/db/open.js';
import type { AgentName, NormalizedEvent } from '../../src/events.js';
import {
  injectForHook,
  runInject,
  type HookContext,
} from '../../src/injection/inject.js';
import { whyReport } from '../../src/injection/ledger.js';
import { ensureDirectories, oboetePaths, type OboetePaths } from '../../src/paths.js';
import { detectSync } from '../../src/privacy/detect.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { resolveRepoIdentity, type RepoIdentity } from '../../src/repo-identity.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_800_000_000_000;

type Fixture = {
  db: DatabaseSync;
  paths: OboetePaths;
  repo: string;
  identity: RepoIdentity;
};

async function withFixture(run: (fixture: Fixture) => Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const repo = join(home, 'workspace');
    mkdirSync(repo, { recursive: true });
    spawnSync('git', ['-C', repo, 'init', '--quiet']);
    const identity = resolveRepoIdentity(repo);
    const { db } = openDatabase({ path: paths.db, timeoutMs: 2_000 });
    db.prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
       VALUES (?, ?, ?, ?, ?, ?)`,
    ).run(identity.id, identity.identityKind, identity.normalizedIdentity, identity.root, NOW, NOW);
    try {
      await run({ db, paths, repo, identity });
    } finally {
      db.close();
    }
  });
}

function insertSession(
  fixture: Fixture,
  input: {
    id: string;
    agent: AgentName;
    nativeId?: string;
    conversationId?: string;
    status?: 'active' | 'ended';
    endedAt?: number;
    summaryState?: 'pending' | 'done' | 'no_content';
    summaryId?: string;
    epoch?: number;
    model?: string;
  },
): void {
  fixture.db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
       started_at, ended_at, status, turn_count, latest_summary_memory_id, context_epoch, summary_state)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?)`,
  ).run(
    input.id,
    fixture.identity.id,
    input.agent,
    input.nativeId ?? `native-${input.id}`,
    input.conversationId ?? input.id,
    input.model ?? null,
    NOW - 10_000,
    input.endedAt ?? null,
    input.status ?? 'active',
    input.summaryId ?? null,
    input.epoch ?? 0,
    input.summaryState ?? null,
  );
}

function insertMemory(
  fixture: Fixture,
  input: { id: string; title: string; body: string; type?: string; pinned?: boolean },
): void {
  fixture.db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, pinned_at, pin_order, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'eligible', 'unreviewed', ?, ?, ?)`,
  ).run(
    input.id,
    fixture.identity.id,
    input.type ?? 'discovery',
    input.title,
    input.body,
    cjkBigrams(`${input.title} ${input.body}`),
    `material-${input.id}`,
    `content-${input.id}`,
    input.pinned ? NOW - 1_000 : null,
    input.pinned ? 1 : null,
    NOW - 5_000,
  );
}

function seedSummary(fixture: Fixture): void {
  insertMemory(fixture, {
    id: 'm-summary',
    type: 'session_summary',
    title: 'Previous session',
    body: 'The previous database migration completed.',
  });
  insertSession(fixture, {
    id: 's-previous',
    agent: 'claude',
    status: 'ended',
    endedAt: NOW - 2_000,
    summaryState: 'done',
    summaryId: 'm-summary',
  });
}

function modelFor(agent: AgentName): string {
  if (agent === 'claude') return 'claude-opus-5[1m]';
  if (agent === 'codex') return 'gpt-5.6-sol';
  if (agent === 'grok') return 'grok-4.6-build';
  return 'gpt-5.6-luna';
}

function eventFor(
  agent: AgentName,
  eventName: string,
  nativeSessionId: string,
  cwd: string,
  text = 'SQLite busy timeout',
): NormalizedEvent {
  const envelope = {
    agent,
    native_session_id: nativeSessionId,
    cwd,
    captured_at: NOW,
    model: modelFor(agent),
  } as const;
  if (eventName === 'SessionStart') {
    return { ...envelope, kind: 'session_start', source: 'startup' };
  }
  if (eventName === 'UserPromptSubmit') {
    return { ...envelope, kind: 'prompt', text, input_source: 'user' };
  }
  if (eventName === 'PreToolUse') {
    return {
      ...envelope,
      kind: 'tool_call',
      tool_call_id: text,
      tool_name_native: 'read_file',
      tool_name: 'read',
      input: { paths: [] },
    };
  }
  if (eventName === 'PostToolUse') {
    return { ...envelope, kind: 'tool_result', tool_call_id: text, output: '', is_error: false };
  }
  if (eventName === 'PostToolUseFailure' || eventName === 'PermissionDenied') {
    return { ...envelope, kind: 'tool_failure', tool_call_id: text, error: 'failed' };
  }
  return { ...envelope, kind: 'turn_end', turn_index: 0, reason: 'end_turn' };
}

function context(
  fixture: Fixture,
  input: {
    agent: AgentName;
    eventName: string;
    sessionId: string;
    conversationId?: string;
    event?: NormalizedEvent;
    sessionCreated?: boolean;
    epoch?: number;
    remainingBudget?: () => number;
    sleep?: (milliseconds: number) => void;
    db?: DatabaseSync;
  },
): HookContext {
  const nativeId = `native-${input.sessionId}`;
  return {
    agent: input.agent,
    eventName: input.eventName,
    event:
      input.event ?? eventFor(input.agent, input.eventName, nativeId, fixture.repo),
    sessionId: input.sessionId,
    conversationId: input.conversationId ?? input.sessionId,
    turnId: null,
    epoch: input.epoch ?? 0,
    repoId: fixture.identity.id,
    repoIdentityDisplay: fixture.identity.normalizedIdentity,
    repoRoot: fixture.identity.root,
    model: modelFor(input.agent),
    cwd: fixture.repo,
    config: configSchema.parse({}),
    paths: fixture.paths,
    db: input.db === undefined ? fixture.db : input.db,
    sessionCreated: input.sessionCreated ?? false,
    secretPaths: [],
    remainingBudget: input.remainingBudget ?? (() => 1_300),
    sleep: input.sleep,
  };
}

function envelope(text: string): { hookSpecificOutput: { hookEventName: string; additionalContext: string } } {
  return JSON.parse(text) as {
    hookSpecificOutput: { hookEventName: string; additionalContext: string };
  };
}

async function stdoutOf(run: () => Promise<number>): Promise<string> {
  const chunks: string[] = [];
  const original = process.stdout.write.bind(process.stdout);
  process.stdout.write = ((chunk: string) => {
    chunks.push(String(chunk));
    return true;
  }) as typeof process.stdout.write;
  try {
    assert.equal(await run(), 0);
  } finally {
    process.stdout.write = original;
  }
  return chunks.join('');
}

test('Claude injects plain session-start and prompt packs and confirms their items', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, { id: 's-claude', agent: 'claude' });
    insertMemory(fixture, {
      id: 'm-prompt',
      title: 'SQLite busy timeout',
      body: 'The hook database waits for a bounded timeout.',
    });

    const start = await injectForHook(
      context(fixture, { agent: 'claude', eventName: 'SessionStart', sessionId: 's-claude' }),
    );
    assert.ok(start.startsWith('oboete memory context'));
    assert.ok(start.endsWith('end of oboete memory context'));

    const prompt = await injectForHook(
      context(fixture, {
        agent: 'claude',
        eventName: 'UserPromptSubmit',
        sessionId: 's-claude',
      }),
    );
    assert.ok(prompt.includes('SQLite busy timeout'));
    assert.ok(whyReport(fixture.db, 's-claude').flatMap((row) => row.items).every((item) => item.decision === 'included'));
    assert.equal(
      fixture.db.prepare('SELECT last_injected_at FROM memories WHERE id = ?').get('m-prompt')?.last_injected_at,
      NOW,
    );
  });
});

test('transcript replay sources never rebuild a Claude or Codex start pack', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    for (const [agent, source] of [
      ['claude', 'resume'],
      ['claude', 'fork'],
      ['codex', 'resume'],
      ['grok', 'resume'],
    ] as const) {
      const sessionId = `s-${agent}-${source}`;
      insertSession(fixture, { id: sessionId, agent });
      const event = {
        ...eventFor(agent, 'SessionStart', `native-${sessionId}`, fixture.repo),
        source,
      } as NormalizedEvent;
      assert.equal(
        await injectForHook(
          context(fixture, { agent, eventName: 'SessionStart', sessionId, event }),
        ),
        '',
      );
      assert.deepEqual(whyReport(fixture.db, sessionId), []);
    }
  });
});

test('capture uses the raised deadline only for hooks that can deliver a pack', () => {
  assert.equal(INJECTION_DEADLINE_MS, 1_300);
  for (const [agent, eventName] of [
    ['claude', 'SessionStart'],
    ['claude', 'UserPromptSubmit'],
    ['codex', 'SessionStart'],
    ['codex', 'UserPromptSubmit'],
    ['grok', 'SessionStart'],
    ['grok', 'UserPromptSubmit'],
    ['grok', 'PreToolUse'],
    ['grok', 'PostToolUse'],
  ] as const) {
    assert.equal(hookDeadlineMs(agent, eventName), INJECTION_DEADLINE_MS);
  }
  assert.equal(hookDeadlineMs('claude', 'PostToolUse'), CAPTURE_DEADLINE_MS);
  assert.equal(hookDeadlineMs('grok', 'Stop'), CAPTURE_DEADLINE_MS);
});

test('Codex uses JSON transport and A18 joins the session-start and prompt packs', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, { id: 's-codex', agent: 'codex' });
    insertMemory(fixture, {
      id: 'm-codex',
      title: 'SQLite busy timeout',
      body: 'The timeout is bounded.',
    });

    const started = envelope(
      await injectForHook(
        context(fixture, { agent: 'codex', eventName: 'SessionStart', sessionId: 's-codex' }),
      ),
    );
    assert.equal(started.hookSpecificOutput.hookEventName, 'SessionStart');
    assert.ok(started.hookSpecificOutput.additionalContext.startsWith('oboete memory context'));
    assert.equal(started.hookSpecificOutput.additionalContext.startsWith('{'), false);

    insertSession(fixture, { id: 's-new', agent: 'codex' });
    const firstPrompt = envelope(
      await injectForHook(
        context(fixture, {
          agent: 'codex',
          eventName: 'UserPromptSubmit',
          sessionId: 's-new',
          sessionCreated: true,
        }),
      ),
    );
    assert.equal(firstPrompt.hookSpecificOutput.hookEventName, 'UserPromptSubmit');
    assert.ok(firstPrompt.hookSpecificOutput.additionalContext.startsWith('oboete memory context'));
    assert.ok(firstPrompt.hookSpecificOutput.additionalContext.includes('previous database migration'));
    assert.ok(firstPrompt.hookSpecificOutput.additionalContext.includes('SQLite busy timeout'));
  });
});

test('Codex carries the new epoch session-start pack on the first prompt after a manual /compact (A21)', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, { id: 's-codex-compact', agent: 'codex' });
    const started = envelope(
      await injectForHook(
        context(fixture, { agent: 'codex', eventName: 'SessionStart', sessionId: 's-codex-compact' }),
      ),
    );
    assert.ok(started.hookSpecificOutput.additionalContext.includes('previous database migration'));
    const startPacks = (): { epoch: number; channel: string; state: string }[] =>
      fixture.db
        .prepare(
          `SELECT context_epoch, channel, state FROM injections
           WHERE conversation_id = ? AND kind = 'session_start' ORDER BY context_epoch`,
        )
        .all('s-codex-compact')
        .map((row) => ({
          epoch: Number(row.context_epoch),
          channel: String(row.channel),
          state: String(row.state),
        }));

    // Epoch 0 has its pack, so a prompt of that epoch adds none.
    await injectForHook(
      context(fixture, { agent: 'codex', eventName: 'UserPromptSubmit', sessionId: 's-codex-compact' }),
    );
    assert.equal(startPacks().length, 1);

    // PostCompact moved the conversation to epoch 1 and the SessionStart(compact) that follows it
    // never arrived (spooled, or the session was cut off): the next prompt carries the epoch's pack.
    fixture.db.prepare('UPDATE sessions SET context_epoch = 1 WHERE id = ?').run('s-codex-compact');
    const afterCompact = envelope(
      await injectForHook(
        context(fixture, {
          agent: 'codex',
          eventName: 'UserPromptSubmit',
          sessionId: 's-codex-compact',
          epoch: 1,
        }),
      ),
    );
    assert.equal(afterCompact.hookSpecificOutput.hookEventName, 'UserPromptSubmit');
    assert.ok(afterCompact.hookSpecificOutput.additionalContext.includes('previous database migration'));
    assert.deepEqual(startPacks(), [
      { epoch: 0, channel: 'codex:SessionStart', state: 'emitted' },
      { epoch: 1, channel: 'codex:UserPromptSubmit', state: 'emitted' },
    ]);

    // A second prompt of the new epoch does not repeat it (FR-026).
    await injectForHook(
      context(fixture, {
        agent: 'codex',
        eventName: 'UserPromptSubmit',
        sessionId: 's-codex-compact',
        epoch: 1,
      }),
    );
    assert.equal(startPacks().length, 2);
  });
});

test('Grok defers, attempts, confirms, and closes a no-tool turn', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, { id: 's-grok', agent: 'grok' });
    insertSession(fixture, { id: 's-grok-fork', agent: 'grok' });

    const fork = {
      ...eventFor('grok', 'SessionStart', 'native-s-grok-fork', fixture.repo),
      source: 'resume',
    } as NormalizedEvent;
    assert.equal(
      await injectForHook(
        context(fixture, {
          agent: 'grok',
          eventName: 'SessionStart',
          sessionId: 's-grok-fork',
          sessionCreated: true,
          event: fork,
        }),
      ),
      '',
    );
    assert.equal(whyReport(fixture.db, 's-grok-fork')[0]?.state, 'pending');

    const startContext = context(fixture, {
      agent: 'grok',
      eventName: 'SessionStart',
      sessionId: 's-grok',
    });
    assert.equal(await injectForHook(startContext), '');
    assert.equal(whyReport(fixture.db, 's-grok')[0]?.state, 'pending');
    assert.equal(whyReport(fixture.db, 's-grok')[0]?.deferred, true);

    const pre = envelope(
      await injectForHook(
        context(fixture, {
          agent: 'grok',
          eventName: 'PreToolUse',
          sessionId: 's-grok',
          event: eventFor('grok', 'PreToolUse', 'native-s-grok', fixture.repo, 'call-1'),
        }),
      ),
    );
    assert.equal(pre.hookSpecificOutput.hookEventName, 'PreToolUse');
    assert.ok(pre.hookSpecificOutput.additionalContext.startsWith('oboete memory context'));
    assert.equal(whyReport(fixture.db, 's-grok')[0]?.attempts.length, 1);

    assert.equal(
      await injectForHook(
        context(fixture, {
          agent: 'grok',
          eventName: 'PostToolUse',
          sessionId: 's-grok',
          event: eventFor('grok', 'PostToolUse', 'native-s-grok', fixture.repo, 'call-1'),
        }),
      ),
      '',
    );
    assert.equal(whyReport(fixture.db, 's-grok')[0]?.state, 'emitted');
    assert.equal(
      fixture.db.prepare('SELECT last_injected_at FROM memories WHERE id = ?').get('m-summary')
        ?.last_injected_at,
      NOW,
    );
    assert.equal(
      await injectForHook(
        context(fixture, {
          agent: 'grok',
          eventName: 'PreToolUse',
          sessionId: 's-grok',
          event: eventFor('grok', 'PreToolUse', 'native-s-grok', fixture.repo, 'call-2'),
        }),
      ),
      '',
    );

    insertSession(fixture, { id: 's-grok-empty', agent: 'grok' });
    insertMemory(fixture, {
      id: 'm-no-tool',
      title: 'No tool call note',
      body: 'This note remains available.',
    });
    await injectForHook(
      context(fixture, {
        agent: 'grok',
        eventName: 'UserPromptSubmit',
        sessionId: 's-grok-empty',
        event: eventFor(
          'grok',
          'UserPromptSubmit',
          'native-s-grok-empty',
          fixture.repo,
          'No tool call note',
        ),
      }),
    );
    await injectForHook(
      context(fixture, { agent: 'grok', eventName: 'Stop', sessionId: 's-grok-empty' }),
    );
    assert.equal(whyReport(fixture.db, 's-grok-empty')[0]?.degradedReason, 'no_tool_call');
    assert.equal(whyReport(fixture.db, 's-grok-empty')[0]?.deferred, true);
  });
});

test('Grok retries a denied attempt and confirms a failed execution', async () => {
  await withFixture(async (fixture) => {
    insertSession(fixture, { id: 's-grok-retry', agent: 'grok' });
    insertMemory(fixture, {
      id: 'm-grok-retry',
      title: 'Deferred retry note',
      body: 'Denied calls leave the note pending.',
    });
    const hook = (eventName: string, callId: string) =>
      context(fixture, {
        agent: 'grok',
        eventName,
        sessionId: 's-grok-retry',
        event: eventFor('grok', eventName, 'native-s-grok-retry', fixture.repo, callId),
      });

    await injectForHook(
      context(fixture, {
        agent: 'grok',
        eventName: 'UserPromptSubmit',
        sessionId: 's-grok-retry',
        event: eventFor(
          'grok',
          'UserPromptSubmit',
          'native-s-grok-retry',
          fixture.repo,
          'Deferred retry note',
        ),
      }),
    );
    assert.notEqual(await injectForHook(hook('PreToolUse', 'call-denied')), '');
    assert.equal(await injectForHook(hook('PermissionDenied', 'call-denied')), '');
    assert.notEqual(await injectForHook(hook('PreToolUse', 'call-failed')), '');
    assert.equal(await injectForHook(hook('PostToolUseFailure', 'call-failed')), '');

    const report = whyReport(fixture.db, 's-grok-retry')[0];
    assert.equal(report?.state, 'emitted');
    assert.deepEqual(
      report?.attempts.map((attempt) => [attempt.execution, attempt.delivery]),
      [
        ['denied', 'dropped'],
        ['failed', 'delivered'],
      ],
    );
  });
});

test('Pi start and prompt run through the strict inject child', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, {
      id: 's-pi-start',
      agent: 'pi',
      nativeId: 'pi-start',
      model: 'gpt-5.6-luna',
    });
    insertMemory(fixture, {
      id: 'm-pi',
      title: 'SQLite busy timeout',
      body: 'Pi can retrieve the same note.',
    });
    const runtime = (body: Record<string, unknown>) => ({
      readStdin: () => JSON.stringify(body),
      now: () => NOW,
      elapsedMs: () => 0,
      sleep: () => {},
    });

    const start = await stdoutOf(() =>
      runInject(
        ['--agent', 'pi', '--kind', 'start'],
        runtime({ cwd: fixture.repo, session_id: 'pi-start', model: 'gpt-5.6-luna' }),
      ),
    );
    assert.ok(start.startsWith('oboete memory context'));

    const prompt = await stdoutOf(() =>
      runInject(
        ['--agent', 'pi', '--kind', 'prompt'],
        runtime({
          cwd: fixture.repo,
          session_id: 'pi-prompt',
          prompt: 'SQLite busy timeout',
          model: 'gpt-5.6-luna',
        }),
      ),
    );
    assert.ok(prompt.includes('SQLite busy timeout'));
    const created = fixture.db
      .prepare("SELECT id, conversation_id FROM sessions WHERE agent = 'pi' AND native_session_id = ?")
      .get('pi-prompt');
    assert.notEqual(created, undefined);
    assert.equal(created?.conversation_id, created?.id);
    assert.equal(
      fixture.db
        .prepare('SELECT session_id FROM injections WHERE session_id = ? LIMIT 1')
        .get(created?.id as string)?.session_id,
      created?.id,
      'the capture child will reuse the same persisted root',
    );

    const invalid = await stdoutOf(() =>
      runInject(
        ['--agent', 'pi', '--kind', 'prompt'],
        runtime({ cwd: fixture.repo, session_id: 'pi-prompt', prompt: 'SQLite', extra: true }),
      ),
    );
    assert.equal(invalid, '', 'the stdin schema is strict');
  });
});

test('only session start polls a pending summary and stops after one second', async () => {
  await withFixture(async (fixture) => {
    insertSession(fixture, {
      id: 's-pending',
      agent: 'claude',
      status: 'ended',
      endedAt: NOW - 2_000,
      summaryState: 'pending',
    });
    fixture.db.prepare(
      `INSERT INTO raw_events (id, repo_id, session_id, agent, kind, content, payload_json,
         sensitivity, classification_state, captured_at, expires_at)
       VALUES ('e-prompt', ?, 's-pending', 'claude', 'prompt', ?, '{}',
         'local_only', 'done', ?, ?)`,
    ).run(fixture.identity.id, '直近の生の活動です。', NOW - 1_000, NOW + 10_000);
    insertSession(fixture, { id: 's-wait', agent: 'claude' });

    let elapsed = 0;
    const start = await injectForHook(
      context(fixture, {
        agent: 'claude',
        eventName: 'SessionStart',
        sessionId: 's-wait',
        remainingBudget: () => 1_300 - elapsed,
        sleep: (milliseconds) => {
          elapsed += milliseconds;
        },
      }),
    );
    assert.ok(elapsed <= 1_000, `waited ${elapsed} ms`);
    assert.ok(start.includes('直近の生の活動です。'));
    assert.match(start, /summary.*not finished/i);

    insertSession(fixture, { id: 's-no-wait', agent: 'claude' });
    insertMemory(fixture, {
      id: 'm-no-wait',
      title: 'Prompt does not wait',
      body: 'Prompt retrieval runs immediately.',
    });
    let promptSleeps = 0;
    const prompt = await injectForHook(
      context(fixture, {
        agent: 'claude',
        eventName: 'UserPromptSubmit',
        sessionId: 's-no-wait',
        event: eventFor(
          'claude',
          'UserPromptSubmit',
          'native-s-no-wait',
          fixture.repo,
          'Prompt does not wait',
        ),
        sleep: () => {
          promptSleeps += 1;
        },
      }),
    );
    assert.ok(prompt.includes('Prompt does not wait'));
    assert.equal(promptSleeps, 0);
  });
});

test('a spooled injection hook prints nothing and logs index_unavailable', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    const repo = join(home, 'workspace');
    mkdirSync(repo, { recursive: true });
    spawnSync('git', ['-C', repo, 'init', '--quiet']);
    const payload = {
      session_id: 'native-spooled',
      cwd: repo,
      source: 'startup',
      model: 'claude-opus-5[1m]',
    };
    const output = await stdoutOf(() =>
      runHook(['--agent', 'claude-or-grok', '--event', 'SessionStart'], {
        deps: {
          detect: (input) => detectSync(input),
          now: () => NOW,
          elapsedMs: () => 0,
          spawnWorker: () => {},
        },
        readStdin: () => ({ text: JSON.stringify(payload), truncated: false }),
      }),
    );

    assert.equal(output, '');
    assert.equal(existsSync(paths.db), false, 'the hook path never creates the missing index');
    assert.match(readFileSync(paths.hookLog, 'utf8'), /index_unavailable/);
  });
});
