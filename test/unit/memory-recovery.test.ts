import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { test } from 'node:test';

import type { ObserveDeps } from '../../src/worker/observe.js';
import type { ObserverInput } from '../../src/observer/contract.js';
import { generationItem } from '../../src/doctor/storage.js';
import { runObserve } from '../../src/worker/observe.js';
import { createBatches } from '../../src/worker/batches.js';
import { claimLease, releaseLease } from '../../src/worker/lease.js';
import { purgeExpiredEvents } from '../../src/worker/purge.js';
import { detectSync } from '../../src/privacy/detect.js';
import {
  DAY,
  NOW,
  captureEndedSession,
  cleanEnv,
  eventId,
  openAiResponse,
  providerOutput,
  runObserveForFixture,
  withFixture,
  writeConfig,
  type Fixture,
} from '../helpers/observe.js';

async function observeAt(fixture: Fixture, at: number, fetch: ObserveDeps['fetch']): Promise<number> {
  const started = performance.now();
  return await runObserveForFixture(fixture, {
    now: () => at + Math.floor(performance.now() - started),
    maxRunMs: 2_000,
    fetch,
  });
}

function sentInput(options: RequestInit | undefined): ObserverInput {
  const body = JSON.parse(String(options?.body)) as { messages: { role: string; content: string }[] };
  return JSON.parse(body.messages.find((message) => message.role === 'user')!.content) as ObserverInput;
}

function fixtureRepo(fixture: Fixture, name: string): string {
  const root = join(fixture.home, name);
  mkdirSync(root);
  assert.equal(spawnSync('git', ['-C', root, 'init', '--quiet']).status, 0);
  assert.equal(spawnSync('git', ['-C', root, 'remote', 'add', 'origin', 'https://example.invalid/privacy-fixture.git']).status, 0);
  return root;
}

test('source path revalidation uses its captured checkout, including tool-result paths', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'checkout-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const original = fixtureRepo(fixture, 'source-checkout');
    const later = fixtureRepo(fixture, 'later-checkout');
    const common = { session_id: 'original-checkout', cwd: original };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'one', prompt: 'Review the retry behavior.' });
    await fixture.capture('PreToolUse', { ...common, tool_use_id: 'read-source', tool_name: 'Read',
      tool_input: { file_path: './secrets/fixture.txt' } });
    await fixture.capture('PostToolUse', { session_id: 'original-checkout', cwd: original,
      tool_use_id: 'read-result', tool_name: 'Read', tool_input: { file_path: './secrets/fixture.txt' },
      tool_response: { file: { filePath: 'secrets/fixture.txt', content: 'The file contains the retry behavior.' } } });
    await fixture.capture('SessionEnd', { session_id: 'original-checkout', cwd: original, reason: 'prompt_input_exit' });
    writeFileSync(join(original, '.oboete.toml'), '[privacy]\nsecret_paths = ["secrets/**"]\n');
    await fixture.capture('SessionStart', { session_id: 'later-checkout', cwd: later, source: 'startup' });
    fixture.withDb((db) => assert.equal(db.prepare('SELECT display_root FROM repos').get()?.display_root, later));
    await observeAt(fixture, NOW, async (_url, options) => {
      const input = sentInput(options);
      assert.ok(input.events.every((event) => event.kind === 'prompt'), 'file material must be excluded by the original checkout rule');
      return openAiResponse(providerOutput(input.events[0].id));
    });
    fixture.withDb((db) => {
      const rows = db.prepare("SELECT kind, processing_state FROM raw_events WHERE kind IN ('tool_call', 'tool_result')").all();
      assert.equal(rows.length, 2);
      assert.ok(rows.every((row) => row.processing_state === 'excluded'), JSON.stringify(rows));
    });
  });
});

for (const at of ['before', 'send']) test(`a checkout replaced ${at} detection cannot authorize old file material`, async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'reused-root-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const root = fixtureRepo(fixture, 'reused-checkout');
    await captureEndedSession(fixture, { sessionId: 'old-root', cwd: root,
      prompts: ['Review the retry behavior.'], tools: [{ id: 'old-file', path: 'secrets/fixture.txt' }] });
    const sourceId = fixture.withDb((db) => String(db.prepare("SELECT id FROM raw_events WHERE kind = 'tool_call'").get()!.id));
    const replace = () => assert.equal(spawnSync('git', ['-C', root, 'remote', 'set-url', 'origin', 'https://example.invalid/replacement.git']).status, 0);
    if (at === 'before') replace();
    let leaked = false;
    await runObserveForFixture(fixture, { now: () => NOW, maxRunMs: 2_000,
      detect: async (input) => {
        const result = await detectSync(input);
        if (at === 'send' && input.text.startsWith('{"repo_ref"')) replace();
        return result;
      },
      fetch: async (_url, options) => {
        const input = sentInput(options);
        leaked ||= input.events.some((event) => event.id === sourceId);
        return openAiResponse(providerOutput(input.events[0].id));
      },
    });
    assert.equal(leaked, false);
    fixture.withDb((db) => assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(sourceId)?.processing_state, 'waiting'));
  });
});

test('retained memory provenance rechecks source paths after full raw activity expires', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'evidence-path-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const original = fixtureRepo(fixture, 'evidence-checkout');
    await captureEndedSession(fixture, { sessionId: 'evidence-before', cwd: original,
      prompts: ['Record the retry behavior.'], tools: [{ id: 'evidence-tool', path: 'secrets/fixture.ts' }] });
    await observeAt(fixture, NOW, async (_url, options) => {
      const input = sentInput(options);
      const output = providerOutput(input.events[0].id);
      output.observations[0].source_event_ids = input.events.map((event) => event.id);
      output.observations[0].citations = { files_read: [], files_modified: [], commits: [] };
      return openAiResponse(output);
    });
    const memoryId = fixture.withDb((db) => {
      const id = String(db.prepare("SELECT id FROM memories WHERE type = 'discovery'").get()!.id);
      const token = claimLease(db, { pid: process.pid, now: NOW + 31 * DAY })!;
      purgeExpiredEvents(db, token, NOW + 31 * DAY);
      releaseLease(db, token, () => true);
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE kind = 'tool_call'").get()?.n, 0);
      return id;
    });
    writeFileSync(join(original, '.oboete.toml'), '[privacy]\nsecret_paths = ["secrets/**"]\n');
    await captureEndedSession(fixture, { sessionId: 'evidence-after', cwd: original, prompts: ['Explain the retry behavior.'] });
    await observeAt(fixture, NOW + 31 * DAY, async (_url, options) => {
      const input = sentInput(options);
      assert.ok(input.nearby.every((candidate) => candidate.id !== memoryId));
      return openAiResponse(providerOutput(input.events[0].id));
    });
    fixture.withDb((db) => assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(memoryId)?.sensitivity, 'secret'));
  });
});

