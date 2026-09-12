import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';

import { loadConfig } from './config.js';
import {
  getMemory,
  memoryScope,
  memorySources,
  setPinned,
  timeline,
  tombstone,
  type MemoryRow,
} from './db/queries.js';
import { openDatabase } from './db/open.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from './paths.js';
import { resolveRepoIdentity } from './repo-identity.js';
import { searchCandidates } from './retrieval/query.js';
import { rankCandidates, type RankedCandidate } from './retrieval/rank.js';
import { readWorkSelection } from './work.js';
import { filterMemoryOutput, filterReadOutput, filterTimelineOutput, localHistoryOutput, type PublicTimelineSession } from './privacy/provenance.js';
import { adoptKnowledge, decideSharing, sharingStatus } from './sharing.js';

const SEARCH_DEFAULT_LIMIT = 10;
const MAX_LIMIT = 50;
export const EMPTY_REASON = 'No memories matched this query in the current repository.';
export const LEXICAL_NOTE = 'M1 search is lexical (word match). Semantic search arrives in M2.';

export type MemoryCliRuntime = {
  cwd: string;
  now(): number;
  writeOut(text: string): void;
  writeError(text: string): void;
};

type CommandOptions = Record<string, { type: 'string' | 'boolean' }>;
const READ_OPTIONS = { binding: { type: 'string' }, history: { type: 'boolean' } } as const;
type ParsedCommand = ReturnType<typeof parseArgs>;
type SearchRow = {
  id: string;
  type: string;
  title: string;
  body: string;
  sensitivity: MemoryRow['sensitivity'];
  created_at: number | null;
  score: number;
  reasons: string[];
};

export function runtimeWith(overrides: Partial<MemoryCliRuntime>): MemoryCliRuntime {
  return {
    cwd: process.cwd(),
    now: Date.now,
    writeOut: (text) => {
      process.stdout.write(text);
    },
    writeError: (text) => {
      process.stderr.write(text);
    },
    ...overrides,
  };
}

export function parseCommand(
  argv: string[],
  options: CommandOptions,
  runtime: MemoryCliRuntime,
): ParsedCommand | null {
  try {
    const parsed = parseArgs({ args: argv, allowPositionals: true, strict: true, options });
    if (typeof parsed.values.binding === 'string' && (parsed.values.binding.trim() === '' || parsed.values.binding.length > 128)) {
      invalid(runtime, '--binding must be a non-empty binding ID of at most 128 characters.');
      return null;
    }
    return parsed;
  } catch (error) {
    invalid(runtime, error instanceof Error ? error.message : String(error));
    return null;
  }
}

export function invalid(runtime: MemoryCliRuntime, message: string): 2 {
  runtime.writeError(`${message}\n`);
  return 2;
}

export function oneArgument(
  positionals: string[],
  name: string,
  runtime: MemoryCliRuntime,
): string | null {
  if (positionals.length !== 1 || positionals[0].trim() === '') {
    invalid(runtime, `${name} requires exactly one non-empty argument.`);
    return null;
  }
  return positionals[0].trim();
}

function integerOption(
  value: unknown,
  name: string,
  minimum: number,
  maximum: number,
  runtime: MemoryCliRuntime,
): number | null {
  if (typeof value !== 'string' || !/^\d+$/u.test(value)) {
    invalid(runtime, `${name} must be an integer from ${minimum} to ${maximum}.`);
    return null;
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < minimum || parsed > maximum) {
    invalid(runtime, `${name} must be an integer from ${minimum} to ${maximum}.`);
    return null;
  }
  return parsed;
}

async function withDatabase<T>(
  runtime: MemoryCliRuntime,
  fn: (db: DatabaseSync, repoId: string, paths: OboetePaths, identity: ReturnType<typeof resolveRepoIdentity>) => T | Promise<T>,
): Promise<T> {
  const identity = resolveRepoIdentity(runtime.cwd);
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
  try {
    return await fn(opened.db, identity.id, paths, identity);
  } finally {
    opened.db.close();
  }
}

