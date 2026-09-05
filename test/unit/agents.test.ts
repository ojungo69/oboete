import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  MAX_TOOL_INPUT_PATHS,
  MAX_TOOL_INPUT_TEXT,
  eventId,
  eventSchema,
  type NormalizedEvent,
  type ToolName,
} from '../../src/events.js';
import {
  adapt,
  resolveAgent,
  scanPartialPrefix,
  textFields,
  type AdapterAgent,
  type AdapterOutput,
} from '../../src/agents/index.js';
import { piEnvelopeSchema } from '../../src/agents/pi.js';
import { z } from 'zod';

type Json = Record<string, unknown>;

const AGENTS: AdapterAgent[] = ['claude', 'codex', 'grok', 'pi'];
const CAPTURED_AT = 1_757_000_000_000;

function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    assert.notEqual(parent, directory, 'the repository root must contain package.json');
    directory = parent;
  }
}

const CONTRACTS = join(repositoryRoot(), 'test', 'contracts');

function fixtureNames(agent: string): string[] {
  return readdirSync(join(CONTRACTS, agent))
    .filter((name) => name.endsWith('.json'))
    .sort();
}

function loadFixture(agent: string, name: string): Json {
  return JSON.parse(readFileSync(join(CONTRACTS, agent, name), 'utf8')) as Json;
}

function record(value: unknown): Json {
  assert.equal(typeof value, 'object');
  assert.notEqual(value, null);
  return value as Json;
}

function text(value: unknown): string {
  assert.equal(typeof value, 'string');
  return value as string;
}

function events(output: AdapterOutput): NormalizedEvent[] {
  assert.equal(output.kind, 'events', `expected events, got ${JSON.stringify(output)}`);
  if (output.kind !== 'events') throw new Error('unreachable');
  for (const event of output.events) {
    const parsed = eventSchema.safeParse(event);
    assert.equal(parsed.success, true, `event does not match the schema: ${JSON.stringify(event)}`);
  }
  return output.events;
}

function detector(output: AdapterOutput): { paths: string[] } {
  assert.equal(output.kind, 'events');
  if (output.kind !== 'events') throw new Error('unreachable');
  return output.contentForDetector;
}

/** Every string the detector is handed for these events: the one table capture redacts through. */
function detectorText(output: AdapterOutput): string {
  return events(output)
    .flatMap((event) => textFields(event).map((field) => field.read()))
    .join('\n');
}

function eventOf<K extends NormalizedEvent['kind']>(
  list: NormalizedEvent[],
  kind: K,
): Extract<NormalizedEvent, { kind: K }> {
  const found = list.find((event) => event.kind === kind);
  assert.ok(found, `no ${kind} event in ${JSON.stringify(list.map((event) => event.kind))}`);
  return found as Extract<NormalizedEvent, { kind: K }>;
}

function piWire(event: string, payload: unknown, extra: Json = {}): Json {
  return { event, session_id: 'pi-session-1', cwd: '/repo', ...extra, payload };
}

// Fixtures that record something other than a tool payload; each has its own test below.
const NON_TOOL_FIXTURES: Record<string, string> = {
  'codex/mcp-frames.json': 'MCP frame log, no hook payload',
  'codex/postcompact.json': 'PostCompact payloads and timeline',
  'codex/rollout-flush.json': 'transcript flush observations, no hook payload',
  'codex/session-start-compact.json': 'SessionStart payloads and timeline',
  'grok/mcp-search.json': 'MCP registration probe',
  'grok/permission-denied.json': 'PermissionDenied payload',
  'grok/postcompact.json': 'PostCompact payload',
  'grok/posttooluse-failure.json': 'failed shell call arriving as PostToolUse',
  'grok/session-start-resume.json': 'SessionStart sources new and load',
  'grok/stop-end-turn.json': 'Stop and SessionEnd payloads',
  'pi/compaction.json': 'session_before_compact and session_compact payloads',
};

// What each tool fixture must produce. Written from the fixture files by hand so that a change of
// the mapping cannot be hidden by recomputing the expectation through the adapter.
type Expectation = {
  paths: string[];
  contains: string[];
  toolName?: ToolName;
  linesAdded?: number;
  linesRemoved?: number;
  isError?: boolean;
  /** The exact string the tool_result must store, where the result is not the raw tool text. */
  output?: string;
};

const EXPECTED: Record<string, Expectation> = {
  'claude/read.json': {
    paths: ['<repo>/README.md'],
    contains: ['oboete probe repository\nsecond line\n'],
  },
  'claude/write.json': {
    paths: ['<repo>/notes.txt'],
    contains: ['alpha\n'],
    linesAdded: 1,
    // The result echoes the whole file back; only the path is kept as the summary line.
    output: '<repo>/notes.txt',
  },
  'claude/edit.json': {
    paths: ['<repo>/notes.txt'],
    contains: ['alpha\n→\nbeta'],
    linesAdded: 1,
    linesRemoved: 1,
    output: '<repo>/notes.txt',
  },
  'claude/bash.json': { paths: [], contains: ['echo probe-ok', 'probe-ok'] },
  'claude/bash-failure.json': {
    paths: [],
    contains: ['echo fail-stderr >&2; exit 3', 'Exit code 3\nfail-stderr'],
  },
  'claude/read-failure.json': {
    paths: ['<repo>/missing-probe-file-does-not-exist.txt'],
    contains: ['File does not exist. Note: your current working directory is <repo>.'],
  },
  'codex/bash.json': { paths: [], contains: ['echo probe-ok', 'probe-ok\n'] },
  // The fixture's normalized_tool records what the probe did (read a file through the shell). The
  // adapter records the tool Codex actually reported, because nothing but the command text says so.
  'codex/bash-read.json': {
    paths: [],
    contains: ['cat README.md', 'oboete probe repository\nsecond line\n'],
    toolName: 'bash',
  },
  'codex/apply_patch-add.json': {
    paths: ['notes.txt'],
    contains: ['*** Add File: notes.txt', '+alpha'],
    linesAdded: 1,
    linesRemoved: 0,
  },
  'codex/apply_patch-update.json': {
    paths: ['notes.txt'],
    contains: ['-alpha\n+beta'],
    linesAdded: 1,
    linesRemoved: 1,
  },
  'grok/read_file.json': {
    paths: ['README.md'],
    contains: ['1→oboete probe repository\nsecond line\n'],
  },
  'grok/write.json': {
    paths: ['notes.txt'],
    contains: ['alpha', 'The file <repo>/notes.txt has been created.'],
    linesAdded: 1,
  },
  'grok/search_replace.json': {
    paths: ['notes.txt'],
    contains: ['alpha\n→\nbeta', 'The file notes.txt has been updated successfully.'],
    linesAdded: 1,
    linesRemoved: 1,
  },
  'grok/run_terminal_command.json': {
    paths: [],
    contains: ['echo probe-ok', 'exit: 0\nprobe-ok\n'],
    isError: false,
  },
  'pi/read.json': {
    paths: ['README.md'],
    contains: ['oboete probe repository\nsecond line\n'],
  },
  'pi/write.json': {
    paths: ['notes.txt'],
    contains: ['alpha', 'Successfully wrote 5 bytes to notes.txt'],
    linesAdded: 1,
  },
  'pi/edit.json': {
    paths: ['notes.txt'],
    contains: ['alpha\n→\nbeta', 'Successfully replaced 1 block(s) in notes.txt.'],
    linesAdded: 1,
    linesRemoved: 1,
  },
  'pi/bash.json': { paths: [], contains: ['echo probe-ok', 'probe-ok\n'] },
};