for (const verdict of ['clean', 'failed', 'late', 'deleted'] as const) {
  test(`nearby ${verdict} detection cannot undo a concurrent privacy upgrade`, async () => {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'nearby-race-fixture-key' });
      writeConfig(fixture, 'openrouter');
      await captureEndedSession(fixture, { sessionId: 'race-before', prompts: ['Record the retry behavior.'] });
      const id = eventId(fixture, 'Record the retry behavior.');
      await observeAt(fixture, NOW, async () => openAiResponse(providerOutput(id)));
      const memory = fixture.withDb((db) => db.prepare("SELECT id, title, body FROM memories WHERE type = 'discovery'").get()!);
      await captureEndedSession(fixture, { sessionId: 'race-after', prompts: ['Explain the retry behavior.'] });
      let raced = false;
      let leaked = false;
      await runObserveForFixture(fixture, { now: () => NOW + 1_000, maxRunMs: 2_000,
        detect: async (input) => {
          const atNearby = input.text === memory.title && input.fields?.includes(String(memory.body));
          const atSend = input.text.startsWith('{"repo_ref"');
          if (!raced && (verdict === 'late' || verdict === 'deleted' ? atSend : atNearby)) {
            raced = true;
            fixture.withDb((db) => {
              if (verdict === 'deleted') db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(NOW, memory.id);
              else db.prepare("UPDATE memories SET sensitivity = 'private' WHERE id = ?").run(memory.id);
            });
            if (verdict === 'failed') return { ok: false, reason: 'detector_error' };
          }
          return await detectSync(input);
        },
        fetch: async (_url, options) => {
          const input = sentInput(options);
          leaked ||= input.nearby.some((candidate) => candidate.id === memory.id);
          const output = providerOutput(input.events[0].id);
          output.observations[0].body = 'A separate retry result was recorded.';
          return openAiResponse(output);
        } });
      assert.equal(raced, true);
      assert.equal(leaked, false);
      fixture.withDb((db) => {
        const stored = db.prepare('SELECT sensitivity, deleted_at FROM memories WHERE id = ?').get(memory.id)!;
        if (verdict === 'deleted') assert.equal(stored.deleted_at, NOW);
        else assert.equal(stored.sensitivity, 'private');
      });
    });
  });
}

test('partial source prefixes are rechecked for secrets without contributing them to generation', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'previous-prefix-fixture-key' });
    writeConfig(fixture, 'openrouter');
    await captureEndedSession(fixture, { sessionId: 'prefix-recheck', prompts: [], tools: [{ id: 'partial', path: 'src/file.ts' }] });
    fixture.withDb((db) => db.exec("UPDATE raw_events SET classification_state = 'partial', content = 'rotated-prefix-fixture-value' WHERE kind = 'tool_call'"));
    fixture.env.OBOETE_OPENROUTER_API_KEY = 'rotated-prefix-fixture-value';
    await observeAt(fixture, NOW, async () => { assert.fail('a partial source cannot be sent'); });
    fixture.withDb((db) => {
      const source = db.prepare("SELECT processing_state, content FROM raw_events WHERE kind = 'tool_call'").get()!;
      assert.equal(source.processing_state, 'excluded');
      assert.equal(source.content, null);
    });
  });
});

for (const eventName of ['PreToolUse', 'PostToolUse']) test(`capture sanitizes ${eventName} path provenance before it enters storage`, async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'path-credential-fixture-value' });
    const root = fixtureRepo(fixture, 'redaction-checkout');
    await fixture.capture(eventName, { session_id: 'path-redaction', cwd: root,
      tool_use_id: 'read', tool_name: 'Read', tool_input: { file_path: 'src/path-credential-fixture-value.txt' },
      tool_response: { file: { content: 'Ordinary file content.' } } });
    fixture.withDb((db) => {
      const stored = db.prepare("SELECT sensitivity, payload_json FROM raw_events WHERE kind IN ('tool_call', 'tool_result')").get()!;
      assert.equal(stored.sensitivity, 'secret');
      assert.ok(!String(stored.payload_json).includes('path-credential-fixture-value'));
    });
  });
});

for (const sensitivity of ['eligible', 'private']) {
  test(`retry rechecks ${sensitivity} sources against rotated credentials`, async () => {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'previous-fixture-key' });
      writeConfig(fixture, 'openrouter');
      await captureEndedSession(fixture, { sessionId: 'credential-recheck', prompts: ['The next fixture credential is rotated-fixture-value.'] });
      fixture.withDb((db) => db.prepare("UPDATE raw_events SET sensitivity = ?, classification_state = 'done' WHERE kind = 'prompt'").run(sensitivity));
      fixture.env.OBOETE_OPENROUTER_API_KEY = 'rotated-fixture-value';
      let calls = 0;
      await observeAt(fixture, NOW, async () => { calls += 1; assert.fail('private content must not be sent'); });
      assert.equal(calls, 0);
      fixture.withDb((db) => {
        const source = db.prepare("SELECT processing_state, content, payload_json FROM raw_events WHERE kind = 'prompt'").get()!;
        assert.equal(source.processing_state, 'excluded');
        assert.equal(source.content, null);
        assert.equal(source.payload_json, null);
      });
    });
  });
}

