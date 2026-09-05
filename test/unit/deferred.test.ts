import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { CHANNEL_CAPS } from '../../src/injection/budget.js';
import {
  attachOnPreToolUse,
  closeOnStop,
  confirmOnPostToolUse,
  markFailure,
  storePending,
  type PackValidation,
} from '../../src/injection/deferred.js';
import { whyReport } from '../../src/injection/ledger.js';
import { buildPromptPack, type PromptPackInput } from '../../src/injection/pack.js';
import { oboetePaths } from '../../src/paths.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_700_000_000_000;
const REPO = 'r1';
const CONVERSATION = 'c1';

function insertMemory(
  db: DatabaseSync,
  memory: { id: string; title: string; body: string },
): void {
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, created_at)
     VALUES (?, ?, 'discovery', ?, ?, ?, ?, ?, 'eligible', 'unreviewed', ?)`,
  ).run(
    memory.id,
    REPO,
    memory.title,
    memory.body,
    cjkBigrams(`${memory.title} ${memory.body}`),
    `material_${memory.id}`,
    `content_${memory.id}`,
    NOW - 1_000,
  );
}

function promptInput(overrides: Partial<PromptPackInput> = {}): PromptPackInput {
  return {
    agent: 'grok',
    repoId: REPO,
    repoIdentityDisplay: 'example.test/one',
    sessionId: 's_now',
    conversationId: CONVERSATION,
    turnId: null,
    epoch: 0,
    model: 'grok-4.6-build',
    channelCap: CHANNEL_CAPS.grok,
    contextFraction: 0.05,
    channel: 'grok:UserPromptSubmit',
    now: NOW,
    detect: () => false,
    directives: [],
    repoRoot: '/nonexistent-repository-root',
    state: 'pending',
    prompt: 'retrieval',
    ...overrides,
  };
}

async function withGrok(
  fn: (db: DatabaseSync) => Promise<void>,
  seed: { id: string; title: string; body: string }[] = [
    { id: 'm_1', title: 'Retrieval note one', body: 'The ranking is lexical.' },
  ],
): Promise<void> {
  await withTempHome(async (home) => {
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 });
    db.prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
       VALUES (?, 'remote', 'example.test/one', '/tmp/one', 1, 1)`,
    ).run(REPO);
    db.prepare(
      `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
         started_at, status, turn_count, context_epoch)
       VALUES ('s_now', ?, 'grok', 'native_s_now', ?, 'grok-4.6-build', ?, 'active', 1, 0)`,
    ).run(REPO, CONVERSATION, NOW - 10_000);
    for (const memory of seed) insertMemory(db, memory);
    try {
      await fn(db);
    } finally {
      db.close();
    }
  });
}

async function pendingPack(
  db: DatabaseSync,
  overrides: Partial<PromptPackInput> = {},
  validation?: PackValidation,
): Promise<string> {
  const input = promptInput(overrides);
  const pack = await buildPromptPack(db, input);
  assert.notEqual(pack, null, 'the prompt pack should have been built');
  return storePending(db, {
    conversationId: CONVERSATION,
    epoch: 0,
    pack: pack!,
    now: NOW,
    // The hook validates the merged text with the detector and the corpus it has (FR-018, FR-021).
    validation: validation ?? { detect: input.detect, directives: input.directives },
  });
}

function injectionRow(db: DatabaseSync, id: string): Record<string, unknown> {
  const row = db.prepare('SELECT * FROM injections WHERE id = ?').get(id);
  assert.notEqual(row, undefined);
  return row as Record<string, unknown>;
}

test('the first tool call that runs receives the pack and confirms it', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    const received: string[] = [];

    const first = attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 1 });
    assert.notEqual(first, null);
    received.push(first!);
    assert.equal(injectionRow(db, id).state, 'attempted');

    const confirmed = confirmOnPostToolUse(db, {
      conversationId: CONVERSATION,
      toolCallId: 'call-1',
      exitCode: 0,
      now: NOW + 2,
    });
    assert.equal(confirmed.status, 'emitted');
    assert.equal(confirmed.text, null, 'the pack was already attached, so it is not printed again');

    const row = injectionRow(db, id);
    assert.equal(row.state, 'emitted');
    assert.equal(row.delivery_count, 1);
    const items = whyReport(db, 's_now')[0].items;
    assert.ok(items.every((item) => item.decision === 'included'));

    // A later call of the same turn adds nothing: the pack is confirmed.
    const second = attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-2', now: NOW + 3 });
    assert.equal(second, null);
    assert.equal(received.length, 1, 'the model received the pack once');
  });
});

