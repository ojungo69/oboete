import type { DatabaseSync } from 'node:sqlite';

import { openDatabase } from './db/open.js';
import { memoryScope } from './db/queries.js';
import {
  whyReport,
  type ItemReason,
  type WhyInjection,
  type WhyItem,
} from './injection/ledger.js';
import { DEGRADED_SENTENCES } from './injection/pack-format.js';
import {
  invalid,
  oneArgument,
  parseCommand,
  runtimeWith,
  type MemoryCliRuntime,
} from './memories-cli.js';
import { ensureDirectories, oboetePaths, resolveHome } from './paths.js';
import { resolveRepoIdentity } from './repo-identity.js';
import { SOURCE_METADATA_COLUMNS, SUMMARIZABLE_ROW_SQL, type RawEventRow } from './worker/batches.js';

const ITEM_SENTENCES: Record<ItemReason, string> = {
  below_threshold: 'Its relevance score was below the threshold.',
  budget: 'The character budget was already used by higher-ranked notes.',
  duplicate_in_conversation: 'It was already handed over earlier in this conversation.',
  stale_path: 'It cites a file that no longer exists at HEAD.',
  stale_commit: 'It cites a commit that is no longer reachable.',
  retired: 'It was retired by a newer note.',
  mmr_redundant: 'It repeats a note that was already selected.',
  pinned: 'It is pinned, so it is always included.',
  summary: 'It records progress at the time of this pack.',
  not_delivered: 'It could not be handed over during this turn.',
  secret_detected: 'Its text carried a secret and was dropped.',
  directive: 'Its text read as an instruction to the agent and was dropped.',
};

const STALE_NOTE = ' (stale: path or commit no longer at HEAD)';
const MATCHED_PROMPT = 'It matched the prompt.';

type SessionRef = { id: string; agent: string; native_session_id: string };

const SOURCE_REASONS = new Set([
  'work_selection_required',
  ...Object.keys(DEGRADED_SENTENCES), 'add', 'update', 'delete', 'deduplicated', 'tombstoned',
  'historical', 'historical_delete', 'historical_update', 'no_memory', 'not_sent', 'unaccounted', 'detector_failed', 'directive',
  'unknown_source', 'destination_changed', 'request_page_limit', 'legacy_processing_unknown', 'partial_capture', 'secret', 'source_context_unknown',
]);

type HistoricalAction = { decision: 'update' | 'delete'; target: string; reason: string };
type SourceDelivery = { injectionId: string; sessionId: string; conversationId: string; epoch: number;
  memoryId: string | null; state: string; decision: string; reason: string | null; deliveryCount: number };

function historicalActions(value: unknown): HistoricalAction[] {
  if (typeof value !== 'string' || value.length > 10_000) return [];
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) return [];
    return parsed.slice(0, 20).flatMap((item: unknown) => {
      if (typeof item !== 'object' || item === null) return [];
      const { decision, target, reason } = item as Record<string, unknown>;
      return (decision === 'update' || decision === 'delete') && typeof target === 'string'
        && /^[a-zA-Z0-9:_-]{1,200}$/u.test(target)
        && (reason === 'capture_time_order' || reason === 'target_unavailable')
        ? [{ decision, target, reason }] : [];
    });
  } catch { return []; }
}

