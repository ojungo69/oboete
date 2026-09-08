import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { isBusyError } from '../db/open.js';
import { contentHash } from '../events.js';
import { stripRecognizedPacks } from '../injection/recognize.js';
import type { OboetePaths } from '../paths.js';
import {
  listSpool,
  quarantineSpoolEntry,
  readSpoolEntry,
  removeSpoolEntry,
  type SpoolEntry,
} from '../spool.js';
import { payloadOf } from './batches.js';
import { assertLease, transactionImmediate } from './lease.js';

// ---------------------------------------------------------------------------
// Spool recovery (FR-003, R6)
// ---------------------------------------------------------------------------

/**
 * The parent rows of one spool entry. The hook could not read the database, so its `sessions.id`
 * is a fresh uuid; the row that already exists for (agent, native_session_id) is the parent the
 * foreign key needs (0001_core.sql UNIQUE), and the same holds for (session, ordinal) on `turns`.
 * Returns the ids the recovered `raw_events` row must carry.
 */
function ensureSessionRows(
  db: DatabaseSync,
  entry: SpoolEntry,
): { sessionId: string; turnId: string | null } {
  db.prepare(
    `INSERT OR IGNORE INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, ?, ?, ?, ?, ?)`,
  ).run(
    entry.repo.id,
    entry.repo.identity_kind,
    entry.repo.normalized_identity,
    entry.repo.display_root,
    entry.row.captured_at,
    entry.row.captured_at,
  );
  db.prepare(
    `INSERT OR IGNORE INTO sessions
       (id, repo_id, agent, native_session_id, conversation_id, model, started_at, status, turn_count)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0)`,
  ).run(
    entry.session.id,
    entry.session.repo_id,
    entry.session.agent,
    entry.session.native_session_id,
    entry.session.conversation_id,
    entry.session.model,
    entry.session.started_at,
    entry.session.status,
  );
  const sessionId = String(
    db
      .prepare('SELECT id FROM sessions WHERE agent = ? AND native_session_id = ?')
      .get(entry.session.agent, entry.session.native_session_id)?.id,
  );

  const turnId = placeRecoveredInTurn(db, sessionId, entry);
  if (entry.row.kind === 'session_end') {
    // Without this the recovered session would never reach the session-end trigger (FR-010).
    db.prepare(
      `UPDATE sessions SET status = 'ended', ended_at = COALESCE(ended_at, ?),
         summary_state = COALESCE(summary_state, 'pending') WHERE id = ?`,
    ).run(entry.row.captured_at, sessionId);
  }
  return { sessionId, turnId };
}

/**
 * The turn a recovered row belongs to, derived here the way capture's `placeInTurn` derives it on
 * the direct path: the hook could not read the database, so the spool entry carries no turn at all.
 * A recovered prompt opens the next turn and moves `turn_count` (the ten-turn trigger of FR-010 and
 * every later event of the session read it); any other kind attaches to the open turn, and a
 * recovered `turn_end` closes it.
 */
function placeRecoveredInTurn(db: DatabaseSync, sessionId: string, entry: SpoolEntry): string | null {
  const turnCount = Number(
    db.prepare('SELECT turn_count FROM sessions WHERE id = ?').get(sessionId)?.turn_count ?? 0,
  );
  if (entry.row.kind === 'prompt') {
    const ordinal = turnCount + 1;
    const id = randomUUID();
    db.prepare('INSERT INTO turns (id, session_id, ordinal, started_at) VALUES (?, ?, ?, ?)').run(
      id,
      sessionId,
      ordinal,
      entry.row.captured_at,
    );
    db.prepare('UPDATE sessions SET turn_count = ? WHERE id = ?').run(ordinal, sessionId);
    return id;
  }
  if (turnCount === 0) return null;
  const open = db
    .prepare('SELECT id FROM turns WHERE session_id = ? AND ordinal = ?')
    .get(sessionId, turnCount);
  const turnId = open === undefined ? null : String(open.id);
  if (entry.row.kind === 'turn_end' && turnId !== null) {
    db.prepare('UPDATE turns SET ended_at = COALESCE(ended_at, ?) WHERE id = ?').run(
      entry.row.captured_at,
      turnId,
    );
  }
  return turnId;
}

