import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, realpathSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { runInject } from '../../src/injection/pi.js';
import { runGet, runSearch } from '../../src/memories-cli.js';
import type { ObserverInput } from '../../src/observer/contract.js';
import { detectSync } from '../../src/privacy/detect.js';
import { resolveRepoIdentity, type GitSpawn } from '../../src/repo-identity.js';
import { chooseWork } from '../../src/work.js';
import { stdoutOf } from '../helpers/inject-fixture.js';
import { git } from '../helpers/git.js';
import { DAY, NOW, cleanEnv, openAiResponse, providerOutput, runObserveForFixture, withFixture, writeConfig,
  type Fixture, type Json } from '../helpers/observe.js';

const AGENTS = ['claude', 'codex', 'grok', 'pi'] as const;
type Agent = typeof AGENTS[number];
type Session = { agent: Agent; id: string; cwd: string };
const WORKS = {
  X: { purpose: 'Repair the upload retry.', checkpoint: 'X checkpoint: verify the upload timeout before deployment.',
    outstanding: 'Outstanding for X: verify the upload timeout before deployment.',
    pending: 'Pending for X: investigate the upload timeout before deployment.' },
  Y: { purpose: 'Improve the search index.', checkpoint: 'Y checkpoint: rebuild the search catalogue before release.',
    outstanding: 'Outstanding for Y: rebuild the search catalogue before release.',
    pending: 'Pending for Y: investigate the search catalogue before release.' },
  Z: { purpose: 'Document the import format.', checkpoint: 'Z checkpoint: validate the migration examples before publication.',
    outstanding: 'Outstanding for Z: validate the migration examples before publication.',
    pending: 'Pending for Z: investigate the migration examples before publication.' },
};
type WorkKey = keyof typeof WORKS;
type Corpus = Awaited<ReturnType<typeof parallelCorpus>>;
const WORK_KEYS = ['X', 'Y', 'Z'] as const;
// Every checkpoint is lexically relevant; work selection must exclude the two siblings.
const CONTINUATION_PROMPT = 'Continue from the last checkpoint: verify the upload timeout, rebuild the search catalogue and validate the migration examples before deployment, release or publication.';

function binding(fixture: Fixture, session: Session) {
  return fixture.withDb((db) => {
    const row = db.prepare(`SELECT b.id, b.work_id, b.context_id, b.candidates_json, s.id AS session_id,
      s.agent, s.conversation_id, w.purpose, c.repo_id FROM work_bindings b
      JOIN sessions s ON s.id = b.session_id JOIN work_contexts c ON c.id = b.context_id
      LEFT JOIN work_items w ON w.id = b.work_id
      WHERE s.agent = ? AND s.native_session_id = ? AND b.closed_at IS NULL`).get(session.agent, session.id);
    assert.ok(row, `${session.agent} native session ${session.id} has a current binding`);
    return row;
  });
}

async function memoryCli(command: typeof runSearch, argv: string[], cwd: string): Promise<string> {
  let stdout = '', stderr = '';
  const status = await command(argv, { cwd, now: () => NOW,
    writeOut: (text) => { stdout += text; }, writeError: (text) => { stderr += text; } });
  assert.equal(status, 0, stderr);
  return stdout;
}

function assertCheckpointPack(fixture: Fixture, session: Session, corpus: Corpus, pack: string, selected: WorkKey | null) {
  if (selected === null) {
    assert.match(pack, /Select which work/, 'the ambiguous start actually supplies a choice');
    assert.ok(pack.includes(String(corpus.bindings.X.work_id)), 'the choice pack lists work X');
    assert.ok(pack.includes(String(corpus.bindings.Y.work_id)), 'the choice pack lists work Y');
    assert.equal(pack.includes(String(corpus.bindings.Z.work_id)), false, 'the choice pack excludes worktree B work Z');
  }
  else assert.ok(pack.includes(WORKS[selected].outstanding), `selected work ${selected} includes its generated checkpoint`);
  for (const key of WORK_KEYS.filter((key) => key !== selected)) {
    assert.equal(pack.includes(WORKS[key].outstanding), false, `work ${key} checkpoint is withheld`);
    assert.equal(pack.includes(WORKS[key].checkpoint), false, `work ${key} raw activity is withheld`);
    assert.equal(pack.includes(WORKS[key].pending), false, `work ${key} pending activity is withheld`);
    assert.equal(pack.includes(`Investigated: ${WORKS[key].purpose}`), false, `work ${key} observation is withheld`);
  }
  const conversation = binding(fixture, session).conversation_id;
  fixture.withDb((db) => {
    const rows = db.prepare(`SELECT i.memory_id, v.work_id FROM injection_items i
      JOIN memory_visibility v ON v.memory_id = i.memory_id AND v.audience = 'work'
      WHERE i.conversation_id = ? AND v.work_id IN (?, ?, ?)
        AND (i.decision = 'included' OR (? AND i.decision = 'planned'))`)
      .all(conversation, corpus.bindings.X.work_id, corpus.bindings.Y.work_id, corpus.bindings.Z.work_id, selected === null ? 1 : 0);
    if (selected === null) assert.deepEqual(rows, [], 'ambiguous delivery includes no work memory');
    else {
      assert.ok(rows.some((row) => row.memory_id === corpus.checkpoints[selected]), 'the selected checkpoint memory is included');
      assert.ok(rows.every((row) => row.work_id === corpus.bindings[selected].work_id), 'the delivery ledger includes no sibling work memory');
    }
  });
}

