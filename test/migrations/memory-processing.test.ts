import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { MIGRATIONS, SchemaAheadError, openDatabase } from '../../src/db/open.js';
import { assertLease, releaseLease } from '../../src/worker/lease.js';
import { memoryScope, listMemories } from '../../src/db/queries.js';
import { withTempHome } from '../helpers/home.js';

function priorDatabase(path: string, version: 3 | 4 | 5 | 6 = 3): DatabaseSync {
  const db = new DatabaseSync(path);
  for (const migration of MIGRATIONS.filter((item) => item.version <= version)) {
    db.exec(migration.sql);
    db.prepare('INSERT INTO schema_migrations VALUES (?, ?, ?, ?)').run(
      migration.version, migration.name,
      createHash('sha256').update(migration.sql).digest('hex'), 100,
    );
  }
  db.exec(`PRAGMA user_version = ${version}`);
  return db;
}

test('version-6 upgrade preserves local data and creates one stable public replica namespace', async () => {
  await withTempHome(async (home) => {
    const path = join(home, 'memory.db');
    const old = priorDatabase(path, 6);
    old.exec("INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('r', 'common_dir', '/fixture')");
    const migrations = old.prepare('SELECT * FROM schema_migrations ORDER BY version').all();
    const repos = old.prepare('SELECT * FROM repos').all();
    old.close();
    const opened = openDatabase({ path, timeoutMs: 1000 });
    const origin = opened.db.prepare('SELECT origin_id FROM replica_identity WHERE id = 1').get()?.origin_id;
    try {
      assert.equal(opened.schemaVersion, 7);
      assert.deepEqual(opened.db.prepare('SELECT * FROM schema_migrations WHERE version <= 6 ORDER BY version').all(), migrations);
      assert.deepEqual(opened.db.prepare('SELECT * FROM repos').all(), repos);
      assert.match(String(origin), /^[0-9a-f]{32}$/u);
      assert.equal(opened.db.prepare('SELECT COUNT(*) AS n FROM migration_records').get()?.n, 0);
      assert.deepEqual(opened.db.prepare('PRAGMA foreign_key_check').all(), []);
      assert.equal(opened.db.prepare('PRAGMA integrity_check').get()?.integrity_check, 'ok');
    } finally { opened.db.close(); }
    const reopened = openDatabase({ path, timeoutMs: 1000 });
    try { assert.equal(reopened.db.prepare('SELECT origin_id FROM replica_identity WHERE id = 1').get()?.origin_id, origin); }
    finally { reopened.db.close(); }
  });
});

test('version-3 upgrade preserves evidence and leaves historical terminal sources unprocessed', async () => {
  await withTempHome(async (home) => {
    const path = join(home, 'memory.db');
    const old = priorDatabase(path);
    old.exec(`
      PRAGMA user_version = 3;
      INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('r', 'common_dir', '/fixture');
      INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
        VALUES ('s', 'r', 'claude', 'native', 'conversation', 'ended');
      INSERT INTO observation_batches (id, session_id, through_event_id, destination, state)
        VALUES ('old', 's', 'old-source', 'fallback', 'fallback'),
               ('queued', 's', 'queued-source', 'remote_observer', 'pending');
      INSERT INTO raw_events (id, repo_id, session_id, kind, content, batch_id, expires_at)
        VALUES ('old-source', 'r', 's', 'prompt', 'historical evidence', 'old', 1000),
               ('queued-source', 'r', 's', 'prompt', 'accepted queued evidence', 'queued', 1000);
    `);
    const hashes = old.prepare('SELECT * FROM schema_migrations ORDER BY version').all();
    old.close();

    const opened = openDatabase({ path, timeoutMs: 1000 });
    try {
      assert.equal(opened.schemaVersion, 7);
      assert.deepEqual(opened.db.prepare('SELECT * FROM schema_migrations WHERE version <= 3 ORDER BY version').all(), hashes);
      assert.deepEqual(opened.db.prepare(
        'SELECT id, content, processing_state, processed_at, expires_at FROM raw_events ORDER BY id',
      ).all().map((row) => ({ ...row })), [
        { id: 'old-source', content: 'historical evidence', processing_state: 'legacy_unknown', processed_at: null, expires_at: 1000 },
        { id: 'queued-source', content: 'accepted queued evidence', processing_state: 'waiting', processed_at: null, expires_at: 1000 },
      ]);
      assert.equal(opened.db.prepare('PRAGMA integrity_check').get()?.integrity_check, 'ok');
      assert.deepEqual(opened.db.prepare('PRAGMA foreign_key_check').all(), []);
    } finally {
      opened.db.close();
    }
  });
});