/** FR-021 on the spool path: the hook had no database to recognize the pack with, so the worker does it here. */
function recognizePacksInEntry(db: DatabaseSync, entry: SpoolEntry): void {
  const payload = payloadOf(entry.row);
  if (entry.row.content === null || payload === null) return;
  const recognized = stripRecognizedPacks(db, entry.row.content);
  if (recognized.hashes.length === 0) return;
  entry.row.content = recognized.text;
  entry.row.content_hash = contentHash(recognized.text);
  entry.row.payload_json = JSON.stringify({ ...payload, recognized_packs: recognized.hashes });
}

/**
 * Recovers every spool file in name order before the next summarization pass (FR-003). The
 * deterministic `raw_events.id` makes `INSERT OR IGNORE` the whole idempotency mechanism, so a
 * file that was already recovered is simply deleted again. A file that does not parse is moved to
 * `spool/failed/` instead of being read into the database.
 */
export function recoverSpool(
  db: DatabaseSync,
  paths: OboetePaths,
  token: string,
  now: number,
): { inserted: number; skipped: number; failed: number } {
  let inserted = 0;
  let skipped = 0;
  let failed = 0;

  for (const name of listSpool(paths)) {
    const entry = readSpoolEntry(paths, name);
    if (entry === null) {
      quarantineSpoolEntry(paths, name);
      failed += 1;
      continue;
    }

    let outcome: 'lease_lost' | 'inserted' | 'skipped';
    try {
      outcome = transactionImmediate(db, () => {
        if (!assertLease(db, token, now)) {
          db.exec('ROLLBACK');
          return 'lease_lost';
        }
        // Recovery is idempotent (FR-003), and the parent rows carry side effects a second pass must
        // not repeat: a row that is already stored ends here, before any turn is opened.
        if (db.prepare('SELECT 1 AS present FROM raw_events WHERE id = ?').get(entry.row.id) !== undefined) {
          return 'skipped';
        }
        const parents = ensureSessionRows(db, entry);
        recognizePacksInEntry(db, entry);
        const changes = Number(
          db
            .prepare(
              `INSERT OR IGNORE INTO raw_events
                 (id, repo_id, session_id, turn_id, agent, kind, content, truncated, payload_json,
                  content_hash, sensitivity, classification_state, captured_at, expires_at, batch_id, via_spool)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 1)`,
            )
            .run(
              entry.row.id,
              entry.row.repo_id,
              parents.sessionId,
              parents.turnId,
              entry.row.agent,
              entry.row.kind,
              entry.row.content,
              entry.row.truncated,
              entry.row.payload_json,
              entry.row.content_hash,
              entry.row.sensitivity,
              entry.row.classification_state,
              entry.row.captured_at,
              entry.row.expires_at,
            ).changes,
        );
        return changes === 0 ? 'skipped' : 'inserted';
      });
    } catch (error) {
      // A busy database is the caller's retry (R6); anything else means the database refused this
      // entry (a foreign key it cannot satisfy, a value the schema rejects), so the file is moved
      // aside rather than replayed on every run (FR-003).
      if (isBusyError(error)) throw error;
      quarantineSpoolEntry(paths, name);
      failed += 1;
      continue;
    }

    if (outcome === 'lease_lost') return { inserted, skipped, failed };
    if (outcome === 'inserted') inserted += 1;
    else skipped += 1;
    // The file goes only after its row is committed, so a crash repeats the recovery instead of
    // losing the event.
    removeSpoolEntry(paths, name);
  }

  return { inserted, skipped, failed };
}
