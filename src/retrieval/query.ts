// Candidate generation over memories_fts / memories_fts_cjk (research.md R5, FR-025).
import type { DatabaseSync, SQLInputValue, SQLOutputValue } from 'node:sqlite';

import { buildMatch, segmentQuery, type QueryTerms } from './fts.js';

export type ScopeFilter = {
  where: string;
  params: unknown[];
};

export type Candidate = {
  id: string;
  title: string;
  body: string;
  pinned_at: number | null;
  created_at: number | null;
  scoreTrigram: number | null;
  scoreCjk: number | null;
  viaLike: boolean;
};

export type SearchInput = {
  text: string;
  scope: ScopeFilter;
  limit?: number;
};

export type SearchResult = {
  rows: Candidate[];
  usedLike: boolean;
  terms: QueryTerms;
};

const DEFAULT_LIMIT = 50;

function asText(value: SQLOutputValue | undefined): string {
  return typeof value === 'string' ? value : '';
}

function asNumber(value: SQLOutputValue | undefined): number | null {
  return typeof value === 'number' ? value : null;
}

function bindParams(values: unknown[]): SQLInputValue[] {
  return values as SQLInputValue[];
}

function likePattern(term: string): string {
  return `%${term.replaceAll('\\', '\\\\').replaceAll('%', '\\%').replaceAll('_', '\\_')}%`;
}

function candidateFrom(
  row: Record<string, SQLOutputValue>,
  scores: Pick<Candidate, 'scoreTrigram' | 'scoreCjk' | 'viaLike'>,
): Candidate {
  return {
    id: asText(row.id),
    title: asText(row.title),
    body: asText(row.body),
    pinned_at: asNumber(row.pinned_at),
    created_at: asNumber(row.created_at),
    scoreTrigram: scores.scoreTrigram,
    scoreCjk: scores.scoreCjk,
    viaLike: scores.viaLike,
  };
}

function mergeRow(
  byId: Map<string, Candidate>,
  row: Record<string, SQLOutputValue>,
  scores: Pick<Candidate, 'scoreTrigram' | 'scoreCjk' | 'viaLike'>,
): void {
  const next = candidateFrom(row, scores);
  const previous = byId.get(next.id);
  if (previous === undefined) {
    byId.set(next.id, next);
    return;
  }
  byId.set(next.id, {
    ...previous,
    scoreTrigram: next.scoreTrigram ?? previous.scoreTrigram,
    scoreCjk: next.scoreCjk ?? previous.scoreCjk,
    viaLike: false,
  });
}

function runFts(
  db: DatabaseSync,
  table: 'memories_fts' | 'memories_fts_cjk',
  match: string,
  scope: ScopeFilter,
  limit: number,
): Record<string, SQLOutputValue>[] {
  const sql = `SELECT m.id, m.title, m.body, m.pinned_at, m.created_at, bm25(${table}) AS s
FROM ${table} f JOIN memories m ON m.rowid = f.rowid
WHERE ${table} MATCH ? AND (${scope.where})
ORDER BY s LIMIT ?`;
  return db.prepare(sql).all(match, ...bindParams(scope.params), limit);
}

export function searchCandidates(db: DatabaseSync, input: SearchInput): SearchResult {
  const terms = segmentQuery(input.text);
  const limit = input.limit ?? DEFAULT_LIMIT;
  const byId = new Map<string, Candidate>();
  let usedLike = false;
  const trigramMatch = buildMatch(terms.trigram);
  const cjkMatch = buildMatch(terms.cjk);

  if (trigramMatch !== null) {
    for (const row of runFts(db, 'memories_fts', trigramMatch, input.scope, limit)) {
      mergeRow(byId, row, {
        scoreTrigram: asNumber(row.s),
        scoreCjk: null,
        viaLike: false,
      });
    }
  }

  if (cjkMatch !== null) {
    for (const row of runFts(db, 'memories_fts_cjk', cjkMatch, input.scope, limit)) {
      mergeRow(byId, row, {
        scoreTrigram: null,
        scoreCjk: asNumber(row.s),
        viaLike: false,
      });
    }
  }

  if (trigramMatch === null && cjkMatch === null && terms.like.length > 0) {
    usedLike = true;
    const patterns = terms.like.map(likePattern);
    const likeClause = patterns
      .map(() => `(m.title LIKE ? ESCAPE '\\' OR m.body LIKE ? ESCAPE '\\')`)
      .join(' AND ');
    const sql = `SELECT m.id, m.title, m.body, m.pinned_at, m.created_at
FROM memories m
WHERE ${likeClause} AND (${input.scope.where})
LIMIT ?`;
    const likeParams = patterns.flatMap((pattern) => [pattern, pattern]);
    const rows = db
      .prepare(sql)
      .all(...bindParams(likeParams), ...bindParams(input.scope.params), limit);
    for (const row of rows) {
      mergeRow(byId, row, { scoreTrigram: null, scoreCjk: null, viaLike: true });
    }
  }

  return { rows: [...byId.values()], usedLike, terms };
}
