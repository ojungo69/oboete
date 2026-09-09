import assert from 'node:assert/strict';
import { existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';
import { test } from 'node:test';

import { renderReport, type BoundRow } from '../../src/fixture/replay-report.js';
import type { ReportComputed } from '../../src/fixture/replay-evaluate.js';
import { replayHome, type MeasureInput } from '../../src/fixture/replay.js';

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
  const recall = { id: 'fact-1', lang: 'en', query: 'Where?', expect: 'There.', hit: false } as const;
  const input: MeasureInput = {
    lines: [{ seq: 1, agent: 'codex', event: 'SessionStart', session: 'codex-01', payload: {} }],
    captureSamples: [sample],
    injectionSamples: [sample],
    readySamples: [sample],
    pendingSamples: [sample],
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
    sessionStartPack: new Map(),
    recallHits: [recall],
    grokRecallWait: [],
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
  assert.match(rendered.markdown, /One or more measured bounds failed/);
});
