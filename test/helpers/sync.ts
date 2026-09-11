// Shared fixtures for the US6 sync tests (contracts/sync.md "Verification"): replicas are separate
// homes with one repository each; `publish`/`pull` move a plaintext snapshot between them without
// the envelope, which has its own tests.
import assert from 'node:assert/strict';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';

import { openDatabase } from '../../src/db/open.js';
import { sha256Hex } from '../../src/hash.js';
import { oboetePaths } from '../../src/paths.js';
import { applyStaged, type ApplyResult } from '../../src/sync/apply.js';
import { captureLocalChanges } from '../../src/sync/capture.js';
import { buildSnapshot } from '../../src/sync/publish.js';
import { stageBundle } from '../../src/sync/stage.js';
import { readOrigin, replicaOriginId, revisionsOfOrigin, type Revision } from '../../src/sync/store.js';
import { withTempHome } from './home.js';

export const REPO = 'r1b2c3d4e5f60718';
export const REMOTE = 'github.com/example/sync';

export async function withStore(fn: (db: DatabaseSync, replica: string) => void | Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      opened.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
        VALUES (?, 'remote', ?, '/work/sync', 1, 1)`).run(REPO, REMOTE);
      await fn(opened.db, replicaOriginId(opened.db));
    } finally { if (opened.db.isOpen) opened.db.close(); }
  });
}

export function insertMemory(db: DatabaseSync, id: string, title: string, body: string,
  extra: { sensitivity?: string; deleted_at?: number | null; pinned_at?: number | null } = {}): { material: string; content: string } {
  const material = sha256Hex(JSON.stringify([title.toLowerCase(), body.toLowerCase()]));
  const content = sha256Hex(JSON.stringify([REPO, material]));
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity,
    review_state, created_at, deleted_at, pinned_at) VALUES (?, ?, 'discovery', ?, ?, '', ?, ?, ?, 'reviewed', 1, ?, ?)`)
    .run(id, REPO, title, body, material, content, extra.sensitivity ?? 'eligible', extra.deleted_at ?? null, extra.pinned_at ?? null);
  return { material, content };
}

export function insertSource(db: DatabaseSync, memoryId: string, citation: string): number {
  db.prepare(`INSERT INTO memory_sources (memory_id, citation_kind, citation_value, source_agent, context_only)
    VALUES (?, 'file_read', ?, 'claude', 0)`).run(memoryId, citation);
  return Number(db.prepare('SELECT last_insert_rowid() AS id').get()?.id);
}

export function revisions(db: DatabaseSync, originId: string): Revision[] {
  return revisionsOfOrigin(db, originId);
}

export type Replica = { db: DatabaseSync; home: string; id: string };

export async function withReplicas(count: number, fn: (replicas: Replica[], dir: string) => void | Promise<void>): Promise<void> {
  const replicas: Replica[] = [];
  const open = async (index: number): Promise<void> => {
    if (index === count) {
      await withTempHome(async (dir) => {
        try { await fn(replicas, dir); } finally { for (const replica of replicas) if (replica.db.isOpen) replica.db.close(); }
      });
      return;
    }
    await withTempHome(async (home) => {
      const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
      opened.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
        VALUES (?, 'remote', ?, '/work/sync', 1, 1)`).run(REPO, REMOTE);
      replicas.push({ db: opened.db, home, id: replicaOriginId(opened.db) });
      await open(index + 1);
    });
  };
  await open(0);
}

export const SPACE = 'd'.repeat(32);

/** Push: capture then build the plaintext; returns the file path (no encryption in these tests). */
export function publish(replica: Replica, dir: string, classes: readonly ('eligible' | 'local_only' | 'private')[] = ['eligible', 'local_only', 'private']): string {
  replica.db.exec('BEGIN IMMEDIATE');
  captureLocalChanges(replica.db, 100);
  replica.db.exec('COMMIT');
  const path = join(dir, `${replica.id}.plain`);
  buildSnapshot(replica.db, { spaceId: SPACE, classes, now: 100, outputPath: path });
  return path;
}

/** Pull one plaintext bundle from `from` into `into`. */
export function pull(into: Replica, from: Replica, path: string, now = 200): ApplyResult {
  const staged = stageBundle(into.db, { plaintextPath: path, scratchPath: `${path}.${into.id}.scratch`, spaceId: SPACE, senderOriginId: from.id });
  try {
    into.db.exec('BEGIN IMMEDIATE');
    try {
      const result = applyStaged(into.db, staged, { senderOriginId: from.id, now });
      into.db.exec('COMMIT');
      return result;
    } catch (error) { into.db.exec('ROLLBACK'); throw error; }
  } finally { staged.close(); }
}

/** The local row an origin maps to on a device (local ids differ between devices). */
export function memoryOf(replica: Replica, creator: Replica, localIdOnCreator: string): Record<string, unknown> {
  const local = readOrigin(replica.db, `${creator.id}:${localIdOnCreator}`)?.local_id;
  assert.ok(local, `origin ${creator.id}:${localIdOnCreator} has a row on ${replica.id}`);
  return replica.db.prepare('SELECT * FROM memories WHERE id = ?').get(local)!;
}

export function revisionCount(db: DatabaseSync): number {
  return Number(db.prepare('SELECT COUNT(*) AS n FROM sync_revisions').get()?.n);
}

export async function withHomes(count: number, fn: (homes: string[], shared: string) => void | Promise<void>): Promise<void> {
  const homes: string[] = [];
  const open = async (index: number): Promise<void> => {
    if (index === count) { await withTempHome((shared) => fn(homes, shared)); return; }
    await withTempHome(async (home) => { homes.push(home); await open(index + 1); });
  };
  await open(0);
}

export function openHome(home: string): DatabaseSync {
  const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
  opened.db.prepare(`INSERT OR IGNORE INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
    VALUES (?, 'remote', ?, '/work/sync', 1, 1)`).run(REPO, REMOTE);
  return opened.db;
}
