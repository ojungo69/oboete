// The pack builder every agent shares (contracts/agents.md "Injection policy shared by all agents"
// and "Pack format (all agents)", FR-021, FR-024, FR-025, FR-026, FR-028, FR-029, FR-044).
// Hook path: no heavy import, no network, no file read beyond the staleness check.
import type { DatabaseSync, SQLInputValue, SQLOutputValue } from 'node:sqlite';

import {
  latestSessionState,
  latestSessionSummary,
  markInjected,
  memoryScope,
  pinnedMemories,
  type MemoryRow,
} from '../db/queries.js';
import type { AgentName } from '../events.js';
import { rejectsDirectives } from '../observer/classify.js';
import { isCjk } from '../retrieval/fts.js';
import { searchCandidates } from '../retrieval/query.js';
import { rankCandidates } from '../retrieval/rank.js';
import { payloadOf, toolInputOf } from '../worker/batches.js';
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
import { PACK_FOOTER as FOOTER, PACK_HEADER as HEADER, packHash } from './recognize.js';
import { checkPaths, repositoryHead } from './staleness.js';


/** Amendment A2: session start waits at most one second for a pending summary, then degrades. */
export const SUMMARY_WAIT_MS = 1_000;

/** data-model.md memories.last_injected_at: a memory not injected for 90 days is retired. */
const RETIREMENT_MS = 90 * 24 * 60 * 60 * 1_000;

const RAW_ACTIVITY_LIMIT = 6;
const PROMPT_EXCERPT = 200;

/**
 * One full sentence per reason code (contracts/agents.md "Pack format"): the code itself stays in
 * the ledger, `why` and doctor, and the reader of a pack sees plain language.
 */
export const DEGRADED_SENTENCES: Record<DegradedReason, string> = {
  summary_pending:
    'The summary of the previous session is not finished yet, so these are its most recent raw notes.',
  index_unavailable: 'The memory index could not be read this time, so some notes are missing.',
  empty: 'There is nothing recorded for this repository yet.',
  window_unknown:
    'The context window of this model is not documented yet, so a deliberately small amount of text was selected.',
  no_tool_call: 'This turn ran no tool, so these notes could not be handed over.',
  not_delivered:
    'These notes could not be handed over during this turn and stay available for the next one.',
  no_provider: 'No summarizer is configured, so these are rule-based notes.',
  unreachable: 'The summarizer could not be reached, so these are rule-based notes.',
  unusable_output: 'The summarizer returned an unusable answer, so these are rule-based notes.',
  language_mismatch:
    'The summarizer answered in another language than the content, so these are rule-based notes.',
  daily_cap: "Today's free summary quota is used up, so these are rule-based notes.",
  provider_exhausted: "The summarizer's free allowance is used up, so these are rule-based notes.",
  provider_paid:
    'The configured model is not on the free plan, so these are rule-based notes.',
  auth_failed: 'The summarizer rejected the credentials, so these are rule-based notes.',
  consent_changed:
    'The summarizer settings changed after consent was given, so these are rule-based notes.',
  model_alias: 'The configured model resolved to a different one, so these are rule-based notes.',
  timeout: 'The summarizer did not answer in time, so these are rule-based notes.',
  rule_based: 'These notes were written by the built-in rules rather than by a summarizer.',
};

const STALE_NOTES: Record<'stale_path' | 'stale_commit', string> = {
  stale_path: 'this file is no longer in the repository',
  stale_commit: 'this commit is not in the current history',
};

/** True when the finished text contains a secret. The caller supplies privacy/detect.ts (FR-018). */
export type SecretDetector = (text: string) => boolean | Promise<boolean>;

/** The control characters `canonicalLine` removed; a finished pack must not carry one back. */
export function hasControlCharacter(text: string): boolean {
  return /[\p{Cc}\p{Cf}]/u.test(text.replace(/\n/g, ''));
}

export type PackChannelInput = {
  agent: AgentName;
  repoId: string;
  /** The normalized repository identity; userinfo is removed again here (R8). */
  repoIdentityDisplay: string;
  sessionId: string;
  conversationId: string;
  turnId?: string | null;
  epoch: number;
  model: string | undefined;
  channelCap: number | null;
  contextFraction: number;
  channel: string;
  now: number;
  detect: SecretDetector;
  directives: readonly string[];
  repoRoot: string;
  /** What is left of the hook's deadline; the pack's one git call stays inside it (FR-002). */
  remainingBudget?: () => number;
  /** Grok stores its pack `pending` until a tool call delivers it (FR-045); everyone else prints. */
  state?: Extract<InjectionState, 'built' | 'pending'>;
};

