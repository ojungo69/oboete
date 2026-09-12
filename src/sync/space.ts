// Sync space, keys, consent, lock, push and pull (contracts/sync.md "Sync space, replicas and
// keys", "Consent", "Push", "Pull", "Commands and health"). The only I/O outside `$OBOETE_HOME`
// is file I/O below the one directory the user named. Security-owned: the key never leaves the
// key file except through `key show`; the space directory only ever receives bytes that passed
// the step 4 check; nothing runs unless consent matches.
import { createHash, randomBytes } from 'node:crypto';
import {
  closeSync, copyFileSync, existsSync, fstatSync, fsyncSync, mkdirSync, openSync, readdirSync, readFileSync, readSync, realpathSync,
  renameSync, rmSync, statSync, writeFileSync,
} from 'node:fs';
import { join, resolve as resolvePath } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';

import type { SyncConfig } from '../config.js';
import { isBusyError, loadSqlite, sqliteErrorInfo } from '../db/open.js';
import { prepared } from '../db/statements.js';
import type { OboetePaths } from '../paths.js';
import { compareCodeUnits } from '../hash.js';
import { updateConfigFile } from '../setup/consent.js';
import { applyStaged, materializeRows, resolveRow, type ApplyResult } from './apply.js';
import { captureLocalChanges } from './capture.js';
import { BundleError, decryptBundle, encryptBundle, keyId, MAX_CIPHERTEXT_BYTES } from './envelope.js';
import { BOUNDS } from './format.js';
import { canonicalJson } from './identity.js';
import { buildSnapshot, PublishError, type Withheld } from './publish.js';
import { BundleRejected, stageBundle } from './stage.js';
import { consentDrift, consentHashOf, loadSyncConfig, SyncError } from './status.js';
import { localRepoOf, replicaOriginId, SyncStoreError } from './store.js';

export { SyncError, loadSyncConfig, syncStatus, type SyncStatus } from './status.js';

export const KEY_LINE_PREFIX = 'oboete-sync-key/1';
const BUNDLE_NAME = /^([0-9a-f]{32})\.osb$/u;

export function syncPaths(paths: OboetePaths): { root: string; staging: string; key(spaceId: string): string; lock(spaceId: string): string } {
  const root = join(paths.home, 'sync');
  return { root, staging: join(root, 'staging'), key: (spaceId) => join(root, `${spaceId}.key`), lock: (spaceId) => join(root, `${spaceId}.lock.db`) };
}

export function spaceDirectory(directory: string, spaceId: string): string {
  return join(directory, 'oboete-sync', 'v1', spaceId);
}

export function keyLine(spaceId: string, key: Uint8Array): string {
  return `${KEY_LINE_PREFIX}:${spaceId}:${Buffer.from(key).toString('base64url')}`;
}

export function parseKeyLine(line: string): { spaceId: string; key: Buffer } {
  const match = /^oboete-sync-key\/1:([0-9a-f]{32}):([A-Za-z0-9_-]{43})$/u.exec(line.trim());
  if (match === null) throw new SyncError('invalid_key_line');
  const key = Buffer.from(match[2]!, 'base64url');
  if (key.length !== 32) throw new SyncError('invalid_key_line');
  return { spaceId: match[1]!, key };
}

function ensureSyncRoot(paths: OboetePaths): void {
  const { root, staging } = syncPaths(paths);
  mkdirSync(root, { recursive: true, mode: 0o700 });
  mkdirSync(staging, { recursive: true, mode: 0o700 });
}

function writeKey(paths: OboetePaths, spaceId: string, key: Uint8Array): void {
  ensureSyncRoot(paths);
  const path = syncPaths(paths).key(spaceId);
  writeFileSync(path, keyLine(spaceId, key), { mode: 0o600, flag: 'wx' });
}

export function readKey(paths: OboetePaths, spaceId: string): Buffer {
  const path = syncPaths(paths).key(spaceId);
  // Open once and check the permissions of the open descriptor, not the path: a check on the path
  // and a later read of the path can see different files (a TOCTOU race). fstat/read share the fd.
  let fd: number;
  try { fd = openSync(path, 'r'); } catch { throw new SyncError('key_missing'); }
  try {
    if ((fstatSync(fd).mode & 0o077) !== 0) throw new SyncError('key_permissions');
    return parseKeyLine(readFileSync(fd, 'utf8')).key;
  } finally { closeSync(fd); }
}

