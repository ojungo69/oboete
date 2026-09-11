import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import type * as Sqlite from 'node:sqlite';

import { sha256Hex } from '../hash.js';
import { stale } from '../worker/lease-clock.js';

import sql0001 from './migrations/0001_core.sql';
import sql0002 from './migrations/0002_memory_search.sql';
import sql0003 from './migrations/0003_operations.sql';
import sql0004 from './migrations/0004_memory_processing.sql';
import sql0005 from './migrations/0005_work_continuity.sql';
import sql0006 from './migrations/0006_memory_visibility.sql';
import sql0007 from './migrations/0007_migration_records.sql';
import sql0008 from './migrations/0008_sync.sql';

type DatabaseSync = Sqlite.DatabaseSync;

/**
 * `node:sqlite` is loaded on the first open, not at import: on Node 22.16 loading it emits an
 * ExperimentalWarning while the module graph is still linking, before cli.ts can install its warning
 * filter, and R6 keeps the hook's stderr for the count of unstored events.
 */
let sqlite: typeof Sqlite | null = null;
function loadSqlite(): typeof Sqlite {
  sqlite ??= createRequire(import.meta.url)('node:sqlite') as typeof Sqlite;
  return sqlite;
}

export const MIGRATIONS: {
  version: 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;
  name: '0001_core' | '0002_memory_search' | '0003_operations' | '0004_memory_processing' | '0005_work_continuity' | '0006_memory_visibility' | '0007_migration_records' | '0008_sync';
  sql: string;
}[] = [
  { version: 1, name: '0001_core', sql: sql0001 },
  { version: 2, name: '0002_memory_search', sql: sql0002 },
  { version: 3, name: '0003_operations', sql: sql0003 },
  { version: 4, name: '0004_memory_processing', sql: sql0004 },
  { version: 5, name: '0005_work_continuity', sql: sql0005 },
  { version: 6, name: '0006_memory_visibility', sql: sql0006 },
  { version: 7, name: '0007_migration_records', sql: sql0007 },
  { version: 8, name: '0008_sync', sql: sql0008 },
];

export const LATEST_SCHEMA_VERSION = 8;

export type OpenedDatabase = {
  db: DatabaseSync;
  schemaVersion: number;
  schemaBehind: boolean;
};

export class DatabaseMissingError extends Error {
  readonly code = 'DATABASE_MISSING';
  constructor(message = 'Database file does not exist') {
    super(message);
    this.name = 'DatabaseMissingError';
  }
}

export class MigrationMismatchError extends Error {
  readonly code = 'MIGRATION_MISMATCH';
  constructor(version: number) {
    super(`Migration ${version} checksum does not match the bundled SQL`);
    this.name = 'MigrationMismatchError';
  }
}

export class SchemaAheadError extends Error {
  readonly code = 'SCHEMA_AHEAD';
  readonly userVersion: number;
  constructor(userVersion: number) {
    super(
      `Database schema version ${userVersion} is newer than this bundle (${LATEST_SCHEMA_VERSION})`,
    );
    this.name = 'SchemaAheadError';
    this.userVersion = userVersion;
  }
}

export class MigrationBusyError extends Error {
  readonly code = 'MIGRATION_BUSY';
  readonly errcode = 5;
  constructor() {
    super('An active worker must finish before the database migration can run.');
    this.name = 'MigrationBusyError';
  }
}

export function sqliteErrorInfo(error: unknown): {
  message: string;
  errcode?: number;
  errstr?: string;
} {
  const message =
    error instanceof Error ? (error.message.split('\n')[0] ?? error.name) : String(error);
  if (typeof error !== 'object' || error === null) return { message };
  const record = error as { errcode?: unknown; errstr?: unknown };
  return {
    message,
    errcode: typeof record.errcode === 'number' ? record.errcode : undefined,
    errstr: typeof record.errstr === 'string' ? record.errstr : undefined,
  };
}

export function isBusyError(error: unknown): boolean {
  const info = sqliteErrorInfo(error);
  if (info.errcode === 5 || info.errcode === 6) return true;
  return typeof info.errstr === 'string' && /database is locked|busy/.test(info.errstr);
}

export function openDatabase(options: {
  path: string;
  timeoutMs: number;
  hook?: boolean;
  readOnly?: boolean;
}): OpenedDatabase {
  const hook = options.hook === true;
  if ((hook || options.readOnly === true) && !existsSync(options.path)) {
    throw new DatabaseMissingError(`Database file does not exist: ${options.path}`);
  }

  return openConfiguredDatabase(options, hook);
}

