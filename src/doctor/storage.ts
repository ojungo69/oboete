import { randomUUID } from 'node:crypto';
import { accessSync, constants, existsSync, readdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';

import { LATEST_SCHEMA_VERSION, SchemaAheadError, openDatabase, sqliteErrorInfo } from '../db/open.js';
import {
  asNumber,
  countOf,
  dbUnread,
  degraded,
  healthy,
  unverified,
  warning,
  type DoctorItem,
} from '../doctor.js';
import type { OboetePaths } from '../paths.js';
import { describe } from '../setup/setup.js';
import { listSpool } from '../spool.js';
import { stale } from '../worker/lease.js';

const DATABASE_TIMEOUT_MS = 5_000;
const SQLITE_CORRUPT = 11;
const SQLITE_READONLY = 8;
const SQLITE_NOTADB = 26;

const CORRUPT_RECOVERY =
  'Back up the file; run `oboete export` if it is readable; move the database aside; run `oboete setup`; then `oboete import` the export.';

export type StorageOpen = {
  item: DoctorItem;
  db: DatabaseSync | null;
  schemaVersion: number | null;
  schemaAhead: boolean;
  integrityFailed: boolean;
};

export function openStorage(paths: OboetePaths): StorageOpen {
  if (!existsSync(paths.db)) {
    return {
      item: degraded(
        'storage',
        `No database at ${paths.db}.`,
        'Hooks spool every event and nothing is summarized or injected.',
        '`oboete setup`',
      ),
      db: null,
      schemaVersion: null,
      schemaAhead: false,
      integrityFailed: false,
    };
  }

  let writable = true;
  try {
    accessSync(paths.db, constants.W_OK);
  } catch {
    writable = false;
  }
  if (!writable) {
    return {
      item: notWritableItem(paths),
      db: null,
      schemaVersion: null,
      schemaAhead: false,
      integrityFailed: false,
    };
  }

  return openExistingStorage(paths);
}

function openExistingStorage(paths: OboetePaths): StorageOpen {
  try {
    // `hook: true` reads the schema version without migrating: a diagnosis must not rewrite the
    // file it examines, and the migration item is the one that names a pending migration.
    const opened = openDatabase({ path: paths.db, timeoutMs: DATABASE_TIMEOUT_MS, hook: true });
    if (opened.schemaVersion > LATEST_SCHEMA_VERSION) {
      closeQuietly(opened.db);
      throw new SchemaAheadError(opened.schemaVersion);
    }
    if (opened.schemaBehind) {
      closeQuietly(opened.db);
      return {
        item: healthy(
          'storage',
          `\`${paths.db}\` opened; a schema migration is pending, so the items that read it are unverified.`,
        ),
        db: null,
        schemaVersion: opened.schemaVersion,
        schemaAhead: false,
        integrityFailed: false,
      };
    }
    return finishStorageOpen(paths, opened.db, opened.schemaVersion, false);
  } catch (error) {
    return storageOpenFailure(paths, error);
  }
}

function storageOpenFailure(paths: OboetePaths, error: unknown): StorageOpen {
  if (error instanceof SchemaAheadError) {
    return schemaAheadStorage(paths, error.userVersion);
  }
  if (isIntegrityFailure(error)) {
    return {
      item: degraded(
        'storage',
        integritySentence(error),
        'Hooks spool every event; nothing is summarized, injected or searchable until storage is repaired.',
        CORRUPT_RECOVERY,
      ),
      db: null,
      schemaVersion: null,
      schemaAhead: false,
      integrityFailed: true,
    };
  }
  if (isReadonlyError(error)) {
    return {
      item: notWritableItem(paths),
      db: null,
      schemaVersion: null,
      schemaAhead: false,
      integrityFailed: false,
    };
  }
  return {
    item: degraded(
      'storage',
      describe(error),
      'Hooks spool every event and nothing is summarized or injected.',
      '`oboete setup`',
    ),
    db: null,
    schemaVersion: null,
    schemaAhead: false,
    integrityFailed: false,
  };
}

function schemaAheadStorage(paths: OboetePaths, schemaVersion: number): StorageOpen {
  return {
    item: healthy(
      'storage',
      `\`${paths.db}\` opened; the schema is newer than this bundle knows.`,
    ),
    db: null,
    schemaVersion,
    schemaAhead: true,
    integrityFailed: false,
  };
}

function finishStorageOpen(
  paths: OboetePaths,
  db: DatabaseSync,
  schemaVersion: number,
  schemaAhead: boolean,
): StorageOpen {
  try {
    const row = db.prepare('PRAGMA quick_check').get();
    const result = row?.quick_check;
    if (result !== 'ok') {
      closeQuietly(db);
      return integrityFailureStorage(
        schemaVersion,
        schemaAhead,
        `PRAGMA quick_check returned ${String(result)}.`,
      );
    }
  } catch (error) {
    if (isIntegrityFailure(error)) {
      closeQuietly(db);
      return integrityFailureStorage(schemaVersion, schemaAhead, integritySentence(error));
    }
    throw error;
  }

  const memories = countOf(db.prepare('SELECT count(*) AS n FROM memories').get());
  return {
    item: healthy(
      'storage',
      `\`${paths.db}\` opened; PRAGMA quick_check returned ok; ${memories} memories.`,
    ),
    db,
    schemaVersion,
    schemaAhead,
    integrityFailed: false,
  };
}

function integrityFailureStorage(
  schemaVersion: number,
  schemaAhead: boolean,
  reason: string,
): StorageOpen {
  return {
    item: degraded(
      'storage',
      reason,
      'Hooks spool every event; nothing is summarized, injected or searchable until storage is repaired.',
      CORRUPT_RECOVERY,
    ),
    db: null,
    schemaVersion,
    schemaAhead,
    integrityFailed: true,
  };
}

function closeQuietly(db: DatabaseSync): void {
  try {
    db.close();
  } catch {
    // The handle must not leak into later items.
  }
}

function notWritableItem(paths: OboetePaths): DoctorItem {
  return degraded(
    'storage',
    `The database at ${paths.db} is not writable.`,
    'Hooks spool every event until the file is writable again; nothing new is summarized.',
    `\`chmod u+rw ${paths.db}\` (and the \`-wal\`/\`-shm\` files next to it)`,
  );
}

export function ftsItem(db: DatabaseSync | null, integrityFailed: boolean): DoctorItem {
  if (db === null) {
    return dbUnread(
      'fts',
      integrityFailed,
      'The database is unavailable, so full-text search could not be verified.',
      'Search and injection cannot be checked until storage is open.',
      '`oboete doctor` after storage is repaired.',
    );
  }
  try {
    db.prepare('SELECT count(*) AS n FROM memories_fts').get();
    db.prepare('SELECT count(*) AS n FROM memories_fts_cjk').get();
    return healthy('fts', 'Full-text search is available (lexical in M1).');
  } catch (error) {
    return degraded(
      'fts',
      describe(error),
      'Search and injection return nothing until full-text search is back (packs say `index_unavailable`).',
      'Use a Node.js build whose bundled SQLite has FTS5 (22.16 and 24.x do), then run `oboete doctor` again.',
    );
  }
}

export function migrationItem(
  schemaVersion: number | null,
  schemaAhead: boolean,
  integrityFailed: boolean,
): DoctorItem {
  if (integrityFailed) {
    return dbUnread(
      'migration',
      true,
      'The database is unavailable, so the schema version could not be verified.',
      'Memories cannot be summarized or searched until storage is open.',
      '`oboete doctor` after storage is repaired.',
    );
  }
  if (schemaVersion === null) {
    return unverified(
      'migration',
      'The database is unavailable, so the schema version could not be verified.',
      'Memories cannot be summarized or searched until storage is open.',
      '`oboete doctor` after storage is repaired.',
    );
  }
  if (schemaAhead || schemaVersion > LATEST_SCHEMA_VERSION) {
    return degraded(
      'migration',
      `The database schema is version ${schemaVersion}, newer than this bundle knows; upgrade oboete.`,
      'This version of oboete cannot migrate or write this database.',
      `Upgrade oboete to a version that knows schema version ${schemaVersion}.`,
    );
  }
  if (schemaVersion < LATEST_SCHEMA_VERSION) {
    return degraded(
      'migration',
      `The schema is at version ${schemaVersion}, which is behind version ${LATEST_SCHEMA_VERSION}, the latest this bundle knows.`,
      'New columns and indexes this version expects are missing.',
      '`oboete setup` migrates the database on the next command that opens it.',
    );
  }
  return healthy(
    'migration',
    `The schema is at version ${schemaVersion}, the latest this bundle knows.`,
  );
}

export function workerItem(
  db: DatabaseSync | null,
  now: number,
  integrityFailed: boolean,
): DoctorItem {
  if (db === null) {
    return dbUnread(
      'worker',
      integrityFailed,
      'The database is unavailable, so the worker lease could not be verified.',
      'Queued events cannot be summarized until storage is open.',
      '`oboete doctor` after storage is repaired.',
    );
  }
  try {
    const row = db.prepare('SELECT owner_token, pid, heartbeat_at FROM worker_lease WHERE id = 1').get();
    if (row?.owner_token == null) {
      return healthy('worker', 'No worker is running; a hook starts one when work is queued.');
    }
    const processId = asNumber(row.pid) ?? 0;
    if (stale(row.heartbeat_at, now)) {
      const heartbeat = asNumber(row.heartbeat_at);
      const seconds =
        heartbeat === null ? 0 : Math.max(0, Math.round((now - heartbeat) / 1000));
      return degraded(
        'worker',
        `The worker process ${processId} holds the lease but its last heartbeat was ${seconds} seconds ago.`,
        'Queued events are not summarized until the lease is reclaimed.',
        '`oboete observe` (it reclaims a stale lease and releases it when the queue is empty)',
      );
    }
    const heartbeat = asNumber(row.heartbeat_at) ?? now;
    const seconds = Math.max(0, Math.round((now - heartbeat) / 1000));
    return healthy(
      'worker',
      `The worker process ${processId} is alive (heartbeat ${seconds} seconds ago).`,
    );
  } catch (error) {
    return degraded(
      'worker',
      describe(error),
      'Queued events are not summarized until the lease is reclaimed.',
      '`oboete observe` (it reclaims a stale lease and releases it when the queue is empty)',
    );
  }
}

export function spoolItem(paths: OboetePaths): DoctorItem {
  const probe = join(paths.spool, `.doctor-${process.pid}-${randomUUID()}`);
  let writable = true;
  let created = false;
  try {
    writeFileSync(probe, '', { flag: 'wx', mode: 0o600 });
    created = true;
  } catch {
    writable = false;
  } finally {
    if (created) {
      try {
        unlinkSync(probe);
      } catch {
        // The probe file is gone already.
      }
    }
  }

  const backlog = listSpool(paths).length;
  const quarantined = countFiles(paths.spoolFailed);

  if (!writable) {
    const waiting = backlog > 0 ? ` (${backlog} events waiting)` : '';
    return degraded(
      'spool',
      `The spool directory ${paths.spool} is not writable${waiting}.`,
      'When the database is also unavailable, events are lost (the hook reports the count on stderr).',
      `\`chmod u+rwx ${paths.spool}\``,
    );
  }
  if (backlog > 0) {
    return degraded(
      'spool',
      `${backlog} events are waiting in the spool.`,
      'They are not summarized or searchable yet.',
      '`oboete observe`',
    );
  }
  if (quarantined > 0) {
    return warning(
      'spool',
      `${quarantined} quarantined files are under ${paths.spoolFailed}.`,
      'Those events were not recovered into storage.',
      `Inspect and delete the files under ${paths.spoolFailed}.`,
    );
  }
  return healthy('spool', 'Spool is writable and empty.');
}

function countFiles(directory: string): number {
  if (!existsSync(directory)) return 0;
  return readdirSync(directory, { withFileTypes: true }).filter((entry) => entry.isFile()).length;
}

/** SQLite's own words ("file is not a database") as a sentence that names the file's state. */
function integritySentence(error: unknown): string {
  const text = describe(error).trim().replace(/\.$/u, '');
  return /not a database/i.test(text)
    ? 'The file is not a SQLite database (its header is not the SQLite format).'
    : `The database is corrupt: ${text}.`;
}

function isIntegrityFailure(error: unknown): boolean {
  const { message, errcode, errstr } = sqliteErrorInfo(error);
  if (errcode === SQLITE_CORRUPT || errcode === SQLITE_NOTADB) return true;
  const text = `${message} ${errstr ?? ''}`;
  return /SQLITE_NOTADB|SQLITE_CORRUPT|file is not a database|database disk image is malformed|not a database/i.test(
    text,
  );
}

function isReadonlyError(error: unknown): boolean {
  const { message, errcode, errstr } = sqliteErrorInfo(error);
  if (errcode === SQLITE_READONLY) return true;
  const text = `${message} ${errstr ?? ''}`;
  return /readonly|SQLITE_READONLY|attempt to write a readonly/i.test(text);
}