test('retry rechecks normalized input and current repository secret paths', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'previous-input-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const root = fixtureRepo(fixture, 'path-checkout');
    await captureEndedSession(fixture, { sessionId: 'path-recheck', cwd: root, prompts: ['Review the upload flow.'],
      tools: [{ id: 'path-change', path: 'src/blocked.ts' }, { id: 'input-change', path: 'src/input.ts' }] });
    fixture.withDb((db) => {
      db.exec("UPDATE raw_events SET sensitivity = 'eligible', classification_state = 'done' WHERE kind = 'tool_call'");
      db.prepare(`UPDATE raw_events SET payload_json = json_set(payload_json, '$.input.command', ?)
        WHERE kind = 'tool_call' AND payload_json LIKE '%src/input.ts%'`).run('printf rotated-input-fixture-value');
    });
    writeFileSync(join(root, '.oboete.toml'), '[privacy]\nsecret_paths = ["src/blocked.ts"]\n');
    fixture.env.OBOETE_OPENROUTER_API_KEY = 'rotated-input-fixture-value';
    await observeAt(fixture, NOW, async (_url, options) => {
      const input = sentInput(options);
      assert.ok(input.events.every((event) => event.kind === 'prompt'));
      const output = providerOutput(input.events[0].id);
      return openAiResponse(output);
    });
    fixture.withDb((db) => {
      const rows = db.prepare("SELECT processing_state, content, payload_json FROM raw_events WHERE kind = 'tool_call'").all();
      assert.equal(rows.length, 2);
      for (const row of rows) {
        assert.equal(row.processing_state, 'excluded');
        assert.equal(row.content, null);
        assert.equal(row.payload_json, null);
      }
    });
  });
});

test('a failed current detector defers cached eligible content without sending it', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'detector-fixture-key' });
    writeConfig(fixture, 'openrouter');
    await captureEndedSession(fixture, { sessionId: 'detector-recheck', prompts: ['Current detection must finish.'] });
    fixture.withDb((db) => db.exec("UPDATE raw_events SET sensitivity = 'eligible' WHERE kind = 'prompt'"));
    let calls = 0;
    await runObserveForFixture(fixture, { now: () => NOW, maxRunMs: 1_000,
      detect: async () => ({ ok: false, reason: 'detector_error' }),
      fetch: async () => { calls += 1; assert.fail('a failed detector must prevent sending'); } });
    assert.equal(calls, 0);
    fixture.withDb((db) => assert.equal(db.prepare("SELECT processing_state FROM raw_events WHERE kind = 'prompt'").get()?.processing_state, 'waiting'));
    await runObserveForFixture(fixture, { now: () => NOW + 5 * 60_000, maxRunMs: 1_000,
      detect: async () => ({ ok: false, reason: 'detector_error' }),
      fetch: async () => { calls += 1; assert.fail('a failed detector must prevent sending'); } });
    assert.equal(calls, 0);
    fixture.withDb((db) => assert.equal(db.prepare("SELECT retry_after FROM raw_events WHERE kind = 'prompt'").get()?.retry_after,
      NOW + 15 * 60_000, 'a second detector failure follows the same exponential retry policy'));
  });
});

test('an explicit source request processes a single active turn immediately', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'manual-active-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const root = fixtureRepo(fixture, 'manual-checkout');
    await fixture.capture('SessionStart', { session_id: 'manual-active', cwd: root, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { session_id: 'manual-active', cwd: root, prompt_id: 'one', prompt: 'Remember the upload decision.' });
    const id = eventId(fixture, 'Remember the upload decision.');
    let calls = 0;
    assert.equal(await runObserve(['--reprocess-source', id], { env: fixture.env, now: () => NOW,
      maxRunMs: 1_000, fetch: async () => { calls += 1; return openAiResponse(providerOutput(id)); } }), 0);
    assert.equal(calls, 1);
  });
});

test('a nearby memory is rechecked before its body is included in a new request', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'previous-nearby-fixture-key' });
    writeConfig(fixture, 'openrouter');
    await captureEndedSession(fixture, { sessionId: 'nearby-before', prompts: ['Record the retry behavior.'] });
    const source = eventId(fixture, 'Record the retry behavior.');
    await observeAt(fixture, NOW, async () => {
      const output = providerOutput(source);
      output.observations[0].body = 'Retry behavior uses rotated-nearby-fixture-value for the example.';
      return openAiResponse(output);
    });
    const memoryId = fixture.withDb((db) => String(db.prepare("SELECT id FROM memories WHERE body LIKE '%rotated-nearby-fixture-value%'").get()!.id));
    fixture.withDb((db) => db.prepare("INSERT INTO memory_sources (memory_id, citation_kind, citation_value) VALUES (?, 'file_read', 'src/rotated-nearby-fixture-value.txt')").run(memoryId));
    fixture.env.OBOETE_OPENROUTER_API_KEY = 'rotated-nearby-fixture-value';
    await captureEndedSession(fixture, { sessionId: 'nearby-after', prompts: ['Explain the retry behavior.'] });
    let calls = 0;
    await observeAt(fixture, NOW + 1_000, async (_url, options) => {
      calls += 1;
      assert.ok(!String(options?.body).includes('rotated-nearby-fixture-value'));
      const input = sentInput(options);
      assert.ok(input.nearby.every((candidate) => candidate.id !== memoryId));
      return openAiResponse(providerOutput(input.events[0].id));
    });
    assert.equal(calls, 1);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(memoryId)?.sensitivity, 'secret');
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ? AND evidence IS NOT NULL').get(memoryId)?.n, 0);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ? AND citation_value IS NOT NULL').get(memoryId)?.n, 0);
    });
  });
});

