import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import {
  AGENTS,
  EVENT_KINDS,
  MAX_TEXT,
  SESSION_START_SOURCES,
  TOOL_NAMES,
  contentHash,
  conversationPolicy,
  envelopeSchema,
  eventContentHash,
  eventId,
  eventIdKey,
  eventSchema,
  isSummarizable,
  normalizeToolName,
} from '../../src/events.js';
import type { AgentName, EventKind, NormalizedEvent, SessionStartSource } from '../../src/events.js';

// The test hashes the documented key itself instead of calling the module, so a change of the key
// shape cannot hide behind the module's own hashing.
const sha256 = (text: string): string => createHash('sha256').update(text, 'utf8').digest('hex');

const envelope = {
  agent: 'claude',
  native_session_id: 's-1',
  cwd: '/home/dev/repo',
  captured_at: 1_757_000_000_000,
};

const toolInput = {
  paths: ['/home/dev/repo/src/a.ts'],
  text: 'old -> new',
  lines_added: 2,
  lines_removed: 1,
};

const samples: Record<EventKind, Record<string, unknown>> = {
  session_start: { ...envelope, kind: 'session_start', source: 'startup' },
  prompt: {
    ...envelope,
    kind: 'prompt',
    text: 'Fix the failing test.',
    input_source: 'user',
    prompt_id: 'p-1',
  },
  tool_call: {
    ...envelope,
    kind: 'tool_call',
    tool_call_id: 'toolu_1',
    tool_name_native: 'Edit',
    tool_name: 'edit',
    input: toolInput,
    prompt_id: 'p-1',
  },
  tool_result: {
    ...envelope,
    kind: 'tool_result',
    tool_call_id: 'toolu_1',
    output: 'The file was updated.',
    is_error: false,
    prompt_id: 'p-1',
  },
  tool_failure: {
    ...envelope,
    kind: 'tool_failure',
    tool_call_id: 'toolu_2',
    error: 'Exit code 3',
    prompt_id: 'p-1',
  },
  turn_end: { ...envelope, kind: 'turn_end', turn_index: 1, reason: 'end_turn', prompt_id: 'p-1' },
  session_end: { ...envelope, kind: 'session_end', reason: 'shutdown' },
  compaction_summary: {
    ...envelope,
    kind: 'compaction_summary',
    text: 'The session was compacted.',
    compaction_key: '2026-09-03T16:01:11Z',
  },
  last_assistant_message: {
    ...envelope,
    kind: 'last_assistant_message',
    text: 'Done.',
    prompt_id: 'p-1',
  },
  probe: { ...envelope, kind: 'probe', marker: 'oboete-probe' },
};

const parse = (event: Record<string, unknown>): NormalizedEvent => eventSchema.parse(event);

test('the schema accepts one event of every kind', () => {
  assert.deepEqual(Object.keys(samples).sort(), [...EVENT_KINDS].sort());
  for (const kind of EVENT_KINDS) {
    assert.equal(parse(samples[kind]).kind, kind);
  }
});

test('the schema rejects an unknown kind', () => {
  assert.equal(eventSchema.safeParse({ ...envelope, kind: 'thinking', text: 'x' }).success, false);
});

test('the schema rejects an agent outside the five known names', () => {
  assert.deepEqual([...AGENTS], ['claude', 'codex', 'grok', 'pi', 'unknown']);
  assert.equal(eventSchema.safeParse({ ...samples.probe, agent: 'cursor' }).success, false);
  for (const agent of AGENTS) {
    assert.equal(eventSchema.safeParse({ ...samples.probe, agent }).success, true, agent);
  }
});

test('the envelope and every event reject a repository field', () => {
  assert.equal(envelopeSchema.safeParse(envelope).success, true);
  for (const key of ['repo', 'repo_id', 'repository']) {
    assert.equal(envelopeSchema.safeParse({ ...envelope, [key]: 'oboete' }).success, false, key);
    assert.equal(eventSchema.safeParse({ ...samples.prompt, [key]: 'oboete' }).success, false, key);
  }
});

