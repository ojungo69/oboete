// The pack builder every agent shares (contracts/agents.md "Injection policy shared by all agents"
// and "Pack format (all agents)", FR-021, FR-024, FR-025, FR-026, FR-028, FR-029, FR-044).
// Hook path: no heavy import, no network, no file read beyond the staleness check.
import type { DatabaseSync, SQLInputValue, SQLOutputValue } from 'node:sqlite';
import { createHash } from 'node:crypto';

import {
  currentWorkCheckpoint,
  markInjected,
  memoryScope,
  pinnedMemories,
  type MemoryRow,
} from '../db/queries.js';
import type { AgentName } from '../events.js';
import { DEGRADED_PRECEDENCE, rejectsDirectives } from '../observer/classify.js';
import { isAllowed, loadDestinationRules, type Sensitivity } from '../privacy/egress.js';
import { injectionPrivacyValid, workPurposeSources, type InjectionPrivacyGuard, type SourceReference } from '../privacy/provenance.js';
import { isCjk } from '../retrieval/fts.js';
import { searchCandidates } from '../retrieval/query.js';
import { rankCandidates } from '../retrieval/rank.js';
import { payloadOf, toolInputOf, SUMMARIZABLE_ROW_SQL } from '../worker/batches.js';
import { capturedPromptReady, readWorkSelection } from '../work.js';
import { transactionImmediate } from '../worker/lease.js';
import { charBudget } from './budget.js';
import {
  alreadyIncluded,
  createInjection,
  planItems,
  sessionStartEmitted,
  type DegradedReason,
  type InjectionKind,
  type InjectionState,
  type ItemReason,
  type LedgerItem,
} from './ledger.js';
import { packHash } from './recognize.js';
import {
  activityItem, canonicalLine, hasControlCharacter, memoryItem, renderPack, withoutUserinfo,
  type ActivityRow, type Citation, type PackItem, type PackMemory,
} from './pack-format.js';
import { checkPaths, repositoryHead } from './staleness.js';


/** data-model.md memories.last_injected_at: a memory not injected for 90 days is retired. */
const RETIREMENT_MS = 90 * 24 * 60 * 60 * 1_000;

const RAW_ACTIVITY_LIMIT = 6;
const PROMPT_EXCERPT = 200;

/** True when the finished text contains a secret. The caller supplies privacy/detect.ts (FR-018). */
export type SecretDetector = (text: string, source?: SourceReference) => boolean | Promise<boolean>;

export type PackChannelInput = {
  agent: AgentName;
  repoId: string;
  /** The normalized repository identity; userinfo is removed again here (R8). */
  repoIdentityDisplay: string;
  sessionId: string;
  conversationId: string;
  turnId?: string | null;
  /** Pi's classified current prompt; null withholds work progress after a missing/failed capture. */
  workPromptId?: string | null;
  epoch: number;
  model: string | undefined;
  channelCap: number | null;
  contextFraction: number;
  channel: string;
  now: number;
  detect: SecretDetector;
  privacyGuard?: (sources: SourceReference[]) => InjectionPrivacyGuard | null;
  directives: readonly string[];
  repoRoot: string;
  /** What is left of the hook's deadline; the pack's one git call stays inside it (FR-002). */
  remainingBudget?: () => number;
  /** Grok stores its pack `pending` until a tool call delivers it (FR-045); everyone else prints. */
  state?: Extract<InjectionState, 'built' | 'pending'>;
};

export type SessionStartInput = PackChannelInput;

export type PromptPackInput = PackChannelInput & { prompt: string; threshold?: number;
  /** Memory IDs in the earlier pack of this same response, still awaiting delivery. */
  excludeMemoryIds?: readonly string[] };

export type BuiltPack = {
  injectionId: string;
  text: string;
  items: PackItem[];
  repositoryLine: string;
  degraded: DegradedReason | null;
  charBudget: number;
  charsUsed: number;
  choices?: string[];
  workGuard?: WorkGuard;
};

export type WorkGuard = Pick<PackChannelInput, 'repoId' | 'repoRoot' | 'sessionId' | 'workPromptId'> & {
  snapshot: string; privacy?: InjectionPrivacyGuard;
};

