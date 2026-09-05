import assert from 'node:assert/strict';
import { test } from 'node:test';

import { observerOutputSchema } from '../../src/observer/contract.js';
import {
  fallbackObserve,
  firstLine,
  firstParagraph,
  firstSentence,
  type FallbackEvent,
} from '../../src/observer/fallback.js';

const REPO_ID = 'repo-fallback';

function event(
  input: Omit<FallbackEvent, 'sensitivity' | 'classification_state'>,
  agent: string,
): FallbackEvent {
  return {
    ...input,
    sensitivity: 'local_only',
    classification_state: 'done',
    agent,
  } as FallbackEvent;
}

test('fallback emits deterministic agent-neutral records for two turns', () => {
  const longPath = `/workspace/${'deep/'.repeat(38)}record.ts`;
  const commit = '0123456789abcdef0123456789abcdef01234567';
  const events = [
    event({ id: 'prompt-0', kind: 'prompt', turn_index: 0, text: '変更してください。' }, 'claude'),
    event(
      {
        id: 'edit-call',
        kind: 'tool_call',
        turn_index: 0,
        tool_call_id: 'native-edit',
        tool_name: 'edit',
        input: { paths: [longPath], lines_added: 4, lines_removed: 2 },
      },
      'codex',
    ),
    event(
      {
        id: 'edit-result',
        kind: 'tool_result',
        turn_index: 0,
        tool_call_id: 'native-edit',
        output: 'updated',
        is_error: false,
      },
      'grok',
    ),
    event(
      {
        id: 'git-call',
        kind: 'tool_call',
        turn_index: 0,
        tool_call_id: 'native-git',
        tool_name: 'bash',
        input: { paths: [], command: 'git commit -m save' },
      },
      'pi',
    ),
    event(
      {
        id: 'git-result',
        kind: 'tool_result',
        turn_index: 0,
        tool_call_id: 'native-git',
        output: `[main ${commit}] saved`,
        is_error: false,
      },
      'claude',
    ),
    event(
      {
        id: 'failed-call',
        kind: 'tool_call',
        turn_index: 0,
        tool_call_id: 'native-failed',
        tool_name: 'edit',
        input: { paths: ['src/retry.ts'] },
      },
      'codex',
    ),
    event(
      {
        id: 'failure',
        kind: 'tool_failure',
        turn_index: 0,
        tool_call_id: 'native-failed',
        tool_name: 'edit',
        error: 'patch rejected\nextra details',
      },
      'grok',
    ),
    event(
      {
        id: 'retry-call',
        kind: 'tool_call',
        turn_index: 0,
        tool_call_id: 'native-retry',
        tool_name: 'edit',
        input: { paths: ['src/retry.ts'], text: 'replacement' },
      },
      'pi',
    ),
    event(
      {
        id: 'retry-result',
        kind: 'tool_result',
        turn_index: 0,
        tool_call_id: 'native-retry',
        output: 'ok',
        is_error: false,
      },
      'claude',
    ),
    {
      ...event(
        {
          id: 'partial-result',
          kind: 'tool_result',
          turn_index: 0,
          tool_call_id: 'native-git',
          output: 'codex hidden-output',
        },
        'codex',
      ),
      classification_state: 'partial' as const,
    },
    event({ id: 'prompt-1', kind: 'prompt', turn_index: 1, text: '確認してください。' }, 'grok'),
    event(
      {
        id: 'read-call',
        kind: 'tool_call',
        turn_index: 1,
        tool_call_id: 'native-read',
        tool_name: 'read',
        input: { paths: ['src/missing.ts'] },
      },
      'pi',
    ),
    event(
      {
        id: 'read-failure',
        kind: 'tool_failure',
        turn_index: 1,
        tool_call_id: 'native-read',
        tool_name: 'read',
        error: 'missing file\nENOENT',
      },
      'claude',
    ),
    event(
      {
        id: 'assistant',
        kind: 'last_assistant_message',
        turn_index: 1,
        text: '保存形式はJSONです。次の文も同じ段落です。\n\n後続の段落です。',
      },
      'codex',
    ),
  ];

  const result = fallbackObserve({ repoId: REPO_ID, events, nearby: [] });
  assert.deepEqual(result.observations.map((item) => item.type), [
    'decision',
    'change',
    'bugfix',
    'discovery',
  ]);

  const [decision, change, bugfix, discovery] = result.observations;
  assert.equal(decision?.title, '保存形式はJSONです。');
  assert.equal(decision?.body, '保存形式はJSONです。次の文も同じ段落です。');
  assert.deepEqual(decision?.concepts, ['why-it-exists']);

  assert.equal(change?.title, `${longPath}, src/retry.ts`.slice(0, 120));
  assert.deepEqual(change?.body.split('\n'), [
    `edit …${longPath.slice(-60)} (+4/-2)`,
    'bash  (+0/-0)',
    'edit src/retry.ts (+0/-0)',
    'edit src/retry.ts (+0/-0)',
  ]);
  assert.deepEqual(change?.citations.files_modified, [longPath, 'src/retry.ts']);
  assert.deepEqual(change?.citations.commits, [commit]);
  assert.deepEqual(change?.concepts, ['what-changed']);

  assert.equal(bugfix?.title, 'edit: patch rejected');
  assert.equal(bugfix?.body, 'patch rejected\nsrc/retry.ts');
  assert.deepEqual(bugfix?.citations.files_modified, ['src/retry.ts']);
  assert.deepEqual(bugfix?.source_event_ids, ['failure', 'retry-call', 'retry-result']);
  assert.deepEqual(bugfix?.concepts, ['problem-solution']);

  assert.equal(discovery?.title, 'read: missing file');
  assert.equal(discovery?.body, 'missing file');
  assert.deepEqual(discovery?.citations.files_read, ['src/missing.ts']);
  assert.deepEqual(discovery?.source_event_ids, ['read-call', 'read-failure']);
  assert.deepEqual(discovery?.concepts, ['gotcha']);

  const rendered = JSON.stringify(result.observations);
  assert.equal(rendered.includes('hidden-output'), false);
  assert.doesNotMatch(rendered, /\b(?:claude|codex|grok|pi)\b/iu);
  const inputIds = new Set(events.map((item) => item.id));
  for (const observation of result.observations) {
    assert.ok(observation.source_event_ids.every((id) => inputIds.has(id)));
  }
  assert.deepEqual(result.suppressed, []);
  observerOutputSchema.parse({ observations: result.observations });
});