test('the schema rejects text above the one megabyte bound', () => {
  assert.equal(eventSchema.safeParse({ ...samples.prompt, text: 'a'.repeat(MAX_TEXT) }).success, true);
  assert.equal(
    eventSchema.safeParse({ ...samples.prompt, text: 'a'.repeat(MAX_TEXT + 1) }).success,
    false,
  );
  assert.equal(
    eventSchema.safeParse({ ...samples.tool_result, output: 'a'.repeat(MAX_TEXT + 1) }).success,
    false,
  );
});

test('the tool input takes no raw payload and bounds the path list', () => {
  assert.equal(
    eventSchema.safeParse({
      ...samples.tool_call,
      input: { ...toolInput, tool_input: { file_path: '/home/dev/repo/src/a.ts' } },
    }).success,
    false,
  );
  const paths = (count: number): string[] =>
    Array.from({ length: count }, (_unused, index) => `/home/dev/repo/src/f${index}.ts`);
  assert.equal(eventSchema.safeParse({ ...samples.tool_call, input: { paths: paths(50) } }).success, true);
  assert.equal(
    eventSchema.safeParse({ ...samples.tool_call, input: { paths: paths(51) } }).success,
    false,
  );
  assert.equal(
    eventSchema.safeParse({ ...samples.tool_call, input: { paths: [], text: 'x'.repeat(20_001) } })
      .success,
    false,
  );
});

test('an empty prompt_id is rejected so it can never become an identity key', () => {
  assert.equal(eventSchema.safeParse({ ...samples.prompt, prompt_id: '' }).success, false);
  assert.equal(eventSchema.safeParse({ ...samples.tool_call, tool_call_id: '' }).success, false);
});

test('a tool name is one of the normalized names or an mcp reference', () => {
  for (const name of TOOL_NAMES) {
    assert.equal(
      eventSchema.safeParse({ ...samples.tool_call, tool_name: name }).success,
      true,
      name,
    );
  }
  assert.equal(
    eventSchema.safeParse({ ...samples.tool_call, tool_name: 'mcp:oboete/search' }).success,
    true,
  );
  assert.equal(eventSchema.safeParse({ ...samples.tool_call, tool_name: 'Edit' }).success, false);
  assert.equal(
    eventSchema.safeParse({ ...samples.tool_call, tool_name: 'mcp:oboete' }).success,
    false,
  );
});

test('contentHash is the sha256 of the text', () => {
  assert.equal(contentHash('oboete'), sha256('oboete'));
});

test('the content hash ignores the envelope and the capture time', () => {
  const first = parse(samples.last_assistant_message);
  const redelivered = parse({
    ...samples.last_assistant_message,
    captured_at: 1,
    cwd: '/somewhere/else',
    model: 'grok-4.6-build',
    agent_id: 'sub-1',
  });
  assert.equal(eventContentHash(first), eventContentHash(redelivered));
  assert.equal(
    eventContentHash(first),
    sha256('{"kind":"last_assistant_message","prompt_id":"p-1","text":"Done."}'),
  );
  const other = parse({ ...samples.last_assistant_message, text: 'Something else.' });
  assert.notEqual(eventContentHash(first), eventContentHash(other));
});

test('the same event hashes to the same id twice', () => {
  assert.equal(eventId(parse(samples.tool_call)), eventId(parse(samples.tool_call)));
  assert.equal(eventId(parse(samples.turn_end)), eventId(parse(samples.turn_end)));
});

test('a tool_call and its tool_result never share an id', () => {
  const call = parse(samples.tool_call);
  const result = parse(samples.tool_result);
  const failure = parse({ ...samples.tool_failure, tool_call_id: 'toolu_1' });
  assert.equal(call.kind === 'tool_call' && call.tool_call_id, 'toolu_1');
  assert.equal(result.kind === 'tool_result' && result.tool_call_id, 'toolu_1');
  assert.notEqual(eventId(call), eventId(result));
  assert.notEqual(eventId(call), eventId(failure));
  assert.notEqual(eventId(result), eventId(failure));
});

