import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { appendFileSync } from 'node:fs';
import { test } from 'node:test';

import { validateObserverOutput, type CheckpointChoice, type ObserverInput } from '../../src/observer/contract.js';
import { applyObservations } from '../../src/observer/apply.js';
import { buildObserverRequest } from '../../src/observer/request.js';
import { loadDestinationRules } from '../../src/privacy/egress.js';
import { excludeSecretSource } from '../../src/worker/batches.js';
import { detectSync } from '../../src/privacy/detect.js';
import type { RawEventRow, SessionRow } from '../../src/worker/batches.js';
import { NOW, REPO_ID, nearbySnapshot, seedBatch, seedEvent, seedRepo, seedSession, withOpened } from '../helpers/observer-fixture.js';
import { NOW as WORKER_NOW, DAY, captureEndedSession, cleanEnv, openAiResponse, providerOutput,
  runObserveForFixture, withFixture, writeConfig } from '../helpers/observe.js';

const context = {
  events: [{ id: 'p1', kind: 'prompt' as const, text: 'Continue the uploader.' }],
  nearby: [], checkpoint_context: { state: 'none' as const },
};
const replacement = {
  decision: 'replace' as const, purpose: 'Reliable uploads',
  constraints: ['Keep the public API stable.'], decisions: ['Use the existing retry helper.'],
  outstanding: ['Verify the interrupted-upload case.'], source_event_ids: ['p1'], reason: 'Progress changed.',
};
const detect = (text: string) => detectSync({ text, paths: [], repoRoot: null, secretPaths: [] });

test('provider output requires a bounded checkpoint decision with admitted sources', () => {
  assert.equal(validateObserverOutput({ observations: [] }, context).ok, false);
  const valid = validateObserverOutput({ observations: [], checkpoint: replacement }, context);
  assert.equal(valid.ok, true);
  if (valid.ok) assert.deepEqual(valid.output.checkpoint, replacement);
  for (const checkpoint of [
    { ...replacement, source_event_ids: [] },
    { ...replacement, source_event_ids: ['foreign'] },
    { ...replacement, purpose: 'x'.repeat(121) },
    { ...replacement, constraints: Array.from({ length: 5 }, () => 'x'.repeat(500)) },
    { ...replacement, reason: '  ' },
  ]) assert.equal(validateObserverOutput({ observations: [], checkpoint }, context).ok, false);
});

for (const change of ['pointer', 'provenance']) test(`a changed checkpoint ${change} cancels the actual provider send`, async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'checkpoint-send-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const capture = async (sessionId: string) => {
      await captureEndedSession(fixture, { sessionId, cwd: fixture.home, prompts: [`Continue the ${sessionId} check.`] });
      fixture.withDb((db) => db.exec("UPDATE raw_events SET sensitivity = 'eligible' WHERE kind = 'prompt'"));
    };
    await capture('earlier');
    assert.equal(await runObserveForFixture(fixture, { fetch: async (_url, options) => {
      const body = JSON.parse(String(options?.body)) as { messages: { role: string; content: string }[] };
      const input = JSON.parse(body.messages.find((message) => message.role === 'user')!.content) as ObserverInput;
      return openAiResponse({ ...providerOutput(input.events[0].id), checkpoint: { ...replacement,
        source_event_ids: input.events.map((event) => event.id) } });
    } }), 0);
    await capture('later');
    let changed = false;
    let calls = 0;
    assert.equal(await runObserveForFixture(fixture, {
      detect: async (input) => {
        const checked = await detectSync(input);
        if (!changed && input.text.startsWith('{"repo_ref"') && input.text.includes('"state":"provided"')) {
          changed = true;
          fixture.withDb((db) => {
            if (change === 'pointer') db.exec('UPDATE work_items SET current_checkpoint_memory_id = NULL');
            else db.exec(`UPDATE memory_sources SET source_paths_json = '["unchecked/path.txt"]'
              WHERE raw_event_id IS NULL AND citation_kind IS NULL`);
          });
        }
        return checked;
      },
      fetch: async () => { calls += 1; assert.fail('the checkpoint changed after validation'); },
    }), change === 'pointer' ? 0 : 1);
    assert.equal(changed, true);
    assert.equal(calls, 0);
    fixture.withDb((db) => assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE kind = 'prompt' AND processing_state <> 'processed'").get()?.n, 1));
  });
});