function sourceProcessing(db: DatabaseSync, sessionId: string, repoId: string, turn?: number) {
  const sources: { id: string; state: string; offset: number; total: number | null;
    outcome: string | null; reason: string | null; retryAt: number | null; historicalActions?: HistoricalAction[];
    memoryIds: string[]; memoriesTruncated: boolean; deliveries: SourceDelivery[]; deliveriesTruncated: boolean }[] = [];
  const scope = memoryScope(db, { repoId, destination: 'injection', history: true });
  const linked = db.prepare(`SELECT DISTINCT m.id FROM memory_sources ms JOIN memories m ON m.id = ms.memory_id
    WHERE ms.raw_event_id = ? AND ms.context_only = 0 AND ${scope.where} ORDER BY m.id LIMIT 21`);
  const delivered = db.prepare(`SELECT i.id AS injectionId, i.session_id AS sessionId, i.conversation_id AS conversationId,
    i.context_epoch AS epoch, ii.memory_id AS memoryId, i.state, ii.decision, ii.reason, i.delivery_count AS deliveryCount
    FROM injection_items ii JOIN injections i ON i.id = ii.injection_id
    JOIN sessions s ON s.id = i.session_id AND s.repo_id = i.repo_id
    WHERE i.repo_id = ? AND ii.conversation_id = i.conversation_id AND ii.context_epoch = i.context_epoch
      AND (ii.raw_event_id = ? OR EXISTS (SELECT 1 FROM memory_sources ms JOIN memories m ON m.id = ms.memory_id
        WHERE ms.memory_id = ii.memory_id AND ms.raw_event_id = ? AND ms.context_only = 0 AND ${scope.where}))
    ORDER BY i.created_at DESC, i.id, ii.id LIMIT 21`);
  const states = new Set(['built', 'emitted', 'omitted', 'pending', 'attempted']);
  const decisions = new Set(['planned', 'included', 'omitted']);
  const rows = db.prepare(`SELECT ${SOURCE_METADATA_COLUMNS} FROM raw_events
    WHERE session_id = ? AND repo_id = ?
      AND (processing_state = 'excluded' OR ${SUMMARIZABLE_ROW_SQL})
      AND (? IS NULL OR turn_id IN (SELECT id FROM turns WHERE session_id = ? AND ordinal = ?))
    ORDER BY processing_state = 'processed', captured_at, id LIMIT 101`);
  const latest = db.prepare(`SELECT s.outcome, s.reason, s.source_total, s.portion_start, s.portion_end, s.historical_actions_json
    FROM observation_batch_sources s
    JOIN observation_batches b ON b.id = s.batch_id WHERE s.raw_event_id = ? AND b.session_id = ? AND b.repo_id = ?
    ORDER BY s.recorded_at DESC, b.rowid DESC LIMIT 1`);
  function addSource(id: string, state: string, offset: number, retryAt: number | null): void {
    const receipt = latest.get(id, sessionId, repoId);
    const reason = typeof receipt?.reason === 'string' ? receipt.reason : null;
    const history = historicalActions(receipt?.historical_actions_json);
    const memories = linked.all(id, ...scope.params);
    const deliveries = delivered.all(repoId, id, id, ...scope.params);
    if (state === 'unavailable') offset = Number((receipt?.outcome === 'processed' ? receipt.portion_end : receipt?.portion_start) ?? 0);
    sources.push({
      id, state, offset,
      total: typeof receipt?.source_total === 'number' ? receipt.source_total : null,
      outcome: typeof receipt?.outcome === 'string' ? receipt.outcome : null,
      reason: reason === null ? null : SOURCE_REASONS.has(reason) ? reason : 'other',
      retryAt,
      memoryIds: memories.slice(0, 20).map((row) => String(row.id)), memoriesTruncated: memories.length > 20,
      deliveries: deliveries.slice(0, 20).map((row) => ({
        injectionId: String(row.injectionId), sessionId: String(row.sessionId), conversationId: String(row.conversationId),
        epoch: Number(row.epoch), memoryId: row.memoryId === null ? null : String(row.memoryId),
        state: states.has(String(row.state)) ? String(row.state) : 'other',
        decision: decisions.has(String(row.decision)) ? String(row.decision) : 'other',
        reason: row.reason === null ? null : Object.hasOwn(ITEM_SENTENCES, String(row.reason)) ? String(row.reason) : 'other',
        deliveryCount: Number(row.deliveryCount ?? 0),
      })), deliveriesTruncated: deliveries.length > 20,
      ...(history.length === 0 ? {} : { historicalActions: history }),
    });
  }
  for (const stored of rows.iterate(sessionId, repoId, turn ?? null, sessionId, turn ?? null)) {
    const row = stored as unknown as RawEventRow;
    addSource(row.id, row.processing_state ?? 'pending', row.processing_offset ?? 0, row.retry_after ?? null);
    if (sources.length > 100) break;
  }
  if (sources.length <= 100) {
    const missing = db.prepare(`SELECT receipt.raw_event_id FROM observation_batch_sources receipt
      JOIN observation_batches b ON b.id = receipt.batch_id
      JOIN sessions session ON session.id = b.session_id
      WHERE b.session_id = ? AND session.repo_id = ?
        AND NOT EXISTS (SELECT 1 FROM raw_events WHERE id = receipt.raw_event_id)
        AND (? IS NULL OR receipt.turn_id IN (SELECT id FROM turns WHERE session_id = ? AND ordinal = ?))
      GROUP BY receipt.raw_event_id ORDER BY MAX(receipt.recorded_at) DESC, receipt.raw_event_id LIMIT ?`);
    for (const row of missing.iterate(sessionId, repoId, turn ?? null, sessionId, turn ?? null, 101 - sources.length)) {
      addSource(String(row.raw_event_id), 'unavailable', 0, null);
    }
  }
  const legacyUnavailable = turn === undefined && db.prepare(`SELECT 1 FROM observation_batches b
    WHERE b.session_id = ? AND b.state IN ('applied', 'fallback')
      AND NOT EXISTS (SELECT 1 FROM observation_batch_sources receipt WHERE receipt.batch_id = b.id) LIMIT 1`)
    .get(sessionId) !== undefined;
  return { sources: sources.slice(0, 100), truncated: sources.length > 100, legacyUnavailable };
}

