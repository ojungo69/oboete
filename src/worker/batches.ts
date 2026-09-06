// Spool recovery, worker-side classification and the batch split by destination (T031).
// Sources: contracts/observer.md ("Batch composition and the outbound boundary"), research.md R6
// and R10, data-model.md (raw_events, observation_batches), spec FR-003, FR-010, FR-017, FR-020,
// amendments A7 and A11. Security-owned (plan.md "Structure Decision"): this module decides which
// rows may be handed to which summarizer, so every branch names the rule it applies.
import { randomUUID } from 'node:crypto';
import type { DatabaseSync, SQLOutputValue } from 'node:sqlite';

import type { ProviderPreset } from '../config.js';
import { isBusyError } from '../db/open.js';
import { contentHash, toolInputSchema, type ToolInput } from '../events.js';
import { stripRecognizedPacks } from '../injection/recognize.js';
import type { OboetePaths } from '../paths.js';
import { promoteSensitivity, strictest } from '../privacy/classify.js';
import type { DetectorResult } from '../privacy/detect.js';
import {
  isAllowed,
  loadDestinationRules,
  type DestinationRules,
  type Sensitivity,
} from '../privacy/egress.js';
import {
  listSpool,
  quarantineSpoolEntry,
  readSpoolEntry,
  removeSpoolEntry,
  type SpoolEntry,
} from '../spool.js';
import { assertLease, transactionImmediate } from './lease.js';

/** R6: a `running` batch of a worker that died is reclaimed only after this long. */
export const RECLAIM_AFTER_MS = 120_000;
/** R6: a row this close to `expires_at` is batched now so purge cannot delete it unread. */
export const RETENTION_HORIZON_MS = 24 * 60 * 60 * 1000;
/** FR-010: a batch every ten turns during a session. */
export const TEN_TURNS = 10;
/** One classification run stays bounded; the loop continues until the queue is empty. */
const CLASSIFY_LIMIT = 500;

export type BatchDestination = 'remote_observer' | 'local_observer' | 'fallback';
export type BatchTrigger = 'ten_turns' | 'session_end' | 'retention';
export type BatchState = 'pending' | 'running' | 'applied' | 'fallback';
export type ClassificationState = 'pending' | 'done' | 'partial' | 'failed';

export type RawEventRow = {
  id: string;
  repo_id: string;
  session_id: string;
  turn_id: string | null;
  agent: string | null;
  kind: string;
  content: string | null;
  truncated: number | null;
  payload_json: string | null;
  content_hash: string | null;
  sensitivity: Sensitivity;
  classification_state: ClassificationState | null;
  captured_at: number | null;
  expires_at: number | null;
  batch_id: string | null;
  via_spool: number | null;
};

export type BatchRow = {
  id: string;
  repo_id: string | null;
  session_id: string;
  through_event_id: string;
  destination: BatchDestination;
  trigger: BatchTrigger;
  state: BatchState;
  owner_token: string | null;
  claimed_at: number | null;
};

export type SessionRow = {
  id: string;
  repo_id: string;
  agent: string;
  started_at: number | null;
  ended_at: number | null;
  status: 'active' | 'ended';
  turn_count: number;
  summary_state: 'pending' | 'done' | 'no_content' | null;
};

export type TurnRow = {
  id: string;
  ordinal: number;
  started_at: number | null;
  ended_at: number | null;
};

export type BatchInput = {
  batch: BatchRow;
  rows: RawEventRow[];
  session: SessionRow;
  turns: TurnRow[];
};

// data-model.md sessions.summary_state and events.ts isSummarizable: only these kinds carry content
// a summarizer can use; the lifecycle kinds never count.
const SUMMARIZABLE_KINDS: ReadonlySet<string> = new Set([
  'prompt',
  'tool_call',
  'tool_result',
  'tool_failure',
  'last_assistant_message',
  'compaction_summary',
]);

