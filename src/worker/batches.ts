// Spool recovery, worker-side classification and the batch split by destination (T031).
// Sources: contracts/observer.md ("Batch composition and the outbound boundary"), research.md R6
// and R10, data-model.md (raw_events, observation_batches), spec FR-003, FR-010, FR-017, FR-020,
// amendments A7 and A11. Security-owned (plan.md "Structure Decision"): this module decides which
// rows may be handed to which summarizer, so every branch names the rule it applies.
import { randomUUID } from 'node:crypto';
import type { DatabaseSync, SQLOutputValue } from 'node:sqlite';

import type { ProviderPreset } from '../config.js';
import { toolInputSchema, type ToolInput } from '../events.js';
import { promoteSensitivity, strictest } from '../privacy/classify.js';
import type { DetectorResult } from '../privacy/detect.js';
import {
  isAllowed,
  loadDestinationRules,
  type DestinationRules,
  type Sensitivity,
} from '../privacy/egress.js';
import { assertLease, transactionImmediate } from './lease.js';

/** R6: a `running` batch of a worker that died is reclaimed only after this long. */
export const RECLAIM_AFTER_MS = 120_000;
/** R6: a row this close to `expires_at` is batched now so purge cannot delete it unread. */
export const RETENTION_HORIZON_MS = 24 * 60 * 60 * 1000;
/** Deferred sources wait between bounded worker runs, never in a resident retry loop. */
export const DUE_SOURCE_SQL = `(processing_state = 'pending' OR (processing_state = 'waiting'
  AND retry_after <= ? AND NOT EXISTS (
    SELECT 1 FROM observation_batch_sources prior
    JOIN observation_batches attempt ON attempt.id = prior.batch_id
    WHERE prior.raw_event_id = raw_events.id AND attempt.owner_token = ?
  )))`;
export const RESOLVED_WORK_SQL = `work_binding_id IN (SELECT id FROM work_bindings WHERE work_id IS NOT NULL)`;
/** FR-010: a batch every ten turns during a session. */
export const TEN_TURNS = 10;
const BATCH_SOURCE_LIMIT = 50;
/** Bound payload allocation independently of event count; one accepted source always fits alone. */
const SOURCE_PAGE_BYTES = 2 * 1024 * 1024;

/** The same bounded schedule applies to admission failures and rejected provider output. */
export function sourceRetryAt(now: number, attempts: number): number {
  return now + Math.min(24 * 60 * 60_000, 5 * 60_000 * 2 ** Math.min(attempts, 9));
}

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
  /** Absent only on old-schema/read-only request fixtures. */
  processing_state?: 'pending' | 'waiting' | 'processed' | 'excluded' | 'legacy_unknown';
  processing_offset?: number;
  processing_hash?: string | null;
  processing_attempts?: number;
  has_processing_history?: number;
  advanced_by_owner?: number;
  retry_after?: number | null;
  source_bytes?: number;
  work_binding_id?: string | null;
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
  work_binding_id?: string | null;
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

const STORED_CONTENT_SQL = `TRIM(COALESCE(content, ''), ${BLANK_CHARACTERS_SQL}) <> ''`;
const TOOL_CONTENT_SQL = `(kind = 'tool_call' AND CASE WHEN json_valid(payload_json) THEN (
  (json_type(payload_json, '$.input.command') = 'text' AND TRIM(json_extract(payload_json, '$.input.command'), ${BLANK_CHARACTERS_SQL}) <> '') OR
  (json_type(payload_json, '$.input.text') = 'text' AND TRIM(json_extract(payload_json, '$.input.text'), ${BLANK_CHARACTERS_SQL}) <> '') OR
  (json_type(payload_json, '$.input.paths') = 'array' AND EXISTS (SELECT 1 FROM json_each(payload_json, '$.input.paths')
    WHERE type = 'text' AND TRIM(value, ${BLANK_CHARACTERS_SQL}) <> ''))
) ELSE 0 END)`;
export const SUMMARIZABLE_ROW_SQL = `(kind IN (${SUMMARIZABLE_KINDS_SQL})
  AND classification_state IS NOT 'failed' AND sensitivity <> 'secret'
  AND (${STORED_CONTENT_SQL} OR ${TOOL_CONTENT_SQL}))`;

