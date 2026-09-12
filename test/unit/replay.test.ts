import assert from 'node:assert/strict';
import { existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';
import { test } from 'node:test';

import { renderReport, type BoundRow } from '../../src/fixture/replay-report.js';
import type { FactTrace, ReportComputed } from '../../src/fixture/replay-evaluate.js';
import { classifyStartSample, holdLease, releaseHeldLease, replayHome, replayTargetsSettled, startInjectionExpected, waitForReplaySettlement, type MeasureInput } from '../../src/fixture/replay.js';
import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';
import { claimLease } from '../../src/worker/lease.js';

test('replay cannot steal a live worker lease or clear a successor owner', async () => {
  await withTempHome(async (home) => {
    const path = oboetePaths(home).db;
    const { db } = openDatabase({ path, timeoutMs: 1_000 });
    try {
      let now = Date.now();
      const owner = claimLease(db, { pid: 123, now });
      assert.ok(owner);
      const before = db.prepare('SELECT * FROM worker_lease').get();
      await holdLease(path, { timeoutMs: 100, now: () => now, sleep: async (ms) => { now += ms; } });
      assert.deepEqual(db.prepare('SELECT * FROM worker_lease').get(), before,
        'acquisition timeout must leave the live owner byte-for-byte unchanged');
      releaseHeldLease(path, 'expired-replay-token');
      assert.deepEqual(db.prepare('SELECT * FROM worker_lease').get(), before,
        'an old replay token must not clear its successor');
    } finally { db.close(); }
  });
});

test('replay barrier uses target receipts and current summaries, including degraded output', async () => {
  await withTempHome(async (home) => {
    const path = oboetePaths(home).db;
    const { db } = openDatabase({ path, timeoutMs: 1_000 });
    try {
      db.exec(`INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('r', 'common_dir', '/r');
        INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status, summary_state, summary_updated_at)
          VALUES ('s', 'r', 'claude', 'native', 's', 'ended', 'pending', 100);
        INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, state, completed_at)
          VALUES ('b', 'r', 's', 'e', 'fallback', 'fallback', 100);
        INSERT INTO raw_events (id, repo_id, session_id, kind, content, classification_state, processing_state)
          VALUES ('e', 'r', 's', 'prompt', 'Retained fact', 'done', 'waiting');
        INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, recorded_at)
          VALUES ('b', 'e', 'deferred', 100);`);
      assert.equal(replayTargetsSettled(db, 'r', ['s']), true);
      assert.equal(replayTargetsSettled(db, 'other', ['s']), false);
      for (const change of [
        "UPDATE raw_events SET processing_state = 'pending'",
        "UPDATE sessions SET summary_updated_at = 99",
        "UPDATE observation_batch_sources SET outcome = 'assigned'",
        "UPDATE observation_batches SET state = 'running'",
      ]) {
        db.exec('SAVEPOINT pending_case');
        db.exec(change);
        assert.equal(replayTargetsSettled(db, 'r', ['s']), false, change);
        db.exec('ROLLBACK TO pending_case; RELEASE pending_case');
      }
      let now = 0;
      const worker = { pid: 123, running: () => now < 65_000, status: () => now < 65_000 ? null : 1 };
      await waitForReplaySettlement(path, 'r', ['s'], worker, {
        timeoutMs: 70_000, now: () => now, sleep: async () => { now += 5_000; },
      });
      assert.equal(now, 65_000, 'the old 60-second wait cannot stand in for worker exit');
      await assert.rejects(waitForReplaySettlement(path, 'r', ['missing'], worker, {
        timeoutMs: 100, now: () => now, sleep: async (ms) => { now += ms; },
      }), /worker_settle_timeout/);
      await assert.rejects(waitForReplaySettlement(join(home, 'missing', 'db'), 'r', [], worker), /storage_error/);
    } finally { db.close(); }
  });
});

test('start timing class follows the exact injection, and missing/ambiguous rows are unclassified', () => {
  const sample = { agent: 'grok', event: 'SessionStart', seq: 1, session: 's', ms: 400 } as const;
  const row = { id: 'i', state: 'pending', degradedReason: 'summary_pending', hash: null };
  assert.equal(classifyStartSample(sample, [row]).classification, 'pending');
  assert.equal(classifyStartSample(sample, [{ ...row, degradedReason: null }]).classification, 'ready');
  assert.equal(classifyStartSample(sample, []).classification, 'unclassified');
  assert.equal(classifyStartSample(sample, [row, { ...row, id: 'another' }]).classification, 'unclassified');
  const start = { seq: 1, agent: 'claude', event: 'SessionStart', session: 's', payload: { source: 'fork' } } as const;
  assert.equal(startInjectionExpected(start), false);
  assert.equal(startInjectionExpected({ ...start, payload: { source: 'startup' } }), true);
  assert.equal(startInjectionExpected({ ...start, payload: {} }), true, 'missing data must still fail the persisted-record check');
});

test('replay refuses foreign active sources before starting a worker or printing a measurement', async () => {
  await withTempHome((home) => {
    writeFileSync(join(home, 'config.toml'), '[observer]\npreset = "none"\n');
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1_000 });
    try {
      db.exec(`INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('foreign', 'common_dir', '/foreign');
        INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
          VALUES ('foreign-session', 'foreign', 'claude', 'foreign-native', 'foreign-session', 'active');
        INSERT INTO raw_events (id, repo_id, session_id, kind, content, classification_state)
          VALUES ('foreign-source', 'foreign', 'foreign-session', 'prompt', 'Pending external work', 'done');`);
      const fixture = readFileSync('test/fixtures/events-1000.jsonl', 'utf8').trim().split('\n').map((line) => JSON.parse(line));
      const lines = [...fixture.slice(0, 3), fixture.find((line) => line.event === 'SessionEnd')];
      const path = join(home, 'replay.jsonl');
      writeFileSync(path, lines.map((line, index) => JSON.stringify({ ...line, seq: index + 1 })).join('\n'));
      const result = spawnSync(process.execPath, ['dist/oboete.mjs', 'fixture', 'replay', path, '--home', home, '--json'],
        { encoding: 'utf8', timeout: 10_000 });
      assert.equal(result.status, 1, result.stderr);
      assert.equal(result.stdout, '', 'invalid readiness must not become a measurement');
      assert.match(result.stderr, /foreign_active_sources/);
      assert.equal(db.prepare('SELECT pid FROM worker_lease WHERE id = 1').get()?.pid, null);
      assert.equal(db.prepare("SELECT processing_state FROM raw_events WHERE id = 'foreign-source'").get()?.processing_state, 'pending');
    } finally { db.close(); }
  });
});