/** The same kinds where the rule has to be applied in SQL: purge and the worker's queue check. */
export const SUMMARIZABLE_KINDS_SQL = [...SUMMARIZABLE_KINDS].map((kind) => `'${kind}'`).join(', ');
/**
 * The characters `String.prototype.trim()` removes (ECMA-262 WhiteSpace and LineTerminator);
 * SQLite's own `TRIM` strips the ASCII space only. The SQL that has to answer "has content" the
 * way `isSummarizableRow` does — purge's delete predicate and the worker's queue check — trims
 * exactly this set: a row only one of the two calls blank is never batched, never deleted and
 * keeps the worker awake for its whole run.
 */
export const BLANK_CODE_POINTS: readonly number[] = [
  0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005,
  0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000, 0xfeff,
];
export const BLANK_CHARACTERS_SQL = `char(${BLANK_CODE_POINTS.join(', ')})`;

const DESTINATION_ORDER: readonly BatchDestination[] = [
  'remote_observer',
  'local_observer',
  'fallback',
];

function asRawEventRows(rows: Record<string, SQLOutputValue>[]): RawEventRow[] {
  // The columns are the ones 0001_core.sql defines; SQLite gives them back untyped.
  return rows as unknown as RawEventRow[];
}

/** `payload_json` as the normalized-fields object capture wrote, or null when it is unreadable. */
export function payloadOf(row: { payload_json: string | null }): Record<string, unknown> | null {
  if (row.payload_json === null) return null;
  try {
    const parsed: unknown = JSON.parse(row.payload_json);
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return null;
    return parsed as Record<string, unknown>;
  } catch {
    return null;
  }
}

/** The normalized tool input's paths, one of the payload fields a tool call carries outside `content`. */
export function toolPaths(row: RawEventRow): string[] {
  const input = payloadOf(row)?.input;
  if (typeof input !== 'object' || input === null) return [];
  const paths = (input as { paths?: unknown }).paths;
  return Array.isArray(paths) ? paths.filter((path): path is string => typeof path === 'string') : [];
}

/**
 * The free text of a stored row's normalized tool input. It travels in the outbound request beside
 * `content` (contracts/observer.md "Input"), so the promotion gate has to scan it as well, and
 * events.ts `isSummarizable` counts it as content.
 */
export function toolInputText(row: RawEventRow): string {
  const input = payloadOf(row)?.input;
  if (typeof input !== 'object' || input === null) return '';
  const { command, text } = input as { command?: unknown; text?: unknown };
  return [command, text].filter((value): value is string => typeof value === 'string').join('\n');
}

/**
 * The normalized tool input of a stored tool call with its text put back. Capture moves a shell
 * tool's `command` and any other tool's `text` out of `payload_json` into `raw_events.content`
 * (src/capture.ts payloadJson, data-model.md raw_events), so a reader that goes straight to
 * `payload_json` sees paths and line counts where the call had a command. Every reader of a stored
 * tool call goes through this one function (contracts/observer.md "Input", FR-015).
 */
export function toolInputOf(row: {
  content: string | null;
  payload_json: string | null;
  classification_state?: ClassificationState | null;
}): ToolInput {
  const payload = payloadOf(row);
  const parsed = toolInputSchema.safeParse(payload?.input);
  const input: ToolInput = parsed.success ? parsed.data : { paths: [] };
  const stored = row.content ?? '';
  // A7: a partial row hands over its paths and never the text it holds.
  if (stored === '' || row.classification_state === 'partial') return input;
  if (input.command !== undefined || input.text !== undefined) return input;
  return payload?.tool_name === 'bash' ? { ...input, command: stored } : { ...input, text: stored };
}

/** events.ts `isSummarizable` over a stored row: the same kinds and the same "has content" rule. */
export function isSummarizableRow(row: RawEventRow): boolean {
  if (!SUMMARIZABLE_KINDS.has(row.kind)) return false;
  // data-model.md raw_events: a failed classification is metadata only, never summarized.
  if (row.classification_state === 'failed') return false;
  // contracts/observer.md: `secret` rows are never summarized, so they never enter a batch.
  if (row.sensitivity === 'secret') return false;
  if ((row.content ?? '').trim() !== '') return true;
  // A tool call whose only content is its command, its text or its paths still describes work
  // (events.ts isSummarizable joins exactly those three fields).
  return `${toolInputText(row)}${toolPaths(row).join('')}`.trim() !== '';
}

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

// ---------------------------------------------------------------------------
// Worker-side classification (FR-017, A7)
// ---------------------------------------------------------------------------