test('inherited source privacy reaches every revision without mutable parent links', async () => {
  await withOpened(async (db, token) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    const first = (await advance(db, token, 'root', null, NOW - 3, replacement)).checkpoint!.memoryId;
    const middle = (await advance(db, token, 'middle', first, NOW - 2, { ...replacement, outstanding: ['Second step.'] })).checkpoint!.memoryId;
    const last = (await advance(db, token, 'last', middle, NOW - 1, { ...replacement, outstanding: ['Third step.'] })).checkpoint!.memoryId;
    db.prepare("UPDATE memories SET sensitivity = 'private' WHERE id = ?").run(first);
    assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(last)?.sensitivity, 'private');
    db.prepare("UPDATE memories SET sensitivity = 'local_only' WHERE id = ?").run(first);
    assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(last)?.sensitivity, 'private');
    assert.throws(() => db.prepare('UPDATE memories SET checkpoint_parent_id = ? WHERE id = ?').run(last, first), /immutable/);
    excludeSecretSource(db, 'root', NOW);
    assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memories WHERE sensitivity = 'secret' AND review_state = 'imported'").get()?.n, 3);
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE evidence IS NOT NULL OR capture_root IS NOT NULL OR source_paths_json IS NOT NULL').get()?.n, 0);
  });
});