export function workGuardValid(db: DatabaseSync, guard: WorkGuard, remainingBudget?: () => number): boolean {
  return createHash('sha256').update(workSnapshot(workProgress(db, guard))).digest('hex') === guard.snapshot
    && (guard.privacy === undefined || injectionPrivacyValid(db, guard.privacy, remainingBudget));
}

function scriptOf(text: string): 'en' | 'cjk' {
  for (const character of text) {
    if (isCjk(character)) return 'cjk';
  }
  return 'en';
}

function citationsOf(db: DatabaseSync, ids: readonly string[]): Map<string, Citation[]> {
  const byMemory = new Map<string, Citation[]>();
  if (ids.length === 0) return byMemory;
  const rows = db
    .prepare(
      `SELECT memory_id, citation_kind, citation_value FROM memory_sources
       WHERE memory_id IN (${ids.map(() => '?').join(', ')})
         AND context_only = 0 AND citation_kind IS NOT NULL AND citation_value IS NOT NULL
       ORDER BY id`,
    )
    .all(...(ids as SQLInputValue[]));
  for (const row of rows) {
    const list = byMemory.get(String(row.memory_id)) ?? [];
    list.push({ kind: row.citation_kind as Citation['kind'], value: String(row.citation_value) });
    byMemory.set(String(row.memory_id), list);
  }
  return byMemory;
}

function lastInjectedAt(db: DatabaseSync, ids: readonly string[]): Map<string, number | null> {
  const byMemory = new Map<string, number | null>();
  if (ids.length === 0) return byMemory;
  const rows = db
    .prepare(
      `SELECT id, last_injected_at FROM memories WHERE id IN (${ids.map(() => '?').join(', ')})`,
    )
    .all(...(ids as SQLInputValue[]));
  for (const row of rows) {
    byMemory.set(String(row.id), (row.last_injected_at as number | null) ?? null);
  }
  return byMemory;
}

/** The memories whose cited commits the worker checked against this `HEAD` (data-model.md memories). */
function freshCitations(db: DatabaseSync, ids: readonly string[], head: string): Set<string> {
  if (ids.length === 0) return new Set();
  const rows = db
    .prepare(
      `SELECT id FROM memories
       WHERE citations_ok = 1 AND citations_head = ? AND id IN (${ids.map(() => '?').join(', ')})`,
    )
    .all(head, ...(ids as SQLInputValue[]));
  return new Set(rows.map((row) => String(row.id)));
}

function activityLine(row: Record<string, SQLOutputValue>): string {
  const content = typeof row.content === 'string' ? row.content : '';
  if (row.kind === 'prompt') return content.slice(0, PROMPT_EXCERPT);

  // Tool activity is named by its tool and by what the call itself said — the paths it touched, or
  // the command capture moved into `content` — never by its output (R12: bodies are never verbatim
  // tool output).
  const payloadJson = typeof row.payload_json === 'string' ? row.payload_json : null;
  const payload = payloadOf({ payload_json: payloadJson });
  const tool = typeof payload?.tool_name === 'string' ? payload.tool_name : 'tool';
  const input = toolInputOf({ content, payload_json: payloadJson });
  const named = input.paths.length > 0 ? input.paths.join(', ') : (input.command ?? input.text ?? '');
  return named === '' ? tool : `${tool} ${named.slice(0, PROMPT_EXCERPT)}`;
}

/**
 * The most recent pending prompts and tool calls of the selected work. Secret,
 * partial and failed rows and every tool result stay out (contracts/agents.md injection policy).
 */
