import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  excerptInput,
  MIN_PROMPT_TEXT,
  observerOutputJsonSchema,
  observerOutputSchema,
  shortenDisplayPath,
  trimObservation,
  validateObserverOutput,
  type Observation,
  type ObserverInput,
} from '../../src/observer/contract.js';

function observation(overrides: Partial<Observation> = {}): Observation {
  return {
    type: 'change',
    title: 'edited src/cli.ts',
    body: 'write src/cli.ts (+12/-3)',
    concepts: ['what-changed'],
    citations: {
      files_read: [],
      files_modified: ['src/cli.ts'],
      commits: [],
    },
    source_event_ids: ['e1'],
    classification: { decision: 'add', target: null, reason: 'new' },
    ...overrides,
  };
}

function output(observations: Observation[] = [observation()]) {
  return { observations };
}

const events: ObserverInput['events'] = [
  { id: 'e1', kind: 'prompt', text: 'fix the parser' },
  { id: 'e2', kind: 'tool_result', output: 'ok', tool_name: 'edit' },
];

const nearby: ObserverInput['nearby'] = [
  {
    id: 'm1',
    type: 'decision',
    title: 'keep zod',
    body: 'one schema both paths',
    deleted: false,
  },
];

test('valid output parses', () => {
  const parsed = observerOutputSchema.safeParse(output());
  assert.equal(parsed.success, true);
  if (parsed.success) {
    assert.equal(parsed.data.observations.length, 1);
    assert.equal(parsed.data.observations[0]?.title, 'edited src/cli.ts');
  }
});

test('21 observations are rejected', () => {
  const parsed = observerOutputSchema.safeParse(
    output(Array.from({ length: 21 }, () => observation())),
  );
  assert.equal(parsed.success, false);
});

test('unknown key is rejected', () => {
  const parsed = observerOutputSchema.safeParse({
    observations: [observation()],
    extra: true,
  });
  assert.equal(parsed.success, false);
});

test('title of 121 characters is rejected', () => {
  const parsed = observerOutputSchema.safeParse(
    output([observation({ title: 't'.repeat(121) })]),
  );
  assert.equal(parsed.success, false);
});

test('empty source_event_ids is rejected', () => {
  const parsed = observerOutputSchema.safeParse(
    output([observation({ source_event_ids: [] })]),
  );
  assert.equal(parsed.success, false);
});

test('validateObserverOutput rejects a foreign source id as unusable_output', () => {
  const result = validateObserverOutput(
    output([observation({ source_event_ids: ['foreign-id'] })]),
    { events, nearby },
  );
  assert.equal(result.ok, false);
  if (!result.ok) {
    assert.equal(result.reason, 'unusable_output');
    assert.match(result.detail, /foreign-id/);
  }
});

test('validateObserverOutput rewrites a missing nearby target to add/null', () => {
  const result = validateObserverOutput(
    output([
      observation({
        classification: {
          decision: 'update',
          target: 'm-missing',
          reason: 'looked similar',
        },
      }),
    ]),
    { events, nearby },
  );
  assert.equal(result.ok, true);
  if (result.ok) {
    assert.equal(result.output.observations[0]?.classification.decision, 'add');
    assert.equal(result.output.observations[0]?.classification.target, null);
  }
});

test('validateObserverOutput turns delete with empty reason into noop', () => {
  const result = validateObserverOutput(
    output([
      observation({
        classification: { decision: 'delete', target: 'm1', reason: '' },
      }),
    ]),
    { events, nearby },
  );
  assert.equal(result.ok, true);
  if (result.ok) {
    assert.equal(result.output.observations[0]?.classification.decision, 'noop');
    assert.equal(result.output.observations[0]?.classification.target, 'm1');
  }
});

test('validateObserverOutput returns ok with the same observations', () => {
  const raw = output([
    observation({
      classification: { decision: 'update', target: 'm1', reason: 'same fact' },
    }),
  ]);
  const result = validateObserverOutput(raw, { events, nearby });
  assert.equal(result.ok, true);
  if (result.ok) {
    assert.deepEqual(result.output.observations, raw.observations);
  }
});

test('trimObservation shortens a 60-line body and keeps 20 paths', () => {
  const line = 'b'.repeat(100);
  const body = Array.from({ length: 60 }, () => line).join('\n');
  const paths = Array.from({ length: 25 }, (_, i) => `src/f${i}.ts`);
  const trimmed = trimObservation(
    observation({
      body,
      citations: {
        files_read: paths,
        files_modified: paths,
        commits: [],
      },
    }),
  );
  assert.ok(trimmed.body.length <= 2000);
  assert.match(trimmed.body, /\.\.\. \(\+\d+ omitted\)$/);
  assert.ok(trimmed.body.startsWith(line));
  assert.equal(trimmed.citations.files_read.length, 20);
  assert.equal(trimmed.citations.files_modified.length, 20);
  assert.deepEqual(trimmed.citations.files_read, paths.slice(0, 20));
});