const CLASSIFY_CANDIDATES = `SELECT * FROM raw_events
  WHERE batch_id IS NULL AND sensitivity = 'local_only'
    AND classification_state IN ('pending', 'done')
    AND TRIM(COALESCE(content, '')) <> ''
  ORDER BY captured_at, id LIMIT ?`;

/**
 * The worker's promotion pass (FR-017): a row stays `local_only` until a complete, clean detector
 * run promotes it. `private` and `secret` rows are not selected at all, and `partial` and `failed`
 * rows are excluded because A7 forbids promoting them.
 */
export async function classifyPending(
  db: DatabaseSync,
  token: string,
  now: number,
  detect: (text: string) => Promise<DetectorResult>,
): Promise<{ examined: number; promoted: number; secret: number; failed: number; leaseLost: boolean }> {
  let examined = 0;
  let promoted = 0;
  let secret = 0;
  let failed = 0;
  // A row the detector could not read keeps its state, so it would be selected again forever;
  // this set is what ends the loop instead.
  const seen = new Set<string>();

  for (;;) {
    const rows = asRawEventRows(db.prepare(CLASSIFY_CANDIDATES).all(CLASSIFY_LIMIT)).filter(
      (row) => !seen.has(row.id),
    );
    if (rows.length === 0) break;

    // The detector is async and may run in a worker thread, so it never runs inside a transaction.
    const updates: { id: string; sensitivity: Sensitivity; content: string | null }[] = [];
    for (const row of rows) {
      seen.add(row.id);
      const content = row.content ?? '';
      const result = await detect(content);
      // FR-017: the outbound request carries the normalized tool input as well as `content`, so a
      // secret found only there has to keep the row off the remote path.
      const inputText = toolInputText(row);
      const inputResult = inputText === '' ? result : await detect(inputText);
      if (!result.ok || !inputResult.ok) {
        // R4 fails closed on promotion, and the row keeps the redacted content capture stored, so
        // a transient detector failure leaves it local_only for the next run rather than marking
        // it `failed` and dropping content that was already classified once.
        failed += 1;
        continue;
      }
      updates.push({
        id: row.id,
        sensitivity: strictest(
          promoteSensitivity(row.sensitivity, result, 'done'),
          promoteSensitivity(row.sensitivity, inputResult, 'done'),
        ),
        // FR-018: what this second run found is redacted in the stored row as well. `payload_json`
        // is capture's normalized output and is not rewritten here; the sensitivity above is what
        // keeps an unredacted tool input from travelling.
        content: result.text === content ? null : result.text,
      });
    }

    const lost = transactionImmediate(db, () => {
      if (!assertLease(db, token, now)) {
        db.exec('ROLLBACK');
        return true;
      }
      const update = db.prepare(
        `UPDATE raw_events SET sensitivity = ?, classification_state = 'done', content = COALESCE(?, content)
         WHERE id = ?`,
      );
      for (const row of updates) update.run(row.sensitivity, row.content, row.id);
      return false;
    });
    if (lost) return { examined, promoted, secret, failed, leaseLost: true };

    for (const row of updates) {
      examined += 1;
      if (row.sensitivity === 'secret') secret += 1;
      else if (row.sensitivity === 'eligible') promoted += 1;
    }
    if (rows.length < CLASSIFY_LIMIT) break;
  }

  return { examined, promoted, secret, failed, leaseLost: false };
}

// ---------------------------------------------------------------------------
// Batch creation (R10 destination split, R6 triggers)
// ---------------------------------------------------------------------------

/**
 * The destination of one row, decided only by the seeded rule table (FR-020). `null` means no
 * destination may have it, which for a summarizable row is only ever `secret`.
 */