function latestRawActivity(db: DatabaseSync, repoId: string, workId: string): ActivityRow[] {
  const rules = loadDestinationRules(db);
  const allowed = ['eligible', 'local_only', 'private'].filter((sensitivity) =>
    isAllowed(rules, 'injection', sensitivity as Sensitivity, true));
  if (allowed.length === 0) return [];
  const rows = db
    .prepare(
      `SELECT e.id, kind, content, payload_json FROM raw_events e JOIN work_bindings b ON b.id = e.work_binding_id
       WHERE e.repo_id = ? AND b.work_id = ? AND kind IN ('prompt', 'tool_call')
         AND classification_state = 'done' AND sensitivity IN (${allowed.map(() => '?').join(', ')})
         AND processing_state <> 'processed'
         AND ${SUMMARIZABLE_ROW_SQL}
       ORDER BY captured_at DESC, e.id DESC LIMIT ?`,
    )
    .all(repoId, workId, ...allowed, RAW_ACTIVITY_LIMIT);
  return rows
    .toReversed()
    .map((row) => ({ rawEventId: String(row.id), line: activityLine(row) }))
    .filter((activity) => activity.line !== '');
}

function memoryOf(row: MemoryRow, label: 'work checkpoint' | 'pinned', reason: ItemReason): PackMemory {
  return {
    id: row.id,
    title: row.title ?? '',
    body: row.body ?? '',
    label,
    reason,
    rank: null,
    createdAt: row.created_at,
  };
}

type ChoiceLine = string | { text: string; fallback: string; rawEventId: string };

type Assembly = {
  kind: InjectionKind;
  memories: PackMemory[];
  activity: ActivityRow[];
  omitted: LedgerItem[];
  degraded: DegradedReason | null;
  budgetChars: number;
  directives: readonly string[];
  selectionSnapshot: string;
  choices: ChoiceLine[];
};

function omittedItem(memoryId: string, reason: ItemReason): LedgerItem {
  return {
    sourceKind: 'memory',
    memoryId,
    rawEventId: null,
    decision: 'omitted',
    reason,
    rank: null,
    stale: 0,
  };
}

/**
 * US5: a memory written by the fallback carries the batch reason it was written under, and a pack
 * built from such memories says so. The most severe reason among the given memories wins, in the
 * order the summarizer uses for a session (contracts/observer.md); a pack-level reason such as
 * `summary_pending` takes precedence and is decided by the caller.
 */
function batchReasonOf(db: DatabaseSync, memoryIds: readonly string[]): DegradedReason | null {
  if (memoryIds.length === 0) return null;
  const reasons = new Set(
    db
      .prepare(
        `SELECT degraded_reason FROM memories
         WHERE degraded_reason IS NOT NULL AND id IN (${memoryIds.map(() => '?').join(', ')})`,
      )
      .all(...memoryIds)
      .map((row) => String(row.degraded_reason)),
  );
  return DEGRADED_PRECEDENCE.find((reason) => reasons.has(reason)) ?? null;
}

function blockCost(block: readonly string[]): number {
  return block.join('\n').length + 1;
}

/** Drops an item from the pack, keeping the reason the ledger shows. */
function omit(item: PackItem, reason: ItemReason): void {
  item.decision = 'omitted';
  item.reason = reason;
  item.lines = [];
}

/** The planned items that fit the budget, in order; the rest are omitted with `budget`. */
function withinBudget(items: PackItem[], budget: { budgetChars: number; used: number }): PackItem[] {
  const kept: PackItem[] = [];
  let used = budget.used;
  for (const item of items) {
    if (item.decision !== 'planned') continue;
    const cost = blockCost(item.lines);
    if (used + cost > budget.budgetChars) {
      omit(item, 'budget');
      continue;
    }
    used += cost;
    kept.push(item);
  }
  return kept;
}

/** Drops every kept item the detector answers for; returns the ones that survive. */
async function dropDetected(kept: PackItem[], detect: SecretDetector): Promise<PackItem[]> {
  for (const item of kept) {
    if (await detect(item.lines.join('\n'))) omit(item, 'secret_detected');
  }
  return kept.filter((item) => item.decision === 'planned');
}

/**
 * docs/dev/conventions.md: the record and the rows it accounts for are one write unit, so no
 * reader ever finds a pack whose items are missing.
 */