test('shortenDisplayPath of a 200-character path is 61 characters', () => {
  const path = 'p'.repeat(200);
  const short = shortenDisplayPath(path);
  assert.equal(short.length, 61);
  assert.equal(short.startsWith('…'), true);
  assert.ok(short.endsWith(path.slice(-60)));
});

test('excerptInput keeps summaries and prompts and the newest tool events', () => {
  const toolEvents: ObserverInput['events'] = [];
  for (let i = 0; i < 30; i += 1) {
    toolEvents.push({
      id: `t${i}`,
      kind: 'tool_result',
      tool_name: 'read',
      output: 'x'.repeat(900),
    });
  }
  const input: ObserverInput = {
    repo_ref: 'repo-1',
    session: {
      started_at: 1,
      turns: [{ ordinal: 0, started_at: 1, ended_at: 2 }],
    },
    events: [
      { id: 'p1', kind: 'prompt', text: 'first prompt' },
      ...toolEvents.slice(0, 20),
      { id: 'p2', kind: 'prompt', text: 'second prompt' },
      ...toolEvents.slice(20),
    ],
    free_summaries: {
      last_assistant_message: 'assistant kept',
      compaction_summary: 'compaction kept',
    },
    nearby: [],
    language_hint: 'en',
  };

  const before = JSON.stringify(input).length;
  assert.ok(before > 25_000);
  assert.ok(before < 40_000);

  const { input: excerpted, excerpted: didExcerpt } = excerptInput(input);
  const after = JSON.stringify(excerpted).length;
  assert.equal(didExcerpt, true);
  assert.ok(after <= 12_000);
  assert.equal(excerpted.free_summaries.last_assistant_message, 'assistant kept');
  assert.equal(excerpted.free_summaries.compaction_summary, 'compaction kept');

  const ids = excerpted.events.map((event) => event.id);
  assert.ok(ids.includes('p1'));
  assert.ok(ids.includes('p2'));

  const survivingTools = excerpted.events
    .filter((event) => event.kind === 'tool_result')
    .map((event) => event.id);
  assert.ok(survivingTools.length > 0);
  assert.ok(survivingTools.length < 30);
  const expected = Array.from({ length: 30 }, (_, i) => `t${i}`).slice(
    30 - survivingTools.length,
  );
  assert.deepEqual(survivingTools, expected);
});

test('observerOutputJsonSchema serializes and contains observations', () => {
  const encoded = JSON.stringify(observerOutputJsonSchema);
  assert.ok(encoded.length > 0);
  const parsed = JSON.parse(encoded) as {
    properties?: { observations?: unknown };
  };
  assert.ok(parsed.properties?.observations);
});

test('trimObservation keeps as much of a single long line as the budget allows', () => {
  const trimmed = trimObservation(observation({ body: 'z'.repeat(2_500) }));
  assert.equal(trimmed.body.length, 2000);
  assert.match(trimmed.body, /\n\.\.\. \(\+1 omitted\)$/);
  assert.ok(trimmed.body.startsWith('z'.repeat(1_983)));
});

test('validateObserverOutput trims an oversized body and title instead of refusing the batch', () => {
  const result = validateObserverOutput(
    output([observation({ title: 't'.repeat(200), body: 'b'.repeat(2_100) })]),
    { events, nearby },
  );
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const observed = result.output.observations[0];
  assert.equal(observed?.title.length, 120);
  assert.ok((observed?.body.length ?? 0) <= 2000);
  assert.match(observed?.body ?? '', /\n\.\.\. \(\+1 omitted\)$/);
});

test('validateObserverOutput cuts the citation lists and normalizes the commit ids', () => {
  const paths = Array.from({ length: 25 }, (_, index) => `src/f${index}.ts`);
  const result = validateObserverOutput(
    output([
      observation({
        citations: { files_read: paths, files_modified: paths, commits: ['ABCDEF1', 'HEAD'] },
      }),
    ]),
    { events, nearby },
  );
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const cited = result.output.observations[0]?.citations;
  assert.equal(cited?.files_read.length, 20);
  assert.equal(cited?.files_modified.length, 20);
  // A symbolic reference is dropped, not a reason to lose the whole batch.
  assert.deepEqual(cited?.commits, ['abcdef1']);
});

test('validateObserverOutput still refuses a structurally broken output', () => {
  const missingField = validateObserverOutput({ observations: [{ type: 'change' }] }, { events, nearby });
  assert.equal(missingField.ok, false);
  const unknownKey = validateObserverOutput(
    { observations: [{ ...observation(), extra: true }] },
    { events, nearby },
  );
  assert.equal(unknownKey.ok, false);
  const tooMany = validateObserverOutput(
    output(Array.from({ length: 21 }, () => observation())),
    { events, nearby },
  );
  assert.equal(tooMany.ok, false);
  const empty = validateObserverOutput(
    output([observation({ source_event_ids: [] })]),
    { events, nearby },
  );
  assert.equal(empty.ok, false);
});

