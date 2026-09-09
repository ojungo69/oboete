import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import type { DatabaseSync } from 'node:sqlite';

import { openDatabase } from '../../src/db/open.js';
import { memoryScope } from '../../src/db/queries.js';
import type { AgentName } from '../../src/events.js';
import { ensureDirectories, oboetePaths, type OboetePaths } from '../../src/paths.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { resolveRepoIdentity, type RepoIdentity } from '../../src/repo-identity.js';
import { withTempHome } from './home.js';

export const NOW = 1_800_000_000_000;

export type Fixture = {
  db: DatabaseSync;
  paths: OboetePaths;
  repo: string;
  identity: RepoIdentity;
};

export const scope = (fixture: Fixture) =>
  memoryScope(fixture.db, { repoId: fixture.identity.id, destination: 'injection' });

export async function withFixture(run: (fixture: Fixture) => Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const repo = join(home, 'workspace');
    mkdirSync(repo, { recursive: true });
    spawnSync('git', ['-C', repo, 'init', '--quiet']);
    const identity = resolveRepoIdentity(repo);
    const { db } = openDatabase({ path: paths.db, timeoutMs: 2_000 });
    db.prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
       VALUES (?, ?, ?, ?, ?, ?)`,
    ).run(identity.id, identity.identityKind, identity.normalizedIdentity, identity.root, NOW, NOW);
    try {
      await run({ db, paths, repo, identity });
    } finally {
      db.close();
    }
  });
}

export function insertSession(
  fixture: Fixture,
  input: {
    id: string;
    agent: AgentName;
    nativeId?: string;
    conversationId?: string;
    status?: 'active' | 'ended';
    endedAt?: number;
    summaryState?: 'pending' | 'done' | 'no_content';
    summaryId?: string;
    epoch?: number;
    model?: string;
  },
): void {
  fixture.db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
       started_at, ended_at, status, turn_count, latest_summary_memory_id, context_epoch, summary_state)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?)`,
  ).run(
    input.id,
    fixture.identity.id,
    input.agent,
    input.nativeId ?? `native-${input.id}`,
    input.conversationId ?? input.id,
    input.model ?? null,
    NOW - 10_000,
    input.endedAt ?? null,
    input.status ?? 'active',
    input.summaryId ?? null,
    input.epoch ?? 0,
    input.summaryState ?? null,
  );
}

export function insertMemory(
  fixture: Fixture,
  input: { id: string; title: string; body: string; type?: string; pinned?: boolean },
): void {
  fixture.db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, pinned_at, pin_order, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'eligible', 'unreviewed', ?, ?, ?)`,
  ).run(
    input.id,
    fixture.identity.id,
    input.type ?? 'discovery',
    input.title,
    input.body,
    cjkBigrams(`${input.title} ${input.body}`),
    `material-${input.id}`,
    `content-${input.id}`,
    input.pinned ? NOW - 1_000 : null,
    input.pinned ? 1 : null,
    NOW - 5_000,
  );
}

export function seedSummary(fixture: Fixture): void {
  insertMemory(fixture, {
    id: 'm-summary',
    type: 'session_summary',
    title: 'Previous session',
    body: 'The previous database migration completed.',
  });
  insertSession(fixture, {
    id: 's-previous',
    agent: 'claude',
    status: 'ended',
    endedAt: NOW - 2_000,
    summaryState: 'done',
    summaryId: 'm-summary',
  });
}

export async function stdoutOf(run: () => Promise<number>): Promise<string> {
  const chunks: string[] = [];
  const original = process.stdout.write.bind(process.stdout);
  process.stdout.write = ((chunk: string) => {
    chunks.push(String(chunk));
    return true;
  }) as typeof process.stdout.write;
  try {
    assert.equal(await run(), 0);
  } finally {
    process.stdout.write = original;
  }
  return chunks.join('');
}