function searchReasons(row: RankedCandidate): string[] {
  const reasons: string[] = [];
  if (row.scoreTrigram !== null) reasons.push('lexical_trigram_match');
  if (row.scoreCjk !== null) reasons.push('lexical_cjk_match');
  if (row.viaLike) reasons.push('lexical_word_match');
  return reasons;
}

function reasonText(reason: string): string {
  return (
    {
      lexical_trigram_match: 'lexical Latin-text matching',
      lexical_cjk_match: 'lexical Chinese, Japanese, or Korean text matching',
      lexical_word_match: 'lexical word matching',
    }[reason] ?? reason
  );
}

function sourceText(sources: ReturnType<typeof memorySources>): string {
  if (sources.length === 0) return 'not recorded';
  return sources
    .map((source) => {
      const value = source.citation_value ?? source.raw_event_id ?? 'an unspecified source';
      return source.source_agent === null ? value : `${value} from ${source.source_agent}`;
    })
    .join(', ');
}

export function renderSearch(rows: SearchRow[]): string {
  const noun = rows.length === 1 ? 'memory' : 'memories';
  return `Found ${rows.length} ${noun}.\n${rows
    .map(
      (row) =>
        `- Memory ${row.id} is a ${row.type} titled ${JSON.stringify(row.title || '(untitled)')}.\n` +
        `  Its relevance score is ${row.score.toFixed(6)}. It matched because of ${row.reasons
          .map(reasonText)
          .join(' and ')}.\n` +
        `  Its body is ${JSON.stringify(row.body)}.`,
    )
    .join('\n')}`;
}

/** The one search every surface uses (CLI, MCP, Pi tools): injection scope, lexical ranking. */
export function searchMemories(
  db: DatabaseSync,
  input: { repoId: string; paths: OboetePaths; query: string; limit: number; workId?: string | null; history?: boolean },
): SearchRow[] {
  const scope = memoryScope(db, { repoId: input.repoId, destination: 'injection', workId: input.workId, history: input.history });
  const found = searchCandidates(db, { text: input.query, scope });
  const ranked = rankCandidates(found.rows, {
    threshold: loadConfig(input.paths).injection.threshold,
    lambda: 0.5,
    limit: input.limit,
  });
  return ranked.included.flatMap((row): SearchRow[] => {
    const memory = getMemory(db, row.id, scope);
    if (memory === null) return [];
    return [
      {
        id: row.id,
        type: memory.type,
        title: row.title,
        body: row.body,
        sensitivity: memory.sensitivity,
        created_at: memory.created_at,
        score: row.score_bm25,
        reasons: searchReasons(row),
      },
    ];
  });
}

function notFound(runtime: MemoryCliRuntime, id: string, json: boolean): 1 {
  if (json) runtime.writeOut(`${JSON.stringify({ error: 'memory_not_found', id })}\n`);
  else runtime.writeError(`Memory ${id} was not found in the current repository.\n`);
  return 1;
}

