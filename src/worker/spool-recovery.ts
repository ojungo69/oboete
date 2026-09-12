import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { isBusyError } from '../db/open.js';
import { findNativeSession, markSessionCaptured, nativeSessionStorage } from '../db/sessions.js';
import { contentHash, repositoryEventId } from '../events.js';
import { stripRecognizedPacks } from '../injection/recognize.js';
import { bindCapturedWork } from '../work.js';
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
): { sessionId: string; late: boolean } {
  let session = findNativeSession(db, entry.row.repo_id, entry.session.agent, entry.session.native_session_id);
  if (session === undefined) {
    const native = nativeSessionStorage(db, entry.session.agent, entry.session.native_session_id);
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
       (id, repo_id, agent, native_session_id, original_native_session_id, conversation_id, model, started_at, status, turn_count)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0)`,
  ).run(
    entry.session.id,
    entry.session.repo_id,
    entry.session.agent,
    native.stored,
    native.original,
    entry.session.conversation_id,
    entry.session.model,
    entry.session.started_at,
    entry.session.status,
  );
    session = findNativeSession(db, entry.row.repo_id, entry.session.agent, entry.session.native_session_id)!;
  }
  return { sessionId: String(session.id), late: typeof session.last_captured_at === 'number'
    && entry.row.captured_at <= session.last_captured_at };
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
        const payload = payloadOf(entry.row);
        const legacyId = typeof payload?.legacy_event_id === 'string' ? payload.legacy_event_id : entry.row.id;
        const scopedId = repositoryEventId(entry.row.repo_id, legacyId);
        if (db.prepare('SELECT 1 FROM raw_events WHERE repo_id = ? AND id IN (?, ?, ?)')
          .get(entry.row.repo_id, entry.row.id, legacyId, scopedId) !== undefined) {
          return 'skipped';
        }
        if (payload?.legacy_event_id !== undefined || db.prepare('SELECT 1 FROM raw_events WHERE id = ?').get(entry.row.id) !== undefined) {
          entry.row.id = scopedId;
        }
        const parents = ensureSessionRows(db, entry);
        recognizePacksInEntry(db, entry);
        // Older spool formats carry no context identity and remain historical/unbound.
        const work = (typeof payload?.work_context_key === 'string' || payload?.work_context_key === null) && typeof payload.capture_root === 'string'
          ? bindCapturedWork(db, {
            repoId: entry.row.repo_id, root: payload.capture_root, contextKey: payload.work_context_key,
            sessionId: parents.sessionId, sourceId: entry.row.id, kind: entry.row.kind,
            content: entry.row.content, inputSource: payload.input_source, sensitivity: entry.row.sensitivity,
            admissible: entry.row.classification_state === 'done', capturedAt: entry.row.captured_at,
            recovered: true, late: parents.late,
            repoSecretPaths: payload.source_repo_rules,
          }) : null;
        let turnId: string | null = null;
        if (parents.late) {
          // A late event may refer to an existing historical turn, but cannot open/close the live one.
          if (typeof payload?.prompt_id === 'string') {
            const turn = db.prepare(`SELECT turn_id FROM raw_events WHERE session_id = ? AND repo_id = ?
              AND work_binding_id IS ? AND turn_id IS NOT NULL AND CASE WHEN json_valid(payload_json)
                THEN json_extract(payload_json, '$.prompt_id') = ? ELSE 0 END LIMIT 1`)
              .get(parents.sessionId, entry.row.repo_id, work?.bindingId ?? null, payload.prompt_id);
            if (typeof turn?.turn_id === 'string') turnId = turn.turn_id;
          }
        } else {
          db.prepare("UPDATE sessions SET status = 'active', ended_at = NULL, summary_state = NULL WHERE id = ? AND status = 'ended'")
            .run(parents.sessionId);
          turnId = placeRecoveredInTurn(db, parents.sessionId, entry);
          if (entry.row.kind === 'session_end') db.prepare(`UPDATE sessions SET status = 'ended', ended_at = ?,
            summary_state = COALESCE(summary_state, 'pending') WHERE id = ?`).run(entry.row.captured_at, parents.sessionId);
        }
        const changes = Number(
          db
            .prepare(
              `INSERT OR IGNORE INTO raw_events
                 (id, repo_id, session_id, turn_id, agent, kind, content, truncated, payload_json,
                  content_hash, sensitivity, classification_state, captured_at, expires_at, batch_id, via_spool, work_binding_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 1, ?)`,
            )
            .run(
              entry.row.id,
              entry.row.repo_id,
              parents.sessionId,
              turnId,
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
              work?.bindingId ?? null,
            ).changes,
        );
        markSessionCaptured(db, parents.sessionId, entry.row.captured_at);
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