// Hook event name -> the kinds one call of adapt must produce for a tool fixture.
const HOOK_KINDS: Record<string, NormalizedEvent['kind'][]> = {
  PreToolUse: ['tool_call'],
  PostToolUse: ['tool_result'],
  PostToolUseFailure: ['tool_failure'],
  tool_result: ['tool_call', 'tool_result'],
};

function callId(agent: AdapterAgent, payload: Json): string {
  if (agent === 'grok') return text(payload.toolUseId);
  if (agent === 'pi') return text(payload.toolCallId);
  return text(payload.tool_use_id);
}

test('every committed fixture is either a tool fixture or covered by its own test', () => {
  let toolFixtures = 0;
  for (const agent of AGENTS) {
    for (const name of fixtureNames(agent)) {
      const key = `${agent}/${name}`;
      const fixture = loadFixture(agent, name);
      if (fixture.events === undefined) {
        assert.ok(key in NON_TOOL_FIXTURES, `${key} records no hook events and has no test`);
        continue;
      }
      if (fixture.normalized_tool === undefined && key !== 'pi/oboete_probe.json') {
        assert.ok(key in NON_TOOL_FIXTURES, `${key} names no normalized tool and has no test`);
        continue;
      }
      toolFixtures += 1;
    }
  }
  assert.equal(toolFixtures, 19);
});

for (const agent of AGENTS) {
  for (const name of fixtureNames(agent)) {
    const key = `${agent}/${name}`;
    const expectation = EXPECTED[key];
    if (expectation === undefined) continue;
    test(`${key} maps to normalized events`, () => {
      const fixture = loadFixture(agent, name);
      const recorded = record(fixture.events);
      let toolCallId: string | undefined;
      for (const [eventName, kinds] of Object.entries(HOOK_KINDS)) {
        const raw = recorded[eventName];
        if (raw === undefined) continue;
        const payload = record(raw);
        const output = adapt({
          agent,
          eventName,
          payload: agent === 'pi' ? piWire(eventName, payload) : payload,
          capturedAt: CAPTURED_AT,
        });
        const produced = events(output);
        assert.deepEqual(
          produced.map((event) => event.kind),
          kinds,
        );
        const content = detector(output);
        for (const event of produced) {
          assert.equal(event.agent, agent);
          assert.equal(event.captured_at, CAPTURED_AT);
        }

        if (kinds.includes('tool_call')) {
          const call = eventOf(produced, 'tool_call');
          assert.equal(call.tool_name, expectation.toolName ?? text(fixture.normalized_tool));
          assert.equal(call.tool_name_native, text(fixture.native_tool));
          assert.deepEqual(call.input.paths, expectation.paths);
          for (const path of expectation.paths) assert.ok(content.paths.includes(path));
          if (expectation.linesAdded !== undefined) {
            assert.equal(call.input.lines_added, expectation.linesAdded);
          }
          if (expectation.linesRemoved !== undefined) {
            assert.equal(call.input.lines_removed, expectation.linesRemoved);
          }
        }
        if (kinds.includes('tool_result')) {
          const result = eventOf(produced, 'tool_result');
          if (expectation.isError !== undefined) assert.equal(result.is_error, expectation.isError);
          if (expectation.output !== undefined) assert.equal(result.output, expectation.output);
          // The result carries the file body, so the detector must also see the path of the file it
          // came from or a repository path rule can never classify the row (FR-017, R4).
          for (const path of expectation.paths) {
            assert.ok(content.paths.includes(path), `${eventName} hides ${path} from the detector`);
          }
        }

        for (const event of produced) {
          if (event.kind === 'tool_call' || event.kind === 'tool_result' || event.kind === 'tool_failure') {
            assert.equal(event.tool_call_id, callId(agent, payload));
            // A tool call and its result are joined by the native call id (R7).
            if (toolCallId !== undefined) assert.equal(event.tool_call_id, toolCallId);
            toolCallId = event.tool_call_id;
          }
        }
      }
      assert.ok(toolCallId !== undefined, 'the fixture produced no tool event');

      // The detector sees every content string the events carry, per hook event of the fixture.
      const seen: string[] = [];
      for (const [eventName] of Object.entries(HOOK_KINDS)) {
        const raw = recorded[eventName];
        if (raw === undefined) continue;
        const output = adapt({
          agent,
          eventName,
          payload: agent === 'pi' ? piWire(eventName, record(raw)) : record(raw),
          capturedAt: CAPTURED_AT,
        });
        seen.push(detectorText(output));
      }
      const joined = seen.join('\n');
      for (const needle of expectation.contains) {
        assert.ok(joined.includes(needle), `contentForDetector is missing ${JSON.stringify(needle)}`);
      }
    });
  }
}