test('a call that runs and fails still delivered the pack', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    const received = [attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 1 })];
    assert.notEqual(received[0], null);

    // The R13 probe: a failed shell call arrives as PostToolUse with exit_code, context delivered.
    const confirmed = confirmOnPostToolUse(db, {
      conversationId: CONVERSATION,
      toolCallId: 'call-1',
      exitCode: 3,
      now: NOW + 2,
    });
    assert.equal(confirmed.status, 'emitted');

    const row = injectionRow(db, id);
    assert.equal(row.state, 'emitted');
    assert.equal(row.delivery_count, 1);
    const attempts = whyReport(db, 's_now')[0].attempts;
    assert.deepEqual(
      attempts.map((attempt) => [attempt.execution, attempt.delivery]),
      [['failed', 'delivered']],
    );
    assert.equal(received.filter((text) => text !== null).length, 1);
  });
});

test('a denied call delivers nothing and the next call carries the pack again', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    const received: string[] = [];

    const first = attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 1 });
    received.push(first!);
    markFailure(db, {
      conversationId: CONVERSATION,
      toolCallId: 'call-1',
      kind: 'PermissionDenied',
      now: NOW + 2,
    });
    assert.equal(injectionRow(db, id).state, 'attempted', 'a denied call leaves the pack pending');

    const second = attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-2', now: NOW + 3 });
    assert.notEqual(second, null, 'the next call attaches the pack again');
    received.push(second!);
    const confirmed = confirmOnPostToolUse(db, {
      conversationId: CONVERSATION,
      toolCallId: 'call-2',
      now: NOW + 4,
    });
    assert.equal(confirmed.status, 'emitted');

    const row = injectionRow(db, id);
    assert.equal(row.delivery_count, 1, 'only the call that ran delivered the pack');
    const attempts = whyReport(db, 's_now')[0].attempts;
    assert.deepEqual(
      attempts.map((attempt) => [attempt.tool_call_id, attempt.execution, attempt.delivery]),
      [
        ['call-1', 'denied', 'dropped'],
        ['call-2', 'ran', 'delivered'],
      ],
    );
    assert.equal(received.length, 2, 'the hook attached twice, and one attempt reached the model');
  });
});

test('a turn whose chain was stopped by another handler records not_delivered', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    const attached = attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 1 });
    assert.notEqual(attached, null);

    // No PostToolUse and no PermissionDenied ever arrive: an earlier handler denied the call.
    const closed = closeOnStop(db, { conversationId: CONVERSATION, sawAnyToolHook: true, now: NOW + 5 });
    assert.equal(closed, 'omitted');

    const row = injectionRow(db, id);
    assert.equal(row.state, 'omitted');
    assert.equal(row.degraded_reason, 'not_delivered');
    assert.equal(row.delivery_count, 0);
    const report = whyReport(db, 's_now')[0];
    assert.deepEqual(report.attempts.map((attempt) => attempt.delivery), ['dropped']);
    assert.ok(report.items.every((item) => item.decision === 'omitted' && item.reason === 'not_delivered'));

    // The memories were never delivered, so the next turn plans them again.
    const next = await buildPromptPack(db, promptInput({ now: NOW + 10 }));
    assert.notEqual(next, null);
    assert.ok(next!.items.some((item) => item.memoryId === 'm_1' && item.decision === 'planned'));
  });
});

test('a parallel batch delivers the pack once per call and includes its items once', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    const received: string[] = [];

    // Both PreToolUse hooks run before either call finishes (probe: 31 ms apart).
    const first = attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-a', now: NOW + 1 });
    const second = attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-b', now: NOW + 31 });
    assert.notEqual(first, null);
    assert.notEqual(second, null);
    received.push(first!, second!);

    assert.equal(
      confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-a', now: NOW + 40 }).status,
      'emitted',
    );
    assert.equal(
      confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-b', now: NOW + 41 }).status,
      'already',
    );

    const row = injectionRow(db, id);
    assert.equal(row.state, 'emitted');
    // A15: per-call duplicates inside one batch are accepted and counted.
    assert.equal(row.delivery_count, 2);
    assert.equal(received.length, 2);

    const report = whyReport(db, 's_now')[0];
    assert.deepEqual(report.attempts.map((attempt) => attempt.delivery), ['delivered', 'delivered']);
    const included = report.items.filter((item) => item.decision === 'included' && item.memoryId === 'm_1');
    assert.equal(included.length, 1, 'the memory is counted once for the conversation');
  });
});

