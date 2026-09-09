import assert from 'node:assert/strict';
import { test } from 'node:test';

import { applyCompaction } from '../../src/capture-compaction.js';
import type { AgentName, NormalizedEvent } from '../../src/events.js';
import { NOW } from '../helpers/capture.js';

function compactionEvent(agent: AgentName, key: string, text: string): NormalizedEvent {
  return {
    agent,
    native_session_id: 'session-1',
    cwd: '/repo',
    captured_at: NOW,
    kind: 'compaction_summary',
    text,
    compaction_key: key,
  };
}

test('applyCompaction advances the epoch once per compaction on Grok', async () => {
  const first = compactionEvent('grok', '2026-09-03T16:01:11.622755654+00:00', '');
  const again = compactionEvent('grok', '2026-09-03T16:04:02.101110000+00:00', '');

  const opened = applyCompaction('grok', first, { contextEpoch: 0, lastCompactionKey: null }, 'id-1');
  assert.equal(opened?.contextEpoch, 1);
  assert.equal(
    applyCompaction('grok', first, opened as never, 'id-1'),
    null,
    'a re-delivery adds no epoch',
  );
  const second = applyCompaction('grok', again, opened as never, 'id-2');
  assert.equal(second?.contextEpoch, 2);
});

test('applyCompaction keys Claude Code and Codex compactions by the event id (A16)', async () => {
  const claudeStart: NormalizedEvent = {
    agent: 'claude',
    native_session_id: 'session-1',
    cwd: '/repo',
    captured_at: NOW,
    kind: 'session_start',
    source: 'compact',
  };
  // On Claude Code the SessionStart(compact) hook runs ~24 ms before PostCompact, so it opens the
  // epoch and PostCompact only confirms it (R13 "Compaction identity and order", A16).
  const opened = applyCompaction(
    'claude',
    claudeStart,
    { contextEpoch: 0, lastCompactionKey: null },
    'start-1',
  );
  assert.equal(opened?.contextEpoch, 1);
  // A16: without a native per-compaction value the key is the stored id of the PostCompact row.
  const confirmed = applyCompaction(
    'claude',
    compactionEvent('claude', '', 'summary text'),
    opened as never,
    'postcompact-1',
  );
  assert.equal(confirmed?.contextEpoch, 1, 'PostCompact must not advance the epoch a second time');
  const next = applyCompaction('claude', claudeStart, confirmed as never, 'start-2');
  assert.equal(next?.contextEpoch, 2, 'the next compaction opens the next epoch');

  const codex = compactionEvent('codex', '', '');
  const codexOpened = applyCompaction(
    'codex',
    codex,
    { contextEpoch: 0, lastCompactionKey: null },
    'postcompact-2',
  );
  assert.equal(codexOpened?.contextEpoch, 1);
  assert.equal(applyCompaction('codex', codex, codexOpened as never, 'postcompact-2'), null);
  // Codex fires SessionStart(compact) after PostCompact, so it only reads the epoch.
  assert.equal(
    applyCompaction(
      'codex',
      { ...codex, kind: 'session_start', source: 'compact' } as never,
      codexOpened as never,
      'start-3',
    ),
    null,
  );
});

test('applyCompaction keys Pi compactions by compactionEntry.id', async () => {
  const first = compactionEvent('pi', '480afbf2', 'summary');
  const second = compactionEvent('pi', '4283239e', 'summary');
  const opened = applyCompaction('pi', first, { contextEpoch: 0, lastCompactionKey: null }, 'id-1');
  assert.equal(opened?.contextEpoch, 1);
  assert.equal(applyCompaction('pi', first, opened as never, 'id-1'), null);
  assert.equal(applyCompaction('pi', second, opened as never, 'id-2')?.contextEpoch, 2);
});