function nativeHook(session: Session, stage: 'start' | 'prompt' | 'end', promptId: string, text: string): [string, Json] {
  const { agent, id, cwd } = session;
  if (agent === 'pi') {
    const event = { start: 'session_start', prompt: 'input', end: 'session_shutdown' }[stage];
    const payload = stage === 'prompt' ? { text, source: 'interactive' } : { reason: stage === 'start' ? 'startup' : 'exit' };
    return [event, { event, session_id: id, cwd, model: 'gpt-5.6-luna', prompt_id: promptId, payload }];
  }
  const event = { start: 'SessionStart', prompt: 'UserPromptSubmit', end: 'SessionEnd' }[stage];
  if (agent === 'grok') {
    const payload = { start: { source: 'new' }, prompt: { prompt: text, promptId }, end: { reason: 'exit' } }[stage];
    return [event, { sessionId: id, cwd, ...payload }];
  }
  const prompt = agent === 'codex' ? { prompt: text, turn_id: promptId } : { prompt: text, prompt_id: promptId };
  const payload = { start: { source: 'startup' }, prompt, end: { reason: 'prompt_input_exit' } }[stage];
  return [event, { session_id: id, cwd, model: agent === 'claude' ? 'claude-opus-5[1m]' : 'gpt-5.6-luna', ...payload }];
}