function readUserVersion(db: DatabaseSync): number {
  const row = db.prepare('PRAGMA user_version').get();
  return typeof row?.user_version === 'number' ? row.user_version : 0;
}

function hasSchemaMigrations(db: DatabaseSync): boolean {
  const row = db
    .prepare(
      `SELECT 1 AS ok FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'`,
    )
    .get();
  return row !== undefined;
}

function verifyAppliedHashes(db: DatabaseSync): void {
  if (!hasSchemaMigrations(db)) {
    return;
  }
  for (const row of db.prepare('SELECT version, sha256 FROM schema_migrations').all()) {
    if (typeof row.version !== 'number' || typeof row.sha256 !== 'string') {
      continue;
    }
    const bundled = MIGRATIONS.find((migration) => migration.version === row.version);
    if (bundled === undefined) {
      continue;
    }
    if (row.sha256 !== sha256Hex(bundled.sql)) {
      throw new MigrationMismatchError(row.version);
    }
  }
}

function rollbackIfNeeded(db: DatabaseSync): void {
  if (db.isTransaction) {
    db.exec('ROLLBACK');
  }
}

/** Runs under the migration's write lock, before the new schema is visible to an old worker. */
function fenceOldWorker(db: DatabaseSync): void {
  const lease = db.prepare('SELECT owner_token, heartbeat_at FROM worker_lease WHERE id = 1').get();
  if (lease === undefined || lease.owner_token === null) return;
  if (!stale(lease.heartbeat_at, Date.now())) throw new MigrationBusyError();
  db.prepare('UPDATE worker_lease SET owner_token = NULL, pid = NULL WHERE id = 1').run();
}

function migrate(db: DatabaseSync): number {
  let userVersion = readUserVersion(db);
  if (userVersion > LATEST_SCHEMA_VERSION) {
    throw new SchemaAheadError(userVersion);
  }
  verifyAppliedHashes(db);

  for (const migration of MIGRATIONS) {
    if (migration.version <= userVersion) {
      continue;
    }

    // BEGIN IMMEDIATE serializes concurrent CLI migrators; the loser re-reads user_version.
    db.exec('BEGIN IMMEDIATE');
    try {
      userVersion = readUserVersion(db);
      if (userVersion > LATEST_SCHEMA_VERSION) {
        throw new SchemaAheadError(userVersion);
      }
      verifyAppliedHashes(db);
      if (migration.version <= userVersion) {
        db.exec('COMMIT');
        continue;
      }

      if (userVersion > 0) fenceOldWorker(db);

      db.exec(migration.sql);
      db.prepare(
        'INSERT INTO schema_migrations(version, name, sha256, applied_at) VALUES (?, ?, ?, ?)',
      ).run(migration.version, migration.name, sha256Hex(migration.sql), Date.now());
      db.exec(`PRAGMA user_version = ${migration.version}`);
      db.exec('COMMIT');
      userVersion = migration.version;
    } catch (error) {
      try {
        rollbackIfNeeded(db);
      } catch {
        // Prefer the original migration error.
      }
      throw error;
    }
  }

  return userVersion;
}

function openConfiguredDatabase(
  options: Parameters<typeof openDatabase>[0],
  hook: boolean,
): OpenedDatabase {
  const db = new (loadSqlite().DatabaseSync)(options.path, {
    timeout: options.timeoutMs, readOnly: options.readOnly === true,
  });
  try {
    if (options.readOnly === true) {
      const schemaVersion = readUserVersion(db);
      if (schemaVersion > LATEST_SCHEMA_VERSION) throw new SchemaAheadError(schemaVersion);
      verifyAppliedHashes(db);
      return { db, schemaVersion, schemaBehind: schemaVersion < LATEST_SCHEMA_VERSION };
    }
    db.exec('PRAGMA journal_mode = WAL');
    db.exec('PRAGMA foreign_keys = ON');
    db.exec('PRAGMA synchronous = NORMAL');
    if (hook) {
      db.exec('PRAGMA wal_autocheckpoint = 0');
      const schemaVersion = readUserVersion(db);
      if (schemaVersion > LATEST_SCHEMA_VERSION) throw new SchemaAheadError(schemaVersion);
      return {
        db,
        schemaVersion,
        schemaBehind: schemaVersion < LATEST_SCHEMA_VERSION,
      };
    }

    const schemaVersion = migrate(db);
    return { db, schemaVersion, schemaBehind: false };
  } catch (error) {
    try {
      db.close();
    } catch {
      // Prefer the original open/migrate error.
    }
    throw error;
  }
}