function destinationFor(
  rules: DestinationRules,
  preset: ProviderPreset['egress'],
  row: RawEventRow,
  now: number,
): BatchDestination | null {
  // The fallback runs on this machine, so it may carry whatever a local summarizer may carry.
  const localClasses = isAllowed(rules, 'local_observer', row.sensitivity, true);
  // A7: a partial row contributes metadata to the rule-based fallback and never text to a provider.
  if (row.classification_state === 'partial') return localClasses ? 'fallback' : null;
  // R6 retention: a row already past `expires_at` is forced into a fallback batch, because waiting
  // for a provider that may be exhausted or unreachable would lose it to purge unread.
  if (row.expires_at !== null && row.expires_at <= now) return localClasses ? 'fallback' : null;
  if (preset === 'remote' && isAllowed(rules, 'remote_observer', row.sensitivity, true)) {
    return 'remote_observer';
  }
  if (preset === 'local' && isAllowed(rules, 'local_observer', row.sensitivity, true)) {
    return 'local_observer';
  }
  return localClasses ? 'fallback' : null;
}

function triggerFor(session: SessionRow, rows: RawEventRow[], now: number): BatchTrigger | null {
  // contracts/observer.md: session end is the definitive batch of a session, so it wins.
  if (session.status === 'ended') return 'session_end';
  // R6: a row past `expires_at` in a pending batch is forced out before purge deletes it.
  if (rows.some((row) => row.expires_at !== null && row.expires_at <= now + RETENTION_HORIZON_MS)) {
    return 'retention';
  }
  // FR-010 "after every 10 turns": the turns that still carry unbatched rows are the ones this
  // batch would summarize, which needs no stored marker and cannot fire twice for the same turns.
  const turns = new Set(rows.map((row) => row.turn_id).filter((id): id is string => id !== null));
  return turns.size >= TEN_TURNS ? 'ten_turns' : null;
}

function readSession(db: DatabaseSync, sessionId: string): SessionRow | null {
  const row = db
    .prepare(
      `SELECT id, repo_id, agent, started_at, ended_at, status, turn_count, summary_state
       FROM sessions WHERE id = ?`,
    )
    .get(sessionId);
  return row === undefined ? null : (row as unknown as SessionRow);
}

/**
 * R6 retention: rows past `expires_at` that sit in a batch nobody ran are detached so this run
 * puts them into a fallback batch, which purge may then delete. A provider batch left with no rows
 * would summarize nothing, so it goes with them.
 */
function detachExpiredPendingRows(db: DatabaseSync, now: number): void {
  db.prepare(
    `UPDATE raw_events SET batch_id = NULL WHERE expires_at <= ? AND batch_id IN (
       SELECT id FROM observation_batches WHERE state = 'pending' AND destination <> 'fallback')`,
  ).run(now);
  db.prepare(
    `DELETE FROM observation_batches WHERE state = 'pending' AND destination <> 'fallback'
       AND NOT EXISTS (SELECT 1 FROM raw_events r WHERE r.batch_id = observation_batches.id)`,
  ).run();
}

/**
 * One batch per (session, through event, destination) after classification (R10). Every row of a
 * round goes to exactly one destination, so the remote batch and the fallback batch of the same
 * range are disjoint and no observation is generated twice (contracts/observer.md).
 */