test('excerptInput bounds an oversized compaction summary and keeps the summary above the prompt', () => {
  const prompt = 'fix the parser '.repeat(40);
  const { input: excerpted, excerpted: didExcerpt } = excerptInput({
    repo_ref: 'repo-1',
    session: { started_at: 1, turns: [{ ordinal: 0, started_at: 1, ended_at: 2 }] },
    events: [{ id: 'p1', kind: 'prompt', text: prompt }],
    free_summaries: { compaction_summary: 's'.repeat(30_000) },
    nearby: [],
    language_hint: 'en',
  });
  assert.equal(didExcerpt, true);
  assert.ok(JSON.stringify(excerpted).length <= 12_000);
  // contracts/observer.md "Input": the free summaries are kept first and the prompts next, so the
  // prompt is shortened to its floor before the summary loses anything.
  assert.equal(prompt.length, 600);
  assert.equal(excerpted.events[0]?.text?.length, MIN_PROMPT_TEXT);
  assert.ok((excerpted.free_summaries.compaction_summary?.length ?? 0) > 10_000);
});

test('excerptInput bounds the nearby memories before it touches the prompts', () => {
  const prompt = 'fix the parser';
  const { input: excerpted, excerpted: didExcerpt } = excerptInput({
    repo_ref: 'repo-1',
    session: { started_at: 1, turns: [] },
    events: [{ id: 'p1', kind: 'prompt', text: prompt }],
    free_summaries: {},
    nearby: Array.from({ length: 40 }, (_, index) => ({
      id: `m${index}`,
      type: 'decision',
      title: `nearby ${index}`,
      body: 'n'.repeat(2_000),
      deleted: false,
    })),
    language_hint: 'en',
  });
  assert.equal(didExcerpt, true);
  assert.ok(JSON.stringify(excerpted).length <= 12_000);
  assert.equal(excerpted.nearby.length, 40);
  for (const row of excerpted.nearby) assert.ok(row.body.length <= 500, 'a nearby body is capped');
  assert.equal(excerpted.events[0]?.text, prompt);
});

test('excerptInput never empties a prompt', () => {
  const { input: excerpted } = excerptInput({
    repo_ref: 'repo-1',
    session: { started_at: 1, turns: [] },
    events: Array.from({ length: 6 }, (_, index) => ({
      id: `p${index}`,
      kind: 'prompt' as const,
      text: 'p'.repeat(3_000),
    })),
    free_summaries: {},
    nearby: [],
    language_hint: 'en',
  });
  assert.equal(excerpted.events.length, 6);
  for (const event of excerpted.events) assert.ok((event.text?.length ?? 0) >= 200);
});

test('excerptInput bounds the input even when every prompt is already at its floor', () => {
  const { input: excerpted, excerpted: didExcerpt } = excerptInput({
    repo_ref: 'repo-1',
    session: { started_at: 1, turns: [] },
    events: Array.from({ length: 100 }, (_, index) => ({
      id: `p${index}`,
      kind: 'prompt' as const,
      text: 'p'.repeat(3_000),
    })),
    free_summaries: {},
    nearby: [],
    language_hint: 'en',
  });
  assert.equal(didExcerpt, true);
  // FR-015 is a cap, not a preference: 100 prompts at MIN_PROMPT_TEXT would be 24,008 characters.
  assert.ok(
    JSON.stringify(excerpted).length <= 12_000,
    `serialized ${JSON.stringify(excerpted).length} characters`,
  );
  // The oldest prompts pay for it, so the newest still carries its text.
  assert.ok((excerpted.events.at(-1)?.text?.length ?? 0) >= MIN_PROMPT_TEXT);
});

test('excerptInput bounds oversized nearby titles and a long turn list', () => {
  const prompt = 'fix the parser';
  const { input: excerpted, excerpted: didExcerpt } = excerptInput({
    repo_ref: 'repo-1',
    session: {
      started_at: 1,
      turns: Array.from({ length: 2_000 }, (_, index) => ({
        ordinal: index,
        started_at: index,
        ended_at: index + 1,
      })),
    },
    events: [{ id: 'p1', kind: 'prompt', text: prompt }],
    free_summaries: {},
    nearby: Array.from({ length: 40 }, (_, index) => ({
      id: `m${index}`,
      type: 'decision',
      title: 't'.repeat(2_000),
      body: '',
      deleted: false,
    })),
    language_hint: 'en',
  });
  assert.equal(didExcerpt, true);
  assert.ok(
    JSON.stringify(excerpted).length <= 12_000,
    `serialized ${JSON.stringify(excerpted).length} characters`,
  );
  // Context from other sessions and the turn list go before the session's own prompt.
  assert.equal(excerpted.events[0]?.text, prompt);
});

test('validateObserverOutput trims a long classification reason instead of refusing the batch', () => {
  const result = validateObserverOutput(
    output([
      observation({
        classification: { decision: 'update', target: 'm1', reason: 'r'.repeat(250) },
      }),
    ]),
    { events, nearby },
  );
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.output.observations[0]?.classification.reason.length, 200);
});
