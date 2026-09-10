import assert from 'node:assert/strict';
import { join } from 'node:path';
import { test } from 'node:test';
import type { DatabaseSync } from 'node:sqlite';

import { openDatabase } from '../../src/db/open.js';
import { deliveredFactItems, evaluateRecall, measure } from '../../src/fixture/replay-evaluate.js';
import type { Line, MeasureInput, RecallProbe, Sample } from '../../src/fixture/replay.js';
import { ensureDirectories, oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';

async function withFact(fn: (db: DatabaseSync, probe: RecallProbe) => void): Promise<void> {
  await withTempHome((home) => {
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1_000 });
    try {
      db.exec(`INSERT INTO repos (id, identity_kind, normalized_identity) VALUES
        ('r', 'common_dir', '/r'), ('foreign', 'common_dir', '/foreign');
        INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status) VALUES
          ('source', 'r', 'claude', 'source-native', 'source', 'ended'),
          ('query', 'r', 'codex', 'query-native', 'query', 'active');
        INSERT INTO raw_events (id, repo_id, session_id, kind, content, classification_state, sensitivity, processing_state)
          VALUES ('fact', 'r', 'source', 'prompt', 'The answer is FACT-771.', 'done', 'eligible', 'processed');
        INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, state, completed_at)
          VALUES ('b', 'r', 'source', 'fact', 'remote_observer', 'applied', 100);
        INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, reason, recorded_at,
          portion_start, portion_end, source_total, source_hash)
          VALUES ('b', 'fact', 'processed', 'add', 100, 0, 10, 10, 'hash');
        INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity)
          VALUES ('memory', 'r', 'decision', 'Answer', 'FACT-771', 'memory-hash', 'eligible');
        INSERT INTO memory_sources (memory_id, raw_event_id, evidence) VALUES ('memory', 'fact', 'FACT-771');
        INSERT INTO injections (id, repo_id, session_id, conversation_id, context_epoch, kind, state, delivery_count)
          VALUES ('delivery', 'r', 'query', 'query', 0, 'prompt', 'emitted', 0);
        INSERT INTO injection_items (injection_id, conversation_id, context_epoch, source_kind, memory_id, decision)
          VALUES ('delivery', 'query', 0, 'memory', 'memory', 'included');`);
      fn(db, { id: 'f-1', lang: 'en', query: 'What is the answer?', expect: 'FACT-771', factSeq: 1, querySeq: 2,
        repoId: 'r', sourceSessionId: 'source', sourceIds: ['fact'], factSourceIds: ['fact'], sessionId: 'query',
        conversationId: 'query', epoch: 0, priorInjectionIds: [], priorDelivery: [],
        currentInjectionIds: ['delivery'], currentTextHit: true });
    } finally { db.close(); }
  });
}

test('recall traces separate retained facts, confirmed delivery, and unrun agent answers', async () => {
  await withFact((db, probe) => {
    const trace = evaluateRecall(db, probe);
    assert.deepEqual(Object.values(trace.stages).map((stage) => stage.status),
      ['pass', 'pass', 'pass', 'pass', 'pass', 'pass', 'not_run']);
    assert.equal(trace.availability, 'current_delivery');
    assert.equal(trace.firstFailure, null);
    assert.equal(evaluateRecall(db, { ...probe, currentTextHit: false }).availability, 'missing',
      'a stored full memory is insufficient if the printed excerpt omitted its fact');
    db.exec('DELETE FROM raw_events');
    const retained = evaluateRecall(db, probe);
    assert.equal(retained.stages.capture.reason, 'retained_receipt');
    assert.equal(retained.stages.retention.status, 'pass');
    assert.equal(retained.availability, 'current_delivery');
  });
});

test('prior delivery is frozen before the query and scoped to repository, conversation, and epoch', async () => {
  await withFact((db, probe) => {
    const prior = deliveredFactItems(db, probe, ['delivery']);
    assert.equal(prior.length, 1, 'ordinary emitted delivery_count=0 is a confirmed handoff');
    const duplicate = evaluateRecall(db, { ...probe, currentInjectionIds: [], currentTextHit: false, priorDelivery: prior });
    assert.equal(duplicate.availability, 'prior_delivery');
    assert.equal(duplicate.stages.retrieval.reason, 'prior_delivery');
    for (const change of [{ repoId: 'foreign' }, { conversationId: 'different' }, { epoch: 1 }]) {
      assert.deepEqual(deliveredFactItems(db, { ...probe, ...change }, ['delivery']), []);
    }
    db.exec("UPDATE injections SET kind = 'grok_deferred', state = 'attempted'");
    const before = deliveredFactItems(db, probe, ['delivery']);
    assert.deepEqual(before, []);
    assert.equal(evaluateRecall(db, probe).availability, 'missing');
    db.exec("UPDATE injections SET state = 'emitted'");
    assert.deepEqual(deliveredFactItems(db, probe, ['delivery']), [], 'Grok requires confirmed delivery_count');
    db.exec('UPDATE injections SET delivery_count = 1');
    assert.equal(evaluateRecall(db, probe).availability, 'current_delivery');
    assert.equal(evaluateRecall(db, { ...probe, currentInjectionIds: [], currentTextHit: false,
      priorInjectionIds: ['delivery'], priorDelivery: before }).availability, 'missing',
    'a formerly pending record confirmed later cannot become prior delivery retroactively');
  });
});

