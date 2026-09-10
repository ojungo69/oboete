import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

/** Native IDs belong to an agent within a repository, including imported older session rows. */
export function findNativeSession(db: DatabaseSync, repoId: string, agent: string, nativeId: string) {
  return db.prepare(`SELECT * FROM sessions WHERE repo_id = ? AND agent = ?
    AND COALESCE(original_native_session_id, native_session_id) = ?`).get(repoId, agent, nativeId);
}

/** Called after scoped lookup, within the caller's immediate transaction. Preserve old IDs. */
export function nativeSessionStorage(db: DatabaseSync, agent: string, nativeId: string) {
  return db.prepare('SELECT 1 FROM sessions WHERE agent = ? AND native_session_id = ?').get(agent, nativeId) === undefined
    ? { stored: nativeId, original: null }
    : { stored: randomUUID(), original: nativeId };
}

export function markSessionCaptured(db: DatabaseSync, sessionId: string, capturedAt: number): void {
  db.prepare('UPDATE sessions SET last_captured_at = MAX(COALESCE(last_captured_at, ?), ?) WHERE id = ?')
    .run(capturedAt, capturedAt, sessionId);
}