function withEnv(value: string | undefined, run: () => void): void {
  const before = process.env.OBOETE_HOME;
  if (value === undefined) delete process.env.OBOETE_HOME;
  else process.env.OBOETE_HOME = value;
  try {
    run();
  } finally {
    if (before === undefined) delete process.env.OBOETE_HOME;
    else process.env.OBOETE_HOME = before;
  }
}

test('--home and a set OBOETE_HOME name a directory the replay does not own', () => {
  const envHome = join(tmpdir(), 'oboete-replay-env-home');
  withEnv(envHome, () => {
    const flag = replayHome({ home: 'relative-home' });
    assert.equal(flag.home, resolve('relative-home'));
    assert.equal(flag.createdHome, false);

    const env = replayHome({});
    assert.equal(env.home, envHome);
    assert.equal(env.createdHome, false);
  });
});

test('an unset or empty OBOETE_HOME makes a temporary home this run owns and removes', () => {
  for (const value of [undefined, '']) {
    withEnv(value, () => {
      const made = replayHome({});
      try {
        assert.equal(isAbsolute(made.home), true);
        assert.equal(existsSync(made.home), true);
        // The empty case used to report false here, so the directory it made was left behind.
        assert.equal(made.createdHome, true);
      } finally {
        rmSync(made.home, { recursive: true, force: true });
      }
    });
  }
});