/** Checkpoint decisions are separate from ordinary source coverage, including after raw purge. */
function checkpointProcessing(db: DatabaseSync, sessionId: string, repoId: string, turn?: number) {
  const rows = db.prepare(`SELECT o.id AS batchId, b.id AS bindingId, b.work_id AS workId,
    o.checkpoint_decision AS decision, o.checkpoint_reason AS reason, o.checkpoint_parent_id AS parentId,
    o.checkpoint_memory_id AS memoryId, o.checkpoint_source_ids_json AS sourceIds
    FROM observation_batches o JOIN work_bindings b ON b.id = o.work_binding_id
    WHERE o.session_id = ? AND o.repo_id = ? AND o.checkpoint_decision IS NOT NULL
      AND (? IS NULL OR EXISTS (SELECT 1 FROM observation_batch_sources receipt JOIN turns t ON t.id = receipt.turn_id
        WHERE receipt.batch_id = o.id AND t.session_id = ? AND t.ordinal = ?))
    ORDER BY o.claimed_at DESC, o.id DESC LIMIT 101`).all(sessionId, repoId, turn ?? null, sessionId, turn ?? null);
  const decisions = new Set(['replace', 'replaced', 'confirmed', 'unchanged', 'historical', 'conflict', 'rejected']);
  const reasons = new Set(['provider_replacement', 'provider_unchanged', 'same_content', 'capture_time_order',
    'parent_changed', 'already_retired', 'tombstoned', 'parent_unavailable', 'source_not_admitted',
    'unusable_output', 'detector_failed', 'directive', 'secret']);
  return { decisions: rows.slice(0, 100).map((row) => {
    let sources: unknown;
    try { sources = JSON.parse(String(row.sourceIds)); } catch { sources = []; }
    return { batchId: row.batchId, bindingId: row.bindingId, workId: row.workId, parentId: row.parentId, memoryId: row.memoryId,
      decision: decisions.has(String(row.decision)) ? row.decision : 'other',
      reason: reasons.has(String(row.reason)) ? row.reason : 'other',
      sourceIds: Array.isArray(sources) ? sources.slice(0, 50).filter((id): id is string =>
        typeof id === 'string' && /^[a-zA-Z0-9:_-]{1,200}$/u.test(id)) : [] };
  }), truncated: rows.length > 100 };
}

function renderSourceProcessing(generation: ReturnType<typeof sourceProcessing>): string {
  if (generation.sources.length === 0 && !generation.legacyUnavailable) return '';
  const lines = ['Source processing:'];
  for (const source of generation.sources) {
    const range = source.total === null ? '' : source.state === 'unavailable'
      ? `, last recorded offset ${source.offset}/${source.total}` : `, ${source.offset}/${source.total} characters processed`;
    const reason = source.reason === null ? '' : ` (${source.reason})`;
    const retry = source.retryAt === null ? '' : `; next retry ${iso(source.retryAt)}`;
    lines.push(`  ${source.id}: ${source.state}${reason}${range}${retry}`);
    if (source.memoryIds.length > 0) lines.push(`    Memories: ${source.memoryIds.join(', ')}.`);
    for (const delivery of source.deliveries) lines.push(
      `    Injection ${delivery.injectionId} in session ${delivery.sessionId}: ${delivery.state}, ` +
      `${delivery.decision}${delivery.reason === null ? '' : ` (${delivery.reason})`}; delivery count ${delivery.deliveryCount}.`);
    if (source.memoriesTruncated || source.deliveriesTruncated) lines.push('    Further memory or delivery links are omitted.');
    for (const action of source.historicalActions ?? []) {
      lines.push(`    Historical ${action.decision} for ${action.target}: ${action.reason}.`);
    }
  }
  if (generation.legacyUnavailable) lines.push('  Original source membership is unavailable for some older attempts.');
  if (generation.truncated) lines.push('  Further source records are omitted from this report.');
  return `${lines.join('\n')}\n`;
}

