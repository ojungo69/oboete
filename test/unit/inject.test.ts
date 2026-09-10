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
} from '../../src/capture.js';
import { runHook } from '../../src/capture-command.js';
import type { AgentName, NormalizedEvent } from '../../src/events.js';
import {
  injectForHook,
  type HookContext,
} from '../../src/injection/inject.js';
import { sessionStartAttempted, whyReport } from '../../src/injection/ledger.js';
import { NOW, insertMemory, insertSession, scope, seedSummary, stdoutOf, withFixture, type Fixture } from '../helpers/inject-fixture.js';
import { detectSync } from '../../src/privacy/detect.js';
import { oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';

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
  };
}

function envelope(text: string): { hookSpecificOutput: { hookEventName: string; additionalContext: string } } {
  return JSON.parse(text) as {
    hookSpecificOutput: { hookEventName: string; additionalContext: string };
  };
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
    const items = whyReport(fixture.db, 's-claude', scope(fixture)).flatMap((row) => row.items);
    assert.ok(items.every((item) => item.decision === 'included' || item.reason === 'duplicate_in_conversation'));
    assert.equal(items.filter((item) => item.memoryId === 'm-summary' && item.decision === 'included').length, 1);
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
      assert.deepEqual(whyReport(fixture.db, sessionId, scope(fixture)), []);
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

test('a first Codex pack cannot be printed after privacy changes while building its second pack', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, { id: 'codex-two-packs', agent: 'codex' });
    insertMemory(fixture, { id: 'long-proof', title: 'SQLite busy timeout', body: 'SQLite busy timeout tuning evidence.' });
    fixture.db.prepare('UPDATE memories SET provenance_complete = 1 WHERE id = ?').run('long-proof');
    fixture.db.prepare(`INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, source_context_id)
      VALUES ('long-proof', ?, ?, ?)`).run(fixture.repo, JSON.stringify(['a'.repeat(70_000)]), `fixture-context:${fixture.identity.id}`);
    const input = context(fixture, { agent: 'codex', eventName: 'UserPromptSubmit', sessionId: 'codex-two-packs' });
    const previous = process.env.OBOETE_OPENROUTER_API_KEY;
    let changed = false;
    input.detect = async (candidate) => {
      if (fixture.db.prepare("SELECT 1 FROM injections WHERE session_id = 'codex-two-packs' AND kind = 'session_start'").get()) {
        process.env.OBOETE_OPENROUTER_API_KEY = 'previous database migration';
        changed = true;
      }
      return detectSync(candidate);
    };
    try {
      const result = await injectForHook(input);
      assert.equal(changed, true, 'the source check of the second pack reaches the async race');
      assert.doesNotMatch(result, /previous database migration/);
      assert.equal(fixture.db.prepare(`SELECT COUNT(*) AS n FROM injection_items ii JOIN injections i ON i.id = ii.injection_id
        WHERE i.session_id = 'codex-two-packs' AND ii.memory_id = 'm-summary' AND ii.decision = 'included'`).get()?.n, 0,
      'an unprinted first pack cannot leave a delivered-memory receipt');
    } finally {
      if (previous === undefined) delete process.env.OBOETE_OPENROUTER_API_KEY;
      else process.env.OBOETE_OPENROUTER_API_KEY = previous;
    }
  });
});

test('Codex cancels an unprinted first pack after a second-pack exception and can retry it', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, { id: 'codex-pair-error', agent: 'codex' });
    insertMemory(fixture, { id: 'error-proof', title: 'SQLite busy timeout', body: 'SQLite busy timeout evidence.' });
    fixture.db.prepare('UPDATE memories SET provenance_complete = 1 WHERE id = ?').run('error-proof');
    fixture.db.prepare(`INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, source_context_id)
      VALUES ('error-proof', ?, ?, ?)`).run(fixture.repo, JSON.stringify(['a'.repeat(70_000)]), `fixture-context:${fixture.identity.id}`);
    const input = context(fixture, { agent: 'codex', eventName: 'UserPromptSubmit', sessionId: 'codex-pair-error' });
    let rejected = false;
    input.detect = async () => { rejected = true; throw new Error('second_pack_failed'); };
    assert.equal(await injectForHook(input), '');
    assert.equal(rejected, true);
    const starts = () => fixture.db.prepare(`SELECT state, degraded_reason FROM injections
      WHERE session_id = 'codex-pair-error' AND kind = 'session_start' ORDER BY rowid`).all();
    assert.equal(starts()[0].state, 'omitted');
    assert.equal(starts()[0].degraded_reason, 'not_delivered');
    assert.equal(fixture.db.prepare(`SELECT COUNT(*) AS n FROM injection_items ii JOIN injections i ON i.id = ii.injection_id
      WHERE i.session_id = 'codex-pair-error' AND ii.decision IN ('planned', 'included')`).get()?.n, 0);
    input.detect = (candidate) => detectSync(candidate);
    assert.match(await injectForHook(input), /previous database migration/);
    assert.deepEqual(starts().map((row) => row.state), ['omitted', 'emitted']);
  });
});