test('fact stages expose source omission, partial coverage, no-memory output, retention and retrieval misses', async () => {
  await withFact((db, probe) => {
    const cases = [
      { sql: "DELETE FROM raw_events; DELETE FROM observation_batch_sources", stage: 'capture', status: 'fail', reason: 'source_missing' },
      { sql: "UPDATE raw_events SET classification_state = 'failed'", stage: 'capture', status: 'fail', reason: 'classification_failed' },
      { sql: "UPDATE raw_events SET processing_state = 'excluded'", stage: 'capture', status: 'fail', reason: 'excluded' },
      { sql: "UPDATE observation_batch_sources SET outcome = 'assigned', portion_end = NULL", stage: 'coverage', status: 'pending', reason: 'no_range' },
      { sql: "UPDATE observation_batch_sources SET portion_end = 5", stage: 'coverage', status: 'partial', reason: 'partial_range' },
      { sql: "UPDATE observation_batch_sources SET outcome = 'uncovered', reason = 'not_sent', portion_end = 0", stage: 'coverage', status: 'pending', reason: 'not_sent' },
      { sql: "UPDATE observation_batch_sources SET outcome = 'uncovered', reason = 'unaccounted'", stage: 'application', status: 'pending', reason: 'unaccounted' },
      { sql: "UPDATE observation_batch_sources SET outcome = 'legacy_unknown'", stage: 'application', status: 'pending', reason: 'legacy_unknown' },
      { sql: "UPDATE observation_batch_sources SET reason = 'no_memory'; DELETE FROM memory_sources", stage: 'application', status: 'pass', reason: 'no_memory' },
      { sql: "UPDATE memories SET body = 'An unrelated answer'", stage: 'retention', status: 'fail', reason: 'no_linked_fact' },
      { sql: "UPDATE memories SET degraded_reason = 'no_provider'", stage: 'retention', status: 'fail', reason: 'temporary_only' },
      { sql: "UPDATE memories SET valid_to = 200", stage: 'retention', status: 'fail', reason: 'retired_or_withheld' },
      { sql: "DELETE FROM injection_items", stage: 'retrieval', status: 'fail', reason: 'no_candidate' },
      ...['mmr_redundant', 'budget', 'below_threshold'].map((reason) => ({
        sql: `UPDATE injection_items SET decision = 'omitted', reason = '${reason}'`, stage: 'retrieval', status: 'fail', reason })),
      { sql: "UPDATE injections SET state = 'built'", stage: 'delivery', status: 'fail', reason: 'missing' },
    ] as const;
    for (const example of cases) {
      db.exec('SAVEPOINT stage_case');
      db.exec(example.sql);
      const trace = evaluateRecall(db, probe);
      const stage = trace.stages[example.stage as keyof typeof trace.stages];
      assert.equal(stage.status, example.status, example.sql);
      assert.equal(stage.reason, example.reason, example.sql);
      assert.equal(trace.stages.answer.status, 'not_run');
      db.exec('ROLLBACK TO stage_case; RELEASE stage_case');
    }
    db.exec(`UPDATE observation_batch_sources SET portion_end = 5;
      INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, state, completed_at)
        VALUES ('b2', 'r', 'source', 'fact2', 'remote_observer', 'applied', 200);
      INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, reason, recorded_at,
        portion_start, portion_end, source_total, source_hash)
        VALUES ('b2', 'fact', 'processed', 'add', 200, 4, 10, 10, 'hash');`);
    assert.deepEqual(evaluateRecall(db, probe).stages.coverage, { status: 'pass', reason: 'complete_range',
      sources: [{ id: 'fact', total: 10, ranges: [[0, 10]] }] });
    db.exec("UPDATE observation_batch_sources SET source_hash = 'changed-content' WHERE batch_id = 'b2'");
    assert.equal(evaluateRecall(db, probe).stages.coverage.status, 'partial', 'different source revisions cannot fill each other\'s gaps');
    db.exec('UPDATE memory_sources SET context_only = 1');
    assert.equal(evaluateRecall(db, probe).stages.retention.reason, 'no_linked_fact', 'privacy dependencies are not factual attribution');
    assert.deepEqual(deliveredFactItems(db, probe, ['delivery']), []);
  });
});

test('raw activity can deliver a captured fact while permanent retention is still pending', async () => {
  await withFact((db, probe) => {
    db.exec("DELETE FROM memory_sources; UPDATE injection_items SET source_kind = 'raw_activity', memory_id = NULL, raw_event_id = 'fact'");
    const trace = evaluateRecall(db, probe);
    assert.equal(trace.availability, 'current_delivery');
    assert.equal(trace.stages.retention.status, 'fail');
    assert.equal(trace.firstFailure, 'retention');
  });
});