/** Consent check at the start of every push and pull: any drift performs no I/O. */
function checkConsent(db: DatabaseSync, config: SyncConfig): void {
  let realpath: string;
  try { realpath = realpathSync(config.directory); } catch { throw new SyncError('consent_mismatch', { changed: ['directory'] }); }
  const changed = consentDrift(db, { ...config, directory_realpath: realpath });
  if (changed.length > 0) throw new SyncError('consent_mismatch', { changed });
}

function recordSpace(db: DatabaseSync, paths: OboetePaths, config: SyncConfig, now: number): void {
  prepared(db, `INSERT INTO sync_spaces (space_id, directory, directory_realpath, key_id, classes_json, consent_hash, created_at)
    VALUES (?, ?, ?, ?, ?, ?, ?)`)
    .run(config.space_id, config.directory, config.directory_realpath, config.key_id, canonicalJson([...config.classes].sort(compareCodeUnits)),
      consentHashOf(config), now);
  updateConfigFile(paths, (root) => { root.sync = { ...config }; });
}

function assertNoSpace(db: DatabaseSync, paths: OboetePaths): void {
  if (loadSyncConfig(paths) !== null || prepared(db, 'SELECT 1 FROM sync_spaces LIMIT 1').get() !== undefined) throw new SyncError('space_exists');
}

function directoryOutsideHome(directory: string, paths: OboetePaths): string {
  const realpath = realpathSync(directory);
  const home = realpathSync(paths.home);
  if (realpath === home || realpath.startsWith(`${home}/`) || home.startsWith(`${realpath}/`)) throw new SyncError('directory_inside_home');
  return realpath;
}

/** `oboete sync init <dir>`: a new space and key; nothing is written to `<dir>` until the first push. */
export function initSpace(db: DatabaseSync, paths: OboetePaths, input: { directory: string; classes: readonly string[]; now: number }): { spaceId: string; keyLine: string } {
  assertNoSpace(db, paths);
  const directory = resolvePath(input.directory);
  const realpath = directoryOutsideHome(directory, paths);
  const spaceId = randomBytes(16).toString('hex');
  const key = randomBytes(32);
  const config: SyncConfig = { directory, directory_realpath: realpath, space_id: spaceId, key_id: keyId(key),
    classes: classesOf(input.classes) };
  writeKey(paths, spaceId, key);
  recordSpace(db, paths, config, input.now);
  return { spaceId, keyLine: keyLine(spaceId, key) };
}

/** `oboete sync join <dir>`: the key line typed on a TTY (never an argument, variable or file). */
export function joinSpace(db: DatabaseSync, paths: OboetePaths, input: { directory: string; keyLine: string; classes: readonly string[]; now: number }): { spaceId: string } {
  assertNoSpace(db, paths);
  const directory = resolvePath(input.directory);
  const realpath = directoryOutsideHome(directory, paths);
  const { spaceId, key } = parseKeyLine(input.keyLine);
  const config: SyncConfig = { directory, directory_realpath: realpath, space_id: spaceId, key_id: keyId(key),
    classes: classesOf(input.classes) };
  writeKey(paths, spaceId, key);
  recordSpace(db, paths, config, input.now);
  return { spaceId };
}

const SELECTABLE = ['eligible', 'local_only', 'private'] as const;

function classesOf(classes: readonly string[]): SyncConfig['classes'] {
  const selected = [...new Set(classes)].filter((value): value is (typeof SELECTABLE)[number] => (SELECTABLE as readonly string[]).includes(value));
  if (selected.length !== classes.length || selected.length === 0) throw new SyncError('invalid_classes');
  return selected;
}

/** `oboete sync key show`: the one line to carry to the next device (the CLI checks the TTY). */
export function showKey(paths: OboetePaths): string {
  const config = loadSyncConfig(paths);
  if (config === null) throw new SyncError('space_not_configured');
  return keyLine(config.space_id, readKey(paths, config.space_id));
}

