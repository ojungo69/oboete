// Grok Build's deferred delivery (FR-045, contracts/agents.md "Grok Build deferred delivery", the
// five numbered rules). Grok has no channel that reaches the model before a turn starts, so the
// pack is stored `pending` and attached to every tool call until one of them actually runs.
// Amendment A15 (R13 probe: `additionalContext` arrives once per call) makes per-call duplicates
// inside one parallel batch accepted and counted rather than suppressed.
import { createHash } from 'node:crypto';
import type { DatabaseSync, SQLOutputValue } from 'node:sqlite';

import { rejectsDirectives } from '../observer/classify.js';
import { transactionImmediate } from '../worker/lease.js';
import { runtimeStateGet, runtimeStateSet } from '../worker/purge.js';
import {
  confirmDeliveryIn,
  omitInjection,
  parseAttempts,
  type DegradedReason,
  type ItemReason,
  type WhyAttempt,
} from './ledger.js';
import {
  hasControlCharacter,
  renderPack,
  type BuiltPack,
  type PackItem,
  type SecretDetector,
} from './pack.js';

/**
 * The rendered pack of the pending record. `injections` stores the hash, not the text, so the text
 * lives in `runtime_state` next to it (data-model.md runtime_state) and is deleted with the record.
 */
type PendingPack = {
  injectionId: string;
  repositoryLine: string;
  degraded: DegradedReason | null;
  blocks: { memoryId: string | null; rawEventId: string | null; lines: string[] }[];
  text: string;
};

// ponytail: one row per conversation, deleted on delivery and at Stop; a conversation that never
// stops leaves its row behind, and the worker's purge can sweep them if that ever adds up.
function packKey(conversationId: string): string {
  return `injection_pending:${conversationId}`;
}

function readPending(db: DatabaseSync, conversationId: string): PendingPack | null {
  const stored = runtimeStateGet(db, packKey(conversationId));
  if (stored === undefined) return null;
  try {
    return JSON.parse(stored) as PendingPack;
  } catch {
    // A damaged entry means the text is gone; the record then delivers nothing and is closed.
    return null;
  }
}

function writePending(
  db: DatabaseSync,
  conversationId: string,
  pending: PendingPack,
  now: number,
): void {
  runtimeStateSet(db, packKey(conversationId), JSON.stringify(pending), now);
}

function clearPending(db: DatabaseSync, conversationId: string): void {
  db.prepare('DELETE FROM runtime_state WHERE key = ?').run(packKey(conversationId));
}

type InjectionRecord = Record<string, SQLOutputValue>;

/**
 * The one record of the conversation that has not been delivered yet (rule 1). The oldest one wins,
 * because that is the record every later pack merges into.
 */
function liveRecord(
  db: DatabaseSync,
  conversationId: string,
  excludeId?: string,
): InjectionRecord | null {
  const row = db
    .prepare(
      `SELECT * FROM injections WHERE conversation_id = ? AND state IN ('pending', 'attempted')
         AND id <> ?
       ORDER BY created_at, id LIMIT 1`,
    )
    .get(conversationId, excludeId ?? '');
  return row === undefined ? null : (row as InjectionRecord);
}

/** The record a tool hook reports about: the live one, or the one that was just delivered. */
function reportedRecord(db: DatabaseSync, conversationId: string): InjectionRecord | null {
  const live = liveRecord(db, conversationId);
  if (live !== null) return live;
  const row = db
    .prepare(
      `SELECT * FROM injections WHERE conversation_id = ? AND state = 'emitted'
       ORDER BY emitted_at DESC, id DESC LIMIT 1`,
    )
    .get(conversationId);
  return row === undefined ? null : (row as InjectionRecord);
}

function saveAttempts(db: DatabaseSync, injectionId: string, attempts: WhyAttempt[]): void {
  db.prepare('UPDATE injections SET attempts_json = ? WHERE id = ?').run(
    JSON.stringify(attempts),
    injectionId,
  );
}