export async function runSearch(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(
    argv,
    { ...READ_OPTIONS, limit: { type: 'string' }, json: { type: 'boolean' } },
    runtime,
  );
  if (parsed === null) return 2;
  const query = oneArgument(parsed.positionals, 'search', runtime);
  if (query === null) return 2;
  const limit =
    parsed.values.limit === undefined
      ? SEARCH_DEFAULT_LIMIT
      : integerOption(parsed.values.limit, '--limit', 1, MAX_LIMIT, runtime);
  if (limit === null) return 2;

  return withDatabase(runtime, async (db, repoId, paths, identity) => {
    const selection = readWorkSelection(db, { repoId, contextKey: identity.worktreeKey,
      bindingId: typeof parsed.values.binding === 'string' ? parsed.values.binding : undefined });
    const candidates = searchMemories(db, { repoId, paths, query, limit, workId: selection.workId, history: parsed.values.history === true });
    const visible = parsed.values.history === true ? { memories: localHistoryOutput(db, candidates), works: selection.choices } : await filterReadOutput(db, {
      repoId, repoRoot: identity.root, contextKey: identity.worktreeKey, workId: selection.workId,
      bindingId: selection.bindingId, home: paths.home,
    }, candidates, selection.choices);
    const rows = visible.memories;
    selection.choices = visible.works;
    const choices = selection.choices.length === 0 ? {} : { selection };

    if (parsed.values.json === true) {
      runtime.writeOut(
        `${JSON.stringify(
          rows.length === 0
            ? { memories: rows, reason: EMPTY_REASON, note: LEXICAL_NOTE, ...choices }
            : { memories: rows, ...choices },
        )}\n`,
      );
    } else if (rows.length === 0) {
      runtime.writeOut(`${EMPTY_REASON}\n${LEXICAL_NOTE}\n`);
    } else {
      runtime.writeOut(`${renderSearch(rows)}\n`);
    }
    if (parsed.values.json !== true && selection.choices.length > 0) runtime.writeOut(
      'Select work with oboete work status and pass its current --binding ID to include progress.\n');
    return 0;
  });
}

export function renderTimeline(sessions: readonly PublicTimelineSession[]): string {
  return sessions
    .map((session) => {
      const turns =
        session.turns.length === 0
          ? '  It has no recorded turns.'
          : session.turns
              .map(
                (turn) =>
                  `  Turn ${turn.ordinal} has ${
                    turn.memory_ids.length === 0
                      ? 'no memories'
                      : `memory identifiers ${turn.memory_ids.join(', ')}`
                  }.`,
              )
              .join('\n');
      const memories =
        session.memories.length === 0
          ? '  It has no visible memories.'
          : session.memories
              .map(
                (memory) =>
                  `  Memory ${memory.id} is a ${memory.type} titled ${JSON.stringify(
                    memory.title ?? '(untitled)',
                  )}. Its sensitivity is ${memory.sensitivity}. Its body is ${JSON.stringify(
                    memory.body ?? '',
                  )}.` + ('sources' in memory ? ` Its sources are ${sourceText(memory.sources)}.` : ''),
              )
              .join('\n');
      return `Session ${session.id} was recorded by ${session.agent} and is ${session.status}.\n${turns}\n${memories}`;
    })
    .join('\n');
}

export async function runTimeline(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(
    argv,
    { ...READ_OPTIONS, session: { type: 'string' }, json: { type: 'boolean' } },
    runtime,
  );
  if (parsed === null) return 2;
  if (parsed.positionals.length !== 0) return invalid(runtime, 'timeline accepts no arguments.');
  const sessionId = parsed.values.session;
  if (sessionId !== undefined && (typeof sessionId !== 'string' || sessionId.trim() === '')) {
    return invalid(runtime, '--session must not be empty.');
  }

  return withDatabase(runtime, async (db, repoId, paths, identity) => {
    const selection = readWorkSelection(db, { repoId, contextKey: identity.worktreeKey,
      bindingId: typeof parsed.values.binding === 'string' ? parsed.values.binding : undefined });
    const candidates = timeline(db, repoId, {
      ...(typeof sessionId === 'string' ? { sessionId: sessionId.trim() } : {}),
      limit: MAX_LIMIT,
      workId: selection.workId, history: parsed.values.history === true,
    });
    const visible = parsed.values.history === true ? { sessions: candidates.map((session) => ({ ...session,
      memories: localHistoryOutput(db, session.memories) })), works: selection.choices } : await filterTimelineOutput(db, {
      repoId, repoRoot: identity.root, contextKey: identity.worktreeKey, workId: selection.workId,
      bindingId: selection.bindingId, home: paths.home,
    }, candidates, selection.choices);
    const sessions = visible.sessions;
    selection.choices = visible.works;
    if (parsed.values.json === true) {
      runtime.writeOut(`${JSON.stringify({ sessions, ...(selection.choices.length === 0 ? {} : { selection }) })}\n`);
    } else if (sessions.length === 0) {
      runtime.writeOut('No sessions were found in the current repository.\n');
    } else {
      runtime.writeOut(`${renderTimeline(sessions)}\n`);
    }
    return 0;
  });
}