/** `oboete sync leave`: key, cursors, consent and this replica's own bundle file. */
export function leaveSpace(db: DatabaseSync, paths: OboetePaths): void {
  const config = loadSyncConfig(paths);
  if (config === null) throw new SyncError('space_not_configured');
  const own = join(spaceDirectory(config.directory, config.space_id), `${replicaOriginId(db)}.osb`);
  rmSync(own, { force: true });
  rmSync(syncPaths(paths).key(config.space_id), { force: true });
  prepared(db, 'DELETE FROM sync_cursors WHERE space_id = ?').run(config.space_id);
  prepared(db, 'DELETE FROM sync_spaces WHERE space_id = ?').run(config.space_id);
  updateConfigFile(paths, (root) => { delete root.sync; });
}

/** The per-space lock: `BEGIN IMMEDIATE` on a one-table SQLite file, released with the process. */
export function withSpaceLock<T>(paths: OboetePaths, spaceId: string, fn: () => T): T {
  ensureSyncRoot(paths);
  const lock = new (loadSqlite().DatabaseSync)(syncPaths(paths).lock(spaceId));
  try {
    try {
      lock.exec('CREATE TABLE IF NOT EXISTS lock (id INTEGER PRIMARY KEY); BEGIN IMMEDIATE');
    } catch (error) {
      if (isBusyError(error)) throw new SyncError('busy');
      throw error;
    }
    try { return fn(); } catch (error) {
      // A store bound (revisions per origin, unknown origin) is a sync outcome, not a crash.
      if (error instanceof SyncStoreError) throw new SyncError(error.code);
      throw error;
    }
  } finally { if (lock.isOpen) lock.close(); }
}

function dataVersion(db: DatabaseSync): number {
  return Number(prepared(db, 'PRAGMA data_version').get()?.data_version);
}

export type PushResult = {
  outcome: 'unchanged' | 'published'; snapshotId: string; revisionLines: number; heads: number; withheld: Withheld; bytes: number; restarts: number;
};

/** `oboete sync push` (contracts/sync.md "Push" steps 0–5). */
export type PushProbe = (step: 'after_capture' | 'after_staging' | 'after_encrypt') => void;

export function pushSpace(db: DatabaseSync, paths: OboetePaths, input: { now: number; republish?: boolean; probe?: PushProbe }): PushResult {
  const config = loadSyncConfig(paths);
  if (config === null) throw new SyncError('space_not_configured');
  return withSpaceLock(paths, config.space_id, () => {
    checkConsent(db, config);
    const key = readKey(paths, config.space_id);
    const replica = replicaOriginId(db);
    const { staging } = syncPaths(paths);
    const plain = join(staging, `${replica}.push.plain`);
    const cipher = join(staging, `${replica}.push.osb`);
    const space = spaceDirectory(config.directory, config.space_id);
    const published = join(space, `${replica}.osb`);
    const cleanup = (): void => { for (const path of [plain, `${plain}.body`, cipher]) rmSync(path, { force: true }); };
    const inTransaction = <T>(mode: 'BEGIN IMMEDIATE' | 'BEGIN', fn: () => T): T => {
      db.exec(mode);
      try { const value = fn(); db.exec('COMMIT'); return value; } catch (error) { db.exec('ROLLBACK'); throw error; }
    };
    try {
      for (let restarts = 0; restarts < 3; restarts += 1) {
        // Step 1: change pass on this connection; D0 is read before it, inside the same transaction.
        const baseline = inTransaction('BEGIN IMMEDIATE', () => { const version = dataVersion(db); captureLocalChanges(db, input.now); return version; });
        input.probe?.('after_capture');
        // Step 2: a read transaction from the same state.
        let snapshot: ReturnType<typeof buildSnapshot> | null;
        try {
          snapshot = inTransaction('BEGIN', () => dataVersion(db) !== baseline ? null
            : buildSnapshot(db, { spaceId: config.space_id, classes: config.classes, now: input.now, outputPath: plain }));
        } catch (error) {
          if (error instanceof PublishError) { cleanup(); throw new SyncError('publish_failed', { code: error.code }); }
          throw error;
        }
        if (snapshot === null) { cleanup(); continue; }
        input.probe?.('after_staging');
        const stored = prepared(db, 'SELECT last_pushed_snapshot_id, published_sha256, published_size FROM sync_spaces WHERE space_id = ?').get(config.space_id)!;
        const intact = existsSync(published) && statSync(published).size === Number(stored.published_size)
          && sha256File(published) === String(stored.published_sha256);
        const unchanged = input.republish !== true && String(stored.last_pushed_snapshot_id) === snapshot.snapshotId && intact;
        // Step 3: encrypt in the private staging directory unless nothing would be written.
        let encrypted: ReturnType<typeof encryptBundle> | null = null;
        if (!unchanged) {
          try { encrypted = encryptBundle(key, plain, cipher); } catch (error) {
            if (error instanceof BundleError && error.code === 'plaintext_too_large') { cleanup(); throw new SyncError('plaintext_too_large', { bytes: snapshot.bytes }); }
            throw error;
          }
        }
        input.probe?.('after_encrypt');
        // Step 4: re-check under the writer lock, then publish and record together.
        db.exec('BEGIN IMMEDIATE');
        if (dataVersion(db) !== baseline) { db.exec('ROLLBACK'); cleanup(); continue; }
        if (unchanged) { db.exec('ROLLBACK'); cleanup(); return { outcome: 'unchanged', restarts, ...snapshot }; }
        try {
          mkdirSync(space, { recursive: true });
          const temporary = join(space, `${replica}.osb.tmp-${randomBytes(6).toString('hex')}`);
          try {
            renameSync(cipher, temporary);
          } catch {
            copyFileSync(cipher, temporary);
            const fd = openSync(temporary, 'r+');
            try { fsyncSync(fd); } finally { closeSync(fd); }
          }
          renameSync(temporary, published);
          prepared(db, 'UPDATE sync_spaces SET last_pushed_snapshot_id = ?, published_sha256 = ?, published_size = ? WHERE space_id = ?')
            .run(snapshot.snapshotId, encrypted!.sha256, encrypted!.size, config.space_id);
          db.exec('COMMIT');
        } catch (error) { db.exec('ROLLBACK'); throw error; }
        // Temporary files an interrupted push left behind are this replica's to remove.
        for (const name of readdirSync(space)) {
          if (name.startsWith(`${replica}.osb.tmp-`)) rmSync(join(space, name), { force: true });
        }
        return { outcome: 'published', restarts, ...snapshot };
      }
      throw new SyncError('busy', { reason: 'concurrent_commits' });
    } finally { cleanup(); }
  });
}

