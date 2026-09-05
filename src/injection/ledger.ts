// The injection ledger: injections and injection_items (data-model.md), the record FR-028 requires
// and the source of `oboete why` (contracts/cli.md). pack.ts and deferred.ts write through this
// module only, so "planned when built, included when delivered" holds on every channel.
import { randomUUID } from 'node:crypto';
import type { DatabaseSync, SQLInputValue } from 'node:sqlite';

import { transactionImmediate } from '../worker/lease.js';

export type InjectionKind = 'session_start' | 'prompt' | 'grok_deferred';
export type InjectionState = 'built' | 'emitted' | 'omitted' | 'pending' | 'attempted';
export type ItemDecision = 'planned' | 'included' | 'omitted';
export type ItemSourceKind = 'memory' | 'raw_activity' | 'session_summary';

/** injections.degraded_reason, exactly the values migration 0003 allows. */
export type DegradedReason =
  | 'summary_pending'
  | 'index_unavailable'
  | 'empty'
  | 'window_unknown'
  | 'no_tool_call'
  | 'not_delivered'
  | 'no_provider'
  | 'unreachable'
  | 'unusable_output'
  | 'language_mismatch'
  | 'daily_cap'
  | 'provider_exhausted'
  | 'provider_paid'
  | 'auth_failed'
  | 'consent_changed'
  | 'model_alias'
  | 'timeout'
  | 'rule_based';

/** injection_items.reason, exactly the values migration 0003 allows. */
export type ItemReason =
  | 'below_threshold'
  | 'budget'
  | 'duplicate_in_conversation'
  | 'stale_path'
  | 'stale_commit'
  | 'retired'
  | 'mmr_redundant'
  | 'pinned'
  | 'summary'
  | 'not_delivered'
  /** The finished text carried a secret, so the item that carried it was dropped (FR-018). */
  | 'secret_detected'
  /** The item read as an instruction to the agent rather than as a record (FR-021). */
  | 'directive';

export type NewInjection = {
  id?: string;
  repoId: string;
  sessionId: string;
  conversationId: string;
  turnId: string | null;
  kind: InjectionKind;
  channel: string;
  state: InjectionState;
  epoch: number;
  packHash: string | null;
  charBudget: number | null;
  charsUsed: number | null;
  degradedReason: DegradedReason | null;
  createdAt: number;
};

export type LedgerItem = {
  sourceKind: ItemSourceKind;
  memoryId: string | null;
  rawEventId: string | null;
  decision: ItemDecision;
  reason: ItemReason | null;
  rank: number | null;
  scoreBm25?: number | null;
  scoreRrf?: number | null;
  scoreMmr?: number | null;
  stale: 0 | 1;
};

export function createInjection(db: DatabaseSync, row: NewInjection): string {
  const id = row.id ?? randomUUID();
  db.prepare(
    `INSERT INTO injections (id, repo_id, session_id, conversation_id, turn_id, kind, channel,
       state, context_epoch, attempts_json, delivery_count, pack_hash, char_budget, chars_used,
       degraded_reason, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0, ?, ?, ?, ?, ?)`,
  ).run(
    id,
    row.repoId,
    row.sessionId,
    row.conversationId,
    row.turnId,
    row.kind,
    row.channel,
    row.state,
    row.epoch,
    row.packHash,
    row.charBudget,
    row.charsUsed,
    row.degradedReason,
    row.createdAt,
  );
  return id;
}