test('retention uses the current injection privacy policy for private memories', async () => {
  await withFact((db, probe) => {
    db.exec("UPDATE memories SET sensitivity = 'private'");
    assert.equal(evaluateRecall(db, probe).stages.retention.status, 'pass', 'local injection permits private facts by default');
    db.exec("UPDATE destination_rules SET allowed = 0 WHERE destination = 'injection' AND sensitivity = 'private'");
    assert.equal(evaluateRecall(db, probe).stages.retention.reason, 'retired_or_withheld');
  });
});

test('completed-run evaluation reads the migrated database and returns every verdict', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const opened = openDatabase({ path: paths.db, timeoutMs: 1_000 });
    try {
      const line: Line = {
        seq: 1,
        agent: 'codex',
        event: 'SessionStart',
        session: 'codex-01',
        payload: { session_id: 'native-codex-01' },
      };
      const sample: Sample = {
        agent: line.agent,
        event: line.event,
        seq: line.seq,
        session: line.session,
        ms: 12.5,
      };
      const input: MeasureInput = {
        repoId: 'r-a',
        lines: [line],
        captureSamples: [sample],
        injectionSamples: [sample],
        readySamples: [sample],
        pendingSamples: [],
        startSamples: [],
        sizeRows: [],
        packs: [],
            recallProbes: [],
        hookFailures: [],
        hookCount: 1,
        resumeChecks: [],
        maps: { secrets: new Map(), secretValues: [], negatives: [], directives: [] },
        observeRssKb: 0,
        observeRuns: 0,
        hookWorkerRssKb: 0,
        hookWorkerRuns: 0,
        dbBytesBefore: 0,
        home,
        fixturePath: join(home, 'fixture.jsonl'),
        bundle: process.execPath,
        startedAt: '2026-09-09T00:00:00.000Z',
        loadAtStart: '0.00 0.00 0.00',
      };

      const evaluated = measure(opened, paths, input);
      const report = evaluated.json as {
        growth: { rawEvents: number; memories: number; injections: number; injectionItems: number };
        bounds: { sc: string; status: string }[];
        lifecycle: { check: string; n: number; pass: boolean }[];
        failed: boolean;
      };

      assert.deepEqual(
        {
          rawEvents: report.growth.rawEvents,
          memories: report.growth.memories,
          injections: report.growth.injections,
          injectionItems: report.growth.injectionItems,
        },
        {
          rawEvents: 0,
          memories: 0,
          injections: 0,
          injectionItems: 0,
        },
      );
      assert.deepEqual(
        report.bounds.map((row) => [row.sc, row.status]),
        [
          ['SC-002', 'pass'],
          ['injection', 'pass'],
          ['session start', 'fail'],
          ['SC-003', 'pass'],
          ['SC-005', 'pass'],
          ['SC-009', 'pass'],
          ['SC-010', 'pass'],
          ['lifecycle', 'fail'],
          ['directives', 'pass'],
          ['hooks', 'pass'],
        ],
      );
      assert.deepEqual(
        report.lifecycle.map((row) => [row.check, row.n, row.pass]),
        [
          ['fork', 0, false],
          ['resume', 0, false],
          ['compact', 0, false],
          ['clear', 0, false],
        ],
      );
      assert.equal(report.failed, true);
      assert.equal(evaluated.failed, true);
      assert.match(evaluated.markdown, /Rows: raw_events=0, memories=0, injections=0, injection_items=0\./);

      opened.db.exec(`
        INSERT INTO repos (id, identity_kind, normalized_identity) VALUES
          ('r-a', 'common_dir', '/fixture-a'), ('r-b', 'common_dir', '/fixture-b');
        INSERT INTO sessions (id, repo_id, agent, native_session_id, original_native_session_id,
          conversation_id, context_epoch, status) VALUES
          ('s-a', 'r-a', 'codex', 'shared-native', NULL, 's-a', 1, 'active'),
          ('s-b', 'r-b', 'codex', 'internal-storage-id', 'shared-native', 's-b', 0, 'active');
        INSERT INTO raw_events (id, repo_id, session_id, kind, content, classification_state)
          VALUES ('compact-a', 'r-a', 's-a', 'compaction_summary', 'Completed work', 'done');
      `);
      const scoped = measure(opened, paths, { ...input, repoId: 'r-a', lines: [{ ...line,
        payload: { session_id: 'shared-native' }, tags: { lifecycle: 'compact' } }] });
      const checks = scoped.json.lifecycle as { check: string; pass: boolean }[];
      assert.equal(checks.find((check) => check.check === 'compact')?.pass, true,
        'the replay repository must use its own lifecycle despite another repo sharing the native ID');
    } finally {
      opened.db.close();
    }
  });
});
