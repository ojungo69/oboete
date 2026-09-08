import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { openDatabase } from '../../src/db/open.js';
import {
  observerOutputSchema,
  type Observation,
  type ObserverOutput,
} from '../../src/observer/contract.js';
import { oboetePaths } from '../../src/paths.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { claimLease } from '../../src/worker/lease.js';
import { withTempHome } from './home.js';

export const NOW = 1_757_000_000_000;
const DAY = 24 * 60 * 60 * 1000;
export const REPO_ID = 'a1b2c3d4e5f60718';

/** The identity rules of A13, recomputed here instead of through src/db/identity.ts. */
function normalize(text: string): string {
  return text.normalize('NFKC').replace(/\s+/gu, ' ').trim().toLowerCase();
}

function sha256(parts: string[]): string {
  return createHash('sha256').update(JSON.stringify(parts), 'utf8').digest('hex');
}

export function expectedIdentity(title: string, body: string): { material: string; content: string; id: string } {
  const material = sha256([normalize(title), normalize(body)]);
  const content = sha256([REPO_ID, material]);
  return { material, content, id: `m_${content.slice(0, 24)}` };
}

export async function withOpened(
  fn: (db: DatabaseSync, token: string) => void | Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      const token = claimLease(opened.db, { pid: 1, now: NOW });
      if (token === null) assert.fail('expected a lease token');
      await fn(opened.db, token);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

export function seedRepo(db: DatabaseSync): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, 'remote', 'github.com/example/uploader', '/work/uploader', 1, 1)`,
  ).run(REPO_ID);
}

export function seedSession(
  db: DatabaseSync,
  id: string,
  options: { status?: 'active' | 'ended'; summaryState?: string | null; turns?: number } = {},
): void {
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, started_at, ended_at, status, turn_count, summary_state)
     VALUES (?, ?, 'claude', ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    id,
    REPO_ID,
    `native-${id}`,
    id,
    NOW - DAY,
    options.status === 'ended' ? NOW - 1000 : null,
    options.status ?? 'active',
    options.turns ?? 1,
    options.summaryState ?? null,
  );
  for (let ordinal = 1; ordinal <= (options.turns ?? 1); ordinal += 1) {
    db.prepare(
      'INSERT INTO turns (id, session_id, ordinal, started_at, ended_at) VALUES (?, ?, ?, ?, ?)',
    ).run(
      `${id}-t${ordinal}`,
      id,
      ordinal,
      NOW - DAY + ordinal,
      ordinal === (options.turns ?? 1) && options.status === 'ended' ? null : NOW - DAY + ordinal + 1,
    );
  }
}

let capturedCounter = 0;

export function seedEvent(
  db: DatabaseSync,
  seed: {
    id: string;
    sessionId?: string;
    kind?: string;
    content?: string | null;
    sensitivity?: string;
    payload?: unknown;
    turn?: number;
    state?: string;
  },
): void {
  capturedCounter += 1;
  const sessionId = seed.sessionId ?? 'sess1';
  db.prepare(
    `INSERT INTO raw_events
       (id, repo_id, session_id, turn_id, agent, kind, content, payload_json, sensitivity,
        classification_state, captured_at, expires_at)
     VALUES (?, ?, ?, ?, 'claude', ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    seed.id,
    REPO_ID,
    sessionId,
    `${sessionId}-t${seed.turn ?? 1}`,
    seed.kind ?? 'prompt',
    seed.content === undefined ? `text of ${seed.id}` : seed.content,
    seed.payload === undefined ? null : JSON.stringify(seed.payload),
    seed.sensitivity ?? 'eligible',
    seed.state ?? 'done',
    NOW - DAY + capturedCounter,
    NOW + 7 * DAY,
  );
}

export function seedBatch(
  db: DatabaseSync,
  id: string,
  options: { state?: string; destination?: string; degraded?: string | null; sessionId?: string } = {},
): void {
  db.prepare(
    `INSERT INTO observation_batches
       (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token,
        provider_attempts, degraded_reason, claimed_at)
     VALUES (?, ?, ?, ?, ?, 'session_end', ?, 'worker', 1, ?, ?)`,
  ).run(
    id,
    REPO_ID,
    options.sessionId ?? 'sess1',
    `through-${id}`,
    options.destination ?? 'remote_observer',
    options.state ?? 'running',
    options.degraded ?? null,
    NOW - 1000,
  );
}

export function seedMemory(
  db: DatabaseSync,
  seed: {
    id: string;
    title: string;
    body: string;
    sensitivity?: string;
    deleted?: boolean;
    supersededBy?: string;
    contentHash?: string;
  },
): void {
  db.prepare(
    `INSERT INTO memories
       (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash,
        sensitivity, review_state, valid_from, valid_to, superseded_by, deleted_at, created_at)
     VALUES (?, ?, 'discovery', ?, ?, '[]', ?, ?, ?, ?, 'unreviewed', ?, ?, ?, ?, ?)`,
  ).run(
    seed.id,
    REPO_ID,
    seed.title,
    seed.body,
    cjkBigrams(`${seed.title} ${seed.body}`),
    sha256([normalize(seed.title), normalize(seed.body)]),
    seed.contentHash ?? expectedIdentity(seed.title, seed.body).content,
    seed.sensitivity ?? 'eligible',
    NOW - DAY,
    seed.supersededBy === undefined ? null : NOW - 100,
    seed.supersededBy ?? null,
    seed.deleted === true ? NOW - DAY : null,
    NOW - DAY,
  );
}

export function observation(overrides: Partial<Observation> = {}): Observation {
  return {
    type: 'discovery',
    title: 'The uploader retries three times',
    body: 'The uploader retries three times before it gives up.',
    concepts: ['gotcha'],
    citations: { files_read: [], files_modified: [], commits: [] },
    source_event_ids: ['p1'],
    classification: { decision: 'add', target: null, reason: 'rule:test' },
    ...overrides,
  };
}

export function output(...observations: Observation[]): ObserverOutput {
  return observerOutputSchema.parse({ observations });
}

export function memoryRow(db: DatabaseSync, id: string): Record<string, unknown> | undefined {
  return db.prepare('SELECT * FROM memories WHERE id = ?').get(id) as Record<string, unknown> | undefined;
}