export function planItems(
  db: DatabaseSync,
  injection: { id: string; conversationId: string; epoch: number },
  items: readonly LedgerItem[],
): void {
  const insert = db.prepare(
    `INSERT INTO injection_items (injection_id, conversation_id, context_epoch, source_kind,
       memory_id, raw_event_id, decision, reason, rank, score_bm25, score_rrf, score_mmr, stale)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  );
  for (const item of items) {
    insert.run(
      injection.id,
      injection.conversationId,
      injection.epoch,
      item.sourceKind,
      item.memoryId,
      item.rawEventId,
      item.decision,
      item.reason,
      item.rank,
      item.scoreBm25 ?? null,
      item.scoreRrf ?? null,
      item.scoreMmr ?? null,
      item.stale,
    );
  }
}

function isUniqueViolation(error: unknown): boolean {
  const code = (error as { errcode?: unknown } | null)?.errcode;
  // SQLITE_CONSTRAINT (19) and SQLITE_CONSTRAINT_UNIQUE (2067) both arrive here.
  return code === 19 || code === 2067 || /UNIQUE constraint failed/.test(String(error));
}

/**
 * Delivery confirmed: the pack's planned items become `included` (FR-026 counts a memory for the
 * conversation only now). The partial unique index rejects a memory that another pack of this
 * epoch already delivered; that item is recorded as a duplicate instead of failing the delivery
 * (SC-010).
 */
export function confirmDelivery(db: DatabaseSync, injectionId: string, now: number): void {
  transactionImmediate(db, () => confirmDeliveryIn(db, injectionId, now));
}

/** The same work without its own transaction, for a caller that already opened one (deferred.ts). */
export function confirmDeliveryIn(db: DatabaseSync, injectionId: string, now: number): void {
  const rows = db
    .prepare(`SELECT id FROM injection_items WHERE injection_id = ? AND decision = 'planned'`)
    .all(injectionId);
  const include = db.prepare(`UPDATE injection_items SET decision = 'included' WHERE id = ?`);
  const duplicate = db.prepare(
    `UPDATE injection_items SET decision = 'omitted', reason = 'duplicate_in_conversation'
     WHERE id = ?`,
  );
  for (const row of rows) {
    try {
      include.run(row.id as SQLInputValue);
    } catch (error) {
      if (!isUniqueViolation(error)) throw error;
      duplicate.run(row.id as SQLInputValue);
    }
  }
  db.prepare(`UPDATE injections SET state = 'emitted', emitted_at = ? WHERE id = ?`).run(
    now,
    injectionId,
  );
}

/** Nothing reached the model: the pack is closed with the reason `why` will show (FR-028). */
export function omitInjection(
  db: DatabaseSync,
  injectionId: string,
  reason: DegradedReason | null,
): void {
  db.prepare(`UPDATE injections SET state = 'omitted', degraded_reason = ? WHERE id = ?`).run(
    reason,
    injectionId,
  );
}

/** The memories already delivered in this conversation and epoch (FR-026, A12). */
export function alreadyIncluded(
  db: DatabaseSync,
  conversationId: string,
  epoch: number,
): Set<string> {
  const rows = db
    .prepare(
      `SELECT memory_id FROM injection_items
       WHERE conversation_id = ? AND context_epoch = ? AND decision = 'included'
         AND memory_id IS NOT NULL`,
    )
    .all(conversationId, epoch);
  return new Set(rows.map((row) => String(row.memory_id)));
}

/**
 * FR-024 with A12: the session-start pack is built at most once per context epoch. A pack that was
 * closed as `omitted` delivered nothing, so it does not consume the epoch and a later session start
 * may try again.
 */
export function sessionStartEmitted(
  db: DatabaseSync,
  conversationId: string,
  epoch: number,
): boolean {
  const row = db
    .prepare(
      `SELECT 1 AS found FROM injections
       WHERE conversation_id = ? AND context_epoch = ? AND kind = 'session_start'
         AND state <> 'omitted' LIMIT 1`,
    )
    .get(conversationId, epoch);
  return row !== undefined;
}

/**
 * Whether any session-start pack, omitted ones included, was built for this conversation and epoch.
 * Codex fires no SessionStart on `/new` (A18), and its `SessionStart(compact)` follows PostCompact by
 * seconds and can be lost (A21), so the first prompt of an epoch that has no pack yet carries the
 * pack; an omitted pack counts, so a repository with nothing to inject is not retried on every prompt.
 */
export function sessionStartAttempted(
  db: DatabaseSync,
  conversationId: string,
  epoch: number,
): boolean {
  const row = db
    .prepare(
      `SELECT 1 AS found FROM injections
       WHERE conversation_id = ? AND context_epoch = ? AND kind = 'session_start' LIMIT 1`,
    )
    .get(conversationId, epoch);
  return row !== undefined;
}

export type WhyAttempt = {
  tool_call_id: string;
  execution: 'pending' | 'ran' | 'failed' | 'denied';
  delivery: 'pending' | 'delivered' | 'dropped';
  at: number;
};

export type WhyItem = {
  sourceKind: ItemSourceKind | null;
  memoryId: string | null;
  rawEventId: string | null;
  title: string | null;
  decision: ItemDecision | null;
  reason: ItemReason | null;
  rank: number | null;
  stale: boolean;
};

export type WhyInjection = {
  id: string;
  kind: InjectionKind;
  channel: string | null;
  state: InjectionState;
  contextEpoch: number;
  turnId: string | null;
  degradedReason: DegradedReason | null;
  charBudget: number | null;
  charsUsed: number | null;
  deliveryCount: number;
  deferred: boolean;
  attempts: WhyAttempt[];
  items: WhyItem[];
  createdAt: number | null;
  emittedAt: number | null;
};

export function parseAttempts(value: unknown): WhyAttempt[] {
  if (typeof value !== 'string' || value === '') return [];
  try {
    const parsed: unknown = JSON.parse(value);
    return Array.isArray(parsed) ? (parsed as WhyAttempt[]) : [];
  } catch {
    // A damaged ledger entry must not stop the report (FR-028 fails open for availability).
    return [];
  }
}

/** The ledger of one session, formatted by `oboete why` (contracts/cli.md; T074 renders it). */
export function whyReport(db: DatabaseSync, sessionId: string, turn?: number): WhyInjection[] {
  const filter =
    turn === undefined
      ? ''
      : 'AND turn_id = (SELECT id FROM turns WHERE session_id = ? AND ordinal = ?)';
  const params: SQLInputValue[] =
    turn === undefined ? [sessionId] : [sessionId, sessionId, turn];

  const injections = db
    .prepare(
      `SELECT * FROM injections WHERE session_id = ? ${filter} ORDER BY created_at, id`,
    )
    .all(...params);

  const itemsOf = db.prepare(
    `SELECT i.source_kind, i.memory_id, i.raw_event_id, i.decision, i.reason, i.rank, i.stale,
            m.title AS title
     FROM injection_items i LEFT JOIN memories m ON m.id = i.memory_id
     WHERE i.injection_id = ? ORDER BY i.rank, i.id`,
  );

  return injections.map((row) => ({
    id: String(row.id),
    kind: row.kind as InjectionKind,
    channel: row.channel === null ? null : String(row.channel),
    state: row.state as InjectionState,
    contextEpoch: Number(row.context_epoch),
    turnId: row.turn_id === null ? null : String(row.turn_id),
    degradedReason: (row.degraded_reason as DegradedReason | null) ?? null,
    charBudget: (row.char_budget as number | null) ?? null,
    charsUsed: (row.chars_used as number | null) ?? null,
    deliveryCount: Number(row.delivery_count ?? 0),
    // FR-045: the Grok lane is the deferred one, and `why` says so even for a pack that reached
    // no tool call. The channel says which lane built it whatever became of the record.
    deferred:
      row.kind === 'grok_deferred' ||
      String(row.channel ?? '').startsWith('grok:') ||
      parseAttempts(row.attempts_json).length > 0,
    attempts: parseAttempts(row.attempts_json),
    items: itemsOf.all(String(row.id)).map((item) => ({
      sourceKind: (item.source_kind as ItemSourceKind | null) ?? null,
      memoryId: item.memory_id === null ? null : String(item.memory_id),
      rawEventId: item.raw_event_id === null ? null : String(item.raw_event_id),
      title: item.title === null || item.title === undefined ? null : String(item.title),
      decision: (item.decision as ItemDecision | null) ?? null,
      reason: (item.reason as ItemReason | null) ?? null,
      rank: (item.rank as number | null) ?? null,
      stale: item.stale === 1,
    })),
    createdAt: (row.created_at as number | null) ?? null,
    emittedAt: (row.emitted_at as number | null) ?? null,
  }));
}