function writeInjection(
  db: DatabaseSync,
  input: PackChannelInput,
  assembly: Assembly,
  items: PackItem[],
  text: string,
): string | null {
  return transactionImmediate(db, () => {
    if (workSnapshot(workProgress(db, input)) !== assembly.selectionSnapshot) return null;
    const id = createInjection(db, {
      repoId: input.repoId,
      sessionId: input.sessionId,
      conversationId: input.conversationId,
      turnId: input.turnId ?? null,
      kind: assembly.kind,
      channel: input.channel,
      state: input.state ?? 'built',
      epoch: input.epoch,
      packHash: packHash(text),
      charBudget: assembly.budgetChars,
      charsUsed: text.length,
      degradedReason: assembly.degraded,
      createdAt: input.now,
    });
    planItems(db, { id, conversationId: input.conversationId, epoch: input.epoch }, [
      ...items,
      ...assembly.omitted,
    ]);
    return id;
  });
}

/** FR-029: every cited path and commit of every candidate, checked before the pack is built. */
function checkedCitations(
  db: DatabaseSync,
  input: PackChannelInput,
  assembly: Assembly,
): { citations: Map<string, Citation[]>; pathState: Map<string, boolean>; fresh: Set<string> } {
  const ids = assembly.memories.map((memory) => memory.id);
  const citations = citationsOf(db, ids);
  const all = [...citations.values()].flat();
  const pathState = checkPaths(
    all.filter((citation) => citation.kind !== 'commit').map((citation) => citation.value),
    input.repoRoot,
  );
  // FR-029 with the 300 ms SLA: the cited commits were checked by the worker against a `HEAD` it
  // recorded, so the pack asks git for `HEAD` once and reads that record (contracts/agents.md
  // "commits via the worker's HEAD-keyed cache"). An unchecked or older answer counts as stale,
  // because a pack never claims a citation it could not check is still current.
  const citesCommit = all.some((citation) => citation.kind === 'commit');
  const repoHead = citesCommit ? repositoryHead(input.repoRoot, input.remainingBudget?.()) : null;
  const fresh = repoHead === null ? new Set<string>() : freshCitations(db, ids, repoHead);
  return { citations, pathState, fresh };
}

/** Every candidate as a pack item, with the ones that read as instructions already omitted. */
function packItems(db: DatabaseSync, input: PackChannelInput, assembly: Assembly): PackItem[] {
  const { citations, pathState, fresh } = checkedCitations(db, input, assembly);
  const items = [
    ...assembly.memories.map((memory) =>
      memoryItem(memory, citations.get(memory.id) ?? [], {
        commitsFresh: fresh.has(memory.id),
        pathState,
        now: input.now,
      }),
    ),
    ...assembly.activity.map(activityItem),
  ];

  // FR-021: an item that reads as an instruction to the agent is dropped, not framed harder. The
  // corpus is matched with the observer's normalization (A13), so a full-width or half-width form
  // of a phrase is the same phrase.
  for (const item of items) {
    if (rejectsDirectives(item.lines.join('\n'), assembly.directives) !== null) {
      omit(item, 'directive');
    }
  }
  return items;
}