test('credential changes after detection cancel the actual send', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'previous-send-fixture-key' });
    writeConfig(fixture, 'openrouter');
    await captureEndedSession(fixture, { sessionId: 'send-recheck', prompts: ['The example value is rotated-send-fixture-value.'] });
    let calls = 0;
    await runObserveForFixture(fixture, { now: () => NOW, maxRunMs: 1_000,
      detect: async (input) => {
        const checked = await detectSync(input);
        if (input.text.startsWith('{"repo_ref"')) fixture.env.OBOETE_OPENROUTER_API_KEY = 'rotated-send-fixture-value';
        return checked;
      },
      fetch: async () => { calls += 1; assert.fail('changed consent must prevent sending'); } });
    assert.equal(calls, 0);
    fixture.withDb((db) => assert.equal(db.prepare("SELECT degraded_reason FROM observation_batches WHERE state = 'fallback'").get()?.degraded_reason, 'consent_changed'));
  });
});

test('a large source and more than fifty sources complete through lossless pages across bounded runs', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'page-test-key' });
    writeConfig(fixture, 'openrouter');
    const large = `FIRST ${'The upload operation is recorded. 😀 '.repeat(1_400)} LAST`;
    await captureEndedSession(fixture, {
      sessionId: 'paged-recovery',
      prompts: [large, ...Array.from({ length: 60 }, (_, index) => `Upload fact number ${index}.`)],
    });
    const sourceId = eventId(fixture, large);
    const sent: ObserverInput['events'] = [];
    let calls = 0;
    const fetch: ObserveDeps['fetch'] = async (_url, options) => {
      calls += 1;
      const input = sentInput(options);
      assert.ok(JSON.stringify(input).length <= 12_000);
      assert.ok(input.events.length <= 50);
      assert.deepEqual(input.free_summaries, {});
      sent.push(...input.events);
      const output = providerOutput(input.events[0].id);
      output.observations[0].source_event_ids = input.events.map((event) => event.id);
      return openAiResponse(output);
    };
    assert.equal(await observeAt(fixture, NOW, fetch), 0);
    // The two-second fixture budget may release between pages; the next worker must resume the
    // exact remaining range. Production retains the same bounded-run contract under load.
    for (let attempt = 1; attempt <= 5; attempt += 1) {
      if (fixture.withDb((db) => db.prepare("SELECT 1 FROM raw_events WHERE kind = 'prompt' AND processing_state <> 'processed' LIMIT 1").get()) === undefined) break;
      assert.equal(await observeAt(fixture, NOW + 5_000 * attempt, fetch), 0);
    }
    assert.ok(calls > 2);
    const fragments = sent.filter((event) => event.id === sourceId).map((event) => event.fragment!);
    assert.ok(fragments.length > 1);
    let offset = 0;
    for (const fragment of fragments) {
      assert.equal(fragment.start, offset, 'acknowledged ranges cannot repeat or skip');
      offset = fragment.end;
    }
    assert.equal(offset, fragments[0].total, JSON.stringify(fixture.withDb((db) => ({ calls,
      source: db.prepare('SELECT processing_state, processing_attempts, retry_after, processing_offset FROM raw_events WHERE id = ?').get(sourceId),
      outcomes: db.prepare(`SELECT outcome, reason, portion_start, portion_end FROM observation_batch_sources
        WHERE raw_event_id = ? ORDER BY recorded_at`).all(sourceId),
    }))));
    assert.equal(JSON.parse(fragments.map((fragment) => fragment.text).join('')).text, large);
    assert.notEqual(sent[1].id, sourceId, 'other sources get a turn before the large source continues');
    fixture.withDb((db) => {
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE kind = 'prompt' AND processing_state <> 'processed'").get()?.n, 0);
      assert.ok(Number(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE evidence IS NOT NULL').get()?.n) > 0);
    });
  });
});

test('an interrupted page resumes the same range without partial memory effects', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'page-crash-test-key' });
    writeConfig(fixture, 'openrouter');
    const large = 'The upload operation is recorded. '.repeat(900);
    await captureEndedSession(fixture, { sessionId: 'page-crash', prompts: [large] });
    const sourceId = eventId(fixture, large);
    const ranges: ObserverInput['events'] = [];
    const fetch: ObserveDeps['fetch'] = async (_url, options) => {
      const input = sentInput(options);
      ranges.push(...input.events);
      return openAiResponse(providerOutput(sourceId));
    };
    await runObserveForFixture(fixture, {
      now: () => NOW, maxRunMs: 2_000, fetch,
      applyHook: async () => { throw new Error('fixture interruption before apply'); },
    });
    assert.equal(ranges.length, 1);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT processing_offset FROM raw_events WHERE id = ?').get(sourceId)?.processing_offset, 0);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
      assert.ok(Number(db.prepare('SELECT portion_end FROM observation_batch_sources WHERE raw_event_id = ?').get(sourceId)?.portion_end) > 0);
    });
    await observeAt(fixture, NOW + 3 * 60_000, fetch);
    assert.deepEqual(ranges[1], ranges[0]);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(sourceId)?.processing_state, 'processed');
      const duplicate = db.prepare(`SELECT COUNT(*) AS n FROM (
        SELECT memory_id, raw_event_id, source_hash, portion_start, portion_end FROM memory_sources
        WHERE evidence IS NOT NULL GROUP BY 1, 2, 3, 4, 5 HAVING COUNT(*) > 1)`).get();
      assert.equal(duplicate?.n, 0);
    });
  });
});

