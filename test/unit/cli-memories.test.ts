import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import {
  runDelete,
  runGet,
  runPin,
  runSearch,
  runTimeline,
  runUnpin,
} from '../../src/memories-cli.js';
import type { MemoryCliRuntime } from '../../src/memories-cli.js';
import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { openDatabase } from '../../src/db/open.js';
import { applyObservations } from '../../src/observer/classify.js';
import type { ObserverOutput } from '../../src/observer/contract.js';
import { oboetePaths } from '../../src/paths.js';
import type { DetectorResult } from '../../src/privacy/detect.js';
import { resolveRepoIdentity, type RepoIdentity } from '../../src/repo-identity.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import type { RawEventRow } from '../../src/worker/batches.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_800_000_000_000;

type MemorySeed = {
  repoId: string;
  title: string;
  body: string;
  type?: string;
  sensitivity?: string;
  deletedAt?: number;
};

type Command = (
  argv: string[],
  runtime?: Partial<MemoryCliRuntime>,
) => Promise<number>;

function insertRepo(db: DatabaseSync, identity: RepoIdentity): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, ?, ?, ?, 1, 1)`,
  ).run(identity.id, identity.identityKind, identity.normalizedIdentity, identity.root);
}

function insertMemory(db: DatabaseSync, seed: MemorySeed): string {
  const material = materialHash(seed.title, seed.body);
  const content = contentHash(seed.repoId, material);
  const id = memoryIdFor(content);
  db.prepare(
    `INSERT INTO memories
       (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash,
        sensitivity, review_state, valid_from, deleted_at, created_at)
     VALUES (?, ?, ?, ?, ?, '[]', ?, ?, ?, ?, 'unreviewed', 1, ?, 1)`,
  ).run(
    id,
    seed.repoId,
    seed.type ?? 'discovery',
    seed.title,
    seed.body,
    cjkBigrams(`${seed.title} ${seed.body}`),
    material,
    content,
    seed.sensitivity ?? 'eligible',
    seed.deletedAt ?? null,
  );
  return id;
}

async function withFixture(
  fn: (fixture: {
    home: string;
    repo: string;
    otherRepo: string;
    identity: RepoIdentity;
    otherIdentity: RepoIdentity;
  }) => Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const repo = join(home, 'repos', 'current');
    const otherRepo = join(home, 'repos', 'other');
    mkdirSync(repo, { recursive: true });
    mkdirSync(otherRepo, { recursive: true });
    await fn({
      home,
      repo,
      otherRepo,
      identity: resolveRepoIdentity(repo),
      otherIdentity: resolveRepoIdentity(otherRepo),
    });
  });
}

async function run(
  command: Command,
  argv: string[],
  cwd: string,
  now = NOW,
): Promise<{ status: number; stdout: string; stderr: string }> {
  let stdout = '';
  let stderr = '';
  const status = await command(argv, {
    cwd,
    now: () => now,
    writeOut: (text) => {
      stdout += text;
    },
    writeError: (text) => {
      stderr += text;
    },
  });
  return { status, stdout, stderr };
}

test('search reports normalized lexical relevance and explains an empty result', async () => {
  await withFixture(async ({ home, repo, identity, otherIdentity }) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    insertRepo(opened.db, identity);
    insertRepo(opened.db, otherIdentity);
    const hit = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'SQLite busy timeout',
      body: 'Set the SQLite busy timeout to 150 milliseconds.',
      type: 'decision',
    });
    const bodyOnly = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'Retry configuration',
      body: 'SQLite busy timeout controls waiting on a locked database.',
    });
    insertMemory(opened.db, {
      repoId: otherIdentity.id,
      title: 'SQLite busy timeout',
      body: 'This higher-scoring result belongs to another repository.',
    });
    opened.db.close();

    const found = await run(runSearch, ['sqlite busy timeout', '--limit', '1', '--json'], repo);
    assert.equal(found.status, 0);
    assert.equal(found.stderr, '');
    const parsed = JSON.parse(found.stdout) as {
      memories: { id: string; type: string; title: string; body: string; score: number; reasons: string[] }[];
    };
    assert.equal(parsed.memories.length, 1);
    assert.equal(parsed.memories[0].id, hit);
    assert.equal(parsed.memories[0].type, 'decision');
    assert.match(parsed.memories[0].body, /150 milliseconds/);
    assert.equal(parsed.memories[0].score, 1, 'the strongest lexical match has normalized relevance 1');
    assert.ok(parsed.memories[0].reasons.length > 0);

    const compared = await run(runSearch, ['sqlite busy timeout', '--limit', '2', '--json'], repo);
    assert.equal(compared.status, 0);
    const matches = (JSON.parse(compared.stdout) as typeof parsed).memories;
    const titleMatch = matches.find((memory) => memory.id === hit);
    const bodyMatch = matches.find((memory) => memory.id === bodyOnly);
    assert.ok(titleMatch && bodyMatch);
    assert.ok(titleMatch.score > bodyMatch.score, 'a full-title match outranks a body-only match in relevance');

    const empty = await run(runSearch, ['meteor zebra quartz'], repo);
    assert.equal(empty.status, 0);
    assert.match(empty.stdout, /No memories matched this query in the current repository\./);
    assert.match(empty.stdout, /M1 search is lexical \(word match\)\./);
    assert.match(empty.stdout, /Semantic search arrives in M2\./);
  });
});

test('get includes provenance but exits 1 outside the repository and for a tombstone', async () => {
  await withFixture(async ({ home, repo, identity, otherIdentity }) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    insertRepo(opened.db, identity);
    insertRepo(opened.db, otherIdentity);
    const active = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'Current memory',
      body: 'Visible in this repository.',
    });
    opened.db.prepare(
      `INSERT INTO memory_sources
         (memory_id, raw_event_id, citation_kind, citation_value, source_agent)
       VALUES (?, NULL, 'file_read', 'src/example.ts', 'codex')`,
    ).run(active);
    const outside = insertMemory(opened.db, {
      repoId: otherIdentity.id,
      title: 'Other memory',
      body: 'Must not cross the repository boundary.',
    });
    const deleted = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'Deleted memory',
      body: 'Must never surface again.',
      deletedAt: NOW - 1,
    });
    opened.db.close();

    const visible = await run(runGet, [active, '--json'], repo);
    assert.equal(visible.status, 0);
    const record = JSON.parse(visible.stdout) as Record<string, unknown> & {
      id: string;
      sources: { citation_value: string }[];
    };
    assert.equal(record.id, active);
    assert.deepEqual(record.sources.map((source) => source.citation_value), ['src/example.ts']);
    assert.deepEqual(Object.keys(record).sort(), [
      'body',
      'citations_head',
      'citations_ok',
      'cjk_bigrams',
      'concepts',
      'content_hash',
      'created_at',
      'degraded_reason',
      'deleted_at',
      'id',
      'last_injected_at',
      'material_hash',
      'pin_order',
      'pinned_at',
      'repo_id',
      'review_state',
      'sensitivity',
      'source_batch_id',
      'source_session_id',
      'sources',
      'superseded_by',
      'title',
      'type',
      'valid_from',
      'valid_to',
    ]);

    for (const id of [outside, deleted]) {
      const hidden = await run(runGet, [id, '--json'], repo);
      assert.equal(hidden.status, 1);
      assert.doesNotMatch(hidden.stdout + hidden.stderr, /Other memory|Deleted memory/);
    }
  });
});

test('timeline filters sessions to the current repository and optional session id', async () => {
  await withFixture(async ({ home, repo, identity, otherIdentity }) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    insertRepo(opened.db, identity);
    insertRepo(opened.db, otherIdentity);
    for (const session of [
      { id: 'session-current', repoId: identity.id },
      { id: 'session-other', repoId: otherIdentity.id },
    ]) {
      opened.db.prepare(
        `INSERT INTO sessions
           (id, repo_id, agent, native_session_id, conversation_id, started_at, status, turn_count)
         VALUES (?, ?, 'codex', ?, ?, 1, 'active', 1)`,
      ).run(session.id, session.repoId, `native-${session.id}`, session.id);
    }
    opened.db.prepare(
      `INSERT INTO turns (id, session_id, ordinal, started_at)
       VALUES ('turn-current', 'session-current', 1, 1)`,
    ).run();
    const memoryId = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'Timeline memory',
      body: 'This memory belongs to the recorded turn.',
      type: 'decision',
    });
    opened.db.prepare('UPDATE memories SET source_session_id = ? WHERE id = ?').run(
      'session-current',
      memoryId,
    );
    opened.db.prepare(
      `INSERT INTO raw_events
         (id, repo_id, session_id, turn_id, agent, kind, content, sensitivity,
          classification_state, captured_at, expires_at)
       VALUES ('event-current', ?, 'session-current', 'turn-current', 'codex', 'prompt',
               'timeline prompt', 'eligible', 'done', 1, ?)`,
    ).run(identity.id, NOW + 1_000);
    opened.db.prepare(
      `INSERT INTO memory_sources
         (memory_id, raw_event_id, citation_kind, citation_value, source_agent)
       VALUES (?, 'event-current', 'file_read', 'src/timeline.ts', 'codex')`,
    ).run(memoryId);
    opened.db.close();

    const result = await run(runTimeline, ['--session', 'session-current', '--json'], repo);
    assert.equal(result.status, 0);
    const parsed = JSON.parse(result.stdout) as {
      sessions: {
        id: string;
        turns: { ordinal: number; memory_ids: string[] }[];
        memories: {
          id: string;
          type: string;
          body: string;
          sensitivity: string;
          sources: { citation_value: string }[];
        }[];
      }[];
    };
    assert.deepEqual(parsed.sessions.map((session) => session.id), ['session-current']);
    assert.deepEqual(parsed.sessions[0].turns.map((turn) => turn.ordinal), [1]);
    assert.deepEqual(parsed.sessions[0].turns[0].memory_ids, [memoryId]);
    assert.equal(parsed.sessions[0].memories[0].type, 'decision');
    assert.match(parsed.sessions[0].memories[0].body, /recorded turn/);
    assert.equal(parsed.sessions[0].memories[0].sensitivity, 'eligible');
    assert.deepEqual(
      parsed.sessions[0].memories[0].sources.map((source) => source.citation_value),
      ['src/timeline.ts'],
    );

    const purged = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    purged.db.prepare("DELETE FROM raw_events WHERE id = 'event-current'").run();
    purged.db.close();
    const afterRetention = await run(
      runTimeline,
      ['--session', 'session-current', '--json'],
      repo,
    );
    const retained = JSON.parse(afterRetention.stdout) as {
      sessions: {
        turns: { memory_ids: string[] }[];
        memories: { id: string; sources: { raw_event_id: string }[] }[];
      }[];
    };
    assert.deepEqual(retained.sessions[0].memories.map((memory) => memory.id), [memoryId]);
    assert.deepEqual(retained.sessions[0].memories[0].sources.map((source) => source.raw_event_id), [
      'event-current',
    ]);
    assert.deepEqual(retained.sessions[0].turns[0].memory_ids, []);

    const outside = await run(runTimeline, ['--session', 'session-other', '--json'], repo);
    assert.equal(outside.status, 0);
    assert.deepEqual(JSON.parse(outside.stdout), { sessions: [] });
  });
});

test('pin and unpin update pinned_at and pin_order inside the repository', async () => {
  await withFixture(async ({ home, repo, identity }) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    insertRepo(opened.db, identity);
    const id = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'Pinned memory',
      body: 'Keep this at the front.',
    });
    opened.db.close();

    const pinned = await run(runPin, [id, '--order', '4', '--json'], repo);
    assert.equal(pinned.status, 0);
    let checked = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    let row = checked.db.prepare('SELECT pinned_at, pin_order FROM memories WHERE id = ?').get(id);
    assert.equal(row?.pinned_at, NOW);
    assert.equal(row?.pin_order, 4);
    checked.db.close();

    const unpinned = await run(runUnpin, [id, '--json'], repo);
    assert.equal(unpinned.status, 0);
    checked = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    row = checked.db.prepare('SELECT pinned_at, pin_order FROM memories WHERE id = ?').get(id);
    assert.equal(row?.pinned_at, null);
    assert.equal(row?.pin_order, null);
    checked.db.prepare('UPDATE memories SET valid_to = ? WHERE id = ?').run(NOW - 1, id);
    checked.db.close();

    const superseded = await run(runPin, [id, '--json'], repo);
    assert.equal(superseded.status, 1);
  });
});


test('pin and delete answer for a hidden memory exactly as they answer for a missing one', async () => {
  await withFixture(async ({ home, repo, identity, otherIdentity }) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    insertRepo(opened.db, identity);
    insertRepo(opened.db, otherIdentity);
    const secret = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'Secret memory',
      body: 'The developer may not read this one.',
      sensitivity: 'secret',
    });
    const outside = insertMemory(opened.db, {
      repoId: otherIdentity.id,
      title: 'Other memory',
      body: 'It belongs to another repository.',
    });
    opened.db.close();

    // FR-044 and contracts/cli.md: an id the reader cannot see answers like an id that is not
    // there, so the exit code never tells the developer that a hidden row exists.
    for (const id of [secret, outside, 'm_no_such_memory']) {
      for (const command of [runPin, runUnpin, runDelete]) {
        const json = await run(command, [id, '--json'], repo);
        assert.equal(json.status, 1);
        assert.equal(json.stdout, `${JSON.stringify({ error: 'memory_not_found', id })}\n`);
        assert.equal(json.stderr, '');

        const plain = await run(command, [id], repo);
        assert.equal(plain.status, 1);
        assert.equal(plain.stdout, '');
        assert.equal(plain.stderr, `Memory ${id} was not found in the current repository.\n`);
      }
    }

    const checked = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    for (const id of [secret, outside]) {
      const row = checked.db
        .prepare('SELECT pinned_at, pin_order, deleted_at FROM memories WHERE id = ?')
        .get(id);
      assert.equal(row?.pinned_at, null);
      assert.equal(row?.pin_order, null);
      assert.equal(row?.deleted_at, null);
    }
    checked.db.close();
  });
});

test('a destination_rules row that allows secret still hides the row from search, get and pin', async () => {
  await withFixture(async ({ home, repo, identity }) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    insertRepo(opened.db, identity);
    const secret = insertMemory(opened.db, {
      repoId: identity.id,
      title: 'SQLite busy timeout',
      body: 'The developer may not read this one.',
      sensitivity: 'secret',
    });
    // FR-020: a restored or tampered database that flips the rule is the threat; the reader's scope
    // excludes secret whatever the table says (src/db/queries.ts memoryScope).
    opened.db.prepare('UPDATE destination_rules SET allowed = 1 WHERE sensitivity = ?').run('secret');
    opened.db.close();

    const found = await run(runSearch, ['sqlite busy timeout', '--json'], repo);
    assert.equal(found.status, 0);
    assert.deepEqual((JSON.parse(found.stdout) as { memories: unknown[] }).memories, []);
    assert.doesNotMatch(found.stdout, /may not read this one/);

    for (const command of [runGet, runPin, runDelete]) {
      const hidden = await run(command, [secret, '--json'], repo);
      assert.equal(hidden.status, 1);
      assert.equal(hidden.stdout, `${JSON.stringify({ error: 'memory_not_found', id: secret })}\n`);
    }

    const checked = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    const row = checked.db.prepare('SELECT pinned_at, deleted_at FROM memories WHERE id = ?').get(secret);
    checked.db.close();
    assert.deepEqual({ ...row }, { pinned_at: null, deleted_at: null });
  });
});

test('delete keeps a tombstone and observer writes cannot recreate it under another type', async () => {
  await withFixture(async ({ home, repo, identity }) => {
    const title = 'Stable memory identity';
    const body = 'The title and body define the memory regardless of type.';
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    insertRepo(opened.db, identity);
    const id = insertMemory(opened.db, { repoId: identity.id, title, body, type: 'discovery' });
    opened.db.prepare(
      `INSERT INTO sessions
         (id, repo_id, agent, native_session_id, conversation_id, started_at, status, turn_count)
       VALUES ('session-1', ?, 'codex', 'native-1', 'session-1', 1, 'active', 1)`,
    ).run(identity.id);
    opened.db.prepare(
      `INSERT INTO turns (id, session_id, ordinal, started_at)
       VALUES ('turn-1', 'session-1', 1, 1)`,
    ).run();
    opened.db.prepare(
      `INSERT INTO raw_events
         (id, repo_id, session_id, turn_id, agent, kind, content, sensitivity,
          classification_state, captured_at, expires_at)
       VALUES ('event-1', ?, 'session-1', 'turn-1', 'codex', 'prompt', 'remember this',
               'eligible', 'done', 1, ?)`,
    ).run(identity.id, NOW + 1_000);
    opened.db.prepare(
      `INSERT INTO observation_batches
         (id, repo_id, session_id, through_event_id, destination, trigger, state,
          owner_token, provider_attempts, claimed_at)
       VALUES ('batch-1', ?, 'session-1', 'event-1', 'remote_observer', 'session_end',
               'running', 'worker', 1, 1)`,
    ).run(identity.id);
    const before = opened.db
      .prepare('SELECT material_hash, content_hash FROM memories WHERE id = ?')
      .get(id);
    opened.db.close();

    const deleted = await run(runDelete, [id, '--json'], repo);
    assert.equal(deleted.status, 0);

    const checked = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    assert.deepEqual(
      checked.db.prepare('SELECT material_hash, content_hash FROM memories WHERE id = ?').get(id),
      before,
    );
    assert.equal(
      checked.db.prepare('SELECT deleted_at FROM memories WHERE id = ?').get(id)?.deleted_at,
      NOW,
    );

    const token = claimLease(checked.db, { pid: 1, now: NOW + 1 });
    if (token === null) assert.fail('expected the observer lease');
    const output: ObserverOutput = {
      observations: [
        {
          type: 'feature',
          title,
          body,
          concepts: ['what-changed'],
          citations: { files_read: [], files_modified: [], commits: [] },
          source_event_ids: ['event-1'],
          classification: { decision: 'add', target: null, reason: 'same content' },
        },
      ],
    };
    const result = await applyObservations(checked.db, token, {
      batchId: 'batch-1',
      repoId: identity.id,
      sessionId: 'session-1',
      output,
      fallbackReason: null,
      rows: checked.db.prepare('SELECT * FROM raw_events').all() as unknown as RawEventRow[],
      nearby: [],
      detect: async (text): Promise<DetectorResult> => ({
        ok: true,
        text,
        texts: [],
        redactions: [],
        privateRemoved: 0,
        sensitivity: 'local_only',
        pathRule: null,
      }),
      now: NOW + 1,
    });

    assert.equal(result.suppressed.length, 1);
    assert.equal(Number(checked.db.prepare('SELECT COUNT(*) AS count FROM memories').get()?.count), 1);
    const tombstone = checked.db.prepare('SELECT type, deleted_at FROM memories WHERE id = ?').get(id);
    assert.equal(tombstone?.type, 'discovery');
    assert.equal(tombstone?.deleted_at, NOW);
    checked.db.close();
  });
});