function sha256File(path: string): string {
  const digest = createHash('sha256');
  const fd = openSync(path, 'r');
  try {
    const chunk = Buffer.alloc(1 << 20);
    for (;;) {
      const read = readSync(fd, chunk, 0, chunk.length, null);
      if (read === 0) break;
      digest.update(chunk.subarray(0, read));
    }
  } finally { closeSync(fd); }
  return digest.digest('hex');
}

export type PullResult = {
  bundles: { replica: string; outcome: 'applied' | 'skipped' | 'rejected'; reason: string | null; result?: ApplyResult }[];
};

/** `oboete sync pull` (contracts/sync.md "Pull" steps 0–7). */
export function pullSpace(db: DatabaseSync, paths: OboetePaths, input: { now: number }): PullResult {
  const config = loadSyncConfig(paths);
  if (config === null) throw new SyncError('space_not_configured');
  return withSpaceLock(paths, config.space_id, () => {
    checkConsent(db, config);
    const key = readKey(paths, config.space_id);
    const replica = replicaOriginId(db);
    const space = spaceDirectory(config.directory, config.space_id);
    const names = existsSync(space) ? readdirSync(space).filter((name) => BUNDLE_NAME.test(name) && !name.startsWith(replica)).sort(compareCodeUnits) : [];
    if (names.length > BOUNDS.replicasPerSpace) {
      const unknown = names.filter((name) => prepared(db, 'SELECT 1 FROM sync_cursors WHERE space_id = ? AND replica_origin_id = ?')
        .get(config.space_id, name.slice(0, 32)) === undefined);
      throw new SyncError('too_many_replicas', { count: names.length, directory: space, without_cursor: unknown });
    }
    const result: PullResult = { bundles: [] };
    const { staging } = syncPaths(paths);
    for (const name of names) {
      const sender = name.slice(0, 32);
      const file = join(space, name);
      const plain = join(staging, `${sender}.pull.plain`);
      const scratch = join(staging, `${sender}.pull.scratch`);
      try {
        if (statSync(file).size > MAX_CIPHERTEXT_BYTES) { result.bundles.push({ replica: sender, outcome: 'rejected', reason: 'oversize' }); continue; }
        decryptBundle(key, file, plain);
        const staged = stageBundle(db, { plaintextPath: plain, scratchPath: scratch, spaceId: config.space_id, senderOriginId: sender });
        try {
          const cursor = prepared(db, 'SELECT snapshot_id FROM sync_cursors WHERE space_id = ? AND replica_origin_id = ?').get(config.space_id, sender);
          if (cursor !== undefined && String(cursor.snapshot_id) === staged.header.snapshot_id) {
            result.bundles.push({ replica: sender, outcome: 'skipped', reason: 'cursor' });
            continue;
          }
          db.exec('BEGIN IMMEDIATE');
          try {
            const applied = applyStaged(db, staged, { senderOriginId: sender, now: input.now });
            prepared(db, `INSERT INTO sync_cursors (space_id, replica_origin_id, snapshot_id, pulled_at) VALUES (?, ?, ?, ?)
              ON CONFLICT(space_id, replica_origin_id) DO UPDATE SET snapshot_id = excluded.snapshot_id, pulled_at = excluded.pulled_at`)
              .run(config.space_id, sender, staged.header.snapshot_id, input.now);
            db.exec('COMMIT');
            result.bundles.push({ replica: sender, outcome: 'applied', reason: null, result: applied });
          } catch (error) { db.exec('ROLLBACK'); throw error; }
        } finally { staged.close(); }
      } catch (error) {
        if (error instanceof BundleError || error instanceof BundleRejected) {
          result.bundles.push({ replica: sender, outcome: 'rejected', reason: error.code });
        } else if (error instanceof SyncError) throw error;
        else if (isBusyError(error)) throw new SyncError('busy');
        else result.bundles.push({ replica: sender, outcome: 'rejected', reason: `apply_failed:${errorLabel(error)}` });
      } finally { rmSync(plain, { force: true }); rmSync(scratch, { force: true }); }
    }
    return result;
  });
}