test('an oversized legacy pending batch is repaged before provider processing', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'legacy-page-test-key' });
    writeConfig(fixture, 'openrouter');
    await captureEndedSession(fixture, {
      sessionId: 'legacy-page', prompts: Array.from({ length: 60 }, (_, index) => `Upload fact ${index}.`),
    });
    fixture.withDb((db) => {
      const session = db.prepare('SELECT id, repo_id FROM sessions').get()!;
      db.prepare(`INSERT INTO observation_batches
        (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, claimed_at)
        VALUES ('legacy-large', ?, ?, 'legacy-end', 'remote_observer', 'session_end', 'pending', 'old-worker', ?)`)
        .run(session.repo_id, session.id, NOW);
      db.exec("UPDATE raw_events SET batch_id = 'legacy-large', sensitivity = 'eligible' WHERE kind = 'prompt'");
      db.prepare(`INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, recorded_at)
        SELECT batch_id, id, 'assigned', ? FROM raw_events WHERE batch_id = 'legacy-large'`).run(NOW);
    });
    let largestBatch = 0;
    await observeAt(fixture, NOW + 3 * 60_000, async (_url, options) => {
      largestBatch = Math.max(largestBatch, fixture.withDb((db) => Number(db.prepare(`SELECT COUNT(*) AS n
        FROM raw_events r JOIN observation_batches b ON b.id = r.batch_id WHERE b.state = 'running'`).get()?.n)));
      const input = sentInput(options);
      const output = providerOutput(input.events[0].id);
      output.observations[0].source_event_ids = input.events.map((event) => event.id);
      return openAiResponse(output);
    });
    assert.ok(largestBatch > 0 && largestBatch <= 50);
    fixture.withDb((db) => assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE kind = 'prompt' AND processing_state <> 'processed'").get()?.n, 0,
      JSON.stringify({
        sources: db.prepare("SELECT processing_state, COUNT(*) AS n FROM raw_events WHERE kind = 'prompt' GROUP BY 1").all(),
        batches: db.prepare('SELECT state, degraded_reason FROM observation_batches').all(),
        outcomes: db.prepare('SELECT outcome, reason, COUNT(*) AS n FROM observation_batch_sources GROUP BY 1, 2').all(),
      })));
  });
});

test('explicit reprocessing selects one retained legacy source without starting the other history', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'legacy-choice-test-key' });
    writeConfig(fixture, 'openrouter');
    const prompts = ['The upload retry uses three attempts.', 'Another historical upload note.'];
    await captureEndedSession(fixture, { sessionId: 'legacy-choice', prompts });
    const ids = prompts.map((prompt) => eventId(fixture, prompt));
    fixture.withDb((db) => db.exec("UPDATE raw_events SET processing_state = 'legacy_unknown' WHERE kind = 'prompt'"));
    await observeAt(fixture, NOW, async () => assert.fail('legacy history needs an explicit choice'));
    let calls = 0;
    assert.equal(await runObserveForFixture(fixture, {
      fetch: async (_url, options) => {
        calls += 1;
        assert.deepEqual(sentInput(options).events.map((event) => event.id), [ids[0]]);
        return openAiResponse(providerOutput(ids[0]));
      },
    }, ['--reprocess-source', ids[0]]), 0);
    assert.equal(calls, 1);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(ids[0])?.processing_state, 'processed');
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(ids[1])?.processing_state, 'legacy_unknown');
    });
  });
});

test('incomplete captured material is held without repeated automatic attempts', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const prompt = 'This accepted prefix came from an incomplete capture.';
    await captureEndedSession(fixture, { sessionId: 'incomplete-capture', prompts: [prompt] });
    const id = eventId(fixture, prompt);
    fixture.withDb((db) => db.prepare("UPDATE raw_events SET classification_state = 'partial', truncated = 1 WHERE id = ?").run(id));
    await observeAt(fixture, NOW, async () => assert.fail('incomplete text cannot reach a provider'));
    await observeAt(fixture, NOW + 40 * DAY, async () => assert.fail('incomplete text cannot reach a provider'));
    fixture.withDb((db) => {
      const row = db.prepare('SELECT content, retry_after, processing_state FROM raw_events WHERE id = ?').get(id);
      assert.equal(row?.content, prompt);
      assert.equal(row?.processing_state, 'waiting');
      assert.equal(row?.retry_after, null);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM observation_batch_sources WHERE raw_event_id = ?').get(id)?.n, 1);
      assert.match(generationItem(db, false).reason, /1 incomplete captures/);
    });
  });
});

for (const decision of ['update', 'delete'] as const) {
  for (const timing of ['older', 'unknown_source', 'unknown_target']) test(`a recovered ${timing} source cannot ${decision} protected knowledge`, async () => {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'source-order-test-key' });
      writeConfig(fixture, 'openrouter');
      const oldText = 'The upload service retry originally ran once.';
      await captureEndedSession(fixture, { sessionId: `older-${decision}`, prompts: [oldText] });
      const oldId = eventId(fixture, oldText);
      fixture.withDb((db) => db.prepare('UPDATE raw_events SET captured_at = ? WHERE id = ?')
        .run(timing === 'unknown_source' ? null : NOW - DAY, oldId));
      await observeAt(fixture, NOW, async () => new Response('unavailable', { status: 503 }));
      const currentText = 'The upload service retry now runs three times.';
      await captureEndedSession(fixture, { sessionId: `newer-${decision}`, prompts: [currentText] });
      const currentId = eventId(fixture, currentText);
      const current = providerOutput(currentId);
      await observeAt(fixture, NOW + 60_000, async () => openAiResponse(current));
      const currentMemory = fixture.withDb((db) => db.prepare(
        "SELECT id FROM memories WHERE type <> 'session_summary' AND degraded_reason IS NULL",
      ).get());
      assert.ok(currentMemory);
      const memoryId = String(currentMemory.id);
      if (timing === 'unknown_target') fixture.withDb((db) => db.prepare('UPDATE memories SET source_captured_at = NULL WHERE id = ?').run(memoryId));
      const old = providerOutput(oldId);
      old.observations[0].title = 'Original upload service retry';
      old.observations[0].body = 'The upload service retry originally ran once.';
      old.observations[0].classification = { decision, target: memoryId, reason: 'Original source describes the retry.' };
      await observeAt(fixture, NOW + 6 * 60_000, async (_url, options) => {
        assert.ok(sentInput(options).nearby.some((memory) => memory.id === memoryId));
        return openAiResponse(old);
      });
      fixture.withDb((db) => {
        const memory = db.prepare('SELECT valid_to, deleted_at FROM memories WHERE id = ?').get(memoryId);
        assert.equal(memory?.valid_to, null);
        assert.equal(memory?.deleted_at, null);
        assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(oldId)?.processing_state, 'processed');
        const receipt = db.prepare('SELECT * FROM observation_batch_sources WHERE raw_event_id = ? ORDER BY recorded_at DESC LIMIT 1').get(oldId)!;
        assert.equal(typeof receipt.historical_actions_json, 'string', 'the original action and admitted target remain inspectable');
        assert.deepEqual(JSON.parse(String(receipt.historical_actions_json)), [{ decision, target: memoryId, reason: 'capture_time_order' }]);
        if (decision === 'update') {
          const historical = db.prepare('SELECT valid_to, superseded_by FROM memories WHERE title = ?').get(old.observations[0].title);
          assert.notEqual(historical?.valid_to, null);
          assert.equal(historical?.superseded_by, memoryId);
        }
      });
    });
  });
}

