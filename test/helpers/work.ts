import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';

/** A resolved fixture purpose for tests that seed SQL directly instead of capturing agent events. */
export function seedWorkBinding(db: DatabaseSync, sessionId: string): string {
  const existing = db.prepare('SELECT id FROM work_bindings WHERE session_id = ? AND closed_at IS NULL').get(sessionId);
  if (typeof existing?.id === 'string') return existing.id;
  const session = db.prepare('SELECT repo_id, started_at FROM sessions WHERE id = ?').get(sessionId);
  assert.ok(session);
  const repoId = String(session.repo_id);
  const contextId = `fixture-context:${repoId}`;
  const workId = `fixture-work:${repoId}`;
  const bindingId = `fixture-binding:${sessionId}`;
  const at = Number(session.started_at ?? 1);
  db.prepare(`INSERT OR IGNORE INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at)
    VALUES (?, ?, 'fixture', '/fixture', ?, ?)`).run(contextId, repoId, at, at);
  db.prepare(`INSERT OR IGNORE INTO work_items (id, repo_id, origin_context_id, purpose, created_at, updated_at)
    VALUES (?, ?, ?, 'Fixture work', ?, ?)`).run(workId, repoId, contextId, at, at);
  db.prepare(`INSERT INTO work_bindings (id, session_id, context_id, work_id, created_at, reason)
    VALUES (?, ?, ?, ?, ?, 'only_active')`).run(bindingId, sessionId, contextId, workId, at);
  return bindingId;
}