function withWhyDatabase<T>(fn: (db: DatabaseSync) => T): T {
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
  try {
    return fn(opened.db);
  } finally {
    opened.db.close();
  }
}

function asSession(row: Record<string, unknown>): SessionRef {
  return {
    id: String(row.id),
    agent: String(row.agent),
    native_session_id: String(row.native_session_id),
  };
}

/** Sessions of the current repository only: another repository's ledger is "not found". */
function findSession(db: DatabaseSync, repoId: string, given: string): SessionRef[] {
  const byId = db
    .prepare('SELECT id, agent, COALESCE(original_native_session_id, native_session_id) AS native_session_id FROM sessions WHERE id = ? AND repo_id = ?')
    .get(given, repoId);
  if (byId !== undefined) return [asSession(byId)];
  return db
    .prepare(
      'SELECT id, agent, COALESCE(original_native_session_id, native_session_id) AS native_session_id FROM sessions WHERE COALESCE(original_native_session_id, native_session_id) = ? AND repo_id = ? ORDER BY agent, id',
    )
    .all(given, repoId)
    .map((row) => asSession(row));
}

function turnOrdinals(db: DatabaseSync, sessionId: string): Map<string, number> {
  const ordinals = new Map<string, number>();
  for (const row of db.prepare('SELECT id, ordinal FROM turns WHERE session_id = ?').all(sessionId)) {
    ordinals.set(String(row.id), Number(row.ordinal));
  }
  return ordinals;
}

function iso(at: number): string {
  return new Date(at).toISOString();
}

function reasonSentence(reason: ItemReason | null): string {
  return reason === null ? MATCHED_PROMPT : ITEM_SENTENCES[reason];
}

function includedLabel(item: WhyItem): string {
  if (item.title !== null && item.title !== '') return item.title;
  if (item.sourceKind === 'raw_activity') return 'raw activity';
  if (item.sourceKind === 'session_summary') return 'session summary';
  return item.sourceKind ?? 'memory';
}

function omittedLabel(item: WhyItem): string {
  if (item.title !== null && item.title !== '') return item.title;
  return item.sourceKind ?? 'memory';
}

function itemLine(label: string, item: WhyItem, rank: boolean): string {
  const stale = item.stale ? STALE_NOTE : '';
  const prefix = rank && item.rank !== null ? `${item.rank}. ` : '';
  return `    ${prefix}${label} — ${reasonSentence(item.reason)}${stale}`;
}