test('failed generation retains accepted information and recovers once after the selected provider returns', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'recovery-test-key' });
    writeConfig(fixture, 'openrouter');
    const prompt = 'The upload service retries once after a failure.';
    const assistant = 'Confirmed: the upload service retries once after a failure.';
    await captureEndedSession(fixture, { sessionId: 'recover-source', prompts: [prompt], assistant });
    const sourceId = eventId(fixture, prompt);
    const assistantId = eventId(fixture, assistant);
    fixture.withDb((db) => assert.match(generationItem(db, false).reason, /2 pending/));
    let attempts = 0;
    let temporaryIds: string[] = [];

    assert.equal(await observeAt(fixture, NOW, async () => {
      attempts += 1;
      return new Response('unavailable', { status: 503 });
    }), 1);
    assert.equal(attempts, 1);
    fixture.withDb((db) => {
      assert.equal(generationItem(db, false).status, 'degraded');
      assert.match(generationItem(db, false).reason, /2 waiting/);
      assert.equal(db.prepare('SELECT summary_state FROM sessions').get()?.summary_state, 'pending');
      assert.notEqual(db.prepare('SELECT latest_summary_memory_id FROM sessions').get()?.latest_summary_memory_id, null);
      temporaryIds = db.prepare('SELECT id FROM memories WHERE degraded_reason IS NOT NULL').all()
        .map((row) => String(row.id));
      assert.ok(temporaryIds.length > 0);
    });

    await observeAt(fixture, NOW + 60_000, async () => {
      assert.fail('generation must wait until its retry is due');
    });
    assert.equal(readFileSync(fixture.paths.observeLog, 'utf8').includes('reason=max_run'), false,
      'waiting sources must let the worker exit without polling to its run limit');

    const recoveredAt = NOW + 40 * DAY;
    assert.equal(await observeAt(fixture, recoveredAt, async () => {
      attempts += 1;
      const answer = providerOutput(sourceId);
      answer.observations[0].source_event_ids.push(assistantId);
      return openAiResponse(answer);
    }), 0);

    fixture.withDb((db) => {
      const source = db.prepare('SELECT * FROM raw_events WHERE id = ?').get(sourceId);
      assert.equal(source?.content, prompt, 'accepted source was lost while generation was unavailable');
      assert.equal(source?.processing_state, 'processed');
      assert.ok(Number(source?.processed_at) >= recoveredAt);
      assert.equal(source?.expires_at, Number(source?.processed_at) + 30 * DAY);
      assert.equal(generationItem(db, false).status, 'healthy');
      assert.match(generationItem(db, false).reason, /2 recovered/);
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
      assert.deepEqual(db.prepare(
        'SELECT outcome FROM observation_batch_sources WHERE raw_event_id = ? ORDER BY recorded_at',
      ).all(sourceId).map((row) => row.outcome), ['deferred', 'processed']);
      for (const id of temporaryIds) {
        const memory = db.prepare('SELECT degraded_reason, valid_to FROM memories WHERE id = ?').get(id);
        assert.ok(memory?.degraded_reason === null || memory?.valid_to !== null,
          'recovery must upgrade or retire temporary guidance');
      }
    });
    assert.equal(attempts, 2);

    await observeAt(fixture, recoveredAt + 60_000, async () => {
      assert.fail('successfully processed sources must not run again');
    });
    fixture.withDb((db) => {
      assert.equal(db.prepare(
        "SELECT COUNT(*) AS n FROM memories WHERE type <> 'session_summary' AND degraded_reason IS NULL",
      ).get()?.n, 1);
    });
  });
});

test('matching provider output upgrades a temporary memory without duplicating it', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const prompt = 'The uploader retries once after a failure.';
    const assistant = 'The retry behavior has been confirmed.';
    await captureEndedSession(fixture, { sessionId: 'matching-recovery', prompts: [prompt], assistant });
    const sourceId = eventId(fixture, assistant);
    await observeAt(fixture, NOW, async () => assert.fail('no provider was selected'));
    const temporary = fixture.withDb((db) => db.prepare(
      "SELECT id, title, body, source_batch_id FROM memories WHERE type <> 'session_summary' LIMIT 1",
    ).get());
    assert.ok(temporary);
    const answer = providerOutput(sourceId);
    answer.observations[0].source_event_ids.push(eventId(fixture, prompt));
    answer.observations[0].title = String(temporary.title);
    answer.observations[0].body = String(temporary.body);
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'matching-recovery-test-key' });
    writeConfig(fixture, 'openrouter');
    await observeAt(fixture, NOW + 6 * 60_000, async () => openAiResponse(answer));
    fixture.withDb((db) => {
      const memory = db.prepare('SELECT degraded_reason, source_batch_id, valid_to FROM memories WHERE id = ?').get(temporary.id);
      assert.equal(memory?.degraded_reason, null);
      assert.equal(memory?.valid_to, null);
      assert.notEqual(memory?.source_batch_id, temporary.source_batch_id);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories WHERE title = ? AND body = ?').get(temporary.title, temporary.body)?.n, 1);
    });
  });
});