export function createBatches(
  db: DatabaseSync,
  token: string,
  now: number,
  options: { preset: ProviderPreset['egress'] },
): { created: BatchRow[]; leaseLost: boolean } {
  const rules = loadDestinationRules(db);

  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return { created: [], leaseLost: true };
    }

    detachExpiredPendingRows(db, now);

    const created: BatchRow[] = [];
    const sessionIds = db
      .prepare('SELECT DISTINCT session_id FROM raw_events WHERE batch_id IS NULL ORDER BY session_id')
      .all()
      .map((row) => String(row.session_id));

    for (const sessionId of sessionIds) {
      const rows = asRawEventRows(
        db
          .prepare(
            'SELECT * FROM raw_events WHERE session_id = ? AND batch_id IS NULL ORDER BY captured_at, id',
          )
          .all(sessionId),
      ).filter(isSummarizableRow);
      if (rows.length === 0) continue;

      const session = readSession(db, sessionId);
      if (session === null) continue;
      const trigger = triggerFor(session, rows, now);
      if (trigger === null) continue;

      // The newest row of the round; both batches of the round carry it, which is what makes them
      // one range in two destinations (data-model.md UNIQUE (session_id, through_event_id, destination)).
      const throughEventId = rows[rows.length - 1].id;

      const byDestination = new Map<BatchDestination, RawEventRow[]>();
      for (const row of rows) {
        const destination = destinationFor(rules, options.preset, row, now);
        if (destination === null) continue;
        const list = byDestination.get(destination) ?? [];
        list.push(row);
        byDestination.set(destination, list);
      }

      const taken = db.prepare(
        `SELECT 1 AS present FROM observation_batches
         WHERE session_id = ? AND through_event_id = ? AND destination = ?`,
      );

      for (const destination of DESTINATION_ORDER) {
        const list = byDestination.get(destination);
        // A batch is created only if it would carry at least one row.
        if (list === undefined || list.length === 0) continue;

        const id = randomUUID();
        // data-model.md UNIQUE (session_id, through_event_id, destination). An earlier round can
        // already own that key: rows detached from a pending provider batch past `expires_at` are
        // batched again, and the fallback batch of the first round may still be there. Those are
        // different rows, so the second round carries its own key rather than failing the run.
        const through =
          taken.get(sessionId, throughEventId, destination) === undefined
            ? throughEventId
            : `${throughEventId}:${id}`;

        const batch: BatchRow = {
          id,
          repo_id: session.repo_id,
          session_id: sessionId,
          through_event_id: through,
          destination,
          trigger,
          state: 'pending',
          owner_token: token,
          claimed_at: now,
        };
        db.prepare(
          `INSERT INTO observation_batches
             (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token,
              provider_attempts, claimed_at)
           VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, 0, ?)`,
        ).run(
          batch.id,
          batch.repo_id,
          batch.session_id,
          batch.through_event_id,
          batch.destination,
          batch.trigger,
          token,
          now,
        );
        const claim = db.prepare('UPDATE raw_events SET batch_id = ? WHERE id = ? AND batch_id IS NULL');
        for (const row of list) claim.run(batch.id, row.id);
        created.push(batch);
      }
    }

    return { created, leaseLost: false };
  });
}

/**
 * R6 and A11: a `running` batch whose worker died is reclaimed after 120 s, which makes provider
 * attempts at-least-once while the fenced apply keeps applied effects exactly-once.
 */
export function reclaimStale(
  db: DatabaseSync,
  token: string,
  now: number,
): { reclaimed: number; leaseLost: boolean } {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return { reclaimed: 0, leaseLost: true };
    }
    const result = db
      .prepare(
        `UPDATE observation_batches SET state = 'pending', owner_token = ?, claimed_at = ?
         WHERE state = 'running' AND owner_token IS NOT ? AND claimed_at IS NOT NULL AND claimed_at <= ?`,
      )
      .run(token, now, token, now - RECLAIM_AFTER_MS);
    return { reclaimed: Number(result.changes), leaseLost: false };
  });
}

/** A7: a partial row hands over its tool name and paths and none of the text it holds. */
/**
 * A7: a `partial` row hands over metadata only — the tool name and the paths — never its truncated
 * text, to a provider or to anything that is injected later.
 */
export function stripPartial(row: RawEventRow): RawEventRow {
  if (row.classification_state !== 'partial') return row;
  const name = payloadOf(row)?.tool_name;
  const paths = toolPaths(row);
  const metadata =
    typeof name === 'string' ? { tool_name: name, input: { paths } } : { input: { paths } };
  return { ...row, content: null, payload_json: JSON.stringify(metadata) };
}

/** Everything the request builder and the rule-based fallback read for one batch. */
export function loadBatchInput(db: DatabaseSync, batchId: string): BatchInput | null {
  const batchRow = db
    .prepare(
      `SELECT id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, claimed_at
       FROM observation_batches WHERE id = ?`,
    )
    .get(batchId);
  if (batchRow === undefined) return null;
  const batch = batchRow as unknown as BatchRow;

  const session = readSession(db, batch.session_id);
  if (session === null) return null;

  const turns = db
    .prepare(
      'SELECT id, ordinal, started_at, ended_at FROM turns WHERE session_id = ? ORDER BY ordinal',
    )
    .all(batch.session_id) as unknown as TurnRow[];

  const rows = asRawEventRows(
    db.prepare('SELECT * FROM raw_events WHERE batch_id = ? ORDER BY captured_at, id').all(batchId),
  ).map(stripPartial);

  return { batch, rows, session, turns };
}
