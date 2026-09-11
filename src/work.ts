import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import { repoSecretPaths } from './config.js';
import { currentWorkCheckpoint, memoryScope } from './db/queries.js';
import { isAllowed, loadDestinationRules, type Sensitivity } from './privacy/egress.js';
import { transactionImmediate } from './worker/lease.js';
import { SUMMARIZABLE_ROW_SQL } from './worker/batches.js';

const MAX_CHOICES = 50;
const MAX_PURPOSE = 300;
// Only an explicit, direct declaration splits work on the hook path. Semantic inference belongs
// after capture; guessing here could inject another task before any worker can correct it.
const NEW_PURPOSE = /^(?:(?:new|next|separate) task|新しい(?:作業|タスク)|別の作業|別件|次の作業)\s*[:：]\s*(\S[\s\S]*)$/iu;

export type WorkBinding = {
  id: string;
  session_id: string;
  context_id: string;
  work_id: string | null;
  candidates_json: string;
  created_at: number;
  closed_at: number | null;
  reason: string;
};

type CaptureWorkInput = {
  repoId: string;
  root: string;
  contextKey: string | null;
  sessionId: string;
  sourceId: string;
  kind: string;
  content: string | null;
  inputSource: unknown;
  sensitivity: Sensitivity;
  admissible: boolean;
  capturedAt: number;
  repoSecretPaths?: unknown;
  recovered?: boolean;
  late?: boolean;
};

export function currentWorkBinding(db: DatabaseSync, sessionId: string): WorkBinding | null {
  return (db.prepare('SELECT * FROM work_bindings WHERE session_id = ? AND closed_at IS NULL')
    .get(sessionId) as WorkBinding | undefined) ?? null;
}

/** The detached Pi capture must establish this exact prompt's current binding before progress is read. */
export function capturedPromptReady(db: DatabaseSync, repoId: string, sessionId: string, promptId: string): boolean {
  const row = db.prepare(`SELECT r.sensitivity FROM raw_events r JOIN work_bindings b ON b.id = r.work_binding_id
    JOIN sessions s ON s.id = b.session_id WHERE r.repo_id = ? AND r.session_id = ? AND r.kind = 'prompt'
      AND r.classification_state = 'done' AND b.session_id = r.session_id AND b.closed_at IS NULL
      AND s.repo_id = r.repo_id AND s.agent = 'pi'
      AND CASE WHEN json_valid(r.payload_json) THEN json_extract(r.payload_json, '$.prompt_id') = ? ELSE 0 END
    LIMIT 1`).get(repoId, sessionId, promptId);
  return row !== undefined && isAllowed(loadDestinationRules(db), 'injection', row.sensitivity as Sensitivity, true);
}

function activeWorkIds(db: DatabaseSync, contextId: string): string[] {
  return db.prepare(`SELECT DISTINCT w.id, w.created_at FROM work_items w
    WHERE w.state = 'active' AND (w.origin_context_id = ? OR EXISTS (
      SELECT 1 FROM work_bindings b WHERE b.work_id = w.id AND b.context_id = ?))
    ORDER BY w.created_at, w.id LIMIT ?`).all(contextId, contextId, MAX_CHOICES + 1)
    .map((row) => String(row.id));
}

function createWork(db: DatabaseSync, input: Pick<CaptureWorkInput, 'repoId' | 'sourceId' | 'sensitivity' | 'capturedAt'>,
  contextId: string, purpose: string | null): string {
  const id = randomUUID();
  db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose,
    purpose_source_event_id, purpose_sensitivity, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(id, input.repoId, contextId, purpose, purpose === null ? null : input.sourceId,
      input.sensitivity, input.capturedAt, input.capturedAt);
  return id;
}

function closeLateSpans(db: DatabaseSync, sessionId: string, at: number): void {
  db.prepare(`UPDATE work_bindings SET closed_at = ? WHERE session_id = ? AND reason = 'late_source'
    AND created_at < ? AND closed_at > ?`).run(at, sessionId, at, at);
}