function omitItem(
  db: DatabaseSync,
  injectionId: string,
  item: { memoryId: string | null; rawEventId: string | null },
  reason: ItemReason,
): void {
  db.prepare(
    `UPDATE injection_items SET decision = 'omitted', reason = ?
     WHERE injection_id = ? AND decision = 'planned' AND memory_id IS ? AND raw_event_id IS ?`,
  ).run(reason, injectionId, item.memoryId, item.rawEventId);
}

function omitPlanned(db: DatabaseSync, injectionId: string, reason: ItemReason | null): void {
  db.prepare(
    `UPDATE injection_items SET decision = 'omitted', reason = ?
     WHERE injection_id = ? AND decision = 'planned'`,
  ).run(reason, injectionId);
}

/**
 * Only the blocks the stored pack rendered reached the model. A record whose text was lost (a hook
 * killed between the ledger write and the stored pack) keeps its own items `planned`, and the pack
 * that merged into it must not carry them into `included`: those memories were never delivered and
 * stay injectable (FR-026, FR-028).
 */
function omitUnrendered(
  db: DatabaseSync,
  injectionId: string,
  blocks: readonly PendingBlock[],
): void {
  const key = (memoryId: unknown, rawEventId: unknown): string =>
    JSON.stringify([memoryId ?? null, rawEventId ?? null]);
  const rendered = new Set(blocks.map((block) => key(block.memoryId, block.rawEventId)));
  const planned = db
    .prepare(
      `SELECT id, memory_id, raw_event_id FROM injection_items
       WHERE injection_id = ? AND decision = 'planned'`,
    )
    .all(injectionId);
  const omit = db.prepare(
    `UPDATE injection_items SET decision = 'omitted', reason = 'not_delivered' WHERE id = ?`,
  );
  for (const row of planned) {
    if (!rendered.has(key(row.memory_id, row.raw_event_id))) omit.run(Number(row.id));
  }
}

/** The merged-away pack keeps every row it planned, so its omissions stay in `why` (FR-028). */
function reparentItems(db: DatabaseSync, injectionId: string, liveId: string): void {
  db.prepare('UPDATE injection_items SET injection_id = ? WHERE injection_id = ?').run(
    liveId,
    injectionId,
  );
  db.prepare('DELETE FROM injections WHERE id = ?').run(injectionId);
}

function blockCost(lines: readonly string[]): number {
  return lines.join('\n').length + 1;
}

type PendingBlock = PendingPack['blocks'][number];

/** The merge rule 1 describes, computed without writing anything. */
type MergePlan = {
  liveId: string;
  repositoryLine: string;
  degraded: DegradedReason | null;
  blocks: PendingBlock[];
  text: string;
  omissions: { item: PackItem; reason: ItemReason }[];
};

/** The checks contracts/agents.md requires of a finished pack ("Pack format (all agents)"). */
export type PackValidation = { detect: SecretDetector; directives: readonly string[] };

type Rejection = 'secret_detected' | 'directive' | 'control_characters';

async function packRejection(
  text: string,
  validation: PackValidation,
): Promise<Rejection | null> {
  if (await validation.detect(text)) return 'secret_detected';
  if (rejectsDirectives(text, validation.directives) !== null) return 'directive';
  if (hasControlCharacter(text)) return 'control_characters';
  return null;
}

/**
 * `null` when the conversation has no live record, which is the case where the built pack simply
 * becomes the pending one. Reads only, so the caller can validate the text it would store before
 * the transaction that stores it.
 */