test('a re-delivered event collapses to one id', () => {
  const again = { captured_at: 1_757_000_009_999, cwd: '/home/dev/repo' };
  assert.equal(
    eventId(parse(samples.tool_result)),
    eventId(parse({ ...samples.tool_result, ...again })),
  );
  assert.equal(
    eventId(parse(samples.session_end)),
    eventId(parse({ ...samples.session_end, ...again })),
  );
});

test('two identical turn_end events inside one turn collapse to one id', () => {
  // Documented limit of the content-hash key (contracts/agents.md, R7): with no delivery counter
  // the two are indistinguishable.
  const first = parse(samples.turn_end);
  const second = parse(samples.turn_end);
  assert.equal(eventId(first), eventId(second));
  // A later turn is a different key because the turn index differs.
  assert.notEqual(eventId(first), eventId(parse({ ...samples.turn_end, turn_index: 2 })));
});

test('a prompt_id decides the prompt id before the text does', () => {
  const first = parse({ ...samples.prompt, text: 'first' });
  const second = parse({ ...samples.prompt, text: 'second' });
  assert.equal(eventId(first), eventId(second));
});

test('an agent without prompt ids collapses identical prompts across turns', () => {
  // Wider face of the same limit: with no prompt id there is no turn key, so the collapse is
  // session-wide rather than per turn (R7 keys this form on the turn ordinal; Pi supplies none).
  const anonymous = { ...envelope, agent: 'pi', kind: 'prompt', input_source: 'user' };
  const turnOne = parse({ ...anonymous, text: 'continue' });
  const turnThree = parse({ ...anonymous, text: 'continue' });
  assert.equal(eventId(turnOne), eventId(turnThree));
});

test('prompts without a prompt_id are separated by their text', () => {
  const anonymous = { ...envelope, kind: 'prompt', input_source: 'user' };
  const first = parse({ ...anonymous, text: 'first' });
  const second = parse({ ...anonymous, text: 'second' });
  assert.notEqual(eventId(first), eventId(second));
});

test('a compaction_key decides the compaction id, an empty one falls back to the content', () => {
  const keyed = parse({ ...samples.compaction_summary, text: 'first' });
  const keyedAgain = parse({ ...samples.compaction_summary, text: 'second' });
  assert.equal(eventId(keyed), eventId(keyedAgain));
  const unkeyed = parse({ ...samples.compaction_summary, compaction_key: '', text: 'first' });
  const unkeyedOther = parse({ ...samples.compaction_summary, compaction_key: '', text: 'second' });
  assert.notEqual(eventId(unkeyed), eventId(unkeyedOther));
});

test('eventIdKey returns the documented key shapes and eventId is their sha256', () => {
  const cases: Array<[NormalizedEvent, string[]]> = [
    [parse(samples.tool_call), ['v1', 'claude', 's-1', 'tool_call', 'toolu_1']],
    [parse(samples.tool_result), ['v1', 'claude', 's-1', 'tool_result', 'toolu_1']],
    [parse(samples.tool_failure), ['v1', 'claude', 's-1', 'tool_failure', 'toolu_2']],
    [parse(samples.prompt), ['v1', 'claude', 's-1', 'prompt', 'p-1']],
    [
      parse(samples.compaction_summary),
      ['v1', 'claude', 's-1', 'compaction_summary', '2026-09-03T16:01:11Z'],
    ],
    [
      parse(samples.turn_end),
      [
        'v1',
        'claude',
        's-1',
        'turn_end',
        'p-1',
        sha256('{"kind":"turn_end","prompt_id":"p-1","reason":"end_turn","turn_index":1}'),
      ],
    ],
    [
      parse(samples.session_end),
      ['v1', 'claude', 's-1', 'session_end', '', sha256('{"kind":"session_end","reason":"shutdown"}')],
    ],
    [
      parse(samples.session_start),
      [
        'v1',
        'claude',
        's-1',
        'session_start',
        '',
        sha256('{"kind":"session_start","source":"startup"}'),
      ],
    ],
  ];
  for (const [event, key] of cases) {
    assert.deepEqual(eventIdKey(event), key, event.kind);
    assert.equal(eventId(event), sha256(JSON.stringify(key)), event.kind);
  }
});