/** Scheduling needs only metadata and size; source eligibility is checked by the shared SQL. */
export const SOURCE_METADATA_COLUMNS = `id, repo_id, session_id, turn_id, kind, sensitivity, work_binding_id,
  classification_state, captured_at, expires_at, processing_state, processing_offset, retry_after,
  COALESCE(length(CAST(content AS BLOB)), 0) + COALESCE(length(CAST(payload_json AS BLOB)), 0) AS source_bytes`;

function sourcePage(rows: Iterable<RawEventRow>): RawEventRow[] {
  const page: RawEventRow[] = [];
  let bytes = 0;
  for (const row of rows) {
    if (page.length > 0 && bytes + (row.source_bytes ?? 0) > SOURCE_PAGE_BYTES) break;
    page.push(row);
    bytes += row.source_bytes ?? 0;
    if (page.length === BATCH_SOURCE_LIMIT) break;
  }
  return page;
}

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
  return row.kind === 'tool_call' && `${toolInputText(row)}${toolPaths(row).join('')}`.trim() !== '';
}

// ---------------------------------------------------------------------------
// Worker-side classification (FR-017, A7)
// ---------------------------------------------------------------------------

const CLASSIFY_CANDIDATES = `SELECT ${SOURCE_METADATA_COLUMNS} FROM raw_events
  WHERE batch_id IS NULL AND sensitivity = 'local_only'
    AND ${DUE_SOURCE_SQL}
    AND classification_state IN ('pending', 'done')
    AND ${SUMMARIZABLE_ROW_SQL}
  ORDER BY captured_at, id LIMIT ?`;

type ClassificationUpdate = { id: string; sensitivity: Sensitivity; content: string | null };

function classificationUpdate(
  row: RawEventRow,
  content: string,
  result: Extract<DetectorResult, { ok: true }>,
  inputResult: Extract<DetectorResult, { ok: true }>,
): ClassificationUpdate {
  return {
    id: row.id,
    sensitivity: strictest(
      promoteSensitivity(row.sensitivity, result, 'done'),
      promoteSensitivity(row.sensitivity, inputResult, 'done'),
    ),
    // FR-018: what this second run found is redacted in the stored row as well. `payload_json`
    // is capture's normalized output and is not rewritten here; the sensitivity above is what
    // keeps an unredacted tool input from travelling.
    content: result.text === content ? null : result.text,
  };
}

function storeClassificationUpdates(
  db: DatabaseSync,
  token: string,
  now: number,
  updates: ClassificationUpdate[],
): boolean {
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return true;
    }
    const update = db.prepare(
      `UPDATE raw_events SET sensitivity = ?, classification_state = 'done', content = COALESCE(?, content)
         WHERE id = ?`,
    );
    for (const row of updates) {
      update.run(row.sensitivity, row.content, row.id);
      if (row.sensitivity === 'secret') excludeSecretSource(db, row.id, now);
    }
    return false;
  });
}