test('a turn with no tool call at all records no_tool_call', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    const closed = closeOnStop(db, { conversationId: CONVERSATION, sawAnyToolHook: false, now: NOW + 5 });
    assert.equal(closed, 'omitted');
    const row = injectionRow(db, id);
    assert.equal(row.state, 'omitted');
    assert.equal(row.degraded_reason, 'no_tool_call');
    assert.deepEqual(whyReport(db, 's_now')[0].attempts, []);
  });
});

test('a PostToolUse whose PreToolUse never ran prints the pack itself', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    const confirmed = confirmOnPostToolUse(db, {
      conversationId: CONVERSATION,
      toolCallId: 'call-1',
      now: NOW + 2,
    });
    assert.equal(confirmed.status, 'emitted');
    assert.notEqual(confirmed.text, null, 'the pack is emitted from PostToolUse instead');
    assert.ok(confirmed.text!.startsWith('oboete memory context'));
    const row = injectionRow(db, id);
    assert.equal(row.state, 'emitted');
    assert.equal(row.delivery_count, 1);
  });
});

test('the attempt record outlives the raw events it came from', async () => {
  await withGrok(async (db) => {
    await pendingPack(db);
    attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 1 });
    confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 2 });

    db.prepare(
      `INSERT INTO raw_events (id, repo_id, session_id, agent, kind, content, sensitivity,
         classification_state, captured_at, expires_at)
       VALUES ('e1', ?, 's_now', 'grok', 'tool_call', 'run', 'local_only', 'done', ?, ?)`,
    ).run(REPO, NOW, NOW);
    db.prepare('DELETE FROM raw_events').run();

    const report = whyReport(db, 's_now')[0];
    assert.equal(report.attempts.length, 1);
    assert.equal(report.attempts[0].delivery, 'delivered');
    assert.equal(report.deferred, true);
  });
});

test('a second prompt before any tool call merges into the one pending record', async () => {
  await withGrok(
    async (db) => {
      const first = await pendingPack(db, { prompt: 'retrieval' });
      const second = await pendingPack(db, { prompt: 'lease', now: NOW + 100 });
      assert.equal(second, first, 'one pending record per conversation');

      const pending = db
        .prepare(`SELECT COUNT(*) AS n FROM injections WHERE state = 'pending'`)
        .get()?.n;
      assert.equal(pending, 1);

      const text = attachOnPreToolUse(db, {
        conversationId: CONVERSATION,
        toolCallId: 'call-1',
        now: NOW + 200,
      });
      assert.notEqual(text, null);
      assert.ok(text!.includes('Retrieval note one'), 'the planned item stays');
      assert.ok(text!.includes('Lease note two'), 'the new item is added');
      assert.equal(text!.split('Retrieval note one').length - 1, 1, 'and it is not repeated');

      confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 201 });
      const included = whyReport(db, 's_now')
        .flatMap((injection) => injection.items)
        .filter((item) => item.decision === 'included')
        .map((item) => item.memoryId)
        .sort();
      assert.deepEqual(included, ['m_1', 'm_2']);
    },
    [
      { id: 'm_1', title: 'Retrieval note one', body: 'The ranking is lexical.' },
      { id: 'm_2', title: 'Lease note two', body: 'The lease is fenced by its owner token.' },
    ],
  );
});

test('a memory that both merged packs carry is delivered once and counted once', async () => {
  await withGrok(async (db) => {
    const first = await pendingPack(db, { prompt: 'retrieval' });
    const second = await pendingPack(db, { prompt: 'retrieval', now: NOW + 100 });
    assert.equal(second, first, 'one pending record per conversation');

    const text = attachOnPreToolUse(db, {
      conversationId: CONVERSATION,
      toolCallId: 'call-1',
      now: NOW + 200,
    });
    assert.ok(text!.includes('Retrieval note one'), 'the repeated memory is in the pack');
    confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 201 });

    const items = whyReport(db, 's_now').flatMap((injection) => injection.items);
    assert.deepEqual(
      items.map((item) => `${item.memoryId}:${item.decision}:${item.reason ?? '-'}`).sort(),
      ['m_1:included:-', 'm_1:omitted:duplicate_in_conversation'],
      'the delivered row is included and only the merged-away copy is a duplicate',
    );

    // FR-026: the delivered memory is not offered again in the same conversation and epoch.
    const third = await buildPromptPack(db, promptInput({ prompt: 'retrieval', now: NOW + 300 }));
    assert.equal(third, null, 'nothing is left to inject once the memory was delivered');
  });
});