/** Shared tail of both builders: staleness, framing, budget, validation, ledger. */
async function assemble(
  db: DatabaseSync,
  input: PackChannelInput,
  assembly: Assembly,
): Promise<BuiltPack | null> {
  const repositoryLine = `> repository: ${canonicalLine(withoutUserinfo(input.repoIdentityDisplay))}`;

  const items = packItems(db, input, assembly);

  const degraded = assembly.degraded;
  // IDs/framing are engine metadata; displayed purposes retain their actual source proof.
  const plannedChoices: ChoiceLine[] = [];
  const reservedChoices: string[] = [];
  for (const line of assembly.choices) {
    const reserved = typeof line === 'string' ? line : line.text.length >= line.fallback.length ? line.text : line.fallback;
    const candidate = [...reservedChoices, reserved];
    if (renderPack({ repositoryLine, blocks: [candidate], degraded }).length > assembly.budgetChars) break;
    plannedChoices.push(line);
    reservedChoices.push(reserved);
  }
  const kept = withinBudget(items, {
    budgetChars: assembly.budgetChars,
    used: renderPack({ repositoryLine, blocks: [reservedChoices], degraded }).length,
  });

  const references = [...kept.map(({ memoryId, rawEventId }) => ({ memoryId, rawEventId })),
    ...plannedChoices.flatMap((line) => typeof line === 'string' ? [] : [{ memoryId: null, rawEventId: line.rawEventId }])];
  const privacy = input.privacyGuard?.([...new Map(references.map((source) => [JSON.stringify(source), source])).values()]);
  if (privacy === null) return recordOmitted(db, input, assembly, items, 'index_unavailable');
  const choices: string[] = [];
  for (const line of plannedChoices) choices.push(typeof line === 'string' ? line
    : await input.detect(line.text, { memoryId: null, rawEventId: line.rawEventId }) ? line.fallback : line.text);
  for (const item of kept) {
    if (await input.detect(item.lines.join('\n'), { memoryId: item.memoryId, rawEventId: item.rawEventId })) omit(item, 'secret_detected');
  }
  const allowed = kept.filter((item) => item.decision !== 'omitted');
  kept.length = 0;
  kept.push(...allowed);

  let text = renderPack({ repositoryLine, blocks: [choices, ...kept.map((item) => item.lines)], degraded });

  // FR-018: the finished pack is scanned as a whole; a hit drops the item that carries it and the
  // pack is rendered again. A pack with a detector hit is never emitted.
  if ((kept.length > 0 || choices.length > 0) && (await input.detect(text))) {
    const survivors = await dropDetected(kept, (text) => input.detect(text));
    text = renderPack({ repositoryLine, blocks: [choices, ...survivors.map((item) => item.lines)], degraded });
    kept.length = 0;
    kept.push(...survivors);
    if (await input.detect(text)) {
      return recordOmitted(db, input, assembly, items, 'index_unavailable');
    }
  }

  // Canonicalization removed them all; this is the assertion that nothing added one back.
  if (hasControlCharacter(text)) {
    return recordOmitted(db, input, assembly, items, 'index_unavailable');
  }

  if (kept.length === 0 && choices.length === 0) return recordOmitted(db, input, assembly, items, 'empty');
  if (privacy !== undefined && !injectionPrivacyValid(db, privacy, input.remainingBudget)) {
    return recordOmitted(db, input, assembly, items, 'index_unavailable');
  }

  // docs/dev/conventions.md: the record and the rows it accounts for are one write unit, so no
  // reader ever finds a pack whose items are missing.
  const injectionId = writeInjection(db, input, assembly, items, text);
  if (injectionId === null) return recordOmitted(db, input, assembly, items, 'index_unavailable');

  return {
    injectionId,
    text,
    items,
    repositoryLine,
    degraded,
    charBudget: assembly.budgetChars,
    charsUsed: text.length,
    choices,
    workGuard: { repoId: input.repoId, repoRoot: input.repoRoot, sessionId: input.sessionId,
      workPromptId: input.workPromptId,
      snapshot: createHash('sha256').update(assembly.selectionSnapshot).digest('hex'),
      ...(privacy === undefined ? {} : { privacy }) },
  };
}

/** Nothing is printed, and the ledger still says what was considered and why (FR-028). */
function recordOmitted(
  db: DatabaseSync,
  input: PackChannelInput,
  assembly: Assembly,
  items: PackItem[],
  reason: DegradedReason,
): null {
  transactionImmediate(db, () => {
    const id = createInjection(db, {
      repoId: input.repoId,
      sessionId: input.sessionId,
      conversationId: input.conversationId,
      turnId: input.turnId ?? null,
      kind: assembly.kind,
      channel: input.channel,
      state: 'omitted',
      epoch: input.epoch,
      packHash: null,
      charBudget: assembly.budgetChars,
      charsUsed: 0,
      degradedReason: reason,
      createdAt: input.now,
    });
    planItems(db, { id, conversationId: input.conversationId, epoch: input.epoch }, [
      ...items.map((item) => ({ ...item, decision: 'omitted' as const })),
      ...assembly.omitted,
    ]);
  });
  return null;
}