/** New secret findings close source processing and quarantine derived text before any reader can use it. */
export function excludeSecretSource(db: DatabaseSync, id: string, now: number): void {
  db.prepare(`UPDATE raw_events SET sensitivity = 'secret', content = NULL, payload_json = NULL,
    processing_state = 'excluded', processed_at = ?, retry_after = NULL
    WHERE id = ?`).run(now, id);
  db.prepare(`UPDATE memory_sources SET evidence = NULL, capture_root = NULL, source_paths_json = NULL, citation_value = NULL
    WHERE memory_id IN (SELECT memory_id FROM memory_sources WHERE raw_event_id = ?)`)
    .run(id);
  // Reuse the existing quarantine detector pass to redact derived memories. Setting the strictest
  // sensitivity in this transaction also protects readers while that asynchronous pass runs.
  db.prepare(`UPDATE memories SET sensitivity = 'secret', review_state = 'imported'
    WHERE id IN (SELECT memory_id FROM memory_sources WHERE raw_event_id = ?)
      OR (type = 'session_summary' AND work_id IS NULL AND source_session_id = (SELECT session_id FROM raw_events WHERE id = ?))`)
    .run(id, id);
  db.prepare(`UPDATE sessions SET summary_state = 'pending', summary_updated_at = NULL
    WHERE id = (SELECT session_id FROM raw_events WHERE id = ?)`)
    .run(id);
  db.prepare(`INSERT INTO diagnostics
    (id, kind, severity, message_code, details_json, count, first_seen_at, last_seen_at)
    VALUES (?, 'source_exclusion', 'info', 'secret', ?, 1, ?, ?)
    ON CONFLICT(id) DO UPDATE SET last_seen_at = excluded.last_seen_at`)
    .run(`source-exclusion:${id}`, JSON.stringify({ source_id: id }), now, now);
}

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

  function countClassifications(updates: { sensitivity: Sensitivity }[]): void {
    for (const row of updates) {
      examined += 1;
      if (row.sensitivity === 'secret') secret += 1;
      else if (row.sensitivity === 'eligible') promoted += 1;
    }
  }

  /** Queue a row's promotion only when both its content and tool input pass detection. */
  async function collectSuccessfulClassifications(
    rows: RawEventRow[],
    updates: ClassificationUpdate[],
  ): Promise<void> {
    for (const row of rows) {
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
      updates.push(classificationUpdate(row, content, result, inputResult));
    }
  }

  const page = sourcePage(asRawEventRows(db.prepare(CLASSIFY_CANDIDATES).all(now, token, BATCH_SOURCE_LIMIT)));
  if (page.length > 0) {
    const read = db.prepare('SELECT * FROM raw_events WHERE id = ?');
    const rows = page.map((row) => read.get(row.id) as unknown as RawEventRow);

    // The detector is async and may run in a worker thread, so it never runs inside a transaction.
    const updates: ClassificationUpdate[] = [];
    await collectSuccessfulClassifications(rows, updates);

    const lost = storeClassificationUpdates(db, token, now, updates);
    if (lost) return { examined, promoted, secret, failed, leaseLost: true };

    countClassifications(updates);
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
): BatchDestination | null {
  // The fallback runs on this machine, so it may carry whatever a local summarizer may carry.
  const localClasses = isAllowed(rules, 'local_observer', row.sensitivity, true);
  // A7: a partial row contributes metadata to the rule-based fallback and never text to a provider.
  if (row.classification_state === 'partial') return localClasses ? 'fallback' : null;
  if (preset === 'remote' && isAllowed(rules, 'remote_observer', row.sensitivity, true)) {
    return 'remote_observer';
  }
  if (preset === 'local' && isAllowed(rules, 'local_observer', row.sensitivity, true)) {
    return 'local_observer';
  }
  return localClasses ? 'fallback' : null;
}

function triggerFor(session: SessionRow, rows: Iterable<RawEventRow>, now: number) {
  if (session.status === 'ended') return 'session_end' as const;
  const turns = new Set<string>();
  for (const row of rows) {
    if (row.processing_state === 'waiting' || row.has_processing_history === 1 ||
      (row.expires_at !== null && row.expires_at <= now + RETENTION_HORIZON_MS)) return 'retention' as const;
    if (row.turn_id !== null) turns.add(row.turn_id);
    if (turns.size >= TEN_TURNS) return 'ten_turns' as const;
  }
  return null;
}

/** Keyset pages bound memory even when metadata or a single turn fills many storage pages. */
function* dueSessionRows(db: DatabaseSync, sessionId: string, repoId: string, workBindingId: string | null, now: number, token: string) {
  const read = db.prepare(`WITH due AS (SELECT ${SOURCE_METADATA_COLUMNS}, EXISTS (SELECT 1 FROM observation_batch_sources s
      WHERE s.raw_event_id = raw_events.id) AS has_processing_history,
      EXISTS (SELECT 1 FROM observation_batch_sources s JOIN observation_batches b ON b.id = s.batch_id
        WHERE s.raw_event_id = raw_events.id AND b.owner_token = ? AND s.outcome = 'processed') AS advanced_by_owner
    FROM raw_events WHERE session_id = ? AND repo_id = ? AND work_binding_id IS ?
      AND batch_id IS NULL AND ${DUE_SOURCE_SQL}
      AND ${SUMMARIZABLE_ROW_SQL})
    SELECT * FROM due WHERE (? IS NULL OR (advanced_by_owner, COALESCE(captured_at, 0), id) > (?, ?, ?))
    ORDER BY advanced_by_owner, COALESCE(captured_at, 0), id LIMIT ?`);
  let after: RawEventRow | undefined;
  for (;;) {
    const rows = asRawEventRows(read.all(token, sessionId, repoId, workBindingId, now, token,
      after?.id ?? null, after?.advanced_by_owner ?? 0, after?.captured_at ?? 0, after?.id ?? '', BATCH_SOURCE_LIMIT));
    if (rows.length === 0) return;
    after = rows.at(-1)!;
    yield* rows;
  }
}