test('renderer preserves the report sections, supplied bounds, and failure evidence', () => {
  const sample = { agent: 'codex', event: 'SessionStart', seq: 1, session: 'codex-01', ms: 12.5 } as const;
  const recall: FactTrace = { id: 'fact-1', lang: 'en', query: 'Where?', expect: 'There.', hit: false,
    factSeq: 1, querySeq: 2, availability: 'missing', firstFailure: 'delivery', stages: {
      capture: { status: 'pass', reason: 'accepted' }, coverage: { status: 'pass', reason: 'complete_range' },
      application: { status: 'pass', reason: 'accounted' }, retention: { status: 'pass', reason: 'retained_fact' },
      retrieval: { status: 'pass', reason: 'selected' }, delivery: { status: 'fail', reason: 'missing' },
      answer: { status: 'not_run', reason: 'receiving_agent_not_run' } } };
  const input: MeasureInput = {
    repoId: 'r-a',
    lines: [{ seq: 1, agent: 'codex', event: 'SessionStart', session: 'codex-01', payload: {} }],
    captureSamples: [sample],
    injectionSamples: [sample],
    readySamples: [sample],
    pendingSamples: [sample],
    startSamples: [],
    sizeRows: [
      {
        seq: 1,
        agent: 'codex',
        event: 'SessionStart',
        tag: 'at_bound',
        fillBytes: 1_048_576,
        ms: 12.5,
        classification: 'done',
        truncated: 0,
      },
    ],
    packs: [],
    recallProbes: [],
    hookFailures: [{ seq: 1, agent: 'codex', event: 'SessionStart', status: '1', stderr: 'left | right' }],
    hookCount: 1,
    resumeChecks: [],
    maps: {
      secrets: new Map(),
      secretValues: [{ id: 'secret-1', secret: 'never-written' }],
      negatives: [],
      directives: [],
    },
    observeRssKb: 1024,
    observeRuns: 1,
    hookWorkerRssKb: 0,
    hookWorkerRuns: 0,
    dbBytesBefore: 100,
    home: join(tmpdir(), 'oboete-render-test'),
    fixturePath: join(tmpdir(), 'fixture.jsonl'),
    bundle: process.execPath,
    startedAt: '2026-09-09T00:00:00.000Z',
    loadAtStart: '0.00 0.00 0.00',
  };
  const computed: ReportComputed = {
    dbBytesAfter: 200,
    perThousand: 100_000,
    rawEvents: 1,
    memories: 1,
    injections: 1,
    injectionItems: 1,
    duplicateGroups: [],
    leakedSecrets: [],
    leakedDirectives: [],
    negativesUnredacted: 0,
    rawDirectiveRows: 0,
    recallJa: [],
    recallEn: [recall],
    recallTraces: [recall],
    stageCounts: { delivery: { fail: 1 }, answer: { not_run: 1 } },
    misses: [recall],
    lifecycleRows: [{ check: 'resume', n: 1, pass: false, offenders: ['codex:codex-01'] }],
    lifecyclePass: false,
    captureValues: [12.5],
    captureUnder: 1,
    captureP99: 12.5,
    sc002: true,
    injectionValues: [12.5],
    injectionUnder: 1,
    injectionP99: 12.5,
    injectionTiming: {
      rows: [['codex', 'SessionStart', '1', '12.5', '12.5', '12.5', '12.5', '300 ms', 'pass']],
      pass: true,
      worstGroup: 'codex/SessionStart p99 12.5 ms',
    },
    injectionPass: true,
    pending: { hits: 1, text: '1/1 packs carry summary_pending' },
    readyMax: 12.5,
    pendingMax: 12.5,
    readyPass: true,
    pendingPass: true,
    sc003: true,
    sc005: true,
    sc009: false,
    sc010: true,
    directivesPass: true,
    leakedDirectivesEllipsis: '',
    hooksPass: false,
    failed: true,
    compactionSummaries: [],
    workerRssKb: 1024,
    workerRuns: 'observe runs: 1 spawned by replay, 0 hook-spawned (polled via worker_lease.pid)',
  };
  const bounds: BoundRow[] = [
    { sc: 'hooks', measured: '1 of 1 hooks failed', bound: 'all hooks exit 0', status: 'fail' },
  ];

  const rendered = renderReport(input, computed, bounds);

  assert.equal(rendered.failed, true);
  assert.deepEqual(rendered.json.bounds, bounds);
  assert.deepEqual(rendered.json.hooks, { n: 1, failures: 1, pass: false });
  assert.deepEqual(rendered.markdown.match(/^### .+$/gm), [
    '### Setup',
    '### SC-002 capture time',
    '### Injection hooks',
    '### Session-start wait',
    '### SC-003 worker memory and database growth',
    '### SC-005 secret scan',
    '### Directive scan',
    '### SC-010 duplicate injections',
    '### SC-009 fact recall',
    '### Lifecycle',
    '### Hook exits',
    '### Bounds',
  ]);
  assert.equal(rendered.markdown.includes('left \\| right'), true);
  assert.match(rendered.markdown, /delivery \| missing/);
  assert.equal((rendered.json.recall as { probes: { stages: { answer: { status: string } } }[] }).probes[0].stages.answer.status, 'not_run');
  assert.match(rendered.markdown, /One or more measured bounds failed/);
});