function planMerge(db: DatabaseSync, input: StorePendingInput): MergePlan | null {
  const live = liveRecord(db, input.conversationId, input.pack.injectionId);
  if (live === null) return null;

  const liveId = String(live.id);
  const previous = readPending(db, input.conversationId) ?? {
    injectionId: liveId,
    repositoryLine: input.pack.repositoryLine,
    degraded: input.pack.degraded,
    blocks: [],
    text: '',
  };
  const degraded = previous.degraded ?? input.pack.degraded;
  const budget = Number(live.char_budget ?? input.pack.charBudget);

  const blocks = [...previous.blocks];
  const known = new Set(
    blocks.map((block) => block.memoryId).filter((id): id is string => id !== null),
  );
  let used = renderPack({
    repositoryLine: previous.repositoryLine,
    blocks: blocks.map((block) => block.lines),
    degraded,
  }).length;

  const omissions: { item: PackItem; reason: ItemReason }[] = [];
  for (const item of input.pack.items.filter((entry) => entry.decision === 'planned')) {
    if (item.memoryId !== null && known.has(item.memoryId)) {
      omissions.push({ item, reason: 'duplicate_in_conversation' });
      continue;
    }
    const cost = blockCost(item.lines);
    if (used + cost > budget) {
      omissions.push({ item, reason: 'budget' });
      continue;
    }
    used += cost;
    blocks.push({ memoryId: item.memoryId, rawEventId: item.rawEventId, lines: item.lines });
    if (item.memoryId !== null) known.add(item.memoryId);
  }

  return {
    liveId,
    repositoryLine: previous.repositoryLine,
    degraded,
    blocks,
    omissions,
    text: renderPack({
      repositoryLine: previous.repositoryLine,
      blocks: blocks.map((block) => block.lines),
      degraded,
    }),
  };
}

export type StorePendingInput = {
  conversationId: string;
  epoch: number;
  pack: BuiltPack;
  now: number;
  /** contracts/agents.md: the finished pack is validated as a whole before it is emitted. */
  validation: PackValidation;
};

/**
 * Rule 1: the built pack becomes the conversation's pending record. An existing live record is not
 * replaced: its planned items stay, the new ones are added under the same budget, and the merged
 * record gets a new `pack_hash`. The merged text is a text no builder ever validated, so it passes
 * the whole-pack checks before it is stored; a hit stores nothing and the live record keeps the
 * text it already passed. Returns the id of the record that now holds the pack.
 */
export async function storePending(db: DatabaseSync, input: StorePendingInput): Promise<string> {
  const validated = planMerge(db, input);
  const rejection =
    validated === null ? null : await packRejection(validated.text, input.validation);

  return transactionImmediate(db, () => {
    const plan = planMerge(db, input);
    if (plan === null) {
      // No live record: this is the pack pack.ts validated as a whole when it built it.
      db.prepare(`UPDATE injections SET state = 'pending' WHERE id = ?`).run(
        input.pack.injectionId,
      );
      writePending(
        db,
        input.conversationId,
        {
          injectionId: input.pack.injectionId,
          repositoryLine: input.pack.repositoryLine,
          degraded: input.pack.degraded,
          blocks: input.pack.items
            .filter((item) => item.decision === 'planned')
            .map((item) => ({
              memoryId: item.memoryId,
              rawEventId: item.rawEventId,
              lines: item.lines,
            })),
          text: input.pack.text,
        },
        input.now,
      );
      return input.pack.injectionId;
    }

    if (validated === null || plan.text !== validated.text) {
      // Another hook changed the record between the check and this transaction, so the text that
      // would be stored is not the text that was validated. Nothing is merged and the memories stay
      // injectable for the next turn (FR-026, rule 5).
      omitPlanned(db, input.pack.injectionId, 'not_delivered');
      reparentItems(db, input.pack.injectionId, plan.liveId);
      return plan.liveId;
    }

    if (rejection !== null) {
      omitPlanned(db, input.pack.injectionId, rejection === 'control_characters' ? null : rejection);
      reparentItems(db, input.pack.injectionId, plan.liveId);
      if (rejection === 'control_characters') {
        // A control character in a stored pack means the stored text itself cannot be trusted, and
        // that is the one case pack.ts answers by emitting nothing at all (index_unavailable).
        omitPlanned(db, plan.liveId, 'not_delivered');
        omitInjection(db, plan.liveId, 'index_unavailable');
        clearPending(db, input.conversationId);
      }
      return plan.liveId;
    }

    // The omissions are written on the new pack's own rows, so the live record's planned row for
    // the same memory keeps standing: it is the copy that is rendered and delivered (FR-026).
    for (const omission of plan.omissions) {
      omitItem(db, input.pack.injectionId, omission.item, omission.reason);
    }
    reparentItems(db, input.pack.injectionId, plan.liveId);
    db.prepare(
      'UPDATE injections SET pack_hash = ?, chars_used = ?, degraded_reason = ? WHERE id = ?',
    ).run(
      createHash('sha256').update(plan.text, 'utf8').digest('hex'),
      plan.text.length,
      plan.degraded,
      plan.liveId,
    );
    writePending(
      db,
      input.conversationId,
      {
        injectionId: plan.liveId,
        repositoryLine: plan.repositoryLine,
        degraded: plan.degraded,
        blocks: plan.blocks,
        text: plan.text,
      },
      input.now,
    );
    return plan.liveId;
  });
}