test('Codex apply_patch parses both patch shapes and counts patch lines', () => {
  const add = record(record(loadFixture('codex', 'apply_patch-add.json').events).PreToolUse);
  const update = record(record(loadFixture('codex', 'apply_patch-update.json').events).PreToolUse);
  const addPatch = text(record(add.tool_input).command);
  const updatePatch = text(record(update.tool_input).command);
  // Counted by hand from the fixture text: the add patch has one "+alpha" line and no "-" line;
  // the update patch has "+beta" and "-alpha".
  assert.equal(addPatch.split('\n').filter((line) => line.startsWith('+')).length, 1);
  assert.equal(addPatch.split('\n').filter((line) => line.startsWith('-')).length, 0);
  assert.equal(updatePatch.split('\n').filter((line) => line.startsWith('+')).length, 1);
  assert.equal(updatePatch.split('\n').filter((line) => line.startsWith('-')).length, 1);

  const addCall = eventOf(
    events(adapt({ agent: 'codex', eventName: 'PreToolUse', payload: add, capturedAt: CAPTURED_AT })),
    'tool_call',
  );
  assert.equal(addCall.tool_name, 'write');
  assert.deepEqual(addCall.input.paths, ['notes.txt']);
  assert.equal(addCall.input.lines_added, 1);
  assert.equal(addCall.input.lines_removed, 0);

  const updateCall = eventOf(
    events(adapt({ agent: 'codex', eventName: 'PreToolUse', payload: update, capturedAt: CAPTURED_AT })),
    'tool_call',
  );
  assert.equal(updateCall.tool_name, 'edit');
  assert.deepEqual(updateCall.input.paths, ['notes.txt']);
  assert.equal(updateCall.input.lines_added, 1);
  assert.equal(updateCall.input.lines_removed, 1);
});