/** One shared eligibility check for batching and quiet worker release. No raw bodies are loaded. */
function nextBatchCohort(db: DatabaseSync, token: string, now: number) {
  const sessions = db.prepare(`SELECT session_id, repo_id, work_binding_id FROM raw_events
    WHERE batch_id IS NULL AND ${DUE_SOURCE_SQL} AND ${SUMMARIZABLE_ROW_SQL}
      AND ${RESOLVED_WORK_SQL}
    GROUP BY session_id, repo_id, work_binding_id ORDER BY MIN(EXISTS (SELECT 1 FROM observation_batch_sources s
      JOIN observation_batches b ON b.id = s.batch_id WHERE s.raw_event_id = raw_events.id
        AND b.owner_token = ? AND s.outcome = 'processed')), MIN(COALESCE(captured_at, 0)), session_id`);
  const priorCohort = db.prepare(`SELECT 1 FROM observation_batches b JOIN raw_events r ON r.id = b.through_event_id
    WHERE b.session_id = ? AND b.repo_id = ? AND b.work_binding_id IS ?
      AND (COALESCE(r.captured_at, 0), r.id) >= (?, ?) LIMIT 1`);
  for (const candidate of sessions.iterate(now, token, token)) {
    const sessionId = String(candidate.session_id);
    const storedSession = readSession(db, sessionId);
    if (storedSession === null) continue;
    const repoId = String(candidate.repo_id);
    const workBindingId = candidate.work_binding_id as string | null;
    const session = { ...storedSession, repo_id: repoId };
    const rows = sourcePage(dueSessionRows(db, sessionId, repoId, workBindingId, now, token));
    if (rows.length === 0) continue;
    const first = rows[0];
    const trigger = session.status === 'ended' ? 'session_end' as const
      : db.prepare('SELECT 1 FROM work_bindings WHERE id = ? AND closed_at IS NOT NULL').get(workBindingId) !== undefined
        || priorCohort.get(sessionId, repoId, workBindingId, first.captured_at ?? 0, first.id) !== undefined ? 'retention' as const
      : triggerFor(session, dueSessionRows(db, sessionId, repoId, workBindingId, now, token), now);
    if (trigger === null) continue;
    // This existing high-water mark lets a triggered ten-turn cohort drain across bounded pages.
    const through = db.prepare(`SELECT id FROM raw_events WHERE session_id = ? AND repo_id = ?
      AND work_binding_id IS ? AND batch_id IS NULL AND ${DUE_SOURCE_SQL}
      ORDER BY COALESCE(captured_at, 0) DESC, id DESC LIMIT 1`).get(sessionId, repoId, workBindingId, now, token);
    return { session, rows, trigger, workBindingId, throughEventId: String(through!.id) };
  }
  return null;
}

export function hasBatchableSources(db: DatabaseSync, token: string, now: number): boolean {
  return nextBatchCohort(db, token, now) !== null;
}