/**
 * Rule 2: while the record is not delivered, every `PreToolUse` carries the pack and records the
 * attempt. Returns the text to emit as `additionalContext`, or null when there is nothing pending.
 */
export function attachOnPreToolUse(
  db: DatabaseSync,
  input: { conversationId: string; toolCallId: string; now: number },
): string | null {
  return transactionImmediate(db, () => {
    const live = liveRecord(db, input.conversationId);
    if (live === null) return null;
    const pending = readPending(db, input.conversationId);
    if (pending === null) return null;

    const attempts = parseAttempts(live.attempts_json);
    if (!attempts.some((attempt) => attempt.tool_call_id === input.toolCallId)) {
      attempts.push({
        tool_call_id: input.toolCallId,
        execution: 'pending',
        delivery: 'pending',
        at: input.now,
      });
      saveAttempts(db, String(live.id), attempts);
    }
    db.prepare(
      `UPDATE injections SET state = 'attempted', attempted_at = COALESCE(attempted_at, ?)
       WHERE id = ?`,
    ).run(input.now, String(live.id));
    return pending.text;
  });
}

export type DeferredDelivery = {
  status: 'emitted' | 'already' | 'none';
  /** Set only when this hook has to print the pack itself (rule 3). */
  text: string | null;
};

/**
 * Rule 3: the call ran, so the pack reached the model. The first delivered attempt makes the record
 * `emitted` and its items `included`; a later attempt of the same parallel batch only raises
 * `delivery_count` (A15). A `PostToolUse` with no attempt of its own means oboete's `PreToolUse`
 * handler did not complete, so the pack is emitted from here instead.
 */
export function confirmOnPostToolUse(
  db: DatabaseSync,
  input: { conversationId: string; toolCallId: string; exitCode?: number; now: number },
): DeferredDelivery {
  // The R13 probe: a failed shell call arrives here with its exit code, and the context was
  // delivered all the same.
  const execution = (input.exitCode ?? 0) === 0 ? 'ran' : 'failed';
  return deliver(db, { ...input, execution });
}

/**
 * Rule 4: `PostToolUseFailure` is a delivered attempt of a call that failed; `PermissionDenied`
 * (which Grok fires only for a permission-rule deny) delivered nothing, so the record stays open
 * and the next `PreToolUse` attaches the pack again.
 */
export function markFailure(
  db: DatabaseSync,
  input: {
    conversationId: string;
    toolCallId: string;
    kind: 'PostToolUseFailure' | 'PermissionDenied';
    now: number;
  },
): 'emitted' | 'attempted' | 'none' {
  if (input.kind === 'PostToolUseFailure') {
    return deliver(db, { ...input, execution: 'failed' }).status === 'none' ? 'none' : 'emitted';
  }

  return transactionImmediate(db, () => {
    // The denied call may belong to the batch whose other call already delivered the pack, so the
    // attempt to update can sit on the emitted record (rule 4: the deny is recorded either way).
    const record = reportedRecord(db, input.conversationId);
    if (record === null) return 'none';
    const attempts = parseAttempts(record.attempts_json);
    const attempt = attempts.find((entry) => entry.tool_call_id === input.toolCallId);
    if (attempt === undefined) {
      // A call the pack was never attached to writes no attempt on a record that is already done.
      if (record.state === 'emitted') return 'none';
      attempts.push({
        tool_call_id: input.toolCallId,
        execution: 'denied',
        delivery: 'dropped',
        at: input.now,
      });
    } else {
      attempt.execution = 'denied';
      attempt.delivery = 'dropped';
    }
    saveAttempts(db, String(record.id), attempts);
    return 'attempted';
  });
}

