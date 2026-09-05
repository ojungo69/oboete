import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import type * as Sqlite from 'node:sqlite';
import type { DatabaseSync } from 'node:sqlite';

import { sha256Hex } from '../hash.js';

import sql0001 from './migrations/0001_core.sql';
import sql0002 from './migrations/0002_memory_search.sql';
import sql0003 from './migrations/0003_operations.sql';

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
  version: 1 | 2 | 3;
  name: '0001_core' | '0002_memory_search' | '0003_operations';
  sql: string;
}[] = [
  { version: 1, name: '0001_core', sql: sql0001 },
  { version: 2, name: '0002_memory_search', sql: sql0002 },
  { version: 3, name: '0003_operations', sql: sql0003 },
];

export const LATEST_SCHEMA_VERSION = 3;

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
  constructor(userVersion: number) {
    super(
      `Database schema version ${userVersion} is newer than this bundle (${LATEST_SCHEMA_VERSION})`,
    );
    this.name = 'SchemaAheadError';
  }
}

export function isBusyError(error: unknown): boolean {
  if (typeof error !== 'object' || error === null) {
    return false;
  }
  const sqliteError = error as { errcode?: unknown; errstr?: unknown };
  if (sqliteError.errcode === 5 || sqliteError.errcode === 6) {
    return true;
  }
  return typeof sqliteError.errstr === 'string' && /database is locked|busy/.test(sqliteError.errstr);
}

export function openDatabase(options: {
  path: string;
  timeoutMs: number;
  hook?: boolean;
}): OpenedDatabase {
  const hook = options.hook === true;
  if (hook && !existsSync(options.path)) {
    throw new DatabaseMissingError(`Database file does not exist: ${options.path}`);
  }

  const db = new (loadSqlite().DatabaseSync)(options.path, { timeout: options.timeoutMs });
  try {
    db.exec('PRAGMA journal_mode = WAL');
    db.exec('PRAGMA foreign_keys = ON');
    db.exec('PRAGMA synchronous = NORMAL');
    if (hook) {
      db.exec('PRAGMA wal_autocheckpoint = 0');
      const schemaVersion = readUserVersion(db);
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
