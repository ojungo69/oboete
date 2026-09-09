import assert from 'node:assert/strict';
import { join } from 'node:path';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { measure } from '../../src/fixture/replay-evaluate.js';
import type { Line, MeasureInput, Sample } from '../../src/fixture/replay.js';
import { ensureDirectories, oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';

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
        lines: [line],
        captureSamples: [sample],
        injectionSamples: [sample],
        readySamples: [sample],
        pendingSamples: [],
        sizeRows: [],
        packs: [],
        sessionStartPack: new Map(),
        recallHits: [],
        grokRecallWait: [],
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
    } finally {
      opened.db.close();
    }
  });
});