test('conversationPolicy follows the documented table', () => {
  const rows: Array<{
    source: SessionStartSource | undefined;
    nativeSessionIdKnown: boolean;
    expected: 'reuse_root' | 'new_root';
  }> = [
    { source: 'startup', nativeSessionIdKnown: false, expected: 'new_root' },
    { source: 'startup', nativeSessionIdKnown: true, expected: 'reuse_root' },
    { source: 'clear', nativeSessionIdKnown: false, expected: 'new_root' },
    { source: 'clear', nativeSessionIdKnown: true, expected: 'reuse_root' },
    { source: 'resume', nativeSessionIdKnown: false, expected: 'new_root' },
    { source: 'resume', nativeSessionIdKnown: true, expected: 'reuse_root' },
    { source: 'compact', nativeSessionIdKnown: false, expected: 'reuse_root' },
    { source: 'compact', nativeSessionIdKnown: true, expected: 'reuse_root' },
    { source: 'fork', nativeSessionIdKnown: false, expected: 'new_root' },
    { source: 'fork', nativeSessionIdKnown: true, expected: 'new_root' },
    { source: 'new', nativeSessionIdKnown: false, expected: 'new_root' },
    { source: 'new', nativeSessionIdKnown: true, expected: 'new_root' },
    { source: undefined, nativeSessionIdKnown: false, expected: 'new_root' },
    { source: undefined, nativeSessionIdKnown: true, expected: 'reuse_root' },
  ];
  const covered = new Set(rows.map((row) => row.source));
  for (const source of SESSION_START_SOURCES) {
    assert.equal(covered.has(source), true, `${source} is untested`);
  }
  const agents: AgentName[] = ['claude', 'codex', 'grok', 'pi'];
  for (const row of rows) {
    for (const agent of agents) {
      assert.equal(
        conversationPolicy({
          agent,
          source: row.source,
          nativeSessionIdKnown: row.nativeSessionIdKnown,
        }),
        row.expected,
        `${agent} ${row.source} ${row.nativeSessionIdKnown}`,
      );
    }
    assert.equal(
      conversationPolicy({
        agent: 'unknown',
        source: row.source,
        nativeSessionIdKnown: row.nativeSessionIdKnown,
      }),
      'new_root',
      `unknown ${row.source} ${row.nativeSessionIdKnown}`,
    );
  }
});

test('isSummarizable is true only for kinds with content', () => {
  const expected: Record<EventKind, boolean> = {
    session_start: false,
    prompt: true,
    tool_call: true,
    tool_result: true,
    tool_failure: true,
    turn_end: false,
    session_end: false,
    compaction_summary: true,
    last_assistant_message: true,
    probe: false,
  };
  for (const kind of EVENT_KINDS) {
    assert.equal(isSummarizable(parse(samples[kind])), expected[kind], kind);
  }
});

test('an empty content makes a summarizable kind unsummarizable', () => {
  assert.equal(isSummarizable(parse({ ...samples.prompt, text: '' })), false);
  assert.equal(isSummarizable(parse({ ...samples.prompt, text: '   ' })), false);
  assert.equal(isSummarizable(parse({ ...samples.tool_result, output: '' })), false);
  assert.equal(isSummarizable(parse({ ...samples.tool_failure, error: '' })), false);
  assert.equal(isSummarizable(parse({ ...samples.last_assistant_message, text: '' })), false);
  assert.equal(
    isSummarizable(parse({ ...samples.compaction_summary, text: '' })),
    false,
  );
  assert.equal(isSummarizable(parse({ ...samples.tool_call, input: { paths: [] } })), false);
  assert.equal(
    isSummarizable(parse({ ...samples.tool_call, input: { paths: ['/home/dev/repo/a.ts'] } })),
    true,
  );
  assert.equal(
    isSummarizable(parse({ ...samples.tool_call, input: { paths: [], command: 'ls' } })),
    true,
  );
});