async function parallelCorpus(fixture: Fixture, seed: Agent, label: string, selected: 'X' | 'Y' = 'X') {
  fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'synthetic-work-pairs-fixture-key' });
  writeConfig(fixture, 'openrouter');
  const main = join(fixture.home, 'main'), a = join(fixture.home, 'worktree-a'), b = join(fixture.home, 'worktree-b');
  mkdirSync(main);
  git(main, 'init', '--quiet', '--initial-branch=main');
  git(main, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '--allow-empty', '-m', 'Fixture');
  git(main, 'worktree', 'add', '--quiet', '-b', 'work-a', a);
  git(main, 'worktree', 'add', '--quiet', '-b', 'work-b', b);
  assert.equal(git(main, 'remote'), '');
  // Snapshot only capture's own identity lookup, outside its 120/250 ms budget.
  // Pack privacy checks, Pi injection and the memory CLI still use real Git.
  const roots = new Map([a, b].map((cwd) => [cwd, git(cwd, 'rev-parse', '--show-toplevel', '--git-common-dir', '--absolute-git-dir')]));
  const gitSpawn: GitSpawn = (file, args) => {
    assert.equal(file, 'git');
    const root = roots.get(args[1]);
    assert.ok(root, 'Git discovery is confined to the fixture worktrees');
    const command = args.slice(2).join(' ');
    assert.ok(['rev-parse --show-toplevel --git-common-dir --absolute-git-dir', 'remote get-url origin', 'remote'].includes(command));
    return { pid: 0, output: [], stdout: command.startsWith('rev-parse') ? root : '', stderr: '',
      status: command === 'remote get-url origin' ? 2 : 0, signal: null };
  };
  const others = AGENTS.filter((agent) => agent !== seed);
  const sessions = {
    X: { agent: selected === 'X' ? seed : others[0], id: `${label}-work-X`, cwd: a },
    Y: { agent: selected === 'Y' ? seed : others[0], id: `${label}-work-Y`, cwd: a },
    Z: { agent: others[1], id: `${label}-${others[1]}-other-Z`, cwd: b },
  };
  let capturedAt = NOW - DAY;
  const capture = (session: Session, eventName: string, payload: Json) => fixture.capture(eventName, payload, 'stored',
    { agent: session.agent, deps: { now: () => ++capturedAt, gitSpawn } });
  const event = async (session: Session, stage: 'start' | 'prompt' | 'end', text = '') => {
    const promptId = randomUUID();
    const [eventName, payload] = nativeHook(session, stage, promptId, text);
    return { ...await capture(session, eventName, payload), promptId };
  };
  for (const key of WORK_KEYS) {
    await event(sessions[key], 'start');
    await event(sessions[key], 'prompt', `New task: ${WORKS[key].purpose}`);
  }
  for (const key of ['X', 'Z', 'Y'] as const) await event(sessions[key], 'prompt', WORKS[key].checkpoint);
  for (const key of ['X', 'Z', 'Y'] as const) await event(sessions[key], 'end');

  const bindings = { X: binding(fixture, sessions.X), Y: binding(fixture, sessions.Y), Z: binding(fixture, sessions.Z) };
  assert.equal(bindings.X.context_id, bindings.Y.context_id);
  assert.notEqual(bindings.X.context_id, bindings.Z.context_id);
  assert.equal(new Set(Object.values(bindings).map((row) => row.repo_id)).size, 1);
  assert.equal(new Set(Object.values(bindings).map((row) => row.work_id)).size, 3);
  fixture.withDb((db) => {
    const prompts = db.prepare(`SELECT r.content, r.agent, r.work_binding_id, s.native_session_id FROM raw_events r
      JOIN sessions s ON s.id = r.session_id WHERE r.kind = 'prompt' ORDER BY r.captured_at`).all();
    assert.deepEqual(prompts.map((row) => row.content), [WORKS.X.purpose, WORKS.Y.purpose, WORKS.Z.purpose]
      .map((purpose) => `New task: ${purpose}`).concat([WORKS.X.checkpoint, WORKS.Z.checkpoint, WORKS.Y.checkpoint]));
    for (const key of WORK_KEYS) {
      assert.equal(bindings[key].purpose, WORKS[key].purpose);
      const sources = prompts.filter((row) => row.native_session_id === sessions[key].id);
      assert.equal(sources.length, 2);
      assert.ok(sources.every((row) => row.agent === sessions[key].agent && row.work_binding_id === bindings[key].id));
    }
  });

  const violations: string[] = [];
  const requests: { work: WorkKey; checkpoint_context: WorkKey[]; nearby: WorkKey[] }[] = [];
  const respond: typeof fetch = async (_url, options) => {
    const messages = (JSON.parse(String(options?.body)) as { messages: { role: string; content: string }[] }).messages;
    const input = JSON.parse(messages.find((message) => message.role === 'user')!.content) as ObserverInput;
    const matches = WORK_KEYS.filter((key) => input.events.some((row) => row.text === WORKS[key].checkpoint));
    if (matches.length !== 1) {
      violations.push(`observer request matched ${matches.length} works instead of one`);
      return new Response('{}', { status: 400 });
    }
    const key = matches[0], work = WORKS[key];
    const siblings = WORK_KEYS.filter((other) => other !== key);
    const leaked = (value: unknown) => siblings.filter((other) => [WORKS[other].checkpoint, WORKS[other].outstanding]
      .some((text) => JSON.stringify(value).includes(text)));
    const request = { work: key, checkpoint_context: leaked(input.checkpoint_context), nearby: leaked(input.nearby) };
    requests.push(request);
    for (const field of ['checkpoint_context', 'nearby'] as const) {
      if (request[field].length > 0) violations.push(`${key} request ${requests.length} ${field} contains ${request[field].join(',')}`);
    }
    const sourceIds = input.events.map((row) => row.id);
    const output = providerOutput(sourceIds[0]);
    output.observations[0] = { ...output.observations[0], visibility: 'work', title: `${key} investigation`,
      body: `Investigated: ${work.purpose}`, source_event_ids: sourceIds };
    output.checkpoint = { decision: 'replace', purpose: work.purpose, constraints: [], decisions: [],
      outstanding: [work.outstanding], source_event_ids: sourceIds, reason: 'Retain the outstanding verification.' };
    return openAiResponse(output);
  };
  const observed = await runObserveForFixture(fixture, { fetch: respond });
  assert.deepEqual(violations, [], 'observer requests contain no sibling checkpoint context or nearby checkpoint');
  assert.equal(observed, 0);
  assert.deepEqual(Object.fromEntries(WORK_KEYS.map((key) => [key, requests.filter((request) => request.work === key).length])),
    { X: 1, Y: 1, Z: 1 }, 'each work is generated exactly once');
  const checkpoints = fixture.withDb((db) => Object.fromEntries(Object.entries(bindings).map(([key, row]) => {
    const checkpoint = db.prepare(`SELECT m.id, m.body, w.state FROM work_items w
      JOIN memories m ON m.id = w.current_checkpoint_memory_id WHERE w.id = ?`).get(row.work_id);
    assert.ok(checkpoint, `${key} has a generated checkpoint`);
    assert.equal(checkpoint.state, 'active', 'session end does not complete outstanding work');
    assert.ok(String(checkpoint.body).includes(WORKS[key as WorkKey].outstanding));
    return [key, String(checkpoint.id)];
  })));
  capturedAt = NOW;
  for (const key of WORK_KEYS) await event(sessions[key], 'prompt', WORKS[key].pending);
  fixture.withDb((db) => {
    for (const key of WORK_KEYS) {
      const pending = db.prepare('SELECT processing_state, work_binding_id FROM raw_events WHERE content = ?').get(WORKS[key].pending);
      assert.equal(pending?.processing_state, 'pending', `${key} has unprocessed activity for the receiver`);
      assert.equal(pending.work_binding_id, bindings[key].id);
    }
  });
  return { main, a, b, sessions, bindings, checkpoints, capture, event, gitSpawn, roots };
}