test('migration waits for a live old worker and can continue after it releases its lease', async () => {
  await withTempHome(async (home) => {
    const path = join(home, 'memory.db');
    const old = priorDatabase(path);
    const handles = [old];
    const upgrade = () => {
      const opened = openDatabase({ path, timeoutMs: 1000 });
      handles.push(opened.db);
      return opened;
    };
    try {
      old.prepare("UPDATE worker_lease SET owner_token = 'old-worker', heartbeat_at = ? WHERE id = 1").run(Date.now());
      assert.throws(upgrade, /worker.*migration/i);
      assert.equal(old.prepare('PRAGMA user_version').get()?.user_version, 3);
      assert.equal(releaseLease(old, 'old-worker', () => true), 'released');
      assert.equal(upgrade().schemaVersion, 7);
    } finally {
      for (const db of handles) db.close();
    }
  });
});

test('migration fences a stale old worker before publishing the new schema', async () => {
  await withTempHome(async (home) => {
    const path = join(home, 'memory.db');
    const old = priorDatabase(path);
    old.prepare("UPDATE worker_lease SET owner_token = 'old-worker', heartbeat_at = ? WHERE id = 1")
      .run(Date.now() - 60_000);
    const opened = openDatabase({ path, timeoutMs: 1000 });
    try {
      assert.equal(opened.schemaVersion, 7);
      assert.equal(assertLease(old, 'old-worker', Date.now()), false,
        'an old worker cannot run its pre-migration purge or apply after the schema changes');
    } finally {
      opened.db.close();
      old.close();
    }
  });
});

test('an unsupported future schema refuses a hook opening as well as a worker opening', async () => {
  await withTempHome(async (home) => {
    const path = join(home, 'memory.db');
    const { db } = openDatabase({ path, timeoutMs: 1000 });
    db.exec('PRAGMA user_version = 999');
    db.close();
    assert.throws(() => openDatabase({ path, timeoutMs: 1000 }), SchemaAheadError);
    assert.throws(() => openDatabase({ path, timeoutMs: 1000, hook: true }), SchemaAheadError);
  });
});

test('version-4 upgrade holds unknown work and fences the old worker without rebuilding sessions', async () => {
  await withTempHome(async (home) => {
    const path = join(home, 'memory.db');
    const old = priorDatabase(path, 4);
    old.exec(`
      INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('r', 'common_dir', '/fixture');
      INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
        VALUES ('s', 'r', 'claude', 'original-native', 's', 'ended');
      INSERT INTO observation_batches (id, session_id, through_event_id, destination, state)
        VALUES ('queued', 's', 'source', 'remote_observer', 'pending');
      INSERT INTO raw_events (id, repo_id, session_id, kind, content, classification_state, batch_id, captured_at)
        VALUES ('source', 'r', 's', 'prompt', 'accepted source', 'done', 'queued', 100);
      INSERT INTO observation_batch_sources (batch_id, raw_event_id, outcome, recorded_at)
        VALUES ('queued', 'source', 'assigned', 100);
    `);
    const checksums = old.prepare('SELECT * FROM schema_migrations ORDER BY version').all();
    old.prepare("UPDATE worker_lease SET owner_token = 'old-v4', heartbeat_at = ? WHERE id = 1").run(Date.now());
    assert.throws(() => openDatabase({ path, timeoutMs: 1000 }), /worker.*migration/i);
    assert.equal(old.prepare('PRAGMA user_version').get()?.user_version, 4);
    releaseLease(old, 'old-v4', () => true);
    const opened = openDatabase({ path, timeoutMs: 1000 });
    try {
      assert.equal(opened.schemaVersion, 7);
      assert.deepEqual(opened.db.prepare('SELECT * FROM schema_migrations WHERE version <= 4 ORDER BY version').all(), checksums);
      assert.deepEqual({ ...opened.db.prepare('SELECT id, native_session_id, last_captured_at FROM sessions').get() },
        { id: 's', native_session_id: 'original-native', last_captured_at: 100 });
      assert.deepEqual({ ...opened.db.prepare('SELECT id, session_id, content, batch_id, work_binding_id, processing_state, retry_after FROM raw_events').get() },
        { id: 'source', session_id: 's', content: 'accepted source', batch_id: null, work_binding_id: null,
          processing_state: 'waiting', retry_after: null });
      assert.equal(opened.db.prepare('SELECT state FROM observation_batches').get()?.state, 'fallback');
      assert.equal(opened.db.prepare('SELECT reason FROM observation_batch_sources').get()?.reason, 'work_selection_required');
      assert.equal(opened.db.prepare('SELECT COUNT(*) AS n FROM work_items').get()?.n, 0);
      assert.deepEqual(opened.db.prepare('PRAGMA foreign_key_check').all(), []);
      assert.equal(opened.db.prepare('PRAGMA integrity_check').get()?.integrity_check, 'ok');
    } finally { opened.db.close(); old.close(); }
  });
});