/** The exact native binding, including its root, is authoritative even when other sessions end. */
function workProgress(db: DatabaseSync, input: Pick<PackChannelInput, 'repoId' | 'repoRoot' | 'sessionId' | 'workPromptId'>) {
  const bound = db.prepare(`SELECT b.id, c.local_key FROM work_bindings b JOIN work_contexts c ON c.id = b.context_id
    JOIN sessions s ON s.id = b.session_id WHERE b.session_id = ? AND b.closed_at IS NULL
      AND s.repo_id = ? AND c.repo_id = ? AND c.root = ?`)
    .get(input.sessionId, input.repoId, input.repoId, input.repoRoot);
  const ready = input.workPromptId === undefined || (input.workPromptId !== null
    && capturedPromptReady(db, input.repoId, input.sessionId, input.workPromptId));
  const selection = bound === undefined || !ready ? null : readWorkSelection(db, {
    repoId: input.repoId, contextKey: String(bound.local_key), bindingId: String(bound.id),
  });
  const workId = selection?.state === 'active' ? selection.workId : null;
  const scope = memoryScope(db, { repoId: input.repoId, destination: 'injection', workId });
  return { selection, purposes: workPurposeSources(db, input.repoId, selection?.choices ?? []),
    summary: workId == null ? null : currentWorkCheckpoint(db, workId, scope),
    activity: workId == null ? [] : latestRawActivity(db, input.repoId, workId), policy: scope,
    knowledgePolicy: { ...scope, where: `${scope.where} AND m.type <> 'session_summary'` } };
}

function workSnapshot(progress: ReturnType<typeof workProgress>): string {
  const m = progress.summary;
  // Delivery counters and the worker's citation cache do not change the checkpoint's meaning or privacy.
  const summary = m === null ? null : [m.id, m.work_id, m.checkpoint_parent_id, m.title, m.body,
    m.content_hash, m.sensitivity, m.review_state, m.valid_from, m.valid_to, m.deleted_at,
    m.provenance_complete, m.degraded_reason];
  // New activity can supply Grok's delivery carrier. The privacy guard rechecks the exact source
  // bodies already in this pack; unrelated appended activity does not invalidate those bodies.
  return JSON.stringify({ selection: progress.selection, purposes: progress.purposes, policy: progress.policy, summary });
}

function choiceLines(progress: ReturnType<typeof workProgress>, directives: readonly string[]): ChoiceLine[] {
  const { selection, purposes } = progress;
  if (selection === null || selection.workId !== null || selection.bindingId === null) return [];
  return [
    '> Select which work to continue before using its progress.',
    `> Binding: ${canonicalLine(selection.bindingId)}`,
    ...selection.choices.map((choice, index) => {
      const fallback = `> Work ${canonicalLine(choice.id)}: Untitled work`;
      const rawEventId = purposes[index]?.rawEventId;
      if (choice.purpose === null || rawEventId == null || rejectsDirectives(choice.purpose, directives) !== null) return fallback;
      return { text: `> Work ${canonicalLine(choice.id)}: ${canonicalLine(choice.purpose)}`, fallback, rawEventId };
    }),
    '> Use work_choose with this binding and the chosen work ID, or oboete work choose <binding-id> <work-id|new>.',
    ...(selection.hasMore ? ['> Additional choices are available through work status.'] : []),
  ];
}