async function receiverPack(fixture: Fixture, corpus: Corpus, session: Session, stage: 'start' | 'prompt', captured: { stdout?: string; promptId: string }) {
  const pack = captured.stdout ?? '';
  if (session.agent === 'grok') {
    assert.equal(pack, '', 'Grok defers delivery until a native tool hook');
    if (stage === 'start') {
      const conversation = binding(fixture, session).conversation_id;
      const pending = fixture.withDb((db) => db.prepare('SELECT value_json FROM runtime_state WHERE key = ?')
        .get(`injection_pending:${String(conversation)}`));
      assert.ok(pending, 'Grok has a pending choice pack');
      return String(JSON.parse(String(pending.value_json)).text);
    }
    const tool = { sessionId: session.id, cwd: session.cwd,
      toolUseId: `${session.id}-read`, toolName: 'read_file', toolInput: { target_file: 'README.md' } };
    const attached = await corpus.capture(session, 'PreToolUse', tool);
    const envelope = JSON.parse(attached.stdout ?? '');
    assert.equal(envelope.hookSpecificOutput.hookEventName, 'PreToolUse');
    await corpus.capture(session, 'PostToolUse', { ...tool, toolResult: { FileContent: { content: 'Synthetic read result.' } } });
    return String(envelope.hookSpecificOutput.additionalContext);
  }
  if (session.agent === 'codex') {
    const envelope = JSON.parse(pack);
    assert.equal(envelope.hookSpecificOutput.hookEventName, stage === 'start' ? 'SessionStart' : 'UserPromptSubmit');
    return String(envelope.hookSpecificOutput.additionalContext);
  }
  if (session.agent === 'pi') {
    const capturedAt = fixture.withDb((db) => Number(db.prepare('SELECT MAX(captured_at) AS at FROM raw_events').get()?.at));
    return stdoutOf(() => runInject(['--agent', 'pi', '--kind', stage], {
      readStdin: () => JSON.stringify({ cwd: session.cwd, session_id: session.id, model: 'gpt-5.6-luna',
        ...(stage === 'prompt' ? { prompt: CONTINUATION_PROMPT, prompt_id: captured.promptId } : {}) }),
      now: () => capturedAt, elapsedMs: () => 0,
      ...(stage === 'prompt' ? { detect: (input: Parameters<typeof detectSync>[0]) => detectSync(input) } : {}),
    }));
  }
  return pack;
}