test('text boundary helpers preserve the selected input text', () => {
  assert.equal(firstLine('first\r\nsecond'), 'first');
  assert.equal(firstSentence('最初です。次です。'), '最初です。');
  assert.equal(firstSentence('first line\nsecond line'), 'first line');
  assert.equal(firstParagraph('line one\nline two\n\nnext'), 'line one\nline two');
});

test('a fallback decision derives its title and body from trimmed text', () => {
  const result = fallbackObserve({
    repoId: REPO_ID,
    events: [
      event(
        {
          id: 'leading-whitespace-decision',
          kind: 'last_assistant_message',
          turn_index: 0,
          text: '\n \t Keep the stable row id. The FTS lookup depends on it.\n\nTrailing notes.',
        },
        'codex',
      ),
    ],
    nearby: [],
  });

  assert.equal(result.observations[0]?.title, 'Keep the stable row id.');
  assert.equal(
    result.observations[0]?.body,
    'Keep the stable row id. The FTS lookup depends on it.',
  );
  observerOutputSchema.parse({ observations: result.observations });
});

test('a 60-call change keeps 40 record lines and reports 20 omitted calls', () => {
  const events = Array.from({ length: 60 }, (_, index) =>
    event(
      {
        id: `call-${index}`,
        kind: 'tool_call',
        turn_index: 0,
        tool_call_id: `native-${index}`,
        tool_name: 'edit',
        input: { paths: [`src/f${index}.ts`] },
      },
      'claude',
    ),
  );
  const result = fallbackObserve({ repoId: REPO_ID, events, nearby: [] });
  const lines = result.observations[0]?.body.split('\n') ?? [];
  assert.equal(result.observations.length, 1);
  assert.equal(lines.slice(0, -1).length, 40);
  assert.equal(lines.at(-1), '... (+20 omitted)');
  assert.equal(result.observations[0]?.source_event_ids.length, 50);
  assert.equal(result.observations[0]?.citations.files_modified.length, 20);
});