test('a merged pack that trips the secret detector is never stored', async () => {
  await withGrok(
    async (db) => {
      // Neither pack carries the secret on its own: only the two together spell it out, which is
      // exactly the text no builder ever validated (contracts/agents.md "Pack format").
      const detect = (text: string): boolean => text.includes('alpha') && text.includes('beta');
      const validation = { detect, directives: [] };

      const first = await pendingPack(db, { prompt: 'retrieval', detect }, validation);
      const second = await pendingPack(
        db,
        { prompt: 'lease', now: NOW + 100, detect },
        validation,
      );
      assert.equal(second, first, 'the live record still holds the conversation pack');

      const text = attachOnPreToolUse(db, {
        conversationId: CONVERSATION,
        toolCallId: 'call-1',
        now: NOW + 200,
      });
      assert.ok(text!.includes('alpha'), 'the validated pack is still deliverable');
      assert.ok(!text!.includes('beta'), 'the merged text was never stored');

      const items = whyReport(db, 's_now').flatMap((injection) => injection.items);
      assert.deepEqual(
        items
          .filter((item) => item.memoryId === 'm_2')
          .map((item) => `${item.decision}:${item.reason ?? '-'}`),
        ['omitted:secret_detected'],
      );
    },
    [
      { id: 'm_1', title: 'Retrieval note one', body: 'The ranking is alpha.' },
      { id: 'm_2', title: 'Lease note two', body: 'The lease is beta.' },
    ],
  );
});

test('a merged pack that trips the directive corpus is never stored', async () => {
  await withGrok(
    async (db) => {
      const first = await pendingPack(db, { prompt: 'retrieval' });
      // The corpus this hook loaded names a phrase the builder of the first pack did not have.
      const second = await pendingPack(db, { prompt: 'lease', now: NOW + 100 }, {
        detect: () => false,
        directives: ['the lease is fenced'],
      });
      assert.equal(second, first);

      const text = attachOnPreToolUse(db, {
        conversationId: CONVERSATION,
        toolCallId: 'call-1',
        now: NOW + 200,
      });
      assert.ok(!text!.includes('Lease note two'), 'the merged text was never stored');

      const items = whyReport(db, 's_now').flatMap((injection) => injection.items);
      assert.deepEqual(
        items
          .filter((item) => item.memoryId === 'm_2')
          .map((item) => `${item.decision}:${item.reason ?? '-'}`),
        ['omitted:directive'],
      );
    },
    [
      { id: 'm_1', title: 'Retrieval note one', body: 'The ranking is lexical.' },
      { id: 'm_2', title: 'Lease note two', body: 'The lease is fenced by its owner token.' },
    ],
  );
});

test('a merged item that no longer fits the budget is recorded as trimmed', async () => {
  await withGrok(
    async (db) => {
      const first = await pendingPack(db, { prompt: 'retrieval', channelCap: 200 });
      const second = await pendingPack(db, { prompt: 'lease', channelCap: 200, now: NOW + 100 });
      assert.equal(second, first);

      const text = attachOnPreToolUse(db, {
        conversationId: CONVERSATION,
        toolCallId: 'call-1',
        now: NOW + 200,
      });
      assert.ok(text!.length <= 200, `the merged pack keeps the budget: ${text!.length}`);
      const items = whyReport(db, 's_now')[0].items;
      assert.ok(
        items.some((item) => item.memoryId === 'm_2' && item.decision === 'omitted' && item.reason === 'budget'),
        JSON.stringify(items),
      );
    },
    [
      {
        id: 'm_1',
        title: 'Retrieval note one',
        body: 'The ranking is lexical and the budget is small here.',
      },
      {
        id: 'm_2',
        title: 'Lease note two',
        body: 'The lease is fenced by its owner token and this body is long enough to be trimmed.',
      },
    ],
  );
});

test('a PostToolUse for a call that never carried the pack changes nothing', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 1 });
    confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 2 });

    // Every later tool call of the conversation reaches PostToolUse with no attempt of its own:
    // the pack was never attached to it, so it delivered nothing (FR-045 rule 3, SC-010).
    for (const toolCallId of ['call-2', 'call-3']) {
      const late = confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId, now: NOW + 10 });
      assert.equal(late.status, 'none');
      assert.equal(late.text, null);
    }

    const row = injectionRow(db, id);
    assert.equal(row.delivery_count, 1, 'only the attached call counts as a delivery');
    assert.deepEqual(
      whyReport(db, 's_now')[0].attempts.map((attempt) => attempt.tool_call_id),
      ['call-1'],
    );
  });
});