/** `oboete sync map-repo <key> <local repo id>`: then re-evaluates withheld payloads without a pull. */
export function mapRepo(db: DatabaseSync, paths: OboetePaths, input: { repoKey: string; localRepoId: string; now: number }): { reapplied: number } {
  const config = loadSyncConfig(paths);
  if (config === null) throw new SyncError('space_not_configured');
  return withSpaceLock(paths, config.space_id, () => {
    if (prepared(db, 'SELECT 1 FROM repos WHERE id = ?').get(input.localRepoId) === undefined) throw new SyncError('unknown_repo');
    if (localRepoOf(db, input.repoKey) !== null) throw new SyncError('already_mapped');
    if (prepared(db, 'SELECT 1 FROM sync_repo_mappings WHERE repo_key = ?').get(input.repoKey) === undefined) throw new SyncError('unknown_repo_key');
    db.exec('BEGIN IMMEDIATE');
    try {
      // Local changes first: the mapping lets withheld origins alias onto rows this device may have edited.
      captureLocalChanges(db, input.now);
      prepared(db, 'UPDATE sync_repo_mappings SET local_repo_id = ? WHERE repo_key = ?').run(input.localRepoId, input.repoKey);
      const reapplied = reapplyWithheld(db, input.now);
      db.exec('COMMIT');
      return { reapplied };
    } catch (error) { db.exec('ROLLBACK'); throw error; }
  });
}

/**
 * Withheld rows are re-evaluated exactly as a pull would materialize them; every origin without a
 * local row is offered the rows the mapping resolves by the pass itself. Only rows that got a
 * local row count.
 */
function reapplyWithheld(db: DatabaseSync, now: number): number {
  const rows = prepared(db, 'SELECT DISTINCT canonical_origin_id AS id FROM sync_origins WHERE withheld_reason IS NOT NULL').all()
    .map((row) => String(row.id));
  return materializeRows(db, rows, now).materialized;
}

export { resolveRow };

/** A stable label for an unexpected apply error: the SQLite result code or the error class, never its message (which may quote input). */
function errorLabel(error: unknown): string {
  const info = sqliteErrorInfo(error);
  if (info.errcode !== undefined) return `sqlite:${String(info.errcode)}`;
  return error instanceof Error ? error.name : 'unknown';
}