export async function buildSessionStartPack(
  db: DatabaseSync,
  input: SessionStartInput,
): Promise<BuiltPack | null> {
  // FR-024 with A12: one session-start pack per conversation and epoch, so a resume adds nothing.
  if (sessionStartEmitted(db, input.conversationId, input.epoch)) return null;

  const previous = workProgress(db, input);
  let degraded: DegradedReason | null = previous.activity.length > 0 ? 'summary_pending' : null;
  const summary = previous.summary;

  const delivered = alreadyIncluded(db, input.conversationId, input.epoch);
  const omitted: LedgerItem[] = [];
  const memories: PackMemory[] = [];
  if (summary !== null && !delivered.has(summary.id)) {
    memories.push(memoryOf(summary, 'work checkpoint', 'summary'));
  } else if (summary !== null) {
    omitted.push(omittedItem(summary.id, 'duplicate_in_conversation'));
  }

  // FR-024: pinned memories follow the summary and are trimmed in pin order.
  for (const pinned of pinnedMemories(db, previous.knowledgePolicy)) {
    if (pinned.id === summary?.id) continue;
    if (delivered.has(pinned.id)) {
      omitted.push(omittedItem(pinned.id, 'duplicate_in_conversation'));
      continue;
    }
    memories.push(memoryOf(pinned, 'pinned', 'pinned'));
  }

  const script = scriptOf(memories.map((memory) => `${memory.title}${memory.body}`).join(' '));
  const budget = charBudget({
    agent: input.agent,
    model: input.model,
    channelCap: input.channelCap,
    contextFraction: input.contextFraction,
    script,
  });
  // FR-025 and the R13 gate: an agent with no verified context window ships no injection lane.
  if (budget.blocked) return null;
  degraded ??= batchReasonOf(db, memories.map((memory) => memory.id));
  if (degraded === null && budget.windowUnknown) degraded = 'window_unknown';

  return assemble(db, input, {
    kind: 'session_start',
    memories,
    activity: previous.activity,
    omitted,
    degraded,
    budgetChars: budget.chars,
    directives: input.directives,
    selectionSnapshot: workSnapshot(previous),
    choices: choiceLines(previous, input.directives),
  });
}

export async function buildPromptPack(
  db: DatabaseSync,
  input: PromptPackInput,
): Promise<BuiltPack | null> {
  const budget = charBudget({
    agent: input.agent,
    model: input.model,
    channelCap: input.channelCap,
    contextFraction: input.contextFraction,
    script: scriptOf(input.prompt),
  });
  if (budget.blocked) return null;

  const previous = workProgress(db, input);
  const found = searchCandidates(db, { text: input.prompt, scope: previous.knowledgePolicy });
  const delivered = alreadyIncluded(db, input.conversationId, input.epoch);
  const selected = new Set([...delivered, ...(input.excludeMemoryIds ?? [])]);
  const injectedAt = lastInjectedAt(
    db,
    found.rows.map((row) => row.id),
  );

  const omitted: LedgerItem[] = [];
  const candidates = found.rows.filter((row) => {
    if (selected.has(row.id)) {
      omitted.push(omittedItem(row.id, 'duplicate_in_conversation'));
      return false;
    }
    const last = injectedAt.get(row.id) ?? null;
    // data-model.md memories.last_injected_at: injected once and untouched for 90 days = retired.
    if (last !== null && input.now - last > RETIREMENT_MS) {
      omitted.push(omittedItem(row.id, 'retired'));
      return false;
    }
    return true;
  });

  const ranked = rankCandidates(candidates, {
    threshold: input.threshold,
    lambda: 0.5,
    budgetChars: budget.chars,
  });
  for (const item of ranked.omitted) omitted.push(omittedItem(item.id, item.reason));
  const summary = previous.summary;
  const checkpoints = summary === null || selected.has(summary.id) ? [] : [memoryOf(summary, 'work checkpoint', 'summary')];
  if (summary !== null && selected.has(summary.id)) omitted.push(omittedItem(summary.id, 'duplicate_in_conversation'));

  return assemble(db, input, {
    kind: 'prompt',
    memories: [...checkpoints, ...ranked.included.map((row, index): PackMemory => ({
      id: row.id,
      title: row.title,
      body: row.body,
      label: 'related',
      reason: null,
      rank: index + 1,
      scoreBm25: row.score_bm25,
      scoreRrf: row.score_rrf,
      scoreMmr: row.score_mmr,
    }))],
    activity: previous.activity,
    omitted,
    degraded:
      (previous.activity.length > 0 ? 'summary_pending' : null) ??
      batchReasonOf(db, [...checkpoints, ...ranked.included].map((row) => row.id)) ??
      (budget.windowUnknown ? 'window_unknown' : null),
    budgetChars: budget.chars,
    directives: input.directives,
    selectionSnapshot: workSnapshot(previous),
    choices: choiceLines(previous, input.directives),
  });
}

/** data-model.md memories.last_injected_at, written by the caller once the pack was delivered. */
export function markInjectedMemories(db: DatabaseSync, ids: string[], now: number): void {
  markInjected(db, ids, now);
}