test('normalizeToolName maps each agent native tool name from the fixtures', () => {
  const cases: Array<[AgentName, string, string]> = [
    ['claude', 'Read', 'read'],
    ['claude', 'Write', 'write'],
    ['claude', 'Edit', 'edit'],
    ['claude', 'MultiEdit', 'edit'],
    ['claude', 'Bash', 'bash'],
    ['claude', 'Grep', 'grep'],
    ['claude', 'Glob', 'glob'],
    ['claude', 'Task', 'task'],
    ['claude', 'mcp__oboete_probe__search', 'mcp:oboete_probe/search'],
    ['claude', 'WebFetch', 'other'],
    ['codex', 'Bash', 'bash'],
    ['codex', 'apply_patch', 'edit'],
    ['codex', 'mcp__oboete_probe__search', 'mcp:oboete_probe/search'],
    ['codex', 'Read', 'other'],
    ['grok', 'read_file', 'read'],
    ['grok', 'write', 'write'],
    ['grok', 'search_replace', 'edit'],
    ['grok', 'run_terminal_command', 'bash'],
    ['grok', 'spawn_subagent', 'task'],
    ['grok', 'oboete_probe__search', 'mcp:oboete_probe/search'],
    ['grok', 'Read', 'other'],
    ['pi', 'read', 'read'],
    ['pi', 'write', 'write'],
    ['pi', 'edit', 'edit'],
    ['pi', 'bash', 'bash'],
    ['pi', 'grep', 'grep'],
    ['pi', 'glob', 'glob'],
    ['pi', 'oboete_probe', 'other'],
    ['unknown', 'Read', 'other'],
  ];
  for (const [agent, native, normalized] of cases) {
    assert.equal(normalizeToolName(agent, native), normalized, `${agent} ${native}`);
  }
});

test('a normalized tool name is always a value the schema accepts', () => {
  // Native names arrive from agent payloads, so the hostile set includes Object.prototype keys.
  const names = [
    'Read',
    'mcp__a__b',
    'mcp__a__b__c',
    'weird/name',
    'mcp__a/b__c',
    '',
    'constructor',
    'toString',
    'valueOf',
    'hasOwnProperty',
    '__proto__',
  ];
  for (const agent of AGENTS) {
    for (const native of names) {
      const normalized = normalizeToolName(agent, native);
      assert.equal(
        eventSchema.safeParse({ ...samples.tool_call, tool_name: normalized }).success,
        true,
        `${agent} ${native} -> ${normalized}`,
      );
    }
  }
});

test('a turn ordinal separates two identical lifecycle events of one session', () => {
  // R7 keys this form on (turn ordinal, kind, content hash). The adapter has no counter, so the
  // ordinal comes from the capture path, which knows sessions.turn_count.
  const event = parse(samples.session_end);
  const content = sha256('{"kind":"session_end","reason":"shutdown"}');
  assert.equal(eventId(event, 3), eventId(event, 3));
  assert.notEqual(eventId(event, 3), eventId(event, 4));
  assert.deepEqual(eventIdKey(event, 3), ['v1', 'claude', 's-1', 'session_end', '3', content]);

  // The agent's own per-turn value wins over the ordinal where the payload carries one.
  const keyed = parse({ ...samples.session_end, prompt_id: 'p-9' });
  assert.deepEqual(eventIdKey(keyed, 3), [
    'v1',
    'claude',
    's-1',
    'session_end',
    'p-9',
    sha256('{"kind":"session_end","prompt_id":"p-9","reason":"shutdown"}'),
  ]);

  // Neither: the documented residual limit, an empty turn key.
  assert.deepEqual(eventIdKey(event), ['v1', 'claude', 's-1', 'session_end', '', content]);
});

test('a compaction without a native key is separated by the turn it belongs to', () => {
  // Codex and Claude Code have no per-compaction value, so without a turn key every compaction of
  // one native session would collapse into a single raw_events row (contracts/agents.md A16).
  const unkeyed = { ...samples.compaction_summary, compaction_key: '', text: '' };
  const first = parse({ ...unkeyed, prompt_id: 't-1' });
  const second = parse({ ...unkeyed, prompt_id: 't-2' });
  assert.notEqual(eventId(first), eventId(second));
  assert.notEqual(eventId(parse(unkeyed), 1), eventId(parse(unkeyed), 2));
});