test('a deny and a stop after the pack was delivered still record their attempts', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db);
    for (const toolCallId of ['call-a', 'call-b', 'call-c']) {
      assert.notEqual(
        attachOnPreToolUse(db, { conversationId: CONVERSATION, toolCallId, now: NOW + 1 }),
        null,
      );
    }
    assert.equal(
      confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-a', now: NOW + 40 }).status,
      'emitted',
    );

    // Rule 4: the deny and the Stop belong to attempts of the batch that delivered the pack.
    markFailure(db, {
      conversationId: CONVERSATION,
      toolCallId: 'call-b',
      kind: 'PermissionDenied',
      now: NOW + 41,
    });
    closeOnStop(db, { conversationId: CONVERSATION, sawAnyToolHook: true, now: NOW + 50 });

    assert.equal(injectionRow(db, id).delivery_count, 1);
    assert.deepEqual(
      whyReport(db, 's_now')[0].attempts.map((attempt) => [
        attempt.tool_call_id,
        attempt.execution,
        attempt.delivery,
      ]),
      [
        ['call-a', 'ran', 'delivered'],
        ['call-b', 'denied', 'dropped'],
        ['call-c', 'pending', 'dropped'],
      ],
    );
  });
});

test('why marks a Grok pack deferred even when no tool call ever ran', async () => {
  await withGrok(async (db) => {
    await pendingPack(db);
    closeOnStop(db, { conversationId: CONVERSATION, sawAnyToolHook: false, now: NOW + 5 });

    // FR-045: the Grok lane is the deferred one, and a pack that reached no call is still its pack.
    assert.equal(whyReport(db, 's_now')[0].deferred, true);
  });
});

test('the degraded reason the pack was built with survives the close at Stop', async () => {
  await withGrok(async (db) => {
    const id = await pendingPack(db, { model: 'grok-not-in-the-catalog' });
    assert.equal(injectionRow(db, id).degraded_reason, 'window_unknown');

    closeOnStop(db, { conversationId: CONVERSATION, sawAnyToolHook: false, now: NOW + 5 });

    const row = injectionRow(db, id);
    assert.equal(row.state, 'omitted');
    assert.equal(row.degraded_reason, 'window_unknown', 'the built reason is not overwritten');
    const items = whyReport(db, 's_now')[0].items;
    assert.ok(items.every((item) => item.decision === 'omitted' && item.reason === 'not_delivered'));
  });
});

test('a record whose stored pack was lost delivers only the items that were rendered', async () => {
  await withGrok(
    async (db) => {
      // The crash window: the ledger row and its planned items are written by the pack builder,
      // the rendered text by storePending. A hook killed in between leaves this record behind.
      const orphan = await buildPromptPack(db, promptInput({ prompt: 'retrieval' }));
      assert.notEqual(orphan, null);
      assert.equal(injectionRow(db, orphan!.injectionId).state, 'pending');

      const merged = await pendingPack(db, { prompt: 'lease', now: NOW + 100 });
      assert.equal(merged, orphan!.injectionId, 'the next pack merges into the record it finds');

      const text = attachOnPreToolUse(db, {
        conversationId: CONVERSATION,
        toolCallId: 'call-1',
        now: NOW + 200,
      });
      assert.notEqual(text, null);
      assert.ok(text!.includes('Lease note two'));
      assert.ok(!text!.includes('Retrieval note one'), 'the lost pack was never rendered');
      confirmOnPostToolUse(db, { conversationId: CONVERSATION, toolCallId: 'call-1', now: NOW + 201 });

      // FR-026: only a delivered memory is counted for the conversation, so m_1 stays injectable.
      const items = whyReport(db, 's_now').flatMap((injection) => injection.items);
      assert.deepEqual(
        items.map((item) => `${item.memoryId}:${item.decision}:${item.reason ?? '-'}`).sort(),
        ['m_1:omitted:not_delivered', 'm_2:included:-'],
      );
      const next = await buildPromptPack(db, promptInput({ prompt: 'retrieval', now: NOW + 300 }));
      assert.ok(next!.items.some((item) => item.memoryId === 'm_1' && item.decision === 'planned'));
    },
    [
      { id: 'm_1', title: 'Retrieval note one', body: 'The ranking is lexical.' },
      { id: 'm_2', title: 'Lease note two', body: 'The lease is fenced by its owner token.' },
    ],
  );
});