/** Capture and spool recovery call this inside their existing write transaction. */
export function bindCapturedWork(db: DatabaseSync, input: CaptureWorkInput): { bindingId: string; closedPrevious: boolean } {
  const contextKey = input.contextKey ?? `unverified:${input.sessionId}`;
  const rules = repoSecretPaths(input.repoSecretPaths);
  const context = db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at, repo_secret_paths_json)
    VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(repo_id, local_key) DO UPDATE SET
      repo_secret_paths_json = CASE WHEN excluded.repo_secret_paths_json IS NOT NULL AND
        (excluded.last_seen_at > last_seen_at OR (excluded.last_seen_at = last_seen_at AND ?))
        THEN excluded.repo_secret_paths_json ELSE repo_secret_paths_json END,
      root = CASE WHEN excluded.last_seen_at > last_seen_at OR (excluded.last_seen_at = last_seen_at AND ?)
        THEN excluded.root ELSE root END,
      last_seen_at = MAX(last_seen_at, excluded.last_seen_at) RETURNING id`)
    .get(randomUUID(), input.repoId, contextKey, input.root, input.capturedAt, input.capturedAt,
      rules === null ? null : JSON.stringify(rules), Number(!input.recovered), Number(!input.recovered))!;
  const contextId = String(context.id);
  const current = currentWorkBinding(db, input.sessionId);
  const direct = input.admissible && input.kind === 'prompt' && input.inputSource === 'user'
    && input.sensitivity !== 'secret' ? input.content?.trim() ?? '' : '';
  const declaration = input.sensitivity === 'private' ? null : NEW_PURPOSE.exec(direct)?.[1] ?? null;
  const purpose = (declaration ?? direct).slice(0, MAX_PURPOSE) || null;
  if (current !== null && (input.late === true || input.capturedAt < current.created_at ||
    (input.recovered === true && input.capturedAt === current.created_at))) {
    const historical = db.prepare(`SELECT id, reason FROM work_bindings WHERE session_id = ? AND context_id = ?
      AND created_at <= ? AND (closed_at IS NULL OR closed_at > ?) ORDER BY created_at DESC LIMIT 2`)
      .all(input.sessionId, contextId, input.capturedAt, input.capturedAt);
    const heldSpan = historical.find((binding) => binding.reason === 'late_source');
    if (heldSpan !== undefined && declaration === null) return { bindingId: String(heldSpan.id), closedPrevious: false };
    if (historical.length === 1 && declaration === null) return { bindingId: String(historical[0].id), closedPrevious: false };
    // An unplaceable late source is retained without changing the active selection.
    const next = db.prepare(`SELECT MIN(created_at) AS at FROM work_bindings
      WHERE session_id = ? AND created_at > ? AND reason <> 'late_source'`).get(input.sessionId, input.capturedAt);
    closeLateSpans(db, input.sessionId, input.capturedAt);
    const id = randomUUID();
    db.prepare(`INSERT INTO work_bindings (id, session_id, context_id, candidates_json, created_at, closed_at, reason)
      VALUES (?, ?, ?, ?, ?, ?, 'late_source')`).run(id, input.sessionId, contextId,
        JSON.stringify(activeWorkIds(db, contextId).slice(0, MAX_CHOICES)), input.capturedAt,
        typeof next?.at === 'number' ? next.at : Number.MAX_SAFE_INTEGER);
    return { bindingId: id, closedPrevious: false };
  }
  const emptyWork = current?.work_id !== null && current !== null && db.prepare(`SELECT 1 FROM work_items
    WHERE id = ? AND state = 'active' AND purpose IS NULL AND purpose_source_event_id IS NULL
      AND NOT EXISTS (SELECT 1 FROM raw_events WHERE work_binding_id = ? AND kind NOT IN ('session_start', 'probe'))`)
    .get(current.work_id, current.id) !== undefined;
  if (current?.context_id === contextId && (declaration === null || emptyWork)
    && (current.work_id === null || db.prepare(`SELECT 1 FROM work_items WHERE id = ? AND state = 'active'`)
      .get(current.work_id) !== undefined)) {
    if (current.work_id !== null && purpose !== null) {
      db.prepare(`UPDATE work_items SET purpose = ?, purpose_source_event_id = ?, purpose_sensitivity = ?, updated_at = ?
        WHERE id = ? AND purpose IS NULL AND purpose_source_event_id IS NULL`)
        .run(purpose, input.sourceId, input.sensitivity, input.capturedAt, current.work_id);
    }
    return { bindingId: current.id, closedPrevious: false };
  }

  if (current !== null) db.prepare('UPDATE work_bindings SET closed_at = ? WHERE id = ?')
    .run(input.capturedAt, current.id);
  closeLateSpans(db, input.sessionId, input.capturedAt);
  const candidates = declaration === null ? activeWorkIds(db, contextId) : [];
  const workId = declaration !== null || candidates.length === 0
    ? createWork(db, input, contextId, purpose)
    : candidates.length === 1 ? candidates[0] : null;
  const reason = declaration !== null ? 'new_purpose' : candidates.length === 0 ? 'new_context'
    : candidates.length === 1 ? 'only_active' : 'ambiguous';
  const id = randomUUID();
  db.prepare(`INSERT INTO work_bindings (id, session_id, context_id, work_id, candidates_json, created_at, reason)
    VALUES (?, ?, ?, ?, ?, ?, ?)`)
    .run(id, input.sessionId, contextId, workId, JSON.stringify(candidates.slice(0, MAX_CHOICES)), input.capturedAt, reason);
  return { bindingId: id, closedPrevious: current !== null };
}

export type WorkLocation = { repoId: string; contextKey: string | null };

export type WorkSelection = {
  bindingId: string | null;
  workId: string | null;
  checkpointId: string | null;
  state: string | null;
  choices: { id: string; purpose: string | null }[];
  hasMore: boolean;
};

/** A CLI/MCP caller has no native session identity; only one active work is safe to infer. */
export function readWorkSelection(db: DatabaseSync, input: WorkLocation & { bindingId?: string }): WorkSelection {
  const context = db.prepare('SELECT id FROM work_contexts WHERE repo_id = ? AND local_key = ?')
    .get(input.repoId, input.contextKey);
  const empty: WorkSelection = { bindingId: null, workId: null, checkpointId: null, state: null, choices: [], hasMore: false };
  if (typeof context?.id !== 'string') return empty;
  let ids: string[];
  let workId: string | null = null;
  if (input.bindingId !== undefined) {
    const binding = db.prepare(`SELECT b.* FROM work_bindings b JOIN sessions s ON s.id = b.session_id
      WHERE b.id = ? AND b.context_id = ? AND b.closed_at IS NULL AND s.repo_id = ?`)
      .get(input.bindingId, context.id, input.repoId) as WorkBinding | undefined;
    if (binding === undefined) return empty;
    empty.bindingId = binding.id;
    workId = binding.work_id;
    try {
      const candidates: unknown = JSON.parse(binding.candidates_json);
      ids = Array.isArray(candidates) ? candidates.filter((id): id is string => typeof id === 'string').slice(0, MAX_CHOICES + 1) : [];
    } catch { ids = []; }
  } else {
    ids = activeWorkIds(db, context.id);
    if (ids.length === 1) workId = ids[0];
  }
  if (workId !== null) {
    const work = db.prepare('SELECT id, state, current_checkpoint_memory_id FROM work_items WHERE id = ? AND repo_id = ?')
      .get(workId, input.repoId);
    return work === undefined ? empty : { ...empty, workId, state: String(work.state),
      checkpointId: typeof work.current_checkpoint_memory_id === 'string' ? work.current_checkpoint_memory_id : null };
  }
  const rules = loadDestinationRules(db);
  const choices = ids.slice(0, MAX_CHOICES).flatMap((id) => {
    const work = db.prepare('SELECT id, purpose, purpose_sensitivity FROM work_items WHERE id = ? AND repo_id = ?')
      .get(id, input.repoId);
    return work === undefined ? [] : [{ id, purpose: isAllowed(rules, 'injection', work.purpose_sensitivity as Sensitivity, true)
      && typeof work.purpose === 'string' ? work.purpose : null }];
  });
  return { ...empty, choices, hasMore: ids.length > MAX_CHOICES };
}

export function workStatus(db: DatabaseSync, location: WorkLocation, all = false) {
  const context = db.prepare('SELECT id FROM work_contexts WHERE repo_id = ? AND local_key = ?')
    .get(location.repoId, location.contextKey);
  const contextId = typeof context?.id === 'string' ? context.id : null;
  const works = db.prepare(`SELECT w.id, w.purpose, w.purpose_sensitivity,
    w.state, w.origin_context_id, w.created_at, w.updated_at, w.completed_at FROM work_items w
    WHERE w.repo_id = ? AND (? OR w.origin_context_id = ? OR EXISTS (
      SELECT 1 FROM work_bindings b WHERE b.work_id = w.id AND b.context_id = ?))
    ORDER BY w.updated_at DESC, w.id LIMIT ?`).all(location.repoId, Number(all), contextId, contextId, MAX_CHOICES + 1);
  const bindings = db.prepare(`SELECT b.id, b.session_id, b.context_id, b.work_id, b.candidates_json, b.reason,
    b.created_at, b.closed_at FROM work_bindings b JOIN work_contexts c ON c.id = b.context_id
    WHERE c.repo_id = ? AND (? OR c.id = ?) AND (b.closed_at IS NULL OR b.work_id IS NULL)
    ORDER BY b.created_at DESC, b.id LIMIT ?`).all(location.repoId, Number(all), contextId, MAX_CHOICES + 1);
  const rules = loadDestinationRules(db);
  return { contextId, contextVerified: location.contextKey !== null, works: works.slice(0, MAX_CHOICES).map(({ purpose_sensitivity, ...work }) => ({ ...work,
    id: String(work.id), state: String(work.state),
    purpose: isAllowed(rules, 'injection', purpose_sensitivity as Sensitivity, true) ? work.purpose as string | null : null,
    checkpoint: currentWorkCheckpoint(db, String(work.id), memoryScope(db, {
      repoId: location.repoId, destination: 'injection', workId: String(work.id), history: true,
    })),
    checkpointOutcome: db.prepare(`SELECT o.checkpoint_decision AS decision, o.checkpoint_reason AS reason,
      o.checkpoint_parent_id AS parentId, o.checkpoint_memory_id AS memoryId FROM observation_batches o
      JOIN work_bindings b ON b.id = o.work_binding_id WHERE b.work_id = ? AND o.checkpoint_decision IS NOT NULL
      ORDER BY o.claimed_at DESC, o.id DESC LIMIT 1`).get(work.id) ?? null,
  })), bindings: bindings.slice(0, MAX_CHOICES),
    hasMore: works.length > MAX_CHOICES || bindings.length > MAX_CHOICES };
}

/** Explicit selection is scoped to an exact current-context binding, never the latest session. */
export function chooseWork(db: DatabaseSync, input: WorkLocation & { bindingId: string; workId: string; now: number }): WorkBinding | null {
  return transactionImmediate(db, () => {
    const binding = db.prepare(`SELECT b.* FROM work_bindings b JOIN work_contexts c ON c.id = b.context_id
      WHERE b.id = ? AND c.repo_id = ? AND c.local_key = ?`)
      .get(input.bindingId, input.repoId, input.contextKey) as WorkBinding | undefined;
    if (binding === undefined || (binding.closed_at !== null && (binding.reason !== 'late_source' || binding.work_id !== null))) return null;
    const target = input.workId === 'new' ? createWork(db, {
      repoId: input.repoId, sourceId: '', sensitivity: 'local_only', capturedAt: input.now,
    }, binding.context_id, null) : db.prepare('SELECT id FROM work_items WHERE id = ? AND repo_id = ?')
      .get(input.workId, input.repoId)?.id;
    if (typeof target !== 'string') return null;
    // Selecting retained work is explicit resumption; ending a native session never does this.
    db.prepare("UPDATE work_items SET state = 'active', completed_at = NULL, updated_at = ? WHERE id = ?")
      .run(input.now, target);
    if (binding.work_id === target) return binding;
    if (binding.work_id === null) {
      const reason = binding.reason === 'late_source' ? 'late_source' : 'explicit';
      db.prepare('UPDATE work_bindings SET work_id = ?, reason = ? WHERE id = ? AND work_id IS NULL')
        .run(target, reason, binding.id);
      if (input.workId !== 'new' || binding.closed_at !== null) return { ...binding, work_id: target, reason };
      // Consume a new-work choice token once, even when the client loses the first response.
    }
    db.prepare('UPDATE work_bindings SET closed_at = ? WHERE id = ?').run(input.now, binding.id);
    closeLateSpans(db, binding.session_id, input.now);
    const id = randomUUID();
    db.prepare(`INSERT INTO work_bindings (id, session_id, context_id, work_id, created_at, reason)
      VALUES (?, ?, ?, ?, ?, 'explicit')`).run(id, binding.session_id, binding.context_id, target, input.now);
    return currentWorkBinding(db, binding.session_id);
  });
}

export function completeWork(db: DatabaseSync, input: { repoId: string; workId: string; now: number }): boolean {
  return Number(db.prepare(`UPDATE work_items SET state = 'completed', completed_at = COALESCE(completed_at, ?),
    updated_at = ? WHERE id = ? AND repo_id = ?`).run(input.now, input.now, input.workId, input.repoId).changes) > 0;
}

/** An explicitly selected legacy source becomes historical work; it cannot replace live progress. */
export function chooseSourceWork(db: DatabaseSync, input: WorkLocation & {
  sourceId: string; workId: string; root: string; now: number;
}): string | null {
  if (input.contextKey === null) return null;
  return transactionImmediate(db, () => {
    const source = db.prepare(`SELECT id, session_id, work_binding_id, captured_at, classification_state FROM raw_events
      WHERE id = ? AND repo_id = ? AND classification_state IS NOT 'partial' AND ${SUMMARIZABLE_ROW_SQL}`)
      .get(input.sourceId, input.repoId);
    if (source === undefined || source.work_binding_id !== null) return null;
    const target = input.workId === 'new' ? undefined
      : db.prepare('SELECT id FROM work_items WHERE id = ? AND repo_id = ?').get(input.workId, input.repoId);
    if (input.workId !== 'new' && target === undefined) return null;
    const context = db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, created_at, last_seen_at)
      VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(repo_id, local_key) DO UPDATE SET last_seen_at = MAX(last_seen_at, excluded.last_seen_at)
      RETURNING id`).get(randomUUID(), input.repoId, input.contextKey, input.root, input.now, input.now)!;
    const contextId = String(context.id);
    const workId = target === undefined ? createWork(db, {
      repoId: input.repoId, sourceId: '', sensitivity: 'local_only', capturedAt: input.now,
    }, contextId, null) : String(target.id);
    if (target === undefined) db.prepare("UPDATE work_items SET state = 'dormant' WHERE id = ?").run(workId);
    const id = randomUUID();
    const capturedAt = typeof source.captured_at === 'number' ? source.captured_at : input.now;
    db.prepare(`INSERT INTO work_bindings (id, session_id, context_id, work_id, created_at, closed_at, reason)
      VALUES (?, ?, ?, ?, ?, ?, 'explicit')`).run(id, source.session_id, contextId, workId, capturedAt, capturedAt + 1);
    db.prepare(`UPDATE raw_events SET work_binding_id = ?, batch_id = NULL,
      processing_state = CASE WHEN processing_state = 'processed' THEN 'processed' ELSE 'pending' END,
      retry_after = NULL WHERE id = ? AND work_binding_id IS NULL`).run(id, source.id);
    return id;
  });
}
