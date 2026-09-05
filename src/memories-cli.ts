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

const SEARCH_DEFAULT_LIMIT = 10;
const MAX_LIMIT = 50;
const EMPTY_REASON = 'No memories matched this query in the current repository.';
const LEXICAL_NOTE = 'M1 search is lexical (word match). Semantic search arrives in M2.';

export type MemoryCliRuntime = {
  cwd: string;
  now(): number;
  writeOut(text: string): void;
  writeError(text: string): void;
};

type CommandOptions = Record<string, { type: 'string' | 'boolean' }>;
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

function runtimeWith(overrides: Partial<MemoryCliRuntime>): MemoryCliRuntime {
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

function parseCommand(
  argv: string[],
  options: CommandOptions,
  runtime: MemoryCliRuntime,
): ParsedCommand | null {
  try {
    return parseArgs({ args: argv, allowPositionals: true, strict: true, options });
  } catch (error) {
    invalid(runtime, error instanceof Error ? error.message : String(error));
    return null;
  }
}

function invalid(runtime: MemoryCliRuntime, message: string): 2 {
  runtime.writeError(`${message}\n`);
  return 2;
}

function oneArgument(
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

function withDatabase<T>(
  runtime: MemoryCliRuntime,
  fn: (db: DatabaseSync, repoId: string, paths: OboetePaths) => T,
): T {
  const identity = resolveRepoIdentity(runtime.cwd);
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
  try {
    return fn(opened.db, identity.id, paths);
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

function renderSearch(rows: SearchRow[]): string {
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
    { limit: { type: 'string' }, json: { type: 'boolean' } },
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

  return withDatabase(runtime, (db, repoId, paths) => {
    const scope = memoryScope(db, { repoId, destination: 'injection' });
    const found = searchCandidates(db, { text: query, scope });
    const ranked = rankCandidates(found.rows, {
      threshold: loadConfig(paths).injection.threshold,
      lambda: 0.5,
      limit,
    });
    const rows = ranked.included.flatMap((row): SearchRow[] => {
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

    if (parsed.values.json === true) {
      runtime.writeOut(
        `${JSON.stringify(
          rows.length === 0
            ? { memories: rows, reason: EMPTY_REASON, note: LEXICAL_NOTE }
            : { memories: rows },
        )}\n`,
      );
    } else if (rows.length === 0) {
      runtime.writeOut(`${EMPTY_REASON}\n${LEXICAL_NOTE}\n`);
    } else {
      runtime.writeOut(`${renderSearch(rows)}\n`);
    }
    return 0;
  });
}

export async function runTimeline(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(
    argv,
    { session: { type: 'string' }, json: { type: 'boolean' } },
    runtime,
  );
  if (parsed === null) return 2;
  if (parsed.positionals.length !== 0) return invalid(runtime, 'timeline accepts no arguments.');
  const sessionId = parsed.values.session;
  if (sessionId !== undefined && (typeof sessionId !== 'string' || sessionId.trim() === '')) {
    return invalid(runtime, '--session must not be empty.');
  }

  return withDatabase(runtime, (db, repoId) => {
    const sessions = timeline(db, repoId, {
      ...(typeof sessionId === 'string' ? { sessionId: sessionId.trim() } : {}),
      limit: MAX_LIMIT,
    });
    if (parsed.values.json === true) {
      runtime.writeOut(`${JSON.stringify({ sessions })}\n`);
    } else if (sessions.length === 0) {
      runtime.writeOut('No sessions were found in the current repository.\n');
    } else {
      runtime.writeOut(
        `${sessions
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
                        )}. Its sources are ${sourceText(memory.sources)}.`,
                    )
                    .join('\n');
            return `Session ${session.id} was recorded by ${session.agent} and is ${session.status}.\n${turns}\n${memories}`;
          })
          .join('\n')}\n`,
      );
    }
    return 0;
  });
}

export async function runGet(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(argv, { json: { type: 'boolean' } }, runtime);
  if (parsed === null) return 2;
  const id = oneArgument(parsed.positionals, 'get', runtime);
  if (id === null) return 2;
  const json = parsed.values.json === true;

  return withDatabase(runtime, (db, repoId) => {
    const memory = getMemory(db, id, memoryScope(db, { repoId, destination: 'injection' }));
    if (memory === null) return notFound(runtime, id, json);
    const sources = memorySources(db, memory.id);
    if (json) {
      runtime.writeOut(`${JSON.stringify({ ...memory, sources })}\n`);
    } else {
      runtime.writeOut(
        `Memory ${memory.id} is a ${memory.type} titled ${JSON.stringify(
          memory.title ?? '(untitled)',
        )}.\n` +
          `Its body is ${JSON.stringify(memory.body ?? '')}.\n` +
          `Its sensitivity is ${memory.sensitivity}.\n` +
          `Its sources are ${sourceText(sources)}.\n`,
      );
    }
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
      ? { order: { type: 'string' }, json: { type: 'boolean' } }
      : { json: { type: 'boolean' } },
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

  return withDatabase(runtime, (db, repoId) => {
    const scope = memoryScope(db, { repoId, destination: 'injection' });
    if (!setPinned(db, { id, scope, pinnedAt, pinOrder: order })) {
      return notFound(runtime, id, json);
    }
    if (json) {
      runtime.writeOut(
        `${JSON.stringify({ id, action: pinned ? 'pinned' : 'unpinned', pinned_at: pinnedAt, pin_order: order })}\n`,
      );
    } else {
      runtime.writeOut(
        pinned
          ? `Pinned memory ${id}${order === null ? '' : ` at order ${order}`}.\n`
          : `Unpinned memory ${id}.\n`,
      );
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
  const parsed = parseCommand(argv, { json: { type: 'boolean' } }, runtime);
  if (parsed === null) return 2;
  const id = oneArgument(parsed.positionals, 'delete', runtime);
  if (id === null) return 2;
  const json = parsed.values.json === true;
  const deletedAt = runtime.now();

  return withDatabase(runtime, (db, repoId) => {
    const scope = memoryScope(db, { repoId, destination: 'injection' });
    if (!tombstone(db, { id, scope, deletedAt })) return notFound(runtime, id, json);
    if (json) runtime.writeOut(`${JSON.stringify({ id, action: 'deleted', deleted_at: deletedAt })}\n`);
    else runtime.writeOut(`Deleted memory ${id}.\n`);
    return 0;
  });
}