test('confirmation followed by deletion cannot restore evidence for the deleted memory', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'delete-evidence-fixture-key' });
    writeConfig(fixture, 'openrouter');
    await captureEndedSession(fixture, { sessionId: 'before-delete', prompts: ['Record the retry behavior.'] });
    const previous = eventId(fixture, 'Record the retry behavior.');
    await observeAt(fixture, NOW, async () => openAiResponse(providerOutput(previous)));
    const memoryId = fixture.withDb((db) => String(db.prepare("SELECT id FROM memories WHERE type = 'discovery'").get()!.id));
    await captureEndedSession(fixture, { sessionId: 'after-delete', prompts: ['Remove the obsolete retry behavior.'] });
    const sourceId = eventId(fixture, 'Remove the obsolete retry behavior.');
    await observeAt(fixture, NOW + 60_000, async (_url, options) => {
      assert.ok(sentInput(options).nearby.some((memory) => memory.id === memoryId));
      const confirm = providerOutput(sourceId).observations[0];
      const remove = providerOutput(sourceId).observations[0];
      confirm.classification = { decision: 'noop', target: memoryId, reason: 'The existing note matches.' };
      remove.classification = { decision: 'delete', target: memoryId, reason: 'The obsolete note is removed.' };
      return openAiResponse({ ...providerOutput(sourceId), observations: [confirm, remove] });
    });
    fixture.withDb((db) => {
      assert.notEqual(db.prepare('SELECT deleted_at FROM memories WHERE id = ?').get(memoryId)?.deleted_at, null);
      assert.equal(db.prepare(`SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ?
        AND (evidence IS NOT NULL OR capture_root IS NOT NULL OR source_paths_json IS NOT NULL)`).get(memoryId)?.n, 0);
    });
  });
});

test('matching only part of a temporary source group keeps its degraded state', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const prompt = 'Record the uploader edit.';
    await captureEndedSession(fixture, {
      sessionId: 'partial-confirmation', prompts: [prompt],
      tools: [{ id: 'edit-uploader', path: 'src/uploader.ts', text: 'retry implementation' }],
    });
    await observeAt(fixture, NOW, async () => assert.fail('no provider was selected'));
    const temporary = fixture.withDb((db) => db.prepare(
      "SELECT id, title, body, degraded_reason FROM memories WHERE type = 'change' LIMIT 1",
    ).get());
    assert.ok(temporary);
    const answer = providerOutput(eventId(fixture, prompt));
    answer.observations[0].title = String(temporary.title);
    answer.observations[0].body = String(temporary.body);
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'partial-confirmation-test-key' });
    writeConfig(fixture, 'openrouter');
    await observeAt(fixture, NOW + 6 * 60_000, async () => openAiResponse(answer));
    fixture.withDb((db) => {
      assert.ok(Number(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE processing_state = 'waiting'").get()?.n) > 0);
      const memory = db.prepare('SELECT degraded_reason, valid_to FROM memories WHERE id = ?').get(temporary.id);
      assert.equal(memory?.degraded_reason, temporary.degraded_reason);
      assert.equal(memory?.valid_to, null);
    });
  });
});

test('a mixed fallback is split again after the user selects remote and then local generation', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const publicText = 'The upload path retries once.';
    const privateText = 'PRIVATE-SOURCE-MARKER describes a local-only experiment.';
    await captureEndedSession(fixture, { sessionId: 'mixed-recovery', prompts: [publicText, privateText] });
    const publicId = eventId(fixture, publicText);
    const privateId = eventId(fixture, privateText);
    fixture.withDb((db) => {
      db.prepare("UPDATE raw_events SET sensitivity = 'private' WHERE id = ?").run(privateId);
    });
    assert.equal(await observeAt(fixture, NOW, async () => assert.fail('no provider was selected')), 1);

    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'mixed-recovery-test-key' });
    writeConfig(fixture, 'openrouter');
    let remoteCalls = 0;
    await observeAt(fixture, NOW + 6 * 60_000, async (_url, options) => {
      remoteCalls += 1;
      assert.equal(String(options?.body).includes(privateText), false);
      assert.equal(String(options?.body).includes(privateId), false);
      return openAiResponse(providerOutput(publicId));
    });
    assert.equal(remoteCalls, 1);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(publicId)?.processing_state, 'processed');
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(privateId)?.processing_state, 'waiting');
    });

    writeFileSync(fixture.paths.config, '[observer]\npreset = "ollama"\nmodel = "fixture-local-model"\n');
    let localCalls = 0;
    await observeAt(fixture, NOW + 20 * 60_000, async (_url, options) => {
      localCalls += 1;
      assert.ok(String(options?.body).includes(privateText));
      return openAiResponse(providerOutput(privateId), 'fixture-local-model');
    });
    assert.equal(localCalls, 1);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(privateId)?.processing_state, 'processed');
    });
  });
});

test('processed activity expires after thirty days while supporting knowledge evidence remains', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'retention-test-key' });
    writeConfig(fixture, 'openrouter');
    const routine = 'A routine progress marker without a reusable fact.';
    const fact = 'The upload path retries after a failure.';
    await captureEndedSession(fixture, { sessionId: 'retained-evidence', prompts: [routine, fact] });
    const routineId = eventId(fixture, routine);
    const factId = eventId(fixture, fact);
    const noop = providerOutput(routineId).observations[0];
    noop.title = 'Routine activity';
    noop.body = '';
    noop.classification = { decision: 'noop', target: null, reason: 'No reusable information.' };
    assert.equal(await observeAt(fixture, NOW, async () => openAiResponse({
      ...providerOutput(factId),
      observations: [noop, ...providerOutput(factId).observations],
    })), 0);

    const noRequest: typeof fetch = async () => assert.fail('processed activity must not be regenerated');
    await observeAt(fixture, NOW + 29 * DAY, noRequest);
    fixture.withDb((db) => assert.equal(db.prepare('SELECT content FROM raw_events WHERE id = ?').get(routineId)?.content, routine));
    await observeAt(fixture, NOW + 31 * DAY, noRequest);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT id FROM raw_events WHERE id = ?').get(routineId), undefined);
      assert.equal(db.prepare('SELECT id FROM raw_events WHERE id = ?').get(factId), undefined);
      const evidence = db.prepare('SELECT memory_id, evidence FROM memory_sources WHERE raw_event_id = ? AND evidence IS NOT NULL').get(factId);
      assert.ok(evidence);
      assert.equal(JSON.parse(String(evidence.evidence)).text, fact);
      assert.equal(db.prepare('SELECT outcome FROM observation_batch_sources WHERE raw_event_id = ?').get(routineId)?.outcome, 'processed');
      db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(NOW + 32 * DAY, evidence.memory_id);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE memory_id = ? AND evidence IS NOT NULL').get(evidence.memory_id)?.n, 0);
    });
  });
});