function renderInjection(injection: WhyInjection, ordinals: Map<string, number>): string {
  const channel = injection.channel ?? '';
  const bits = [`epoch ${injection.contextEpoch}`];
  if (injection.turnId !== null) {
    const ordinal = ordinals.get(injection.turnId);
    if (ordinal !== undefined) bits.push(`turn ${ordinal}`);
  }
  if (injection.createdAt !== null) bits.push(`built ${iso(injection.createdAt)}`);
  if (injection.emittedAt !== null) bits.push(`delivered ${iso(injection.emittedAt)}`);
  const lines = [
    `${injection.kind} pack (${channel}) — ${injection.state}, ${bits.join(', ')}`,
  ];

  const used = injection.charsUsed ?? 0;
  const budget = injection.charBudget ?? 0;
  const trimmed = injection.items.filter(
    (item) => item.decision === 'omitted' && item.reason === 'budget',
  ).length;
  let budgetLine = `  budget: ${used} of ${budget} characters`;
  if (trimmed > 0) budgetLine += `; trimmed: ${trimmed} candidates omitted for budget`;
  lines.push(budgetLine);

  if (injection.degradedReason !== null) {
    const sentence = DEGRADED_SENTENCES[injection.degradedReason];
    lines.push(`  degraded: ${sentence} (${injection.degradedReason})`);
  }

  if (injection.deferred) {
    lines.push(
      `  deferred: delivered with tool calls (${injection.deliveryCount} deliveries)`,
    );
    injection.attempts.forEach((attempt, index) => {
      lines.push(
        `    attempt ${index + 1}: call ${attempt.tool_call_id} execution ${attempt.execution}, delivery ${attempt.delivery}, at ${iso(attempt.at)}`,
      );
    });
  }

  const included = injection.items.filter((item) => item.decision === 'included');
  const omitted = injection.items.filter((item) => item.decision === 'omitted');
  if (included.length > 0) {
    lines.push('  included:');
    for (const item of included) lines.push(itemLine(includedLabel(item), item, true));
  }
  if (omitted.length > 0) {
    lines.push('  omitted:');
    for (const item of omitted) lines.push(itemLine(omittedLabel(item), item, false));
  }
  return lines.join('\n');
}

function parseTurn(value: unknown, runtime: MemoryCliRuntime): number | null {
  if (typeof value !== 'string' || !/^\d+$/u.test(value)) {
    invalid(runtime, '--turn must be a non-negative integer.');
    return null;
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    invalid(runtime, '--turn must be a non-negative integer.');
    return null;
  }
  return parsed;
}

/** `oboete why <session-id> [--turn N] [--json]`: the injection ledger for one session (FR-028, FR-045). */
export async function runWhy(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(
    argv,
    { turn: { type: 'string' }, json: { type: 'boolean' } },
    runtime,
  );
  if (parsed === null) return 2;
  const givenId = oneArgument(parsed.positionals, 'why', runtime);
  if (givenId === null) return 2;
  const json = parsed.values.json === true;
  let turn: number | undefined;
  if (parsed.values.turn !== undefined) {
    const parsedTurn = parseTurn(parsed.values.turn, runtime);
    if (parsedTurn === null) return 2;
    turn = parsedTurn;
  }

  const repoId = resolveRepoIdentity(runtime.cwd).id;
  return withWhyDatabase((db) => {
    const scope = memoryScope(db, { repoId, destination: 'injection', history: true });
    const matches = findSession(db, repoId, givenId);
    if (matches.length === 0) {
      runtime.writeError(`Session ${givenId} was not found.\n`);
      return 1;
    }
    if (matches.length > 1) {
      const list = matches.map((row) => `  ${row.agent}: ${row.id}`).join('\n');
      return invalid(
        runtime,
        `${givenId} matches more than one session. Pass the oboete session id instead:\n${list}`,
      );
    }

    const session = matches[0];
    const generation = sourceProcessing(db, session.id, repoId, turn);
    const checkpoints = checkpointProcessing(db, session.id, repoId, turn);
    const injections =
      turn === undefined
        ? whyReport(db, session.id, scope)
        : whyReport(db, session.id, scope, turn);
    if (json) {
      runtime.writeOut(`${JSON.stringify({ session, injections, generation, checkpoints })}\n`);
      return 0;
    }
    for (const checkpoint of checkpoints.decisions) runtime.writeOut(
      `Work ${String(checkpoint.workId)} checkpoint: ${String(checkpoint.decision)} (${String(checkpoint.reason)}); ` +
      `parent ${String(checkpoint.parentId ?? 'none')}, memory ${String(checkpoint.memoryId ?? 'none')}, ` +
      `sources ${checkpoint.sourceIds.join(', ')}.\n`);
    if (injections.length === 0) {
      runtime.writeOut(renderSourceProcessing(generation));
      runtime.writeOut(
        turn === undefined
          ? `No injection was built for session ${session.id}.\n`
          : `No injection was built for turn ${turn} of session ${session.id}.\n`,
      );
      return 0;
    }

    const ordinals = turnOrdinals(db, session.id);
    runtime.writeOut(`${injections.map((injection) => renderInjection(injection, ordinals)).join('\n\n')}\n`);
    runtime.writeOut(renderSourceProcessing(generation));
    return 0;
  });
}