// One fresh corpus per pair. Alternation selects the older X six times and newer Y six times.
const PAIRS = AGENTS.flatMap((seed) => AGENTS.filter((receiver) => receiver !== seed).map((receiver) => ({ seed, receiver })));
for (const [index, { seed, receiver }] of PAIRS.entries()) {
  const selected: 'X' | 'Y' = index % 2 === 0 ? 'X' : 'Y';
  test(`synthetic ${seed} -> ${receiver}: ambiguous start withholds checkpoints, explicit ${selected} delivers only its checkpoint`, async () => {
    await withFixture(async (fixture) => {
      const corpus = await parallelCorpus(fixture, seed, `${seed}-to-${receiver}`, selected);
      const session: Session = { agent: receiver, id: `${seed}-to-${receiver}-fresh-receiver`, cwd: corpus.a };
      const started = await corpus.event(session, 'start');
      const current = binding(fixture, session);
      assert.equal(current.work_id, null, 'two purposes require an explicit choice');
      assert.deepEqual(new Set(JSON.parse(String(current.candidates_json))), new Set([corpus.bindings.X.work_id, corpus.bindings.Y.work_id]));
      assert.equal(corpus.bindings[selected].agent, seed, 'the selected work was captured by the pair seed agent');
      assert.notEqual(current.conversation_id, corpus.bindings[selected].conversation_id, 'the receiver does not reuse the seed conversation');
      assert.equal(current.conversation_id, current.session_id, 'the receiver has its own fresh native conversation');
      const ambiguous = await receiverPack(fixture, corpus, session, 'start', started);
      assertCheckpointPack(fixture, session, corpus, ambiguous, null);
      const identity = resolveRepoIdentity(corpus.a, { spawn: corpus.gitSpawn });
      fixture.withDb((db) => assert.ok(chooseWork(db, { repoId: identity.id, contextKey: identity.worktreeKey,
        bindingId: String(current.id), workId: String(corpus.bindings[selected].work_id),
        now: Number(db.prepare('SELECT MAX(captured_at) AS at FROM raw_events').get()?.at) })));
      assert.equal(binding(fixture, session).work_id, corpus.bindings[selected].work_id);
      const continued = await corpus.event(session, 'prompt', CONTINUATION_PROMPT);
      const pack = await receiverPack(fixture, corpus, session, 'prompt', continued);
      assertCheckpointPack(fixture, session, corpus, pack, selected);
    });
  });
}

test('removed worktree: search and explicit continuation retain Z checkpoint and provenance without recreating the directory', async () => {
  await withFixture(async (fixture) => {
    const corpus = await parallelCorpus(fixture, 'claude', 'removed-worktree');
    const originalPath = realpathSync(corpus.b);
    git(corpus.main, 'worktree', 'remove', corpus.b);
    corpus.roots.delete(corpus.b);
    assert.equal(existsSync(corpus.b), false);
    // History includes older revisions, but still requires an explicit work scope in this ambiguous context.
    const reader: Session = { agent: 'claude', id: 'removed-worktree-history-reader', cwd: corpus.a };
    await corpus.event(reader, 'start');
    const current = binding(fixture, reader);
    const identity = resolveRepoIdentity(corpus.a, { spawn: corpus.gitSpawn });
    const selected = fixture.withDb((db) => chooseWork(db, { repoId: identity.id, contextKey: identity.worktreeKey,
      bindingId: String(current.id), workId: String(corpus.bindings.Z.work_id),
      now: Number(db.prepare('SELECT MAX(captured_at) AS at FROM raw_events').get()?.at) }));
    assert.ok(selected);
    const continued = await corpus.event(reader, 'prompt', CONTINUATION_PROMPT);
    assertCheckpointPack(fixture, reader, corpus, continued.stdout ?? '', 'Z');
    const history = ['--binding', selected.id, '--history', '--json'];
    const searched = JSON.parse(await memoryCli(runSearch, ['migration examples', ...history], corpus.a));
    const found = searched.memories.find((row: { id: string }) => row.id === corpus.checkpoints.Z);
    assert.ok(found, 'CLI history search returns the removed worktree checkpoint');
    assert.ok(found.body.includes(WORKS.Z.outstanding));
    // Search returns ranked text; get on that search hit is the CLI's provenance reader.
    const retained = JSON.parse(await memoryCli(runGet, [found.id, ...history], corpus.a));
    assert.equal(retained.work_id, corpus.bindings.Z.work_id);
    assert.ok(retained.body.includes(WORKS.Z.outstanding));
    assert.ok(retained.sources.length > 0);
    assert.ok(retained.sources.every((source: { source_agent: string | null }) => source.source_agent === corpus.sessions.Z.agent),
      'every public source of Z checkpoint belongs to Z agent');
    // Public source arrays omit capture_root; correlate the actual search hit with stored provenance.
    fixture.withDb((db) => {
      const sources = db.prepare(`SELECT source_agent, capture_root, source_context_id FROM memory_sources
        WHERE memory_id = ? AND context_only = 0`).all(found.id);
      assert.ok(sources.length > 0);
      assert.ok(sources.every((source) => source.source_agent === corpus.sessions.Z.agent
        && source.capture_root === originalPath && source.source_context_id === corpus.bindings.Z.context_id),
      'every checkpoint source retains Z agent, original worktree path and source context');
    });
    assert.equal(existsSync(corpus.b), false, 'search and provenance retrieval do not recreate the worktree');
  });
});
