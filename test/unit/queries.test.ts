import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import {
  getMemory,
  latestSessionState,
  latestSessionSummary,
  listMemories,
  markInjected,
  memoriesForSession,
  memoryScope,
  pinnedMemories,
  setPinned,
  timeline,
  tombstone,
} from '../../src/db/queries.js';
import { oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';

const REPO_A = 'aaaaaaaaaaaaaaa1';
const REPO_B = 'bbbbbbbbbbbbbbb2';

const SENSITIVITIES = ['eligible', 'local_only', 'private', 'secret'] as const;
const REVIEW_STATES = ['unreviewed', 'reviewed', 'imported'] as const;
const LIFECYCLES = ['active', 'deleted', 'superseded'] as const;

type MemorySeed = {
  id: string;
  repoId: string;
  type: string;
  sensitivity: (typeof SENSITIVITIES)[number];
  reviewState: (typeof REVIEW_STATES)[number];
  lifecycle: (typeof LIFECYCLES)[number];
  createdAt: number;
};

/** Every sensitivity x review_state x lifecycle in both repositories, plus the session summaries. */
const MEMORY_SEEDS: MemorySeed[] = buildMemorySeeds();

function buildMemorySeeds(): MemorySeed[] {
  const seeds: MemorySeed[] = [];
  let createdAt = 1_000;
  for (const repoId of [REPO_A, REPO_B]) {
    const shortRepo = repoId === REPO_A ? 'a' : 'b';
    for (const sensitivity of SENSITIVITIES) {
      for (const reviewState of REVIEW_STATES) {
        for (const lifecycle of LIFECYCLES) {
          createdAt += 10;
          seeds.push({
            id: `m_${shortRepo}_${sensitivity}_${reviewState}_${lifecycle}`,
            repoId,
            type: 'discovery',
            sensitivity,
            reviewState,
            lifecycle,
            createdAt,
          });
        }
      }
    }
  }
  for (const summary of [
    { id: 'm_a_summary_done', repoId: REPO_A, sensitivity: 'eligible' as const },
    { id: 'm_a_summary_newer', repoId: REPO_A, sensitivity: 'eligible' as const },
    { id: 'm_a_summary_secret', repoId: REPO_A, sensitivity: 'secret' as const },
    { id: 'm_b_summary_done', repoId: REPO_B, sensitivity: 'eligible' as const },
  ]) {
    createdAt += 10;
    seeds.push({
      ...summary,
      type: 'session_summary',
      reviewState: 'unreviewed',
      lifecycle: 'active',
      createdAt,
    });
  }
  return seeds;
}

/** The expectation from data-model.md "destination_rules", written out instead of queried. */
const ALLOWED_SENSITIVITIES: Record<string, string[]> = {
  injection: ['eligible', 'local_only', 'private'],
  local_observer: ['eligible', 'local_only', 'private'],
  remote_observer: ['eligible'],
};

function expectedInScope(destination: string, override?: string[]): MemorySeed[] {
  const allowed = override ?? ALLOWED_SENSITIVITIES[destination] ?? [];
  return MEMORY_SEEDS.filter(
    (seed) =>
      seed.repoId === REPO_A &&
      seed.lifecycle === 'active' &&
      seed.reviewState !== 'imported' &&
      allowed.includes(seed.sensitivity),
  );
}

function seedIdsIn(destination: string, override?: string[]): string[] {
  return expectedInScope(destination, override)
    .map((seed) => seed.id)
    .sort();
}

// pinned_at deliberately contradicts pin_order, so an implementation that sorts by the timestamp
// alone returns the reverse order (FR-024 trims pinned memories in pin order).
const PINS: { id: string; pinnedAt: number; pinOrder: number }[] = [
  { id: 'm_a_local_only_reviewed_active', pinnedAt: 300, pinOrder: 1 },
  { id: 'm_a_local_only_unreviewed_active', pinnedAt: 250, pinOrder: 2 },
  { id: 'm_a_private_unreviewed_active', pinnedAt: 200, pinOrder: 2 },
  { id: 'm_a_eligible_unreviewed_active', pinnedAt: 100, pinOrder: 3 },
  { id: 'm_a_secret_unreviewed_active', pinnedAt: 50, pinOrder: 0 },
  { id: 'm_a_eligible_imported_active', pinnedAt: 60, pinOrder: 0 },
];

function seed(db: DatabaseSync): void {
  const now = 1_000_000;
  for (const repoId of [REPO_A, REPO_B]) {
    db.prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
       VALUES (?, 'remote', ?, ?, ?, ?)`,
    ).run(repoId, `https://example.test/${repoId}`, `/tmp/${repoId}`, now, now);
  }

  const sessions = [
    { id: 's_a_done', repoId: REPO_A, startedAt: 1000, endedAt: 3000, status: 'ended', summaryState: 'done', summaryId: 'm_a_summary_done' },
    { id: 's_a_done_newer', repoId: REPO_A, startedAt: 1400, endedAt: 3400, status: 'ended', summaryState: 'done', summaryId: 'm_a_summary_newer' },
    { id: 's_a_done_secret', repoId: REPO_A, startedAt: 1500, endedAt: 3500, status: 'ended', summaryState: 'done', summaryId: 'm_a_summary_secret' },
    { id: 's_a_pending', repoId: REPO_A, startedAt: 2000, endedAt: 4000, status: 'ended', summaryState: 'pending', summaryId: null },
    { id: 's_a_active', repoId: REPO_A, startedAt: 5000, endedAt: null, status: 'active', summaryState: null, summaryId: null },
    { id: 's_b_done', repoId: REPO_B, startedAt: 8000, endedAt: 9000, status: 'ended', summaryState: 'done', summaryId: 'm_b_summary_done' },
  ];
  for (const session of sessions) {
    db.prepare(
      `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
         started_at, ended_at, status, turn_count, latest_summary_memory_id, context_epoch,
         summary_state)
       VALUES (?, ?, 'claude', ?, ?, 'test-model', ?, ?, ?, 2, ?, 0, ?)`,
    ).run(
      session.id,
      session.repoId,
      `native-${session.id}`,
      `conversation-${session.id}`,
      session.startedAt,
      session.endedAt,
      session.status,
      session.summaryId,
      session.summaryState,
    );
  }

  for (const ordinal of [1, 2]) {
    db.prepare(
      'INSERT INTO turns (id, session_id, ordinal, started_at, ended_at) VALUES (?, ?, ?, ?, ?)',
    ).run(`t_a_done_${ordinal}`, 's_a_done', ordinal, 1000 + ordinal, 1100 + ordinal);
  }

  db.prepare(
    `INSERT INTO raw_events (id, repo_id, session_id, turn_id, agent, kind, content, sensitivity,
       classification_state, captured_at, expires_at)
     VALUES (?, ?, ?, ?, 'claude', 'prompt', 'seed prompt', 'eligible', 'done', ?, ?)`,
  ).run('e_a_done_1', REPO_A, 's_a_done', 't_a_done_1', 1001, 1_000_000);

  for (const memory of MEMORY_SEEDS) {
    db.prepare(
      `INSERT INTO memories (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash,
         content_hash, sensitivity, review_state, valid_from, valid_to, superseded_by, deleted_at,
         created_at)
       VALUES (?, ?, ?, ?, ?, '[]', '', ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(
      memory.id,
      memory.repoId,
      memory.type,
      `Title of ${memory.id}`,
      `Body of ${memory.id}`,
      `material-${memory.id}`,
      `content-${memory.id}`,
      memory.sensitivity,
      memory.reviewState,
      memory.createdAt,
      memory.lifecycle === 'superseded' ? memory.createdAt + 1 : null,
      memory.lifecycle === 'superseded' ? 'm_a_eligible_unreviewed_active' : null,
      memory.lifecycle === 'deleted' ? memory.createdAt + 1 : null,
      memory.createdAt,
    );
  }

  for (const pin of PINS) {
    db.prepare('UPDATE memories SET pinned_at = ?, pin_order = ? WHERE id = ?').run(
      pin.pinnedAt,
      pin.pinOrder,
      pin.id,
    );
  }

  for (const memoryId of ['m_a_summary_done', 'm_a_eligible_unreviewed_active', 'm_a_secret_unreviewed_active']) {
    db.prepare(
      `INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value, source_agent)
       VALUES (?, 'e_a_done_1', 'file_read', 'src/db/queries.ts', 'claude')`,
    ).run(memoryId);
  }
}

async function withSeededDatabase(fn: (db: DatabaseSync) => void): Promise<void> {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2000 });
    try {
      seed(opened.db);
      fn(opened.db);
    } finally {
      opened.db.close();
    }
  });
}

function idsInScope(db: DatabaseSync, destination: 'injection' | 'local_observer' | 'remote_observer'): string[] {
  const scope = memoryScope(db, { repoId: REPO_A, destination });
  return db
    .prepare(`SELECT m.id AS id FROM memories m WHERE ${scope.where} ORDER BY m.id`)
    .all(...scope.params)
    .map((row) => String(row.id));
}

test('the injection scope keeps same-repository active rows that are not secret and not imported', async () => {
  await withSeededDatabase((db) => {
    const ids = idsInScope(db, 'injection');

    assert.deepEqual(ids, seedIdsIn('injection'));
    assert.ok(ids.includes('m_a_eligible_unreviewed_active'), 'FR-042: an unreviewed memory is injectable at once');
    assert.ok(ids.includes('m_a_private_reviewed_active'));
    for (const excluded of [
      'm_b_eligible_unreviewed_active',
      'm_a_secret_unreviewed_active',
      'm_a_eligible_imported_active',
      'm_a_eligible_unreviewed_deleted',
      'm_a_eligible_unreviewed_superseded',
    ]) {
      assert.ok(!ids.includes(excluded), `${excluded} must stay out of the injection scope`);
    }
  });
});

test('the remote observer scope keeps eligible rows only and the local observer scope excludes secret only', async () => {
  await withSeededDatabase((db) => {
    assert.deepEqual(idsInScope(db, 'remote_observer'), seedIdsIn('remote_observer'));
    assert.deepEqual(idsInScope(db, 'local_observer'), seedIdsIn('local_observer'));
  });
});

test('the allowed sensitivities come from destination_rules, not from a constant', async () => {
  await withSeededDatabase((db) => {
    db.prepare('DELETE FROM destination_rules WHERE destination = ? AND sensitivity = ?').run(
      'injection',
      'private',
    );

    assert.deepEqual(idsInScope(db, 'injection'), seedIdsIn('injection', ['eligible', 'local_only']));
    assert.deepEqual(idsInScope(db, 'local_observer'), seedIdsIn('local_observer'));
  });
});

test('a destination with no allowed sensitivity left matches nothing', async () => {
  await withSeededDatabase((db) => {
    db.prepare('DELETE FROM destination_rules WHERE destination = ?').run('injection');

    assert.deepEqual(idsInScope(db, 'injection'), []);
  });
});

test('a destination_rules row that allows secret never widens the scope', async () => {
  await withSeededDatabase((db) => {
    // FR-020: the table may narrow the readable set, never widen it past the hard rule, so a
    // restored or tampered database cannot open secret rows to a reader. src/privacy/egress.ts
    // isAllowed refuses secret the same way on the observer path.
    db.prepare('UPDATE destination_rules SET allowed = 1 WHERE sensitivity = ?').run('secret');
    const scope = memoryScope(db, { repoId: REPO_A, destination: 'injection' });
    const secret = 'm_a_secret_unreviewed_active';

    for (const destination of ['injection', 'local_observer', 'remote_observer'] as const) {
      assert.deepEqual(idsInScope(db, destination), seedIdsIn(destination));
    }
    assert.equal(getMemory(db, secret, scope), null);
    assert.equal(setPinned(db, { id: secret, scope, pinnedAt: 4242, pinOrder: 7 }), false);
    assert.equal(tombstone(db, { id: secret, scope, deletedAt: 5555 }), false);
    assert.deepEqual(
      { ...db.prepare('SELECT pinned_at, deleted_at FROM memories WHERE id = ?').get(secret) },
      { pinned_at: 50, deleted_at: null },
    );
  });
});

test('the sync destination is refused in M1', async () => {
  await withSeededDatabase((db) => {
    assert.throws(
      () => memoryScope(db, { repoId: REPO_A, destination: 'sync' }),
      /not available in M1/,
    );
  });
});

test('getMemory hides out-of-boundary and secret rows the same way it hides missing ones', async () => {
  await withSeededDatabase((db) => {
    const scope = memoryScope(db, { repoId: REPO_A, destination: 'injection' });

    const found = getMemory(db, 'm_a_eligible_unreviewed_active', scope);
    assert.equal(found?.id, 'm_a_eligible_unreviewed_active');
    assert.equal(found?.repo_id, REPO_A);
    assert.equal(found?.sensitivity, 'eligible');
    assert.equal(found?.title, 'Title of m_a_eligible_unreviewed_active');

    assert.equal(getMemory(db, 'm_b_eligible_unreviewed_active', scope), null);
    assert.equal(getMemory(db, 'm_a_secret_unreviewed_active', scope), null);
    assert.equal(getMemory(db, 'm_a_eligible_imported_active', scope), null);
    assert.equal(getMemory(db, 'no_such_memory', scope), null);
  });
});

test('setPinned and tombstone reach exactly the rows the reader can see', async () => {
  await withSeededDatabase((db) => {
    const scope = memoryScope(db, { repoId: REPO_A, destination: 'injection' });
    const visible = 'm_a_eligible_unreviewed_active';
    const hidden = [
      'm_a_secret_unreviewed_active',
      'm_a_eligible_imported_active',
      'm_a_eligible_unreviewed_superseded',
      'm_a_eligible_unreviewed_deleted',
      'm_b_eligible_unreviewed_active',
      'no_such_memory',
    ];

    assert.equal(setPinned(db, { id: visible, scope, pinnedAt: 4242, pinOrder: 7 }), true);
    for (const id of hidden) {
      assert.equal(
        setPinned(db, { id, scope, pinnedAt: 4242, pinOrder: 7 }),
        false,
        `${id} must be unpinnable, exactly as a missing id is`,
      );
      assert.equal(
        tombstone(db, { id, scope, deletedAt: 5555 }),
        false,
        `${id} must be undeletable, exactly as a missing id is`,
      );
    }

    // The writes reached the one row in scope and no other, so an exit code tells the developer
    // nothing about a row they may not read (contracts/cli.md "within the cwd repository boundary").
    assert.deepEqual(
      db.prepare('SELECT id AS id FROM memories WHERE pinned_at = 4242 ORDER BY id').all().map((row) => String(row.id)),
      [visible],
    );
    assert.deepEqual(db.prepare('SELECT id AS id FROM memories WHERE deleted_at = 5555').all(), []);

    assert.equal(tombstone(db, { id: visible, scope, deletedAt: 5555 }), true);
    assert.equal(getMemory(db, visible, scope), null);
    // The tombstone left the scope, so a second delete of the same id is a miss like any other.
    assert.equal(tombstone(db, { id: visible, scope, deletedAt: 5556 }), false);
    assert.equal(setPinned(db, { id: visible, scope, pinnedAt: 4243, pinOrder: 7 }), false);
  });
});

test('listMemories returns the newest first and honours limit and offset', async () => {
  await withSeededDatabase((db) => {
    const scope = memoryScope(db, { repoId: REPO_A, destination: 'injection' });
    const newestFirst = expectedInScope('injection')
      .slice()
      .sort((a, b) => b.createdAt - a.createdAt)
      .map((memory) => memory.id);

    assert.deepEqual(
      listMemories(db, scope, { limit: 3 }).map((row) => row.id),
      newestFirst.slice(0, 3),
    );
    assert.deepEqual(
      listMemories(db, scope, { limit: 2, offset: 3 }).map((row) => row.id),
      newestFirst.slice(3, 5),
    );
  });
});

test('pinnedMemories follows pin order and drops rows outside the scope', async () => {
  await withSeededDatabase((db) => {
    const scope = memoryScope(db, { repoId: REPO_A, destination: 'injection' });

    assert.deepEqual(
      pinnedMemories(db, scope).map((row) => row.id),
      [
        'm_a_local_only_reviewed_active',
        'm_a_private_unreviewed_active',
        'm_a_local_only_unreviewed_active',
        'm_a_eligible_unreviewed_active',
      ],
    );
  });
});

test('latestSessionSummary returns the newest summary that survives the injection scope', async () => {
  await withSeededDatabase((db) => {
    // Newest ended session first: s_a_done_secret (3500) is dropped by the scope, so s_a_done_newer
    // (3400) wins over the older s_a_done (3000).
    assert.equal(latestSessionSummary(db, REPO_A)?.id, 'm_a_summary_newer');
    assert.equal(latestSessionSummary(db, 'cccccccccccccccc'), null);
  });
});

test('latestSessionState reports the pending summary of the most recently ended session', async () => {
  await withSeededDatabase((db) => {
    assert.deepEqual(latestSessionState(db, REPO_A), {
      sessionId: 's_a_pending',
      summaryState: 'pending',
      endedAt: 4000,
    });
    assert.equal(latestSessionState(db, 'cccccccccccccccc'), null);
  });
});

test('timeline carries the sessions of the repository with their turns and in-scope memory ids', async () => {
  await withSeededDatabase((db) => {
    const sessions = timeline(db, REPO_A, { limit: 10 });

    assert.deepEqual(
      sessions.map((session) => session.id),
      ['s_a_active', 's_a_pending', 's_a_done_secret', 's_a_done_newer', 's_a_done'],
    );

    const done = sessions.find((session) => session.id === 's_a_done');
    assert.deepEqual(
      done?.turns.map((turn) => turn.ordinal),
      [1, 2],
    );
    assert.equal(done?.agent, 'claude');
    assert.equal(done?.summary_state, 'done');
    assert.deepEqual(done?.memory_ids, ['m_a_summary_done', 'm_a_eligible_unreviewed_active']);

    assert.deepEqual(
      timeline(db, REPO_A, { sessionId: 's_a_done', limit: 10 }).map((session) => session.id),
      ['s_a_done'],
    );
    assert.equal(timeline(db, REPO_A, { limit: 2 }).length, 2);
    assert.deepEqual(timeline(db, 'cccccccccccccccc', { limit: 10 }), []);
  });
});

test('memoriesForSession stays inside the scope', async () => {
  await withSeededDatabase((db) => {
    const scope = memoryScope(db, { repoId: REPO_A, destination: 'injection' });

    assert.deepEqual(
      memoriesForSession(db, 's_a_done', scope).map((row) => row.id),
      ['m_a_summary_done', 'm_a_eligible_unreviewed_active'],
    );
    assert.deepEqual(memoriesForSession(db, 's_a_active', scope), []);
  });
});

test('memoriesForSession joins both paths to the session and returns each memory once', async () => {
  await withSeededDatabase((db) => {
    const scope = memoryScope(db, { repoId: REPO_A, destination: 'injection' });
    // m_a_summary_done already cites a raw event of s_a_done, so both paths now name it.
    db.prepare("UPDATE memories SET source_session_id = 's_a_done' WHERE id IN (?, ?)").run(
      'm_a_summary_done',
      'm_a_private_reviewed_active', // this one has no sources: the session id is its only path
    );

    assert.deepEqual(
      memoriesForSession(db, 's_a_done', scope).map((row) => row.id),
      ['m_a_summary_done', 'm_a_private_reviewed_active', 'm_a_eligible_unreviewed_active'],
    );
  });
});

test('markInjected stamps only the given memories', async () => {
  await withSeededDatabase((db) => {
    markInjected(db, ['m_a_eligible_unreviewed_active', 'm_a_summary_done'], 777);
    markInjected(db, [], 999);

    const stamped = db
      .prepare('SELECT id AS id FROM memories WHERE last_injected_at = 777 ORDER BY id')
      .all()
      .map((row) => String(row.id));

    assert.deepEqual(stamped, ['m_a_eligible_unreviewed_active', 'm_a_summary_done']);
  });
});
