import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  DatabaseMissingError,
  isBusyError,
  MIGRATIONS,
  MigrationMismatchError,
  openDatabase,
  SchemaAheadError,
} from '../../src/db/open.js';

const previousVersionDb = fileURLToPath(
  new URL('../../../test/fixtures/previous-version.db', import.meta.url),
);

function sha256Hex(text: string): string {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

function closeQuietly(db: DatabaseSync): void {
  try {
    if (db.isOpen) {
      db.close();
    }
  } catch {
    // Directory cleanup still runs.
  }
}

test('empty database applies all migrations', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  const dbPath = path.join(dir, 'memory.db');
  t.after(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  const opened = openDatabase({ path: dbPath, timeoutMs: 1000 });
  t.after(() => closeQuietly(opened.db));

  assert.equal(opened.schemaVersion, 3);
  assert.equal(opened.schemaBehind, false);
  assert.equal(opened.db.prepare('PRAGMA user_version').get()?.user_version, 3);

  const rows = opened.db
    .prepare('SELECT version, name, sha256 FROM schema_migrations ORDER BY version')
    .all();
  assert.equal(rows.length, 3);
  for (const [i, migration] of MIGRATIONS.entries()) {
    assert.equal(rows[i]?.version, migration.version);
    assert.equal(rows[i]?.name, migration.name);
    assert.equal(rows[i]?.sha256, sha256Hex(migration.sql));
  }

  assert.equal(opened.db.prepare('PRAGMA integrity_check').get()?.integrity_check, 'ok');
  assert.equal(opened.db.prepare('PRAGMA journal_mode').get()?.journal_mode, 'wal');
  assert.equal(opened.db.prepare('PRAGMA foreign_keys').get()?.foreign_keys, 1);
});

test('reopen applies nothing', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  const dbPath = path.join(dir, 'memory.db');
  const dbs: DatabaseSync[] = [];
  t.after(() => {
    for (const db of dbs) closeQuietly(db);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  const first = openDatabase({ path: dbPath, timeoutMs: 1000 });
  dbs.push(first.db);
  const snapshot = first.db
    .prepare('SELECT version, applied_at FROM schema_migrations ORDER BY version')
    .all();
  first.db.close();

  const second = openDatabase({ path: dbPath, timeoutMs: 1000 });
  dbs.push(second.db);
  const again = second.db
    .prepare('SELECT version, applied_at FROM schema_migrations ORDER BY version')
    .all();
  assert.equal(second.schemaBehind, false);
  assert.deepEqual(again, snapshot);
});

test('previous version upgrades to 3 and keeps version-1 applied_at', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  const dbPath = path.join(dir, 'memory.db');
  t.after(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  fs.copyFileSync(previousVersionDb, dbPath);
  const peek = new DatabaseSync(dbPath, { timeout: 1000 });
  const originalAppliedAt = peek
    .prepare('SELECT applied_at FROM schema_migrations WHERE version = 1')
    .get()?.applied_at;
  peek.close();
  assert.equal(typeof originalAppliedAt, 'number');

  const opened = openDatabase({ path: dbPath, timeoutMs: 1000 });
  t.after(() => closeQuietly(opened.db));
  assert.equal(opened.schemaVersion, 3);

  const versions = opened.db
    .prepare('SELECT version FROM schema_migrations ORDER BY version')
    .all()
    .map((row) => row.version);
  assert.deepEqual(versions, [1, 2, 3]);
  assert.equal(
    opened.db.prepare('SELECT applied_at FROM schema_migrations WHERE version = 1').get()
      ?.applied_at,
    originalAppliedAt,
  );

  const tables = opened.db
    .prepare(
      `SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('memories', 'injections', 'provider_usage') ORDER BY name`,
    )
    .all()
    .map((row) => row.name);
  assert.deepEqual(tables, ['injections', 'memories', 'provider_usage']);

  opened.db
    .prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, created_at, last_seen_at)
       VALUES (?, 'common_dir', ?, 1, 1)`,
    )
    .run('r_fts', '/tmp/oboete-fts');
  opened.db
    .prepare(
      `INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity)
       VALUES (?, ?, 'discovery', ?, ?, ?, 'local_only')`,
    )
    .run('m_fts', 'r_fts', 'pterodactyl memory title', 'body about pterodactyl', 'hash_fts_1');
  const hits = opened.db
    .prepare(`SELECT title FROM memories_fts WHERE memories_fts MATCH 'pterodactyl'`)
    .all();
  assert.equal(hits.length, 1);
  assert.equal(hits[0]?.title, 'pterodactyl memory title');
});

test('memories declares an INTEGER PRIMARY KEY rid and a NOT NULL TEXT id', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  t.after(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  const opened = openDatabase({ path: path.join(dir, 'memory.db'), timeoutMs: 1000 });
  t.after(() => closeQuietly(opened.db));
  const db = opened.db;
  // SQLite rowid-table documentation (https://www.sqlite.org/rowidtable.html): a persistent rowid
  // is guaranteed only for an explicit INTEGER PRIMARY KEY.
  const columns = db.prepare('PRAGMA table_info(memories)').all();
  assert.deepEqual(
    columns.slice(0, 2).map((row) => ({
      name: row.name,
      type: row.type,
      notnull: row.notnull,
      pk: row.pk,
    })),
    [
      { name: 'rid', type: 'INTEGER', notnull: 0, pk: 1 },
      { name: 'id', type: 'TEXT', notnull: 1, pk: 0 },
    ],
  );
});

test('hook role does not migrate and does not create a missing file', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  const dbPath = path.join(dir, 'memory.db');
  const missingPath = path.join(dir, 'missing.db');
  t.after(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  fs.copyFileSync(previousVersionDb, dbPath);
  const opened = openDatabase({ path: dbPath, timeoutMs: 1000, hook: true });
  t.after(() => closeQuietly(opened.db));
  assert.equal(opened.schemaBehind, true);
  assert.equal(opened.schemaVersion, 1);
  assert.equal(opened.db.prepare('PRAGMA user_version').get()?.user_version, 1);
  assert.equal(opened.db.prepare('PRAGMA wal_autocheckpoint').get()?.wal_autocheckpoint, 0);
  const tableCount = opened.db
    .prepare(`SELECT COUNT(*) AS n FROM sqlite_master WHERE type = 'table' AND name = 'memories'`)
    .get()?.n;
  assert.equal(tableCount, 0);

  assert.throws(
    () => openDatabase({ path: missingPath, timeoutMs: 1000, hook: true }),
    DatabaseMissingError,
  );
  assert.equal(fs.existsSync(missingPath), false);
});

test('mismatch throws MigrationMismatchError', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  const dbPath = path.join(dir, 'memory.db');
  t.after(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  fs.copyFileSync(previousVersionDb, dbPath);
  const peek = new DatabaseSync(dbPath, { timeout: 1000 });
  peek.prepare('UPDATE schema_migrations SET sha256 = ? WHERE version = 1').run('0'.repeat(64));
  peek.close();

  assert.throws(() => openDatabase({ path: dbPath, timeoutMs: 1000 }), MigrationMismatchError);
});

test('ahead throws SchemaAheadError', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  const dbPath = path.join(dir, 'memory.db');
  t.after(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  const opened = openDatabase({ path: dbPath, timeoutMs: 1000 });
  opened.db.exec('PRAGMA user_version = 99');
  opened.db.close();

  assert.throws(() => openDatabase({ path: dbPath, timeoutMs: 1000 }), SchemaAheadError);
});

test('busy connection throws isBusyError and retry succeeds', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  const dbPath = path.join(dir, 'memory.db');
  const dbs: DatabaseSync[] = [];
  t.after(() => {
    for (const db of dbs) closeQuietly(db);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  const first = openDatabase({ path: dbPath, timeoutMs: 1000 });
  dbs.push(first.db);
  first.db.exec('BEGIN IMMEDIATE');

  const second = openDatabase({ path: dbPath, timeoutMs: 50 });
  dbs.push(second.db);

  let caught: unknown;
  try {
    second.db
      .prepare('INSERT INTO runtime_state (key, value_json, updated_at) VALUES (?, ?, ?)')
      .run('busy', '{}', 1);
    assert.fail('expected SQLITE_BUSY');
  } catch (error) {
    caught = error;
  }
  assert.equal(isBusyError(caught), true);

  first.db.exec('ROLLBACK');
  second.db
    .prepare('INSERT INTO runtime_state (key, value_json, updated_at) VALUES (?, ?, ?)')
    .run('busy', '{}', 1);
  assert.equal(
    second.db.prepare('SELECT key FROM runtime_state WHERE key = ?').get('busy')?.key,
    'busy',
  );
});

test('an update that touches neither indexed column leaves both FTS indexes alone', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oboete-mig-'));
  t.after(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  const opened = openDatabase({ path: path.join(dir, 'memory.db'), timeoutMs: 1000 });
  t.after(() => closeQuietly(opened.db));
  const db = opened.db;
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, created_at, last_seen_at)
     VALUES ('r_fts', 'common_dir', '/tmp/oboete-fts-trigger', 1, 1)`,
  ).run();
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, content_hash, sensitivity)
     VALUES ('m_fts', 'r_fts', 'discovery', ?, ?, ?, 'hash_fts_trigger', 'local_only')`,
  ).run('pterodactyl title', 'body about the index', 'あい うえ');

  const index = (): string =>
    JSON.stringify([
      db.prepare('SELECT id, quote(block) AS block FROM memories_fts_data ORDER BY id').all(),
      db.prepare('SELECT id, quote(block) AS block FROM memories_fts_cjk_data ORDER BY id').all(),
    ]);
  const matches = (table: string, column: string, term: string): number =>
    Number(
      db.prepare(`SELECT COUNT(*) AS n FROM ${table} WHERE ${column} MATCH ?`).get(term)?.n ?? -1,
    );

  // The injection path writes last_injected_at inside the 300 ms hook budget, so it must not
  // re-tokenize either index (contracts/agents.md hook SLAs).
  const before = index();
  db.prepare('UPDATE memories SET last_injected_at = 5, pinned_at = 6 WHERE id = ?').run('m_fts');
  assert.equal(index(), before);

  // A change of an indexed column still reaches its own index, and only its own.
  db.prepare('UPDATE memories SET title = ? WHERE id = ?').run('brontosaurus title', 'm_fts');
  assert.equal(matches('memories_fts', 'memories_fts', 'pterodactyl'), 0);
  assert.equal(matches('memories_fts', 'memories_fts', 'brontosaurus'), 1);
  assert.equal(matches('memories_fts_cjk', 'memories_fts_cjk', 'あい'), 1);

  db.prepare('UPDATE memories SET cjk_bigrams = ? WHERE id = ?').run('かき くけ', 'm_fts');
  assert.equal(matches('memories_fts_cjk', 'memories_fts_cjk', 'あい'), 0);
  assert.equal(matches('memories_fts_cjk', 'memories_fts_cjk', 'かき'), 1);
  assert.equal(matches('memories_fts', 'memories_fts', 'brontosaurus'), 1);
});