export type SessionStartInput = PackChannelInput & {
  waitForSummary: (waitMs: number) => 'ready' | 'pending' | 'none';
};

export type PromptPackInput = PackChannelInput & { prompt: string; threshold?: number };

export type PackItem = LedgerItem & { lines: string[] };

export type BuiltPack = {
  injectionId: string;
  text: string;
  items: PackItem[];
  repositoryLine: string;
  degraded: DegradedReason | null;
  charBudget: number;
  charsUsed: number;
};

/**
 * FR-021: every external string becomes one line before it is framed, so no title, path, remote or
 * body can produce an unprefixed line or a line of its own that starts with `{`.
 */
function canonicalLine(value: string): string {
  return value
    .replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function bodyLines(body: string): string[] {
  // The documented format is one line per paragraph; each paragraph is canonicalized on its own and
  // then framed, so a body can produce no unprefixed line (FR-021).
  return body
    .split(/\n\s*\n/)
    .map(canonicalLine)
    .filter((line) => line !== '')
    .map((line) => `> ${line}`);
}

/** FR-021: a pack that began with `{` would be parsed as JSON and dropped by Claude Code. */
export function guardLeadingBrace(text: string): string {
  return text.startsWith('{') ? ` ${text}` : text;
}

/**
 * R8: an identity that still carries `user:password@` never reaches a pack. Only the remote form
 * `host/path` can carry one; a machine-local identity is a file path and is left as it is.
 */
function withoutUserinfo(identity: string): string {
  if (identity.startsWith('/') || /^[A-Za-z]:[\\/]/.test(identity)) return identity;
  const segments = identity.split('/');
  const at = segments[0].lastIndexOf('@');
  if (at === -1) return identity;
  segments[0] = segments[0].slice(at + 1);
  return segments.join('/');
}

function relativeTime(from: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - from) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? '' : 's'} ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} hour${hours === 1 ? '' : 's'} ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? 'yesterday' : `${days} days ago`;
}

export function renderPack(input: {
  repositoryLine: string;
  blocks: readonly (readonly string[])[];
  degraded: DegradedReason | null;
}): string {
  const lines = [HEADER, input.repositoryLine];
  for (const block of input.blocks) lines.push(...block);
  if (input.degraded !== null) lines.push(`> degraded: ${DEGRADED_SENTENCES[input.degraded]}`);
  lines.push(FOOTER);
  return guardLeadingBrace(lines.join('\n'));
}

function scriptOf(text: string): 'en' | 'cjk' {
  for (const character of text) {
    if (isCjk(character)) return 'cjk';
  }
  return 'en';
}

type Citation = { kind: 'file_read' | 'file_modified' | 'commit'; value: string };