export async function runGet(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(argv, { ...READ_OPTIONS, json: { type: 'boolean' } }, runtime);
  if (parsed === null) return 2;
  const id = oneArgument(parsed.positionals, 'get', runtime);
  if (id === null) return 2;
  const json = parsed.values.json === true;

  return withDatabase(runtime, async (db, repoId, paths, identity) => {
    const selection = readWorkSelection(db, { repoId, contextKey: identity.worktreeKey,
      bindingId: typeof parsed.values.binding === 'string' ? parsed.values.binding : undefined });
    const memory = getMemory(db, id, memoryScope(db, { repoId, destination: 'injection',
      workId: selection.workId, history: parsed.values.history === true }));
    if (memory === null) return notFound(runtime, id, json);
    const sources = memorySources(db, memory.id);
    const visible = (parsed.values.history === true ? localHistoryOutput(db, [{ ...memory, sources }]) : await filterMemoryOutput(db, {
      repoId, repoRoot: identity.root, contextKey: identity.worktreeKey, workId: selection.workId,
      bindingId: selection.bindingId, home: paths.home,
    }, [{ ...memory, sources }]))[0];
    if (visible === undefined) return notFound(runtime, id, json);
    if (json) {
      runtime.writeOut(`${JSON.stringify(visible)}\n`);
    } else {
      runtime.writeOut(
        `Memory ${memory.id} is a ${memory.type} titled ${JSON.stringify(
          memory.title ?? '(untitled)',
        )}.\n` +
          `Its body is ${JSON.stringify(memory.body ?? '')}.\n` +
          `Its sensitivity is ${memory.sensitivity}.\n` +
          ('sources' in visible ? `Its sources are ${sourceText(visible.sources)}.\n` : ''),
      );
    }
    return 0;
  });
}

export async function runShare(argv: string[], overrides: Partial<MemoryCliRuntime> = {}): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(argv, { json: { type: 'boolean' }, binding: { type: 'string' } }, runtime);
  if (parsed === null) return 2;
  const [action, ...args] = parsed.positionals;
  if (!['status', 'approve', 'reject', 'adopt'].includes(action) || args.length !== (action === 'status' ? 0 : 1)
    || args.some((id) => id.trim() === '' || id.length > 128) || (parsed.values.binding !== undefined && action !== 'adopt')) {
    return invalid(runtime, 'Usage: oboete share status | approve <proposal-id> | reject <proposal-id> | adopt <memory-id> [--binding <binding-id>] [--json]');
  }
  return withDatabase(runtime, async (db, repoId, paths, identity) => {
    const selection = readWorkSelection(db, { repoId, contextKey: identity.worktreeKey,
      bindingId: typeof parsed.values.binding === 'string' ? parsed.values.binding : undefined });
    const location = { repoId, repoRoot: identity.root, contextKey: identity.worktreeKey, home: paths.home,
      bindingId: selection.bindingId, workId: selection.workId };
    if (action === 'status') {
      const status = await sharingStatus(db, location);
      runtime.writeOut(parsed.values.json ? `${JSON.stringify(status)}\n` : status.proposals.length === 0
        ? 'No sharing proposals are available in this repository.\n'
        : status.proposals.map((row) => `${row.id}  ${row.state}  ${JSON.stringify(row.candidate_title)}: ${JSON.stringify(row.candidate_body)}`).join('\n') + '\n');
      if (!parsed.values.json && status.hasMore) runtime.writeOut('More proposals are available. Review these to see the next ones.\n');
      return 0;
    }
    const result = action === 'adopt'
      ? await adoptKnowledge(db, location, args[0], runtime.now()) ? { id: args[0], audience: 'project' } : null
      : await decideSharing(db, location, { id: args[0], decision: action as 'approve' | 'reject', channel: 'cli', now: runtime.now() });
    if (result === null) {
      runtime.writeError('The memory or proposal is unavailable in this scope, or the decision is no longer current.\n');
      return 1;
    }
    runtime.writeOut(parsed.values.json ? `${JSON.stringify(result)}\n` : action === 'adopt'
      ? 'This knowledge is now available throughout the project.\n'
      : action === 'approve' ? 'This personal preference is now available across your projects.\n' : 'The sharing proposal was rejected.\n');
    return 0;
  });
}