test('Codex maps a delete patch, a prompt and a compaction without a summary', () => {
  const patch = '*** Begin Patch\n*** Delete File: notes.txt\n*** End Patch';
  const call = eventOf(
    events(
      adapt({
        agent: 'codex',
        eventName: 'PreToolUse',
        payload: {
          session_id: 's-1',
          cwd: '/repo',
          turn_id: 't-1',
          model: 'gpt-5.6-sol',
          tool_name: 'apply_patch',
          tool_input: { command: patch },
          tool_use_id: 'exec-1',
        },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'tool_call',
  );
  assert.equal(call.tool_name, 'edit');
  assert.deepEqual(call.input.paths, ['notes.txt']);
  assert.equal(call.model, 'gpt-5.6-sol');
  assert.equal(call.prompt_id, 't-1');

  const prompt = eventOf(
    events(
      adapt({
        agent: 'codex',
        eventName: 'UserPromptSubmit',
        payload: { session_id: 's-1', cwd: '/repo', turn_id: 't-1', prompt: 'fix the parser' },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'prompt',
  );
  assert.equal(prompt.text, 'fix the parser');
  assert.equal(prompt.input_source, 'user');
  assert.equal(prompt.prompt_id, 't-1');

  const post = record(record(loadFixture('codex', 'postcompact.json').headless).post);
  const compaction = eventOf(
    events(adapt({ agent: 'codex', eventName: 'PostCompact', payload: record(post[0]), capturedAt: CAPTURED_AT })),
    'compaction_summary',
  );
  assert.equal(compaction.text, '');
  assert.equal(compaction.compaction_key, '');

  const start = record(loadFixture('codex', 'session-start-compact.json').session_starts);
  const started = eventOf(
    events(adapt({ agent: 'codex', eventName: 'SessionStart', payload: record(start[0]), capturedAt: CAPTURED_AT })),
    'session_start',
  );
  assert.equal(started.source, 'startup');

  // Codex hands the final assistant text to `codex exec --output-last-message`, not to Stop.
  const stop = events(
    adapt({
      agent: 'codex',
      eventName: 'Stop',
      payload: { session_id: 's-1', cwd: '/repo', turn_id: 't-1' },
      capturedAt: CAPTURED_AT,
    }),
  );
  assert.deepEqual(stop.map((event) => event.kind), ['turn_end']);
});

test('Grok reads camelCase keys and ignores the snake_case duplicates', () => {
  const payload = record(record(loadFixture('grok', 'read_file.json').events).PreToolUse);
  const trimmed: Json = {};
  for (const [key, value] of Object.entries(payload)) {
    if (!key.includes('_')) trimmed[key] = value;
  }
  assert.equal(trimmed.session_id, undefined);
  assert.equal(trimmed.tool_input, undefined);
  const call = eventOf(
    events(adapt({ agent: 'grok', eventName: 'PreToolUse', payload: trimmed, capturedAt: CAPTURED_AT })),
    'tool_call',
  );
  assert.equal(call.native_session_id, text(payload.sessionId));
  assert.equal(call.tool_call_id, text(payload.toolUseId));
  assert.deepEqual(call.input.paths, ['README.md']);
});

test('Grok reports a failed shell call from exit_code and never the byte array', () => {
  const payload = record(loadFixture('grok', 'posttooluse-failure.json').PostToolUse);
  const output = adapt({ agent: 'grok', eventName: 'PostToolUse', payload, capturedAt: CAPTURED_AT });
  const result = eventOf(events(output), 'tool_result');
  assert.equal(result.output, 'exit: 3\nboom\n');
  assert.equal(result.is_error, true);
  assert.equal(JSON.stringify(output).includes('[98,111,111,109,10]'), false);
});

test('Grok stores nothing when a tool result lacks the one field oboete names', () => {
  const stripped = (name: string, drop: (result: Json) => void): Json => {
    const payload = record(record(loadFixture('grok', name).events).PostToolUse);
    const result = record(structuredClone(payload.toolResult));
    drop(result);
    return { ...payload, toolResult: result };
  };
  const cases: Json[] = [
    stripped('run_terminal_command.json', (result) => {
      delete result.output_for_prompt;
    }),
    stripped('read_file.json', (result) => {
      delete record(result.FileContent).content;
    }),
    stripped('write.json', (result) => {
      delete record(result.EditsApplied).tool_output_for_prompt;
    }),
  ];
  for (const payload of cases) {
    const output = adapt({ agent: 'grok', eventName: 'PostToolUse', payload, capturedAt: CAPTURED_AT });
    // An unexpected result shape stores no output at all, never every field oboete never named.
    assert.equal(eventOf(events(output), 'tool_result').output, '');
    const encoded = JSON.stringify(output);
    assert.equal(encoded.includes('[112,114,111,98,101'), false);
    assert.equal(encoded.includes('absolute_path'), false);
    assert.equal(encoded.includes('raw_output'), false);
  }
});

test('Grok PermissionDenied becomes a tool failure keyed by the tool use id', () => {
  const payload = record(loadFixture('grok', 'permission-denied.json').permissionRule);
  const failure = eventOf(
    events(adapt({ agent: 'grok', eventName: 'PermissionDenied', payload, capturedAt: CAPTURED_AT })),
    'tool_failure',
  );
  assert.equal(failure.tool_call_id, text(payload.toolUseId));
  assert.equal(failure.error, 'permission denied by a permission rule');
});

test('Grok PostCompact uses the payload timestamp as the compaction key', () => {
  const payload = record(loadFixture('grok', 'postcompact.json').PostCompact);
  const compaction = eventOf(
    events(adapt({ agent: 'grok', eventName: 'PostCompact', payload, capturedAt: CAPTURED_AT })),
    'compaction_summary',
  );
  assert.equal(compaction.compaction_key, text(payload.timestamp));
  assert.equal(compaction.text, '');
});

test('Grok SessionStart maps new to startup and load to resume', () => {
  const fixture = loadFixture('grok', 'session-start-resume.json');
  const first = eventOf(
    events(adapt({ agent: 'grok', eventName: 'SessionStart', payload: record(fixture.A), capturedAt: CAPTURED_AT })),
    'session_start',
  );
  assert.equal(first.source, 'startup');
  const resumed = eventOf(
    events(adapt({ agent: 'grok', eventName: 'SessionStart', payload: record(fixture.B), capturedAt: CAPTURED_AT })),
    'session_start',
  );
  assert.equal(resumed.source, 'resume');
  // A fork also reports "load", with a new session id; capture tells the two apart, not the adapter.
  const forked = eventOf(
    events(adapt({ agent: 'grok', eventName: 'SessionStart', payload: record(fixture.C), capturedAt: CAPTURED_AT })),
    'session_start',
  );
  assert.equal(forked.source, 'resume');
});

test('Grok Stop carries the last assistant message only on end_turn', () => {
  const fixture = loadFixture('grok', 'stop-end-turn.json');
  const endTurn = record(fixture.end_turn);
  const produced = events(adapt({ agent: 'grok', eventName: 'Stop', payload: endTurn, capturedAt: CAPTURED_AT }));
  assert.deepEqual(produced.map((event) => event.kind), ['last_assistant_message', 'turn_end']);
  assert.equal(eventOf(produced, 'last_assistant_message').text, 'DONE');
  assert.equal(eventOf(produced, 'turn_end').reason, 'end_turn');
  assert.equal(eventOf(produced, 'turn_end').prompt_id, text(endTurn.promptId));

  const shutdown = adapt({ agent: 'grok', eventName: 'Stop', payload: record(fixture.shutdown), capturedAt: CAPTURED_AT });
  assert.equal(shutdown.kind, 'unmapped');
  if (shutdown.kind === 'unmapped') assert.equal(shutdown.reason, 'event_not_captured');

  const ended = eventOf(
    events(adapt({ agent: 'grok', eventName: 'SessionEnd', payload: record(fixture.SessionEnd), capturedAt: CAPTURED_AT })),
    'session_end',
  );
  assert.equal(ended.reason, 'shutdown');
});

test('Claude PostCompact carries the compact summary and no compaction key', () => {
  const summary = 'The session read README.md and edited notes.txt.';
  const compaction = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'PostCompact',
        payload: {
          session_id: 's-1',
          transcript_path: '/home/dev/.claude/projects/x.jsonl',
          cwd: '/repo',
          prompt_id: 'p-1',
          hook_event_name: 'PostCompact',
          trigger: 'auto',
          compact_summary: summary,
        },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'compaction_summary',
  );
  assert.equal(compaction.text, summary);
  assert.equal(compaction.compaction_key, '');
});

test('Claude Stop produces the assistant message and the turn end', () => {
  const produced = events(
    adapt({
      agent: 'claude',
      eventName: 'Stop',
      payload: {
        session_id: 's-1',
        cwd: '/repo',
        prompt_id: 'p-1',
        hook_event_name: 'Stop',
        stop_hook_active: false,
        last_assistant_message: 'DONE',
      },
      capturedAt: CAPTURED_AT,
    }),
  );
  assert.deepEqual(produced.map((event) => event.kind), ['last_assistant_message', 'turn_end']);
  assert.equal(eventOf(produced, 'last_assistant_message').text, 'DONE');
  const turnEnd = eventOf(produced, 'turn_end');
  assert.equal(turnEnd.turn_index, 0);
  assert.equal(turnEnd.reason, 'stop');
  assert.equal(turnEnd.prompt_id, 'p-1');
});

test('Claude maps session start, prompt, subagent fields and session end', () => {
  for (const source of ['startup', 'resume', 'clear', 'compact', 'fork']) {
    const started = eventOf(
      events(
        adapt({
          agent: 'claude',
          eventName: 'SessionStart',
          payload: { session_id: 's-1', cwd: '/repo', source, model: 'claude-opus-5' },
          capturedAt: CAPTURED_AT,
        }),
      ),
      'session_start',
    );
    assert.equal(started.source, source);
    assert.equal(started.model, 'claude-opus-5');
  }

  const prompt = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'UserPromptSubmit',
        payload: {
          session_id: 's-1',
          cwd: '/repo',
          prompt: 'fix the parser',
          prompt_id: 'p-1',
          agent_id: 'a-1',
          agent_type: 'general',
        },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'prompt',
  );
  assert.equal(prompt.text, 'fix the parser');
  assert.equal(prompt.input_source, 'user');
  assert.equal(prompt.prompt_id, 'p-1');
  assert.equal(prompt.agent_id, 'a-1');
  assert.equal(prompt.agent_type, 'general');

  const ended = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'SessionEnd',
        payload: { session_id: 's-1', cwd: '/repo', reason: 'prompt_input_exit' },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'session_end',
  );
  assert.equal(ended.reason, 'prompt_input_exit');
});

test('Claude renders an MCP tool input as JSON and keeps its path fields', () => {
  const call = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'PreToolUse',
        payload: {
          session_id: 's-1',
          cwd: '/repo',
          tool_name: 'mcp__oboete__search',
          tool_input: { query: 'parser', path: '/repo/src' },
          tool_use_id: 'toolu_1',
        },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'tool_call',
  );
  assert.equal(call.tool_name, 'mcp:oboete/search');
  assert.deepEqual(call.input.paths, ['/repo/src']);
  assert.equal(call.input.text, '{"query":"parser","path":"/repo/src"}');
});

test('Claude Grep keeps the path but never treats the pattern as one', () => {
  const call = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'PreToolUse',
        payload: {
          session_id: 's-1',
          cwd: '/repo',
          tool_name: 'Grep',
          tool_input: { pattern: 'adapt\\(', path: '/repo/src' },
          tool_use_id: 'toolu_2',
        },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'tool_call',
  );
  assert.equal(call.tool_name, 'grep');
  assert.deepEqual(call.input.paths, ['/repo/src']);
});

test('Pi maps a tool result to a call and a result, and rejects a broken envelope', () => {
  const fixture = loadFixture('pi', 'read.json');
  const payload = record(record(fixture.events).tool_result);
  const output = adapt({
    agent: 'pi',
    eventName: 'tool_result',
    payload: piWire('tool_result', payload, { prompt_id: 'turn-1', model: 'gpt-5.6-luna' }),
    capturedAt: CAPTURED_AT,
  });
  const produced = events(output);
  assert.deepEqual(produced.map((event) => event.kind), ['tool_call', 'tool_result']);
  const call = eventOf(produced, 'tool_call');
  assert.equal(call.prompt_id, 'turn-1');
  assert.equal(call.model, 'gpt-5.6-luna');
  assert.equal(call.tool_name, 'read');
  assert.deepEqual(call.input.paths, ['README.md']);
  const result = eventOf(produced, 'tool_result');
  assert.equal(result.tool_call_id, call.tool_call_id);
  assert.equal(result.output, 'oboete probe repository\nsecond line\n');
  assert.equal(result.is_error, false);

  // Pi subscribes to tool_result only: the input is on that event (contracts/agents.md Pi row).
  const call_only = adapt({
    agent: 'pi',
    eventName: 'tool_call',
    payload: piWire('tool_call', record(record(fixture.events).tool_call)),
    capturedAt: CAPTURED_AT,
  });
  assert.equal(call_only.kind, 'unmapped');
  if (call_only.kind === 'unmapped') assert.equal(call_only.reason, 'event_not_captured');

  assert.equal(piEnvelopeSchema.safeParse(piWire('input', { text: 'hello' })).success, true);
  assert.equal(
    piEnvelopeSchema.safeParse({ event: 'input', cwd: '/repo', payload: { text: 'hello' } }).success,
    false,
  );
  assert.equal(
    piEnvelopeSchema.safeParse({ event: 'input', session_id: '', cwd: '/repo', payload: {} }).success,
    false,
  );
});

test('Pi maps the session lifecycle events', () => {
  const started = eventOf(
    events(
      adapt({
        agent: 'pi',
        eventName: 'session_start',
        payload: piWire('session_start', { type: 'session_start', reason: 'startup' }),
        capturedAt: CAPTURED_AT,
      }),
    ),
    'session_start',
  );
  assert.equal(started.source, 'startup');

  const prompt = eventOf(
    events(
      adapt({
        agent: 'pi',
        eventName: 'input',
        payload: piWire('input', { type: 'input', text: 'fix the parser', source: 'interactive' }, { prompt_id: 'turn-1' }),
        capturedAt: CAPTURED_AT,
      }),
    ),
    'prompt',
  );
  assert.equal(prompt.text, 'fix the parser');
  assert.equal(prompt.input_source, 'user');
  assert.equal(prompt.prompt_id, 'turn-1');

  const rpc = eventOf(
    events(
      adapt({
        agent: 'pi',
        eventName: 'input',
        payload: piWire('input', { type: 'input', text: 'run it', source: 'rpc' }),
        capturedAt: CAPTURED_AT,
      }),
    ),
    'prompt',
  );
  assert.equal(rpc.input_source, 'rpc');

  const settled = events(
    adapt({
      agent: 'pi',
      eventName: 'agent_settled',
      payload: piWire('agent_settled', { type: 'agent_settled', text: 'DONE' }, { prompt_id: 'turn-1' }),
      capturedAt: CAPTURED_AT,
    }),
  );
  assert.deepEqual(settled.map((event) => event.kind), ['last_assistant_message', 'turn_end']);
  assert.equal(eventOf(settled, 'turn_end').prompt_id, 'turn-1');

  const quiet = events(
    adapt({
      agent: 'pi',
      eventName: 'agent_settled',
      payload: piWire('agent_settled', { type: 'agent_settled' }),
      capturedAt: CAPTURED_AT,
    }),
  );
  assert.deepEqual(quiet.map((event) => event.kind), ['turn_end']);

  const ended = eventOf(
    events(
      adapt({
        agent: 'pi',
        eventName: 'session_shutdown',
        payload: piWire('session_shutdown', { type: 'session_shutdown', reason: 'fork' }),
        capturedAt: CAPTURED_AT,
      }),
    ),
    'session_end',
  );
  assert.equal(ended.reason, 'fork');
});

test('Pi session_compact keys on the compaction entry id', () => {
  const compactions = record(loadFixture('pi', 'compaction.json').events).session_compact as unknown[];
  const first = record(compactions[0]);
  const entry = record(first.compactionEntry);
  const compaction = eventOf(
    events(
      adapt({
        agent: 'pi',
        eventName: 'session_compact',
        payload: piWire('session_compact', first),
        capturedAt: CAPTURED_AT,
      }),
    ),
    'compaction_summary',
  );
  assert.equal(compaction.compaction_key, text(entry.id));
  assert.equal(compaction.text, text(entry.summary));
  assert.notEqual(text(entry.id), text(record(record(compactions[1]).compactionEntry).id));
});

test('Pi keeps a tool without a fixture as metadata', () => {
  const output = adapt({
    agent: 'pi',
    eventName: 'tool_result',
    payload: piWire('tool_result', record(record(loadFixture('pi', 'oboete_probe.json').events).tool_result)),
    capturedAt: CAPTURED_AT,
  });
  assert.equal(output.kind, 'unmapped');
  if (output.kind !== 'unmapped') return;
  assert.equal(output.reason, 'unmapped_payload');
  assert.equal(output.metadata.toolName, 'oboete_probe');
  assert.equal(output.metadata.nativeSessionId, 'pi-session-1');
});

const SECRET_PATH = '/repo/secrets/prod.pem';
const SECRET_BODY = 'BEGIN RSA PRIVATE KEY fake';

test('a tool without a fixture is stored as metadata and leaks nothing', () => {
  const planted = { file_path: SECRET_PATH, content: SECRET_BODY };
  const payloads: Record<AdapterAgent, { eventName: string; payload: unknown }> = {
    claude: {
      eventName: 'PreToolUse',
      payload: {
        session_id: 'claude-session',
        cwd: '/repo',
        tool_name: 'FooTool',
        tool_input: planted,
        tool_use_id: 'toolu_9',
      },
    },
    codex: {
      eventName: 'PreToolUse',
      payload: {
        session_id: 'codex-session',
        cwd: '/repo',
        tool_name: 'FooTool',
        tool_input: planted,
        tool_use_id: 'exec-9',
      },
    },
    grok: {
      eventName: 'PreToolUse',
      payload: {
        sessionId: 'grok-session',
        cwd: '/repo',
        toolName: 'FooTool',
        toolInput: planted,
        toolUseId: 'call-9',
      },
    },
    pi: {
      eventName: 'tool_result',
      payload: {
        event: 'tool_result',
        session_id: 'pi-session',
        cwd: '/repo',
        payload: {
          type: 'tool_result',
          toolName: 'FooTool',
          toolCallId: 'call_9',
          input: planted,
          content: [{ type: 'text', text: SECRET_BODY }],
          isError: false,
        },
      },
    },
  };
  const sessionIds: Record<AdapterAgent, string> = {
    claude: 'claude-session',
    codex: 'codex-session',
    grok: 'grok-session',
    pi: 'pi-session',
  };
  for (const agent of AGENTS) {
    const { eventName, payload } = payloads[agent];
    const output = adapt({ agent, eventName, payload, capturedAt: CAPTURED_AT });
    assert.equal(output.kind, 'unmapped', agent);
    if (output.kind !== 'unmapped') continue;
    assert.equal(output.reason, 'unmapped_payload', agent);
    assert.equal(output.metadata.toolName, 'FooTool', agent);
    assert.equal(output.metadata.nativeSessionId, sessionIds[agent], agent);
    assert.equal(output.metadata.eventName, eventName, agent);
    const encoded = JSON.stringify(output);
    assert.equal(encoded.includes(SECRET_PATH), false, agent);
    assert.equal(encoded.includes(SECRET_BODY), false, agent);
  }
});

test('an unknown event name and a payload that is not an object are refused', () => {
  for (const agent of AGENTS) {
    const unknownEvent = adapt({
      agent,
      eventName: 'Notification',
      payload:
        agent === 'pi'
          ? piWire('Notification', {})
          : { session_id: 's-1', sessionId: 's-1', cwd: '/repo' },
      capturedAt: CAPTURED_AT,
    });
    assert.equal(unknownEvent.kind, 'unmapped', agent);
    if (unknownEvent.kind === 'unmapped') {
      assert.equal(unknownEvent.reason, 'event_not_captured', agent);
      assert.equal(unknownEvent.metadata.eventName, 'Notification', agent);
    }

    for (const payload of ['[]', '"text"', 'null', '42'].map((value) => JSON.parse(value) as unknown)) {
      const broken = adapt({ agent, eventName: 'PreToolUse', payload, capturedAt: CAPTURED_AT });
      assert.equal(broken.kind, 'unmapped', agent);
      if (broken.kind === 'unmapped') {
        assert.equal(broken.reason, 'payload_invalid', agent);
        assert.equal(broken.metadata.nativeSessionId, null, agent);
        assert.equal(broken.metadata.toolName, null, agent);
      }
    }

    const missingSession = adapt({
      agent,
      eventName: agent === 'pi' ? 'input' : 'UserPromptSubmit',
      payload: { cwd: '/repo', prompt: 'hello', event: 'input', payload: { text: 'hello' } },
      capturedAt: CAPTURED_AT,
    });
    assert.equal(missingSession.kind, 'unmapped', agent);
    if (missingSession.kind === 'unmapped') assert.equal(missingSession.reason, 'payload_invalid', agent);
  }
});

test('the adapters cap the text and the path list they emit', () => {
  const long = 'x'.repeat(MAX_TOOL_INPUT_TEXT + 5_000);
  const call = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'PreToolUse',
        payload: {
          session_id: 's-1',
          cwd: '/repo',
          tool_name: 'Write',
          tool_input: { file_path: '/repo/big.txt', content: long },
          tool_use_id: 'toolu_3',
        },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'tool_call',
  );
  assert.equal(call.input.text?.length, MAX_TOOL_INPUT_TEXT);

  const patch = [
    '*** Begin Patch',
    ...Array.from({ length: 60 }, (_, index) => `*** Add File: file${index}.txt\n+line`),
    '*** End Patch',
  ].join('\n');
  const patched = eventOf(
    events(
      adapt({
        agent: 'codex',
        eventName: 'PreToolUse',
        payload: {
          session_id: 's-1',
          cwd: '/repo',
          tool_name: 'apply_patch',
          tool_input: { command: patch },
          tool_use_id: 'exec-3',
        },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'tool_call',
  );
  assert.equal(patched.input.paths.length, MAX_TOOL_INPUT_PATHS);
  assert.equal(patched.input.lines_added, 60);
});

test('scanPartialPrefix recovers the session id, the tool name and the paths', () => {
  const payload = {
    session_id: '31654ffb-33da-460d-9fd7-2a0f3b21873b',
    transcript_path: '/home/dev/.claude/projects/x.jsonl',
    cwd: '/repo',
    hook_event_name: 'PostToolUse',
    tool_name: 'Read',
    tool_input: { file_path: '/repo/README.md' },
    tool_response: { type: 'text', file: { filePath: '/repo/README.md', content: 'y'.repeat(8_000) } },
  };
  const prefix = JSON.stringify(payload).slice(0, 4_096);
  assert.equal(prefix.length, 4_096);
  assert.throws(() => JSON.parse(prefix), 'the prefix must be cut inside a string');
  const scanned = scanPartialPrefix('claude', prefix);
  assert.equal(scanned.nativeSessionId, payload.session_id);
  assert.equal(scanned.toolName, 'Read');
  assert.ok(scanned.paths.includes('/repo/README.md'));

  const grok = scanPartialPrefix(
    'grok',
    JSON.stringify({
      hookEventName: 'pre_tool_use',
      sessionId: '01a067f7-94ec-7d90-9e94-7fac5f306978',
      toolName: 'read_file',
      toolInput: { target_file: 'README.md' },
    }).slice(0, 200),
  );
  assert.equal(grok.nativeSessionId, '01a067f7-94ec-7d90-9e94-7fac5f306978');
  assert.equal(grok.toolName, 'read_file');
  assert.deepEqual(grok.paths, ['README.md']);

  const garbage = scanPartialPrefix('claude', 'not json at all { "unrelated": tru');
  assert.equal(garbage.nativeSessionId, null);
  assert.equal(garbage.toolName, null);
  assert.deepEqual(garbage.paths, []);

  const many = JSON.stringify(
    Array.from({ length: 80 }, (_, index) => ({ file_path: `/repo/f${index}.ts` })),
  );
  assert.equal(scanPartialPrefix('claude', many).paths.length, MAX_TOOL_INPUT_PATHS);

  // A search pattern is not a path (contracts/agents.md size cap, A7 bounded scan).
  assert.deepEqual(scanPartialPrefix('claude', '{"pattern":"adapt","path":"/repo/src"}').paths, ['/repo/src']);
});

test('resolveAgent follows the fixed selectors', () => {
  assert.equal(resolveAgent('codex', {}), 'codex');
  assert.equal(resolveAgent('pi', {}), 'pi');
  assert.equal(resolveAgent('claude-or-grok', {}), 'claude');
  assert.equal(resolveAgent('claude-or-grok', { GROK_SESSION_ID: 's-1' }), 'grok');
  assert.equal(resolveAgent('claude-or-grok', { GROK_HOOK_EVENT: 'PreToolUse' }), 'grok');
  assert.equal(resolveAgent('claude-or-grok', { GROK_SESSION_ID: '' }), 'claude');
  assert.equal(resolveAgent(undefined, { GROK_SESSION_ID: 's-1' }), 'unknown');
  assert.equal(resolveAgent('claude', {}), 'unknown');
});

test('two Codex compactions of one session keep the turn they belong to', () => {
  const post = record(record(loadFixture('codex', 'postcompact.json').headless).post);
  const payload = record(post[0]);
  const compaction = (turnId: string): NormalizedEvent =>
    eventOf(
      events(
        adapt({
          agent: 'codex',
          eventName: 'PostCompact',
          payload: { ...payload, turn_id: turnId },
          capturedAt: CAPTURED_AT,
        }),
      ),
      'compaction_summary',
    );
  const first = compaction(text(payload.turn_id));
  const second = compaction('01a067f5-0000-7de0-aead-000000000000');
  assert.equal(first.prompt_id, text(payload.turn_id));
  // Codex sends no summary and no per-compaction key, so the turn id is what keeps the two apart.
  assert.notEqual(eventId(first), eventId(second));
});

test('Claude copies the prompt id onto the compaction, the session start and the session end', () => {
  const payload = { session_id: 's-1', cwd: '/repo', prompt_id: 'p-1' };
  const compaction = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'PostCompact',
        payload: { ...payload, compact_summary: 'summary' },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'compaction_summary',
  );
  assert.equal(compaction.prompt_id, 'p-1');

  const ended = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'SessionEnd',
        payload: { ...payload, reason: 'clear' },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'session_end',
  );
  assert.equal(ended.prompt_id, 'p-1');

  const started = eventOf(
    events(
      adapt({
        agent: 'claude',
        eventName: 'SessionStart',
        payload: { ...payload, source: 'resume' },
        capturedAt: CAPTURED_AT,
      }),
    ),
    'session_start',
  );
  assert.equal(started.prompt_id, 'p-1');
});

test('Pi copies the envelope prompt id onto its lifecycle events', () => {
  const ended = eventOf(
    events(
      adapt({
        agent: 'pi',
        eventName: 'session_shutdown',
        payload: piWire('session_shutdown', { type: 'session_shutdown', reason: 'quit' }, { prompt_id: 'turn-4' }),
        capturedAt: CAPTURED_AT,
      }),
    ),
    'session_end',
  );
  assert.equal(ended.prompt_id, 'turn-4');
});

test('a payload that names another hook is refused', () => {
  // contracts/agents.md "Normalized events": the kind comes from the fixed --event argument and is
  // cross-checked against the payload whenever one is parsed.
  const payload: Json = {
    session_id: 's-1',
    cwd: '/repo',
    hook_event_name: 'PostToolUse',
    tool_name: 'Read',
    tool_use_id: 'toolu_1',
    tool_input: { file_path: '/repo/README.md' },
    tool_response: { type: 'text', file: { filePath: '/repo/README.md', content: 'body' } },
  };
  const mismatched = adapt({ agent: 'claude', eventName: 'Stop', payload, capturedAt: CAPTURED_AT });
  assert.equal(mismatched.kind, 'unmapped');
  if (mismatched.kind === 'unmapped') {
    assert.equal(mismatched.reason, 'payload_invalid');
    assert.equal(mismatched.metadata.eventName, 'Stop');
    assert.equal(mismatched.metadata.nativeSessionId, 's-1');
    assert.equal(mismatched.metadata.toolName, 'Read');
  }

  // The matching name passes.
  assert.equal(
    adapt({ agent: 'claude', eventName: 'PostToolUse', payload, capturedAt: CAPTURED_AT }).kind,
    'events',
  );

  // Grok spells the same hook in snake_case, which is the same hook.
  const grok = adapt({
    agent: 'grok',
    eventName: 'PostToolUse',
    payload: {
      hookEventName: 'post_tool_use',
      sessionId: 's-1',
      cwd: '/repo',
      toolName: 'read_file',
      toolUseId: 'call-1',
      toolInput: { target_file: 'README.md' },
      toolResult: { FileContent: { content: 'body' } },
    },
    capturedAt: CAPTURED_AT,
  });
  assert.equal(grok.kind, 'events');

  // A Pi envelope names its own event, and a payload without the field is not judged by it.
  const pi = adapt({
    agent: 'pi',
    eventName: 'input',
    payload: piWire('agent_settled', { type: 'input', text: 'hello' }),
    capturedAt: CAPTURED_AT,
  });
  assert.equal(pi.kind, 'unmapped');
  if (pi.kind === 'unmapped') assert.equal(pi.reason, 'payload_invalid');
  assert.equal(
    adapt({
      agent: 'claude',
      eventName: 'SessionEnd',
      payload: { session_id: 's-1', cwd: '/repo', reason: 'clear' },
      capturedAt: CAPTURED_AT,
    }).kind,
    'events',
  );
});

test('scanPartialPrefix reads a JSON-escaped Windows path', () => {
  const prefix = JSON.stringify({
    session_id: 's-1',
    tool_name: 'Read',
    tool_input: { file_path: 'C:\\repo\\.env' },
  });
  const scanned = scanPartialPrefix('claude', prefix);
  assert.equal(scanned.nativeSessionId, 's-1');
  assert.equal(scanned.toolName, 'Read');
  assert.deepEqual(scanned.paths, ['C:\\repo\\.env']);
});

// -- the one text table (FR-018) ----------------------------------------------------------------

type JsonSchema = {
  type?: string;
  const?: unknown;
  enum?: unknown[];
  maxLength?: number;
  properties?: Record<string, JsonSchema>;
};

/**
 * Content strings the detector deliberately never sees, with the reason. Everything else the event
 * union declares as a capped string has to be named by `textFields`.
 */
const UNSCANNED: Record<string, string> = {
  compaction_key:
    'the native per-compaction identifier (A16): redacting it would break the epoch key',
  marker: 'the probe kind is oboete\'s own event; no adapter ever produces one',
};

/** One sample event per union member, with a recognizable value in every capped string. */
function sampleOf(schema: JsonSchema, path: string, texts: Map<string, string>): unknown {
  if (schema.const !== undefined) return schema.const;
  if (schema.enum !== undefined) return schema.enum[0];
  switch (schema.type) {
    case 'string': {
      // `text` and the capped tool-input strings carry a maximum length; an identifier does not.
      if (schema.maxLength === undefined) return `identifier-${path}`;
      const value = `content of ${path}`;
      texts.set(path, value);
      return value;
    }
    case 'integer':
    case 'number':
      return 1;
    case 'boolean':
      return true;
    // The declared paths travel as `contentForDetector.paths`, not as text.
    case 'array':
      return [];
    case 'object': {
      const sample: Record<string, unknown> = {};
      for (const [name, property] of Object.entries(schema.properties ?? {})) {
        sample[name] = sampleOf(property, path === '' ? name : `${path}.${name}`, texts);
      }
      return sample;
    }
    default:
      // `z.custom` has no JSON Schema form; the only one in the union is the normalized tool name.
      return 'other';
  }
}

test('textFields names every content string of every event kind (FR-018)', () => {
  const schema = z.toJSONSchema(eventSchema, { unrepresentable: 'any', io: 'output' }) as {
    oneOf: JsonSchema[];
  };
  assert.equal(schema.oneOf.length, 10, 'every kind of the union is enumerated');

  for (const variant of schema.oneOf) {
    const texts = new Map<string, string>();
    const sample = sampleOf(variant, '', texts);
    const parsed = eventSchema.safeParse(sample);
    assert.equal(parsed.success, true, `the sample is not an event: ${JSON.stringify(sample)}`);
    const event = parsed.data as NormalizedEvent;

    const expected = new Set(
      [...texts].filter(([path]) => !(path.split('.').pop() as string in UNSCANNED)).map(([, value]) => value),
    );
    const scanned = new Set(textFields(event).map((field) => field.read()));
    for (const value of expected) {
      assert.ok(scanned.has(value), `${event.kind}: ${value} reaches storage unscanned`);
    }
    for (const value of scanned) {
      assert.ok(expected.has(value), `${event.kind}: ${value} is scanned but is not a content string`);
    }
  }
});

test('every text field writes its redacted value back where it came from', () => {
  const event: NormalizedEvent = {
    agent: 'claude',
    native_session_id: 'session-1',
    cwd: '/repo',
    captured_at: CAPTURED_AT,
    kind: 'tool_call',
    tool_call_id: 'call-1',
    tool_name_native: 'Bash',
    tool_name: 'bash',
    input: { paths: [], command: 'echo one', text: 'two' },
  };

  for (const field of textFields(event)) field.write(`[REDACTED] ${field.read()}`);

  assert.equal(event.input.command, '[REDACTED] echo one');
  assert.equal(event.input.text, '[REDACTED] two');
});