function deliver(
  db: DatabaseSync,
  input: {
    conversationId: string;
    toolCallId: string;
    execution: 'ran' | 'failed';
    now: number;
  },
): DeferredDelivery {
  return transactionImmediate(db, () => {
    const record = reportedRecord(db, input.conversationId);
    if (record === null) return { status: 'none', text: null };

    const injectionId = String(record.id);
    const emitted = record.state === 'emitted';
    const pending = readPending(db, input.conversationId);
    const attempts = parseAttempts(record.attempts_json);
    const attempt = attempts.find((entry) => entry.tool_call_id === input.toolCallId);
    let text: string | null = null;

    if (attempt !== undefined && attempt.delivery === 'delivered') {
      return { status: 'already', text: null };
    }
    // A15 counts the calls of the batch that carried the pack. A later call of the conversation
    // reaches PostToolUse with no attempt of its own: it carried nothing, so it delivers nothing.
    if (attempt === undefined && emitted) return { status: 'none', text: null };
    if (attempt === undefined) {
      attempts.push({
        tool_call_id: input.toolCallId,
        execution: input.execution,
        delivery: 'delivered',
        at: input.now,
      });
      // Rule 3: the pack was never attached, so this hook prints it.
      if (!emitted) text = pending?.text ?? null;
    } else {
      attempt.execution = input.execution;
      attempt.delivery = 'delivered';
    }

    saveAttempts(db, injectionId, attempts);
    db.prepare('UPDATE injections SET delivery_count = COALESCE(delivery_count, 0) + 1 WHERE id = ?').run(
      injectionId,
    );
    if (emitted) return { status: 'already', text: null };

    omitUnrendered(db, injectionId, pending?.blocks ?? []);
    confirmDeliveryIn(db, injectionId, input.now);
    clearPending(db, input.conversationId);
    return { status: 'emitted', text };
  });
}

/**
 * Rule 5: at `Stop` a record that was never delivered is closed. Its items become `omitted` /
 * `not_delivered`, so the memories stay injectable in the next turn (FR-045, FR-026).
 */
export function closeOnStop(
  db: DatabaseSync,
  input: { conversationId: string; sawAnyToolHook: boolean; now: number },
): 'omitted' | 'none' {
  return transactionImmediate(db, () => {
    // Rule 4: Stop drops every attempt still pending, on the delivered record as well as on the
    // open one, so `why` never reports a call of this turn as still waiting.
    const record = reportedRecord(db, input.conversationId);
    if (record === null) return 'none';

    const attempts = parseAttempts(record.attempts_json);
    for (const attempt of attempts) {
      if (attempt.delivery === 'pending') attempt.delivery = 'dropped';
    }
    saveAttempts(db, String(record.id), attempts);
    if (record.state === 'emitted') return 'none';

    db.prepare(
      `UPDATE injection_items SET decision = 'omitted', reason = 'not_delivered'
       WHERE injection_id = ? AND decision = 'planned'`,
    ).run(String(record.id));
    // "All denied" is not distinguishable from a chain an earlier handler stopped, so it is not
    // claimed: with no tool hook at all the reason is no_tool_call, otherwise not_delivered.
    // The reason the pack was built with is the more specific one and stays (FR-028).
    omitInjection(
      db,
      String(record.id),
      (record.degraded_reason as DegradedReason | null) ??
        (input.sawAnyToolHook ? 'not_delivered' : 'no_tool_call'),
    );
    clearPending(db, input.conversationId);
    return 'omitted';
  });
}