function citationsOf(db: DatabaseSync, ids: readonly string[]): Map<string, Citation[]> {
  const byMemory = new Map<string, Citation[]>();
  if (ids.length === 0) return byMemory;
  const rows = db
    .prepare(
      `SELECT memory_id, citation_kind, citation_value FROM memory_sources
       WHERE memory_id IN (${ids.map(() => '?').join(', ')})
         AND citation_kind IS NOT NULL AND citation_value IS NOT NULL
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

type ActivityRow = { rawEventId: string; line: string };

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
 * The most recent prompts and tool calls of the session whose summary is still pending. Secret,
 * partial and failed rows and every tool result stay out (contracts/agents.md injection policy).
 */
function latestRawActivity(db: DatabaseSync, sessionId: string): ActivityRow[] {
  const rows = db
    .prepare(
      `SELECT id, kind, content, payload_json FROM raw_events
       WHERE session_id = ? AND kind IN ('prompt', 'tool_call')
         AND classification_state = 'done' AND sensitivity <> 'secret'
         AND content IS NOT NULL AND content <> ''
       ORDER BY captured_at DESC, id DESC LIMIT ?`,
    )
    .all(sessionId, RAW_ACTIVITY_LIMIT);
  return rows
    .reverse()
    .map((row) => ({ rawEventId: String(row.id), line: activityLine(row) }))
    .filter((activity) => activity.line !== '');
}

type PackMemory = {
  id: string;
  title: string;
  body: string;
  label: 'summary' | 'pinned' | 'related';
  reason: ItemReason | null;
  rank: number | null;
  scoreBm25?: number | null;
  scoreRrf?: number | null;
  scoreMmr?: number | null;
  createdAt?: number | null;
};

function memoryOf(row: MemoryRow, label: 'summary' | 'pinned', reason: ItemReason): PackMemory {
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

type Assembly = {
  kind: InjectionKind;
  memories: PackMemory[];
  activity: ActivityRow[];
  omitted: LedgerItem[];
  degraded: DegradedReason | null;
  budgetChars: number;
  directives: readonly string[];
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

function blockCost(block: readonly string[]): number {
  return block.join('\n').length + 1;
}

/** Shared tail of both builders: staleness, framing, budget, validation, ledger. */
async function assemble(
  db: DatabaseSync,
  input: PackChannelInput,
  assembly: Assembly,
): Promise<BuiltPack | null> {
  const repositoryLine = `> repository: ${canonicalLine(withoutUserinfo(input.repoIdentityDisplay))}`;

  // FR-029: every cited path and commit of every candidate is checked before the pack is built.
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

  const items: PackItem[] = [];
  for (const memory of assembly.memories) {
    const own = citations.get(memory.id) ?? [];
    const commitsFresh = fresh.has(memory.id);
    const stale = own.find((citation) =>
      citation.kind === 'commit' ? !commitsFresh : pathState.get(citation.value) === false,
    );
    const staleReason: 'stale_path' | 'stale_commit' | null =
      stale === undefined ? null : stale.kind === 'commit' ? 'stale_commit' : 'stale_path';
    const shown = stale ?? own[0];
    const note =
      memory.label !== 'related' || shown === undefined
        ? ''
        : ` [${canonicalLine(shown.value)}${staleReason === null ? '' : `; ${STALE_NOTES[staleReason]}`}]`;

    const head =
      memory.label === 'summary'
        ? `> session summary (${relativeTime(memory.createdAt ?? input.now, input.now)}):`
        : `> ${memory.label}: ${canonicalLine(memory.title)}${note}`;

    items.push({
      sourceKind: memory.label === 'summary' ? 'session_summary' : 'memory',
      memoryId: memory.id,
      rawEventId: null,
      decision: 'planned',
      // A stale citation is the more specific record; the memory is still injected, marked.
      reason: staleReason ?? memory.reason,
      rank: memory.rank,
      scoreBm25: memory.scoreBm25 ?? null,
      scoreRrf: memory.scoreRrf ?? null,
      scoreMmr: memory.scoreMmr ?? null,
      stale: stale === undefined ? 0 : 1,
      lines: [head, ...bodyLines(memory.body)],
    });
  }

  for (const activity of assembly.activity) {
    items.push({
      sourceKind: 'raw_activity',
      memoryId: null,
      rawEventId: activity.rawEventId,
      decision: 'planned',
      reason: null,
      rank: null,
      stale: 0,
      lines: [`> recent activity: ${canonicalLine(activity.line)}`],
    });
  }

  // FR-021: an item that reads as an instruction to the agent is dropped, not framed harder. The
  // corpus is matched with the observer's normalization (A13), so a full-width or half-width form
  // of a phrase is the same phrase.
  for (const item of items) {
    if (rejectsDirectives(item.lines.join('\n'), assembly.directives) !== null) {
      item.decision = 'omitted';
      item.reason = 'directive';
      item.lines = [];
    }
  }

  const degraded = assembly.degraded;
  const kept: PackItem[] = [];
  let used = renderPack({ repositoryLine, blocks: [], degraded }).length;
  for (const item of items) {
    if (item.decision !== 'planned') continue;
    const cost = blockCost(item.lines);
    if (used + cost > assembly.budgetChars) {
      item.decision = 'omitted';
      item.reason = 'budget';
      item.lines = [];
      continue;
    }
    used += cost;
    kept.push(item);
  }

  let text = renderPack({ repositoryLine, blocks: kept.map((item) => item.lines), degraded });

  // FR-018: the finished pack is scanned as a whole; a hit drops the item that carries it and the
  // pack is rendered again. A pack with a detector hit is never emitted.
  if (kept.length > 0 && (await input.detect(text))) {
    for (const item of kept) {
      if (await input.detect(item.lines.join('\n'))) {
        item.decision = 'omitted';
        item.reason = 'secret_detected';
        item.lines = [];
      }
    }
    const survivors = kept.filter((item) => item.decision === 'planned');
    text = renderPack({ repositoryLine, blocks: survivors.map((item) => item.lines), degraded });
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

  if (kept.length === 0) return recordOmitted(db, input, assembly, items, 'empty');

  // docs/dev/conventions.md: the record and the rows it accounts for are one write unit, so no
  // reader ever finds a pack whose items are missing.
  const injectionId = transactionImmediate(db, () => {
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
      degradedReason: degraded,
      createdAt: input.now,
    });
    planItems(db, { id, conversationId: input.conversationId, epoch: input.epoch }, [
      ...items,
      ...assembly.omitted,
    ]);
    return id;
  });

  return {
    injectionId,
    text,
    items,
    repositoryLine,
    degraded,
    charBudget: assembly.budgetChars,
    charsUsed: text.length,
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

export async function buildSessionStartPack(
  db: DatabaseSync,
  input: SessionStartInput,
): Promise<BuiltPack | null> {
  // FR-024 with A12: one session-start pack per conversation and epoch, so a resume adds nothing.
  if (sessionStartEmitted(db, input.conversationId, input.epoch)) return null;

  const scope = memoryScope(db, { repoId: input.repoId, destination: 'injection' });
  let degraded: DegradedReason | null = null;
  let activity: ActivityRow[] = [];

  // FR-024 with A2: the previous session decides. While its summary is pending the pack waits at
  // most one second, and an older session's summary never stands in for it.
  const state = latestSessionState(db, input.repoId);
  const outcome =
    state?.summaryState === 'pending' ? input.waitForSummary(SUMMARY_WAIT_MS) : 'ready';
  let summary = latestSessionSummary(db, input.repoId);
  if (outcome === 'pending' && state !== null) {
    summary = null;
    activity = latestRawActivity(db, state.sessionId);
    degraded = 'summary_pending';
  }

  const delivered = alreadyIncluded(db, input.conversationId, input.epoch);
  const omitted: LedgerItem[] = [];
  const memories: PackMemory[] = [];
  if (summary !== null && !delivered.has(summary.id)) {
    memories.push(memoryOf(summary, 'summary', 'summary'));
  } else if (summary !== null) {
    omitted.push(omittedItem(summary.id, 'duplicate_in_conversation'));
  }

  // FR-024: pinned memories follow the summary and are trimmed in pin order.
  for (const pinned of pinnedMemories(db, scope)) {
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
  if (degraded === null && budget.windowUnknown) degraded = 'window_unknown';

  return assemble(db, input, {
    kind: 'session_start',
    memories,
    activity,
    omitted,
    degraded,
    budgetChars: budget.chars,
    directives: input.directives,
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

  const scope = memoryScope(db, { repoId: input.repoId, destination: 'injection' });
  const found = searchCandidates(db, { text: input.prompt, scope });
  const delivered = alreadyIncluded(db, input.conversationId, input.epoch);
  const injectedAt = lastInjectedAt(
    db,
    found.rows.map((row) => row.id),
  );

  const omitted: LedgerItem[] = [];
  const candidates = found.rows.filter((row) => {
    if (delivered.has(row.id)) {
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

  return assemble(db, input, {
    kind: 'prompt',
    memories: ranked.included.map((row, index) => ({
      id: row.id,
      title: row.title,
      body: row.body,
      label: 'related',
      reason: null,
      rank: index + 1,
      scoreBm25: row.score_bm25,
      scoreRrf: row.score_rrf,
      scoreMmr: row.score_mmr,
    })),
    activity: [],
    omitted,
    degraded: budget.windowUnknown ? 'window_unknown' : null,
    budgetChars: budget.chars,
    directives: input.directives,
  });
}

/** data-model.md memories.last_injected_at, written by the caller once the pack was delivered. */
export function markInjectedMemories(db: DatabaseSync, ids: string[], now: number): void {
  markInjected(db, ids, now);
}