test('ordinary empty or legacy start omissions still suppress repeated start attempts', async () => {
  await withFixture(async (fixture) => {
    insertSession(fixture, { id: 'empty-start', agent: 'codex' });
    assert.equal(await injectForHook(context(fixture, { agent: 'codex', eventName: 'SessionStart', sessionId: 'empty-start' })), '');
    assert.equal(sessionStartAttempted(fixture.db, 'empty-start', 0), true);
    fixture.db.exec("UPDATE injections SET degraded_reason = NULL WHERE session_id = 'empty-start'");
    assert.equal(sessionStartAttempted(fixture.db, 'empty-start', 0), true);
    fixture.db.exec("UPDATE injections SET degraded_reason = 'not_delivered' WHERE session_id = 'empty-start'");
    assert.equal(sessionStartAttempted(fixture.db, 'empty-start', 0), false);
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
    assert.equal(whyReport(fixture.db, 's-grok-fork', scope(fixture))[0]?.state, 'pending');

    const startContext = context(fixture, {
      agent: 'grok',
      eventName: 'SessionStart',
      sessionId: 's-grok',
    });
    assert.equal(await injectForHook(startContext), '');
    assert.equal(whyReport(fixture.db, 's-grok', scope(fixture))[0]?.state, 'pending');
    assert.equal(whyReport(fixture.db, 's-grok', scope(fixture))[0]?.deferred, true);

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
    assert.equal(whyReport(fixture.db, 's-grok', scope(fixture))[0]?.attempts.length, 1);

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
    assert.equal(whyReport(fixture.db, 's-grok', scope(fixture))[0]?.state, 'emitted');
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
    assert.equal(whyReport(fixture.db, 's-grok-empty', scope(fixture))[0]?.degradedReason, 'no_tool_call');
    assert.equal(whyReport(fixture.db, 's-grok-empty', scope(fixture))[0]?.deferred, true);
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

    const report = whyReport(fixture.db, 's-grok-retry', scope(fixture))[0];
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

test('pending work activity is immediately available at session start and prompt retrieval never waits', async () => {
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
    fixture.db.prepare('UPDATE raw_events SET work_binding_id = ? WHERE id = ?').run('fixture-binding:s-pending', 'e-prompt');
    fixture.db.prepare('UPDATE raw_events SET payload_json = ? WHERE id = ?')
      .run(JSON.stringify({ capture_root: fixture.identity.root, source_paths: [] }), 'e-prompt');
    insertSession(fixture, { id: 's-wait', agent: 'claude' });

    const start = await injectForHook(
      context(fixture, {
        agent: 'claude',
        eventName: 'SessionStart',
        sessionId: 's-wait',
      }),
    );
    assert.ok(start.includes('直近の生の活動です。'));
    assert.match(start, /selected work is still waiting to be processed/i);

    insertSession(fixture, { id: 's-no-wait', agent: 'claude' });
    insertMemory(fixture, {
      id: 'm-no-wait',
      title: 'Prompt does not wait',
      body: 'Prompt retrieval runs immediately.',
    });
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
      }),
    );
    assert.ok(prompt.includes('Prompt does not wait'));
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

test('an exhausted hook budget produces no pack or ledger entry', async () => {
  await withFixture(async (fixture) => {
    insertSession(fixture, { id: 'no-budget', agent: 'claude' });
    insertMemory(fixture, { id: 'm-budget', title: 'SQLite busy timeout', body: 'A retained note.', pinned: true });
    assert.equal(await injectForHook(context(fixture, {
      agent: 'claude', eventName: 'SessionStart', sessionId: 'no-budget', remainingBudget: () => 0,
    })), '');
    assert.equal(fixture.db.prepare('SELECT count(*) AS n FROM injections').get()?.n, 0);
    assert.equal(fixture.db.prepare('SELECT last_injected_at FROM memories WHERE id = ?').get('m-budget')?.last_injected_at, null);
  });
});

test('a Grok stop with a turn id distinguishes an undelivered tool call from no tool call', async () => {
  await withFixture(async (fixture) => {
    insertSession(fixture, { id: 's-turn', agent: 'grok' });
    fixture.db.prepare("INSERT INTO turns (id, session_id, ordinal, started_at) VALUES ('turn-current', 's-turn', 1, ?)").run(NOW);
    insertMemory(fixture, { id: 'm-turn', title: 'SQLite busy timeout', body: 'The database has one writer.' });
    const prompt = { ...context(fixture, { agent: 'grok', eventName: 'UserPromptSubmit', sessionId: 's-turn' }), turnId: 'turn-current' };
    assert.equal(await injectForHook(prompt), '');
    fixture.db.prepare(
      `INSERT INTO raw_events (id, repo_id, session_id, turn_id, agent, kind, sensitivity, captured_at)
       VALUES ('raw-tool', ?, 's-turn', 'turn-current', 'grok', 'tool_call', 'local_only', ?)`,
    ).run(fixture.identity.id, NOW);
    assert.equal(await injectForHook({
      ...context(fixture, { agent: 'grok', eventName: 'Stop', sessionId: 's-turn' }), turnId: 'turn-current',
    }), '');
    assert.deepEqual(
      { ...fixture.db.prepare('SELECT state, degraded_reason FROM injections').get() },
      { state: 'omitted', degraded_reason: 'not_delivered' },
    );
    assert.deepEqual(
      { ...fixture.db.prepare('SELECT decision, reason FROM injection_items').get() },
      { decision: 'omitted', reason: 'not_delivered' },
    );
  });
});

test('a storage error in a Grok delivery hook is contained and logged without event text', async () => {
  await withFixture(async (fixture) => {
    fixture.db.exec('DROP TABLE injections');
    const hook = context(fixture, { agent: 'grok', eventName: 'PreToolUse', sessionId: 'missing' });
    assert.equal(await injectForHook(hook), '');
    const log = readFileSync(fixture.paths.hookLog, 'utf8');
    assert.match(log, /injection failed agent=grok event=PreToolUse reason=ERR_SQLITE_ERROR/);
    assert.doesNotMatch(log, /SQLite busy timeout|no such table/);
  });
});