for (const restricted of ['private', 'inherited_path']) test(`a worker continues after raw retention and withholds a ${restricted} parent`, async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'checkpoint-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const capture = async (sessionId: string) => {
      await captureEndedSession(fixture, { sessionId, cwd: fixture.home, prompts: [`Continue uploader work in ${sessionId}.`],
        ...(sessionId === 'initial' ? { tools: [{ id: 'origin-file', path: 'protected/old.txt' }] } : {}) });
      fixture.withDb((db) => db.exec("UPDATE raw_events SET sensitivity = 'eligible', classification_state = 'done' WHERE kind IN ('prompt', 'tool_call')"));
    };
    const fullChoice = { ...replacement, constraints: ['CHECKPOINT_BOUNDARY Preserve all API callers. '.repeat(10)] };
    let phase = 0;
    let parentId = '';
    let parentBody = '';
    const respond: typeof fetch = async (_url, options) => {
      const body = JSON.parse(String(options?.body)) as { messages: { role: string; content: string }[] };
      const input = JSON.parse(body.messages.find((message) => message.role === 'user')!.content) as ObserverInput;
      assert.ok(input.nearby.every((item) => item.type !== 'session_summary'));
      if (phase === 0) assert.deepEqual(input.checkpoint_context, { state: 'none' });
      if (phase === 1) assert.deepEqual(input.checkpoint_context, {
        state: 'provided', id: parentId, title: fullChoice.purpose, body: parentBody,
      });
      if (phase === 2) {
        assert.deepEqual(input.checkpoint_context, { state: 'withheld' });
        assert.ok(!JSON.stringify(input).includes('CHECKPOINT_BOUNDARY'));
      }
      const output = providerOutput(input.events[0].id);
      output.observations[0].source_event_ids = input.events.map((event) => event.id);
      output.checkpoint = phase === 2 ? { decision: 'unchanged', source_event_ids: input.events.map((event) => event.id),
        reason: 'The previous checkpoint is unavailable.' }
        : { ...fullChoice, outstanding: phase === 0 ? fullChoice.outstanding : ['Review the final local check.'],
          source_event_ids: input.events.map((event) => event.id) };
      return openAiResponse(output);
    };
    await capture('initial');
    assert.equal(await runObserveForFixture(fixture, { fetch: respond }), 0);
    fixture.withDb((db) => {
      const checkpoint = db.prepare(`SELECT m.* FROM work_items w JOIN memories m ON m.id = w.current_checkpoint_memory_id`).get()!;
      assert.ok(checkpoint);
      parentId = String(checkpoint.id);
      parentBody = String(checkpoint.body);
      assert.ok(parentBody.length > 500);
      // An older plain link alongside retained independent evidence must not erase its origin.
      db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id)
        SELECT memory_id, raw_event_id FROM memory_sources WHERE memory_id = ? LIMIT 1`).run(parentId);
    });
    assert.equal(await runObserveForFixture(fixture, { now: () => WORKER_NOW + 31 * DAY }), 0);
    fixture.withDb((db) => assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE kind = 'prompt'").get()?.n, 0));
    phase = 1;
    await capture('continued');
    assert.equal(await runObserveForFixture(fixture, { now: () => WORKER_NOW + 31 * DAY + 1, fetch: respond }), 0);
    fixture.withDb((db) => {
      const current = db.prepare('SELECT current_checkpoint_memory_id FROM work_items').get()!.current_checkpoint_memory_id;
      assert.notEqual(current, parentId);
      if (restricted === 'private') db.prepare("UPDATE memories SET sensitivity = 'private' WHERE id = ?").run(current);
      parentId = String(current);
    });
    if (restricted === 'inherited_path') appendFileSync(fixture.paths.config, '\n[privacy]\nsecret_paths = ["protected/old.txt"]\n');
    phase = 2;
    await capture('private-parent');
    assert.equal(await runObserveForFixture(fixture, { now: () => WORKER_NOW + 31 * DAY + 2, fetch: respond }), 0);
    fixture.withDb((db) => assert.equal(db.prepare('SELECT current_checkpoint_memory_id FROM work_items').get()?.current_checkpoint_memory_id, parentId));
  });
});

async function advance(db: DatabaseSync, token: string, id: string, parent: string | null, capturedAt: number,
  choice: CheckpointChoice, detector = detect) {
  seedEvent(db, { id, content: `Progress for ${id}.`, payload: { capture_root: '/fixture', source_paths: [] } });
  seedBatch(db, `batch-${id}`);
  db.prepare('UPDATE observation_batches SET checkpoint_parent_id = ? WHERE id = ?').run(parent, `batch-${id}`);
  db.prepare('UPDATE raw_events SET batch_id = ?, captured_at = ? WHERE id = ?').run(`batch-${id}`, capturedAt, id);
  const rows = db.prepare('SELECT * FROM raw_events WHERE id = ?').all(id) as unknown as RawEventRow[];
  const request = buildObserverRequest({ rows, session: db.prepare('SELECT * FROM sessions').get() as unknown as SessionRow,
    turns: [], repoId: REPO_ID, destination: 'remote_observer', nearby: [], rules: loadDestinationRules(db) });
  db.prepare('UPDATE raw_events SET processing_hash = ? WHERE id = ?').run(request.coverage[0].sourceHash, id);
  return await applyObservations(db, token, { batchId: `batch-${id}`, repoId: REPO_ID, sessionId: 'sess1', rows,
    nearby: [], output: { checkpoint: { ...choice, source_event_ids: [id] }, observations: [{
      type: 'change', visibility: 'work', title: 'Routine progress', body: '', concepts: [],
      citations: { files_read: [], files_modified: [], commits: [] }, source_event_ids: [id],
      classification: { decision: 'noop', target: null, reason: 'No independent reusable fact.' },
    }] }, fallbackReason: null, coverage: request.coverage,
    providedCheckpoint: parent === null ? undefined : nearbySnapshot(db, parent), detect: detector, now: NOW });
}

test('an unchanged checkpoint retains its cited source without adding parent evidence', async () => {
  await withOpened(async (db, token) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    const first = (await advance(db, token, 'first', null, NOW - 2, replacement)).checkpoint!;
    const result = await advance(db, token, 'unchanged', first.memoryId, NOW - 1,
      { decision: 'unchanged', source_event_ids: ['unchanged'], reason: 'No progress changed.' });
    assert.deepEqual(result.checkpoint?.sourceIds, ['unchanged']);
    assert.equal(db.prepare('SELECT checkpoint_source_ids_json FROM observation_batches WHERE id = ?')
      .get('batch-unchanged')?.checkpoint_source_ids_json, '["unchanged"]');
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE raw_event_id = ?').get('unchanged')?.n, 0);
    assert.equal(db.prepare('SELECT current_checkpoint_memory_id FROM work_items').get()?.current_checkpoint_memory_id, first.memoryId);
  });
});

for (const provided of [true, false]) test(`a changed ${provided ? 'provided checkpoint cancels generation' : 'withheld checkpoint does not taint generation'}`, async () => {
  await withOpened(async (db, token) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    const parent = (await advance(db, token, 'parent', null, NOW - 2, replacement)).checkpoint!.memoryId!;
    db.prepare(`UPDATE memory_sources SET source_paths_json = '["protected/parent.ts"]'
      WHERE memory_id = ? AND raw_event_id IS NULL AND source_memory_id IS NULL`).run(parent);
    const before = db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ?').get(parent)?.n;
    seedEvent(db, { id: 'ordinary', payload: { capture_root: '/fixture', source_paths: [] } });
    seedBatch(db, 'ordinary');
    db.prepare("UPDATE observation_batches SET checkpoint_parent_id = ? WHERE id = 'ordinary'").run(parent);
    const rows = db.prepare("SELECT * FROM raw_events WHERE id = 'ordinary'").all() as unknown as RawEventRow[];
    const request = buildObserverRequest({ rows, nearby: [], repoId: REPO_ID, destination: 'remote_observer', turns: [],
      session: db.prepare('SELECT * FROM sessions').get() as unknown as SessionRow, rules: loadDestinationRules(db) });
    db.prepare("UPDATE raw_events SET batch_id = 'ordinary', processing_hash = ? WHERE id = 'ordinary'").run(request.coverage[0].sourceHash);
    const pending = applyObservations(db, token, { batchId: 'ordinary', repoId: REPO_ID, sessionId: 'sess1', rows,
      coverage: request.coverage, nearby: [], providedCheckpoint: provided ? nearbySnapshot(db, parent) : undefined,
      fallbackReason: null, now: NOW, output: { ...providerOutput('ordinary'), checkpoint: {
        decision: 'unchanged', source_event_ids: ['ordinary'], reason: 'No progress update.' } }, detect: async (text) => {
        // The stricter label arrives while the returned text is being checked.
        db.prepare("UPDATE memories SET sensitivity = 'private' WHERE id = ?").run(parent);
        return detect(text);
      } });
    if (provided) {
      await assert.rejects(pending, /sharing_context_changed/);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 1);
      assert.equal(db.prepare("SELECT processing_state FROM raw_events WHERE id = 'ordinary'").get()?.processing_state, 'pending');
      return;
    }
    const result = await pending;
    const id = result.applied[0].memoryId!;
    assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(id)?.sensitivity, provided ? 'private' : 'eligible');
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ? AND source_memory_id = ?').get(id, parent)?.n, provided ? 1 : 0);
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ?').get(parent)?.n, before);
    assert.equal(db.prepare('SELECT current_checkpoint_memory_id FROM work_items').get()?.current_checkpoint_memory_id, parent);
  });
});

test('failed checkpoint validation preserves the retry opportunity for progress', async () => {
  await withOpened(async (db, token) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    const result = await advance(db, token, 'retry-progress', null, NOW - 1, replacement,
      async (text) => text.startsWith('Purpose\n') ? { ok: false, reason: 'detector_error' } : await detect(text));
    assert.equal(result.checkpoint?.decision, 'rejected');
    assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get('retry-progress')?.processing_state, 'waiting');
    assert.equal(db.prepare('SELECT state FROM observation_batches WHERE id = ?').get('batch-retry-progress')?.state, 'fallback');
  });
});

test('equal-time portions advance; old backlog stays historical and independent progress conflicts', async () => {
  await withOpened(async (db, token) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    const first = (await advance(db, token, 'first', null, NOW - 100, replacement)).checkpoint!;
    const secondChoice = { ...replacement, outstanding: ['Confirm deployment remains a separate step.'] };
    const second = (await advance(db, token, 'second', first.memoryId, NOW - 100, secondChoice)).checkpoint!;
    assert.equal(second.decision, 'replaced');
    assert.notEqual(first.memoryId, second.memoryId);
    assert.equal(db.prepare('SELECT superseded_by FROM memories WHERE id = ?').get(first.memoryId)?.superseded_by, second.memoryId);
    const late = (await advance(db, token, 'late', second.memoryId, NOW - 200,
      { ...replacement, outstanding: ['Old work remaining.'] })).checkpoint!;
    assert.equal(late.decision, 'historical');
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM sync_conflicts').get()?.n, 0);
    const sibling = (await advance(db, token, 'sibling', first.memoryId, NOW,
      { ...replacement, outstanding: ['Independent progress.'] })).checkpoint!;
    assert.equal(sibling.decision, 'conflict');
    assert.equal(db.prepare("SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'").get()?.n, 1);
    assert.equal(db.prepare('SELECT current_checkpoint_memory_id FROM work_items').get()?.current_checkpoint_memory_id, second.memoryId);
    assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get('late')?.processing_state, 'processed');

    const returned = (await advance(db, token, 'return', second.memoryId, NOW, replacement)).checkpoint!;
    assert.equal(returned.decision, 'replaced');
    assert.notEqual(returned.memoryId, first.memoryId);
    assert.equal(db.prepare('SELECT checkpoint_parent_id FROM memories WHERE id = ?').get(first.memoryId)?.checkpoint_parent_id, null);
    assert.equal(db.prepare('SELECT checkpoint_parent_id FROM memories WHERE id = ?').get(returned.memoryId)?.checkpoint_parent_id, second.memoryId);
    db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(NOW, first.memoryId);
    assert.equal(db.prepare('SELECT deleted_at FROM memories WHERE id = ?').get(returned.memoryId)?.deleted_at, NOW);
    const suppressed = (await advance(db, token, 'deleted-return', returned.memoryId, NOW + 1, replacement)).checkpoint!;
    assert.equal(suppressed.decision, 'rejected');
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories WHERE work_id IS NOT NULL').get()?.n, 5);
  });
});

test('a published checkpoint preserves source evidence without settling ordinary information', async () => {
  await withOpened(async (db, token) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    seedEvent(db, { id: 'p1', content: 'Keep the API stable; verify interrupted uploads.',
      payload: { capture_root: '/fixture', source_paths: [] } });
    seedBatch(db, 'b1');
    db.prepare('UPDATE raw_events SET batch_id = ?').run('b1');
    const rows = db.prepare('SELECT * FROM raw_events').all() as unknown as RawEventRow[];
    const request = buildObserverRequest({ rows, session: db.prepare('SELECT * FROM sessions').get() as unknown as SessionRow,
      turns: [], repoId: REPO_ID, destination: 'remote_observer', nearby: [], rules: loadDestinationRules(db) });
    const portion = request.coverage[0];
    db.prepare('UPDATE raw_events SET processing_hash = ?').run(portion.sourceHash);
    const applied = await applyObservations(db, token, { batchId: 'b1', repoId: REPO_ID, sessionId: 'sess1',
      rows, nearby: [], output: { observations: [], checkpoint: replacement }, fallbackReason: null,
      coverage: request.coverage, detect, now: NOW });
    assert.equal(applied.leaseLost, false);
    const current = db.prepare('SELECT * FROM work_items').get()!;
    assert.equal(typeof current.current_checkpoint_memory_id, 'string');
    const memory = db.prepare('SELECT * FROM memories WHERE id = ?').get(current.current_checkpoint_memory_id)!;
    assert.equal(memory.type, 'session_summary');
    assert.equal(memory.work_id, current.id);
    assert.equal(memory.checkpoint_parent_id, null);
    assert.match(String(memory.body), /Keep the public API stable/);
    assert.match(String(memory.body), /interrupted-upload/);
    const evidence = db.prepare('SELECT evidence FROM memory_sources WHERE memory_id = ?').get(memory.id);
    assert.equal(evidence?.evidence, portion.text);
    assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get('p1')?.processing_state, 'waiting');
    assert.equal(db.prepare('SELECT reason FROM observation_batch_sources WHERE raw_event_id = ?').get('p1')?.reason, 'unaccounted');
  });
});

test('a withheld checkpoint permits only an explicit unchanged decision', () => {
  const withheld = { ...context, checkpoint_context: { state: 'withheld' as const } };
  assert.equal(validateObserverOutput({ observations: [], checkpoint: replacement }, withheld).ok, false);
  assert.equal(validateObserverOutput({ observations: [], checkpoint: {
    decision: 'unchanged', source_event_ids: ['p1'], reason: 'The prior state is unavailable.',
  } }, withheld).ok, true);
});

test('the complete parent takes request space before new source portions', async () => {
  await withOpened((db) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    seedEvent(db, { id: 'p1', content: 'Large accepted information. '.repeat(1_000) });
    const checkpointContext = { state: 'provided' as const, id: 'parent', title: 'Reliable uploads',
      body: 'Keep every constraint. '.repeat(85) };
    const request = buildObserverRequest({
      rows: db.prepare('SELECT * FROM raw_events').all() as unknown as RawEventRow[],
      session: db.prepare('SELECT * FROM sessions').get() as unknown as SessionRow,
      turns: [], repoId: REPO_ID, destination: 'remote_observer', nearby: [],
      rules: loadDestinationRules(db), checkpointContext,
    });
    assert.deepEqual(request.input.checkpoint_context, checkpointContext);
    assert.ok(JSON.stringify(request.input).length <= 12_000);
    assert.equal(request.coverage[0].state, 'partial');
    assert.ok(request.coverage[0].end > 0);
    assert.ok(request.coverage[0].end < request.coverage[0].total);
  });
});