test('the 20-observation budget keeps decisions first and the earliest change turns', () => {
  const events: FallbackEvent[] = [
    event(
      {
        id: 'decision',
        kind: 'compaction_summary',
        turn_index: 24,
        text: '方針を維持します。',
      },
      'grok',
    ),
  ];
  for (let turn = 0; turn < 25; turn += 1) {
    events.push(
      event(
        {
          id: `change-${turn}`,
          kind: 'tool_call',
          turn_index: turn,
          tool_call_id: `native-change-${turn}`,
          tool_name: 'write',
          input: { paths: [`src/${turn}.ts`] },
        },
        'pi',
      ),
    );
  }

  const result = fallbackObserve({ repoId: REPO_ID, events, nearby: [] });
  assert.equal(result.observations.length, 20);
  assert.equal(result.observations[0]?.type, 'decision');
  assert.deepEqual(
    result.observations.slice(1).map((item) => item.title),
    Array.from({ length: 19 }, (_, turn) => `src/${turn}.ts`),
  );
});

test('a matching tombstone suppresses content while an active match becomes noop', () => {
  const events = [
    event(
      {
        id: 'same-decision',
        kind: 'last_assistant_message',
        turn_index: 0,
        text: '保持します。',
      },
      'codex',
    ),
  ];
  const contentHash = 'd2bffcd74b53a980a192e6c89a2e2a58cdd3537f650bce7f74a6eb2b4ffaff9f';

  const tombstoned = fallbackObserve({
    repoId: REPO_ID,
    events,
    nearby: [{ id: 'deleted-memory', content_hash: contentHash, deleted: true }],
  });
  assert.deepEqual(tombstoned.observations, []);
  assert.deepEqual(tombstoned.suppressed, [
    { title: '保持します。', content_hash: contentHash, target: 'deleted-memory' },
  ]);

  const active = fallbackObserve({
    repoId: REPO_ID,
    events,
    nearby: [{ id: 'active-memory', content_hash: contentHash, deleted: false }],
  });
  assert.deepEqual(active.suppressed, []);
  assert.deepEqual(active.observations[0]?.classification, {
    decision: 'noop',
    target: 'active-memory',
    reason: 'rule:decision',
  });
  observerOutputSchema.parse({ observations: active.observations });
});

test('a same-tool retry without call ids counts as a call without a failure', () => {
  const events = [
    event(
      { id: 'first-call', kind: 'tool_call', turn_index: 0, tool_name: 'read', input: { paths: ['a'] } },
      'claude',
    ),
    event(
      { id: 'first-failure', kind: 'tool_failure', turn_index: 0, tool_name: 'read', error: 'failed' },
      'codex',
    ),
    event(
      { id: 'retry-without-id', kind: 'tool_call', turn_index: 0, tool_name: 'read', input: { paths: ['b'] } },
      'grok',
    ),
  ];
  const result = fallbackObserve({ repoId: REPO_ID, events, nearby: [] });
  assert.deepEqual(result.observations.map((item) => item.type), ['bugfix']);
  assert.deepEqual(result.observations[0]?.source_event_ids, ['first-failure', 'retry-without-id']);
});
