import type { DatabaseSync, StatementSync } from 'node:sqlite';

// One prepared statement per (database, SQL). A statement prepared inside a per-record loop is
// native memory that only a garbage collection releases, and a million-record import prepares
// millions of them before that happens. Entries die with their database object.
const statements = new WeakMap<DatabaseSync, Map<string, StatementSync>>();

export function prepared(db: DatabaseSync, sql: string): StatementSync {
  let cache = statements.get(db);
  if (cache === undefined) { cache = new Map(); statements.set(db, cache); }
  let statement = cache.get(sql);
  if (statement === undefined) { statement = db.prepare(sql); cache.set(sql, statement); }
  return statement;
}