async function changePin(
  argv: string[],
  pinned: boolean,
  overrides: Partial<MemoryCliRuntime>,
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(
    argv,
    pinned
      ? { ...READ_OPTIONS, order: { type: 'string' }, json: { type: 'boolean' } }
      : { ...READ_OPTIONS, json: { type: 'boolean' } },
    runtime,
  );
  if (parsed === null) return 2;
  const id = oneArgument(parsed.positionals, pinned ? 'pin' : 'unpin', runtime);
  if (id === null) return 2;
  const order =
    !pinned || parsed.values.order === undefined
      ? null
      : integerOption(parsed.values.order, '--order', 0, Number.MAX_SAFE_INTEGER, runtime);
  if (pinned && order === null && parsed.values.order !== undefined) return 2;
  const json = parsed.values.json === true;
  const pinnedAt = pinned ? runtime.now() : null;

  return withDatabase(runtime, (db, repoId, _paths, identity) => {
    const selection = readWorkSelection(db, { repoId, contextKey: identity.worktreeKey,
      bindingId: typeof parsed.values.binding === 'string' ? parsed.values.binding : undefined });
    const scope = memoryScope(db, { repoId, destination: 'injection', workId: selection.workId, history: parsed.values.history === true });
    if (!setPinned(db, { id, scope, pinnedAt, pinOrder: order })) {
      return notFound(runtime, id, json);
    }
    if (json) {
      runtime.writeOut(
        `${JSON.stringify({ id, action: pinned ? 'pinned' : 'unpinned', pinned_at: pinnedAt, pin_order: order })}\n`,
      );
    } else if (pinned) {
      const atOrder = order === null ? '' : ` at order ${order}`;
      runtime.writeOut(`Pinned memory ${id}${atOrder}.\n`);
    } else {
      runtime.writeOut(`Unpinned memory ${id}.\n`);
    }
    return 0;
  });
}

export async function runPin(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  return changePin(argv, true, overrides);
}

export async function runUnpin(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  return changePin(argv, false, overrides);
}

export async function runDelete(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(argv, { ...READ_OPTIONS, json: { type: 'boolean' } }, runtime);
  if (parsed === null) return 2;
  const id = oneArgument(parsed.positionals, 'delete', runtime);
  if (id === null) return 2;
  const json = parsed.values.json === true;
  const deletedAt = runtime.now();

  return withDatabase(runtime, (db, repoId, _paths, identity) => {
    const selection = readWorkSelection(db, { repoId, contextKey: identity.worktreeKey,
      bindingId: typeof parsed.values.binding === 'string' ? parsed.values.binding : undefined });
    const scope = memoryScope(db, { repoId, destination: 'injection', workId: selection.workId, history: parsed.values.history === true });
    if (!tombstone(db, { id, scope, deletedAt })) return notFound(runtime, id, json);
    if (json) runtime.writeOut(`${JSON.stringify({ id, action: 'deleted', deleted_at: deletedAt })}\n`);
    else runtime.writeOut(`Deleted memory ${id}.\n`);
    return 0;
  });
}
