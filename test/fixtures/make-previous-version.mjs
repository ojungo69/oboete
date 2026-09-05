import { createHash } from 'node:crypto';
import { readFileSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';

const dir = dirname(fileURLToPath(import.meta.url));
const sql = readFileSync(join(dir, '../../src/db/migrations/0001_core.sql'), 'utf8');
const out = join(dir, 'previous-version.db');
for (const suffix of ['', '-wal', '-shm']) {
  rmSync(out + suffix, { force: true });
}
const db = new DatabaseSync(out, { timeout: 1000 });
// Default 4 KiB pages make this schema 106 KB; the committed fixture must stay under 100 KB.
db.exec('PRAGMA page_size = 1024');
db.exec('PRAGMA journal_mode = WAL');
db.exec('PRAGMA foreign_keys = ON');
db.exec('PRAGMA synchronous = NORMAL');
db.exec('BEGIN');
db.exec(sql);
db.prepare(
  'INSERT INTO schema_migrations(version, name, sha256, applied_at) VALUES (?, ?, ?, ?)',
).run(1, '0001_core', createHash('sha256').update(sql, 'utf8').digest('hex'), Date.now());
db.exec('PRAGMA user_version = 1');
db.exec('COMMIT');
db.exec('PRAGMA journal_mode = DELETE');
db.exec('VACUUM');
db.close();