for (const state of ['pending', 'running'] as const) {
  test(`a ${state} local batch cannot send private sources after a remote provider is selected`, async () => {
    await withFixture(async (fixture) => {
      const privateText = 'PRIVATE-RECLAIM-MARKER belongs only to local generation.';
      await captureEndedSession(fixture, { sessionId: `stale-local-${state}`, prompts: [privateText] });
      const sourceId = eventId(fixture, privateText);
      fixture.withDb((db) => {
        db.prepare("UPDATE raw_events SET sensitivity = 'private' WHERE id = ?").run(sourceId);
        const token = claimLease(db, { pid: 1, now: NOW });
        assert.notEqual(token, null);
        const batches = createBatches(db, token!, NOW, { preset: 'local' }).created;
        assert.equal(batches.length, 1);
        db.prepare('UPDATE observation_batches SET state = ? WHERE id = ?').run(state, batches[0].id);
        assert.equal(releaseLease(db, token!, () => true), 'released');
      });
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'reclaim-test-key' });
      writeConfig(fixture, 'openrouter');
      let remoteCalls = 0;
      await observeAt(fixture, NOW + 3 * 60_000, async () => {
        remoteCalls += 1;
        return openAiResponse(providerOutput(sourceId));
      });
      assert.equal(remoteCalls, 0, 'a previous local destination cannot authorize the current remote provider');
      fixture.withDb((db) => {
        assert.equal(db.prepare('SELECT content FROM raw_events WHERE id = ?').get(sourceId)?.content, privateText);
        assert.notEqual(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(sourceId)?.processing_state, 'processed');
      });
    });
  });
}

test('an early failed source is not retried again during the same long worker run', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'same-run-test-key' });
    writeConfig(fixture, 'openrouter');
    const prompts = ['The first service retries once.', 'The second service retries once.'];
    for (const [index, prompt] of prompts.entries()) {
      await captureEndedSession(fixture, { sessionId: `separate-${index}`, prompts: [prompt] });
    }
    const ids = prompts.map((prompt) => eventId(fixture, prompt));
    let now = NOW;
    let calls = 0;
    await runObserveForFixture(fixture, {
      now: () => now,
      fetch: async (_url, options) => {
        calls += 1;
        if (calls === 1 || calls > 2) return new Response('unavailable', { status: 503 });
        // The second session takes long enough that the first source's retry becomes due.
        now += 6 * 60_000;
        const id = ids.find((sourceId) => String(options?.body).includes(sourceId));
        assert.ok(id);
        return openAiResponse(providerOutput(id));
      },
    });
    assert.equal(calls, 2, 'a settled failure belongs to a later worker invocation');
  });
});

test('the worker time budget also bounds an inherited queue of pending batches', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'backlog-time-fixture-key' });
    writeConfig(fixture, 'openrouter');
    for (let index = 0; index < 3; index += 1) await captureEndedSession(fixture, {
      sessionId: `backlog-${index}`, prompts: [`Record retry decision ${index}.`],
    });
    fixture.withDb((db) => {
      db.exec("UPDATE raw_events SET sensitivity = 'eligible' WHERE kind = 'prompt'");
      const token = claimLease(db, { pid: process.pid, now: NOW })!;
      for (let index = 0; index < 3; index += 1) createBatches(db, token, NOW + index, { preset: 'remote' });
      releaseLease(db, token, () => true);
    });
    let at = NOW;
    let calls = 0;
    await runObserveForFixture(fixture, { now: () => at, maxRunMs: 1_000,
      fetch: async (_url, options) => {
        calls += 1;
        at += 2_000;
        return openAiResponse(providerOutput(sentInput(options).events[0].id));
      } });
    assert.equal(calls, 1);
    fixture.withDb((db) => {
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM observation_batches WHERE state = 'pending'").get()?.n, 2);
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
  });
});

test('a waiting source later classified as secret is excluded and its temporary guidance is quarantined', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const marker = 'DETECT-LATER-MARKER';
    const prompt = `${marker}: ${'The initial classification is temporarily unavailable. '.repeat(4)}`;
    await captureEndedSession(fixture, { sessionId: 'later-secret', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    await runObserveForFixture(fixture, {
      detect: async (input) => input.text === prompt
        ? { ok: false, reason: 'detector_error' }
        : await detectSync(input),
    });
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(sourceId)?.processing_state, 'waiting');
      assert.equal(db.prepare('SELECT summary_state FROM sessions').get()?.summary_state, 'pending');
    });

    await runObserveForFixture(fixture, {
      now: () => NOW + 6 * 60_000,
      detect: async (input) => input.text.includes(marker)
        ? {
          ok: true, text: input.text.replaceAll(marker, '[REDACTED:fixture]'), texts: [],
          redactions: [{ rule: 'fixture', count: 1 }], privateRemoved: 0, sensitivity: 'secret', pathRule: null,
        }
        : await detectSync(input),
    });
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(sourceId)?.processing_state, 'excluded');
      assert.equal(db.prepare('SELECT summary_state FROM sessions').get()?.summary_state, 'no_content');
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memories WHERE title LIKE ? OR body LIKE ?").get(`%${marker}%`, `%${marker}%`)?.n, 0);
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memories WHERE sensitivity <> 'secret' AND deleted_at IS NULL").get()?.n, 0);
      assert.equal(db.prepare("SELECT message_code FROM diagnostics WHERE kind = 'source_exclusion'").get()?.message_code, 'secret');
    });
  });
});