/** Reclaiming an old attempt never reuses its destination authorization for a different preset. */
export function reconcilePendingDestinations(
  db: DatabaseSync,
  token: string,
  now: number,
  preset: ProviderPreset['egress'],
): { requeued: number; leaseLost: boolean } {
  const rules = loadDestinationRules(db);
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return { requeued: 0, leaseLost: true };
    }
    let requeued = 0;
    const pending = db.prepare("SELECT id, destination, work_binding_id FROM observation_batches WHERE state = 'pending'").all();
    const readRows = db.prepare(`SELECT ${SOURCE_METADATA_COLUMNS} FROM raw_events WHERE batch_id = ? LIMIT ?`);
    const release = db.prepare(`UPDATE raw_events SET batch_id = NULL,
      processing_state = CASE WHEN ${RESOLVED_WORK_SQL} THEN 'pending' ELSE 'waiting' END, retry_after = NULL
      WHERE batch_id = ? AND processing_state IN ('pending', 'waiting')`);
    const finish = db.prepare(`UPDATE observation_batches SET state = 'fallback',
      completed_at = ?, degraded_reason = ? WHERE id = ?`);
    const receipt = db.prepare(`UPDATE observation_batch_sources SET outcome = 'deferred',
      reason = ?, recorded_at = ? WHERE batch_id = ? AND outcome = 'assigned'`);
    for (const batch of pending) {
      const rows = asRawEventRows(readRows.all(batch.id, BATCH_SOURCE_LIMIT + 1));
      const oversized = rows.length > BATCH_SOURCE_LIMIT || sourcePage(rows).length < rows.length;
      const selectionRequired = db.prepare('SELECT 1 FROM work_bindings WHERE id = ? AND work_id IS NOT NULL')
        .get(batch.work_binding_id) === undefined || rows.some((row) => row.work_binding_id !== batch.work_binding_id);
      if (!selectionRequired && !oversized && !rows.some((row) => destinationFor(rules, preset, row) !== batch.destination)) continue;
      release.run(batch.id);
      finish.run(now, oversized || selectionRequired ? 'rule_based' : 'consent_changed', batch.id);
      receipt.run(selectionRequired ? 'work_selection_required' : oversized ? 'request_page_limit' : 'destination_changed', now, batch.id);
      requeued += 1;
    }
    return { requeued, leaseLost: false };
  });
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
  const cohort = nextBatchCohort(db, token, now);

  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now)) {
      db.exec('ROLLBACK');
      return { created: [], leaseLost: true };
    }

    const created: BatchRow[] = [];
    if (cohort === null) return { created, leaseLost: false };
    const { session, trigger, throughEventId: throughId } = cohort;
    const sessionId = session.id;
    const rows = cohort.rows;

    const byDestination = new Map<BatchDestination, RawEventRow[]>();

    const taken = db.prepare(
      `SELECT 1 AS present FROM observation_batches
       WHERE session_id = ? AND through_event_id = ? AND destination = ?`,
    );

    function createDestinationBatch(
      destination: BatchDestination, session: SessionRow, trigger: BatchTrigger,
    ): void {
      const list = byDestination.get(destination);
      // A batch is created only if it would carry at least one row.
      if (list === undefined || list.length === 0) return;

      const id = randomUUID();
      // data-model.md UNIQUE (session_id, through_event_id, destination). An earlier round can
      // already own that key. Each retry records a new attempt without changing the old
      // destination or membership history.
      const through =
        taken.get(sessionId, throughId, destination) === undefined
          ? throughId
          : `${throughId}:${id}`;

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
        work_binding_id: cohort!.workBindingId,
      };
      db.prepare(
        `INSERT INTO observation_batches
           (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token,
            provider_attempts, claimed_at, work_binding_id)
         VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, 0, ?, ?)`,
      ).run(
        batch.id,
        batch.repo_id,
        batch.session_id,
        batch.through_event_id,
        batch.destination,
        batch.trigger,
        token,
        now,
        batch.work_binding_id ?? null,
      );
      const claim = db.prepare('UPDATE raw_events SET batch_id = ? WHERE id = ? AND batch_id IS NULL');
      const membership = db.prepare(`INSERT INTO observation_batch_sources
        (batch_id, raw_event_id, turn_id, outcome, recorded_at) VALUES (?, ?, ?, 'assigned', ?)`);
      for (const row of list) {
        claim.run(batch.id, row.id);
        membership.run(batch.id, row.id, row.turn_id, now);
      }
      created.push(batch);
      byDestination.delete(destination);
    }

    for (const row of rows) {
      const destination = destinationFor(rules, options.preset, row);
      if (destination === null) continue;
      const list = byDestination.get(destination) ?? [];
      list.push(row);
      byDestination.set(destination, list);
    }
    for (const destination of DESTINATION_ORDER) {
      createDestinationBatch(destination, session, trigger);
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
      `SELECT id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, claimed_at, work_binding_id
       FROM observation_batches WHERE id = ?`,
    )
    .get(batchId);
  if (batchRow === undefined) return null;
  const batch = batchRow as unknown as BatchRow;

  const session = readSession(db, batch.session_id);
  if (session === null) return null;

  const turns = db
    .prepare(
      `SELECT id, ordinal, started_at, ended_at FROM turns WHERE session_id = ?
        AND id IN (SELECT turn_id FROM raw_events WHERE batch_id = ?) ORDER BY ordinal`,
    )
    .all(batch.session_id, batchId) as unknown as TurnRow[];

  const rows = asRawEventRows(
    db.prepare(`SELECT * FROM raw_events r WHERE batch_id = ? ORDER BY
      EXISTS (SELECT 1 FROM observation_batch_sources s JOIN observation_batches b ON b.id = s.batch_id
        WHERE s.raw_event_id = r.id AND b.owner_token = ? AND s.outcome = 'processed'),
      captured_at, id`).all(batchId, batch.owner_token),
  ).map(stripPartial);

  return { batch, rows, session, turns };
}