test('version-5 sharing upgrade preserves ordinary knowledge and keeps fallback in its proven work', async () => {
  await withTempHome(async (home) => {
    const path = join(home, 'memory.db');
    const old = priorDatabase(path, 5);
    old.exec(`INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('r', 'common_dir', '/fixture');
      INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status) VALUES ('s', 'r', 'claude', 's', 's', 'ended');
      INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at) VALUES ('c', 'r', 'c', '/fixture', 1, 1);
      INSERT INTO work_items (id, repo_id, origin_context_id, created_at, updated_at) VALUES ('w', 'r', 'c', 1, 1);
      INSERT INTO work_bindings (id, session_id, context_id, work_id, created_at, closed_at, reason)
        VALUES ('b', 's', 'c', 'w', 1, 2, 'only_active');
      INSERT INTO observation_batches (id, repo_id, session_id, work_binding_id, state, destination, through_event_id)
        VALUES ('batch', 'r', 's', 'b', 'fallback', 'fallback', 'last-source');
      INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity, source_batch_id, degraded_reason)
        VALUES ('knowledge', 'r', 'discovery', 'Current knowledge', 'Retain this fact.', 'fact', 'eligible', NULL, NULL),
        ('fallback', 'r', 'discovery', 'Temporary progress', 'Keep it with the origin work.', 'fallback', 'eligible', 'batch', 'rule_based'),
        ('unknown', 'r', 'discovery', 'Unbound progress', 'Retained but held.', 'unknown', 'eligible', NULL, 'rule_based');`);
    const before = old.prepare('SELECT * FROM memories ORDER BY id').all();
    const checksums = old.prepare('SELECT * FROM schema_migrations ORDER BY version').all();
    old.close();
    const { db } = openDatabase({ path, timeoutMs: 1000 });
    try {
      assert.deepEqual(db.prepare('SELECT * FROM memories ORDER BY id').all(), before);
      assert.deepEqual(db.prepare('SELECT * FROM schema_migrations WHERE version <= 5 ORDER BY version').all(), checksums);
      const ids = (workId: string | null) => listMemories(db, memoryScope(db, { repoId: 'r', workId, destination: 'injection' }),
        { limit: 10 }).map((row) => row.id).sort();
      assert.deepEqual(ids('w'), ['fallback', 'knowledge']);
      assert.deepEqual(ids(null), ['knowledge']);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM sharing_proposals').get()?.n, 0);
      assert.deepEqual(db.prepare('PRAGMA foreign_key_check').all(), []);
      assert.equal(db.prepare('PRAGMA integrity_check').get()?.integrity_check, 'ok');
    } finally { db.close(); }
  });
});
